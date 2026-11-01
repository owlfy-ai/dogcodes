//! Standalone CLI upgrades must not overwrite a desktop application's bundled engine.
use super::{InstallationSummary, Plan, command, package, verified_version, version};
use agent_registry::{DiscoveryContext, Platform};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn desktop_cli(provider: &str, executable: &Path) -> bool {
    match provider {
        "codex" => executable
            .ancestors()
            .any(|p| p.ends_with("OpenAI/Codex/bin")),
        "claude-code" => executable
            .ancestors()
            .any(|p| p.ends_with("Claude/claude-code")),
        "deepseek-harness" => {
            let windows = executable.ends_with("resources/runtime/cli/bin/dsh.cmd")
                && executable
                    .ancestors()
                    .nth(5)
                    .is_some_and(|root| root.join("DeepSeek Harness.exe").is_file());
            let macos = executable.ends_with("Contents/Resources/runtime/cli/bin/dsh")
                && executable
                    .ancestors()
                    .nth(5)
                    .is_some_and(|contents| contents.join("MacOS/DeepSeek Harness").is_file());
            windows || macos
        }
        _ => false,
    }
}

pub(crate) fn prefix(context: &DiscoveryContext, provider: &str) -> Option<PathBuf> {
    if !matches!(provider, "codex" | "claude-code" | "deepseek-harness") {
        return None;
    }
    let home = context.home_dir.as_deref()?;
    let root = if context.platform == Platform::Windows {
        crate::terminal_installation::user_data_dir(home, "LOCALAPPDATA", "AppData/Local")
            .join("dogcodes/cli")
    } else {
        home.join(".local/share/dogcodes/cli")
    };
    Some(root.join(provider))
}

pub(super) fn launcher(prefix: &Path, provider: &str) -> PathBuf {
    let name = match provider {
        "codex" => "codex",
        "claude-code" => "claude",
        "deepseek-harness" => "dsh",
        _ => unreachable!("unsupported managed CLI provider"),
    };
    if cfg!(windows) {
        prefix.join(format!("{name}.cmd"))
    } else {
        prefix.join("bin").join(name)
    }
}

fn package_info(prefix: &Path, provider: &str) -> Option<Value> {
    let modules = if cfg!(windows) {
        "node_modules"
    } else {
        "lib/node_modules"
    };
    let path = prefix
        .join(modules)
        .join(package(provider)?)
        .join("package.json");
    if std::fs::metadata(&path).ok()?.len() > 65_536 {
        return None;
    }
    let info: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (info["name"].as_str() == package(provider)
        && info["version"].as_str().and_then(version).is_some())
    .then_some(info)
}

pub(crate) fn candidate(context: &DiscoveryContext, provider: &str) -> Option<PathBuf> {
    let prefix = prefix(context, provider)?;
    registered_launcher(&prefix, provider)
}

fn registered_launcher(prefix: &Path, provider: &str) -> Option<PathBuf> {
    let marker_path = prefix.join("managed-cli.json");
    if std::fs::metadata(&marker_path).ok()?.len() > 16_384 {
        return None;
    }
    let marker: Value = serde_json::from_slice(&std::fs::read(marker_path).ok()?).ok()?;
    if marker["kind"] != "dogcodes-managed-cli"
        || marker["schemaVersion"] != 1
        || marker["provider"] != provider
        || marker["package"].as_str() != package(provider)
    {
        return None;
    }
    package_info(prefix, provider)?;
    let launcher = launcher(prefix, provider);
    launcher.is_file().then_some(launcher)
}

pub(super) fn version_executable(i: &InstallationSummary, prefix: &Path) -> PathBuf {
    registered_launcher(prefix, &i.provider_id).unwrap_or_else(|| i.executable.clone().into())
}

pub(super) fn existing_prefix(i: &InstallationSummary) -> Option<PathBuf> {
    let context = DiscoveryContext::from_current_process();
    let selected = candidate(&context, &i.provider_id)?.canonicalize().ok()?;
    (Path::new(&i.executable).canonicalize().ok()? == selected)
        .then(|| prefix(&context, &i.provider_id))
        .flatten()
}

fn register(prefix: &Path, provider: &str, target: &str, actual: &str) -> Result<(), String> {
    let info = package_info(prefix, provider).ok_or("CLI 安装包校验失败")?;
    if !verified_version(provider, actual, target)
        || version(actual) != info["version"].as_str().and_then(version)
    {
        return Err("CLI 升级后版本校验失败，尚未切换安装".into());
    }
    let mut marker =
        tempfile::NamedTempFile::new_in(prefix).map_err(|_| "无法保存 CLI 安装记录")?;
    marker.write_all(json!({"kind":"dogcodes-managed-cli","schemaVersion":1,"provider":provider,"package":package(provider),"version":version(actual).map(|v|v.to_string())}).to_string().as_bytes()).map_err(|_| "无法保存 CLI 安装记录")?;
    marker
        .persist(prefix.join("managed-cli.json"))
        .map_err(|_| "无法保存 CLI 安装记录")?;
    Ok(())
}

pub(super) async fn activate(
    i: &InstallationSummary,
    plan: &Plan,
    target: &str,
) -> Result<(), String> {
    let Plan::StandaloneCli { prefix, .. } = plan else {
        return Ok(());
    };
    let mut fresh = i.clone();
    fresh.executable = launcher(prefix, &i.provider_id)
        .to_string_lossy()
        .into_owned();
    let actual = command(super::new_agents::version_spec(&fresh, plan).await?).await?;
    register(prefix, &i.provider_id, target, &actual)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn desktop_dsh_launchers_use_an_independent_cli_on_both_platforms() {
        let dir = tempfile::tempdir().unwrap();
        for (relative, app_binary) in [
            (
                "DeepSeek Harness/resources/runtime/cli/bin/dsh.cmd",
                "DeepSeek Harness/DeepSeek Harness.exe",
            ),
            (
                "DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
                "DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
            ),
        ] {
            let executable = dir.path().join(relative);
            let binary = dir.path().join(app_binary);
            std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
            std::fs::write(&executable, "desktop launcher").unwrap();
            assert!(!desktop_cli("deepseek-harness", &executable));
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(&binary, "desktop").unwrap();
            assert!(desktop_cli("deepseek-harness", &executable));
            assert!(matches!(
                super::super::plan(&super::super::tests::installation_for(
                    "deepseek-harness",
                    &executable,
                    "0.1.0"
                )),
                Plan::StandaloneCli {
                    package: "@deepseek-ai/dsh",
                    ..
                }
            ));
        }
        let downloaded = dir
            .path()
            .join("Library/Application Support/Claude/claude-code/2.1.0/hash/claude");
        std::fs::create_dir_all(downloaded.parent().unwrap()).unwrap();
        std::fs::write(&downloaded, "CLI").unwrap();
        assert!(matches!(
            super::super::plan(&super::super::tests::installation_for(
                "claude-code",
                &downloaded,
                "2.1.0"
            )),
            Plan::StandaloneCli { .. }
        ));
    }

    pub(crate) fn fixture(context: &DiscoveryContext, provider: &str, v: &str) -> PathBuf {
        let root = prefix(context, provider).unwrap();
        let modules = if cfg!(windows) {
            "node_modules"
        } else {
            "lib/node_modules"
        };
        let package = root.join(modules).join(package(provider).unwrap());
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            json!({"name":super::package(provider),"version":v}).to_string(),
        )
        .unwrap();
        let launcher = launcher(&root, provider);
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::write(&launcher, "standalone CLI").unwrap();
        root
    }

    pub(crate) fn verified_fixture(context: &DiscoveryContext, provider: &str) -> PathBuf {
        let root = fixture(context, provider, "1.1.0");
        register(&root, provider, "1.1.0", "1.1.0").unwrap();
        launcher(&root, provider)
    }

    #[test]
    fn only_verified_cli_installs_are_selected_and_desktop_payloads_stay_intact() {
        let home = tempfile::tempdir().unwrap();
        let context = DiscoveryContext {
            platform: Platform::current(),
            home_dir: Some(home.path().into()),
            path_entries: vec![],
        };
        for provider in ["codex", "claude-code", "deepseek-harness"] {
            let original = home.path().join(format!("desktop-{provider}.exe"));
            std::fs::write(&original, "desktop-owned CLI").unwrap();
            let root = fixture(&context, provider, "1.1.0");
            let installed = super::super::tests::installation_for(provider, &original, "1.0.0");
            assert_eq!(version_executable(&installed, &root), original);
            assert!(candidate(&context, provider).is_none());
            assert!(register(&root, provider, "1.1.0", "1.0.0").is_err());
            assert!(candidate(&context, provider).is_none());
            register(&root, provider, "1.1.0", "CLI 1.1.0").unwrap();
            assert_eq!(
                version_executable(&installed, &root),
                launcher(&root, provider)
            );
            assert_eq!(
                candidate(&context, provider),
                Some(launcher(&root, provider))
            );
            let marker = root.join("managed-cli.json");
            std::fs::write(marker, r#"{"kind":"foreign"}"#).unwrap();
            assert!(candidate(&context, provider).is_none());
            assert_eq!(
                std::fs::read_to_string(original).unwrap(),
                "desktop-owned CLI"
            );
        }
    }
}
