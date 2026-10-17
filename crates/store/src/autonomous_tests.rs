use super::*;
use agent_protocol::NewEvent;

#[tokio::test]
async fn continuation_winning_native_admission_does_not_replace_its_session() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut fixture = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    fixture.reuse = true;
    *fixture.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    *fixture.autonomous_sender.lock().unwrap() = Some(tx);
    let driver = Arc::new(fixture);
    fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "original").await;
    wait_until_idle(&core).await;
    let before = core.bootstrap().await.unwrap();
    driver
        .autonomous_during_resume
        .store(true, std::sync::atomic::Ordering::Release);
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "new input".into(),
        attachments: vec![],
        executor_override: None,
        model: Some("other-model".into()),
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    submit.expected_stream_version = Some(before.tasks[0].stream_version);
    core.execute(submit).await.unwrap();
    wait_text(&core, &task, "racing continuation").await;
    wait_until_idle(&core).await;
    let after = core.bootstrap().await.unwrap();
    assert_eq!(
        before.session_maps[0].session_map_id,
        after.session_maps[0].session_map_id
    );
    assert_eq!(
        driver.contexts.lock().unwrap().len(),
        1,
        "new input was never dispatched"
    );
    let events = core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(
        |e| e.event_type == "turn.failed" && e.payload["failureKind"] == "autonomous_turn_busy"
    ));
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "session.ready")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "turn.autonomous.started")
            .count(),
        1
    );
}

async fn wait_text(core: &Core, task: &TaskId, text: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if core
                .timeline(task)
                .await
                .unwrap()
                .iter()
                .any(|i| i.text.contains(text))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing live text: {text}"));
}

#[tokio::test]
async fn autonomous_turn_streams_cancels_and_rebuilds_without_a_fake_prompt() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut driver = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    driver.reuse = true;
    *driver.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    *driver.autonomous_sender.lock().unwrap() = Some(tx.clone());
    fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "original user prompt").await;
    wait_until_idle(&core).await;
    // Existing native continuation keeps its original adapter after selection
    // removes it from the registry; cancellation must still reach that adapter.
    core.select_installation("scripted", None).await;
    assert!(core.registered_executors().await.is_empty());
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: Some("native-auto-1".into()),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "answer".into(),
        text: "live continuation".into(),
    }))
    .unwrap();
    wait_text(&core, &task, "live continuation").await;
    let snapshot = core.bootstrap().await.unwrap();
    let active = snapshot.tasks[0].active_primary_run.clone().unwrap();
    assert_eq!(active.state, agent_domain::RunState::Running);
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "must not submit".into(),
        attachments: vec![],
        executor_override: None,
        model: None,
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    submit.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    assert!(core.execute(submit).await.is_err());
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(agent_protocol::CancelTurn {
        task_id: task.clone(),
        turn_id: active.turn_id.clone(),
        run_id: active.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(cancel).await.unwrap();
    wait_until_idle(&core).await;
    let events = core.sync_events(0, 1000).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "turn.queued")
            .count(),
        1
    );
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["turnId"] == json!(active.turn_id)
        && e.payload["failureKind"] == "interrupted"));
    let timeline = core.timeline(&task).await.unwrap();
    assert_eq!(
        timeline.iter().filter(|i| i.kind == "user_message").count(),
        1
    );
    store.rebuild_projections().unwrap();
    assert_eq!(core.timeline(&task).await.unwrap(), timeline);
    assert!(
        core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_none()
    );

    // A second continuation gets its own identity and succeeds independently.
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: Some("native-auto-2".into()),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "answer".into(),
        text: "second continuation".into(),
    }))
    .unwrap();
    wait_text(&core, &task, "second continuation").await;
    tx.send(Ok(DriverEvent::TurnCompleted {
        summary: "done".into(),
    }))
    .unwrap();
    wait_until_idle(&core).await;
    let events = core.sync_events(0, 1000).await.unwrap();
    let starts: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "turn.autonomous.started")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_ne!(starts[0].payload["turnId"], starts[1].payload["turnId"]);
    assert_eq!(
        core.bootstrap().await.unwrap().companion.workspaces[0]
            .last_terminal
            .as_ref()
            .unwrap()
            .state,
        agent_domain::RunState::Completed
    );
}

#[tokio::test]
async fn autonomous_eof_fails_and_late_child_events_stay_in_their_turn() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut driver = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    driver.reuse = true;
    *driver.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "go").await;
    wait_until_idle(&core).await;
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: Some("first".into()),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::Presentation {
        item_id: "child".into(),
        data: json!({"type":"tool","name":"Bash","background":true,"status":"running"}),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "answer".into(),
        text: "first auto".into(),
    }))
    .unwrap();
    wait_text(&core, &task, "first auto").await;
    let first = core.bootstrap().await.unwrap().tasks[0]
        .active_primary_run
        .clone()
        .unwrap();
    tx.send(Ok(DriverEvent::TurnCompleted {
        summary: "done".into(),
    }))
    .unwrap();
    wait_until_idle(&core).await;
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: Some("second".into()),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::Presentation { item_id: "child".into(), data: json!({"type":"tool","background":true,"status":"completed","output":"late child result"}) })).unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "answer".into(),
        text: "second auto".into(),
    }))
    .unwrap();
    wait_text(&core, &task, "second auto").await;
    let second = core.bootstrap().await.unwrap().tasks[0]
        .active_primary_run
        .clone()
        .unwrap();
    drop(tx);
    wait_until_idle(&core).await;
    let events = core.sync_events(0, 1000).await.unwrap();
    let late = events
        .iter()
        .find(|e| e.payload["data"]["output"] == "late child result")
        .unwrap();
    assert_eq!(late.payload["turnId"], json!(first.turn_id));
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "turn.failed" && e.payload["turnId"] == json!(second.turn_id))
    );
    let timeline = core.timeline(&task).await.unwrap();
    store.rebuild_projections().unwrap();
    assert_eq!(core.timeline(&task).await.unwrap(), timeline);
}

#[tokio::test]
async fn autonomous_approval_is_owned_and_expired_after_terminal() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut driver = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    driver.reuse = true;
    *driver.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    *driver.autonomous_sender.lock().unwrap() = Some(tx.clone());
    fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "go").await;
    wait_until_idle(&core).await;
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: None,
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::ApprovalRequested {
        native_request_id: "auto-approval".into(),
        tool_name: "Bash".into(),
        command_summary: "fixture command".into(),
        options: vec![driver_api::ApprovalOption {
            option_id: "deny".into(),
            label: "Deny".into(),
            semantic_role: driver_api::ApprovalSemanticRole::Reject,
        }],
    }))
    .unwrap();
    let pending = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let pending = core.pending_approvals(&task, 10).await.unwrap();
            if !pending.is_empty() {
                break pending;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let active = core.bootstrap().await.unwrap().tasks[0]
        .active_primary_run
        .clone()
        .unwrap();
    assert_eq!(pending[0].turn_id, active.turn_id);
    assert_eq!(active.state, agent_domain::RunState::WaitingForApproval);
    let reply = || {
        CommandEnvelope::local(CoreCommand::RespondToApproval(
            agent_protocol::RespondToApproval {
                turn_id: pending[0].turn_id.clone(),
                requested_at_sequence: pending[0].requested_at_sequence,
                task_id: task.clone(),
                approval_id: "auto-approval".into(),
                option_id: "deny".into(),
                answer: None,
            },
        ))
    };
    let mut response = reply();
    response.expected_stream_version =
        Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(response).await.unwrap();
    wait_until_idle(&core).await;
    assert!(core.pending_approvals(&task, 10).await.unwrap().is_empty());
    let mut stale = reply();
    stale.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    assert!(core.execute(stale).await.is_err());
    store.rebuild_projections().unwrap();
    assert!(core.pending_approvals(&task, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn orphaned_autonomous_run_recovers_without_replaying_input() {
    use agent_domain::{CommandId, RunId, TurnId};
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    fixture_installations::register(
        &core,
        store.as_ref(),
        Arc::new(ScriptedDriver::new(
            "scripted",
            vec![DriverEvent::TurnCompleted {
                summary: "done".into(),
            }],
        )),
    )
    .await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "original").await;
    wait_until_idle(&core).await;
    let turn = TurnId::new();
    let event = NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Task,
            stream_id: task.to_string(),
        },
        "turn.autonomous.started",
        json!({"taskId":task,"turnId":turn,"runId":RunId::new(),"executor":executor("scripted"),"startedAt":Utc::now()}),
    );
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: event.event_id.to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![event],
        })
        .await
        .unwrap();
    store.rebuild_projections().unwrap();
    let restarted = Core::new(store.clone());
    restarted.recover_interrupted_turns().await.unwrap();
    assert!(
        restarted.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_none()
    );
    let events = restarted.sync_events(0, 1000).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "turn.queued")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "turn.connection_lost" && e.payload["turnId"] == json!(turn))
    );
    assert!(
        !events
            .iter()
            .any(|e| e.event_type == "turn.completed" && e.payload["turnId"] == json!(turn))
    );
    let timeline = restarted.timeline(&task).await.unwrap();
    store.rebuild_projections().unwrap();
    assert_eq!(restarted.timeline(&task).await.unwrap(), timeline);
}

#[tokio::test]
async fn native_approval_expiry_persists_before_the_autonomous_turn_ends() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut driver = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    driver.reuse = true;
    *driver.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    *driver.autonomous_sender.lock().unwrap() = Some(tx.clone());
    fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "go").await;
    wait_until_idle(&core).await;
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: None,
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::ApprovalRequested {
        native_request_id: "auto-approval".into(),
        tool_name: "Bash".into(),
        command_summary: "fixture command".into(),
        options: vec![driver_api::ApprovalOption {
            option_id: "deny".into(),
            label: "Deny".into(),
            semantic_role: driver_api::ApprovalSemanticRole::Reject,
        }],
    }))
    .unwrap();
    let pending = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let pending = core.pending_approvals(&task, 10).await.unwrap();
            if !pending.is_empty() {
                break pending;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let active = core.bootstrap().await.unwrap().tasks[0]
        .active_primary_run
        .clone()
        .unwrap();
    assert_eq!(pending[0].turn_id, active.turn_id);
    assert_eq!(active.state, agent_domain::RunState::WaitingForApproval);
    let reply = || {
        CommandEnvelope::local(CoreCommand::RespondToApproval(
            agent_protocol::RespondToApproval {
                turn_id: pending[0].turn_id.clone(),
                requested_at_sequence: pending[0].requested_at_sequence,
                task_id: task.clone(),
                approval_id: "auto-approval".into(),
                option_id: "deny".into(),
                answer: None,
            },
        ))
    };
    tx.send(Ok(DriverEvent::ApprovalClosed {
        native_request_id: "auto-approval".into(),
    }))
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if core.pending_approvals(&task, 10).await.unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
    );
    let mut stale = reply();
    stale.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    assert!(core.execute(stale).await.is_err());
    store.rebuild_projections().unwrap();
    assert!(core.pending_approvals(&task, 10).await.unwrap().is_empty());
    tx.send(Ok(DriverEvent::TurnCompleted {
        summary: "done".into(),
    }))
    .unwrap();
    wait_until_idle(&core).await;
}
