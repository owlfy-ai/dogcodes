//! Explicit opt-in, isolated dual-root cancellation and native reattachment acceptance.
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionHandle, SessionRequest, SessionResumeCapability,
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
        "kimi" | "grok" | "cursor" => {
            let (path, args) = match name {
                "kimi" => (".kimi-code/bin/kimi", vec!["acp"]),
                "grok" => (
                    ".local/bin/grok",
                    vec!["--permission-mode", "default", "agent", "stdio"],
                ),
                _ => (".local/bin/cursor-agent", vec!["acp"]),
            };
            let launch = acp_driver::AcpLaunch {
                executable: PathBuf::from(&home).join(path),
                args: args.into_iter().map(String::from).collect(),
            };
            let report = acp_driver::AcpDriver::probe_ready(&launch).await?;
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
fn request(scope: &SessionRequest, text: &str) -> PromptRequest {
    PromptRequest {
        attachments: vec![],
        permission_mode: None,
        working_directory: scope.working_directory.clone(),
        additional_directories: scope.additional_directories.clone(),
        context: vec![],
        prompt: text.into(),
    }
}
async fn completed(
    driver: &dyn AgentDriver,
    session: &SessionHandle,
    scope: &SessionRequest,
    text: &str,
) -> Result<(String, Option<String>), Error> {
    let mut stream = driver.prompt(session, request(scope, text)).await?;
    let result = tokio::time::timeout(Duration::from_mins(2), async {
        let mut answer = String::new();
        let mut durable_id = None;
        while let Some(event) = stream.next().await {
            match event? {
                DriverEvent::TextDelta { text, .. } => answer.push_str(&text),
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                    if let Some(text) = data["textDelta"].as_str() {
                        answer.push_str(text);
                    }
                }
                DriverEvent::SessionReady { native_session_id } => {
                    durable_id = Some(native_session_id);
                }
                DriverEvent::TurnCompleted { .. } => return Ok((answer, durable_id)),
                DriverEvent::ApprovalRequested { .. } => {
                    return Err("unexpected approval for no-tool prompt".into());
                }
                DriverEvent::TurnQuotaExhausted { .. } => return Err("quota_exhausted".into()),
                DriverEvent::TurnFailed { reason } => {
                    let category = if reason == "ZCode turn ended with an error result" {
                        "native_failure_error_result"
                    } else {
                        "native_turn_failed"
                    };
                    return Err(category.into());
                }
                DriverEvent::TurnInterrupted { .. } => {
                    return Err("unexpected_native_interruption".into());
                }
                _ => (),
            }
        }
        Err("stream ended without terminal".into())
    })
    .await;
    if let Ok(value) = result {
        value
    } else {
        let _ = driver.interrupt(session).await;
        Err("turn timed out".into())
    }
}
async fn cancel_case(
    driver: &dyn AgentDriver,
    session: &SessionHandle,
    scope: &SessionRequest,
    approval: bool,
) -> Result<String, Error> {
    let prompt = if approval {
        format!(
            "Isolated approval cancellation test. Use a shell tool to write exactly the text DOGCODES_CANCEL_TEST into {}. Request normal approval if required. Do not bypass restrictions or access any other paths.",
            PathBuf::from(&scope.additional_directories[0])
                .join("approval.txt")
                .display()
        )
    } else {
        "Do not use tools. Explain Rust ownership in 3000 words, starting with a numbered list."
            .into()
    };
    let mut stream = driver.prompt(session, request(scope, &prompt)).await?;
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        let mut sent = false;
        while let Some(event) = stream.next().await {
            match event? {
                DriverEvent::ApprovalRequested { .. } if !sent => {
                    driver.interrupt(session).await?;
                    sent = true;
                }
                DriverEvent::TextDelta { .. } | DriverEvent::Presentation { .. }
                    if !approval && !sent =>
                {
                    driver.interrupt(session).await?;
                    sent = true;
                }
                DriverEvent::TurnInterrupted { .. } if sent => return Ok("interrupted".to_owned()),
                DriverEvent::TurnCompleted { .. } => {
                    return Ok(if sent {
                        "completed_after_cancel"
                    } else {
                        "no_approval_observed"
                    }
                    .into());
                }
                DriverEvent::TurnQuotaExhausted { .. } => return Err("quota_exhausted".into()),
                DriverEvent::TurnFailed { .. } => return Err("native_failure".into()),
                _ => (),
            }
        }
        Err("stream ended without terminal".into())
    })
    .await;
    if let Ok(value) = result {
        value
    } else {
        let _ = driver.interrupt(session).await;
        Err("cancel timed out".into())
    }
}
fn main() -> Result<(), Error> {
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}
async fn run() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 || args[1] != "--allow-model-usage" {
        return Err("usage: live_recovery_matrix --allow-model-usage PROVIDER (up to four isolated real turns)".into());
    }
    let name = &args[2];
    let temp = tempfile::tempdir()?;
    let primary = temp.path().join("primary");
    let secondary = temp.path().join("secondary");
    std::fs::create_dir(&primary)?;
    std::fs::create_dir(&secondary)?;
    let scope = SessionRequest {
        app_mcps: vec![],
        permission_mode: None,
        working_directory: primary.to_string_lossy().into_owned(),
        additional_directories: vec![secondary.to_string_lossy().into_owned()],
    };
    let installation = agent_domain::InstallationId::new().to_string();
    let first = driver(name, &installation).await?;
    let session = first.create_session(scope.clone()).await?;
    let mut durable_id = session.native_session_id.clone();
    let run=async {
        let answer=completed(first.as_ref(),&session,&scope,"Do not use tools. Remember the conversation-only marker RECOVERY-ORCHID-731. Reply ACK.").await?;
        if let Some(id) = answer.1 { durable_id = id; }
        if answer.0.is_empty(){return Err("empty first response".into());} println!("{name}: dual-root session created, first turn completed");
        println!("{name}: active cancellation = {}",cancel_case(first.as_ref(),&session,&scope,false).await?);
        Ok::<_,Error>(())
    }.await;
    first.shutdown(session.clone()).await?;
    run?;
    drop(first);
    let second = driver(name, &installation).await?;
    let resumed = second
        .resume_selected_session(&durable_id, scope.clone(), None, None)
        .await?;
    let run=async {
        let answer=completed(second.as_ref(),&resumed,&scope,"Do not use tools. State the conversation-only marker from before the interrupted turn.").await?;
        if !answer.0.contains("RECOVERY-ORCHID-731"){return Err("native reattachment did not recall marker".into());}
        println!("{name}: new driver reattached and recalled marker = PASS");
        let outcome=cancel_case(second.as_ref(),&resumed,&scope,true).await?;
        let written=secondary.join("approval.txt").exists();
        println!("{name}: pending-approval cancellation = {outcome}; secondary_file_exists={written}");
        if outcome=="interrupted" && written {return Err("approval cancellation unexpectedly wrote target file".into());}
        Ok::<_,Error>(())
    }.await;
    second.shutdown(resumed).await?;
    run
}
