//! Update only the selected installation; do not migrate cached CLIs to a global prefix.
use super::{InstallationSummary, Plan, ProcessSpec, spec, version};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn manifest(path: &Path) -> Option<Value> {
    if std::fs::metadata(path).ok()?.len() > 65_536 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(super) fn pi_managed_root(executable: &Path) -> Option<PathBuf> {
    for parent in executable.ancestors().skip(1).take(10) {
        let root = if parent.file_name().is_some_and(|n| n == "install") {
            parent.to_path_buf()
        } else {
            parent.join("install")
        };
        let Some(marker) = manifest(&root.join("managed-install.json")) else {
            continue;
        };
        if marker["kind"] != "pi-managed-install"
            || marker["schemaVersion"] != 1
            || marker["layout"] != "releases-v1"
        {
            continue;
        }
        let current = std::fs::read_to_string(root.join("current-version")).ok()?;
        let current = semver::Version::parse(current.trim()).ok()?.to_string();
        let release = root.join("releases").join(current);
        let package =
            manifest(&release.join("node_modules/@earendil-works/pi-coding-agent/package.json"))?;
        if package["name"] != "@earendil-works/pi-coding-agent" {
            return None;
        }
        let launcher = root.parent()?.join("bin/pi").canonicalize().ok()?;
        if executable == launcher || executable.starts_with(release.canonicalize().ok()?) {
            return Some(root);
        }
    }
    None
}

pub(super) fn is_dsh_cache(prefix: &Path, package: &Path) -> bool {
    prefix
        .parent()
        .is_some_and(|p| p.file_name().is_some_and(|n| n == "_npx"))
        && prefix
            .join("node_modules/@deepseek-ai/dsh")
            .canonicalize()
            .is_ok_and(|p| p == package)
        && manifest(&prefix.join("package.json"))
            .is_some_and(|v| v["dependencies"]["@deepseek-ai/dsh"].is_string())
}

pub(super) fn is_opencode_native(executable: &Path) -> bool {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| home.join(".opencode/bin/opencode").canonicalize().ok())
        .is_some_and(|native| {
            native == executable
                && executable
                    .parent()
                    .is_some_and(|p| p.ends_with(".opencode/bin"))
        })
}

fn with_node_path(mut spec: ProcessSpec, node: &Path, npm: &Path) -> Result<ProcessSpec, String> {
    let bin = node.parent().ok_or("Node 安装目录无效")?;
    // Native Pi invokes npm itself; make both executables visible from GUI launches.
    let npm_bin = npm.parent().ok_or("npm 安装目录无效")?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        [bin.to_path_buf(), npm_bin.to_path_buf()]
            .into_iter()
            .chain(std::env::split_paths(&inherited)),
    )
    .map_err(|_| "Node PATH 无效")?;
    spec.env_overrides.insert("PATH".into(), path);
    Ok(spec)
}

fn local_npm_spec(
    prefix: &Path,
    package: &str,
    target: &str,
    node: &Path,
    npm: &Path,
) -> Result<ProcessSpec, String> {
    let target = version(target).ok_or("更新版本无效")?;
    let mut command = spec(
        node,
        vec![
            npm.to_string_lossy().into_owned(),
            "install".into(),
            "--prefix".into(),
            prefix.to_string_lossy().into_owned(),
            "--save-exact".into(),
            format!("{package}@{target}"),
            "--no-audit".into(),
            "--no-fund".into(),
        ],
        true,
    );
    command.cwd = Some(prefix.to_path_buf());
    with_node_path(command, node, npm)
}

pub(super) async fn version_spec(
    i: &InstallationSummary,
    plan: &Plan,
) -> Result<ProcessSpec, String> {
    let command = spec(Path::new(&i.executable), vec!["--version".into()], false);
    if matches!(plan, Plan::NpmLocal { .. })
        || (i.provider_id == "pi" && matches!(plan, Plan::Native(_)))
    {
        let (node, npm) = crate::terminal_installation::npm_runtime()
            .await
            .ok_or("无法定位终端使用的 Node/npm，请安装 npm 后重试")?;
        return with_node_path(command, &node, &npm);
    }
    Ok(command)
}

pub(super) async fn install_spec(
    i: &InstallationSummary,
    plan: &Plan,
    latest: &str,
) -> Result<ProcessSpec, String> {
    if let Plan::NpmLocal { prefix, package } = plan {
        let (node, npm) = crate::terminal_installation::npm_runtime()
            .await
            .ok_or("无法定位终端使用的 Node/npm，请安装 npm 后重试")?;
        return local_npm_spec(prefix, package, latest, &node, &npm);
    }
    let mut command = super::install_spec(i, plan, latest)?;
    if i.provider_id == "pi" && matches!(plan, Plan::Native(_)) {
        let root = Path::new(&i.executable)
            .canonicalize()
            .ok()
            .and_then(|p| pi_managed_root(&p))
            .ok_or("Pi 托管安装已变化，请重新检测")?;
        command.executable = root.parent().ok_or("Pi 安装目录无效")?.join("bin/pi");
        let (node, npm) = crate::terminal_installation::npm_runtime()
            .await
            .ok_or("Pi 更新需要 Node/npm，请确认终端可用")?;
        command = with_node_path(command, &node, &npm)?;
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_updates::{inspect, plan, tests::installation_for};
    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn dsh_cache_is_updated_in_place_without_global_install_or_shell_interpolation() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("_npx/cache with spaces");
        let package = prefix.join("node_modules/@deepseek-ai/dsh");
        let executable = package.join("lib/bin.js");
        write(&executable, "");
        write(
            &package.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh"}"#,
        );
        write(
            &prefix.join("package.json"),
            r#"{"dependencies":{"@deepseek-ai/dsh":"^0.1.7-rc.2"}}"#,
        );
        let installation = installation_for("deepseek-harness", &executable, "0.1.7-rc.2");
        assert!(matches!(plan(&installation), Plan::NpmLocal { .. }));
        let s = local_npm_spec(
            &prefix,
            "@deepseek-ai/dsh",
            "0.2.0",
            Path::new("/tools/bin/node"),
            Path::new("/tools/lib/node_modules/npm/bin/npm-cli.js"),
        )
        .unwrap();
        assert_eq!(s.cwd.as_deref(), Some(prefix.as_path()));
        assert!(s.args.contains(&prefix.to_string_lossy().into_owned()));
        assert!(s.args.contains(&"@deepseek-ai/dsh@0.2.0".into()));
        assert!(!s.args.contains(&"--global".into()));
        assert!(
            local_npm_spec(
                &prefix,
                "@deepseek-ai/dsh",
                "0.2.0;evil",
                Path::new("/node"),
                Path::new("/npm")
            )
            .is_err()
        );
        write(&prefix.join("package.json"), "{}");
        assert!(matches!(plan(&installation), Plan::Unsupported));
    }

    #[test]
    fn managed_pi_validates_marker_and_current_package_and_scopes_version_directory() {
        let dir = tempfile::tempdir().unwrap();
        let agent = dir.path().join("pi agent");
        let root = agent.join("install");
        let launcher = agent.join("bin/pi");
        let release = root.join("releases/0.87.1/node_modules/@earendil-works/pi-coding-agent");
        write(&launcher, "launcher");
        write(
            &root.join("managed-install.json"),
            r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
        );
        write(&root.join("current-version"), "0.87.1\n");
        write(
            &release.join("package.json"),
            r#"{"name":"@earendil-works/pi-coding-agent"}"#,
        );
        write(&release.join("dist/cli.js"), "");
        let launcher = launcher.canonicalize().unwrap();
        assert_eq!(
            pi_managed_root(&launcher),
            Some(root.canonicalize().unwrap())
        );
        let i = installation_for("pi", &launcher, "0.87.1");
        assert!(matches!(plan(&i), Plan::Native(_)));
        let s = super::super::install_spec(&i, &plan(&i), "0.88.0").unwrap();
        assert_eq!(s.args, ["update", "--self"]);
        write(&root.join("current-version"), "../../other");
        assert!(pi_managed_root(&launcher).is_none());
    }

    #[test]
    fn opencode_native_upgrade_pins_target_and_method() {
        let i = installation_for("opencode", Path::new("/selected/opencode"), "1.15.10");
        let s = super::super::install_spec(&i, &Plan::Native("test"), "1.18.0").unwrap();
        assert_eq!(s.executable, Path::new("/selected/opencode"));
        assert_eq!(s.args, ["upgrade", "1.18.0", "--method", "curl"]);
    }

    #[test]
    fn npm_installs_keep_their_original_runtime_prefix_and_package() {
        for provider in ["pi", "opencode", "deepseek-harness"] {
            let dir = tempfile::tempdir().unwrap();
            let prefix = dir.path().join("node installation");
            let name = super::super::package(provider).unwrap();
            let package = prefix.join("lib/node_modules").join(name);
            let executable = package.join("bin/agent.js");
            write(&executable, "");
            write(
                &package.join("package.json"),
                &serde_json::json!({"name":name}).to_string(),
            );
            write(&prefix.join("bin/node"), "");
            write(&prefix.join("lib/node_modules/npm/bin/npm-cli.js"), "");
            let i = installation_for(provider, &executable, "1.0.0");
            let p = plan(&i);
            assert!(matches!(p, Plan::Npm { .. }));
            let s = super::super::install_spec(&i, &p, "1.1.0").unwrap();
            assert_eq!(
                s.executable,
                prefix.join("bin/node").canonicalize().unwrap()
            );
            assert!(s.args.contains(&format!("{name}@1.1.0")));
            assert!(
                s.args.contains(
                    &prefix
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                )
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cached_dsh_update_command_runs_only_in_selected_prefix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("selected cache");
        std::fs::create_dir(&prefix).unwrap();
        let node = dir.path().join("runtime/node");
        // Stand-in runtime records structured argv and cwd; it never downloads packages.
        write(
            &node,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > received-args\npwd > received-cwd\n",
        );
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let s = local_npm_spec(
            &prefix,
            "@deepseek-ai/dsh",
            "0.2.0",
            &node,
            Path::new("/selected/npm-cli.js"),
        )
        .unwrap();
        super::super::command(s).await.unwrap();
        let args = std::fs::read_to_string(prefix.join("received-args")).unwrap();
        assert!(args.contains("@deepseek-ai/dsh@0.2.0\n"));
        assert!(!args.contains("--global"));
        assert_eq!(
            Path::new(
                std::fs::read_to_string(prefix.join("received-cwd"))
                    .unwrap()
                    .trim()
            )
            .canonicalize()
            .unwrap(),
            prefix.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    #[ignore = "read-only update checks for local installations and official release endpoints"]
    async fn real_new_agent_update_checks() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        for (provider, path) in [
            ("pi", home.join(".local/bin/pi")),
            ("opencode", home.join(".opencode/bin/opencode")),
            (
                "deepseek-harness",
                PathBuf::from(std::env::var("DOGCODES_DSH_BINARY").unwrap()),
            ),
        ] {
            let current = super::super::command(spec(&path, vec!["--version".into()], false))
                .await
                .unwrap();
            let i = installation_for(provider, &path, &version(&current).unwrap().to_string());
            let p = plan(&i);
            assert!(!matches!(p, Plan::Unsupported | Plan::Host(_)));
            let status = inspect(&i, &p).await.unwrap();
            let command = install_spec(&i, &p, status.latest_version.as_deref().unwrap())
                .await
                .unwrap();
            assert!(!command.args.is_empty());
            println!(
                "{provider}: current={:?}, latest={:?}, status={}, source={}",
                status.current_version, status.latest_version, status.status, status.source
            );
        }
    }
}
