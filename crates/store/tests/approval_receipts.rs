use agent_core::{AppendRequest, Core, CoreRepository, approval_receipts::ApprovalDelivery};
use agent_domain::{CommandId, TaskId, TurnId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, NewEvent, RespondToApproval, StreamRef, StreamType,
};
use agent_store::SqliteStore;
use serde_json::json;
use std::sync::Arc;

fn command() -> CommandEnvelope<CoreCommand> {
    let mut envelope = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        task_id: TaskId::new(),
        turn_id: TurnId::new(),
        requested_at_sequence: 17,
        approval_id: "native-reused-id".into(),
        option_id: "chosen-option".into(),
        answer: Some(agent_domain::QuestionAnswer {
            cancelled: false,
            values: json!({"password":"private-answer"}),
        }),
    }));
    envelope.expected_stream_version = Some(0);
    envelope
}
async fn accepted(
    store: &SqliteStore,
    envelope: &CommandEnvelope<CoreCommand>,
    fingerprint: String,
) {
    let CoreCommand::RespondToApproval(command) = &envelope.payload else {
        unreachable!()
    };
    let mut event = NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Task,
            stream_id: command.task_id.to_string(),
        },
        "approval.resolved",
        json!(command),
    );
    event.causation_id = Some(envelope.command_id.to_string());
    store
        .append(AppendRequest {
            command_id: envelope.command_id.clone(),
            idempotency_key: envelope.idempotency_key.clone(),
            request_fingerprint: Some(fingerprint),
            expected_stream_version: Some(0),
            artifacts: vec![],
            events: vec![event],
        })
        .await
        .unwrap();
}
async fn delivered(store: &SqliteStore, envelope: &CommandEnvelope<CoreCommand>, event_type: &str) {
    let CoreCommand::RespondToApproval(command) = &envelope.payload else {
        unreachable!()
    };
    store.append(AppendRequest {
        command_id: CommandId::new(), idempotency_key: CommandId::new().to_string(), request_fingerprint: None,
        expected_stream_version: None, artifacts: vec![], events: vec![NewEvent::hub(
            StreamRef {stream_type: StreamType::Task, stream_id: command.task_id.to_string()}, event_type,
            json!({"commandId":envelope.command_id,"turnId":command.turn_id,"approvalId":command.approval_id,"requestedAtSequence":command.requested_at_sequence}),
        )],
    }).await.unwrap();
}

#[tokio::test]
async fn approval_receipt_distinguishes_missing_unknown_sent_and_failed_without_mutations() {
    for state in ["approval.delivery_sent", "approval.delivery_failed"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let core = Core::new(store.clone());
        let envelope = command();
        let query = core.prepare_approval_response(&envelope).unwrap();
        let serialized = serde_json::to_string(&query).unwrap();
        assert!(!serialized.contains("private-answer"));
        assert!(!serialized.contains("chosen-option"));
        assert!(core.approval_receipt(&query).await.unwrap().is_none());
        assert_eq!(store.event_head().await.unwrap(), 0);
        // Simulate process loss after durable acceptance but before delivery.
        accepted(&store, &envelope, query.request_fingerprint.clone()).await;
        assert_eq!(
            core.approval_receipt(&query)
                .await
                .unwrap()
                .unwrap()
                .delivery,
            ApprovalDelivery::Unknown
        );
        delivered(&store, &envelope, state).await;
        let head = store.event_head().await.unwrap();
        drop(core);
        drop(store);
        let store = Arc::new(SqliteStore::open(path).unwrap());
        let core = Core::new(store.clone());
        let receipt = core.approval_receipt(&query).await.unwrap().unwrap();
        assert_eq!(
            receipt.delivery,
            if state.ends_with("sent") {
                ApprovalDelivery::Sent
            } else {
                ApprovalDelivery::Failed
            }
        );
        assert_eq!(receipt.acknowledgement.command_id, envelope.command_id);
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(!serialized.contains("private-answer"));
        assert!(!serialized.contains("chosen-option"));
        assert_eq!(store.event_head().await.unwrap(), head);
    }
}

#[tokio::test]
async fn approval_receipt_rejects_changed_identity_scope_and_conflicting_delivery() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Core::new(store.clone());
    let envelope = command();
    let query = core.prepare_approval_response(&envelope).unwrap();
    accepted(&store, &envelope, query.request_fingerprint.clone()).await;
    let mut variants = vec![query.clone(); 5];
    variants[0].request_fingerprint = "v1:other".into();
    variants[1].turn_id = TurnId::new();
    variants[2].approval_id = "different-request".into();
    variants[3].requested_at_sequence += 1;
    variants[4].task_id = TaskId::new();
    for changed in variants {
        assert!(core.approval_receipt(&changed).await.is_err());
    }
    assert_eq!(store.event_head().await.unwrap(), 1);
    let mut unrelated = envelope.clone();
    unrelated.command_id = CommandId::new();
    delivered(&store, &unrelated, "approval.delivery_sent").await;
    assert_eq!(
        core.approval_receipt(&query)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        ApprovalDelivery::Unknown
    );
    delivered(&store, &envelope, "approval.delivery_sent").await;
    delivered(&store, &envelope, "approval.delivery_failed").await;
    assert!(core.approval_receipt(&query).await.is_err());
    assert_eq!(store.event_head().await.unwrap(), 4);
}

#[tokio::test]
async fn prepare_approval_requires_original_source_and_version_without_admission() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Core::new(store.clone());
    let original = command();
    let query = core.prepare_approval_response(&original).unwrap();
    assert_eq!(
        query.request_fingerprint,
        agent_core::command_identity::fingerprint(&original).unwrap()
    );
    let mut variants = vec![original.clone(); 5];
    variants[0].schema_version = u16::MAX;
    variants[1].expected_stream_version = None;
    variants[2].actor = ActorRef::SystemRecovery;
    variants[3].idempotency_key = "invalid".into();
    if let CoreCommand::RespondToApproval(command) = &mut variants[4].payload {
        command.requested_at_sequence = 0;
    }
    for changed in variants {
        assert!(core.prepare_approval_response(&changed).is_err());
    }
    assert_eq!(store.event_head().await.unwrap(), 0);
    assert!(
        store
            .command_ack(&original.idempotency_key)
            .await
            .unwrap()
            .is_none()
    );
}

fn acceptance_request(
    envelope: &CommandEnvelope<CoreCommand>,
    fingerprint: String,
) -> AppendRequest {
    let CoreCommand::RespondToApproval(command) = &envelope.payload else {
        unreachable!()
    };
    let mut event = NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Task,
            stream_id: command.task_id.to_string(),
        },
        "approval.resolved",
        json!(command),
    );
    event.causation_id = Some(envelope.command_id.to_string());
    AppendRequest {
        command_id: envelope.command_id.clone(),
        idempotency_key: envelope.idempotency_key.clone(),
        request_fingerprint: Some(fingerprint),
        expected_stream_version: envelope.expected_stream_version,
        artifacts: vec![],
        events: vec![event],
    }
}
async fn task_command(core: &Arc<Core>) -> CommandEnvelope<CoreCommand> {
    let project_id = agent_domain::ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        agent_protocol::RegisterProject {
            project_id: project_id.clone(),
            display_name: "fixture".into(),
            path: "/tmp/fixture".into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let mut envelope = command();
    let CoreCommand::RespondToApproval(command) = &envelope.payload else {
        unreachable!()
    };
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        agent_protocol::CreateTask {
            task_id: command.task_id.clone(),
            project_id,
            title: "fixture".into(),
            preferred_executor: agent_domain::ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                installation_id: None,
            },
        },
    )))
    .await
    .unwrap();
    envelope.expected_stream_version = Some(1);
    envelope
}
#[tokio::test]
async fn approval_invalidation_blocks_original_version_after_reopen_without_answer_storage() {
    use agent_core::approval_receipts::ApprovalInvalidation;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let envelope = task_command(&core).await;
    let query = core.prepare_approval_response(&envelope).unwrap();
    assert!(core.invalidate_approval_response(&query, 2).await.is_err());
    assert!(matches!(
        core.invalidate_approval_response(&query, 0).await.unwrap(),
        ApprovalInvalidation::Invalidated
    ));
    let head = store.event_head().await.unwrap();
    assert!(matches!(
        core.invalidate_approval_response(&query, 0).await.unwrap(),
        ApprovalInvalidation::Invalidated
    ));
    assert_eq!(head, store.event_head().await.unwrap());
    let events = core.sync_events(0, 100).await.unwrap();
    let barrier = events
        .iter()
        .find(|e| e.event_type == "approval.response_invalidated")
        .unwrap();
    let text = serde_json::to_string(barrier).unwrap();
    assert!(!text.contains("private-answer"));
    assert!(!text.contains("chosen-option"));
    drop(core);
    drop(store);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    assert!(matches!(
        store
            .append(acceptance_request(
                &envelope,
                query.request_fingerprint.clone()
            ))
            .await,
        Err(agent_core::RepositoryError::Conflict { .. })
    ));
    assert!(core.approval_receipt(&query).await.unwrap().is_none());
    assert_eq!(head, store.event_head().await.unwrap());
    assert_eq!(core.bootstrap().await.unwrap().tasks[0].stream_version, 2);
}
#[tokio::test]
async fn approval_invalidation_and_acceptance_cannot_both_win_the_original_version() {
    use agent_core::approval_receipts::ApprovalInvalidation;
    for acceptance_first in [false, true] {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let envelope = task_command(&core).await;
        let query = core.prepare_approval_response(&envelope).unwrap();
        let (invalidated, accepted) = tokio::join!(
            async {
                if acceptance_first {
                    tokio::task::yield_now().await;
                }
                core.invalidate_approval_response(&query, 1).await.unwrap()
            },
            store.append(acceptance_request(
                &envelope,
                query.request_fingerprint.clone()
            ))
        );
        match invalidated {
            ApprovalInvalidation::Accepted { receipt } => {
                assert_eq!(receipt.acknowledgement, accepted.unwrap().ack);
                assert_eq!(receipt.delivery, ApprovalDelivery::Unknown);
                assert!(acceptance_first);
            }
            ApprovalInvalidation::Invalidated => {
                assert!(matches!(
                    accepted,
                    Err(agent_core::RepositoryError::Conflict { .. })
                ));
                assert!(!acceptance_first);
            }
        }
        let before = store.event_head().await.unwrap();
        core.invalidate_approval_response(&query, 1).await.unwrap();
        assert_eq!(store.event_head().await.unwrap(), before);
    }
}

#[tokio::test]
async fn approval_invalidation_retry_does_not_invalidate_a_newer_decision_version() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = task_command(&core).await;
    let query = core.prepare_approval_response(&original).unwrap();
    core.invalidate_approval_response(&query, 1).await.unwrap();
    let mut next = original.clone();
    next.command_id = CommandId::new();
    next.idempotency_key = CommandId::new().to_string();
    next.expected_stream_version = Some(2);
    let new_query = core.prepare_approval_response(&next).unwrap();
    let head = store.event_head().await.unwrap();
    core.invalidate_approval_response(&query, 1).await.unwrap();
    assert_eq!(store.event_head().await.unwrap(), head);
    store
        .append(acceptance_request(
            &next,
            new_query.request_fingerprint.clone(),
        ))
        .await
        .unwrap();
    assert!(core.approval_receipt(&new_query).await.unwrap().is_some());
    let mut changed = query.clone();
    changed.turn_id = TurnId::new();
    assert!(
        core.invalidate_approval_response(&changed, 1)
            .await
            .is_err()
    );
}
