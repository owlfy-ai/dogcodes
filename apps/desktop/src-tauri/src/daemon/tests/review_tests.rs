use super::*;
use agent_core::{AppendRequest, CoreRepository, NewArtifact};
use agent_domain::{ArtifactId, CommandId, ExecutorRef, ProviderId};
use agent_protocol::{CreateTask, TaskLifecycle};

async fn rpc(client: &CoreClient, operation: &str, mut arguments: Value) -> Result<Value, String> {
    if matches!(operation, "discard" | "batch_files" | "undo") && arguments.get("request").is_none()
    {
        arguments = json!({"request":arguments});
    }
    if matches!(
        operation,
        "git_action" | "index" | "ignore" | "discard" | "batch_files" | "undo"
    ) && arguments.get("operationId").is_none()
    {
        arguments["operationId"] = json!(uuid::Uuid::new_v4().to_string());
    }
    client
        .call(
            "review",
            json!({"operation":operation,"arguments":arguments}),
        )
        .await
}
fn git(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
async fn register(f: &Fixture, path: &Path) -> (ProjectId, TaskId) {
    let project = ProjectId::new();
    let task = TaskId::new();
    f.core
        .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project.clone(),
                display_name: "review fixture".into(),
                path: path.to_string_lossy().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
    f.core
        .execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project.clone(),
                title: "review".into(),
                preferred_executor: ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
    (project, task)
}
async fn repo(f: &Fixture) -> (PathBuf, ProjectId, TaskId) {
    let path = f.directory.path().join("workspace");
    std::fs::create_dir(&path).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git(&path, &args);
    }
    std::fs::write(path.join("file"), "before\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-qm", "base"]);
    std::fs::write(path.join("file"), "after\n").unwrap();
    let (project, task) = register(f, &path).await;
    (path, project, task)
}
async fn query(client: &CoreClient, task: &TaskId) -> Value {
    let request = serde_json::from_value(
        json!({"operation":"query","arguments":{"taskId":task,"mode":"uncommitted"}}),
    )
    .unwrap();
    let snapshot: crate::review::ReviewSnapshot = client.review(request).await.unwrap();
    serde_json::to_value(snapshot).unwrap()
}
fn file_args(snapshot: &Value) -> Value {
    json!({"taskId":snapshot["taskId"],"snapshotId":snapshot["id"],"rootId":snapshot["roots"][0]["root"]["id"],"path":"file"})
}
async fn lease(client: &CoreClient, snapshot: &Value, id: &str, revision: u64, release: bool) {
    rpc(client, "lease", json!({"taskId":snapshot["taskId"],"snapshotId":snapshot["id"],"leaseId":id,"revision":revision,"release":release})).await.unwrap();
}
#[tokio::test]
async fn review_rpc_shares_frozen_reads_and_independent_leases_across_clients() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let other = CoreClient::Remote(f.directory.path().into());
    let snapshot = query(&f.client, &task).await;
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    lease(&f.client, &snapshot, &a, 1, false).await;
    lease(&other, &snapshot, &b, 1, false).await;
    let args = file_args(&snapshot);
    let request = serde_json::from_value(json!({"operation":"file","arguments":args})).unwrap();
    let typed: crate::review::ReviewFile = other.review(request).await.unwrap();
    let body = serde_json::to_value(typed).unwrap();
    assert_eq!(body["text"]["after"], json!(["after\n"]));
    std::fs::write(path.join("file"), "later\n").unwrap();
    lease(&f.client, &snapshot, &a, 3, true).await;
    lease(&f.client, &snapshot, &a, 2, false).await; // Late renewal is ignored.
    // Fill/evict the cache from another client. The remaining reader protects its frozen bytes.
    for _ in 0..17 {
        let fresh = query(&other, &task).await;
        lease(&other, &fresh, &uuid::Uuid::new_v4().to_string(), 1, true).await;
    }
    assert_eq!(rpc(&other, "file", args.clone()).await.unwrap(), body);
    let (_, other_task) = register(&f, &path).await;
    let mut wrong = args;
    wrong["taskId"] = json!(other_task);
    assert!(rpc(&other, "file", wrong).await.is_err());
    lease(&other, &snapshot, &b, 2, true).await;
    let embedded = CoreClient::local_at(f.core.clone(), &f.directory.path().join("fresh-service"));
    assert!(rpc(&embedded, "file", file_args(&snapshot)).await.is_err());
    let fresh = query(&embedded, &task).await;
    assert_eq!(
        rpc(&embedded, "file", file_args(&fresh)).await.unwrap()["text"]["after"],
        json!(["later\n"])
    );
}
#[tokio::test]
async fn review_rpc_owns_writes_after_reply_disconnect_and_rejects_stale_previews() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let other = CoreClient::Remote(f.directory.path().into());
    let snapshot = query(&f.client, &task).await;
    let mut index = file_args(&snapshot);
    index.as_object_mut().unwrap().remove("path");
    index["paths"] = json!(["file"]);
    index["action"] = json!("stage");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut accepted = index.clone();
    accepted["operationId"] = json!(operation);
    let gate = f.control.review.gate.lock().await;
    let stream = f
        .client
        .connect("review", json!({"operation":"index","arguments":accepted}))
        .await
        .unwrap();
    drop(stream);
    drop(gate);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = rpc(
                &other,
                "file_operation",
                json!({"taskId":task,"operationId":operation}),
            )
            .await
            .unwrap();
            if receipt["outcome"]["status"] == "succeeded" {
                break;
            }
            assert!(
                receipt.is_null() || receipt["outcome"]["status"] == "running",
                "{receipt}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // Replaying the accepted request returns its receipt despite the consumed preview.
    assert!(rpc(&other, "index", accepted).await.unwrap().is_null());
    let fresh = query(&other, &task).await;
    let staged = git(&path, &["diff", "--cached", "--name-only"]);
    assert_eq!(
        staged.trim(),
        "file",
        "accepted index operation did not publish; fresh snapshot: {fresh}"
    );
    assert!(rpc(&other, "index", index).await.is_err());
    assert_eq!(
        rpc(&other, "file", file_args(&snapshot)).await.unwrap()["text"]["before"],
        json!(["before\n"])
    );
    let args = json!({"taskId":task,"rootId":fresh["roots"][0]["root"]["id"],"snapshotId":fresh["id"],"action":"commit","message":"service commit","stageAll":false});
    assert_eq!(rpc(&other, "git_action", args.clone()).await.unwrap(), true);
    assert!(rpc(&other, "git_action", args).await.is_err());
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert!(
        rpc(
            &other,
            "history",
            json!({"taskId":task,"rootId":fresh["roots"][0]["root"]["id"],"skip":0})
        )
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .len()
            >= 2
    );
    assert!(
        rpc(
            &other,
            "branches",
            json!({"taskId":task,"rootId":fresh["roots"][0]["root"]["id"]})
        )
        .await
        .unwrap()
        .is_array()
    );
}
#[tokio::test]
async fn review_rpc_file_actions_scope_checks_and_deletion_use_the_same_host() {
    let f = fixture().await;
    let (path, project, task) = repo(&f).await;
    std::fs::write(path.join("scratch"), "scratch").unwrap();
    let snapshot = query(&f.client, &task).await;
    let args = file_args(&snapshot);
    let mut preview = args.clone();
    preview["version"] = json!("working");
    assert!(
        rpc(&f.client, "preview", preview.clone())
            .await
            .unwrap()
            .to_string()
            .contains("after")
    );
    preview["version"] = json!("head");
    assert!(
        rpc(&f.client, "preview", preview)
            .await
            .unwrap()
            .to_string()
            .contains("before")
    );
    let mut ignore = args.clone();
    ignore["path"] = json!("scratch");
    rpc(&f.client, "ignore", ignore).await.unwrap();
    assert!(
        std::fs::read_to_string(path.join(".gitignore"))
            .unwrap()
            .contains("scratch")
    );
    let fresh = query(&f.client, &task).await;
    let mut discard = file_args(&fresh);
    discard["confirmed"] = json!(false);
    assert!(rpc(&f.client, "discard", discard.clone()).await.is_err());
    discard["confirmed"] = json!(true);
    rpc(&f.client, "discard", discard).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(path.join("file")).unwrap(),
        "before\n"
    );
    let fresh = query(&f.client, &task).await;
    let batch = json!({"taskId":task,"snapshotId":fresh["id"],"rootId":fresh["roots"][0]["root"]["id"],"paths":[".gitignore"],"action":"discard","confirmed":true});
    assert_eq!(
        rpc(&f.client, "batch_files", batch).await.unwrap()["completed"],
        json!([".gitignore"])
    );
    assert!(!path.join(".gitignore").exists());
    let sub = path.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("allowed"), "ok").unwrap();
    let bootstrap = f.core.bootstrap().await.unwrap();
    let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
        project_id: project.clone(),
        display_name: "scope".into(),
        path: sub.to_string_lossy().into(),
        additional_directories: vec![],
    }));
    update.expected_stream_version = Some(
        bootstrap
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .unwrap()
            .stream_version,
    );
    f.core.execute(update).await.unwrap();
    assert!(rpc(&f.client, "file", args).await.is_err());
    assert!(
        rpc(
            &f.client,
            "workspace_path",
            json!({"taskId":task,"path":path.join("file")})
        )
        .await
        .is_err()
    );
    assert!(
        rpc(
            &f.client,
            "workspace_path",
            json!({"taskId":task,"path":sub.join("allowed")})
        )
        .await
        .is_ok()
    );
    assert!(
        f.control
            .review
            .has_snapshot(snapshot["id"].as_str().unwrap())
    );
    let bootstrap = f.core.bootstrap().await.unwrap();
    let mut delete = CommandEnvelope::local(CoreCommand::DeleteTask(TaskLifecycle {
        task_id: task.clone(),
    }));
    delete.expected_stream_version = Some(
        bootstrap
            .tasks
            .iter()
            .find(|t| t.task_id == task)
            .unwrap()
            .stream_version,
    );
    f.client.execute(delete).await.unwrap();
    assert!(
        !f.control
            .review
            .has_snapshot(snapshot["id"].as_str().unwrap())
    );
    assert!(
        rpc(
            &f.client,
            "query",
            json!({"taskId":task,"mode":"uncommitted"})
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn review_rpc_verifies_turn_artifacts_and_preserves_undo_status_across_service_instances() {
    let f = fixture().await;
    let path = f.directory.path().join("plain");
    std::fs::create_dir(&path).unwrap();
    let (_, task) = register(&f, &path).await;
    let store =
        agent_workspace::review::ReviewStore::open(f.directory.path().join("review")).unwrap();
    std::fs::write(path.join("file"), "after").unwrap();
    let version = |content: &[u8]| json!({"hex":content.iter().fold(String::new(), |mut hex, b| { use std::fmt::Write; let _ = write!(hex, "{b:02x}"); hex }),"sha256":store.put_blob(content).unwrap(),"mode":"100644"});
    let data = json!({"schemaVersion":1,"workspacePaths":[path],"attribution":"workspace_interval","files":[{"path":"file","before":version(b"before"),"after":version(b"after")}]});
    let content = serde_json::to_vec(&data).unwrap();
    let metadata = agent_protocol::ArtifactMetadata {
        artifact_id: ArtifactId::new(),
        payload_sha256: store.put_blob(&content).unwrap(),
        media_type: "application/vnd.dogcodes.turn-changes.v1+json".into(),
        byte_length: content.len() as u64,
        created_at: chrono::Utc::now(),
        redaction_ruleset_version: 0,
        redaction_count: 0,
    };
    f.repository.append(AppendRequest { request_fingerprint: None, command_id:CommandId::new(), idempotency_key:CommandId::new().to_string(), expected_stream_version:None, artifacts:vec![NewArtifact { metadata:metadata.clone(), content }], events:vec![agent_protocol::NewEvent::hub(agent_protocol::StreamRef { stream_type:agent_protocol::StreamType::Task, stream_id:task.to_string() }, "workspace.turn_changes_captured", json!({"taskId":task,"turnId":agent_domain::TurnId::new(),"runId":RunId::new(),"artifact":metadata}))] }).await.unwrap();
    let snapshot = query(&f.client, &task).await;
    assert_eq!(
        rpc(&f.client, "file", file_args(&snapshot)).await.unwrap()["text"]["before"],
        json!(["before"])
    );
    let status = json!({"taskId":task,"artifactId":metadata.artifact_id});
    assert_eq!(
        rpc(&f.client, "undo_status", status.clone()).await.unwrap(),
        false
    );
    let request = json!({"taskId":task,"artifactId":metadata.artifact_id,"expectedPayloadSha256":metadata.payload_sha256,"confirmed":true});
    let mut wrong = request.clone();
    wrong["expectedPayloadSha256"] = json!("0".repeat(64));
    assert!(rpc(&f.client, "undo", wrong).await.is_err());
    let operation = uuid::Uuid::new_v4().to_string();
    let request = json!({"operationId":operation,"request":request});
    let mut first_watch = f.client.review_watch(task.to_string()).await.unwrap();
    let other = CoreClient::Remote(f.directory.path().into());
    let mut second_watch = other.review_watch(task.to_string()).await.unwrap();
    assert!(matches!(
        first_watch.next().await.unwrap(),
        crate::workspace_service::watch::WatchMessage::Invalidated
    ));
    assert!(matches!(
        second_watch.next().await.unwrap(),
        crate::workspace_service::watch::WatchMessage::Invalidated
    ));
    // Drop the reply connection: completion must still reach other clients via the service.
    drop(
        f.client
            .connect("review", json!({"operation":"undo","arguments":request}))
            .await
            .unwrap(),
    );
    for watcher in [&mut first_watch, &mut second_watch] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(15), watcher.next())
                .await
                .unwrap()
                .unwrap(),
            crate::workspace_service::watch::WatchMessage::Invalidated
        ));
    }
    // Acceptance now invalidates as well as completion. Hints are not terminal
    // facts: re-query and wait for the next hint while the receipt is running.
    let receipt = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = rpc(
                &other,
                "file_operation",
                json!({"taskId":task,"operationId":operation}),
            )
            .await
            .unwrap();
            if receipt["outcome"]["status"] != "running" {
                break receipt;
            }
            first_watch.next().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(receipt["outcome"]["status"], "succeeded");
    assert_eq!(
        std::fs::read_to_string(path.join("file")).unwrap(),
        "before"
    );
    let restarted = CoreClient::local_at(f.core.clone(), f.directory.path());
    assert_eq!(rpc(&restarted, "undo_status", status).await.unwrap(), true);
    std::fs::write(path.join("file"), "later").unwrap();
    rpc(&restarted, "undo", request.clone()).await.unwrap();
    let mut changed = request;
    changed["request"]["expectedPayloadSha256"] = json!("0".repeat(64));
    assert!(rpc(&restarted, "undo", changed).await.is_err());
    assert_eq!(std::fs::read_to_string(path.join("file")).unwrap(), "later");
}

#[tokio::test]
async fn review_rpc_builds_project_packet_in_service_and_replays_without_gui_snapshot() {
    let f = fixture().await;
    let (_, _, task) = repo(&f).await;
    let driver = readonly_test_driver(&f.core).await;
    driver.allow_shutdown.notify_one();
    f.core.register_driver(driver.clone()).await;
    let snapshot = query(&f.client, &task).await;
    let request = agent_core::turn_review::ReviewRequest {
        task_id: task.clone(),
        turn_id: None,
        source_snapshot: Some(snapshot["id"].as_str().unwrap().into()),
        run_id: RunId::new(),
        executor: driver.executor.clone(),
        kind: agent_core::turn_review::ReviewKind::Code,
        retry_of: None,
        recheck_of: None,
    };
    let run = f
        .client
        .start_project_review(request.clone())
        .await
        .unwrap();
    assert_eq!(run.request, request);
    assert!(run.workspace_review);
    assert!(!run.input_sha256.is_empty());
    // A different service instance has no in-memory snapshot, but the original
    // accepted request and scope must replay from Core's persisted review data.
    let reopened = CoreClient::local_at(f.core.clone(), &f.directory.path().join("reopened"));
    let replay = reopened
        .start_project_review(request.clone())
        .await
        .unwrap();
    assert_eq!(replay.input_sha256, run.input_sha256);
    let remote = CoreClient::Remote(f.directory.path().into());
    assert_eq!(
        remote.review_executors().await.unwrap(),
        std::slice::from_ref(&driver.executor)
    );
    let mut disable = CommandEnvelope::local(CoreCommand::SetAgentPreferences(
        agent_protocol::AgentPreferences {
            disabled: vec!["fake".into()],
            ..Default::default()
        },
    ));
    disable.expected_stream_version = Some(0);
    remote.execute(disable).await.unwrap();
    assert!(remote.review_executors().await.unwrap().is_empty());
    assert_eq!(
        remote
            .start_project_review(request.clone())
            .await
            .unwrap()
            .request,
        request
    );
    let mut denied = request.clone();
    denied.run_id = RunId::new();
    assert!(
        remote
            .start_project_review(denied.clone())
            .await
            .unwrap_err()
            .contains("已禁用")
    );
    let mut enable = CommandEnvelope::local(CoreCommand::SetAgentPreferences(
        agent_protocol::AgentPreferences::default(),
    ));
    enable.expected_stream_version = Some(1);
    remote.execute(enable).await.unwrap();
    f.core
        .execute(test_agent_probe(agent_protocol::InstallationStatus::Broken))
        .await
        .unwrap();
    assert!(remote.review_executors().await.unwrap().is_empty());
    assert!(
        remote
            .start_project_review(denied)
            .await
            .unwrap_err()
            .contains("尚未就绪")
    );
    assert_eq!(
        remote
            .start_project_review(request.clone())
            .await
            .unwrap()
            .request,
        request
    );
    f.core
        .execute(test_agent_probe(agent_protocol::InstallationStatus::Ready))
        .await
        .unwrap();
    let mut changed = request.clone();
    changed.source_snapshot = Some("different".into());
    assert!(reopened.start_project_review(changed).await.is_err());
    let mut fresh = request.clone();
    fresh.run_id = RunId::new();
    assert!(
        reopened
            .start_project_review(fresh)
            .await
            .unwrap_err()
            .contains("比较结果已过期")
    );
    assert!(
        f.client
            .call::<Value>(
                "start_project_review",
                json!((request, json!({"snapshotId":"forged"})))
            )
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.core.has_active_reviews().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn review_rpc_project_scope_changes_wait_for_accepted_workspace_operations() {
    let f = fixture().await;
    let (path, project, _) = repo(&f).await;
    let sub = path.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let bootstrap = f.core.bootstrap().await.unwrap();
    let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
        project_id: project.clone(),
        display_name: "narrow".into(),
        path: sub.to_string_lossy().into(),
        additional_directories: vec![],
    }));
    update.expected_stream_version = Some(
        bootstrap
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .unwrap()
            .stream_version,
    );
    let guard = f.control.review.gate.lock().await;
    // Poll dispatch itself to Pending: it must stop at the gate before Core
    // accepts metadata changes. This does not rely on sleeps or network timing.
    let operation = dispatch_service(&f.core, &f.control, "execute", json!((update,)));
    tokio::pin!(operation);
    assert!(
        std::future::Future::poll(
            operation.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop())
        )
        .is_pending()
    );
    assert_eq!(
        f.core
            .bootstrap()
            .await
            .unwrap()
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .unwrap()
            .path,
        path.to_string_lossy()
    );
    drop(guard);
    operation.await.unwrap();
    assert_eq!(
        f.core
            .bootstrap()
            .await
            .unwrap()
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .unwrap()
            .path,
        sub.to_string_lossy()
    );
}

#[tokio::test]
async fn git_operation_rpc_lost_commit_reply_replays_receipt_without_a_second_commit() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let operation = uuid::Uuid::new_v4().to_string();
    let request = json!({"operationId":operation,"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],"action":"commit","message":"one commit","stageAll":true,"snapshotId":snapshot["id"]});
    let guard = f.control.review.gate.lock().await;
    drop(
        f.client
            .connect(
                "review",
                json!({"operation":"git_action","arguments":request}),
            )
            .await
            .unwrap(),
    );
    drop(guard);
    let other = CoreClient::Remote(f.directory.path().into());
    let status = json!({"taskId":task,"operationId":operation});
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = rpc(&other, "git_operation", status.clone()).await.unwrap();
            if receipt["outcome"]["status"] == "succeeded" {
                break;
            }
            assert!(
                receipt.is_null() || receipt["outcome"]["status"] == "running",
                "{receipt}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        rpc(&other, "git_action", request.clone()).await.unwrap(),
        true
    );
    let reopened = CoreClient::local_at(f.core.clone(), f.directory.path());
    assert_eq!(
        rpc(&reopened, "git_action", request.clone()).await.unwrap(),
        true
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
    let history = rpc(
        &other,
        "operations",
        json!({"taskId":task,"kind":"git","after":null,"limit":20}),
    )
    .await
    .unwrap();
    assert_eq!(history["entries"][0]["operationId"], operation);
    assert_eq!(history["entries"][0]["action"], "commit");
    assert_eq!(history["entries"][0]["status"], "succeeded");
    assert!(history["entries"][0].get("request").is_none());
    let mut changed = request;
    changed["message"] = json!("different");
    assert!(rpc(&other, "git_action", changed).await.is_err());
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[tokio::test]
async fn review_rpc_file_operation_discard_replay_preserves_later_edits_and_original_backup_id() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let mut discard = file_args(&snapshot);
    discard["confirmed"] = json!(true);
    let operation = uuid::Uuid::new_v4().to_string();
    let args = json!({"operationId":operation,"request":discard});
    let record = rpc(&f.client, "discard", args.clone()).await.unwrap();
    assert!(record.as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(
        std::fs::read_to_string(path.join("file")).unwrap(),
        "before\n"
    );
    std::fs::write(path.join("file"), "user edited after discard\n").unwrap();
    let reopened = CoreClient::local_at(f.core.clone(), f.directory.path());
    assert_eq!(
        rpc(&reopened, "discard", args.clone()).await.unwrap(),
        record
    );
    let status = rpc(
        &reopened,
        "file_operation",
        json!({"taskId":task,"operationId":operation}),
    )
    .await
    .unwrap();
    assert_eq!(status["outcome"]["result"], record);
    assert_eq!(status["request"]["arguments"], json!({"request":discard}));
    let history = rpc(
        &reopened,
        "operations",
        json!({"taskId":task,"kind":"file","after":null,"limit":20}),
    )
    .await
    .unwrap();
    assert_eq!(history["entries"][0]["operationId"], operation);
    assert_eq!(history["entries"][0]["action"], "discard");
    assert_eq!(history["entries"][0]["status"], "succeeded");
    assert!(
        rpc(
            &reopened,
            "operations",
            json!({"taskId":agent_domain::TaskId::new(),"kind":"file","after":null,"limit":20})
        )
        .await
        .is_err()
    );
    let mut changed = args;
    changed["request"]["path"] = json!("other");
    assert!(rpc(&reopened, "discard", changed).await.is_err());
    assert_eq!(
        std::fs::read_to_string(path.join("file")).unwrap(),
        "user edited after discard\n"
    );
    // The typed mutation contract rejects requests that could bypass durable identity.
    assert!(
        f.client
            .call::<Value>(
                "review",
                json!({"operation":"discard","arguments":{"request":discard}})
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn review_watch_limits_release_and_reconnect_invalidate_without_replaying_operations() {
    use crate::workspace_service::watch::WatchMessage;
    let f = fixture().await;
    let (_, _, task) = repo(&f).await;
    let changes = &f.control.review.changes;
    let mut subscriptions = Vec::new();
    for _ in 0..64 {
        subscriptions.push(
            changes
                .open(f.core.clone(), task.to_string())
                .await
                .unwrap(),
        );
    }
    assert!(
        changes
            .open(f.core.clone(), task.to_string())
            .await
            .is_err()
    );
    subscriptions.pop();
    let mut replacement = changes
        .open(f.core.clone(), task.to_string())
        .await
        .unwrap();
    assert!(matches!(
        replacement.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    // A slow client needs only the latest invalidation, never an unbounded event queue.
    for _ in 0..1000 {
        changes.changed();
    }
    assert!(matches!(
        replacement.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    drop(replacement);
    drop(subscriptions);
    let mut remote = f.client.review_watch(task.to_string()).await.unwrap();
    assert!(matches!(
        remote.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    drop(remote);
    let mut reconnected = f.client.review_watch(task.to_string()).await.unwrap();
    assert!(matches!(
        reconnected.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    assert!(
        f.client
            .review_watch(agent_domain::TaskId::new().to_string())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn review_rpc_git_workflow_survives_new_clients_and_deduplicates_continuations() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let remote = f.directory.path().join("remote.git");
    git(
        &path,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let branch = git(&path, &["branch", "--show-current"]).trim().to_owned();
    git(
        &path,
        &["config", &format!("branch.{branch}.remote"), "origin"],
    );
    git(
        &path,
        &[
            "config",
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{branch}"),
        ],
    );
    let snapshot = query(&f.client, &task).await;
    let operation = uuid::Uuid::new_v4().to_string();
    let push = uuid::Uuid::new_v4().to_string();
    let request = json!({"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],
        "operationId":operation,"action":"commit","message":"workflow commit","stageAll":true,"snapshotId":snapshot["id"],
        "workflow":{"nextOperationId":push,"parentOperationId":null}});
    assert_eq!(rpc(&f.client, "git_action", request).await.unwrap(), true);
    let other = CoreClient::Remote(f.directory.path().into());
    let parent = rpc(
        &other,
        "git_operation",
        json!({"taskId":task,"operationId":operation}),
    )
    .await
    .unwrap();
    assert_eq!(parent["request"]["workflow"]["nextOperationId"], push);
    let args = json!({"taskId":task,"operationId":operation});
    let guard = f.control.review.gate.lock().await;
    drop(
        f.client
            .connect(
                "review",
                json!({"operation":"git_advance","arguments":args.clone()}),
            )
            .await
            .unwrap(),
    );
    drop(guard); // The accepted push survives loss of its caller/reply channel.
    let (a, b) = tokio::join!(
        rpc(&f.client, "git_advance", args.clone()),
        rpc(&other, "git_advance", args.clone())
    );
    let failed = a.unwrap();
    assert_eq!(failed, b.unwrap());
    assert_eq!(failed["request"]["operationId"], push);
    assert_eq!(failed["outcome"]["status"], "failed"); // The configured destination is not available yet.
    let retry = failed["request"]["workflow"]["nextOperationId"].clone();
    assert_ne!(retry, push);
    git(
        f.directory.path(),
        &["init", "--bare", "-q", remote.to_str().unwrap()],
    );
    // A fresh service reads the links from disk. Re-clicking the old parent
    // returns the original failure, rather than silently allocating a retry.
    let reopened = CoreClient::local_at(f.core.clone(), f.directory.path());
    assert_eq!(rpc(&reopened, "git_advance", args).await.unwrap(), failed);
    let retry_args = json!({"taskId":task,"operationId":push});
    let succeeded = rpc(&reopened, "git_advance", retry_args.clone())
        .await
        .unwrap();
    assert_eq!(succeeded["request"]["operationId"], retry);
    assert_eq!(succeeded["outcome"]["status"], "succeeded");
    assert_eq!(
        rpc(&other, "git_advance", retry_args).await.unwrap(),
        succeeded
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(
        git(&remote, &["rev-parse", &branch]),
        git(&path, &["rev-parse", "HEAD"])
    );
    // Completed pushes have no automatic next step; different task IDs cannot
    // use a discovered operation to authorize another push.
    assert!(
        rpc(
            &other,
            "git_advance",
            json!({"taskId":task,"operationId":retry})
        )
        .await
        .is_err()
    );
    let (_, wrong) = register(&f, &path).await;
    assert!(
        rpc(
            &other,
            "git_advance",
            json!({"taskId":wrong,"operationId":operation})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn review_rpc_git_workflow_refuses_unknown_unconfirmed_or_unrelated_parent() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let directory = f
        .directory
        .path()
        .join("review/git-operations")
        .join(task.to_string());
    std::fs::create_dir_all(&directory).unwrap();
    for outcome in [
        json!({"status":"running"}),
        json!({"status":"unknown"}),
        json!({"status":"succeeded","completed":false}),
        json!({"status":"failed","message":"commit rejected"}),
    ] {
        let operation = uuid::Uuid::new_v4().to_string();
        let next = uuid::Uuid::new_v4().to_string();
        let request = json!({"operationId":operation,"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],
            "action":"commit","message":"interrupted","stageAll":true,"snapshotId":snapshot["id"],
            "workflow":{"nextOperationId":next,"parentOperationId":null}});
        // Model durable intent left by a stopped service. Running without a live
        // owner must become unknown, never sufficient evidence for a push.
        std::fs::write(
            directory.join(format!("{operation}.json")),
            serde_json::to_vec(&json!({"request":request,"outcome":outcome})).unwrap(),
        )
        .unwrap();
        assert!(
            rpc(
                &f.client,
                "git_advance",
                json!({"taskId":task,"operationId":operation})
            )
            .await
            .is_err()
        );
        assert!(!directory.join(format!("{next}.json")).exists());
    }
    // A normal successful commit must not be reinterpreted as commit-and-push.
    let operation = uuid::Uuid::new_v4().to_string();
    let request = json!({"operationId":operation,"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],
        "action":"commit","message":"commit only","stageAll":true,"snapshotId":snapshot["id"]});
    assert_eq!(rpc(&f.client, "git_action", request).await.unwrap(), true);
    assert!(
        rpc(
            &f.client,
            "git_advance",
            json!({"taskId":task,"operationId":operation})
        )
        .await
        .is_err()
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[tokio::test]
async fn review_rpc_git_close_is_shared_durable_and_preserves_failed_outcomes() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let operation = uuid::Uuid::new_v4().to_string();
    let push = uuid::Uuid::new_v4().to_string();
    let request = json!({"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],"operationId":operation,
        "action":"commit","message":"close workflow","stageAll":true,"snapshotId":snapshot["id"],
        "workflow":{"nextOperationId":push,"parentOperationId":null}});
    assert_eq!(rpc(&f.client, "git_action", request).await.unwrap(), true);
    let failed = rpc(
        &f.client,
        "git_advance",
        json!({"taskId":task,"operationId":operation}),
    )
    .await
    .unwrap();
    assert_eq!(failed["outcome"]["status"], "failed");
    let other = CoreClient::Remote(f.directory.path().into());
    let closed = rpc(
        &other,
        "git_close",
        json!({"taskId":task,"operationId":push}),
    )
    .await
    .unwrap();
    assert_eq!(closed["closed"], true);
    assert_eq!(closed["outcome"], failed["outcome"]);
    let reopened = CoreClient::local_at(f.core.clone(), f.directory.path());
    for id in [&operation, &push] {
        let args = json!({"taskId":task,"operationId":id});
        assert_eq!(
            rpc(&reopened, "git_operation", args.clone()).await.unwrap()["closed"],
            true
        );
        assert!(rpc(&other, "git_advance", args.clone()).await.is_err());
        assert_eq!(
            rpc(&reopened, "git_close", args).await.unwrap()["closed"],
            true
        );
    }
    assert!(
        rpc(&other, "git_action", failed["request"].clone())
            .await
            .is_err()
    );
    let history = rpc(
        &other,
        "operations",
        json!({"taskId":task,"kind":"git","limit":20}),
    )
    .await
    .unwrap();
    assert!(
        history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["closed"] == true)
    );
    let retry = failed["request"]["workflow"]["nextOperationId"]
        .as_str()
        .unwrap();
    assert!(
        rpc(
            &other,
            "git_operation",
            json!({"taskId":task,"operationId":retry})
        )
        .await
        .unwrap()
        .is_null()
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
    let (_, wrong) = register(&f, &path).await;
    assert!(
        rpc(
            &other,
            "git_close",
            json!({"taskId":wrong,"operationId":operation})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn review_rpc_git_close_arbitrates_with_a_queued_continuation() {
    let f = fixture().await;
    let (_, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let operation = uuid::Uuid::new_v4().to_string();
    let next = uuid::Uuid::new_v4().to_string();
    assert_eq!(rpc(&f.client, "git_action", json!({"taskId":task,"rootId":snapshot["roots"][0]["root"]["id"],
        "operationId":operation,"action":"commit","message":"race","stageAll":true,"snapshotId":snapshot["id"],
        "workflow":{"nextOperationId":next,"parentOperationId":null}})).await.unwrap(), true);
    let guard = f.control.review.gate.lock().await;
    let args = json!({"taskId":task,"operationId":operation});
    let close = dispatch_service(
        &f.core,
        &f.control,
        "review",
        json!({"operation":"git_close","arguments":args}),
    );
    let advance = dispatch_service(
        &f.core,
        &f.control,
        "review",
        json!({"operation":"git_advance","arguments":args}),
    );
    tokio::pin!(close, advance);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(close.as_mut(), &mut context).is_pending());
    assert!(std::future::Future::poll(advance.as_mut(), &mut context).is_pending());
    drop(guard);
    let (closed, advanced) = tokio::join!(close, advance);
    assert_eq!(closed.unwrap()["closed"], true);
    assert!(advanced.is_err());
    assert!(
        rpc(
            &f.client,
            "git_operation",
            json!({"taskId":task,"operationId":next})
        )
        .await
        .unwrap()
        .is_null()
    );
}

#[tokio::test]
async fn review_rpc_git_commit_success_is_preserved_when_push_target_cannot_be_bound() {
    let f = fixture().await;
    let (path, _, task) = repo(&f).await;
    let snapshot = query(&f.client, &task).await;
    let id = uuid::Uuid::new_v4().to_string();
    let next = uuid::Uuid::new_v4().to_string();
    assert_eq!(rpc(&f.client, "git_action", json!({"operationId":id,"taskId":task,
        "rootId":snapshot["roots"][0]["root"]["id"],"snapshotId":snapshot["id"],
        "action":"commit","message":"no upstream","stageAll":true,"workflow":{"nextOperationId":next}})).await.unwrap(), true);
    let receipt = rpc(
        &f.client,
        "git_operation",
        json!({"taskId":task,"operationId":id}),
    )
    .await
    .unwrap();
    assert_eq!(receipt["outcome"]["status"], "succeeded");
    assert_eq!(receipt["outcome"]["completed"], true);
    assert!(receipt["outcome"]["target"].is_null());
    assert!(
        receipt["outcome"]["targetError"]
            .as_str()
            .unwrap()
            .contains("上游")
    );
    let result = rpc(
        &f.client,
        "git_advance",
        json!({"taskId":task,"operationId":id}),
    )
    .await
    .unwrap();
    assert_eq!(result["outcome"]["status"], "failed");
    assert!(
        result["outcome"]["message"]
            .as_str()
            .unwrap()
            .contains("提交已完成")
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]).trim(), "2");
}

#[tokio::test]
async fn project_runs_watch_rpc_observes_catalog_and_receipt_transitions_across_clients() {
    use crate::review::mutation_operations::{MutationOutcome, MutationRequest};
    use crate::workspace_service::watch::WatchMessage;
    let f = fixture().await;
    let (_, _, task) = repo(&f).await;
    let other = CoreClient::Remote(f.directory.path().into());
    let mut watch = other.project_runs_watch(task.to_string()).await.unwrap();
    assert!(matches!(
        watch.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    let _: Value = f.client.call("project_runs", json!(task)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.control.project_runs.has_active() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        watch.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    let request = MutationRequest {
        task_id: task.to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        command: "fixture".into(),
        arguments: json!({}),
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let journal = f.control.review.mutations.clone();
    let worker = tokio::spawn({
        let request = request.clone();
        let entered = entered.clone();
        let release = release.clone();
        async move {
            journal
                .execute(request, || async {
                    entered.notify_one();
                    release.notified().await;
                    Ok(json!({"completed":["file"],"error":"partial fixture"}))
                })
                .await
                .unwrap()
        }
    });
    entered.notified().await;
    assert!(matches!(
        watch.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    assert!(matches!(
        f.control
            .review
            .mutations
            .get(task.as_str(), &request.operation_id)
            .unwrap()
            .unwrap()
            .outcome,
        MutationOutcome::Running
    ));
    release.notify_one();
    worker.await.unwrap();
    assert!(matches!(
        watch.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    assert!(matches!(
        f.control
            .review
            .mutations
            .get(task.as_str(), &request.operation_id)
            .unwrap()
            .unwrap()
            .outcome,
        MutationOutcome::Succeeded { .. }
    ));
    let closed = rpc(
        &other,
        "file_close",
        json!({"taskId":task,"operationId":request.operation_id}),
    )
    .await
    .unwrap();
    assert_eq!(closed["closed"], true);
    assert!(matches!(
        watch.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    let observed = rpc(
        &f.client,
        "file_operation",
        json!({"taskId":task,"operationId":request.operation_id}),
    )
    .await
    .unwrap();
    assert_eq!(observed["closed"], true);
    let mut reconnect = other.project_runs_watch(task.to_string()).await.unwrap();
    assert!(matches!(
        reconnect.next().await.unwrap(),
        WatchMessage::Invalidated
    ));
    drop(watch);
    drop(reconnect);
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.control.review.changes.source().active_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
