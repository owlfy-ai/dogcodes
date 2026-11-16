//! Resolve user commands in the login + interactive shell.
//! Prefer verified desktop-managed runtimes, with standalone terminal CLIs as fallback.
use agent_registry::{DiscoveryContext, ProviderManifest};
use agent_runtime::{ProcessSpec, run_bounded};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
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

fn parse_shell_path(output: &str) -> Option<std::ffi::OsString> {
    let record = output.split("\0DOGCODES_PATH\0").nth(1)?;
    let (path, _) = record.split_once('\0')?;
    Some(path.into())
}

async fn shell_commands(context: &DiscoveryContext) -> Option<Commands> {
    // Share concurrent shell startups, but never retain a completed command
    // lookup across rescans after an installer changes shell configuration.
    type Outcome = Option<Option<Commands>>;
    type Pending = std::sync::Mutex<Option<tokio::sync::watch::Receiver<Outcome>>>;
    static RUNNING: std::sync::OnceLock<Pending> = std::sync::OnceLock::new();
    let running = RUNNING.get_or_init(Default::default);
    let mut receiver = {
        let mut pending = running.lock().expect("shell probe registry");
        if let Some(receiver) = pending.as_ref() {
            receiver.clone()
        } else {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            *pending = Some(receiver.clone());
            let context = context.clone();
            tokio::spawn(async move {
                let result = tokio::spawn(async move { probe_shell(&context).await })
                    .await
                    .ok()
                    .flatten();
                let mut pending = running.lock().expect("shell probe registry");
                sender.send_replace(Some(result));
                *pending = None;
            });
            receiver
        }
    };
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return None;
        }
    }
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
    let mut script = names.iter().map(|name| {
        if family == "fish" {
            format!("set -l resolved_executable (command -v {name} 2>/dev/null); if functions -q {name}; set resolved_executable shell-function; end; printf '\\0DOGCODES_COMMAND\\0{name}\\0%s\\0' \"$resolved_executable\";")
        } else {
            format!("printf '\\0DOGCODES_COMMAND\\0{name}\\0%s\\0' \"$(command -v {name} 2>/dev/null)\";")
        }
    }).collect::<String>();
    script.push_str(if family == "fish" {
        "printf '\\0DOGCODES_PATH\\0%s\\0' (string join : $PATH);"
    } else {
        "printf '\\0DOGCODES_PATH\\0%s\\0' \"$PATH\";"
    });
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
        // Start each login shell from the inherited launch environment, rather
        // than repeatedly prepending rc-file entries to our last shell result.
        env_overrides: std::env::var_os("PATH")
            .map(|path| BTreeMap::from([("PATH".into(), path)]))
            .unwrap_or_default(),
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
    let commands = parse_commands(&output.stdout, &names)?;
    agent_runtime::record_shell_path(parse_shell_path(&output.stdout)?);
    Some(commands)
}

/// Resolve the user's actual Node/npm pair for an existing local npm installation.
pub(crate) async fn npm_runtime() -> Option<(PathBuf, PathBuf)> {
    let context = DiscoveryContext::from_current_process();
    if context.platform == agent_registry::Platform::Windows {
        return windows_npm_runtime(&context.path_entries);
    }
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
            .flat_map(|dir| agent_registry::command_paths(dir, name, context.platform))
            .find_map(|path| canonical_executable(&path))
    };
    let node = resolve("node")?;
    let npm = resolve("npm")?;
    let root = npm.parent()?.parent()?;
    let info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("package.json")).ok()?).ok()?;
    (info["name"] == "npm" && npm.file_name()?.to_str()? == "npm-cli.js").then_some((node, npm))
}

fn windows_npm_runtime(directories: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    directories
        .iter()
        .filter(|p| p.is_absolute())
        .find_map(|bin| {
            // GUI launchers can prepend a private Node without npm. Continue to
            // the next complete pair instead of combining unrelated runtimes.
            let node = bin.join("node.exe");
            let npm = bin.join("node_modules/npm/bin/npm-cli.js");
            if !node.is_file() || !npm.is_file() {
                return None;
            }
            let manifest = bin.join("node_modules/npm/package.json");
            if std::fs::metadata(&manifest).ok()?.len() > 65_536 {
                return None;
            }
            let info: serde_json::Value =
                serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
            (info["name"] == "npm").then_some((node.canonicalize().ok()?, npm.canonicalize().ok()?))
        })
}

fn select_executable(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
    commands: Option<&Commands>,
) -> Result<Option<PathBuf>, String> {
    let selected = select_existing_executable(manifest, context, commands)?;
    if selected.as_ref().is_none_or(|path| {
        crate::agent_updates::managed_cli::desktop_cli(manifest.id.as_str(), path)
    }) && let Some(managed) =
        crate::agent_updates::managed_cli::candidate(context, manifest.id.as_str())
    {
        return Ok(Some(
            managed
                .canonicalize()
                .map_err(|_| "backendText.mcee0fad9")?,
        ));
    }
    Ok(selected)
}

fn select_existing_executable(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
    commands: Option<&Commands>,
) -> Result<Option<PathBuf>, String> {
    // Desktop candidates have already failed verification when fallback is needed.
    // Do not rediscover the rejected runtime through a symlink, user path or PATH.
    let desktops: std::collections::BTreeSet<_> = desktop_runtime_candidates(manifest, context)
        .iter()
        .filter_map(|path| canonical_executable(path))
        .collect();
    let standalone =
        |path: &Path| canonical_executable(path).filter(|path| !desktops.contains(path));
    for name in &manifest.discovery.candidate_executables {
        match commands.and_then(|values| values.get(name)) {
            Some(Resolution::Executable(path)) if !desktops.contains(path) => {
                return Ok(Some(path.clone()));
            }
            Some(Resolution::ShellOverride) => return Err("backendText.shellOverride".into()),
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
                for path in agent_registry::command_paths(directory, name, context.platform) {
                    if let Some(path) = standalone(&path) {
                        return Ok(Some(path));
                    }
                }
            }
        }
    }
    // Check conventional standalone paths after terminal command resolution.
    if let Some(home) = &context.home_dir {
        for relative in manifest
            .discovery
            .user_paths
            .get(&context.platform)
            .into_iter()
            .flatten()
        {
            if let Some(path) = standalone(&home.join(relative)) {
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
        if let Some(path) = standalone(path) {
            return Ok(Some(path));
        }
    }
    for path in agent_registry::windows_desktop_executables(manifest.id.as_str(), context)
        .into_iter()
        .chain(agent_registry::macos_desktop_executables(
            manifest.id.as_str(),
            context,
        ))
    {
        if let Some(path) = standalone(&path) {
            return Ok(Some(path));
        }
    }
    // DeepSeek Harness is commonly used via npx, as in codex-host. Reuse an
    // already cached, named package without invoking npx or installing anything.
    // A real terminal/global installation always wins over this fallback.
    if manifest.id.as_str() == "deepseek-harness" {
        return Ok(cached_deepseek(
            &deepseek_cache_roots(context),
            context.platform,
        ));
    }
    Ok(None)
}

fn deepseek_cache_roots(context: &DiscoveryContext) -> Vec<PathBuf> {
    let Some(home) = context.home_dir.as_deref() else {
        return vec![];
    };
    let current_user = agent_registry::home_directory().as_deref() == Some(home);
    let mut roots = Vec::new();
    if current_user
        && let Some(cache) = std::env::var_os("npm_config_cache")
            .or_else(|| std::env::var_os("NPM_CONFIG_CACHE"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    {
        roots.push(cache);
    }
    if context.platform == agent_registry::Platform::Windows {
        roots.push(user_data_dir(home, "LOCALAPPDATA", "AppData/Local").join("npm-cache"));
    }
    roots.push(home.join(".npm"));
    roots
}

fn cached_deepseek(roots: &[PathBuf], platform: agent_registry::Platform) -> Option<PathBuf> {
    let mut candidates = roots
        .iter()
        .flat_map(|root| {
            std::fs::read_dir(root.join("_npx"))
                .into_iter()
                .flatten()
                .take(256)
        })
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
            let script = canonical_executable(&package.join("lib/bin.js"))?;
            let executable = if platform == agent_registry::Platform::Windows {
                // Windows cannot directly execute the JavaScript entry point.
                // Reuse npm's existing launcher and its selected Node runtime.
                canonical_executable(&entry.path().join("node_modules/.bin/dsh.cmd"))?
            } else {
                script
            };
            Some((metadata.modified().ok()?, executable))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().next().map(|(_, path)| path)
}

// Desktop-managed OpenCode caches supplement the app-bundled runtime.
fn opencode_desktop_candidates(
    platform: agent_registry::Platform,
    home: &Path,
) -> Vec<(semver::Version, PathBuf)> {
    let (root, binary) = match platform {
        agent_registry::Platform::Macos => (
            home.join("Library/Application Support/ai.opencode.desktop/cli"),
            "opencode-cli",
        ),
        agent_registry::Platform::Windows => (
            user_data_dir(home, "APPDATA", "AppData/Roaming").join("ai.opencode.desktop/cli"),
            "opencode-cli.exe",
        ),
        agent_registry::Platform::Linux => return vec![],
    };
    let mut candidates = Vec::new();
    if platform == agent_registry::Platform::Windows {
        // Prefer the bundled copy: its path stays stable across desktop updates.
        let resources = user_data_dir(home, "LOCALAPPDATA", "AppData/Local")
            .join("Programs/ai.opencode.desktop/resources");
        if let Some(version) = std::fs::read(resources.join("opencode-cli.version"))
            .ok()
            .filter(|bytes| bytes.len() <= 64)
            .and_then(|bytes| semver::Version::parse(String::from_utf8(bytes).ok()?.trim()).ok())
            && let Some(path) = canonical_executable(&resources.join(binary))
        {
            candidates.push((version, path));
        }
    }
    candidates.extend(
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .take(64)
            .filter_map(|entry| {
                // Windows names these directories `<version>-<arch>`.
                let name = entry.file_name();
                let name = name.to_str()?;
                let name = ["-x64", "-arm64"]
                    .into_iter()
                    .find_map(|arch| name.strip_suffix(arch))
                    .unwrap_or(name);
                Some((
                    semver::Version::parse(name).ok()?,
                    canonical_executable(&entry.path().join(binary))?,
                ))
            }),
    );
    candidates.retain(|(version, _)| version.major >= 2);
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates
}

pub(crate) use agent_management::user_data_dir;

fn desktop_runtime_candidates(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Vec<PathBuf> {
    let provider = manifest.id.as_str();
    if !matches!(
        provider,
        "claude-code" | "codex" | "zcode" | "deepseek-harness" | "opencode"
    ) {
        return vec![];
    }
    let mut candidates = agent_registry::windows_desktop_executables(provider, context);
    candidates.extend(agent_registry::macos_desktop_executables(provider, context));
    if provider == "opencode"
        && let Some(home) = &context.home_dir
    {
        candidates.extend(
            opencode_desktop_candidates(context.platform, home)
                .into_iter()
                .map(|(_, path)| path),
        );
    }
    candidates
}

async fn preferred_desktop_runtime(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Option<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    for path in desktop_runtime_candidates(manifest, context) {
        let Some(path) = canonical_executable(&path) else {
            continue;
        };
        if seen.insert(path.clone())
            && let Ok(report) = agent_registry::probe_installation(manifest, path.clone()).await
            && report.status == agent_registry::InstallationStatus::Installed
            && report.version.is_some()
        {
            return Some(path);
        }
    }
    None
}

pub(crate) async fn preferred_executable(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Result<Option<PathBuf>, String> {
    // This return also prevents a DogCodes-managed standalone CLI from
    // replacing a working desktop runtime, and needs no terminal shell startup.
    if let Some(desktop) = preferred_desktop_runtime(manifest, context).await {
        return Ok(Some(desktop));
    }
    let commands = shell_commands(context).await;
    #[cfg(unix)]
    if commands.is_none() {
        return Err("backendText.terminalPathUnreadable".into());
    }
    select_executable(manifest, context, commands.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn rejected_desktop_runtime_falls_back_to_managed_or_custom_cli() {
        let home = tempfile::tempdir().unwrap();
        let context = DiscoveryContext {
            platform: agent_registry::Platform::Windows,
            home_dir: Some(home.path().into()),
            path_entries: vec![],
        };
        for (provider, manifest, relative) in [
            (
                "codex",
                include_str!("../../../../drivers/manifests/codex.toml"),
                "AppData/Local/OpenAI/Codex/bin/hash/codex.exe",
            ),
            (
                "deepseek-harness",
                include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
                "AppData/Local/Programs/DeepSeek Harness/resources/runtime/cli/bin/dsh.cmd",
            ),
        ] {
            let manifest = ProviderManifest::parse_toml(manifest).unwrap();
            let original = home.path().join(relative);
            std::fs::create_dir_all(original.parent().unwrap()).unwrap();
            std::fs::write(&original, "desktop CLI").unwrap();
            if provider == "deepseek-harness" {
                std::fs::write(
                    home.path()
                        .join("AppData/Local/Programs/DeepSeek Harness/DeepSeek Harness.exe"),
                    "desktop",
                )
                .unwrap();
            }
            let original = original.canonicalize().unwrap();
            assert_eq!(select_executable(&manifest, &context, None).unwrap(), None);
            let standalone =
                crate::agent_updates::managed_cli::fixtures::verified_fixture(&context, provider)
                    .canonicalize()
                    .unwrap();
            assert_eq!(
                select_executable(&manifest, &context, None).unwrap(),
                Some(standalone)
            );
            let custom = home.path().join("custom.exe");
            std::fs::write(&custom, "custom CLI").unwrap();
            let mut commands = Commands::new();
            commands.insert(
                manifest.discovery.candidate_executables[0].clone(),
                Resolution::Executable(custom.clone()),
            );
            assert_eq!(
                select_executable(&manifest, &context, Some(&commands)).unwrap(),
                Some(custom)
            );
            assert_eq!(std::fs::read_to_string(original).unwrap(), "desktop CLI");
        }
    }

    #[test]
    fn windows_updates_skip_private_node_without_npm_and_keep_a_matching_pair() {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("private runtime");
        let complete = dir.path().join("complete runtime");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::create_dir_all(complete.join("node_modules/npm/bin")).unwrap();
        std::fs::write(private.join("node.exe"), "").unwrap();
        std::fs::write(complete.join("node.exe"), "").unwrap();
        std::fs::write(complete.join("node_modules/npm/bin/npm-cli.js"), "").unwrap();
        let manifest = complete.join("node_modules/npm/package.json");
        std::fs::write(&manifest, r#"{"name":"npm"}"#).unwrap();
        let directories = vec![private, complete.clone()];
        assert_eq!(
            windows_npm_runtime(&directories),
            Some((
                complete.join("node.exe").canonicalize().unwrap(),
                complete
                    .join("node_modules/npm/bin/npm-cli.js")
                    .canonicalize()
                    .unwrap(),
            ))
        );
        std::fs::write(manifest, r#"{"name":"foreign-package"}"#).unwrap();
        assert!(windows_npm_runtime(&directories).is_none());
    }
    use agent_registry::Platform;
    #[test]
    fn shell_path_records_preserve_unicode_spaces_and_require_a_complete_frame() {
        assert_eq!(
            parse_shell_path("banner\0DOGCODES_PATH\0/Volumes/中文 用户/bin:/usr/bin\0"),
            Some("/Volumes/中文 用户/bin:/usr/bin".into())
        );
        assert!(parse_shell_path("\0DOGCODES_PATH\0truncated").is_none());
        assert!(parse_shell_path("shell failed").is_none());
    }
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
    async fn opencode_desktop_cache_precedes_modern_cli_and_rejects_foreign_binaries() {
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
        let cli = dir.path().join(".opencode/bin/opencode");
        let desktop = dir
            .path()
            .join("Library/Application Support/ai.opencode.desktop/cli/2.2.6/opencode-cli");
        for (path, version) in [(&cli, "2.2.5"), (&desktop, "2.2.6")] {
            executable(path);
            std::fs::write(path, format!("#!/bin/sh\necho 'opencode v{version}'\n")).unwrap();
        }
        assert_eq!(
            preferred_executable(&manifest, &context).await.unwrap(),
            Some(desktop.canonicalize().unwrap())
        );
        std::fs::write(&desktop, "#!/bin/sh\necho 'unrelated v2.2.6'\n").unwrap();
        assert!(
            preferred_desktop_runtime(&manifest, &context)
                .await
                .is_none()
        );
        let commands = Commands::from([("opencode".into(), Resolution::Executable(cli.clone()))]);
        assert_eq!(
            select_executable(&manifest, &context, Some(&commands)).unwrap(),
            Some(cli)
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn all_existing_desktop_adapters_precede_terminal_and_managed_cli() {
        for (source, mac, windows, output) in [
            (
                include_str!("../../../../drivers/manifests/codex.toml"),
                "Applications/Codex.app/Contents/Resources/codex",
                "AppData/Local/OpenAI/Codex/bin/hash/codex.exe",
                "codex-cli 0.60.0",
            ),
            (
                include_str!("../../../../drivers/manifests/zcode.toml"),
                "Applications/ZCode.app/Contents/MacOS/ZCode",
                "AppData/Local/Programs/ZCode/ZCode.exe",
                "zcode-cli 0.16.9",
            ),
            (
                include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
                "Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
                "AppData/Local/Programs/DeepSeek Harness/resources/runtime/cli/bin/dsh.cmd",
                "dsh 0.2.2",
            ),
            (
                include_str!("../../../../drivers/manifests/opencode.toml"),
                "Applications/OpenCode.app/Contents/Resources/opencode-cli",
                "AppData/Local/Programs/ai.opencode.desktop/resources/opencode-cli.exe",
                "opencode 2.2.6",
            ),
        ] {
            for (platform, relative) in [(Platform::Macos, mac), (Platform::Windows, windows)] {
                let home = tempfile::tempdir().unwrap();
                let context = DiscoveryContext {
                    platform,
                    path_entries: vec![],
                    home_dir: Some(home.path().into()),
                };
                let mut manifest = ProviderManifest::parse_toml(source).unwrap();
                // Fixtures are shell scripts, not Electron binaries. Real runtime probes retain the manifest environment.
                manifest.discovery.env_overrides.clear();
                let desktop = home.path().join(relative);
                executable(&desktop);
                std::fs::write(&desktop, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n"))
                    .unwrap();
                if platform == Platform::Windows {
                    let extra = match manifest.id.as_str() {
                        "zcode" => Some("AppData/Local/Programs/ZCode/resources/glm/zcode.cjs"),
                        "deepseek-harness" => {
                            Some("AppData/Local/Programs/DeepSeek Harness/DeepSeek Harness.exe")
                        }
                        "opencode" => {
                            Some("AppData/Local/Programs/ai.opencode.desktop/OpenCode.exe")
                        }
                        _ => None,
                    };
                    if let Some(extra) = extra {
                        executable(&home.path().join(extra));
                    }
                }
                let cli = home.path().join("bin/custom-cli");
                executable(&cli);
                let _managed =
                    matches!(manifest.id.as_str(), "codex" | "deepseek-harness").then(|| {
                        crate::agent_updates::managed_cli::fixtures::verified_fixture(
                            &context,
                            manifest.id.as_str(),
                        )
                    });
                assert_eq!(
                    preferred_executable(&manifest, &context).await.unwrap(),
                    Some(desktop.canonicalize().unwrap()),
                    "{} {platform:?}",
                    manifest.id
                );
                std::fs::write(&desktop, "#!/bin/sh\nexit 1\n").unwrap();
                assert!(
                    preferred_desktop_runtime(&manifest, &context)
                        .await
                        .is_none()
                );
                let desktop_alias = Commands::from([(
                    manifest.discovery.candidate_executables[0].clone(),
                    Resolution::Executable(desktop.canonicalize().unwrap()),
                )]);
                assert_ne!(
                    select_executable(&manifest, &context, Some(&desktop_alias)).unwrap(),
                    Some(desktop.canonicalize().unwrap())
                );
                let commands = Commands::from([(
                    manifest.discovery.candidate_executables[0].clone(),
                    Resolution::Executable(cli.clone()),
                )]);
                assert_eq!(
                    select_executable(&manifest, &context, Some(&commands)).unwrap(),
                    Some(cli)
                );
            }
        }
    }

    #[test]
    fn windows_opencode_bundled_and_cached_runtimes_are_recognized() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let bundled = home.join("AppData/Local/Programs/ai.opencode.desktop/resources");
        executable(&bundled.join("opencode-cli.exe"));
        std::fs::write(bundled.join("opencode-cli.version"), "2.2.5\n").unwrap();
        for name in ["2.2.5-x64", "2.3.0-arm64", "1.9.0-x64", "not-a-version"] {
            executable(&home.join(format!(
                "AppData/Roaming/ai.opencode.desktop/cli/{name}/opencode-cli.exe"
            )));
        }
        let canonical = |relative: &str| std::fs::canonicalize(home.join(relative)).unwrap();
        assert_eq!(
            opencode_desktop_candidates(Platform::Windows, home),
            vec![
                (
                    semver::Version::new(2, 3, 0),
                    canonical(
                        "AppData/Roaming/ai.opencode.desktop/cli/2.3.0-arm64/opencode-cli.exe"
                    )
                ),
                (
                    semver::Version::new(2, 2, 5),
                    canonical(
                        "AppData/Local/Programs/ai.opencode.desktop/resources/opencode-cli.exe"
                    )
                ),
                (
                    semver::Version::new(2, 2, 5),
                    canonical("AppData/Roaming/ai.opencode.desktop/cli/2.2.5-x64/opencode-cli.exe")
                ),
            ]
        );
        assert!(opencode_desktop_candidates(Platform::Linux, home).is_empty());
    }

    #[tokio::test]
    #[ignore = "checks installed desktop adapters and CLI fallback; no model calls"]
    async fn current_desktop_adapters_are_preferred() {
        let context = DiscoveryContext::from_current_process();
        for source in [
            include_str!("../../../../drivers/manifests/codex.toml"),
            include_str!("../../../../drivers/manifests/claude-code.toml"),
            include_str!("../../../../drivers/manifests/zcode.toml"),
            include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
            include_str!("../../../../drivers/manifests/opencode.toml"),
        ] {
            let manifest = ProviderManifest::parse_toml(source).unwrap();
            let desktop = preferred_desktop_runtime(&manifest, &context).await;
            let selected = preferred_executable(&manifest, &context)
                .await
                .unwrap()
                .unwrap();
            if let Some(desktop) = desktop {
                assert_eq!(selected, desktop);
                println!("{}: desktop {}", manifest.id, selected.display());
            } else {
                println!("{}: CLI fallback {}", manifest.id, selected.display());
            }
            let report = agent_registry::probe_installation(&manifest, selected)
                .await
                .unwrap();
            assert_eq!(report.status, agent_registry::InstallationStatus::Installed);
        }
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
        let roots = [dir.path().join(".npm")];
        assert!(cached_deepseek(&roots, Platform::Macos).is_none());
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh"}"#,
        )
        .unwrap();
        assert_eq!(
            cached_deepseek(&roots, Platform::Macos),
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
    fn executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_desktop_runtime_precedes_terminal_and_managed_cli_and_skips_broken_payloads() {
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        // The Windows layout can be probed with private shell fixtures on Unix,
        // exercising the same ordering without touching a real installation.
        for (platform, suffix, prefix) in [
            (
                Platform::Macos,
                "claude.app/Contents/MacOS/claude",
                "Library/Application Support/Claude/claude-code",
            ),
            (
                Platform::Windows,
                "claude.exe",
                "AppData/Roaming/Claude/claude-code",
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let context = DiscoveryContext {
                platform,
                home_dir: Some(home.path().into()),
                path_entries: vec![],
            };
            let standalone = home.path().join(".local/bin/claude");
            executable(&standalone);
            let _managed = crate::agent_updates::managed_cli::fixtures::verified_fixture(
                &context,
                "claude-code",
            );
            assert!(
                crate::agent_updates::managed_cli::candidate(&context, "claude-code").is_some()
            );
            let old = home.path().join(prefix).join("2.1.325/hash").join(suffix);
            let newest = home.path().join(prefix).join("2.1.326/hash").join(suffix);
            for (path, version) in [(&old, "2.1.325"), (&newest, "2.1.326")] {
                executable(path);
                std::fs::write(
                    path,
                    format!("#!/bin/sh\nprintf '%s\\n' '{version} (Claude Code)'\n"),
                )
                .unwrap();
            }
            assert_eq!(
                preferred_executable(&manifest, &context).await.unwrap(),
                Some(newest.canonicalize().unwrap())
            );
            std::fs::write(&newest, "#!/bin/sh\nexit 1\n").unwrap();
            assert_eq!(
                preferred_executable(&manifest, &context).await.unwrap(),
                Some(old.canonicalize().unwrap())
            );
            // A version-looking file with another product marker is not Claude.
            std::fs::write(&old, "#!/bin/sh\nprintf '2.1.325 (Other Agent)\\n'\n").unwrap();
            assert!(
                preferred_desktop_runtime(&manifest, &context)
                    .await
                    .is_none()
            );
            let commands =
                Commands::from([("claude".into(), Resolution::Executable(standalone.clone()))]);
            assert_eq!(
                select_executable(&manifest, &context, Some(&commands)).unwrap(),
                Some(standalone)
            );
        }
    }

    #[test]
    fn windows_deepseek_uses_native_npm_launchers_and_local_appdata_cache() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("迁移主目录/中文 用户");
        let cache = home.join("AppData/Local/npm-cache/_npx/hash/node_modules");
        let package = cache.join("@deepseek-ai/dsh");
        executable(&package.join("lib/bin.js"));
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh"}"#,
        )
        .unwrap();
        let context = DiscoveryContext {
            platform: Platform::Windows,
            path_entries: vec![],
            home_dir: Some(home),
        };
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/deepseek-harness.toml"
        ))
        .unwrap();
        // The JavaScript file alone is not runnable on Windows.
        assert!(
            select_executable(&manifest, &context, None)
                .unwrap()
                .is_none()
        );
        let cached = cache.join(".bin/dsh.cmd");
        executable(&cached);
        assert_eq!(
            select_executable(&manifest, &context, None).unwrap(),
            Some(std::fs::canonicalize(&cached).unwrap())
        );
        let bin = dir.path().join("自定义 npm 安装/bin");
        executable(&bin.join("dsh"));
        let command = bin.join("dsh.cmd");
        executable(&command);
        let context = DiscoveryContext {
            path_entries: vec![bin],
            ..context
        };
        assert_eq!(
            select_executable(&manifest, &context, None).unwrap(),
            Some(std::fs::canonicalize(&command).unwrap())
        );
        assert_eq!(
            agent_registry::discover_executables(&manifest, &context).unwrap(),
            vec![std::fs::canonicalize(command).unwrap()]
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "uses installed Windows DeepSeek/Antigravity CLIs; no model calls"]
    async fn local_windows_deepseek_and_antigravity_discovery_and_readiness() {
        for source in [
            include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
            include_str!("../../../../drivers/manifests/antigravity.toml"),
        ] {
            let manifest = ProviderManifest::parse_toml(source).unwrap();
            let path = preferred_executable(&manifest, &DiscoveryContext::from_current_process())
                .await
                .unwrap()
                .expect("installed CLI should be discovered");
            let report = agent_registry::probe_installation(&manifest, path.clone())
                .await
                .unwrap();
            assert_eq!(
                report.status,
                agent_registry::InstallationStatus::Installed,
                "{report:?}"
            );
            eprintln!("{}: installed {:?}", manifest.id, report.version);
            let result = if manifest.id.as_str() == "antigravity" {
                antigravity_driver::AntigravityDriver::probe_ready(&path).await
            } else {
                acp_driver::AcpDriver::connect_ready(
                    manifest.id.as_str(),
                    &agent_domain::InstallationId::new().to_string(),
                    acp_driver::AcpLaunch {
                        executable: (path).into(),
                        args: manifest.discovery.launch_args,
                    },
                    report.version.map(|version| version.to_string()),
                )
                .await
                .map(drop)
            };
            match result {
                Ok(()) => eprintln!("{}: native readiness passed", manifest.id),
                Err(driver_api::DriverError::NotConfigured(reason)) => {
                    eprintln!(
                        "{}: explicit native account/configuration issue: {reason}",
                        manifest.id
                    );
                }
                Err(error) => panic!("{} readiness failed: {error:?}", manifest.id),
            }
        }
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
    #[tokio::test]
    #[cfg(unix)]
    async fn codex_desktop_remains_discoverable_after_bundle_layout_upgrade() {
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
                std::fs::write(&legacy, "#!/bin/sh\necho codex-cli 0.60.0\n").unwrap();
                assert_eq!(
                    preferred_desktop_runtime(&manifest, &context)
                        .await
                        .or_else(
                            || select_executable(&manifest, &context, Some(&commands)).unwrap()
                        ),
                    Some(std::fs::canonicalize(&legacy).unwrap())
                );
                std::fs::remove_file(legacy).unwrap();
                let current = resources.join("codex-cli/bin/codex");
                executable(&current);
                std::fs::write(&current, "#!/bin/sh\necho codex-cli 0.60.0\n").unwrap();
                assert_eq!(
                    preferred_desktop_runtime(&manifest, &context)
                        .await
                        .or_else(
                            || select_executable(&manifest, &context, Some(&commands)).unwrap()
                        ),
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

    #[tokio::test]
    #[ignore = "requires a local Claude Desktop runtime; version/discovery only, no model turn"]
    async fn current_claude_desktop_runtime_is_preferred() {
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        let context = DiscoveryContext::from_current_process();
        let selected = preferred_executable(&manifest, &context)
            .await
            .unwrap()
            .expect("Claude runtime");
        assert!(
            selected
                .ancestors()
                .any(|path| path.ends_with("Claude/claude-code"))
        );
        println!("selected desktop Claude runtime: {}", selected.display());
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
                    target_id: None,
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
