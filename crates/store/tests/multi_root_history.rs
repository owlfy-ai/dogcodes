#[path = "support/installations.rs"]
mod installations;
use agent_core::WorkspaceContextProvider;
use agent_core::{Core, CoreRepository, history::HistoryQuery};
use agent_domain::{ArtifactId, ExecutorRef, ProjectId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use driver_api::{
    AgentDriver, DriverError, DriverEvent, DriverEventStream, PromptRequest, SessionHandle,
    SessionRequest,
};
use serde_json::Value;
use std::sync::Arc;
struct Writer {
    executor: ExecutorRef,
}
#[async_trait::async_trait]
impl AgentDriver for Writer {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle {
            native_session_id: "fixture".into(),
        })
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        assert_eq!(request.additional_directories.len(), 1);
        std::fs::write(
            std::path::Path::new(&request.working_directory).join("same.txt"),
            "primary version\n",
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&request.additional_directories[0]).join("same.txt"),
            "secondary version\n",
        )
        .unwrap();
        Ok(Box::pin(futures::stream::iter([Ok(
            DriverEvent::TurnCompleted {
                summary: "finished".into(),
            },
        )])))
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
#[tokio::test]
async fn two_real_directories_capture_history_and_restart_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let paths: Vec<String> = ["one", "two"]
        .iter()
        .map(|name| {
            let p = temp.path().join(name);
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("same.txt"), "before\n").unwrap();
            p.to_string_lossy().into_owned()
        })
        .collect();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let workspace = Arc::new(agent_workspace::GitWorkspaceContextProvider::default());
    let core = Arc::new(Core::with_workspace_context(
        store.clone(),
        workspace.clone(),
    ));
    let executor = installations::executor("fake");
    installations::register(
        &core,
        store.as_ref(),
        Arc::new(Writer {
            executor: executor.clone(),
        }),
    )
    .await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "test".into(),
            path: paths[0].clone(),
            additional_directories: vec![paths[1].clone()],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "test".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        task_id: task.clone(),
        prompt: "find literal % 中文 needle".into(),
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
    }));
    submit.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(submit).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let events = store
        .task_events(
            &task,
            0,
            core.bootstrap().await.unwrap().last_sequence,
            1000,
        )
        .await
        .unwrap();
    let changes = events
        .iter()
        .find(|e| e.event_type == "workspace.turn_changes_captured")
        .unwrap();
    let id =
        ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
    let content = store.artifact(&id).await.unwrap().unwrap().content;
    let value: Value = serde_json::from_slice(&content).unwrap();
    assert_eq!(value["schemaVersion"], 2);
    assert_eq!(value["files"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["files"][1]["path"],
        std::path::Path::new(&paths[1])
            .join("same.txt")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        value["roots"][1]["changes"]["patch"]
            .as_str()
            .unwrap()
            .contains("secondary version")
    );
    assert!(
        changes.sequence
            < events
                .iter()
                .find(|e| e.event_type == "turn.completed")
                .unwrap()
                .sequence
    );
    let query = HistoryQuery {
        task_id: task.clone(),
        after_sequence: 0,
        through_sequence: None,
        limit: 1,
        kind: "search".into(),
        text: "% 中文".into(),
    };
    let found = core.query_history(&query).await.unwrap();
    assert_eq!(found.items.len(), 1);
    assert!(found.items[0]["text"].as_str().unwrap().contains("needle"));
    assert!(
        core.query_history(&HistoryQuery {
            task_id: TaskId::new(),
            ..query.clone()
        })
        .await
        .is_err()
    );
    let first = core
        .query_history(&HistoryQuery {
            kind: "turns".into(),
            text: String::new(),
            ..query.clone()
        })
        .await
        .unwrap();
    let mut cursor = first.next_cursor;
    let mut last = first.items.last().unwrap()["sequence"].as_u64().unwrap();
    while let Some(after) = cursor {
        let page = core
            .query_history(&HistoryQuery {
                kind: "turns".into(),
                text: String::new(),
                after_sequence: after,
                through_sequence: Some(first.through_sequence),
                ..query.clone()
            })
            .await
            .unwrap();
        for item in &page.items {
            assert!(item["sequence"].as_u64().unwrap() > last);
            last = item["sequence"].as_u64().unwrap();
        }
        cursor = page.next_cursor;
    }
    let tail = store
        .recent_task_events(&task, 0, first.through_sequence, 2)
        .await
        .unwrap();
    assert_eq!(tail.len(), 2);
    assert!(tail[0].sequence < tail[1].sequence);

    let checkpoint = events
        .iter()
        .find(|e| e.event_type == "workspace.turn_checkpoint_captured")
        .unwrap();
    // Recovery consumes the exact persisted checkpoint, exercised via the provider below;
    // event replay/host interruption idempotency is covered by the Core recovery tests.
    let checkpoint_id = ArtifactId::parse(
        checkpoint.payload["artifact"]["artifactId"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let persisted: Value = serde_json::from_slice(
        &store
            .artifact(&checkpoint_id)
            .await
            .unwrap()
            .unwrap()
            .content,
    )
    .unwrap();
    for root in persisted["roots"].as_array().unwrap() {
        let recovered = workspace
            .recover_turn(
                root["workspacePath"].as_str().unwrap(),
                root["capture"].clone(),
            )
            .await
            .unwrap();
        assert_eq!(recovered["files"].as_array().unwrap().len(), 1);
    }
    assert!(!std::path::Path::new(&paths[0]).join(".git").exists());
}

struct Controlled {
    executor: ExecutorRef,
    approval: bool,
    fail_interrupt: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
    stop: tokio::sync::watch::Sender<bool>,
}
#[async_trait::async_trait]
impl AgentDriver for Controlled {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle {
            native_session_id: "controlled".into(),
        })
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        std::fs::write(
            std::path::Path::new(&request.additional_directories[0]).join("same.txt"),
            "before cancellation\n",
        )
        .unwrap();
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let event = if self.approval {
            DriverEvent::ApprovalRequested {
                native_request_id: "same-id".into(),
                tool_name: "fixture".into(),
                command_summary: "synthetic pending write".into(),
                options: vec![driver_api::ApprovalOption {
                    option_id: "allow".into(),
                    label: "Allow once".into(),
                    semantic_role: driver_api::ApprovalSemanticRole::ApproveOnce,
                }],
            }
        } else {
            DriverEvent::TextDelta {
                item_id: "text".into(),
                text: "running".into(),
            }
        };
        let mut stopped = self.stop.subscribe();
        Ok(Box::pin(futures::stream::iter([Ok(event)]).chain(
            futures::stream::once(async move {
                if !*stopped.borrow() {
                    stopped.changed().await.unwrap();
                }
                Ok(DriverEvent::TurnInterrupted {
                    summary: "stopped".into(),
                })
            }),
        )))
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        if self
            .fail_interrupt
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(DriverError::Disconnected("fixture stop failed".into()));
        }
        self.stop.send_replace(true);
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
use futures::StreamExt;
#[tokio::test]
async fn cancellation_preserves_second_root_and_failed_stop_restores_approval() {
    use agent_protocol::{CancelTurn, RespondToApproval};
    for (approval, fail) in [(false, false), (true, false), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("primary");
        let secondary = temp.path().join("secondary");
        std::fs::create_dir(&primary).unwrap();
        std::fs::create_dir(&secondary).unwrap();
        std::fs::write(secondary.join("same.txt"), "before\n").unwrap();
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::with_workspace_context(
            store.clone(),
            Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
        ));
        let driver = Arc::new(Controlled {
            executor: installations::executor("fake"),
            approval,
            fail_interrupt: std::sync::atomic::AtomicBool::new(fail),
            calls: std::sync::atomic::AtomicUsize::new(0),
            stop: tokio::sync::watch::channel(false).0,
        });
        installations::register(&core, store.as_ref(), driver.clone()).await;
        let project = ProjectId::new();
        let task = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project.clone(),
                display_name: "cancel".into(),
                path: primary.to_string_lossy().into_owned(),
                additional_directories: vec![secondary.to_string_lossy().into_owned()],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project,
                title: "cancel".into(),
                preferred_executor: driver.executor.clone(),
            },
        )))
        .await
        .unwrap();
        let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: vec![],
            task_id: task.clone(),
            prompt: "fixture".into(),
            model: None,
            thinking: None,
            permission_mode: None,
            executor_override: None,
        }));
        submit.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        core.execute(submit).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if driver.calls.load(std::sync::atomic::Ordering::SeqCst) > 0
                    && (!approval || !core.pending_approvals(&task, 10).await.unwrap().is_empty())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let original_turn = core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .as_ref()
            .unwrap()
            .turn_id
            .clone();
        let original_sequence = core
            .pending_approvals(&task, 10)
            .await
            .unwrap()
            .first()
            .map_or(0, |a| a.requested_at_sequence);
        for attempt in 0..=usize::from(fail) {
            let state = core.bootstrap().await.unwrap();
            let active = state.tasks[0].active_primary_run.as_ref().unwrap();
            let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
                task_id: task.clone(),
                turn_id: active.turn_id.clone(),
                run_id: active.run_id.clone(),
            }));
            cancel.expected_stream_version = Some(state.tasks[0].stream_version);
            core.execute(cancel).await.unwrap();
            if fail && attempt == 0 {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        if core
                            .sync_events(0, 1000)
                            .await
                            .unwrap()
                            .iter()
                            .any(|e| e.event_type == "turn.cancel_failed")
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(
                    core.pending_approvals(&task, 10).await.unwrap().len(),
                    1,
                    "failed stop must restore pending approval"
                );
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(core.pending_approvals(&task, 10).await.unwrap().is_empty());
        let mut stale = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
            turn_id: original_turn,
            requested_at_sequence: original_sequence,
            answer: None,
            task_id: task.clone(),
            approval_id: "same-id".into(),
            option_id: "allow".into(),
        }));
        stale.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        assert!(core.execute(stale).await.is_err());
        let events = core.sync_events(0, 1000).await.unwrap();
        let changes = events
            .iter()
            .find(|e| e.event_type == "workspace.turn_changes_captured")
            .unwrap();
        let id =
            ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
        let body: Value =
            serde_json::from_slice(&store.artifact(&id).await.unwrap().unwrap().content).unwrap();
        assert_eq!(
            body["files"][0]["workspacePath"],
            secondary.to_string_lossy().as_ref()
        );
        assert!(
            body["roots"][1]["changes"]["patch"]
                .as_str()
                .unwrap()
                .contains("before cancellation")
        );
        assert!(
            changes.sequence
                < events
                    .iter()
                    .find(|e| e.event_type == "turn.completed" || e.event_type == "turn.failed")
                    .unwrap()
                    .sequence
        );
    }
}

#[tokio::test]
async fn missing_additional_directory_fails_before_native_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let secondary = temp.path().join("secondary");
    std::fs::create_dir(&secondary).unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::with_workspace_context(
        store.clone(),
        Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
    ));
    let executor = installations::executor("fake");
    installations::register(
        &core,
        store.as_ref(),
        Arc::new(Writer {
            executor: executor.clone(),
        }),
    )
    .await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "missing root".into(),
            path: temp.path().to_string_lossy().into_owned(),
            additional_directories: vec![secondary.to_string_lossy().into_owned()],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "missing root".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    std::fs::remove_dir(&secondary).unwrap();
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        task_id: task.clone(),
        prompt: "must not dispatch".into(),
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
    }));
    submit.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    let outcome = core.execute(submit).await;
    if outcome.is_ok() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    let events = core.sync_events(0, 1000).await.unwrap();
    assert!(
        !temp.path().join("same.txt").exists(),
        "native prompt must not run"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "workspace.turn_changes_captured")
    );
}
