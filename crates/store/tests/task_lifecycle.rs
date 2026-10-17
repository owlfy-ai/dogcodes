use agent_core::{
    AppendRequest, Core, CoreRepository,
    turn_review::{ReviewKind, ReviewRequest, ReviewRun, ReviewStatus},
};
use agent_domain::{
    CommandId, ExecutorRef, ProjectId, ProviderId, RunId, SessionMapId, TaskId, TurnId,
};
use agent_protocol::{
    CommandEnvelope, CoreCommand, CreateTask, NewEvent, RegisterProject, StreamRef, StreamType,
    TaskLifecycle,
};
use agent_store::SqliteStore;
use chrono::Utc;
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
            title: "Keep me".into(),
            preferred_executor: executor(),
        },
    )))
    .await
    .unwrap();
    (core, task_id)
}
fn executor() -> ExecutorRef {
    ExecutorRef {
        provider_id: ProviderId::parse("codex").unwrap(),
        installation_id: None,
    }
}
fn request(task: &TaskId, kind: &str, payload: Value, version: Option<u64>) -> AppendRequest {
    AppendRequest {
        request_fingerprint: None,
        command_id: CommandId::new(),
        idempotency_key: CommandId::new().to_string(),
        expected_stream_version: version,
        artifacts: vec![],
        events: vec![NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task.to_string(),
            },
            kind,
            payload,
        )],
    }
}
fn command(task: &TaskId, restore: bool, version: Option<u64>) -> CommandEnvelope<CoreCommand> {
    let data = TaskLifecycle {
        task_id: task.clone(),
    };
    let mut envelope = CommandEnvelope::local(if restore {
        CoreCommand::RestoreTask(data)
    } else {
        CoreCommand::DeleteTask(data)
    });
    envelope.expected_stream_version = version;
    envelope
}
fn review(task: &TaskId) -> ReviewRun {
    ReviewRun {
        activities: vec![],
        activity_revision: 0,
        workspace_review: false,
        request: ReviewRequest {
            task_id: task.clone(),
            turn_id: None,
            source_snapshot: Some("frozen".into()),
            run_id: RunId::new(),
            executor: executor(),
            kind: ReviewKind::Security,
            retry_of: None,
            recheck_of: None,
        },
        status: ReviewStatus::Queued,
        created_at: Utc::now(),
        finished_at: None,
        native_session_id: Some("native-review".into()),
        source_sequence: 1,
        source_artifact: None,
        input_sha256: "fixed".into(),
        coverage: "all".into(),
        result: "frozen report".into(),
        report: None,
        workflow: None,
        error: None,
        repairs: vec![],
    }
}
#[tokio::test]
async fn permanent_deletion_removes_history_reviews_and_sessions_after_rebuild() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.db");
    let file = directory.path().join("source.rs");
    std::fs::write(&file, "unchanged").unwrap();
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let (core, task) = setup(store.clone()).await;
    let turn = TurnId::new();
    let run = RunId::new();
    store.append(request(&task,"turn.queued",json!({"taskId":task,"turnId":turn,"runId":run,"prompt":"Stored message","executor":executor(),"startedAt":Utc::now()}),None)).await.unwrap();
    store.append(request(&task,"session.ready",json!({"taskId":task,"sessionMapId":SessionMapId::new(),"executor":executor(),"nativeSessionId":"native-preserved","workspaceLocator":"/tmp","createdAt":Utc::now(),"resumeCapability":"supported","status":"active"}),None)).await.unwrap();
    store
        .append(request(
            &task,
            "turn.completed",
            json!({"taskId":task,"turnId":turn,"runId":run,"completedAt":Utc::now()}),
            None,
        ))
        .await
        .unwrap();
    let mut report = review(&task);
    report.status = ReviewStatus::Completed;
    report.finished_at = Some(Utc::now());
    store
        .append(request(
            &task,
            "review.updated",
            json!({"taskId":task,"review":report}),
            None,
        ))
        .await
        .unwrap();
    assert!(!core.timeline(&task).await.unwrap().is_empty());
    let snapshot = core.bootstrap().await.unwrap();
    assert_eq!(snapshot.session_maps.len(), 1);
    assert!(snapshot.companion.recent_completion.is_some());
    let version = snapshot.tasks[0].stream_version;
    assert!(core.execute(command(&task, false, None)).await.is_err());
    assert!(
        core.execute(command(&task, false, Some(version - 1)))
            .await
            .is_err()
    );
    let delete = command(&task, false, Some(version));
    let ack = core.execute(delete.clone()).await.unwrap();
    assert_eq!(core.execute(delete).await.unwrap(), ack);
    assert!(core.timeline(&task).await.is_err());
    assert!(core.turn_reviews(&task).await.is_err());
    assert!(
        store
            .append(request(
                &task,
                "review.started",
                json!({"taskId":task,"review":review(&task)}),
                None
            ))
            .await
            .is_err()
    );
    drop(core);
    drop(store);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    for rebuild in [false, true] {
        if rebuild {
            store.rebuild_projections().unwrap();
        }
        let snapshot = store.bootstrap().await.unwrap();
        assert!(snapshot.tasks.is_empty());
        assert!(snapshot.session_maps.is_empty());
        assert!(snapshot.active_runs.is_empty());
        assert!(snapshot.companion.workspaces.is_empty());
        assert!(snapshot.companion.recent_completion.is_none());
        assert_eq!(snapshot.projects[0].task_count, 0);
        assert!(snapshot.deleted_tasks.is_empty());
    }
    let core = Arc::new(Core::new(store.clone()));
    assert!(core.execute(command(&task, true, None)).await.is_err());
    assert!(
        core.execute(command(&task, true, Some(version)))
            .await
            .is_err()
    );
    assert!(
        core.execute(command(&task, true, Some(version + 1)))
            .await
            .is_err()
    );
    let events = store
        .task_events(&task, 0, u64::MAX / 2, 1000)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "task.deleted");
    assert_eq!(events[0].payload["permanent"], true);
    assert_eq!(events[0].payload["cleanupPending"], false);
    assert!(store.task_timeline(&task).await.unwrap().is_empty());
    assert!(
        store
            .append(request(
                &task,
                "task.restored",
                json!({"taskId":task}),
                Some(version + 1)
            ))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "unchanged");
}
#[tokio::test]
async fn active_primary_and_queued_running_cancelling_reviews_prevent_deletion() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (core, task) = setup(store.clone()).await;
    let mut report = review(&task);
    for status in [
        ReviewStatus::Queued,
        ReviewStatus::Running,
        ReviewStatus::Cancelling,
    ] {
        report.status = status;
        let kind = if status == ReviewStatus::Queued {
            "review.started"
        } else {
            "review.updated"
        };
        store
            .append(request(
                &task,
                kind,
                json!({"taskId":task,"review":report}),
                None,
            ))
            .await
            .unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.companion.workspaces[0].secondary_agent_count, 1);
        let version = snapshot.tasks[0].stream_version;
        assert!(
            core.execute(command(&task, false, Some(version)))
                .await
                .is_err()
        );
        // Exercise the transactional admission guard, bypassing Core's earlier check.
        assert!(
            store
                .append(request(
                    &task,
                    "task.deleted",
                    json!({"taskId":task}),
                    Some(version)
                ))
                .await
                .is_err()
        );
    }
    report.status = ReviewStatus::Cancelled;
    store
        .append(request(
            &task,
            "review.updated",
            json!({"taskId":task,"review":report}),
            None,
        ))
        .await
        .unwrap();
    let turn = TurnId::new();
    let run = RunId::new();
    store.append(request(&task,"turn.queued",json!({"taskId":task,"turnId":turn,"runId":run,"prompt":"Running","executor":executor(),"startedAt":Utc::now()}),None)).await.unwrap();
    let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
    assert!(
        core.execute(command(&task, false, Some(version)))
            .await
            .is_err()
    );
    assert!(
        store
            .append(request(
                &task,
                "task.deleted",
                json!({"taskId":task}),
                Some(version)
            ))
            .await
            .is_err()
    );
    store
        .append(request(
            &task,
            "turn.completed",
            json!({"taskId":task,"turnId":turn,"runId":run,"completedAt":Utc::now()}),
            None,
        ))
        .await
        .unwrap();
    let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
    core.execute(command(&task, false, Some(version)))
        .await
        .unwrap();
    assert!(store.append(request(&task,"turn.queued",json!({"taskId":task,"turnId":TurnId::new(),"runId":RunId::new(),"prompt":"Late admission","executor":executor(),"startedAt":Utc::now()}),Some(version+1))).await.is_err());
}

#[tokio::test]
async fn deletes_unique_artifacts_but_keeps_shared_ids_and_deduplicated_blobs() {
    use agent_core::NewArtifact;
    use agent_domain::ArtifactId;
    use agent_protocol::ArtifactMetadata;
    use sha2::{Digest, Sha256};
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("artifacts.db");
    let store = Arc::new(SqliteStore::open(&database).unwrap());
    let (core, first) = setup(store.clone()).await;
    let (_, second) = setup(store.clone()).await;
    let artifact = |bytes: &[u8]| NewArtifact {
        metadata: ArtifactMetadata {
            artifact_id: ArtifactId::new(),
            media_type: "application/json".into(),
            payload_sha256: hex::encode(Sha256::digest(bytes)),
            byte_length: bytes.len() as u64,
            created_at: Utc::now(),
            redaction_ruleset_version: 0,
            redaction_count: 0,
        },
        content: bytes.to_vec(),
    };
    let unique = artifact(b"unique diff");
    let shared = artifact(b"shared diff");
    let duplicate = artifact(b"shared diff");
    for item in [&unique, &shared] {
        let mut append = request(
            &first,
            "workspace.turn_changes_captured",
            json!({"taskId":first,"turnId":TurnId::new(),"artifact":item.metadata}),
            None,
        );
        append.artifacts.push(item.clone());
        store.append(append).await.unwrap();
    }
    store
        .append(request(
            &second,
            "workspace.turn_changes_captured",
            json!({"taskId":second,"turnId":TurnId::new(),"artifact":shared.metadata}),
            None,
        ))
        .await
        .unwrap();
    let mut append = request(
        &second,
        "workspace.turn_changes_captured",
        json!({"taskId":second,"turnId":TurnId::new(),"artifact":duplicate.metadata}),
        None,
    );
    append.artifacts.push(duplicate.clone());
    store.append(append).await.unwrap();
    let version = core
        .bootstrap()
        .await
        .unwrap()
        .tasks
        .iter()
        .find(|t| t.task_id == first)
        .unwrap()
        .stream_version;
    core.execute(command(&first, false, Some(version)))
        .await
        .unwrap();
    assert!(
        store
            .artifact(&unique.metadata.artifact_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .artifact(&shared.metadata.artifact_id)
            .await
            .unwrap()
            .unwrap()
            .content,
        shared.content
    );
    assert_eq!(
        store
            .artifact(&duplicate.metadata.artifact_id)
            .await
            .unwrap()
            .unwrap()
            .content,
        duplicate.content
    );
    let inspection = rusqlite::Connection::open(&database).unwrap();
    assert_eq!(
        inspection
            .query_row("SELECT COUNT(*) FROM artifact_blobs", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    store.rebuild_projections().unwrap();
    assert!(!core.timeline(&second).await.unwrap().is_empty());
    let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
    core.execute(command(&second, false, Some(version)))
        .await
        .unwrap();
    assert!(
        store
            .artifact(&shared.metadata.artifact_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .artifact(&duplicate.metadata.artifact_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        inspection
            .query_row("SELECT COUNT(*) FROM artifact_blobs", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        inspection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn snapshot_cleanup_failure_remains_durable_and_restart_retries() {
    use agent_workspace::{
        GitWorkspaceContextProvider,
        review::{ReviewStore, SnapshotBudget},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source"), "original").unwrap();
    let directory = temp.path().join("review");
    let store = Arc::new(SqliteStore::open(temp.path().join("state.db")).unwrap());
    let (_, task) = setup(store.clone()).await;
    let review = ReviewStore::open(directory.clone()).unwrap();
    review
        .ensure_baseline(task.as_str(), &root, SnapshotBudget::default())
        .unwrap();
    let provider =
        Arc::new(GitWorkspaceContextProvider::default().with_review_directory(directory.clone()));
    let core = Arc::new(Core::with_workspace_context(
        store.clone(),
        provider.clone(),
    ));
    // A live reader prevents exclusive cleanup, but cannot undo DB deletion.
    let envelope = command(&task, false, Some(1));
    assert!(core.execute(envelope.clone()).await.is_err());
    assert!(store.bootstrap().await.unwrap().tasks.is_empty());
    assert_eq!(
        store.pending_task_cleanups().await.unwrap(),
        vec![task.clone()]
    );
    drop(review);
    drop(core);
    let restarted = Arc::new(Core::with_workspace_context(store.clone(), provider));
    // Reading a lost acknowledgement must not resume now-unblocked cleanup.
    assert!(
        restarted
            .command_receipt(&envelope)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store.pending_task_cleanups().await.unwrap(),
        vec![task.clone()]
    );
    let unsent = command(&task, false, Some(1));
    assert!(restarted.command_receipt(&unsent).await.unwrap().is_none());
    assert!(
        store
            .command_ack(&unsent.idempotency_key)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.pending_task_cleanups().await.unwrap(),
        vec![task.clone()]
    );
    restarted.recover_interrupted_turns().await.unwrap();
    assert!(store.pending_task_cleanups().await.unwrap().is_empty());
    assert!(
        ReviewStore::open(directory)
            .unwrap()
            .load_baseline(task.as_str(), &root)
            .is_err()
    );
    // Original acknowledgement remains idempotent after completed cleanup.
    assert!(restarted.execute(envelope).await.is_ok());
    let retry = command(&task, false, Some(1));
    let ack = restarted.execute(retry.clone()).await.unwrap();
    assert_eq!(restarted.execute(retry).await.unwrap(), ack);
    assert_eq!(std::fs::read(root.join("source")).unwrap(), b"original");
}
