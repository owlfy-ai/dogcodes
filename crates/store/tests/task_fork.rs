#[path = "support/installations.rs"]
mod installations;
use agent_core::{
    Core, CoreRepository,
    task_fork::{ForkEnvironment, ForkRequest},
};
use agent_domain::{ProjectId, TaskId, TurnId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use agent_workspace::GitWorkspaceContextProvider;
use driver_api::FakeDriver;
use serde_json::Value;
use std::{path::Path, process::Command, sync::Arc, time::Duration};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
async fn setup(path: &Path) -> (Arc<Core>, Arc<SqliteStore>, Arc<FakeDriver>, TaskId) {
    setup_repository(path, Arc::new(SqliteStore::open_in_memory().unwrap())).await
}
async fn setup_repository(
    path: &Path,
    store: Arc<SqliteStore>,
) -> (Arc<Core>, Arc<SqliteStore>, Arc<FakeDriver>, TaskId) {
    let core = Arc::new(Core::with_workspace_context(
        store.clone(),
        Arc::new(GitWorkspaceContextProvider::default()),
    ));
    let driver = Arc::new(FakeDriver::with_executor(installations::executor("fake")));
    installations::register(&core, store.as_ref(), driver.clone()).await;
    let project_id = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project_id.clone(),
            display_name: "Fork test".into(),
            path: path.to_str().unwrap().into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id,
            title: "Source".into(),
            preferred_executor: installations::executor("fake"),
        },
    )))
    .await
    .unwrap();
    (core, store, driver, task)
}
async fn turn(core: &Arc<Core>, store: &SqliteStore, task: &TaskId, prompt: &str) -> TurnId {
    let task_state = core
        .bootstrap()
        .await
        .unwrap()
        .tasks
        .into_iter()
        .find(|t| &t.task_id == task)
        .unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: prompt.into(),
        executor_override: None,
        model: None,
        thinking: None,
        permission_mode: None,
        review_fix: None,
        attachments: vec![],
    }));
    command.expected_stream_version = Some(task_state.stream_version);
    core.execute(command).await.unwrap();
    // Polls for the end of the turn; slow shared runners need a generous ceiling.
    tokio::time::timeout(Duration::from_mins(1), async {
        loop {
            let snapshot = core.bootstrap().await.unwrap();
            if snapshot.active_runs.is_empty() {
                let events = store
                    .task_events(task, 0, snapshot.last_sequence, 1000)
                    .await
                    .unwrap();
                let terminal = events
                    .iter()
                    .rev()
                    .find(|e| matches!(e.event_type.as_str(), "turn.completed" | "turn.failed"))
                    .unwrap();
                assert_eq!(
                    terminal.event_type, "turn.completed",
                    "{}",
                    terminal.payload
                );
                break TurnId::parse(terminal.payload["turnId"].as_str().unwrap()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn request(task: &TaskId, turn: &TurnId, environment: ForkEnvironment) -> ForkRequest {
    ForkRequest {
        task_id: task.clone(),
        turn_id: turn.clone(),
        new_task_id: TaskId::new(),
        environment,
    }
}
async fn inherited(core: &Core, task: &TaskId) -> Vec<Value> {
    let item = core
        .timeline(task)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == "fork_context")
        .unwrap();
    let data: Value = serde_json::from_str(&item.text).unwrap();
    serde_json::from_str(data["text"].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn fork_preserves_only_selected_history_and_delivers_it_to_a_fresh_session() {
    let directory = tempfile::tempdir().unwrap();
    let (core, store, driver, task) = setup(directory.path()).await;
    let first = turn(&core, &store, &task, "first-only-history").await;
    turn(&core, &store, &task, "later-excluded-history").await;
    let before = core.timeline(&task).await.unwrap();
    let request = request(&task, &first, ForkEnvironment::Workspace);
    let fork = core.fork_task(request.clone()).await.unwrap();
    assert_eq!(core.fork_task(request.clone()).await.unwrap(), fork);
    let receipt = core.fork_receipt(&task, &fork).await.unwrap().unwrap();
    assert_eq!(receipt.request, request);
    assert!(receipt.task_available);
    assert!(
        core.fork_receipt(&task, &TaskId::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(core.fork_receipt(&fork, &fork).await.is_err());
    let mut mismatched = request;
    mismatched.environment = ForkEnvironment::Worktree;
    assert!(core.fork_task(mismatched).await.is_err());
    assert_eq!(core.bootstrap().await.unwrap().tasks.len(), 2);
    assert_eq!(core.timeline(&task).await.unwrap(), before);
    let history = inherited(&core, &fork).await;
    let text = serde_json::to_string(&history).unwrap();
    assert!(text.contains("first-only-history"));
    assert!(!text.contains("later-excluded-history"));
    assert_eq!(
        history
            .iter()
            .filter(|m| m["kind"] == "message.delta")
            .count(),
        1
    );
    assert!(
        core.bootstrap()
            .await
            .unwrap()
            .session_maps
            .iter()
            .all(|m| m.task_id != fork)
    );
    let second = turn(&core, &store, &fork, "new-fork-request").await;
    let requests = driver.received_requests().await;
    let received = requests.last().unwrap();
    assert_eq!(received.context.len(), 1);
    let context: Value = serde_json::from_slice(&received.context[0].content).unwrap();
    let context = context.to_string();
    assert!(context.contains("first-only-history"), "{context}");
    assert!(!context.contains("later-excluded-history"));
    assert_eq!(
        received.working_directory,
        directory.path().to_str().unwrap()
    );
    let nested = core
        .fork_task(request_for_nested(&fork, &second))
        .await
        .unwrap();
    let inherited = inherited(&core, &nested).await;
    assert_eq!(
        inherited
            .iter()
            .filter(|m| m["kind"] == "turn.queued")
            .count(),
        2
    );
    assert!(!inherited.iter().any(|m| m["kind"] == "task.forked"));
}
fn request_for_nested(task: &TaskId, turn: &TurnId) -> ForkRequest {
    request(task, turn, ForkEnvironment::Workspace)
}

#[tokio::test]
async fn fork_worktree_restores_selected_turn_and_preserves_source_files_and_branch() {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(directory.path().join("tracked.txt"), "committed\n").unwrap();
    git(directory.path(), &["add", "."]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    std::fs::write(directory.path().join("tracked.txt"), "local change\n").unwrap();
    std::fs::write(directory.path().join("untracked.txt"), "untracked\n").unwrap();
    std::fs::write(directory.path().join("large.bin"), vec![255; 256_000]).unwrap();
    let original_head = git(directory.path(), &["rev-parse", "HEAD"]);
    let index_before = std::fs::read(directory.path().join(".git/index")).unwrap();
    let (core, store, driver, task) = setup(directory.path()).await;
    let first = turn(&core, &store, &task, "worktree history").await;
    // Later turns and a later HEAD must never leak into an earlier fork.
    assert_eq!(
        std::fs::read(directory.path().join(".git/index")).unwrap(),
        index_before
    );
    std::fs::write(directory.path().join("tracked.txt"), "later change\n").unwrap();
    std::fs::remove_file(directory.path().join("large.bin")).unwrap();
    std::fs::write(directory.path().join("untracked.txt"), "later untracked\n").unwrap();
    git(directory.path(), &["add", "tracked.txt"]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "later",
        ],
    );
    turn(&core, &store, &task, "later turn").await;
    let before = git(directory.path(), &["status", "--porcelain"]);
    // Private refs must survive normal garbage collection and temporary-index cleanup.
    git(directory.path(), &["gc", "--prune=now"]);
    let request = request(&task, &first, ForkEnvironment::Worktree);
    let fork = core.fork_task(request.clone()).await.unwrap();
    assert_eq!(core.fork_task(request).await.unwrap(), fork);
    let snapshot = core.bootstrap().await.unwrap();
    let new_task = snapshot.tasks.iter().find(|t| t.task_id == fork).unwrap();
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id == new_task.project_id)
        .unwrap();
    let target = Path::new(&project.path);
    assert_ne!(target, directory.path());
    assert_eq!(
        std::fs::read_to_string(target.join("tracked.txt")).unwrap(),
        "local change\n"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("untracked.txt")).unwrap(),
        "untracked\n"
    );
    assert_eq!(
        std::fs::read(target.join("large.bin")).unwrap(),
        vec![255; 256_000]
    );
    assert_eq!(git(target, &["rev-parse", "HEAD"]), original_head);
    let changes = git(target, &["status", "--porcelain"]);
    assert!(changes.contains("M tracked.txt"), "{changes}");
    assert!(changes.contains("?? untracked.txt"), "{changes}");
    assert_eq!(
        git(target, &["branch", "--show-current"]),
        format!("codex/fork-{fork}")
    );
    assert_eq!(git(directory.path(), &["branch", "--show-current"]), "main");
    assert_eq!(git(directory.path(), &["status", "--porcelain"]), before);
    assert_eq!(snapshot.projects.len(), 2);
    turn(&core, &store, &fork, "continue in worktree").await;
    assert_eq!(
        driver
            .received_requests()
            .await
            .last()
            .unwrap()
            .working_directory,
        project.path
    );
}

#[tokio::test]
async fn fork_rejects_unknown_turns_and_non_git_worktrees_without_creating_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let (core, store, _, task) = setup(directory.path()).await;
    assert!(
        core.fork_task(request(&task, &TurnId::new(), ForkEnvironment::Workspace))
            .await
            .is_err()
    );
    let first = turn(&core, &store, &task, "source").await;
    assert!(
        core.fork_task(request(&task, &first, ForkEnvironment::Worktree))
            .await
            .is_err()
    );
    let snapshot = core.bootstrap().await.unwrap();
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.projects.len(), 1);
}

#[tokio::test]
async fn historical_turn_without_snapshot_rejects_worktree_but_keeps_workspace_fork_available() {
    use agent_core::AppendRequest;
    use agent_domain::CommandId;
    use agent_protocol::{NewEvent, StreamRef, StreamType};
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    let (core, store, _, task) = setup(directory.path()).await;
    // Imported/legacy terminal events predate the durable snapshot feature.
    let old_turn = TurnId::new();
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: "legacy-turn".into(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: task.to_string(),
                },
                "turn.interrupted",
                json!({"taskId":task,"turnId":old_turn}),
            )],
        })
        .await
        .unwrap();
    let error = core
        .fork_task(request(&task, &old_turn, ForkEnvironment::Worktree))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("没有可恢复的文件快照"),
        "{error}"
    );
    assert_eq!(core.bootstrap().await.unwrap().projects.len(), 1);
    assert_eq!(core.bootstrap().await.unwrap().tasks.len(), 1);
    core.fork_task(request(&task, &old_turn, ForkEnvironment::Workspace))
        .await
        .unwrap();
}

#[tokio::test]
async fn fork_receipt_survives_database_reopen_without_recreating_the_session() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("fork.sqlite");
    let (core, store, driver, task) = setup_repository(
        directory.path(),
        Arc::new(SqliteStore::open(&database).unwrap()),
    )
    .await;
    let first = turn(&core, &store, &task, "saved context").await;
    let request = request(&task, &first, ForkEnvironment::Workspace);
    core.fork_task(request.clone()).await.unwrap();
    drop(core);
    drop(store);
    drop(driver);
    let repository = Arc::new(SqliteStore::open(&database).unwrap());
    let restored = Core::new(repository.clone());
    let head = repository.event_head().await.unwrap();
    let receipt = restored
        .fork_receipt(&task, &request.new_task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.request, request);
    assert!(receipt.task_available);
    assert_eq!(
        restored.fork_task(request.clone()).await.unwrap(),
        request.new_task_id
    );
    assert_eq!(repository.event_head().await.unwrap(), head);
}

#[tokio::test]
async fn incomplete_fork_retains_original_request_and_rejects_changed_retry_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (core, store, _, task) = setup(directory.path()).await;
    let first = turn(&core, &store, &task, "source").await;
    let original = request(&task, &first, ForkEnvironment::Worktree);
    assert!(core.fork_task(original.clone()).await.is_err());
    let receipt = core
        .fork_receipt(&task, &original.new_task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.request, original);
    assert!(!receipt.completed);
    assert!(!receipt.task_available);
    let restored = Core::new(store);
    let mut changed = original.clone();
    changed.environment = ForkEnvironment::Workspace;
    assert!(
        restored
            .fork_task(changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同的原请求")
    );
    assert_eq!(
        restored
            .fork_receipt(&task, &original.new_task_id)
            .await
            .unwrap()
            .unwrap()
            .request,
        original
    );
    assert_eq!(restored.bootstrap().await.unwrap().tasks.len(), 1);
}

struct FailFirstCheckout {
    fail: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl agent_core::WorkspaceContextProvider for FailFirstCheckout {
    async fn collect(&self, _: &str) -> agent_core::WorkspaceContextSnapshot {
        agent_core::WorkspaceContextSnapshot::default()
    }
    async fn fork_worktrees(&self, snapshot: Value, name: &str) -> Result<Vec<String>, String> {
        if self.fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err("injected checkout interruption".into());
        }
        agent_core::WorkspaceContextProvider::fork_worktrees(
            &GitWorkspaceContextProvider::default(),
            snapshot,
            name,
        )
        .await
    }
}
#[tokio::test]
async fn failed_fork_resumes_frozen_plan_after_source_changes_and_host_restart() {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(directory.path().join("file"), "base\n").unwrap();
    git(directory.path(), &["add", "."]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "base",
        ],
    );
    let (source, store, _, task) = setup(directory.path()).await;
    let ended = turn(&source, &store, &task, "frozen history").await;
    let original = request(&task, &ended, ForkEnvironment::Worktree);
    let provider = Arc::new(FailFirstCheckout { fail: true.into() });
    let core = Arc::new(Core::with_workspace_context(
        store.clone(),
        provider.clone(),
    ));
    assert!(
        core.fork_task(original.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("injected")
    );
    let receipt = core
        .fork_receipt(&task, &original.new_task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "failed");
    assert!(receipt.error.unwrap().contains("injected"));
    let snapshot = core.bootstrap().await.unwrap();
    let state = snapshot
        .tasks
        .iter()
        .find(|value| value.task_id == task)
        .unwrap();
    let mut rename = CommandEnvelope::local(CoreCommand::RenameTask(agent_protocol::RenameTask {
        task_id: task.clone(),
        title: "Changed source".into(),
    }));
    rename.expected_stream_version = Some(state.stream_version);
    core.execute(rename).await.unwrap();
    let project = snapshot.projects.first().unwrap();
    let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
        project_id: project.project_id.clone(),
        display_name: "Changed project".into(),
        path: project.path.clone(),
        additional_directories: vec![],
    }));
    update.expected_stream_version = Some(project.stream_version);
    core.execute(update).await.unwrap();
    drop(core);
    let restored = Core::with_workspace_context(store, provider);
    let target = restored.fork_task(original.clone()).await.unwrap();
    let snapshot = restored.bootstrap().await.unwrap();
    let created = snapshot
        .tasks
        .iter()
        .find(|value| value.task_id == target)
        .unwrap();
    assert_eq!(created.title, "frozen history · Fork");
    let project = snapshot
        .projects
        .iter()
        .find(|value| value.project_id == created.project_id)
        .unwrap();
    assert_eq!(project.display_name, "Fork test · Fork");
    assert!(
        serde_json::to_string(&inherited(&restored, &target).await)
            .unwrap()
            .contains("frozen history")
    );
    assert_eq!(
        restored
            .fork_receipt(&task, &target)
            .await
            .unwrap()
            .unwrap()
            .status,
        "succeeded"
    );
}

struct BlockedCheckout {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl agent_core::WorkspaceContextProvider for BlockedCheckout {
    async fn collect(&self, _: &str) -> agent_core::WorkspaceContextSnapshot {
        agent_core::WorkspaceContextSnapshot::default()
    }
    async fn fork_worktrees(&self, snapshot: Value, name: &str) -> Result<Vec<String>, String> {
        self.entered.notify_one();
        self.release.notified().await;
        agent_core::WorkspaceContextProvider::fork_worktrees(
            &GitWorkspaceContextProvider::default(),
            snapshot,
            name,
        )
        .await
    }
}
#[tokio::test]
async fn active_fork_prevents_source_deletion_and_directory_rebinding_until_finished() {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(directory.path().join("file"), "base\n").unwrap();
    git(directory.path(), &["add", "."]);
    git(
        directory.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "base",
        ],
    );
    let (source, store, _, task) = setup(directory.path()).await;
    let ended = turn(&source, &store, &task, "fork boundary").await;
    let request = request(&task, &ended, ForkEnvironment::Worktree);
    let provider = Arc::new(BlockedCheckout {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let core = Arc::new(Core::with_workspace_context(store, provider.clone()));
    let worker = tokio::spawn({
        let core = core.clone();
        async move { core.fork_task(request).await.unwrap() }
    });
    tokio::time::timeout(Duration::from_secs(3), provider.entered.notified())
        .await
        .unwrap();
    let snapshot = core.bootstrap().await.unwrap();
    let state = snapshot
        .tasks
        .iter()
        .find(|value| value.task_id == task)
        .unwrap();
    let mut delete =
        CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
            task_id: task.clone(),
        }));
    delete.expected_stream_version = Some(state.stream_version);
    assert!(
        core.execute(delete)
            .await
            .unwrap_err()
            .to_string()
            .contains("Fork")
    );
    let project = snapshot.projects.first().unwrap();
    let extra = directory.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
        project_id: project.project_id.clone(),
        display_name: project.display_name.clone(),
        path: project.path.clone(),
        additional_directories: vec![extra.to_string_lossy().into_owned()],
    }));
    update.expected_stream_version = Some(project.stream_version);
    assert!(
        core.execute(update)
            .await
            .unwrap_err()
            .to_string()
            .contains("Fork")
    );
    provider.release.notify_one();
    // Finalization includes real Git and SQLite I/O. Keep this a bounded
    // correctness check without imposing a five-second disk-speed contract.
    let target = tokio::time::timeout(Duration::from_secs(30), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(
        core.fork_receipt(&task, &target)
            .await
            .unwrap()
            .unwrap()
            .completed
    );
}
