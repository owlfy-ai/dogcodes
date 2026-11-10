//! Conversations running at the same time in one folder (every conversation
//! without a project shares the scratch folder) cannot tell whose writes a
//! folder comparison saw. Their captures say so and are not offered for undo.
#[path = "support/installations.rs"]
mod installations;

use agent_core::{Core, CoreRepository};
use agent_domain::{ArtifactId, ExecutorRef, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::*;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

type Output = mpsc::UnboundedSender<Result<DriverEvent, DriverError>>;

struct Agent {
    executor: ExecutorRef,
    sessions: AtomicUsize,
    /// Open turns, by the order they were prompted.
    held: Mutex<HashMap<usize, Output>>,
    prompts: AtomicUsize,
}

impl Agent {
    fn finish(&self, prompt: usize) {
        let output = self.held.lock().unwrap().remove(&prompt).unwrap();
        output
            .send(Ok(DriverEvent::TurnCompleted {
                summary: "done".into(),
            }))
            .unwrap();
    }
}

#[async_trait]
impl AgentDriver for Agent {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "shared-folder-fixture"
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        let id = self.sessions.fetch_add(1, Ordering::AcqRel);
        Ok(SessionHandle::new(format!("session-{id}")))
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let prompt = self.prompts.fetch_add(1, Ordering::AcqRel);
        self.held.lock().unwrap().insert(prompt, tx);
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })))
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}

struct Fixture {
    core: Arc<Core>,
    store: Arc<SqliteStore>,
    agent: Arc<Agent>,
    tasks: Vec<TaskId>,
    folder: tempfile::TempDir,
}

async fn fixture(conversations: usize) -> Fixture {
    fixture_with(conversations, false).await
}

/// `drafts`: the folder is the scratch folder of conversations without a
/// project, so each new conversation gets its own folder inside it.
async fn fixture_with(conversations: usize, drafts: bool) -> Fixture {
    let folder = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut core = Core::with_workspace_context(
        store.clone(),
        Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
    );
    if drafts {
        core = core.with_conversation_drafts(folder.path().to_path_buf());
    }
    let core = Arc::new(core);
    let executor = installations::probe(&core, "codex").await;
    let agent = Arc::new(Agent {
        executor: executor.clone(),
        sessions: AtomicUsize::new(0),
        held: Mutex::new(HashMap::new()),
        prompts: AtomicUsize::new(0),
    });
    core.register_driver(agent.clone()).await;
    let project = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            target_id: None,
            project_id: project.clone(),
            display_name: "scratch".into(),
            path: folder.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..conversations {
        let task = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project.clone(),
                title: "conversation".into(),
                preferred_executor: executor.clone(),
            },
        )))
        .await
        .unwrap();
        tasks.push(task);
    }
    Fixture {
        core,
        store,
        agent,
        tasks,
        folder,
    }
}

async fn task_state(core: &Core, task: &TaskId) -> TaskSummary {
    core.bootstrap()
        .await
        .unwrap()
        .tasks
        .into_iter()
        .find(|t| &t.task_id == task)
        .unwrap()
}

async fn until<F: std::future::Future<Output = bool>>(mut test: impl FnMut() -> F) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !test().await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

impl Fixture {
    async fn start(&self, task: usize) {
        let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            task_id: self.tasks[task].clone(),
            prompt: "look at the app".into(),
            model: None,
            thinking: None,
            permission_mode: None,
            attachments: vec![],
            executor_override: None,
            review_fix: None,
        }));
        request.expected_stream_version = Some(
            task_state(&self.core, &self.tasks[task])
                .await
                .stream_version,
        );
        self.core.execute(request).await.unwrap();
    }

    async fn wait_prompted(&self, count: usize) {
        until(|| async { self.agent.prompts.load(Ordering::Acquire) == count }).await;
    }

    /// Ends the conversation's turn and returns its captured changes.
    async fn finish(&self, task: usize, prompt: usize) -> Value {
        self.agent.finish(prompt);
        let task = &self.tasks[task];
        until(|| async {
            task_state(&self.core, task)
                .await
                .active_primary_run
                .is_none()
        })
        .await;
        let events = self.core.sync_events(0, 10_000).await.unwrap();
        let captured = events
            .iter()
            .rev()
            .find(|e| {
                e.event_type == "workspace.turn_changes_captured"
                    && e.stream.stream_id == task.as_str()
            })
            .expect("turn changes");
        let id = ArtifactId::parse(captured.payload["artifact"]["artifactId"].as_str().unwrap())
            .unwrap();
        let artifact = self.store.artifact(&id).await.unwrap().unwrap();
        serde_json::from_slice(&artifact.content).unwrap()
    }
}

fn paths(change: &Value) -> Vec<&str> {
    change["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn turns_overlapping_in_one_folder_mark_their_changes_as_shared() {
    let f = fixture(2).await;
    f.start(0).await;
    f.wait_prompted(1).await;
    f.start(1).await;
    f.wait_prompted(2).await;
    // Written by one of them; the folder cannot tell which.
    std::fs::write(f.folder.path().join("jianying_home.png"), b"\x89PNG").unwrap();

    let second = f.finish(1, 1).await;
    let first = f.finish(0, 0).await;
    for change in [&first, &second] {
        assert_eq!(paths(change), ["jianying_home.png"]);
        assert_eq!(change["attribution"], "shared_workspace_interval");
        assert_eq!(change["sharedWorkspace"], true);
    }

    // Alone in the folder again: the capture belongs to this turn.
    f.start(0).await;
    f.wait_prompted(3).await;
    std::fs::write(f.folder.path().join("notes.txt"), b"mine").unwrap();
    let alone = f.finish(0, 2).await;
    assert_eq!(paths(&alone), ["notes.txt"]);
    assert_eq!(alone["attribution"], "workspace_interval");
    assert!(alone.get("sharedWorkspace").is_none());
}

async fn recovered_changes(core: &Core, store: &SqliteStore, task: &TaskId) -> Value {
    let events = core.sync_events(0, 10_000).await.unwrap();
    let captured = events
        .iter()
        .find(|e| {
            e.event_type == "workspace.turn_changes_captured" && e.stream.stream_id == task.as_str()
        })
        .expect("recovered turn changes");
    let id =
        ArtifactId::parse(captured.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
    serde_json::from_slice(&store.artifact(&id).await.unwrap().unwrap().content).unwrap()
}

#[tokio::test]
async fn turns_interrupted_together_in_one_folder_recover_as_shared() {
    let f = fixture(2).await;
    f.start(0).await;
    f.wait_prompted(1).await;
    f.start(1).await;
    f.wait_prompted(2).await;
    std::fs::write(f.folder.path().join("jianying_editor.png"), b"\x89PNG").unwrap();

    // The host stops with both turns open; the next start recovers them.
    let restarted = Core::with_workspace_context(
        f.store.clone(),
        Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
    );
    restarted.recover_interrupted_turns().await.unwrap();
    for task in &f.tasks {
        let change = recovered_changes(&restarted, &f.store, task).await;
        assert_eq!(paths(&change), ["jianying_editor.png"]);
        assert_eq!(change["recoveredAfterInterruption"], true);
        assert_eq!(change["sharedWorkspace"], true);
    }
}

#[tokio::test]
async fn a_turn_interrupted_alone_in_its_folder_recovers_as_its_own() {
    let f = fixture(2).await;
    f.start(0).await;
    f.wait_prompted(1).await;
    std::fs::write(f.folder.path().join("mine.txt"), b"mine").unwrap();
    let restarted = Core::with_workspace_context(
        f.store.clone(),
        Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
    );
    restarted.recover_interrupted_turns().await.unwrap();
    let change = recovered_changes(&restarted, &f.store, &f.tasks[0]).await;
    assert_eq!(paths(&change), ["mine.txt"]);
    assert_eq!(change["recoveredAfterInterruption"], true);
    assert!(change.get("sharedWorkspace").is_none());
}

#[tokio::test]
async fn conversations_without_a_project_each_work_in_their_own_folder() {
    let f = fixture_with(2, true).await;
    let mut folders = Vec::new();
    for task in &f.tasks {
        let summary = task_state(&f.core, task).await;
        let folder = f
            .folder
            .path()
            .join("drafts")
            .join(task.as_str())
            .to_string_lossy()
            .into_owned();
        assert_eq!(summary.working_directory.as_deref(), Some(folder.as_str()));
        assert!(std::path::Path::new(&folder).is_dir());
        let state = f.store.task_execution_state(task).await.unwrap().unwrap();
        assert_eq!(state.workspace.path, folder);
        folders.push(std::path::PathBuf::from(folder));
    }

    // Running at the same time, each turn is credited only with its own file.
    f.start(0).await;
    f.wait_prompted(1).await;
    f.start(1).await;
    f.wait_prompted(2).await;
    std::fs::write(folders[0].join("first.txt"), b"first").unwrap();
    std::fs::write(folders[1].join("second.png"), b"\x89PNG").unwrap();
    let second = f.finish(1, 1).await;
    let first = f.finish(0, 0).await;
    assert_eq!(paths(&first), ["first.txt"]);
    assert_eq!(paths(&second), ["second.png"]);
    for change in [&first, &second] {
        assert_eq!(change["attribution"], "workspace_interval");
    }

    // A folder removed meanwhile is recreated for the next turn.
    std::fs::remove_dir_all(&folders[0]).unwrap();
    f.start(0).await;
    f.wait_prompted(3).await;
    assert!(folders[0].is_dir());
    assert_eq!(paths(&f.finish(0, 2).await), Vec::<&str>::new());
}

#[tokio::test]
async fn a_conversation_in_the_scratch_folder_itself_does_not_see_their_folders() {
    // Created before conversations had their own folders.
    let f = fixture(1).await;
    assert!(
        task_state(&f.core, &f.tasks[0])
            .await
            .working_directory
            .is_none()
    );
    let drafts = Arc::new(
        Core::with_workspace_context(
            f.store.clone(),
            Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
        )
        .with_conversation_drafts(f.folder.path().to_path_buf()),
    );
    let project = task_state(&f.core, &f.tasks[0]).await.project_id;
    let newer = TaskId::new();
    drafts
        .execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: newer.clone(),
                project_id: project,
                title: "newer".into(),
                preferred_executor: f.agent.executor.clone(),
            },
        )))
        .await
        .unwrap();
    let newer_folder = f.folder.path().join("drafts").join(newer.as_str());
    assert!(newer_folder.is_dir());
    // The older conversation keeps the scratch folder, and its turns leave
    // the newer conversation's folder out.
    assert!(
        task_state(&f.core, &f.tasks[0])
            .await
            .working_directory
            .is_none()
    );
    f.start(0).await;
    f.wait_prompted(1).await;
    std::fs::write(newer_folder.join("theirs.txt"), b"theirs").unwrap();
    std::fs::write(f.folder.path().join("mine.txt"), b"mine").unwrap();
    let change = f.finish(0, 0).await;
    assert_eq!(paths(&change), ["mine.txt"]);
}
