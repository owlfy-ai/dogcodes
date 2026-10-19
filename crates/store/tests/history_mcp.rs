#[path = "support/installations.rs"]
mod installations;
use agent_core::{AppendRequest, Core, CoreRepository};
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use driver_api::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
struct WaitingDriver {
    executor: ExecutorRef,
    mcp: Mutex<Option<HistoryMcp>>,
    replies: Mutex<Vec<(String, String)>>,
    sink: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>>,
}
#[async_trait::async_trait]
impl AgentDriver for WaitingDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        *self.mcp.lock().unwrap() = request.history_mcp;
        Ok(SessionHandle {
            native_session_id: "fixture".into(),
        })
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        assert_eq!(request.prompt, "literal user question");
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.sink.lock().unwrap() = Some(tx);
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })))
    }
    async fn respond_to_interaction(
        &self,
        _: &SessionHandle,
        request: &str,
        option: &str,
    ) -> Result<(), DriverError> {
        self.replies
            .lock()
            .unwrap()
            .push((request.into(), option.into()));
        Ok(())
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        self.sink
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(Ok(DriverEvent::TurnInterrupted {
                summary: "cancelled".into(),
            }))
            .unwrap();
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
async fn request(mcp: &HistoryMcp, query: Value) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(&mcp.args[1]).await.unwrap();
    stream
        .write_all(format!("{}\n", json!({"token":mcp.args[2],"query":query})).as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(stream)
        .read_line(&mut line)
        .await
        .unwrap();
    serde_json::from_str(&line).unwrap()
}
async fn query(mcp: &HistoryMcp) -> Value {
    request(mcp, json!({"operation":"getTurns"})).await
}
async fn append(store: &SqliteStore, events: Vec<NewEvent>) -> Vec<EventEnvelope> {
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: agent_domain::CommandId::new(),
            idempotency_key: agent_domain::CommandId::new().to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events,
        })
        .await
        .unwrap()
        .events
}
fn event(task: &TaskId, kind: &str, payload: Value) -> NewEvent {
    NewEvent::hub(
        StreamRef {
            stream_type: StreamType::Task,
            stream_id: task.to_string(),
        },
        kind,
        payload,
    )
}
async fn artifact(store: &SqliteStore, task: &TaskId, text: &str) -> agent_domain::ArtifactId {
    use sha2::{Digest, Sha256};
    let id = agent_domain::ArtifactId::new();
    let metadata = ArtifactMetadata {
        artifact_id: id.clone(),
        media_type: "application/vnd.dogcodes.test-report.v1+json".into(),
        payload_sha256: hex::encode(Sha256::digest(text.as_bytes())),
        byte_length: text.len() as u64,
        created_at: chrono::Utc::now(),
        redaction_ruleset_version: 2,
        redaction_count: 0,
    };
    store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: agent_domain::CommandId::new(),
            idempotency_key: id.to_string(),
            expected_stream_version: None,
            artifacts: vec![agent_core::NewArtifact {
                metadata: metadata.clone(),
                content: text.as_bytes().to_vec(),
            }],
            events: vec![event(
                task,
                "test.completed",
                json!({"taskId":task,"command":"fixture","outcome":"passed","artifact":metadata}),
            )],
        })
        .await
        .unwrap();
    id
}
#[tokio::test]
async fn core_injects_out_of_band_and_revokes_on_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let executor = installations::executor("fixture");
    let driver = Arc::new(WaitingDriver {
        executor: executor.clone(),
        mcp: Mutex::new(None),
        replies: Mutex::new(vec![]),
        sink: Mutex::new(None),
    });
    let store = Arc::new(SqliteStore::open(dir.path().join("history.db")).unwrap());
    let core = Arc::new(
        Core::new(store.clone()).with_history_query_program(std::env::current_exe().unwrap()),
    );
    installations::register(&core, store.as_ref(), driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "test".into(),
            path: dir.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "test".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    // More than the former 16 MiB total budget, including an individual chunked fact.
    let old_turn = agent_domain::TurnId::new();
    let large = format!("needle 中文{}", "x".repeat(1_000_000));
    let seeded = append(&store, (0..18).map(|i| event(&task,"message.delta",json!({"taskId":task,"turnId":old_turn,"itemId":format!("old-{i}"),"text":large}))).collect()).await;
    let last = seeded.last().unwrap().sequence;
    append(
        &store,
        vec![
            event(
                &task,
                "message.delta",
                json!({"taskId":task,"turnId":old_turn,"itemId":"split","text":"cross fragment "}),
            ),
            event(
                &task,
                "message.delta",
                json!({"taskId":task,"turnId":old_turn,"itemId":"split","text":"phrase"}),
            ),
        ],
    )
    .await;
    let old_artifact = artifact(&store, &task, "中文 evidence").await;
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        task_id: task.clone(),
        prompt: "literal user question".into(),
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
    }));
    command.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(command).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while driver.sink.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mcp = driver.mcp.lock().unwrap().clone().unwrap();
    let response = query(&mcp).await;
    assert!(response["error"].is_null());
    assert_eq!(response["result"]["taskId"], task.to_string());
    // User prompt is not modified by MCP injection (asserted inside WaitingDriver).
    let head = response["result"]["throughSequence"].as_u64().unwrap();
    let body = request(
        &mcp,
        json!({"operation":"getArtifact","artifactId":old_artifact,"byteLimit":4}),
    )
    .await;
    assert_eq!(body["result"]["text"], "中");
    assert_eq!(body["result"]["nextOffset"], 3);
    assert!(
        request(
            &mcp,
            json!({"operation":"getArtifact","artifactId":old_artifact,"offset":1})
        )
        .await["error"]
            .is_string()
    );
    let future_artifact = artifact(&store, &task, "future evidence").await;
    assert!(
        request(
            &mcp,
            json!({"operation":"getArtifact","artifactId":future_artifact})
        )
        .await["error"]
            .is_string()
    );
    assert!(
        request(
            &mcp,
            json!({"operation":"getArtifact","artifactId":agent_domain::ArtifactId::new()})
        )
        .await["error"]
            .is_string()
    );

    // Mutate a projection that existed at the head: canonical reads must remain frozen.
    append(
        &store,
        vec![event(
            &task,
            "message.delta",
            json!({"taskId":task,"turnId":old_turn,"itemId":"old-17","text":"FUTURE_ONLY"}),
        )],
    )
    .await;
    let phrase = request(
        &mcp,
        json!({"operation":"searchHistory","text":"fragment phrase"}),
    )
    .await;
    assert_eq!(
        phrase["result"]["items"][0]["text"],
        "cross fragment phrase"
    );
    let future = request(
        &mcp,
        json!({"operation":"searchHistory","text":"FUTURE_ONLY"}),
    )
    .await;
    assert_eq!(future["result"]["items"], json!([]));
    let page = request(
        &mcp,
        json!({"operation":"getTurns","afterSequence":last-1,"limit":1}),
    )
    .await;
    assert_eq!(page["result"]["items"][0]["sequence"], last);
    assert_eq!(page["result"]["nextCursor"], last - 1);
    assert_eq!(page["result"]["nextOffset"], 32768);
    assert!(
        request(&mcp, json!({"operation":"getTurns","afterSequence":head+1})).await["error"]
            .is_string()
    );
    let session = core.bootstrap().await.unwrap().session_maps[0]
        .session_map_id
        .clone();
    let missing = |through| {
        agent_core::history_delivery::missing_events(
            store.as_ref(),
            &task,
            Some(&session),
            through,
            100,
        )
    };
    let receipt = page["result"]["deliveryReceipt"].as_str().unwrap();
    assert!(
        missing(core.bootstrap().await.unwrap().last_sequence)
            .await
            .unwrap()
            .iter()
            .any(|e| e.sequence == last)
    );
    assert_eq!(
        request(&mcp, json!({"ackReceipt":receipt})).await["result"]["acknowledged"],
        true
    );
    assert!(request(&mcp, json!({"ackReceipt":receipt})).await["error"].is_string());
    // Partial delivery must not hide the remaining bytes.
    assert!(
        missing(core.bootstrap().await.unwrap().last_sequence)
            .await
            .unwrap()
            .iter()
            .any(|e| e.sequence == last)
    );
    let mut content = page["result"]["items"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut offset = 32768;
    while offset < large.len() {
        let page = request(
            &mcp,
            json!({"operation":"getTurns","afterSequence":last-1,"limit":1,"offset":offset}),
        )
        .await;
        assert!(page["error"].is_null(), "{page}");
        content.push_str(page["result"]["items"][0]["text"].as_str().unwrap());
        request(
            &mcp,
            json!({"ackReceipt":page["result"]["deliveryReceipt"]}),
        )
        .await;
        offset = page["result"]["nextOffset"]
            .as_u64()
            .map_or(large.len(), |v| usize::try_from(v).unwrap());
    }
    assert_eq!(content, large);
    let through = core.bootstrap().await.unwrap().last_sequence;
    assert!(
        !missing(through)
            .await
            .unwrap()
            .iter()
            .any(|e| e.sequence == last)
    );
    store.rebuild_projections().unwrap();
    assert!(
        !missing(through)
            .await
            .unwrap()
            .iter()
            .any(|e| e.sequence == last)
    );
    let reopened = SqliteStore::open(dir.path().join("history.db")).unwrap();
    assert!(
        !agent_core::history_delivery::missing_events(
            &reopened,
            &task,
            Some(&session),
            through,
            100
        )
        .await
        .unwrap()
        .iter()
        .any(|e| e.sequence == last)
    );
    assert!(
        agent_core::history_delivery::missing_events(
            store.as_ref(),
            &task,
            Some(&agent_domain::SessionMapId::new()),
            through,
            100
        )
        .await
        .unwrap()
        .iter()
        .any(|e| e.sequence == last)
    );
    // A wrong token cannot read another endpoint or acknowledge a receipt.
    let mut wrong = mcp.clone();
    wrong.args[2] = "wrong".into();
    assert!(query(&wrong).await["error"].is_string());
    let snapshot = core.bootstrap().await.unwrap();
    let active = snapshot.tasks[0].active_primary_run.as_ref().unwrap();
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task,
        turn_id: active.turn_id.clone(),
        run_id: active.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(cancel).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if query(&mcp).await["error"].is_string() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(query(&mcp).await["error"].is_string());
}

#[tokio::test]
async fn builtin_history_tools_are_approved_once_but_external_tools_still_require_approval() {
    let dir = tempfile::tempdir().unwrap();
    let executor = installations::executor("fixture");
    let driver = Arc::new(WaitingDriver {
        executor: executor.clone(),
        mcp: Mutex::new(None),
        replies: Mutex::new(vec![]),
        sink: Mutex::new(None),
    });
    let store = Arc::new(SqliteStore::open(dir.path().join("history.db")).unwrap());
    let core = Arc::new(
        Core::new(store.clone()).with_history_query_program(std::env::current_exe().unwrap()),
    );
    installations::register(&core, store.as_ref(), driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "test".into(),
            path: dir.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "test".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        task_id: task.clone(),
        prompt: "literal user question".into(),
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
    }));
    command.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(command).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while driver.sink.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mcp = driver.mcp.lock().unwrap().clone().unwrap();
    let approval = |id: &str, tool: String, input: Value| DriverEvent::ApprovalRequested {
        native_request_id: id.into(),
        tool_name: tool,
        command_summary: input.to_string(),
        options: vec![
            ApprovalOption {
                option_id: "persist-all".into(),
                label: "Always".into(),
                semantic_role: ApprovalSemanticRole::ApprovePersistent,
            },
            ApprovalOption {
                option_id: "native-once".into(),
                label: "Once".into(),
                semantic_role: ApprovalSemanticRole::ApproveOnce,
            },
        ],
    };
    let send = |event| {
        driver
            .sink
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(Ok(event))
            .unwrap();
    };
    let tools = [
        "getTurns",
        "getDecisionLog",
        "searchHistory",
        "getChangesSinceCheckpoint",
        "getArtifact",
    ];
    for tool in tools {
        send(approval(
            tool,
            format!("{}-{tool}: {tool}", mcp.name),
            json!({"providerIdentifier":mcp.name,"toolName":tool,"args":{"limit":20}}),
        ));
    }
    send(approval(
        "external",
        "mcp__outside__getTurns".into(),
        json!({"limit":20}),
    ));
    send(approval(
        "shell",
        "Bash".into(),
        json!({"command":"git diff --stat"}),
    ));
    send(approval(
        "unqualified",
        "getTurns".into(),
        json!({"limit":20}),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if core.pending_approvals(&task, 50).await.unwrap().len() == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *driver.replies.lock().unwrap(),
        tools
            .map(|tool| (tool.to_owned(), "native-once".into()))
            .to_vec()
    );
    let pending = core.pending_approvals(&task, 50).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|item| item.approval_id.as_str())
            .collect::<Vec<_>>(),
        vec!["external", "shell", "unqualified"]
    );
    send(DriverEvent::TurnCompleted {
        summary: "done".into(),
    });
    // Non-background drivers end their stream after the terminal event.
    driver.sink.lock().unwrap().take();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // This task's endpoint survives ordinary turn completion for reuse, then
    // must be removed by the real durable permanent-deletion cleanup path.
    let native_directory = std::path::Path::new(&mcp.directory);
    assert!(native_directory.exists());
    let snapshot = core.bootstrap().await.unwrap();
    let mut delete = CommandEnvelope::local(CoreCommand::DeleteTask(TaskLifecycle {
        task_id: task.clone(),
    }));
    delete.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(delete).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while native_directory.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect(&mcp.args[1]).await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
