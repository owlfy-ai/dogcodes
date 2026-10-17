//! Opt-in production Core/Driver/MCP acceptance. Isolated DB/workspace, no user file tools.
#[path = "support/installation.rs"]
mod installation;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::{
    CommandEnvelope, CoreCommand, CreateTask, EventEnvelope, RegisterProject, RespondToApproval,
    SubmitTurn,
};
use agent_store::SqliteStore;
use driver_api::{AgentDriver, SessionHandle, SessionResumeCapability};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
fn driver(name: &str, id: &str) -> Result<Arc<dyn AgentDriver>, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;
    let supported = SessionResumeCapability::Supported;
    Ok(match name {
        "codex" => Arc::new(codex_driver::CodexDriver::new(
            &PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
            id.into(),
            "0.153.4".into(),
        )?),
        "claude" => Arc::new(claude_driver::ClaudeDriver::new(
            name,
            id,
            format!("{home}/.local/bin/claude").into(),
            None,
            supported,
        )?),
        "antigravity" => Arc::new(antigravity_driver::AntigravityDriver::new(
            name,
            id,
            format!("{home}/.local/bin/agy").into(),
            None,
            supported,
        )?),
        "zcode" => Arc::new(zcode_driver::ZcodeDriver::new(
            name,
            id,
            zcode_driver::ZcodeLaunch {
                app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
                cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
            },
            None,
            supported,
        )?),
        _ => {
            let (exe, args) = match name {
                "kimi" => (format!("{home}/.kimi-code/bin/kimi"), vec!["acp"]),
                "grok" => (
                    format!("{home}/.grok/bin/grok"),
                    vec![
                        "--no-auto-update",
                        "--permission-mode",
                        "default",
                        "agent",
                        "stdio",
                    ],
                ),
                "cursor" => (format!("{home}/.local/bin/cursor-agent"), vec!["acp"]),
                "codebuddy" => (
                    format!(
                        "{home}/.nvm-arm/versions/node/v22.23.2/lib/node_modules/@tencent-ai/codebuddy-code/bin/codebuddy"
                    ),
                    vec!["--acp"],
                ),
                _ => return Err("unknown provider".into()),
            };
            Arc::new(acp_driver::AcpDriver::new(
                name,
                id,
                acp_driver::AcpLaunch {
                    executable: exe.into(),
                    args: args.into_iter().map(Into::into).collect(),
                },
                None,
                supported,
            )?)
        }
    })
}
async fn turn(
    core: &Arc<Core>,
    task: &TaskId,
    prompt: &str,
) -> Result<Vec<EventEnvelope>, Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let start = snapshot.last_sequence;
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        task_id: task.clone(),
        prompt: prompt.into(),
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
    }));
    submit.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(submit).await?;
    tokio::time::timeout(Duration::from_secs(160),async {
        loop {
            let events=core.sync_events(start,10000).await?;
            if events.iter().any(|e|e.event_type=="turn.failed") {return Err("native turn failed".into());}
            for approval in core.pending_approvals(task,20).await? {
                let summary=format!("{} {}",approval.tool_name,approval.command_summary);
                let allow=summary.contains("dogcodes_") && summary.contains("searchHistory");
                let role=if allow {"approve_once"} else {"reject"};
                let option=approval.options.iter().find(|o|o.semantic_role==role || o.option_id==role || (!allow && o.option_id=="deny")).ok_or("no safe approval option")?;
                println!("{}",json!({"approvalTool":approval.tool_name,"allow":allow,"role":option.semantic_role}));
                let mut cmd=CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval{turn_id:approval.turn_id.clone(),requested_at_sequence:approval.requested_at_sequence,answer:None,task_id:task.clone(),approval_id:approval.approval_id.clone(),option_id:option.option_id.clone()}));
                cmd.expected_stream_version=Some(core.bootstrap().await?.tasks[0].stream_version);core.execute(cmd).await?;
            }
            if events.iter().any(|e|e.event_type=="turn.completed") {return Ok(events);}
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await?
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if agent_core::history_bridge::run_query_if_requested() {
        return Ok(());
    }
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    let args: Vec<_> = std::env::args().collect();
    let name = args.get(1).ok_or("provider required")?;
    let out = PathBuf::from(args.get(2).ok_or("evidence directory required")?);
    std::fs::create_dir_all(&out)?;
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace)?;
    let core = Arc::new(
        Core::new(Arc::new(SqliteStore::open(temp.path().join("probe.db"))?))
            .with_history_query_program(
                args.get(3).map_or(std::env::current_exe()?, PathBuf::from),
            ),
    );
    let installation = installation::identity(&core, name).await?;
    let mut native = driver(name, &installation)?;
    let executor = native.executor().clone();
    installation::register(&core, native.clone()).await?;
    let task = TaskId::new();
    let project = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "History MCP acceptance".into(),
            path: workspace.to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "History MCP acceptance".into(),
            preferred_executor: executor,
        },
    )))
    .await?;
    let marker = format!(
        "MCP_FACT_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            % 1_000_000
    );
    let first=turn(&core,&task,&format!("Remember this test fact: {marker}. Reply ACK. Do not call any tools or access any files.")).await;
    let mut result = Err("seed turn failed".into());
    if let Ok(events) = first {
        std::fs::write(out.join("seed.json"), serde_json::to_vec_pretty(&events)?)?;
        let before = core.bootstrap().await?.session_maps[0]
            .native_session_id
            .clone();
        // New native driver process, same stored conversation and Core endpoint.
        native
            .shutdown(SessionHandle {
                native_session_id: before.clone(),
            })
            .await?;
        native = driver(name, &installation)?;
        installation::register(&core, native.clone()).await?;
        result=turn(&core,&task,"Call the supplied dogcodes searchHistory MCP tool with text MCP_FACT_ to retrieve the previous test fact, and return that fact verbatim. You may discover MCP tools. You MUST actually query history even if you remember the fact. Do not use shell, file tools, web, or unrelated tools. If unavailable report HISTORY_TOOL_UNAVAILABLE.").await;
        if let Ok(events) = &result {
            std::fs::write(out.join("query.json"), serde_json::to_vec_pretty(events)?)?;
            let answer = events
                .iter()
                .filter_map(|e| {
                    if e.event_type == "message.delta" {
                        e.payload["text"].as_str()
                    } else if e.event_type == "activity.updated"
                        && e.payload["data"]["type"] == "message"
                    {
                        e.payload["data"]["textDelta"]
                            .as_str()
                            .or_else(|| e.payload["data"]["text"].as_str())
                    } else {
                        None
                    }
                })
                .collect::<String>();
            let tool = events.iter().any(|e| {
                (e.event_type == "tool.started" || e.event_type == "activity.updated")
                    && e.payload.to_string().contains("searchHistory")
            });
            let tool_result_contains_fact = events.iter().any(|e| {
                e.event_type == "activity.updated"
                    && (e.payload["data"]["output"].to_string().contains(&marker)
                        || e.payload["data"]["content"].to_string().contains(&marker))
            });
            let tool_result_contains_fact = tool_result_contains_fact
                || (args.get(3).is_some()
                    && std::fs::read_to_string(out.join("mcp-results.jsonl"))
                        .is_ok_and(|text| text.contains(&marker)));
            let after = core.bootstrap().await?.session_maps[0]
                .native_session_id
                .clone();
            let summary = json!({"provider":name,"nativeIdPreserved":before==after,"toolObserved":tool,"toolResultContainsFact":tool_result_contains_fact,"factReturned":answer.contains(&marker),"sessionId":after});
            std::fs::write(
                out.join("summary.json"),
                serde_json::to_vec_pretty(&summary)?,
            )?;
            println!("{summary}");
            if !tool || !tool_result_contains_fact || !answer.contains(&marker) || before != after {
                result = Err("history acceptance assertion failed".into());
            }
        }
    }
    for mapping in core.bootstrap().await?.session_maps {
        let _ = native
            .shutdown(SessionHandle {
                native_session_id: mapping.native_session_id,
            })
            .await;
    }
    result.map(|_| ())
}
