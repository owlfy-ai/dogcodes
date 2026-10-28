use super::*;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[test]
fn closed_control_channels_are_unavailable_before_process_cleanup_finishes() {
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (cancels, cancel_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle {
        native: None,
        codebuddy_cwd: None,
        cursor_force: false,
        commands,
        cancels,
        finished: Arc::new(AtomicBool::new(false)),
    };
    assert!(!handle.unavailable());
    drop(command_rx);
    assert!(handle.unavailable());
    assert!(!handle.finished.load(Ordering::Acquire));
    drop(cancel_rx);
}

#[tokio::test]
async fn cleanup_marker_preserves_exit_confirmation_without_retaining_the_old_transport() {
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    let (cancels, mut cancel_rx) = mpsc::unbounded_channel();
    let handle = ConnectionHandle {
        native: None,
        codebuddy_cwd: Some("/work".into()),
        cursor_force: true,
        commands,
        cancels,
        finished: Arc::new(AtomicBool::new(false)),
    };
    let marker = handle.cleanup_marker();
    assert!(marker.unavailable());
    assert_eq!(marker.codebuddy_cwd, handle.codebuddy_cwd);
    assert!(marker.cursor_force);
    drop(handle);
    assert!(command_rx.recv().await.is_none());
    assert!(cancel_rx.recv().await.is_none());
    assert!(!marker.finished.load(Ordering::Acquire));
    marker.finished.store(true, Ordering::Release);
    assert!(marker.finished.load(Ordering::Acquire));
}

fn load(session_id: &SessionId) -> (Command, oneshot::Receiver<Result<SessionId, DriverError>>) {
    let (reply, response) = oneshot::channel();
    (
        Command::LoadSession {
            session_id: session_id.clone(),
            cwd: "/work".into(),
            additional_directories: vec![],
            history_mcp: None,
            grok: false,
            permission_mode: None,
            model: None,
            thinking: None,
            reply,
        },
        response,
    )
}

#[tokio::test]
async fn malformed_prompt_response_invalidates_residency_and_concurrent_restore_loads_once() {
    let (client, agent) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client);
    let (agent_read, agent_write) = tokio::io::split(agent);
    let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let prompts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_loads = loads.clone();
    let observed_prompts = prompts.clone();
    let agent_task = tokio::spawn(async move {
        Agent.builder()
            .on_receive_request(async |request: InitializeRequest, responder, _connection| {
                responder.respond(InitializeResponse::new(request.protocol_version).agent_capabilities(AgentCapabilities::new().load_session(true)))
            }, on_receive_request!())
            .on_receive_request(async move |request: agent_client_protocol::UntypedMessage, responder: Responder<serde_json::Value>, _connection| {
                let response = match request.method.as_str() {
                    "session/new" => serde_json::json!({"sessionId":"s","models":{"currentModelId":"actual"}}),
                    "session/load" => {
                        observed_loads.fetch_add(1, Ordering::SeqCst);
                        serde_json::json!({"models":{"currentModelId":"actual"}})
                    }
                    "session/prompt" => {
                        if observed_prompts.fetch_add(1, Ordering::SeqCst) == 0 {
                            serde_json::json!({"stopReason":"malformed-native-value"})
                        } else { serde_json::json!({"stopReason":"end_turn"}) }
                    }
                    _ => return responder.respond(serde_json::Value::Null),
                };
                responder.respond(response)
            }, on_receive_request!())
            .connect_with(ByteStreams::new(agent_write.compat_write(), agent_read.compat()), async |_connection| {
                std::future::pending::<Result<(), AcpError>>().await
            }).await
    });
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (cancels, cancel_rx) = mpsc::unbounded_channel();
    let quota = Arc::new(AcpQuotaState::default());
    let task = tokio::spawn(run_connection(
        ByteStreams::new(client_write.compat_write(), client_read.compat()),
        command_rx,
        cancel_rx,
        ConnectionTasks {
            owner: None,
            background: lifecycle::Tasks::new(),
            updates: Arc::new(std::sync::Mutex::new(None)),
            pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
            questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            activity: Arc::new(std::sync::Mutex::new(activity::Activities::default())),
            quota: quota.clone(),
        },
    ));
    let (reply, response) = oneshot::channel();
    commands
        .send(Command::NewSession {
            cwd: "/work".into(),
            additional_directories: vec![],
            history_mcp: None,
            grok: false,
            permission_mode: None,
            model: None,
            thinking: None,
            reply,
        })
        .unwrap();
    let id = tokio::time::timeout(std::time::Duration::from_secs(3), response)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (reply, response) = oneshot::channel();
    commands
        .send(Command::Prompt {
            turn: None,
            session_id: id.clone(),
            blocks: vec![],
            image_compatibility: false,
            reply,
        })
        .unwrap();
    assert!(matches!(
        response.await.unwrap(),
        Err(DriverError::Protocol(_))
    ));
    assert_eq!(prompts.load(Ordering::SeqCst), 1); // The failed input is never automatically replayed.
    let (first, a) = load(&id);
    let (second, b) = load(&id);
    commands.send(first).unwrap();
    commands.send(second).unwrap();
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(
        quota.usage.get(Some("s")).unwrap().model.as_deref(),
        Some("actual")
    );
    let (reply, response) = oneshot::channel();
    commands
        .send(Command::Prompt {
            turn: None,
            session_id: id,
            blocks: vec![],
            image_compatibility: false,
            reply,
        })
        .unwrap();
    assert_eq!(response.await.unwrap().unwrap(), StopReason::EndTurn);
    drop(commands);
    drop(cancels);
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    agent_task.abort();
    let _ = agent_task.await;
}
