//! Explicit live desktop acceptance through the production Core and agent drivers.
#[path = "support/installation.rs"]
mod installation;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use driver_api::AgentDriver;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

type Error = Box<dyn std::error::Error>;
fn main() -> Result<(), Error> {
    if agent_core::computer_use::run_proxy_if_requested()
        || agent_core::history_bridge::run_query_if_requested()
    {
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Error> {
    let args: Vec<_> = std::env::args().collect();
    let provider = args
        .get(1)
        .ok_or("provider required: codex | deepseek-harness")?;
    let executable = PathBuf::from(args.get(2).ok_or("agent executable required")?);
    let driver_path = PathBuf::from(args.get(3).ok_or("cua-driver executable required")?);
    let workspace = PathBuf::from(
        args.get(4)
            .ok_or("isolated acceptance workspace required")?,
    )
    .canonicalize()?;
    let prompt_path = PathBuf::from(args.get(5).ok_or("prompt file required")?);
    let output = workspace.join("evidence");
    std::fs::create_dir_all(&output)?;
    let core = Arc::new(
        Core::new(Arc::new(SqliteStore::open(output.join("acceptance.db"))?))
            .with_history_query_program(std::env::current_exe()?)
            .with_computer_use(
                std::env::current_exe()?,
                driver_path,
                output.join("driver-home"),
            ),
    );
    let identity = installation::identity(&core, provider).await?;
    let native: Arc<dyn AgentDriver> = match provider.as_str() {
        "codex" => Arc::new(codex_driver::CodexDriver::new(
            &executable,
            identity,
            "0.164.0".into(),
        )?),
        "deepseek-harness" => Arc::new(
            acp_driver::AcpDriver::connect_ready(
                provider,
                &identity,
                acp_driver::AcpLaunch {
                    executable,
                    args: vec!["--profile".into(), "acp".into()],
                },
                None,
            )
            .await?,
        ),
        _ => return Err("unsupported acceptance provider".into()),
    };
    installation::register(&core, native.clone()).await?;
    let model = if provider == "deepseek-harness" {
        // Use the account route advertised by this installation. Its configured
        // default API-key route may have no key even while desktop login works.
        Some(native
            .model_catalog_in_workspace(None, workspace.to_str())
            .await?
            .models
            .into_iter()
            .find(|model| model.id == "[\"deepseek-account\",\"deepseek-flash\"]")
            .map(|model| model.id)
            .ok_or("DeepSeek account flash model is unavailable; select a configured multimodal Flash route before running acceptance")?)
    } else {
        None
    };
    let permission = if args.get(6).map(String::as_str) == Some("edit") {
        native
            .permission_catalog(workspace.to_str())
            .await?
            .review_mode()
    } else {
        None
    };
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "Computer-use acceptance".into(),
            path: workspace.to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: format!("Computer use: {provider}"),
            preferred_executor: native.executor().clone(),
        },
    )))
    .await?;
    let initial = core
        .computer_use_control(&task, json!({"type":"status"}))
        .await?;
    if initial["enabled"] != false {
        return Err("computer use must default off".into());
    }
    // Start with an ordinary turn so dynamic MCP injection is tested on resume.
    let seed = execute_turn(
        &core,
        &task,
        "Reply READY. Do not use any tools.",
        model.as_deref(),
        permission.as_deref(),
        None,
    )
    .await;
    if let Err(error) = seed {
        cleanup(&core, &native, &task).await;
        return Err(error);
    }
    core.computer_use_control(&task, json!({"type":"set","enabled":true}))
        .await?;
    let before = core.bootstrap().await?.last_sequence;
    let prompt = std::fs::read_to_string(prompt_path)?;
    let result = execute_turn(
        &core,
        &task,
        &prompt,
        model.as_deref(),
        permission.as_deref(),
        Some(&output),
    )
    .await;
    let events = core.sync_events(before, 10000).await?;
    let actions: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "computer_use.action")
        .collect();
    let read = actions.iter().any(|event| {
        matches!(
            event.payload["tool"].as_str(),
            Some("get_window_state" | "get_desktop_state")
        )
    });
    let input = actions.iter().any(|event| {
        matches!(
            event.payload["tool"].as_str(),
            Some("click" | "type_text" | "set_value" | "press_key" | "hotkey" | "run_steps")
        )
    });
    std::fs::write(
        output.join("actions.json"),
        serde_json::to_vec_pretty(&actions)?,
    )?;
    let summary = json!({"provider":provider,"model":model,"turnCompleted":result.is_ok(),"observed":read,"inputObserved":input,"actionCount":actions.len(),"error":result.as_ref().err().map(ToString::to_string)});
    std::fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!("{summary}");
    cleanup(&core, &native, &task).await;
    result?;
    if !read || !input {
        return Err("agent did not observe and operate the GUI through dogcodes tools".into());
    }
    Ok(())
}

async fn execute_turn(
    core: &Arc<Core>,
    task: &TaskId,
    prompt: &str,
    model: Option<&str>,
    permission: Option<&str>,
    output: Option<&std::path::Path>,
) -> Result<(), Error> {
    let snapshot = core.bootstrap().await?;
    let start = snapshot.last_sequence;
    let task_state = snapshot
        .tasks
        .iter()
        .find(|item| item.task_id == *task)
        .ok_or("missing task")?;
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        permission_mode: permission.map(str::to_owned),
        thinking: None,
        model: model.map(str::to_owned),
        task_id: task.clone(),
        prompt: prompt.into(),
        executor_override: None,
    }));
    command.expected_stream_version = Some(task_state.stream_version);
    core.execute(command).await?;
    let result = tokio::time::timeout(Duration::from_mins(7), async {
        let mut reported = std::collections::BTreeSet::new();
        loop {
            let events = core.sync_events(start, 10000).await?;
            for event in &events {
                if reported.insert(event.sequence)
                    && (event.event_type == "computer_use.action"
                        || event.event_type == "turn.failed")
                {
                    println!("{}", json!({"event":event.event_type,"data":event.payload}));
                }
            }
            if let Some(event) = events
                .iter()
                .find(|event| event.event_type == "turn.failed")
            {
                return Err(format!("turn failed: {}", event.payload).into());
            }
            if !core.pending_approvals(task, 20).await?.is_empty() {
                return Err(
                    "unexpected native approval; acceptance never auto-approves unrelated tools"
                        .into(),
                );
            }
            if events
                .iter()
                .any(|event| event.event_type == "turn.completed")
            {
                if let Some(output) = output {
                    std::fs::write(
                        output.join("events.json"),
                        serde_json::to_vec_pretty(&events)?,
                    )?;
                }
                return Ok::<_, Error>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    result.map_err(|_| -> Error { "computer-use acceptance turn timed out".into() })?
}

async fn cleanup(core: &Core, native: &Arc<dyn AgentDriver>, task: &TaskId) {
    let _ = core
        .computer_use_control(task, json!({"type":"set","enabled":false}))
        .await;
    if let Ok(snapshot) = core.bootstrap().await {
        for mapping in snapshot.session_maps {
            if let Some(handle) = native
                .current_session_handle(&mapping.native_session_id)
                .await
            {
                let _ = native.shutdown(handle).await;
            }
        }
    }
}
