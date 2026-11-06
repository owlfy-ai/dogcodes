//! Desktop release metadata is separate from the CLI release stream.
use super::{AgentUpdate, newer};
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

fn resources(app: &Path) -> PathBuf {
    if app.extension().is_some_and(|e| e == "app") {
        app.join("Contents/Resources")
    } else {
        app.join("resources")
    }
}

fn package(app: &Path) -> Option<Value> {
    let mut file = File::open(resources(app).join("app.asar")).ok()?;
    let mut header = [0_u8; 16];
    file.read_exact(&mut header).ok()?;
    let header_size = u32::from_le_bytes(header[4..8].try_into().ok()?);
    let json_size = u32::from_le_bytes(header[12..16].try_into().ok()?);
    if json_size > 8 * 1024 * 1024 || header_size < json_size.checked_add(8)? {
        return None;
    }
    let mut bytes = vec![0; usize::try_from(json_size).ok()?];
    file.read_exact(&mut bytes).ok()?;
    let index: Value = serde_json::from_slice(&bytes).ok()?;
    let entry = &index["files"]["package.json"];
    if entry["unpacked"] == true || !entry["link"].is_null() {
        return None;
    }
    let size = entry["size"].as_u64()?;
    if size > 65_536 {
        return None;
    }
    let offset = entry["offset"].as_str()?.parse::<u64>().ok()?;
    let start = 8_u64
        .checked_add(u64::from(header_size))?
        .checked_add(offset)?;
    if start.checked_add(size)? > file.metadata().ok()?.len() {
        return None;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    bytes.resize(usize::try_from(size).ok()?, 0);
    file.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn version(app: &Path) -> Option<String> {
    semver::Version::parse(package(app)?["version"].as_str()?)
        .ok()
        .map(|v| v.to_string())
}

pub fn opencode_app(executable: &Path) -> Option<PathBuf> {
    if executable
        .file_name()
        .is_some_and(|n| n == "opencode-cli.exe")
        && let Some(resources) = executable.parent()
        && resources.file_name().is_some_and(|n| n == "resources")
        && let Some(app) = resources.parent()
        && app.join("OpenCode.exe").is_file()
    {
        return Some(app.into());
    }
    // Desktop-downloaded engines live outside the app bundle. Do not compare
    // them with npm even if the desktop was subsequently moved/uninstalled.
    let filename = executable.file_name()?.to_str()?;
    if !matches!(filename, "opencode-cli" | "opencode-cli.exe") {
        return None;
    }
    let version_dir = executable.parent()?;
    let name = version_dir.file_name()?.to_str()?;
    let name = name
        .strip_suffix("-x64")
        .or_else(|| name.strip_suffix("-arm64"))
        .unwrap_or(name);
    semver::Version::parse(name).ok()?;
    let cache = version_dir.parent()?;
    if !cache.ends_with("ai.opencode.desktop/cli") {
        return None;
    }
    let context = agent_registry::DiscoveryContext::from_current_process();
    let Some(home) = context.home_dir.as_deref() else {
        return Some(cache.into());
    };
    let candidates = if executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        vec![
            crate::user_data_dir(home, "LOCALAPPDATA", "AppData/Local")
                .join("Programs/ai.opencode.desktop"),
        ]
    } else {
        vec![
            home.join("Applications/OpenCode.app"),
            PathBuf::from("/Applications/OpenCode.app"),
        ]
    };
    Some(
        candidates
            .into_iter()
            .find(|app| package(app).is_some_and(|p| p["name"] == "@opencode/desktop"))
            .unwrap_or_else(|| cache.into()),
    )
}

pub fn official_opencode_feed(app: &Path) -> bool {
    if !package(app).is_some_and(|p| p["name"] == "@opencode/desktop") {
        return false;
    }
    let path = resources(app).join("app-update.yml");
    if !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 16_384) {
        return false;
    }
    std::fs::read_to_string(path).is_ok_and(|config| {
        config
            .lines()
            .any(|line| line.trim() == "provider: generic")
            && config.lines().any(|line| {
                line.trim() == "url: https://opencode.ai/update/api/latest/desktop/opencode/"
            })
            && config.lines().any(|line| line.trim() == "channel: latest")
    })
}

pub fn check_opencode(mut out: AgentUpdate, body: &str) -> Result<AgentUpdate, String> {
    let latest = body
        .lines()
        .find_map(|line| line.strip_prefix("version:"))
        .map(|s| s.trim().trim_matches(['\'', '"']))
        .filter(|s| semver::Version::parse(s).is_ok())
        .ok_or("OpenCode 更新源缺少应用版本")?;
    let current = out
        .desktop_version
        .clone()
        .ok_or("无法读取 OpenCode 应用版本")?;
    let available = newer(&current, latest)?;
    out.status = "managed".into();
    out.source = "桌面应用".into();
    out.current_version = Some(current);
    out.latest_version = Some(latest.into());
    out.can_upgrade = false;
    // The shared UI renders the actual version and opens the app only if newer.
    out.message = if available {
        None
    } else {
        Some("桌面应用已是最新版本".into())
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{Plan, base, desktop_update_available, plan, tests::installation_for};

    fn fixture(app: &Path, v: &str) {
        let resources = resources(app);
        std::fs::create_dir_all(&resources).unwrap();
        let package = serde_json::json!({"name":"@opencode/desktop", "version":v}).to_string();
        let json =
            serde_json::json!({"files":{"package.json":{"offset":"0","size":package.len()}}})
                .to_string();
        let len = u32::try_from(json.len()).unwrap();
        let mut bytes = Vec::new();
        for size in [4, len + 8, len + 4, len] {
            bytes.extend_from_slice(&size.to_le_bytes());
        }
        bytes.extend_from_slice(json.as_bytes());
        bytes.extend_from_slice(package.as_bytes());
        std::fs::write(resources.join("app.asar"), bytes).unwrap();
        std::fs::write(resources.join("app-update.yml"), "provider: generic\nurl: https://opencode.ai/update/api/latest/desktop/opencode/\nchannel: latest\n").unwrap();
    }

    #[test]
    fn opencode_desktop_feed_never_compares_against_the_cli_version() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("desktop");
        fixture(&app, "2.2.5");
        std::fs::write(app.join("OpenCode.exe"), "desktop").unwrap();
        let cli = app.join("resources/opencode-cli.exe");
        std::fs::write(&cli, "CLI").unwrap();
        let i = installation_for("opencode", &cli, "2.0.18");
        assert!(matches!(plan(&i), Plan::Host(_)));
        assert!(official_opencode_feed(&app));
        assert_eq!(base(&i).desktop_version.as_deref(), Some("2.2.5"));
        let current = check_opencode(base(&i), "version: 2.2.5\n").unwrap();
        assert_eq!(current.current_version.as_deref(), Some("2.2.5"));
        assert!(!desktop_update_available(&current));
        let newer = check_opencode(base(&i), "version: '2.2.6'\n").unwrap();
        assert!(desktop_update_available(&newer));
        assert!(!newer.can_upgrade);
        assert!(check_opencode(base(&i), "files:\n  - version: 3.0.0\n").is_err());
        std::fs::write(
            app.join("resources/app-update.yml"),
            "url: https://example.com/\n",
        )
        .unwrap();
        assert!(!official_opencode_feed(&app));
    }

    #[test]
    fn macos_opencode_uses_the_owning_app_version() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("OpenCode.app");
        fixture(&app, "2.2.5");
        let cli = app.join("Contents/Resources/opencode-cli");
        std::fs::write(&cli, "CLI").unwrap();
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleShortVersionString".into(),
            plist::Value::String("2.2.5".into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(app.join("Contents/Info.plist"))
            .unwrap();
        let i = installation_for("opencode", &cli, "2.0.18");
        assert!(matches!(plan(&i), Plan::Host(_)));
        assert!(official_opencode_feed(&app));
        let checked = check_opencode(base(&i), "version: 2.2.6\n").unwrap();
        assert_eq!(checked.current_version.as_deref(), Some("2.2.5"));
        assert!(desktop_update_available(&checked));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "read-only checks of local desktop-bundled CLIs and official release feeds"]
    async fn live_windows_desktop_update_routes() {
        let programs = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Programs");
        for (provider, relative) in [
            ("opencode", "ai.opencode.desktop/resources/opencode-cli.exe"),
            (
                "deepseek-harness",
                "DeepSeek Harness/resources/runtime/cli/bin/dsh.cmd",
            ),
        ] {
            let executable = programs.join(relative);
            assert!(executable.is_file());
            let actual = super::super::command(super::super::spec(
                &executable,
                vec!["--version".into()],
                false,
            ))
            .await
            .unwrap();
            let v = super::super::version(&actual).unwrap().to_string();
            let i = installation_for(provider, &executable, &v);
            let plan = plan(&i);
            if provider == "opencode" {
                assert!(matches!(plan, Plan::Host(_)));
            } else {
                assert!(matches!(plan, Plan::StandaloneCli { .. }));
            }
            let checked = super::super::inspect(&i, &plan).await.unwrap();
            if provider == "opencode" {
                assert!(checked.desktop_version.is_some());
                assert!(checked.latest_version.is_some());
                assert_eq!(checked.status, "managed");
            }
            println!(
                "{provider}: CLI={v}, desktop={:?}, latest={:?}, status={}, CLI upgrade={}",
                checked.desktop_version,
                checked.latest_version,
                checked.status,
                checked.can_upgrade
            );
        }
    }

    #[tokio::test]
    async fn orphaned_desktop_engines_are_not_reported_as_current_npm_clis() {
        let dir = tempfile::tempdir().unwrap();
        for binary in ["opencode-cli", "opencode-cli.exe"] {
            let cli = dir
                .path()
                .join("ai.opencode.desktop/cli/2.0.18-x64")
                .join(binary);
            std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
            std::fs::write(&cli, "CLI").unwrap();
            assert!(opencode_app(&cli).is_some());
            let cache = dir.path().join("missing-desktop");
            let i = installation_for("opencode", &cli, "2.0.18");
            let out = super::super::inspect(&i, &Plan::Host(cache)).await.unwrap();
            assert_eq!(out.status, "managed");
            assert!(out.latest_version.is_none());
            assert!(!desktop_update_available(&out));
        }
        assert!(opencode_app(&dir.path().join("custom/bin/opencode-cli")).is_none());
    }
}
