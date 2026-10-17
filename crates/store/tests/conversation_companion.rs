use agent_core::{AppendRequest, Core, CoreRepository};
use agent_domain::{CommandId, ExecutorRef, ProjectId, RunId, TaskId, TurnId};
use agent_protocol::{
    CommandEnvelope, CoreCommand, CreateTask, NewEvent, RegisterProject, RenameTask, StreamRef,
    StreamType,
};
use agent_store::SqliteStore;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::sync::Arc;

async fn setup(store: Arc<SqliteStore>) -> (Arc<Core>, TaskId) {
    let core = Arc::new(Core::new(store));
    let project_id = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project_id.clone(),
            display_name: "Fixture".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let task_id = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task_id.clone(),
            project_id,
            title: "Original".into(),
            preferred_executor: ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("codex").unwrap(),
                installation_id: None,
            },
        },
    )))
    .await
    .unwrap();
    (core, task_id)
}
async fn append(store: &SqliteStore, task: &TaskId, kind: &str, payload: Value) {
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: CommandId::new().to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: task.to_string(),
                },
                kind,
                payload,
            )],
        })
        .await
        .unwrap();
}
async fn message(store: &SqliteStore, task: &TaskId, prompt: &str) {
    append(store, task, "turn.queued", json!({"taskId":task, "turnId":TurnId::new(), "runId":RunId::new(), "prompt":prompt, "executor":{"providerId":"codex","installationId":null},"startedAt":Utc::now()})).await;
}

#[tokio::test]
async fn rename_survives_messages_reopen_and_projection_rebuild() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.db");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let (core, task) = setup(store.clone()).await;
    message(&store, &task, "Automatic title").await;
    assert_eq!(
        core.bootstrap().await.unwrap().tasks[0].title,
        "Automatic title"
    );
    let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
    for invalid in [
        String::new(),
        " ".into(),
        "line\nbreak".into(),
        "字".repeat(121),
    ] {
        let mut command = CommandEnvelope::local(CoreCommand::RenameTask(RenameTask {
            task_id: task.clone(),
            title: invalid,
        }));
        command.expected_stream_version = Some(version);
        assert!(core.execute(command).await.is_err());
    }
    let mut command = CommandEnvelope::local(CoreCommand::RenameTask(RenameTask {
        task_id: task.clone(),
        title: "  手动名称  ".into(),
    }));
    assert!(core.execute(command.clone()).await.is_err());
    command.expected_stream_version = Some(version);
    let ack = core.execute(command.clone()).await.unwrap();
    assert_eq!(core.execute(command).await.unwrap(), ack);
    let mut stale = CommandEnvelope::local(CoreCommand::RenameTask(RenameTask {
        task_id: task.clone(),
        title: "Stale overwrite".into(),
    }));
    stale.expected_stream_version = Some(version);
    assert!(core.execute(stale).await.is_err());
    message(&store, &task, "Must not overwrite").await;
    drop(core);
    drop(store);
    let store = SqliteStore::open(path).unwrap();
    for rebuild in [false, true] {
        if rebuild {
            store.rebuild_projections().unwrap();
        }
        let snapshot = store.bootstrap().await.unwrap();
        assert_eq!(snapshot.tasks[0].title, "手动名称");
        assert_eq!(snapshot.companion.workspaces[0].display_name, "手动名称");
        assert!(snapshot.tasks[0].active_primary_run.is_some());
        assert_eq!(store.task_timeline(&task).await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn companion_uses_latest_review_status_and_keeps_primary_independent() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (core, task) = setup(store.clone()).await;
    message(&store, &task, "Primary running").await;
    let a = RunId::new();
    let b = RunId::new();
    let review = |run: &RunId, status: &str, time| {
        json!({"taskId":task,"review":{
            "request":{"runId":run,"executor":{"providerId":"kimi"},"kind":"security"},
            "status":status,"createdAt":Utc::now(),"finishedAt":time,
            "workflow":{"groups":[{"status":"completed"},{"status":"running"}]},"result":""
        }})
    };
    append(
        &store,
        &task,
        "review.updated",
        review(&a, "queued", None::<chrono::DateTime<Utc>>),
    )
    .await;
    append(&store, &task, "review.updated", review(&a, "running", None)).await;
    append(&store, &task, "review.updated", review(&b, "running", None)).await;
    let snapshot = core.bootstrap().await.unwrap();
    let row = &snapshot.companion.workspaces[0];
    assert_eq!(row.secondary_agent_count, 2);
    assert_eq!(row.reviews.len(), 2);
    assert_eq!(
        (row.reviews[0].completed_groups, row.reviews[0].total_groups),
        (1, 2)
    );
    assert!(row.active_primary_run.is_some());
    append(
        &store,
        &task,
        "review.updated",
        review(&a, "cancelled", Some(Utc::now())),
    )
    .await;
    append(
        &store,
        &task,
        "review.updated",
        review(&b, "failed", Some(Utc::now())),
    )
    .await;
    store.rebuild_projections().unwrap();
    let snapshot = core.bootstrap().await.unwrap();
    let row = &snapshot.companion.workspaces[0];
    assert_eq!(row.secondary_agent_count, 0);
    assert_eq!(
        row.reviews
            .iter()
            .map(|r| r.status.as_str())
            .collect::<Vec<_>>(),
        vec!["failed", "cancelled"]
    );
    assert!(row.active_primary_run.is_some());
    append(
        &store,
        &task,
        "review.updated",
        review(&a, "cancelled", Some(Utc::now() - Duration::seconds(61))),
    )
    .await;
    append(
        &store,
        &task,
        "review.updated",
        review(&b, "failed", Some(Utc::now() - Duration::seconds(61))),
    )
    .await;
    assert!(
        core.bootstrap().await.unwrap().companion.workspaces[0]
            .reviews
            .is_empty()
    );
}
