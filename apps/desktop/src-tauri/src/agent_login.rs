//! Open authentication in the user's native client, never in a background probe.
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum LoginTarget {
    App(PathBuf),
    Cli(PathBuf, &'static [&'static str]),
    /// A CLI the desktop app bundles as a Node script, run by the app's own
    /// Electron binary in node mode: (Electron executable, script, arguments).
    NodeScript(PathBuf, PathBuf, &'static [&'static str]),
}

/// The CLI script a desktop app bundles, from the provider's built-in manifest.
fn bundled_script(provider: &str, executable: &Path) -> Option<PathBuf> {
    let manifest = crate::BUILTIN_AGENT_MANIFESTS
        .iter()
        .filter_map(|source| agent_registry::ProviderManifest::parse_toml(source).ok())
        .find(|manifest| manifest.id.as_str() == provider)?;
    let script = agent_runtime::electron_resources_directory(executable)?
        .join(manifest.discovery.bundled_script?);
    script.is_file().then_some(script)
}

fn login_target(
    provider: &str,
    executable: &Path,
    home: Option<&Path>,
) -> Result<LoginTarget, String> {
    let args: &[&str] = match provider {
        "codex" | "cursor" | "grok" | "kimi" | "kiro" | "qoder-cn" => &["login"],
        "claude-code" | "opencode" => &["auth", "login"],
        "antigravity" | "zcode" | "codebuddy" | "pi" | "deepseek-harness" => &[],
        _ => return Err("backendText.mb577082e".into()),
    };
    if !executable.is_absolute() || !executable.is_file() {
        return Err("backendText.m6142282c".into());
    }
    // Signing in to the Qoder CN app does not sign in its bundled CLI, so
    // its own login runs in a terminal like any other CLI.
    if provider == "qoder-cn" {
        let script = bundled_script(provider, executable).ok_or("backendText.m6142282c")?;
        return Ok(LoginTarget::NodeScript(executable.into(), script, args));
    }
    // Only GUI providers should open their app bundle. Codex/OpenCode can ship
    // a CLI inside an app but still need to authenticate that exact CLI.
    if matches!(provider, "zcode" | "antigravity") {
        let canonical = executable
            .canonicalize()
            .map_err(|_| "backendText.mfc624e76")?;
        if let Some(bundle) = canonical
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
        {
            return Ok(LoginTarget::App(bundle.into()));
        }
        if provider == "antigravity" {
            let candidates = home
                .map(|h| h.join("Applications/Antigravity.app"))
                .into_iter()
                .chain([PathBuf::from("/Applications/Antigravity.app")]);
            if let Some(bundle) = candidates.into_iter().find(|p| p.is_dir()) {
                return Ok(LoginTarget::App(bundle));
            }
        }
    }
    Ok(LoginTarget::Cli(executable.into(), args))
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn cli_command(executable: &Path, args: &[&str]) -> Result<String, String> {
    let path = executable.to_str().ok_or("backendText.ma627abbe")?;
    // Terminal commands may be entered in different shells; reject control
    // characters and quote every argument, including spaces and shell syntax.
    if path.chars().any(char::is_control) {
        return Err("backendText.m8a8abc7a".into());
    }
    Ok(std::iter::once(path)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" "))
}

/// The terminal command line for a CLI login, in POSIX shell syntax.
#[cfg(unix)]
fn unix_login_command(target: LoginTarget) -> Result<String, String> {
    match target {
        LoginTarget::Cli(executable, args) => cli_command(&executable, args),
        LoginTarget::NodeScript(executable, script, args) => {
            // On macOS this is the background helper, so no Dock icon appears.
            let node = agent_runtime::electron_node_executable(&executable)
                .map_err(|_| "backendText.m6142282c")?;
            let script = script.to_str().ok_or("backendText.ma627abbe")?;
            if script.chars().any(char::is_control) {
                return Err("backendText.m8a8abc7a".into());
            }
            let arguments: Vec<&str> = std::iter::once(script)
                .chain(args.iter().copied())
                .collect();
            // `env` keeps the assignment valid in bash, zsh, sh and fish alike.
            Ok(format!(
                "env ELECTRON_RUN_AS_NODE=1 {}",
                cli_command(&node, &arguments)?
            ))
        }
        LoginTarget::App(_) => Err("backendText.m63ce5eae".into()),
    }
}

// Linux spawns its terminal directly and never waits on a launcher.
#[cfg(any(target_os = "macos", windows))]
async fn launch(mut command: tokio::process::Command) -> Result<(), String> {
    command.kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(15), command.status())
        .await
        .map_err(|_| "backendText.m9b9bea62")?
        .map_err(|_| "backendText.m13537f48")?;
    if status.success() {
        Ok(())
    } else {
        Err("backendText.m8e99e9a6".into())
    }
}

#[cfg(target_os = "macos")]
async fn open_target(target: LoginTarget) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let mut open = tokio::process::Command::new("/usr/bin/open");
    match target {
        LoginTarget::App(bundle) => {
            open.arg("-a").arg(bundle);
            launch(open).await
        }
        target @ (LoginTarget::Cli(..) | LoginTarget::NodeScript(..)) => {
            let command = unix_login_command(target)?;
            let context = agent_registry::DiscoveryContext::from_current_process();
            let shell = crate::terminal_installation::user_shell(&context)
                .await
                .filter(|p| {
                    matches!(
                        p.file_name().and_then(|n| n.to_str()),
                        Some("bash" | "zsh" | "sh" | "fish")
                    )
                })
                .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
            // A .command file opens Terminal without Apple Events permissions.
            // Start a login shell so Node-based CLIs inherit the user's PATH.
            let mut script = tempfile::Builder::new()
                .prefix("dogcodes-login-")
                .suffix(".command")
                .tempfile()
                .map_err(|_| "backendText.m18b6ac8c")?;
            writeln!(script, "#!/bin/sh\n/bin/rm -f -- \"$0\"\ncd \"$HOME\" || exit 1\n{} -lic {}\nprintf '\\nLogin finished. Return to dog.codes and detect again.\\n'",
                shell_quote(&shell.to_string_lossy()), shell_quote(&command))
                .map_err(|_| "backendText.m94803045")?;
            script
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "backendText.mdc9f649a")?;
            let (_, path) = script.keep().map_err(|_| "backendText.m1c64d5bb")?;
            open.arg("-a").arg("Terminal").arg(&path);
            let result = launch(open).await;
            if result.is_err() {
                let _ = std::fs::remove_file(path);
            }
            result
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
// Same async signature as the macOS and Windows variants the caller awaits.
#[allow(clippy::unused_async)]
async fn open_target(target: LoginTarget) -> Result<(), String> {
    let command = unix_login_command(target)?;
    let script = format!(
        "cd \"$HOME\" && {command}; printf '\\nLogin finished. Return to dog.codes and detect again. Press Enter to close.'; read reply"
    );
    for (terminal, flag) in [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xterm", "-e"),
    ] {
        let mut launcher = tokio::process::Command::new(terminal);
        launcher.args([flag, "/bin/sh", "-lc", &script]);
        if launcher.spawn().is_ok() {
            return Ok(());
        }
    }
    Err("backendText.m5ab1e7dc".into())
}

#[cfg(windows)]
fn windows_cli_command(executable: &Path, args: &[&str]) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
    std::iter::once(
        agent_runtime::external_path(executable)
            .to_string_lossy()
            .into_owned(),
    )
    .chain(args.iter().map(ToString::to_string))
    .map(|s| quote(&s))
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(windows)]
async fn open_target(target: LoginTarget) -> Result<(), String> {
    use base64::Engine;
    let (environment, command) = match target {
        LoginTarget::Cli(executable, args) => ("", windows_cli_command(&executable, args)),
        LoginTarget::NodeScript(executable, script, args) => {
            let script = script.to_string_lossy();
            let arguments: Vec<&str> = std::iter::once(&*script)
                .chain(args.iter().copied())
                .collect();
            (
                "$env:ELECTRON_RUN_AS_NODE='1'; ",
                windows_cli_command(&executable, &arguments),
            )
        }
        LoginTarget::App(_) => return Err("backendText.m63ce5eae".into()),
    };
    let script = format!("Set-Location $HOME; {environment}& {command}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    // Only the login terminal started below should be visible, not this launcher.
    let mut launcher = agent_runtime::background_command("powershell.exe");
    launcher.args([
        "-NoProfile",
        "-Command",
        &format!(
            "Start-Process powershell.exe -ArgumentList '-NoExit','-EncodedCommand','{encoded}'"
        ),
    ]);
    launch(launcher).await
}

#[tauri::command]
pub async fn open_agent_login(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::DesktopState>,
    installation_id: String,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    // The renderer provides only an ID, never a path or an arbitrary command.
    let installation = state
        .core
        .bootstrap()
        .await?
        .installations
        .into_iter()
        // A path on a server means nothing to this machine's terminal.
        .find(|i| i.installation_id.to_string() == installation_id && i.target_id.is_none())
        .ok_or("backendText.updateInstallationGone")?;
    if installation.version.as_deref() == Some("demo") {
        return Err("backendText.me015b290".into());
    }
    let context = agent_registry::DiscoveryContext::from_current_process();
    let target = login_target(
        installation.provider_id.as_str(),
        Path::new(&installation.executable),
        context.home_dir.as_deref(),
    )?;
    open_target(target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_login_quotes_a_plain_path_without_changing_the_selected_installation() {
        let identity = Path::new(r"\\?\C:\用户\Agent's directory\agent.cmd");
        assert_eq!(
            windows_cli_command(identity, &["auth", "login"]),
            r"'C:\用户\Agent''s directory\agent.cmd' 'auth' 'login'"
        );
        assert!(identity.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn routes_native_login_commands_without_protocol_arguments() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for (provider, args) in [
            ("codex", vec!["login"]),
            ("grok", vec!["login"]),
            ("cursor", vec!["login"]),
            ("kimi", vec!["login"]),
            ("kiro", vec!["login"]),
            ("claude-code", vec!["auth", "login"]),
            ("opencode", vec!["auth", "login"]),
            ("codebuddy", vec![]),
            ("pi", vec![]),
            ("deepseek-harness", vec![]),
        ] {
            let LoginTarget::Cli(path, actual_args) =
                login_target(provider, file.path(), None).unwrap()
            else {
                panic!("CLI provider must open a terminal");
            };
            assert_eq!(path, file.path());
            assert_eq!(actual_args, args);
        }
        assert!(login_target("unknown", file.path(), None).is_err());
        assert!(login_target("grok", Path::new("/missing/grok"), None).is_err());
        assert!(login_target("grok", Path::new("grok"), None).is_err());
    }

    #[test]
    fn qoder_cn_signs_in_its_bundled_cli_through_electron_node_mode() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("Qoder CN/Qoder CN.exe");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "").unwrap();
        // Without the bundled runtime there is nothing to sign in.
        assert!(login_target("qoder-cn", &executable, None).is_err());
        let script = agent_runtime::electron_resources_directory(&executable)
            .unwrap()
            .join("app.asar.unpacked/node_modules/@qoder-ai/qoder-cn-agent-sdk/dist/_worker/qoder-worker-runtime.obf.mjs");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "").unwrap();
        assert_eq!(
            login_target("qoder-cn", &executable, None).unwrap(),
            LoginTarget::NodeScript(executable.clone(), script.clone(), &["login"])
        );
        #[cfg(unix)]
        assert_eq!(
            unix_login_command(LoginTarget::NodeScript(
                "/Apps/Qoder CN".into(),
                "/Apps/resources/cli.mjs".into(),
                &["login"]
            ))
            .unwrap(),
            "env ELECTRON_RUN_AS_NODE=1 '/Apps/Qoder CN' '/Apps/resources/cli.mjs' 'login'"
        );
    }

    #[test]
    fn opens_gui_bundle_but_keeps_bundled_codex_cli() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("ZCode.app");
        let executable = bundle.join("Contents/MacOS/ZCode");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "").unwrap();
        assert_eq!(
            login_target("zcode", &executable, None).unwrap(),
            LoginTarget::App(bundle.canonicalize().unwrap())
        );
        assert_eq!(
            login_target("codex", &executable, None).unwrap(),
            LoginTarget::Cli(executable, &["login"])
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminal_command_preserves_paths_without_evaluating_shell_syntax() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("agent ' $(exit 41) `exit 42` \\");
        std::fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &cli_command(&path, &["auth", "login"]).unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"auth\nlogin\n");
        assert!(cli_command(Path::new("/tmp/agent\nexit"), &[]).is_err());
    }
}
