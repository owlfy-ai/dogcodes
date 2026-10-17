//! Explicit opt-in native question acceptance. Uses disposable sessions and no file operations.
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
                executable: if name == "cursor" {
                    std::env::var("DOGCODES_CURSOR_TEST_BINARY")
                        .map_or_else(|_| PathBuf::from(&home).join(path), PathBuf::from)
                } else {
                    PathBuf::from(&home).join(path)
                },
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
fn main() -> Result<(), Error> {
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}
async fn run() -> Result<(), Error> {
    let name = std::env::args().nth(1).ok_or("provider required")?;
    let cancel = std::env::args().nth(2).as_deref() == Some("cancel");
    let d = driver(&name, &agent_domain::InstallationId::new().to_string()).await?;
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().to_string_lossy().to_string();
    let mode = if name == "antigravity" {
        Some("desktop-approvals".into())
    } else {
        None
    };
    let session = d
        .create_selected_session(
            SessionRequest {
                history_mcp: None,
                working_directory: cwd.clone(),
                additional_directories: vec![],
                permission_mode: mode.clone(),
            },
            None,
            None,
        )
        .await?;
    let nonce = format!("Cobalt-release-{}", agent_domain::InstallationId::new());
    let outcome=tokio::time::timeout(Duration::from_secs(150),async {
        let mut stream=d.prompt(&session,PromptRequest{attachments:vec![],working_directory:cwd,additional_directories:vec![],permission_mode:mode,context:vec![],prompt:"This is an interactive question integration test. Use your native structured ask-user/question tool now to ask exactly one question: Which release label should I use? Offer two options: Amber and Violet, and allow a custom answer if supported. Wait for the tool response, then repeat the exact user's answer in your final reply. If the user cancels, acknowledge the cancellation and finish; do not ask again. Do not read or write files, run commands, search, or delegate. You MUST invoke the native question tool, not merely ask in text.".into()}).await?;
        let mut asked=0;let mut text=String::new();let mut complete=false;let mut expected=String::new();
        while let Some(event)=stream.next().await {
            match event?{
                DriverEvent::QuestionRequested{native_request_id,form}=>{
                    asked+=1;if asked>2{return Err("Repeated question after answer".into());}
                    eprintln!("question form={}",serde_json::to_string(&form)?);
                    let props=form.schema["properties"].as_object().ok_or("No question fields")?;
                    let mut values=serde_json::Map::new();
                    for (key,prop) in props {
                        let item=if prop["type"]=="array"{&prop["items"]}else{prop};
                        let options=item["oneOf"].as_array().or_else(||item["anyOf"].as_array());
                        let value=if let Some(options)=options{options.first().ok_or("empty options")?["const"].clone()}else if let Some(options)=item["enum"].as_array(){options.first().ok_or("empty options")?.clone()}else{serde_json::json!(nonce)};
                        expected=item["x-options"].as_array().and_then(|opts|opts.iter().find(|o|o["const"]==value)).and_then(|o|o["title"].as_str()).or_else(||value.as_str()).unwrap_or("").into();
                        values.insert(key.clone(),if prop["type"]=="array"{serde_json::json!([value])}else{value});
                    }
                    d.respond_to_question(&session,&native_request_id,agent_domain::QuestionAnswer{cancelled:cancel,values:values.into()}).await?;
                    eprintln!("answer delivered cancelled={cancel}");
                }
                DriverEvent::ApprovalRequested{native_request_id,tool_name,command_summary,options}=>{
                    eprintln!("unexpected approval tool={tool_name} summary={command_summary}");
                    let option=options.iter().find(|o|o.semantic_role==driver_api::ApprovalSemanticRole::Reject).ok_or("No deny option")?;
                    d.respond_to_interaction(&session,&native_request_id,&option.option_id).await?;
                }
                DriverEvent::TextDelta{text:t,..}=>text.push_str(&t),
                DriverEvent::Presentation{data,..}=>{for k in ["text","textDelta"]{if let Some(t)=data[k].as_str(){text.push_str(t);}}}
                DriverEvent::TurnCompleted{..}=>complete=true,
                DriverEvent::TurnFailed{reason}=>return Err(format!("Failed: {reason}").into()),
                DriverEvent::ProtocolWarning{message}=>eprintln!("warning: {message}"),
                _=>{}
            }
        }
        eprintln!("final={text}");
        if asked==0{return Err("No structured question received".into());}
        if !complete{return Err("Turn did not complete".into());}
        if !cancel && !text.contains(&expected){return Err("Final reply does not contain returned answer".into());}
        println!("PASS provider={name} cancel={cancel} questions={asked} completed={complete}");
        Ok::<(),Error>(())
    }).await;
    let _ = d.shutdown(session).await;
    outcome??;
    Ok(())
}
