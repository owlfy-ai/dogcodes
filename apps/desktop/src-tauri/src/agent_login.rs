//! Open authentication in the user's native client, never in a background probe.
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum LoginTarget {
    App(PathBuf),
    Cli(PathBuf, &'static [&'static str]),
}

fn login_target(
    provider: &str,
    executable: &Path,
    home: Option<&Path>,
) -> Result<LoginTarget, String> {
    let args: &[&str] = match provider {
        "codex" | "cursor" | "grok" | "kimi" | "kiro" => &["login"],
        "claude-code" | "opencode" => &["auth", "login"],
        "antigravity" | "zcode" | "codebuddy" | "pi" | "deepseek-harness" => &[],
        _ => return Err("此 Agent 暂不支持快捷登录".into()),
    };
    if !executable.is_absolute() || !executable.is_file() {
        return Err("此 Agent 的安装位置已失效，请重新检测".into());
    }
    // Only GUI providers should open their app bundle. Codex/OpenCode can ship
    // a CLI inside an app but still need to authenticate that exact CLI.
    if matches!(provider, "zcode" | "antigravity") {
        let canonical = executable
            .canonicalize()
            .map_err(|_| "无法读取客户端安装位置")?;
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
    let path = executable.to_str().ok_or("客户端安装路径编码无效")?;
    // Terminal commands may be entered in different shells; reject control
    // characters and quote every argument, including spaces and shell syntax.
    if path.chars().any(char::is_control) {
        return Err("客户端安装路径包含无效字符".into());
    }
    Ok(std::iter::once(path)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" "))
}

// Linux spawns its terminal directly and never waits on a launcher.
#[cfg(any(target_os = "macos", windows))]
async fn launch(mut command: tokio::process::Command) -> Result<(), String> {
    command.kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(15), command.status())
        .await
        .map_err(|_| "打开登录入口超时，请重试")?
        .map_err(|_| "无法打开登录入口，请检查终端或客户端是否可用")?;
    if status.success() {
        Ok(())
    } else {
        Err("无法打开登录入口，请手动打开客户端或重试".into())
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
        LoginTarget::Cli(executable, args) => {
            let command = cli_command(&executable, args)?;
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
                .map_err(|_| "无法创建登录启动文件")?;
            writeln!(script, "#!/bin/sh\n/bin/rm -f -- \"$0\"\ncd \"$HOME\" || exit 1\n{} -lic {}\nprintf '\\n登录操作结束，请返回 dogcodes 重新检测。\\n'",
                shell_quote(&shell.to_string_lossy()), shell_quote(&command))
                .map_err(|_| "无法写入登录启动文件")?;
            script
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "无法设置登录启动文件权限")?;
            let (_, path) = script.keep().map_err(|_| "无法保存登录启动文件")?;
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
    let LoginTarget::Cli(executable, args) = target else {
        return Err("此客户端需要在 macOS 中打开".into());
    };
    let command = cli_command(&executable, args)?;
    let script = format!(
        "cd \"$HOME\" && {command}; printf '\\n登录操作结束，请返回 dogcodes 重新检测。按回车关闭。'; read reply"
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
    Err("未找到可用终端，请手动打开客户端登录".into())
}

#[cfg(windows)]
async fn open_target(target: LoginTarget) -> Result<(), String> {
    use base64::Engine;
    let LoginTarget::Cli(executable, args) = target else {
        return Err("此客户端需要在 macOS 中打开".into());
    };
    let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
    let command = std::iter::once(executable.to_string_lossy().into_owned())
        .chain(args.iter().map(ToString::to_string))
        .map(|s| quote(&s))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!("Set-Location $HOME; & {command}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut launcher = tokio::process::Command::new("powershell.exe");
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
        .find(|i| i.installation_id.to_string() == installation_id)
        .ok_or("此 Agent 安装已不存在，请重新检测")?;
    if installation.version.as_deref() == Some("demo") {
        return Err("演示 Agent 不支持本机登录".into());
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
