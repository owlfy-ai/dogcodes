//! Resolve user commands in the login + interactive shell.
//! `OpenCode` may require its desktop-managed runtime for shared database compatibility.
use agent_registry::{DiscoveryContext, ProviderManifest};
use agent_runtime::{ProcessSpec, run_bounded};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Resolution {
    Executable(PathBuf),
    Missing,
    ShellOverride,
}
type Commands = BTreeMap<String, Resolution>;

fn canonical_executable(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    std::fs::canonicalize(path).ok()
}

fn parse_commands(output: &str, names: &[String]) -> Option<Commands> {
    let mut commands = Commands::new();
    for record in output.split("\0DOGCODES_COMMAND\0").skip(1) {
        let mut fields = record.split('\0');
        let name = fields.next()?;
        let value = fields.next()?;
        if !names.iter().any(|candidate| candidate == name) {
            continue;
        }
        let resolution = if value.is_empty() {
            Resolution::Missing
        } else if let Some(path) = canonical_executable(Path::new(value)) {
            Resolution::Executable(path)
        } else {
            Resolution::ShellOverride
        };
        commands.insert(name.to_owned(), resolution);
    }
    (commands.len() == names.len()).then_some(commands)
}

async fn shell_commands(context: &DiscoveryContext) -> Option<Commands> {
    // The eight provider scans share one bounded shell probe, including concurrent
    // callers from the welcome page. A short cache avoids eight shell startups.
    type Cache = Option<(Instant, Option<Commands>)>;
    static CACHE: std::sync::OnceLock<tokio::sync::Mutex<Cache>> = std::sync::OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().await;
    if let Some((at, commands)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(2)
    {
        return commands.clone();
    }
    let result = probe_shell(context).await;
    *cache = Some((Instant::now(), result.clone()));
    result
}

// Only the macOS directory lookup awaits; other platforms rely on SHELL.
#[cfg_attr(not(target_os = "macos"), allow(clippy::unused_async))]
pub(crate) async fn user_shell(_context: &DiscoveryContext) -> Option<PathBuf> {
    if let Some(shell) = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(shell);
    }
    // Finder launches may not inherit SHELL. Ask the macOS user directory for
    // the configured login shell rather than assuming zsh or the app's PATH.
    #[cfg(target_os = "macos")]
    {
        // A relocated/localized home folder need not match the account's short
        // name. Query the effective user instead of deriving it from HOME.
        let identity = run_bounded(&ProcessSpec::probe(
            PathBuf::from("/usr/bin/id"),
            vec!["-un".into()],
        ))
        .await
        .ok()?;
        if !identity.success {
            return None;
        }
        let record = directory_user_record(&identity.stdout)?;
        let spec = ProcessSpec::probe(
            PathBuf::from("/usr/bin/dscl"),
            vec![".".into(), "-read".into(), record, "UserShell".into()],
        );
        let result = run_bounded(&spec).await.ok()?;
        if result.success {
            return result
                .stdout
                .trim()
                .strip_prefix("UserShell:")
                .map(str::trim)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute());
        }
    }
    None
}

#[cfg(any(target_os = "macos", test))]
fn directory_user_record(identity: &str) -> Option<String> {
    // /Users is the dscl record namespace, not a filesystem home directory.
    let account = identity.trim();
    (!account.is_empty()
        && !matches!(account, "." | "..")
        && !account.contains('/')
        && !account.chars().any(char::is_control))
    .then(|| format!("/Users/{account}"))
}

async fn probe_shell(context: &DiscoveryContext) -> Option<Commands> {
    let shell = user_shell(context).await?;
    if !shell.is_absolute() {
        return None;
    }
    let family = shell.file_name()?.to_str()?;
    if !matches!(family, "zsh" | "bash" | "sh" | "fish") {
        return None;
    }
    let names: Vec<String> = super::BUILTIN_AGENT_MANIFESTS
        .iter()
        .filter_map(|source| ProviderManifest::parse_toml(source).ok())
        .flat_map(|manifest| manifest.discovery.candidate_executables)
        .chain(["node".into(), "npm".into()])
        .collect();
    // Names come only from validated, built-in manifests; no user text or path
    // is interpolated into shell code. command -v does not launch the agents.
    let script = names.iter().map(|name| {
        if family == "fish" {
            format!("set -l resolved_executable (command -v {name} 2>/dev/null); if functions -q {name}; set resolved_executable shell-function; end; printf '\\0DOGCODES_COMMAND\\0{name}\\0%s\\0' \"$resolved_executable\";")
        } else {
            format!("printf '\\0DOGCODES_COMMAND\\0{name}\\0%s\\0' \"$(command -v {name} 2>/dev/null)\";")
        }
    }).collect::<String>();
    let output = run_bounded(&ProcessSpec {
        executable: shell,
        args: vec!["-lic".into(), script],
        env_allowlist: [
            "HOME",
            "PATH",
            "SHELL",
            "USER",
            "LOGNAME",
            "ZDOTDIR",
            "XDG_CONFIG_HOME",
            "NVM_DIR",
            "TERM",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        env_overrides: BTreeMap::new(),
        cwd: context.home_dir.clone(),
        timeout: Duration::from_secs(5),
        stdout_limit: 65536,
        stderr_limit: 4096,
    })
    .await
    .ok()?;
    if !output.success || output.stdout_truncated {
        return None;
    }
    parse_commands(&output.stdout, &names)
}

/// Resolve the user's actual Node/npm pair for an existing local npm installation.
pub(crate) async fn npm_runtime() -> Option<(PathBuf, PathBuf)> {
    let context = DiscoveryContext::from_current_process();
    let commands = shell_commands(&context).await;
    let resolve = |name: &str| {
        if let Some(commands) = &commands {
            return match commands.get(name) {
                Some(Resolution::Executable(path)) => Some(path.clone()),
                _ => None,
            };
        }
        context
            .path_entries
            .iter()
            .find_map(|dir| canonical_executable(&dir.join(name)))
    };
    let node = resolve("node")?;
    let npm = resolve("npm")?;
    let root = npm.parent()?.parent()?;
    let info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("package.json")).ok()?).ok()?;
    (info["name"] == "npm" && npm.file_name()?.to_str()? == "npm-cli.js").then_some((node, npm))
}

fn select_executable(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
    commands: Option<&Commands>,
) -> Result<Option<PathBuf>, String> {
    for name in &manifest.discovery.candidate_executables {
        match commands.and_then(|values| values.get(name)) {
            Some(Resolution::Executable(path)) => return Ok(Some(path.clone())),
            Some(Resolution::ShellOverride) => return Err("终端中的命令使用了自定义别名或函数，暂时无法确认对应安装。请使用直接指向客户端的命令后重新检测。".into()),
            _ => {}
        }
    }
    // If the shell was read successfully, a missing command must not be replaced
    // with a different PATH inherited by the GUI launcher.
    if commands.is_none() {
        for name in &manifest.discovery.candidate_executables {
            for directory in &context.path_entries {
                if !directory.is_absolute() {
                    continue;
                }
                if let Some(path) = canonical_executable(&directory.join(name)) {
                    return Ok(Some(path));
                }
                #[cfg(windows)]
                if let Some(path) = canonical_executable(&directory.join(format!("{name}.exe"))) {
                    return Ok(Some(path));
                }
            }
        }
    }
    // Some agents (such as ZCode) have a desktop app but no terminal command.
    if let Some(home) = &context.home_dir {
        for relative in manifest
            .discovery
            .user_paths
            .get(&context.platform)
            .into_iter()
            .flatten()
        {
            if let Some(path) = canonical_executable(&home.join(relative)) {
                return Ok(Some(path));
            }
        }
    }
    for path in manifest
        .discovery
        .platform_paths
        .get(&context.platform)
        .into_iter()
        .flatten()
    {
        if let Some(path) = canonical_executable(path) {
            return Ok(Some(path));
        }
    }
    for path in agent_registry::windows_desktop_executables(manifest.id.as_str(), context) {
        if let Some(path) = canonical_executable(&path) {
            return Ok(Some(path));
        }
    }
    // DeepSeek Harness is commonly used via npx, as in codex-host. Reuse an
    // already cached, named package without invoking npx or installing anything.
    // A real terminal/global installation always wins over this fallback.
    if manifest.id.as_str() == "deepseek-harness" {
        return Ok(context.home_dir.as_deref().and_then(cached_deepseek));
    }
    Ok(None)
}

fn cached_deepseek(home: &Path) -> Option<PathBuf> {
    let mut candidates = std::fs::read_dir(home.join(".npm/_npx"))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let package = entry.path().join("node_modules/@deepseek-ai/dsh");
            let manifest_path = package.join("package.json");
            let metadata = std::fs::metadata(&manifest_path).ok()?;
            if metadata.len() > 65536 {
                return None;
            }
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(manifest_path).ok()?).ok()?;
            if manifest["name"] != "@deepseek-ai/dsh" {
                return None;
            }
            let executable = canonical_executable(&package.join("lib/bin.js"))?;
            Some((metadata.modified().ok()?, executable))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().next().map(|(_, path)| path)
}

// OpenCode 2 desktop migrates the shared database beyond the standalone 1.x
// CLI schema. Only replace the standard home installation, never a custom shell
// command/wrapper. Other providers retain strict terminal-command precedence.
fn opencode_desktop_candidates(home: &Path) -> Vec<(semver::Version, PathBuf)> {
    let root = home.join("Library/Application Support/ai.opencode.desktop/cli");
    let mut candidates = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(64)
        .filter_map(|entry| {
            let version = semver::Version::parse(entry.file_name().to_str()?).ok()?;
            if version.major < 2 {
                return None;
            }
            Some((
                version,
                canonical_executable(&entry.path().join("opencode-cli"))?,
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates
}

async fn compatible_opencode(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
    selected: Option<PathBuf>,
) -> Option<PathBuf> {
    if manifest.id.as_str() != "opencode" || context.platform != agent_registry::Platform::Macos {
        return selected;
    }
    let Some(home) = context.home_dir.as_deref() else {
        return selected;
    };
    if let Some(path) = &selected {
        if canonical_executable(&home.join(".opencode/bin/opencode")).as_ref() != Some(path) {
            return selected;
        }
        let Ok(report) = agent_registry::probe_installation(manifest, path.clone()).await else {
            return selected;
        };
        if report.version.is_none_or(|version| version.major != 1) {
            return selected;
        }
    }
    for (version, path) in opencode_desktop_candidates(home) {
        if let Ok(report) = agent_registry::probe_installation(manifest, path.clone()).await
            && report.version.as_ref() == Some(&version)
            && report.status == agent_registry::InstallationStatus::Installed
        {
            return Some(path);
        }
    }
    selected
}

pub(crate) async fn preferred_executable(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Result<Option<PathBuf>, String> {
    let commands = shell_commands(context).await;
    #[cfg(unix)]
    if commands.is_none() {
        return Err("暂时无法读取终端的命令位置，请检查终端能否正常打开后重新检测。".into());
    }
    let selected = select_executable(manifest, context, commands.as_ref())?;
    Ok(compatible_opencode(manifest, context, selected).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_registry::Platform;
    #[test]
    fn shell_command_records_preserve_unicode_and_spaces_in_installation_paths() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("迁移主目录/中文 用户/bin/claude");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, []).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let names = vec!["claude".into()];
        let commands = parse_commands(
            &format!("\0DOGCODES_COMMAND\0claude\0{}\0", path.display()),
            &names,
        )
        .unwrap();
        assert_eq!(
            commands["claude"],
            Resolution::Executable(std::fs::canonicalize(path).unwrap())
        );
    }
    #[test]
    fn directory_account_lookup_uses_the_reported_identity_without_assuming_a_home_folder() {
        assert_eq!(
            directory_user_record("short-account\n"),
            Some("/Users/short-account".into())
        );
        assert_eq!(
            directory_user_record("中文用户\n"),
            Some("/Users/中文用户".into())
        );
        for value in ["", "..", "/Volumes/迁移目录/张三", "user\nother"] {
            assert!(directory_user_record(value).is_none());
        }
    }
    #[tokio::test]
    #[cfg(unix)]
    async fn opencode_shared_database_uses_compatible_desktop_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/opencode.toml"
        ))
        .unwrap();
        let context = DiscoveryContext {
            platform: Platform::Macos,
            path_entries: vec![],
            home_dir: Some(dir.path().into()),
        };
        let old = dir.path().join(".opencode/bin/opencode");
        let new = dir
            .path()
            .join("Library/Application Support/ai.opencode.desktop/cli/2.0.18/opencode-cli");
        for (path, version) in [(&old, "1.15.10"), (&new, "2.0.18")] {
            executable(path);
            std::fs::write(path, format!("#!/bin/sh\necho 'opencode v{version}'\n")).unwrap();
        }
        let old = std::fs::canonicalize(old).unwrap();
        let new = std::fs::canonicalize(new).unwrap();
        assert_eq!(
            compatible_opencode(&manifest, &context, Some(old.clone())).await,
            Some(new.clone())
        );
        assert_eq!(
            compatible_opencode(&manifest, &context, None).await,
            Some(new.clone())
        );
        let custom = dir.path().join("custom-opencode");
        executable(&custom);
        assert_eq!(
            compatible_opencode(&manifest, &context, Some(custom.clone())).await,
            Some(custom)
        );
        std::fs::write(&new, "#!/bin/sh\necho 'unrelated v2.0.18'\n").unwrap();
        assert_eq!(
            compatible_opencode(&manifest, &context, Some(old.clone())).await,
            Some(old.clone())
        );
        std::fs::write(&old, "#!/bin/sh\necho 'opencode v2.1.0'\n").unwrap();
        assert_eq!(
            compatible_opencode(&manifest, &context, Some(old.clone())).await,
            Some(old)
        );
    }

    #[tokio::test]
    #[ignore = "probes local OpenCode discovery without a model call"]
    async fn current_opencode_selects_desktop_compatible_runtime() {
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/opencode.toml"
        ))
        .unwrap();
        let path = preferred_executable(&manifest, &DiscoveryContext::from_current_process())
            .await
            .unwrap()
            .unwrap();
        let report = agent_registry::probe_installation(&manifest, path)
            .await
            .unwrap();
        assert!(report.version.unwrap().major >= 2);
    }

    #[test]
    #[cfg(unix)]
    fn deepseek_cache_is_offline_fallback_and_never_overrides_terminal_installation() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir
            .path()
            .join(".npm/_npx/cache/node_modules/@deepseek-ai/dsh");
        let cached = package.join("lib/bin.js");
        executable(&cached);
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"unrelated-package"}"#,
        )
        .unwrap();
        assert!(cached_deepseek(dir.path()).is_none());
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh"}"#,
        )
        .unwrap();
        assert_eq!(
            cached_deepseek(dir.path()),
            Some(std::fs::canonicalize(&cached).unwrap())
        );
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/deepseek-harness.toml"
        ))
        .unwrap();
        let preferred = dir.path().join("preferred-dsh");
        executable(&preferred);
        let commands = BTreeMap::from([("dsh".into(), Resolution::Executable(preferred.clone()))]);
        let context = DiscoveryContext {
            platform: Platform::Macos,
            path_entries: vec![],
            home_dir: Some(dir.path().into()),
        };
        assert_eq!(
            select_executable(&manifest, &context, Some(&commands)).unwrap(),
            Some(preferred)
        );
    }
    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn shell_target_beats_newer_or_lexically_first_installations() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("versions/1");
        let new = dir.path().join("versions/9");
        executable(&old);
        executable(&new);
        let link = dir.path().join("claude");
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let names = vec!["claude".into()];
        let commands = parse_commands(
            &format!(
                "shell banner\n\0DOGCODES_COMMAND\0claude\0{}\0",
                link.display()
            ),
            &names,
        )
        .unwrap();
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        let context = DiscoveryContext {
            platform: Platform::Macos,
            path_entries: vec![],
            home_dir: None,
        };
        assert_eq!(
            select_executable(&manifest, &context, Some(&commands)).unwrap(),
            Some(std::fs::canonicalize(old).unwrap())
        );
    }
    #[test]
    #[cfg(unix)]
    fn codex_desktop_remains_discoverable_after_bundle_layout_upgrade() {
        for app in ["ChatGPT.app", "Codex.app"] {
            for user_install in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut manifest = ProviderManifest::parse_toml(include_str!(
                    "../../../../drivers/manifests/codex.toml"
                ))
                .unwrap();
                // Relocate system candidates into the fixture; retain the actual
                // manifest's relative paths for the user Applications case.
                for path in manifest.discovery.platform_paths.values_mut().flatten() {
                    *path = dir.path().join(path.strip_prefix("/").unwrap());
                }
                let root = if user_install {
                    dir.path().join("home")
                } else {
                    dir.path().to_path_buf()
                };
                let context = DiscoveryContext {
                    platform: Platform::Macos,
                    path_entries: vec![],
                    home_dir: Some(dir.path().join("home")),
                };
                let commands = Commands::from([("codex".into(), Resolution::Missing)]);
                let resources = root.join(format!("Applications/{app}/Contents/Resources"));
                let legacy = resources.join("codex");
                executable(&legacy);
                assert_eq!(
                    select_executable(&manifest, &context, Some(&commands)).unwrap(),
                    Some(std::fs::canonicalize(&legacy).unwrap())
                );
                std::fs::remove_file(legacy).unwrap();
                let current = resources.join("codex-cli/bin/codex");
                executable(&current);
                assert_eq!(
                    select_executable(&manifest, &context, Some(&commands)).unwrap(),
                    Some(std::fs::canonicalize(&current).unwrap())
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "probes the locally installed Codex and account readiness, no model turn"]
    async fn current_codex_desktop_is_ready() {
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../../drivers/manifests/codex.toml"))
                .unwrap();
        let executable = preferred_executable(&manifest, &DiscoveryContext::from_current_process())
            .await
            .unwrap()
            .expect("Codex installation must be discovered");
        let report = agent_registry::probe_installation(&manifest, executable.clone())
            .await
            .unwrap();
        assert_eq!(report.status, agent_registry::InstallationStatus::Installed);
        codex_driver::CodexDriver::probe_ready(&executable)
            .await
            .expect("Codex initialize/account-read must succeed");
    }

    #[test]
    fn aliases_and_functions_are_not_silently_replaced_with_an_unrelated_installation() {
        let names = vec!["claude".into()];
        for value in ["alias claude='other-client'", "claude"] {
            let commands =
                parse_commands(&format!("\0DOGCODES_COMMAND\0claude\0{value}\0"), &names).unwrap();
            let manifest = ProviderManifest::parse_toml(include_str!(
                "../../../../drivers/manifests/claude-code.toml"
            ))
            .unwrap();
            let context = DiscoveryContext {
                platform: Platform::Macos,
                path_entries: vec![],
                home_dir: None,
            };
            assert!(select_executable(&manifest, &context, Some(&commands)).is_err());
        }
        assert!(parse_commands("startup failed", &names).is_none());
    }
    #[test]
    #[cfg(unix)]
    fn fallback_respects_path_order_and_resolves_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first/claude");
        let second = dir.path().join("second/claude");
        executable(&first);
        executable(&second);
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        let context = DiscoveryContext {
            platform: Platform::Macos,
            path_entries: vec![
                first.parent().unwrap().into(),
                second.parent().unwrap().into(),
            ],
            home_dir: None,
        };
        assert_eq!(
            select_executable(&manifest, &context, None).unwrap(),
            Some(std::fs::canonicalize(first).unwrap())
        );
    }

    #[tokio::test]
    async fn bootstrap_and_registered_drivers_use_only_the_selected_installation() {
        use agent_core::Core;
        use agent_protocol::{
            ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, RecordInstallationProbe,
        };
        use agent_store::SqliteStore;
        use driver_api::FakeDriver;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let core = Arc::new(Core::new(Arc::new(
            SqliteStore::open(dir.path().join("state.db")).unwrap(),
        )));
        for (path, version) in [
            ("/bin/terminal-claude", "1.0.0"),
            ("/bin/other-claude", "9.0.0"),
        ] {
            let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
                RecordInstallationProbe {
                    provider_id: "claude-code".into(),
                    executable: path.into(),
                    fingerprint: path.into(),
                    status: InstallationStatus::Ready,
                    version: Some(version.into()),
                    version_text: None,
                    diagnostic: None,
                    elapsed_millis: 1,
                },
            ));
            command.actor = ActorRef::SystemRecovery;
            core.execute(command).await.unwrap();
        }
        core.register_driver(Arc::new(FakeDriver::new("claude-code").unwrap()))
            .await;
        core.select_installation("claude-code", Some("/bin/terminal-claude".into()))
            .await;
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.installations.len(), 1);
        assert_eq!(snapshot.installations[0].version.as_deref(), Some("1.0.0"));
        assert!(core.registered_executors().await.is_empty());
        core.select_installation("claude-code", None).await;
        assert!(core.bootstrap().await.unwrap().installations.is_empty());
    }

    #[tokio::test]
    #[ignore = "reads the current user's shell configuration"]
    async fn current_terminal_claude() {
        let commands = probe_shell(&DiscoveryContext::from_current_process())
            .await
            .expect("shell commands");
        println!("claude: {:?}", commands.get("claude"));
        assert!(matches!(
            commands.get("claude"),
            Some(Resolution::Executable(_))
        ));
    }
}
