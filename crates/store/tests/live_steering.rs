//! Opt-in native driver acceptance. Requires authenticated installations on PATH.
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionRequest, SteerDelivery, TurnSteering,
};
use futures::StreamExt;
use std::{path::PathBuf, sync::Arc, time::Duration};

fn executable(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .expect("installed executable")
}

async fn exercise(driver: Arc<dyn AgentDriver>, model: Option<&str>) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().to_str().unwrap().to_owned();
    let session = driver
        .create_selected_session(
            SessionRequest {
                working_directory: cwd.clone(),
                additional_directories: vec![],
                app_mcps: vec![],
                permission_mode: None,
            },
            model,
            None,
        )
        .await
        .unwrap();
    let mut previous: Option<Arc<dyn TurnSteering>> = None;
    for tool in [true, false] {
        let prompt = if tool {
            "Run exactly sleep 8 using the shell, then answer ORIGINAL. Do not use other tools or touch files."
        } else {
            "Without using tools write 50 numbered sentences on fruit varieties. End with ORIGINAL. Follow any updated instructions."
        };
        let marker = format!("REVISED{}", agent_domain::InstallationId::new());
        let mut stream = driver
            .prompt(
                &session,
                PromptRequest {
                    prompt: prompt.into(),
                    working_directory: cwd.clone(),
                    additional_directories: vec![],
                    context: vec![],
                    attachments: vec![],
                    permission_mode: None,
                },
            )
            .await
            .unwrap();
        if let Some(stale) = previous.take() {
            assert_eq!(
                stale.send("expired", "Reply STALE").await.unwrap(),
                SteerDelivery::Rejected
            );
        }
        let sender = driver
            .turn_steering(&session)
            .await
            .expect("active turn supports steering");
        let mut receipt = None;
        let mut text = String::new();
        let mut terminals = 0;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let inject = if tool {
                matches!(
                    &event,
                    DriverEvent::ToolStarted { .. } | DriverEvent::ApprovalRequested { .. }
                )
            } else {
                matches!(&event, DriverEvent::TextDelta {text,..} if !text.is_empty())
            };
            if inject && receipt.is_none() {
                let sender = sender.clone();
                let correction = format!(
                    "Correction: stop the original answer. Reply with only {marker}. No further tools."
                );
                receipt = Some(tokio::spawn(async move {
                    sender.send("correction", &correction).await
                }));
            }
            match event {
                DriverEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                DriverEvent::ApprovalRequested {
                    native_request_id,
                    command_summary,
                    ..
                } => {
                    let parsed = serde_json::from_str::<serde_json::Value>(&command_summary).ok();
                    let command = parsed
                        .as_ref()
                        .and_then(|input| input["command"].as_str())
                        .unwrap_or(&command_summary);
                    assert!(
                        command.trim() == "sleep 8",
                        "unexpected approval: {command_summary}"
                    );
                    driver
                        .respond_to_interaction(&session, &native_request_id, "allow")
                        .await
                        .unwrap();
                }
                DriverEvent::TurnCompleted { .. } => {
                    assert!(
                        text.contains(&marker),
                        "completed before correction appeared: {text}"
                    );
                    terminals += 1;
                }
                DriverEvent::TurnFailed { reason } => panic!("native turn failed: {reason}"),
                DriverEvent::TurnInterrupted { .. } | DriverEvent::TurnQuotaExhausted { .. } => {
                    panic!("unexpected terminal: {event:?}")
                }
                _ => {}
            }
        }
        assert_eq!(terminals, 1);
        assert_eq!(
            receipt
                .expect("steer dispatched before completion")
                .await
                .unwrap()
                .unwrap(),
            SteerDelivery::Accepted
        );
        previous = Some(sender);
        eprintln!("verified {} window", if tool { "tool" } else { "text" });
    }
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
#[ignore = "uses real Codex model quota"]
async fn codex() {
    let path = executable("codex");
    let version = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .unwrap();
    let driver = codex_driver::CodexDriver::new(
        path,
        agent_domain::InstallationId::new().to_string(),
        String::from_utf8(version.stdout).unwrap(),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_mins(5), exercise(Arc::new(driver), None))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "uses real Claude Code model quota"]
async fn claude() {
    let driver = claude_driver::ClaudeDriver::new(
        "claude-code",
        &agent_domain::InstallationId::new().to_string(),
        executable("claude"),
        None,
        driver_api::SessionResumeCapability::Supported,
    )
    .unwrap();
    tokio::time::timeout(
        Duration::from_mins(5),
        exercise(
            Arc::new(driver),
            std::env::var("DOGCODES_TEST_CLAUDE_MODEL").ok().as_deref(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "uses the configured Pi model"]
async fn pi() {
    let driver = pi_driver::PiDriver::new(
        &agent_domain::InstallationId::new().to_string(),
        executable("pi"),
        None,
    )
    .unwrap();
    tokio::time::timeout(
        Duration::from_mins(5),
        exercise(
            Arc::new(driver),
            std::env::var("DOGCODES_TEST_PI_MODEL").ok().as_deref(),
        ),
    )
    .await
    .unwrap();
}

#[path = "support/installations.rs"]
mod installations;

#[tokio::test]
#[ignore = "uses real Kimi quota for two turns through Core and SQLite"]
async fn kimi_responds_to_a_queued_message_after_its_current_turn() {
    use agent_core::Core;
    use agent_domain::{ProjectId, TaskId};
    use agent_protocol::{
        CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SteerTurn, SubmitTurn,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(agent_store::SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store));
    let executor = installations::probe(&core, "kimi").await;
    let launch = acp_driver::AcpLaunch {
        executable: executable("kimi").into(),
        args: vec!["acp".into()],
    };
    let probe = acp_driver::AcpDriver::probe_ready(&launch).await.unwrap();
    let driver = acp_driver::AcpDriver::new(
        "kimi",
        executor.installation_id.as_ref().unwrap(),
        launch,
        probe.native_version,
        probe.resume_capability,
    )
    .unwrap();
    core.register_driver(Arc::new(driver)).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            target_id: None,
            project_id: project.clone(),
            display_name: "follow-up".into(),
            path: dir.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "follow-up".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    let mut initial = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "Without tools, give me 35 short facts about fruit, then write FRUITDONE.".into(),
        executor_override: None,
        attachments: vec![],
        review_fix: None,
        model: None,
        thinking: None,
        permission_mode: None,
    }));
    initial.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(initial).await.unwrap();
    // ACP root responses are projected as message activities in the same timeline as native text.
    let answer = |row: &agent_protocol::TimelineItem| -> String {
        if row.kind == "assistant_message" {
            return row.text.clone();
        }
        if row.kind == "activity"
            && let Ok(data) = serde_json::from_str::<serde_json::Value>(&row.text)
            && data["type"] == "message"
        {
            return data["text"].as_str().unwrap_or("").into();
        }
        String::new()
    };
    let marker = format!("NEXT{}", agent_domain::InstallationId::new());
    tokio::time::timeout(Duration::from_mins(4), async {
        let run = loop {
            let task_state = core.bootstrap().await.unwrap().tasks.remove(0);
            let events = core.sync_events(0, 200).await.unwrap();
            let run = task_state.active_primary_run.unwrap_or_else(|| {
                panic!(
                    "first ended: {:?}",
                    events
                        .iter()
                        .filter(|e| e.event_type.starts_with("turn."))
                        .map(|e| (
                            &e.event_type,
                            &e.payload["failureKind"],
                            &e.payload["message"]
                        ))
                        .collect::<Vec<_>>()
                )
            });
            if events
                .iter()
                .any(|e| e.event_type == "history.native_prompt_accepted")
            {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        };
        assert!(!run.supports_steering);
        let text = format!("Without tools reply with only {marker}.");
        let followup = CommandEnvelope::local(CoreCommand::SteerTurn(SteerTurn {
            task_id: task.clone(),
            run_id: run.run_id,
            turn_id: run.turn_id.clone(),
            text: text.clone(),
        }));
        let receipt = core.execute(followup.clone()).await.unwrap();
        assert_eq!(receipt, core.execute(followup).await.unwrap());
        assert!(
            core.timeline(&task)
                .await
                .unwrap()
                .iter()
                .any(|r| r.kind == "followup_queued" && r.text == text)
        );
        loop {
            let timeline = core.timeline(&task).await.unwrap();
            assert!(
                !timeline
                    .iter()
                    .any(|r| matches!(r.kind.as_str(), "error" | "followup_failed")),
                "native turn failed"
            );
            if let Some(sent) = timeline
                .iter()
                .find(|r| r.kind == "user_message" && r.text == text)
            {
                assert_ne!(sent.turn_id.as_ref(), Some(&run.turn_id));
                if timeline.iter().any(|r| {
                    r.turn_id == sent.turn_id
                        && answer(r).contains(&marker)
                        && r.turn_completion.is_some()
                }) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .unwrap();
}
