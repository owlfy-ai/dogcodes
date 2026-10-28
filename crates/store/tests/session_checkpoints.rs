use agent_core::{AppendRequest, Core, CoreRepository};
use agent_domain::{
    CommandId, ExecutorRef, InstallationId, ProjectId, ProviderId, RunId, TaskId, TurnId,
};
use agent_protocol::{NewEvent, SessionMapStatus, StreamRef, StreamType};
use agent_store::SqliteStore;
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::Arc;

async fn append(store: &SqliteStore, stream: StreamRef, kind: &str, payload: Value) {
    let event = NewEvent::hub(stream, kind, payload);
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
}

async fn checkpoint_case(kind: &str, status: SessionMapStatus, reusable: bool, installed: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let project = ProjectId::new();
    let task = TaskId::new();
    let turn = TurnId::new();
    let run = RunId::new();
    let executor = |provider: &str, installation_id| ExecutorRef {
        provider_id: ProviderId::parse(provider).unwrap(),
        installation_id,
    };
    let instances = [
        executor("scripted", Some(InstallationId::new().to_string())),
        executor("scripted", Some(InstallationId::new().to_string())),
        executor("scripted", None),
        executor("other", Some(InstallationId::new().to_string())),
    ];
    let active = &instances[if installed { 0 } else { 2 }];
    let stream = StreamRef {
        stream_type: StreamType::Task,
        stream_id: task.to_string(),
    };
    append(&store, StreamRef { stream_type: StreamType::Project, stream_id: project.to_string() },
        "project.registered", json!({"projectId":project,"displayName":"private checkpoint fixture","path":directory.path()})).await;
    append(&store, stream.clone(), "task.created", json!({
        "taskId":task,"projectId":project,"title":"Instance checkpoints","preferredExecutor":active,
    })).await;
    for (i, instance) in instances.iter().enumerate() {
        append(&store, stream.clone(), "session.ready", json!({
            "taskId":task,"sessionMapId":agent_domain::SessionMapId::new(),
            "executor":instance,"nativeSessionId":format!("private-native-{i}"),
            "workspaceLocator":directory.path(),"createdAt":Utc::now(),
            "resumeCapability":"supported","status":"active","model":"selected","thinking":"high",
        })).await;
    }
    let before = store.bootstrap().await.unwrap().session_maps;
    assert_eq!(before.len(), instances.len());
    append(
        &store,
        stream.clone(),
        "turn.queued",
        json!({
            "taskId":task,"turnId":turn,"runId":run,"executor":active,
            "prompt":"private fixture prompt, never dispatched","startedAt":Utc::now(),
        }),
    )
    .await;
    if kind == "restart" {
        // Real startup repair over a reopened store; there is no driver or
        // native process to adopt, start, interrupt or replay.
        drop(store);
        let reopened = Arc::new(SqliteStore::open(&path).unwrap());
        Core::new(reopened)
            .recover_interrupted_turns()
            .await
            .unwrap();
    } else {
        append(&store, stream, kind, json!({
            "taskId":task,"turnId":turn,"runId":run,"executor":active,
            "completedAt":Utc::now(),"failedAt":Utc::now(),
            "failureKind":"interrupted","message":"fixture interruption","sessionReusable":reusable,
        })).await;
        drop(store);
    }
    let store = SqliteStore::open(&path).unwrap();
    let after = store.bootstrap().await.unwrap();
    assert!(after.tasks.iter().all(|t| t.active_primary_run.is_none()));
    for original in &before {
        let current = after
            .session_maps
            .iter()
            .find(|m| m.executor == original.executor)
            .unwrap();
        if &current.executor == active {
            assert_eq!(current.status, status);
            assert!(current.last_seen_sequence > original.last_seen_sequence);
            assert_eq!(current.native_session_id, original.native_session_id);
        } else {
            assert_eq!(
                serde_json::to_value(current).unwrap(),
                serde_json::to_value(original).unwrap(),
                "{kind} changed another installation's checkpoint: {:?}",
                current.executor
            );
        }
    }
    let expected = serde_json::to_value(&after.session_maps).unwrap();
    store.rebuild_projections().unwrap();
    assert_eq!(
        serde_json::to_value(store.bootstrap().await.unwrap().session_maps).unwrap(),
        expected
    );
}

#[tokio::test]
async fn completed_turn_only_advances_its_exact_installation_checkpoint() {
    for installed in [true, false] {
        checkpoint_case("turn.completed", SessionMapStatus::Active, false, installed).await;
    }
}

#[tokio::test]
async fn failed_turn_only_detaches_its_exact_installation() {
    for installed in [true, false] {
        checkpoint_case("turn.failed", SessionMapStatus::Detached, false, installed).await;
    }
}

#[tokio::test]
async fn reusable_failure_does_not_advance_another_installations_history_cursor() {
    for installed in [true, false] {
        checkpoint_case("turn.failed", SessionMapStatus::Active, true, installed).await;
    }
}

#[tokio::test]
async fn startup_recovery_only_detaches_the_interrupted_installation() {
    for installed in [true, false] {
        checkpoint_case("restart", SessionMapStatus::Detached, false, installed).await;
    }
}
