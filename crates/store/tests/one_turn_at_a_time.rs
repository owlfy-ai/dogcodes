//! Agents that run one turn at a time (Antigravity) through Core and `SQLite`:
//! a later conversation is accepted, told it is waiting, and starts by itself.
#[path = "support/installations.rs"]
mod installations;

use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::*;
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
    prompts: AtomicUsize,
    running: AtomicUsize,
    most_running: AtomicUsize,
    held: Mutex<HashMap<String, Output>>,
}

impl Agent {
    fn finish(&self, session: &str) {
        let output = self.held.lock().unwrap().remove(session).unwrap();
        self.running.fetch_sub(1, Ordering::AcqRel);
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
        "one-turn-fixture"
    }
    fn runs_one_turn_at_a_time(&self) -> bool {
        true
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        let id = self.sessions.fetch_add(1, Ordering::AcqRel);
        Ok(SessionHandle::new(format!("session-{id}")))
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.prompts.fetch_add(1, Ordering::AcqRel);
        let running = self.running.fetch_add(1, Ordering::AcqRel) + 1;
        self.most_running.fetch_max(running, Ordering::AcqRel);
        let (tx, rx) = mpsc::unbounded_channel();
        if request.prompt.ends_with("slow") {
            self.held
                .lock()
                .unwrap()
                .insert(handle.native_session_id.clone(), tx);
        } else {
            self.running.fetch_sub(1, Ordering::AcqRel);
            tx.send(Ok(DriverEvent::TurnCompleted {
                summary: "done".into(),
            }))
            .unwrap();
        }
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
    agent: Arc<Agent>,
    tasks: Vec<TaskId>,
}

async fn fixture(conversations: usize) -> Fixture {
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let executor = installations::probe(&core, "antigravity").await;
    let agent = Arc::new(Agent {
        executor: executor.clone(),
        sessions: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        running: AtomicUsize::new(0),
        most_running: AtomicUsize::new(0),
        held: Mutex::new(HashMap::new()),
    });
    core.register_driver(agent.clone()).await;
    let project = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            target_id: None,
            project_id: project.clone(),
            display_name: "one turn at a time".into(),
            path: std::env::temp_dir().to_string_lossy().into_owned(),
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
    Fixture { core, agent, tasks }
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

async fn submit(core: &Arc<Core>, task: &TaskId, message: &str) {
    let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: message.into(),
        model: None,
        thinking: None,
        permission_mode: None,
        attachments: vec![],
        executor_override: None,
        review_fix: None,
    }));
    request.expected_stream_version = Some(task_state(core, task).await.stream_version);
    core.execute(request).await.unwrap();
}

async fn until<F: std::future::Future<Output = bool>>(mut test: impl FnMut() -> F) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !test().await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn idle(core: &Core, task: &TaskId) {
    until(|| async { task_state(core, task).await.active_primary_run.is_none() }).await;
}

async fn notices(core: &Core, task: &TaskId) -> Vec<TimelineItem> {
    core.timeline(task)
        .await
        .unwrap()
        .into_iter()
        .filter(|item| item.kind == "agent.queued")
        .collect()
}

#[tokio::test]
async fn a_later_conversation_waits_with_a_notice_and_starts_by_itself() {
    let Fixture { core, agent, tasks } = fixture(2).await;
    submit(&core, &tasks[0], "slow").await;
    until(|| async { agent.prompts.load(Ordering::Acquire) == 1 }).await;
    assert!(notices(&core, &tasks[0]).await.is_empty());

    // Accepted at once, told that it waits, and not sent to the agent yet.
    submit(&core, &tasks[1], "fast").await;
    until(|| async { !notices(&core, &tasks[1]).await.is_empty() }).await;
    let notice = notices(&core, &tasks[1]).await.remove(0);
    assert_eq!(
        notice.text,
        "Antigravity同一时间只能执行一个任务，之前的任务执行完成后会开始执行此任务"
    );
    assert_eq!(notice.provider_id.as_deref(), Some("antigravity"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        task_state(&core, &tasks[1])
            .await
            .active_primary_run
            .is_some()
    );
    assert_eq!(agent.prompts.load(Ordering::Acquire), 1);

    agent.finish("session-0");
    idle(&core, &tasks[0]).await;
    idle(&core, &tasks[1]).await;
    assert_eq!(agent.prompts.load(Ordering::Acquire), 2);
    assert_eq!(agent.most_running.load(Ordering::Acquire), 1);
    let completed = core
        .timeline(&tasks[1])
        .await
        .unwrap()
        .iter()
        .any(|item| item.turn_completion.is_some());
    assert!(completed, "queued turn did not complete");

    // A turn that finds the agent free starts without a notice.
    submit(&core, &tasks[1], "fast").await;
    idle(&core, &tasks[1]).await;
    assert_eq!(notices(&core, &tasks[1]).await.len(), 1);
}

#[tokio::test]
async fn stopping_a_waiting_turn_cancels_it_without_reaching_the_agent() {
    let Fixture { core, agent, tasks } = fixture(2).await;
    submit(&core, &tasks[0], "slow").await;
    until(|| async { agent.prompts.load(Ordering::Acquire) == 1 }).await;
    submit(&core, &tasks[1], "fast").await;
    until(|| async { !notices(&core, &tasks[1]).await.is_empty() }).await;

    let state = task_state(&core, &tasks[1]).await;
    let run = state.active_primary_run.clone().unwrap();
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: tasks[1].clone(),
        turn_id: run.turn_id,
        run_id: run.run_id,
    }));
    cancel.expected_stream_version = Some(state.stream_version);
    core.execute(cancel).await.unwrap();
    idle(&core, &tasks[1]).await;
    assert_eq!(agent.prompts.load(Ordering::Acquire), 1);
    assert_eq!(agent.sessions.load(Ordering::Acquire), 1);

    // The running conversation is untouched and later turns still run.
    assert!(
        task_state(&core, &tasks[0])
            .await
            .active_primary_run
            .is_some()
    );
    agent.finish("session-0");
    idle(&core, &tasks[0]).await;
    submit(&core, &tasks[1], "fast").await;
    idle(&core, &tasks[1]).await;
    assert_eq!(agent.prompts.load(Ordering::Acquire), 2);
}
