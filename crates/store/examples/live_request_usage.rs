//! Opt-in paid, two-turn telemetry acceptance through the production drivers.
//! Usage: `live_request_usage PROVIDER OUTPUT_DIRECTORY [EXECUTABLE] [MODEL] [multi|resume]`
//! `OpenCode` requires EXECUTABLE: its standalone and desktop CLIs may differ.
//! Only usage and status are saved. `multi` reads isolated fixture files with native tools.
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, QuotaProvider, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
type Error = Box<dyn std::error::Error>;
type Drivers = (Arc<dyn AgentDriver>, Arc<dyn QuotaProvider>);
fn pair<T: AgentDriver + QuotaProvider + 'static>(driver: T) -> Drivers {
    let driver = Arc::new(driver);
    (driver.clone(), driver)
}
fn driver(provider: &str, override_path: Option<String>) -> Result<Drivers, Error> {
    let home = PathBuf::from(std::env::var("HOME")?);
    let id = agent_domain::InstallationId::new().to_string();
    let resume = SessionResumeCapability::Supported;
    let path = |relative: &str| {
        override_path
            .as_ref()
            .map_or_else(|| home.join(relative), PathBuf::from)
    };
    Ok(match provider {
        "codex" => pair(codex_driver::CodexDriver::new(
            &path(
                "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            ),
            id,
            "live-probe".into(),
        )?),
        "claude-code" => pair(claude_driver::ClaudeDriver::new(
            provider,
            &id,
            path(".local/bin/claude"),
            None,
            resume,
        )?),
        "pi" => pair(pi_driver::PiDriver::new(&id, path(".local/bin/pi"), None)?),
        "antigravity" => pair(antigravity_driver::AntigravityDriver::new(
            provider,
            &id,
            path(".local/bin/agy"),
            None,
            resume,
        )?),
        "zcode" => pair(zcode_driver::ZcodeDriver::new(
            provider,
            &id,
            zcode_driver::ZcodeLaunch {
                app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
                cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
            },
            None,
            resume,
        )?),
        "kimi" | "grok" | "cursor" | "codebuddy" | "opencode" | "deepseek-harness" | "kiro" => {
            let (binary, args) = match provider {
                "kimi" => (".kimi-code/bin/kimi", vec!["acp"]),
                "grok" => (".local/bin/grok", vec!["agent", "--no-leader", "stdio"]),
                "codebuddy" => (
                    ".nvm-arm/versions/node/v22.23.2/bin/codebuddy",
                    vec!["--acp"],
                ),
                "cursor" => (".local/bin/cursor-agent", vec!["acp"]),
                "opencode" => (".opencode/bin/opencode", vec!["acp"]),
                "kiro" => (
                    ".local/bin/kiro-cli",
                    vec!["acp", "--agent-engine", "v3", "--auth-method", "cli"],
                ),
                _ => (".local/bin/dsh", vec!["--profile", "acp"]),
            };
            pair(acp_driver::AcpDriver::new(
                provider,
                &id,
                acp_driver::AcpLaunch {
                    executable: path(binary),
                    args: args.into_iter().map(String::from).collect(),
                },
                None,
                resume,
            )?)
        }
        _ => return Err("unknown provider".into()),
    })
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().collect();
    let provider = args
        .get(1)
        .ok_or("provider required: invokes a real model")?;
    if provider == "opencode" && args.get(3).is_none() {
        return Err("OpenCode requires the executable selected by the desktop app as the third argument; standalone and desktop CLIs may differ".into());
    }
    let output = PathBuf::from(args.get(2).ok_or("output directory required")?);
    std::fs::create_dir_all(&output)?;
    let workspace = output.join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let cwd = workspace.canonicalize()?.to_string_lossy().into_owned();
    let multi = args.get(5).is_some_and(|s| s == "multi");
    if multi {
        let name = format!("second-{}.txt", agent_domain::InstallationId::new());
        std::fs::write(workspace.join("first.txt"), format!("Read {name} next."))?;
        std::fs::write(
            workspace.join(name),
            "Reply MULTI_REQUEST_OK after reading this file.",
        )?;
    }
    let (driver, quota) = driver(provider, args.get(3).cloned())?;
    let session = tokio::time::timeout(
        Duration::from_mins(1),
        driver.create_selected_session(
            SessionRequest {
                working_directory: cwd.clone(),
                additional_directories: vec![],
                app_mcps: vec![],
                permission_mode: (multi && provider == "codex").then(|| "hub-analysis".into()),
            },
            args.get(4).map(String::as_str),
            None,
        ),
    )
    .await;
    let mut session = match session {
        Ok(Ok(s)) => s,
        result => {
            let report = json!({"provider":provider,"executableOverride":args.get(3),"modelOverride":args.get(4),"stage":"create_session","error":format!("{result:?}")});
            std::fs::write(
                output.join("report.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            println!("{report}");
            return Ok(());
        }
    };
    let mut turns = vec![];
    for n in 1..=2 {
        if n == 2 && args.get(5).is_some_and(|s| s == "resume") {
            driver.shutdown(session.clone()).await?;
            session = driver
                .resume_selected_session(
                    &session.native_session_id,
                    SessionRequest {
                        working_directory: cwd.clone(),
                        additional_directories: vec![],
                        app_mcps: vec![],
                        permission_mode: None,
                    },
                    args.get(4).map(String::as_str),
                    None,
                )
                .await?;
        }
        let before = quota.read_session_usage(&session.native_session_id).await;
        let started = std::time::Instant::now();
        let mut first_output_ms = None;
        let mut observed_requests = driver_api::usage::RequestUsageReport::default();
        let work = async {
            let prompt = if multi {
                "Use a native read-only tool to read first.txt in the working directory. Its contents name a second file: after seeing the first result, read that second file using another native tool call. Follow its reply instruction. Do not delegate, write files, execute other commands, or access the network.".into()
            } else {
                format!(
                    "Telemetry acceptance {n}. Reply exactly USAGE_OK_{n}. Do not use tools, delegate, read or modify files, or access the network."
                )
            };
            let mut stream = driver
                .prompt(
                    &session,
                    PromptRequest {
                        prompt,
                        working_directory: cwd.clone(),
                        context: vec![],
                        additional_directories: vec![],
                        attachments: vec![],
                        permission_mode: None,
                    },
                )
                .await?;
            while let Some(event) = stream.next().await {
                match event? {
                    DriverEvent::FirstOutput | DriverEvent::TextDelta { .. } => {
                        first_output_ms.get_or_insert_with(|| {
                            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
                        });
                    }
                    DriverEvent::UsageRecorded { request } => observed_requests.observe(request),
                    DriverEvent::TurnCompleted { .. } => {
                        return Ok::<_, Error>("completed".to_owned());
                    }
                    DriverEvent::TurnFailed { reason } => return Ok(format!("failed: {reason}")),
                    DriverEvent::TurnQuotaExhausted { .. } => return Ok("quota_exhausted".into()),
                    DriverEvent::TurnInterrupted { .. } => return Ok("interrupted".into()),
                    DriverEvent::ApprovalRequested { .. }
                    | DriverEvent::QuestionRequested { .. } => {
                        return Ok("unexpected_interaction".into());
                    }
                    _ => {}
                }
            }
            Ok("stream_closed_without_terminal".into())
        };
        let status = match tokio::time::timeout(Duration::from_mins(2), work).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => format!("error: {e}"),
            Err(_) => "timeout".into(),
        };
        if status != "completed" {
            let _ = tokio::time::timeout(Duration::from_secs(8), driver.interrupt(&session)).await;
        }
        let usage = tokio::time::timeout(
            Duration::from_secs(15),
            quota.read_session_usage(&session.native_session_id),
        )
        .await
        .ok()
        .flatten();
        let repeated = tokio::time::timeout(
            Duration::from_secs(15),
            quota.read_session_usage(&session.native_session_id),
        )
        .await
        .ok()
        .flatten();
        turns.push(json!({"turn":n,"status":status,"usage":usage,"repeatedUsage":repeated,
            "firstOutputMs":first_output_ms,"beforeUsage":before,"observedRequests":observed_requests,
            "observedTurnUsage":agent_core::summarize_recorded_requests(&observed_requests)}));
        println!(
            "provider={provider} turn={n} status={status} usage={}",
            usage.is_some()
        );
        if status != "completed" {
            break;
        }
    }
    let cleanup =
        tokio::time::timeout(Duration::from_secs(12), driver.shutdown(session.clone())).await;
    let report = json!({"provider":provider,"executableOverride":args.get(3),"modelOverride":args.get(4),"nativeSessionId":session.native_session_id,"workspace":cwd,"turns":turns,"cleanup":format!("{cleanup:?}")});
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
