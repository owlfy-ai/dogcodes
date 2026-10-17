use agent_core::{Core, CoreRepository, command_invalidation::CommandInvalidation};
use agent_domain::{ExecutorRef, ProjectId, ProviderId, TaskId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn,
};
use agent_store::SqliteStore;
use std::sync::Arc;

fn executor() -> ExecutorRef {
    ExecutorRef {
        provider_id: ProviderId::parse("fake").unwrap(),
        installation_id: None,
    }
}
async fn creation(core: &Arc<Core>) -> CommandEnvelope<CoreCommand> {
    let project_id = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project_id.clone(),
            display_name: "fixture".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let mut envelope = CommandEnvelope::local(CoreCommand::CreateTask(CreateTask {
        task_id: TaskId::new(),
        project_id,
        title: "private-title".into(),
        preferred_executor: executor(),
    }));
    envelope.expected_stream_version = Some(0);
    envelope
}
fn task_id(envelope: &CommandEnvelope<CoreCommand>) -> TaskId {
    let CoreCommand::CreateTask(command) = &envelope.payload else {
        unreachable!()
    };
    command.task_id.clone()
}
fn submission(task_id: TaskId) -> CommandEnvelope<CoreCommand> {
    let mut envelope = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id,
        prompt: "private-prompt".into(),
        model: None,
        thinking: None,
        permission_mode: None,
        attachments: vec![],
        executor_override: Some(executor()),
        review_fix: None,
    }));
    envelope.expected_stream_version = Some(1);
    envelope
}
#[tokio::test]
async fn retired_creation_survives_reopen_without_creating_a_task_or_losing_the_barrier() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let command = creation(&core).await;
    assert!(core.command_receipt(&command).await.unwrap().is_none());
    let before = store.event_head().await.unwrap();
    assert!(
        core.pending_command_invalidation_receipt(&command)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.event_head().await.unwrap(), before);
    assert!(matches!(
        core.invalidate_pending_command(&command).await.unwrap(),
        CommandInvalidation::Invalidated
    ));
    assert!(core.bootstrap().await.unwrap().tasks.is_empty());
    let head = store.event_head().await.unwrap();
    let text = serde_json::to_string(&core.sync_events(1, 100).await.unwrap()).unwrap();
    assert!(!text.contains("private-title"));
    drop(core);
    drop(store);
    let store = Arc::new(SqliteStore::open(path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    assert!(core.execute(command.clone()).await.is_err());
    assert!(matches!(
        core.invalidate_pending_command(&command).await.unwrap(),
        CommandInvalidation::Invalidated
    ));
    assert!(matches!(
        core.pending_command_invalidation_receipt(&command)
            .await
            .unwrap(),
        Some(CommandInvalidation::Invalidated)
    ));
    assert_eq!(store.event_head().await.unwrap(), head);
    assert!(core.command_receipt(&command).await.unwrap().is_none());
    assert!(core.bootstrap().await.unwrap().tasks.is_empty());
}
#[tokio::test]
async fn accepted_creation_is_reported_even_after_deletion_without_recreating_it() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let command = creation(&core).await;
    let ack = core.execute(command.clone()).await.unwrap();
    let mut delete =
        CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
            task_id: task_id(&command),
        }));
    delete.expected_stream_version = Some(1);
    core.execute(delete).await.unwrap();
    let head = store.event_head().await.unwrap();
    let CommandInvalidation::Accepted { acknowledgement } =
        core.invalidate_pending_command(&command).await.unwrap()
    else {
        panic!("acceptance must win")
    };
    assert_eq!(acknowledgement, ack);
    let Some(CommandInvalidation::Accepted { acknowledgement }) = core
        .pending_command_invalidation_receipt(&command)
        .await
        .unwrap()
    else {
        panic!("read-only query must find original acceptance")
    };
    assert_eq!(acknowledgement, ack);
    assert!(core.bootstrap().await.unwrap().tasks.is_empty());
    assert_eq!(store.event_head().await.unwrap(), head);
}
#[tokio::test]
async fn creation_and_retirement_race_never_both_take_effect() {
    for retire_first in [true, false] {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let command = creation(&core).await;
        let (retired, sent) = tokio::join!(
            async {
                if !retire_first {
                    tokio::task::yield_now().await;
                }
                core.invalidate_pending_command(&command).await.unwrap()
            },
            core.execute(command.clone())
        );
        match retired {
            CommandInvalidation::Accepted { acknowledgement } => {
                assert_eq!(acknowledgement, sent.unwrap());
            }
            CommandInvalidation::Invalidated => {
                assert!(sent.is_err());
                assert!(core.bootstrap().await.unwrap().tasks.is_empty());
            }
        }
    }
}
#[tokio::test]
async fn retiring_submission_does_not_run_it_and_retries_do_not_block_a_newer_version() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let create = creation(&core).await;
    core.execute(create.clone()).await.unwrap();
    let original = submission(task_id(&create));
    assert!(matches!(
        core.invalidate_pending_command(&original).await.unwrap(),
        CommandInvalidation::Invalidated
    ));
    assert!(core.execute(original.clone()).await.is_err());
    assert!(
        core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_none()
    );
    let head = store.event_head().await.unwrap();
    let mut rename = CommandEnvelope::local(CoreCommand::RenameTask(agent_protocol::RenameTask {
        task_id: task_id(&create),
        title: "newer state".into(),
    }));
    rename.expected_stream_version = Some(2);
    core.execute(rename).await.unwrap();
    core.invalidate_pending_command(&original).await.unwrap();
    assert_eq!(store.event_head().await.unwrap(), head + 1);
    let mut changed = original.clone();
    if let CoreCommand::SubmitTurn(input) = &mut changed.payload {
        input.prompt = "changed".into();
    }
    assert!(core.invalidate_pending_command(&changed).await.is_err());
    assert!(
        core.pending_command_invalidation_receipt(&changed)
            .await
            .is_err()
    );
    let events = core.sync_events(0, 100).await.unwrap();
    assert!(!events.iter().any(|e| e.event_type == "turn.queued"));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private-prompt")
    );
}
#[tokio::test]
async fn retiring_context_confirmation_does_not_build_or_switch_context() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let create = creation(&core).await;
    core.execute(create.clone()).await.unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::ConfirmContextPackage(
        agent_protocol::ConfirmContextPackage {
            request: agent_protocol::ContextPreviewRequest {
                task_id: task_id(&create),
                kind: agent_protocol::ContextPackageKind::Handoff,
                strength: agent_protocol::ContextStrength::Standard,
                source_executor: executor(),
                target_executor: ExecutorRef {
                    provider_id: ProviderId::parse("another").unwrap(),
                    installation_id: None,
                },
                after_sequence: 0,
                through_sequence: 2,
            },
            expected_payload_sha256: "frozen-hash".into(),
            edited_package: None,
        },
    ));
    command.expected_stream_version = Some(1);
    assert!(matches!(
        core.invalidate_pending_command(&command).await.unwrap(),
        CommandInvalidation::Invalidated
    ));
    let task = core.bootstrap().await.unwrap().tasks.remove(0);
    assert_eq!(task.preferred_executor, executor());
    assert_eq!(task.stream_version, 2);
    assert!(core.command_receipt(&command).await.unwrap().is_none());
    assert!(
        !core
            .sync_events(0, 100)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type.ends_with("package_confirmed"))
    );
}
#[tokio::test]
async fn invalidation_rejects_unknown_commands_actors_versions_and_schema_without_writes() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = creation(&core).await;
    let mut cases = vec![original.clone(); 5];
    cases[0].expected_stream_version = None;
    cases[1].expected_stream_version = Some(1);
    cases[2].actor = ActorRef::SystemRecovery;
    cases[3].schema_version = u16::MAX;
    cases[4].idempotency_key = " ".into();
    for value in cases {
        assert!(core.invalidate_pending_command(&value).await.is_err());
        assert!(
            core.pending_command_invalidation_receipt(&value)
                .await
                .is_err()
        );
    }
    let other = CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
        task_id: task_id(&original),
    }));
    assert!(core.invalidate_pending_command(&other).await.is_err());
    assert_eq!(store.event_head().await.unwrap(), 1);
}

#[tokio::test]
async fn read_only_invalidation_lookup_rejects_wrong_stream_or_event_boundary() {
    use agent_core::{AppendRequest, command_identity};
    use agent_domain::CommandId;
    use agent_protocol::{NewEvent, StreamRef, StreamType};
    for wrong_stream in [true, false] {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let original = creation(&core).await;
        let original_task = task_id(&original);
        let fingerprint = command_identity::fingerprint(&original).unwrap();
        let command_id = CommandId::new();
        let mut event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: if wrong_stream {
                    TaskId::new()
                } else {
                    original_task.clone()
                }
                .to_string(),
            },
            "task.command_invalidated",
            serde_json::json!({"taskId": original_task, "originalOperationId": original.idempotency_key, "originalVersion":0,
                "originalCommandType":"create_task", "requestFingerprint":fingerprint}),
        );
        event.causation_id = Some(if wrong_stream {
            command_id.to_string()
        } else {
            CommandId::new().to_string()
        });
        store
            .append(AppendRequest {
                command_id,
                idempotency_key: format!("command-invalidate:{}", original.idempotency_key),
                request_fingerprint: Some(fingerprint),
                expected_stream_version: Some(0),
                artifacts: vec![],
                events: vec![event],
            })
            .await
            .unwrap();
        let head = store.event_head().await.unwrap();
        assert!(
            core.pending_command_invalidation_receipt(&original)
                .await
                .is_err()
        );
        assert_eq!(store.event_head().await.unwrap(), head);
    }
}
