use driver_api::{AgentDriver, SessionResumeCapability};
use std::{path::PathBuf, sync::Arc};
type Error = Box<dyn std::error::Error>;
pub async fn driver(name: &str, installation: &str) -> Result<Arc<dyn AgentDriver>, Error> {
    let home = std::env::var("HOME")?;
    Ok(match name {
        "codex" => {
            let path = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex");
            let out = std::process::Command::new(&path)
                .arg("--version")
                .output()?;
            let text = String::from_utf8(out.stdout)?;
            Arc::new(codex_driver::CodexDriver::new(
                &path,
                installation.into(),
                text.trim()
                    .strip_prefix("codex-cli ")
                    .ok_or("version unavailable")?
                    .into(),
            )?)
        }
        "kimi" | "grok" | "cursor" | "codebuddy" => {
            let (path, args) = match name {
                "kimi" => (".kimi-code/bin/kimi", vec!["acp"]),
                "grok" => (".local/bin/grok", vec!["agent", "--no-leader", "stdio"]),
                "codebuddy" => (
                    ".nvm-arm/versions/node/v22.23.2/bin/codebuddy",
                    vec!["--acp"],
                ),
                _ => (".local/bin/cursor-agent", vec!["acp"]),
            };
            let launch = acp_driver::AcpLaunch {
                executable: PathBuf::from(&home).join(path),
                args: args.into_iter().map(String::from).collect(),
            };
            let mut report = acp_driver::AcpDriver::probe_ready(&launch).await?;
            if report.native_version.is_none() {
                let output = std::process::Command::new(&launch.executable)
                    .arg("--version")
                    .output()?;
                report.native_version = Some(String::from_utf8(output.stdout)?.trim().to_owned());
            }
            eprintln!("stage=probe_ready version={:?}", report.native_version);
            Arc::new(acp_driver::AcpDriver::new(
                name,
                installation,
                launch,
                report.native_version,
                report.resume_capability,
            )?)
        }
        "claude-code" => Arc::new(claude_driver::ClaudeDriver::new(
            name,
            installation,
            PathBuf::from(&home).join(".local/bin/claude"),
            None,
            SessionResumeCapability::Supported,
        )?),
        "antigravity" => Arc::new(antigravity_driver::AntigravityDriver::new(
            name,
            installation,
            PathBuf::from(&home).join(".local/bin/agy"),
            None,
            SessionResumeCapability::Supported,
        )?),
        "zcode" => Arc::new(zcode_driver::ZcodeDriver::new(
            name,
            installation,
            zcode_driver::ZcodeLaunch {
                app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
                cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
            },
            None,
            SessionResumeCapability::Supported,
        )?),
        _ => return Err("unknown provider".into()),
    })
}
