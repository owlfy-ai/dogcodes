#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use driver_api::{
    AgentDriver, AppMcp, DriverError, DriverEvent, DriverEventStream, PromptRequest, SessionHandle,
    SessionRequest,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

fn assert_capture_response(response: &Value) {
    let content = response["result"]["content"].as_array().unwrap();
    let image = content.iter().find(|item| item["type"] == "image").unwrap();
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(
        image["data"],
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a1XcAAAAASUVORK5CYII="
    );
    assert!(content.iter().any(|item| {
        item["text"]
            .as_str()
            .is_some_and(|text| text.contains("fixture:1"))
    }));
}

struct Waiting {
    executor: ExecutorRef,
    servers: Mutex<Vec<AppMcp>>,
    sink: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>>,
}
#[async_trait::async_trait]
impl AgentDriver for Waiting {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "fixture"
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        *self.servers.lock().unwrap() = request.app_mcps;
        Ok(SessionHandle::new(TaskId::new().to_string()))
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        assert!(request.prompt.contains("computer use enabled"));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.sink.lock().unwrap() = Some(tx);
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })))
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
async fn query(server: &AppMcp, id: u64, name: &str, args: Value) -> Value {
    let mut socket = tokio::net::TcpStream::connect(&server.args[1])
        .await
        .unwrap();
    let request = json!({"token":server.args[2],"client":"fixture","request":{"id":id,"method":if name=="catalog"{"tools/list"}else{"tools/call"},"params":{"name":name,"arguments":args}}});
    socket
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(socket)
        .read_line(&mut line)
        .await
        .unwrap();
    serde_json::from_str(&line).unwrap()
}
async fn wait(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn task_opt_in_preserves_images_deduplicates_actions_and_preempts_in_flight_work() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("driver");
    let store = Arc::new(SqliteStore::open(dir.path().join("test.db")).unwrap());
    let core = Arc::new(
        Core::new(store.clone())
            .with_history_query_program(std::env::current_exe().unwrap())
            .with_computer_use(
                std::env::current_exe().unwrap(),
                env!("CARGO_BIN_EXE_computer-use-mcp-fixture").into(),
                home.clone(),
            )
            .with_computer_use_notice(),
    );
    let driver = Arc::new(Waiting {
        executor: installations::executor("codex"),
        servers: Mutex::new(vec![]),
        sink: Mutex::new(None),
    });
    installations::register(&core, store.as_ref(), driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "fixture".into(),
            path: dir.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project.clone(),
            title: "fixture".into(),
            preferred_executor: driver.executor.clone(),
        },
    )))
    .await
    .unwrap();
    assert_eq!(
        core.computer_use_control(&task, json!({"type":"status"}))
            .await
            .unwrap()["enabled"],
        false
    );
    core.computer_use_control(&task, json!({"type":"set","enabled":true}))
        .await
        .unwrap();
    let reopened = Core::new(store.clone()).with_computer_use(
        std::env::current_exe().unwrap(),
        env!("CARGO_BIN_EXE_computer-use-mcp-fixture").into(),
        home.clone(),
    );
    assert_eq!(
        reopened
            .computer_use_control(&task, json!({"type":"status"}))
            .await
            .unwrap()["enabled"],
        true
    );
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        permission_mode: None,
        thinking: None,
        model: None,
        task_id: task.clone(),
        prompt: "test".into(),
        executor_override: None,
    }));
    command.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(command).await.unwrap();
    wait(|| driver.sink.lock().unwrap().is_some()).await;
    let servers = driver.servers.lock().unwrap().clone();
    assert_eq!(servers.len(), 2);
    let computer = servers
        .iter()
        .find(|server| server.is_computer())
        .unwrap()
        .clone();
    let catalog = query(&computer, 1, "catalog", json!({})).await;
    assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 2);
    let state = query(&computer, 2, "get_window_state", json!({})).await;
    assert_capture_response(&state);
    let timeline = core.timeline(&task).await.unwrap();
    let audit = timeline
        .iter()
        .find(|item| item.kind == "computer_use")
        .expect("computer action in timeline");
    assert!(audit.turn_id.is_some());
    let details: Value = serde_json::from_str(&audit.text).unwrap();
    assert_eq!(details["tool"], "get_window_state");
    assert_eq!(details["imageCount"], 1);
    assert!(details.get("arguments").is_none());
    let first_sink = driver.sink.lock().unwrap().take().unwrap();
    let second = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: second.clone(),
            project_id: project,
            title: "Concurrent coding task".into(),
            preferred_executor: driver.executor.clone(),
        },
    )))
    .await
    .unwrap();
    core.computer_use_control(&second, json!({"type":"set","enabled":true}))
        .await
        .unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        permission_mode: None,
        thinking: None,
        model: None,
        task_id: second.clone(),
        prompt: "test".into(),
        executor_override: None,
    }));
    command.expected_stream_version = Some(
        core.bootstrap()
            .await
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.task_id == second)
            .unwrap()
            .stream_version,
    );
    core.execute(command).await.unwrap();
    wait(|| driver.sink.lock().unwrap().is_some()).await;
    let second_computer = driver
        .servers
        .lock()
        .unwrap()
        .iter()
        .find(|server| server.is_computer())
        .unwrap()
        .clone();
    assert!(query(&second_computer, 1, "catalog", json!({})).await["result"]["tools"].is_array());
    assert!(
        query(&second_computer, 2, "get_window_state", json!({})).await["error"]
            .as_str()
            .unwrap()
            .contains("desktop_busy")
    );
    driver
        .sink
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(DriverEvent::TurnCompleted {
            summary: "continued coding without desktop access".into(),
        }))
        .unwrap();
    let first = query(&computer, 3, "click", json!({})).await;
    assert_eq!(query(&computer, 3, "click", json!({})).await, first);
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "1");
    // Foreground input must not reach the driver before a matching painted notice.
    let foreground = {
        let computer = computer.clone();
        tokio::spawn(async move {
            query(
                &computer,
                90,
                "click",
                json!({"delivery_mode":"foreground"}),
            )
            .await
        })
    };
    let activity = core.subscribe_computer_use_activity();
    wait(|| activity.borrow().mode == agent_core::computer_use::ActivityMode::Foreground).await;
    assert!(activity.borrow().preparing);
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "1");
    assert_eq!(
        core.computer_use_activity(json!({"type":"presented","operationId":"stale"}))
            .await
            .unwrap()["accepted"],
        false
    );
    let operation = activity.borrow().operation_id.clone().unwrap();
    core.computer_use_activity(json!({"type":"presented","operationId":operation}))
        .await
        .unwrap();
    assert!(foreground.await.unwrap()["result"]["content"].is_array());
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "2");
    assert_eq!(
        activity.borrow().mode,
        agent_core::computer_use::ActivityMode::Idle
    );
    std::fs::remove_file(home.join("in-flight")).unwrap();
    let delayed = {
        let computer = computer.clone();
        tokio::spawn(async move { query(&computer, 4, "click", json!({"delay_ms":2500})).await })
    };
    wait(|| home.join("in-flight").exists()).await;
    let stopped = std::time::Instant::now();
    let manual = core.begin_computer_use_manual_input().await;
    assert!(stopped.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        activity.borrow().mode,
        agent_core::computer_use::ActivityMode::Idle
    );
    assert!(
        delayed.await.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    assert!(query(&computer, 5, "click", json!({})).await["error"].is_string());
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "2");
    assert_eq!(
        core.computer_use_control(&task, json!({"type":"status"}))
            .await
            .unwrap()["externalInput"],
        true
    );
    drop(manual);
    // A new observation resumes only when explicitly requested; the canceled
    // click was not replayed when the manual-input permit was released.
    let resumed = query(&computer, 6, "get_window_state", json!({})).await;
    assert_capture_response(&resumed);
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "2");
    core.computer_use_control(&task, json!({"type":"set","enabled":false}))
        .await
        .unwrap();
    first_sink
        .send(Ok(DriverEvent::TurnCompleted {
            summary: "done".into(),
        }))
        .unwrap();
    drop(first_sink);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .any(|task| task.active_primary_run.is_some())
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(home.join("effects")).unwrap(), "2");
}
