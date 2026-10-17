use agent_core::{AppendRequest, Core, CoreRepository, command_identity::fingerprint};
use agent_domain::{CommandId, ProjectId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, NewEvent, RegisterProject, StreamRef, StreamType,
};
use agent_store::SqliteStore;
use serde_json::json;
use std::sync::Arc;

fn command() -> CommandEnvelope<CoreCommand> {
    CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
        project_id: ProjectId::new(),
        display_name: "original".into(),
        path: "/project".into(),
        additional_directories: vec![],
    }))
}
fn append(envelope: &CommandEnvelope<CoreCommand>, bound: bool) -> AppendRequest {
    let CoreCommand::RegisterProject(project) = &envelope.payload else {
        unreachable!()
    };
    AppendRequest {
        request_fingerprint: bound.then(|| fingerprint(envelope).unwrap()),
        command_id: envelope.command_id.clone(),
        idempotency_key: envelope.idempotency_key.clone(),
        expected_stream_version: Some(0),
        artifacts: vec![],
        events: vec![NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Project,
                stream_id: project.project_id.to_string(),
            },
            "project.registered",
            json!(project),
        )],
    }
}
#[tokio::test]
async fn command_identity_survives_reopen_and_allows_changed_delivery_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = command();
    let ack = core.execute(original.clone()).await.unwrap();
    assert!(ack.request_fingerprint.as_ref().unwrap().starts_with("v1:"));
    drop(core);
    drop(store);
    let store = Arc::new(SqliteStore::open(path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let mut retry = original.clone();
    retry.command_id = CommandId::new();
    retry.issued_at += chrono::Duration::seconds(30);
    assert_eq!(core.execute(retry).await.unwrap(), ack);
    assert_eq!(store.event_head().await.unwrap(), 1);
    assert_eq!(store.bootstrap().await.unwrap().projects.len(), 1);
}
#[tokio::test]
async fn command_identity_rejects_changed_payload_actor_version_and_variant() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = command();
    core.execute(original.clone()).await.unwrap();
    let mut changed = original.clone();
    if let CoreCommand::RegisterProject(project) = &mut changed.payload {
        project.display_name = "different".into();
    }
    assert!(
        core.execute(changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同")
    );
    let mut changed = original.clone();
    changed.actor = ActorRef::SystemRecovery;
    assert!(
        core.execute(changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同")
    );
    let mut changed = original.clone();
    changed.expected_stream_version = Some(0);
    assert!(
        core.execute(changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同")
    );
    let mut changed = original.clone();
    if let CoreCommand::RegisterProject(project) = &changed.payload {
        changed.payload = CoreCommand::UpdateProject(project.clone());
    }
    assert!(
        core.execute(changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同")
    );
    assert_eq!(store.event_head().await.unwrap(), 1);
    assert_eq!(
        store.bootstrap().await.unwrap().projects[0].display_name,
        "original"
    );
}
#[tokio::test]
async fn command_identity_is_checked_inside_append_transaction_before_dedup_return() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let first = command();
    let mut changed = first.clone();
    if let CoreCommand::RegisterProject(project) = &mut changed.payload {
        project.display_name = "other".into();
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut handles = vec![];
    for value in [first.clone(), changed.clone()] {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            store.append(append(&value, true)).await
        }));
    }
    let one = handles.remove(0).await.unwrap();
    let two = handles.remove(0).await.unwrap();
    assert_ne!(one.is_ok(), two.is_ok());
    let winner = if one.is_ok() { first } else { changed };
    assert!(store.append(append(&winner, true)).await.unwrap().duplicate);
    assert!(store.append(append(&winner, false)).await.is_err());
    assert_eq!(store.event_head().await.unwrap(), 1);
}
#[tokio::test]
async fn command_identity_does_not_guess_legacy_or_unrelated_internal_receipts() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let original = command();
    store.append(append(&original, false)).await.unwrap();
    let core = Arc::new(Core::new(store.clone()));
    assert!(
        core.execute(original.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("旧操作回执")
    );
    assert!(
        store
            .command_ack(&original.idempotency_key)
            .await
            .unwrap()
            .unwrap()
            .request_fingerprint
            .is_none()
    );
    assert_eq!(store.event_head().await.unwrap(), 1);
}

#[tokio::test]
async fn command_receipt_reads_original_acceptance_after_reopen_without_reexecution() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("receipts.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let mut original = command();
    original.expected_stream_version = Some(0);
    assert_eq!(core.command_receipt(&original).await.unwrap(), None);
    assert_eq!(store.event_head().await.unwrap(), 0);
    assert!(store.bootstrap().await.unwrap().projects.is_empty());
    let ack = core.execute(original.clone()).await.unwrap();
    // Advance the resource so repeating current version validation would fail.
    let CoreCommand::RegisterProject(mut project) = original.payload.clone() else {
        unreachable!()
    };
    project.display_name = "later name".into();
    let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(project));
    update.expected_stream_version = Some(1);
    core.execute(update).await.unwrap();
    drop(core);
    drop(store);

    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let head = store.event_head().await.unwrap();
    original.command_id = CommandId::new();
    original.issued_at += chrono::Duration::seconds(45);
    assert_eq!(core.command_receipt(&original).await.unwrap(), Some(ack));
    assert_eq!(store.event_head().await.unwrap(), head);
    assert_eq!(
        store.bootstrap().await.unwrap().projects[0].display_name,
        "later name"
    );
}

#[tokio::test]
async fn command_receipt_validates_request_identity_and_does_not_guess_legacy_records() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = command();
    core.execute(original.clone()).await.unwrap();
    let mut changed_payload = original.clone();
    if let CoreCommand::RegisterProject(project) = &mut changed_payload.payload {
        project.display_name = "other".into();
    }
    let mut changed_actor = original.clone();
    changed_actor.actor = ActorRef::SystemRecovery;
    let mut changed_version = original.clone();
    changed_version.expected_stream_version = Some(8);
    for changed in [changed_payload, changed_actor, changed_version] {
        assert!(
            core.command_receipt(&changed)
                .await
                .unwrap_err()
                .to_string()
                .contains("不同")
        );
    }
    let legacy = command();
    store.append(append(&legacy, false)).await.unwrap();
    assert!(
        core.command_receipt(&legacy)
            .await
            .unwrap_err()
            .to_string()
            .contains("旧操作回执")
    );
    let head = store.event_head().await.unwrap();
    let mut invalid = command();
    invalid.schema_version = u16::MAX;
    assert!(core.command_receipt(&invalid).await.is_err());
    invalid.schema_version = agent_protocol::SCHEMA_VERSION;
    invalid.idempotency_key = " ".into();
    assert!(core.command_receipt(&invalid).await.is_err());
    assert_eq!(store.event_head().await.unwrap(), head);
}
