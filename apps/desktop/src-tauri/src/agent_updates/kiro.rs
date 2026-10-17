//! Kiro CLI app updates through its own updater and release index.
use super::{InstallationSummary, command, json, spec};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn metadata(app: &Path) -> Option<Value> {
    let path = app.join("Contents/Resources/manifest.json");
    if std::fs::metadata(&path).ok()?.len() > 65_536 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(super) fn bundle(executable: &Path) -> Option<PathBuf> {
    let executable = executable.canonicalize().ok()?;
    let app = executable.ancestors().nth(3)?;
    if app.extension()? != "app" || executable != app.join("Contents/MacOS/kiro-cli") {
        return None;
    }
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let info = info.as_dictionary()?;
    if info.get("CFBundleIdentifier")?.as_string()? != "com.amazon.codewhisperer"
        || info.get("CFBundleExecutable")?.as_string()? != "kiro_cli_desktop"
    {
        return None;
    }
    let manifest = metadata(app)?;
    if manifest["managed_by"] != "dmg" || manifest["kind"] != "dmg" || manifest["variant"] != "full"
    {
        return None;
    }
    Some(app.into())
}

fn matches_package(package: &Value) -> bool {
    package["targetTriple"] == "universal-apple-darwin"
        && package["variant"] == "full"
        && package["fileType"] == "dmg"
}

fn latest_from_index(index: &Value) -> Result<String, String> {
    if !index["supported"]
        .as_array()
        .is_some_and(|items| items.iter().any(matches_package))
    {
        return Err("Kiro 更新源不支持当前 CLI 安装平台".into());
    }
    let releases = index["versions"]
        .as_array()
        .ok_or("Kiro 更新源缺少版本列表")?;
    let mut latest = None;
    for release in releases {
        if !release["packages"]
            .as_array()
            .is_some_and(|items| items.iter().any(matches_package))
        {
            continue;
        }
        let version = release["version"]
            .as_str()
            .and_then(|s| semver::Version::parse(s).ok())
            .ok_or("Kiro 更新源包含无效版本")?;
        if latest.as_ref().is_none_or(|previous| &version > previous) {
            latest = Some(version);
        }
    }
    latest
        .map(|v| v.to_string())
        .ok_or("Kiro 当前平台没有可用发布版本".into())
}

pub(super) async fn latest(i: &InstallationSummary) -> Result<String, String> {
    let executable = Path::new(&i.executable);
    let app = bundle(executable).ok_or("Kiro CLI 安装已变化，请重新检测")?;
    let manifest = metadata(&app).ok_or("无法读取 Kiro CLI 发布信息")?;
    // Only the verified stable channel is enabled. Never advertise a stable
    // release as the target for an unverified beta/custom-channel updater.
    if manifest["default_channel"] != "stable" {
        return Err("此 Kiro 发布频道暂不支持自动更新，请使用原生更新器".into());
    }
    let beta = json(
        &command(spec(
            executable,
            vec![
                "settings".into(),
                "app.beta".into(),
                "--format".into(),
                "json".into(),
            ],
            false,
        ))
        .await?,
    )?;
    if beta != Value::Null && beta != false {
        return Err("Kiro 测试频道暂不支持自动更新，请使用原生更新器".into());
    }
    // Native get-index honours the installed CLI's release endpoint and returns
    // JSON without downloading/installing an update or starting a model turn.
    let mut index_request = spec(
        executable,
        vec!["debug".into(), "get-index".into(), "stable".into()],
        false,
    );
    // The native index includes release history (~1.2 MiB at 2.24.1), so the
    // generic 64 KiB command cap would truncate valid JSON. Keep a finite cap.
    index_request.stdout_limit = 4 * 1024 * 1024;
    let index = command(index_request).await?;
    latest_from_index(&json(&index)?)
}

pub(super) async fn verify_updated(executable: &Path, target: &str) -> Result<(), String> {
    wait_for_version(executable, target, std::time::Duration::from_secs(30)).await
}

async fn wait_for_version(
    executable: &Path,
    target: &str,
    limit: std::time::Duration,
) -> Result<(), String> {
    tokio::time::timeout(limit, async {
        loop {
            let mut probe = spec(executable, vec!["--version".into()], false);
            probe.timeout = std::time::Duration::from_secs(3);
            if let Ok(actual) = command(probe).await
                && super::verified_version("kiro", &actual, target)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| "Kiro 更新器已退出，但尚未确认目标版本生效，请重新检测后重试".into())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent_updates::{Plan, inspect, install_spec, plan, tests::installation_for};
    use serde_json::json;

    pub(crate) fn fixture_app(root: &Path) -> PathBuf {
        let app = root.join("Kiro CLI.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String("com.amazon.codewhisperer".into()),
        );
        info.insert(
            "CFBundleExecutable".into(),
            plist::Value::String("kiro_cli_desktop".into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(app.join("Contents/Info.plist"))
            .unwrap();
        std::fs::write(
            app.join("Contents/Resources/manifest.json"),
            r#"{"managed_by":"dmg","kind":"dmg","variant":"full","default_channel":"stable"}"#,
        )
        .unwrap();
        let executable = app.join("Contents/MacOS/kiro-cli");
        std::fs::write(&executable, "").unwrap();
        executable
    }

    fn package() -> Value {
        json!({"targetTriple":"universal-apple-darwin","variant":"full","fileType":"dmg"})
    }

    #[test]
    fn cli_bundle_uses_native_updater_but_other_hosts_stay_managed() {
        let dir = tempfile::tempdir().unwrap();
        let executable = fixture_app(dir.path());
        let i = installation_for("kiro", &executable, "2.24.1");
        let p = plan(&i);
        assert!(matches!(p, Plan::Native(_)));
        let command = install_spec(&i, &p, "2.25.0").unwrap();
        assert_eq!(command.executable, executable);
        assert_eq!(command.args, ["update", "--non-interactive"]);
        std::fs::write(
            executable
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("Resources/manifest.json"),
            "{}",
        )
        .unwrap();
        assert!(matches!(plan(&i), Plan::Host(_)));
    }

    #[test]
    fn index_matches_platform_and_orders_versions_semantically() {
        let index = json!({"supported":[package()],"versions":[
            {"version":"2.9.0","packages":[package()]},
            {"version":"9.0.0","packages":[{"targetTriple":"x86_64-unknown-linux-gnu","variant":"full","fileType":"deb"}]},
            {"version":"2.25.0","packages":[package()]},
            {"version":"2.24.1","packages":[package()]}
        ]});
        assert_eq!(latest_from_index(&index).unwrap(), "2.25.0");
        for invalid in [
            json!({}),
            json!({"supported":[package()],"versions":[]}),
            json!({"supported":[package()],"versions":[{"version":"bad-version","packages":[package()]}]}),
        ] {
            assert!(latest_from_index(&invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn check_is_read_only_and_update_uses_noninteractive_native_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = fixture_app(dir.path());
        let payload = json!({"supported":[package()],"versions":[{"version":"2.25.0","packages":[package()]}], "historyPadding":"x".repeat(70 * 1024)});
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in\n'settings app.beta --format json') echo null;;\n'debug get-index stable') printf '%s\\n' '{payload}';;\n'update --non-interactive') printf '2.25.0' > \"$0.updated\";;\n*) exit 9;;\nesac\n"
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let i = installation_for("kiro", &executable, "2.24.1");
        let p = plan(&i);
        let checked = inspect(&i, &p).await.unwrap();
        assert_eq!(checked.status, "available");
        let marker = executable.with_file_name("kiro-cli.updated");
        assert!(!marker.exists());
        command(install_spec(&i, &p, "2.25.0").unwrap())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "2.25.0");
        std::fs::write(&executable, "#!/bin/sh\necho true\n").unwrap();
        assert!(inspect(&i, &p).await.unwrap_err().contains("测试频道"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verification_waits_for_bundle_replacement_and_rejects_unchanged_version() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = fixture_app(dir.path());
        std::fs::write(&executable, "#!/bin/sh\necho 'kiro-cli 2.24.1'\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            wait_for_version(&executable, "2.25.0", std::time::Duration::from_millis(100))
                .await
                .is_err()
        );
        let replacement = executable.clone();
        let replace = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            std::fs::write(replacement, "#!/bin/sh\necho 'kiro-cli 2.25.0'\n").unwrap();
        });
        wait_for_version(&executable, "2.25.0", std::time::Duration::from_secs(3))
            .await
            .unwrap();
        replace.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "read-only check of local Kiro CLI and its native release index"]
    async fn real_kiro_update_check() {
        let executable =
            PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/bin/kiro-cli");
        let current = command(spec(&executable, vec!["--version".into()], false))
            .await
            .unwrap();
        let i = installation_for(
            "kiro",
            &executable,
            &super::super::version(&current).unwrap().to_string(),
        );
        let p = plan(&i);
        assert!(matches!(p, Plan::Native(_)));
        let checked = inspect(&i, &p).await.unwrap();
        assert!(checked.latest_version.is_some());
        println!(
            "Kiro: current={:?}, latest={:?}, status={}",
            checked.current_version, checked.latest_version, checked.status
        );
    }
}
