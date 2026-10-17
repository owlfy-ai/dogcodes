//! Explicit opt-in native attachment acceptance using disposable files and a bundled icon.
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;
use std::{path::PathBuf, sync::Arc, time::Duration};
type Error = Box<dyn std::error::Error>;
async fn driver(name: &str, installation: &str) -> Result<Arc<dyn AgentDriver>, Error> {
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
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn is_fixture_cat(value: &serde_json::Value, file: &std::path::Path) -> bool {
    let Some(command) = value.as_str() else {
        return false;
    };
    let path = file.to_string_lossy();
    for path in [
        path.as_ref(),
        path.strip_prefix("/private").unwrap_or(&path),
    ] {
        for executable in ["/bin/cat", "cat"] {
            for argument in [quoted(path), format!("\"{path}\"")] {
                let script = format!("{executable} {argument}");
                if command == script {
                    return true;
                }
                for shell in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
                    if command == format!("{shell} -lc {}", quoted(&script))
                        || command == format!("{shell} -lc \"{}\"", script.replace('\"', "\\\""))
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}
fn main() -> Result<(), Error> {
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}
async fn run() -> Result<(), Error> {
    let name = std::env::args()
        .nth(1)
        .ok_or("pass the agent name explicitly")?;
    let driver = driver(&name, &agent_domain::InstallationId::new().to_string()).await?;
    let workspace = tempfile::tempdir()?;
    let attachments = tempfile::tempdir()?;
    let cwd = std::fs::canonicalize(workspace.path())?
        .to_string_lossy()
        .into_owned();
    let directory = std::fs::canonicalize(attachments.path())?;
    let image = directory.join("sample.png");
    std::fs::write(
        &image,
        include_bytes!("../../../apps/desktop/src/assets/kimi.png"),
    )?;
    let file = directory.join("reference #1.txt");
    let nonce = agent_domain::InstallationId::new().to_string();
    std::fs::write(&file, format!("ATTACHMENT_TEST_TOKEN={nonce}"))?;
    let scope = SessionRequest {
        history_mcp: None,
        working_directory: cwd.clone(),
        additional_directories: vec![directory.to_string_lossy().into()],
        permission_mode: if name == "antigravity" {
            Some("desktop-approvals".into())
        } else {
            None
        },
    };
    eprintln!("stage=create_session");
    let session = driver
        .create_selected_session(scope.clone(), None, None)
        .await?;
    eprintln!("stage=prompt");
    let result = tokio::time::timeout(Duration::from_mins(2), async {
        let mut stream = driver.prompt(&session, PromptRequest {
            working_directory: cwd, additional_directories: scope.additional_directories,
            permission_mode: scope.permission_mode, context: vec![],
            prompt: "Read the two user attachments. Describe the image's large letter and background color. Read the attached text file and repeat its exact ATTACHMENT_TEST_TOKEN. Use native read tools if needed. If no file reader exists, a read-only /bin/cat command for the attached text file is allowed. Do not execute other commands, edit files, or delegate.".into(),
            attachments: vec![
                agent_domain::PromptAttachment { integrity: None, name:"sample.png".into(), path:image.to_string_lossy().into(), mime_type:"image/png".into() },
                agent_domain::PromptAttachment { integrity: None, name:"reference #1.txt".into(), path:file.to_string_lossy().into(), mime_type:"text/plain".into() },
            ],
        }).await?;
        let mut answer = String::new();
        let mut completed = false;
        while let Some(event) = stream.next().await {
            match event? {
                DriverEvent::TextDelta { text, .. } => answer.push_str(&text),
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                    for key in ["text", "textDelta"] { if let Some(text) = data[key].as_str() { answer.push_str(text); } }
                }
                DriverEvent::TurnInterrupted { summary } => eprintln!("interrupted={summary}"),
                DriverEvent::TurnCompleted { .. } => completed = true,
                DriverEvent::TurnFailed { reason } => return Err::<(), Error>(reason.into()),
                DriverEvent::ApprovalRequested { native_request_id, tool_name, command_summary, options } => {
                    let tool = tool_name.to_lowercase();
                    let input = serde_json::from_str::<serde_json::Value>(&command_summary).unwrap_or_default();
                    let path_only_read = input.as_object().is_some_and(|object| {
                        object.keys().all(|key| matches!(key.as_str(), "file_path" | "path" | "offset" | "limit" | "start_line" | "end_line"))
                            && ["file_path", "path"].iter().any(|key| input[*key].as_str().is_some_and(|path| {
                                let Ok(path) = std::fs::canonicalize(path) else { return false; };
                                if path == file { return true; }
                                // CodeBuddy stages our inline image in its own clipboard cache and asks to read it.
                                // Approve only the exact bytes sent by this test, never an arbitrary cached image.
                                name == "codebuddy" && path.parent().is_some_and(|p| p.ends_with("workbuddy-clipboard-images"))
                                    && std::fs::metadata(&path).is_ok_and(|m| m.len() == include_bytes!("../../../apps/desktop/src/assets/kimi.png").len() as u64)
                                    && std::fs::read(path).is_ok_and(|bytes| bytes == include_bytes!("../../../apps/desktop/src/assets/kimi.png"))
                            }))
                    });
                    let safe = ((tool.contains("read") || tool.contains("view_file")) && command_summary.contains(directory.to_str().unwrap())) || path_only_read || (tool == "commandexecution" && is_fixture_cat(&input, &file));
                    eprintln!("approval_keys={:?} fixture_reference={}", input.as_object().map(|o| o.keys().collect::<Vec<_>>()), path_only_read);
                    let role = if safe { driver_api::ApprovalSemanticRole::ApproveOnce } else { driver_api::ApprovalSemanticRole::Reject };
                    eprintln!("approval_tool={tool_name} allow_fixture_read={safe}");
                    let option = options.iter().find(|o| o.semantic_role == role).ok_or("no matching option")?;
                    driver.respond_to_interaction(&session, &native_request_id, &option.option_id).await?;
                }
                _ => {}
            }
        }
        println!("agent={name} completed={completed} file_reference={} image_description={}\n{answer}", answer.contains(&nonce), answer.contains('K') && (answer.to_lowercase().contains("black") || answer.contains("黑")));
        if !completed || !answer.contains(&nonce) || !answer.contains('K') || !(answer.to_lowercase().contains("black") || answer.contains("黑")) { return Err("attachment recognition incomplete".into()); }
        Ok(())
    }).await;
    driver.shutdown(session).await?;
    result??;
    Ok(())
}
