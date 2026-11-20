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
    steering: std::sync::OnceLock<Arc<SteeringProbe>>,
    sessions: AtomicUsize,
    prompts: AtomicUsize,
    received: Mutex<Vec<String>>,
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
    async fn turn_steering(&self, _: &SessionHandle) -> Option<Arc<dyn TurnSteering>> {
        self.steering
            .get()
            .map(|probe| probe.clone() as Arc<dyn TurnSteering>)
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.received.lock().unwrap().push(request.prompt.clone());
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
        if let Some(probe) = self.steering.get() {
            probe.interrupted.store(true, Ordering::Release);
        }
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
    fixture_with_store(
        conversations,
        Arc::new(SqliteStore::open_in_memory().unwrap()),
    )
    .await
}

async fn fixture_with_store(conversations: usize, store: Arc<SqliteStore>) -> Fixture {
    let core = Arc::new(Core::new(store));
    let executor = installations::probe(&core, "antigravity").await;
    let agent = Arc::new(Agent {
        executor: executor.clone(),
        steering: std::sync::OnceLock::new(),
        sessions: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        received: Mutex::default(),
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
        "backendText.agentRunsOneTaskAtATime\u{1e}[\"Antigravity\"]"
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

#[derive(Default)]
struct SteeringProbe {
    messages: Mutex<Vec<String>>,
    release: tokio::sync::Notify,
    interrupted: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl TurnSteering for SteeringProbe {
    async fn send(&self, _: &str, text: &str) -> Result<SteerDelivery, DriverError> {
        self.messages.lock().unwrap().push(text.into());
        match text {
            "waiting for native ack" => {
                self.release.notified().await;
                Ok(SteerDelivery::Accepted)
            }
            "rejected" => Ok(SteerDelivery::Rejected),
            "lost receipt" => Err(DriverError::Disconnected("lost receipt".into())),
            _ => Ok(SteerDelivery::Accepted),
        }
    }
}

#[tokio::test]
async fn steer_targets_one_run_and_receipt_replay_never_resends() {
    let Fixture { core, agent, tasks } = fixture(1).await;
    let probe = agent.steering.get_or_init(Default::default).clone();
    let task = &tasks[0];
    submit(&core, task, "slow").await;
    until(|| async {
        task_state(&core, task)
            .await
            .active_primary_run
            .is_some_and(|r| r.supports_steering && r.state == agent_domain::RunState::Running)
    })
    .await;
    let run = task_state(&core, task).await.active_primary_run.unwrap();
    let steer = |text: &str| {
        CommandEnvelope::local(CoreCommand::SteerTurn(SteerTurn {
            task_id: task.clone(),
            turn_id: run.turn_id.clone(),
            run_id: run.run_id.clone(),
            text: text.into(),
        }))
    };
    let first = steer("first");
    let (one, two) = tokio::join!(core.execute(first.clone()), core.execute(first.clone()));
    let ack = one.unwrap();
    assert_eq!(two.unwrap(), ack);
    for text in ["second", "rejected", "lost receipt"] {
        let cmd = steer(text);
        core.execute(cmd.clone()).await.unwrap();
        core.execute(cmd).await.unwrap();
    }
    until(|| async { probe.messages.lock().unwrap().len() == 4 }).await;
    agent.finish("session-0");
    idle(&core, task).await;
    submit(&core, task, "replacement slow").await;
    until(|| async {
        task_state(&core, task)
            .await
            .active_primary_run
            .is_some_and(|r| r.supports_steering)
    })
    .await;
    assert!(core.execute(steer("stale")).await.is_err());
    assert_eq!(core.execute(first).await.unwrap(), ack);
    assert_eq!(
        probe.messages.lock().unwrap().as_slice(),
        ["first", "second", "rejected", "lost receipt"]
    );
    let rows: Vec<_> = core
        .timeline(task)
        .await
        .unwrap()
        .into_iter()
        .filter(|item| item.item_id.starts_with("steer-"))
        .map(|item| (item.text, item.kind))
        .collect();
    assert_eq!(
        rows,
        [
            ("first", "user_message"),
            ("second", "user_message"),
            ("rejected", "steer_rejected"),
            ("lost receipt", "steer_unconfirmed")
        ]
        .map(|(t, k)| (t.to_string(), k.to_string()))
    );
    let session = agent.held.lock().unwrap().keys().next().unwrap().clone();
    agent.finish(&session);
    idle(&core, task).await;
}

#[tokio::test]
async fn stop_reaches_driver_while_steering_receipt_is_pending() {
    let Fixture { core, agent, tasks } = fixture(1).await;
    let task = &tasks[0];
    let probe = agent.steering.get_or_init(Default::default).clone();
    submit(&core, task, "slow").await;
    until(|| async {
        task_state(&core, task)
            .await
            .active_primary_run
            .is_some_and(|r| r.supports_steering && r.state == agent_domain::RunState::Running)
    })
    .await;
    let run = task_state(&core, task).await.active_primary_run.unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::SteerTurn(SteerTurn {
        task_id: task.clone(),
        turn_id: run.turn_id.clone(),
        run_id: run.run_id.clone(),
        text: "waiting for native ack".into(),
    })))
    .await
    .unwrap();
    until(|| async { !probe.messages.lock().unwrap().is_empty() }).await;
    let mut stop = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task.clone(),
        run_id: run.run_id,
        turn_id: run.turn_id,
    }));
    stop.expected_stream_version = Some(task_state(&core, task).await.stream_version);
    tokio::time::timeout(Duration::from_secs(3), core.execute(stop))
        .await
        .unwrap()
        .unwrap();
    until(|| async { probe.interrupted.load(Ordering::Acquire) }).await;
    probe.release.notify_one();
    agent.finish("session-0");
    idle(&core, task).await;
}

#[tokio::test]
async fn unsupported_messages_start_once_in_fifo_order_on_the_original_executor() {
    let Fixture { core, agent, tasks } = fixture(1).await;
    let task = &tasks[0];
    submit(&core, task, "slow").await;
    until(|| async {
        task_state(&core, task)
            .await
            .active_primary_run
            .is_some_and(|r| r.state == agent_domain::RunState::Running)
    })
    .await;
    let run = task_state(&core, task).await.active_primary_run.unwrap();
    let steer = |text: &str| {
        CommandEnvelope::local(CoreCommand::SteerTurn(SteerTurn {
            task_id: task.clone(),
            run_id: run.run_id.clone(),
            turn_id: run.turn_id.clone(),
            text: text.into(),
        }))
    };
    let first = steer("queued one");
    let ack = core.execute(first.clone()).await.unwrap();
    core.execute(steer("queued two")).await.unwrap();
    assert_eq!(core.execute(first.clone()).await.unwrap(), ack);
    let rows = core.timeline(task).await.unwrap();
    let queued: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "followup_queued")
        .collect();
    assert_eq!(queued.len(), 2);
    assert!(
        queued
            .iter()
            .all(|r| r.provider_id.as_deref() == Some("antigravity"))
    );
    assert_eq!(agent.prompts.load(Ordering::Acquire), 1);
    agent.finish("session-0");
    until(|| async { agent.prompts.load(Ordering::Acquire) == 3 }).await;
    idle(&core, task).await;
    assert_eq!(
        agent.received.lock().unwrap().as_slice(),
        ["slow", "queued one", "queued two"]
    );
    assert_eq!(core.execute(first).await.unwrap(), ack);
    let rows = core.timeline(task).await.unwrap();
    for q in queued {
        assert_eq!(rows.iter().filter(|r| r.text == q.text).count(), 1);
        let sent = rows.iter().find(|r| r.item_id == q.item_id).unwrap();
        assert_eq!(sent.kind, "user_message");
        assert_ne!(sent.turn_id, q.turn_id);
    }
    assert_eq!(agent.most_running.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn restart_dispatches_persisted_followup_without_replaying_the_original_turn() {
    use agent_core::{AppendRequest, CoreRepository};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.sqlite");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let Fixture { core, agent, tasks } = fixture_with_store(1, store.clone()).await;
    let task = &tasks[0];
    submit(&core, task, "slow").await;
    until(|| async {
        task_state(&core, task)
            .await
            .active_primary_run
            .is_some_and(|r| r.state == agent_domain::RunState::Running)
    })
    .await;
    let run = task_state(&core, task).await.active_primary_run.unwrap();
    let event = NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Task,
            stream_id: task.to_string(),
        },
        "turn.followup_queued",
        serde_json::json!({"taskId":task,"turnId":run.turn_id,"runId":run.run_id,"messageId":"saved","actor":{"kind":"local_user"},"executor":run.executor,"text":"after restart","settings":{}}),
    );
    store
        .append(AppendRequest {
            command_id: agent_domain::CommandId::new(),
            idempotency_key: "saved".into(),
            request_fingerprint: None,
            expected_stream_version: None,
            events: vec![event],
            artifacts: vec![],
        })
        .await
        .unwrap();
    agent.finish("session-0");
    idle(&core, task).await;
    drop(core);
    drop(store);
    let store = Arc::new(SqliteStore::open(path).unwrap());
    store.rebuild_projections().unwrap();
    let restarted = Arc::new(Core::new(store));
    restarted.recover_queued_followups().await.unwrap();
    restarted.register_driver(agent.clone()).await;
    restarted.recover_queued_followups().await.unwrap();
    until(|| async { agent.prompts.load(Ordering::Acquire) == 2 }).await;
    idle(&restarted, task).await;
    assert_eq!(
        agent.received.lock().unwrap().as_slice(),
        ["slow", "after restart"]
    );
}
