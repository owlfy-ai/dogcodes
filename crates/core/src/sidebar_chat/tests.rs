use super::*;
use crate::tests::MemoryRepository;
use agent_domain::{ProjectId, ProviderId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, RecordInstallationProbe,
    RegisterProject, TaskSummary,
};
use driver_api::{DriverError, DriverEventStream, SessionHandle, SessionRequest};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Driver {
    executor: ExecutorRef,
    workspace: tempfile::TempDir,
    safe: bool,
    events: Mutex<Option<Vec<DriverEvent>>>,
    streamed_events:
        Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Result<DriverEvent, DriverError>>>>,
    prompts: Mutex<Vec<PromptRequest>>,
    starts: AtomicUsize,
    stops: AtomicUsize,
    interrupts: AtomicUsize,
    shutdown_pause: Mutex<Option<Arc<tokio::sync::Notify>>>,
}
#[async_trait::async_trait]
impl AgentDriver for Driver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    fn supports_readonly_chat(&self) -> bool {
        self.safe
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        panic!("coding session must never be used");
    }
    fn allows_readonly_chat_tool(&self, name: &str) -> bool {
        matches!(name, "Read" | "Glob" | "Grep")
    }
    async fn create_chat_session(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        assert_eq!(
            std::path::Path::new(directory),
            self.workspace.path().join("fork")
        );
        assert_eq!(
            additional,
            &[self
                .workspace
                .path()
                .join("shared")
                .to_string_lossy()
                .into_owned()]
        );
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(directory).join("main.rs")).unwrap(),
            "// uncommitted fork code"
        );
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(SessionHandle {
            native_session_id: "chat".into(),
        })
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        assert!(request.context.is_empty() && request.attachments.is_empty());
        assert!(request.permission_mode.is_none());
        assert_eq!(
            std::path::Path::new(&request.working_directory),
            self.workspace.path().join("fork")
        );
        assert_eq!(
            request.additional_directories,
            vec![
                self.workspace
                    .path()
                    .join("shared")
                    .to_string_lossy()
                    .into_owned()
            ]
        );
        assert!(request.prompt.contains("不修改、新建或删除任何项目文件"));
        // The directory travels in the JSON context, so Windows backslashes arrive escaped.
        assert!(
            request
                .prompt
                .contains(&serde_json::json!(request.working_directory).to_string())
        );
        self.prompts.lock().unwrap().push(request);
        if let Some(receiver) = self.streamed_events.lock().unwrap().take() {
            return Ok(Box::pin(
                tokio_stream::wrappers::UnboundedReceiverStream::new(receiver),
            ));
        }
        match self.events.lock().unwrap().clone() {
            Some(events) => Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok)))),
            None => Ok(Box::pin(futures::stream::pending())),
        }
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        self.interrupts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let pause = self.shutdown_pause.lock().unwrap().clone();
        if let Some(pause) = pause {
            pause.notified().await;
        }
        Ok(())
    }
}
fn probe(status: InstallationStatus) -> CommandEnvelope<CoreCommand> {
    let mut request = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        RecordInstallationProbe {
            provider_id: "codex".into(),
            executable: "test-sidebar-agent".into(),
            fingerprint: "test-sidebar-fingerprint".into(),
            status,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    request.actor = ActorRef::SystemRecovery;
    request
}

async fn setup(
    safe: bool,
    events: Option<Vec<DriverEvent>>,
) -> (Arc<Core>, Arc<MemoryRepository>, Arc<Driver>, ChatRequest) {
    let repo = Arc::new(MemoryRepository::default());
    let core = Arc::new(Core::new(repo.clone()));
    core.execute(probe(InstallationStatus::Ready))
        .await
        .unwrap();
    let installation = core.bootstrap().await.unwrap().installations.remove(0);
    let task_id = TaskId::new();
    let executor = ExecutorRef {
        provider_id: ProviderId::parse("codex").unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    };
    let project_id = ProjectId::new();
    *repo.task_override.lock().await = Some(TaskSummary {
        task_id: task_id.clone(),
        project_id: project_id.clone(),
        title: "Coding task".into(),
        preferred_executor: executor.clone(),
        last_turn_executor: None,
        updated_at: None,
        stream_version: 1,
        active_primary_run: None,
    });
    let workspace = tempfile::tempdir().unwrap();
    for name in ["original", "fork", "shared"] {
        std::fs::create_dir(workspace.path().join(name)).unwrap();
    }
    std::fs::write(
        workspace.path().join("fork/main.rs"),
        "// uncommitted fork code",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("original/main.rs"),
        "// original code",
    )
    .unwrap();
    for (id, name, additional) in [
        (ProjectId::new(), "original", vec![]),
        (
            project_id,
            "fork",
            vec![
                workspace
                    .path()
                    .join("shared")
                    .to_string_lossy()
                    .into_owned(),
            ],
        ),
    ] {
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: id,
                display_name: name.into(),
                path: workspace.path().join(name).to_string_lossy().into_owned(),
                additional_directories: additional,
            },
        )))
        .await
        .unwrap();
    }
    let driver = Arc::new(Driver {
        executor: executor.clone(),
        workspace,
        safe,
        events: Mutex::new(events),
        streamed_events: Mutex::new(None),
        prompts: Mutex::new(vec![]),
        starts: AtomicUsize::new(0),
        stops: AtomicUsize::new(0),
        interrupts: AtomicUsize::new(0),
        shutdown_pause: Mutex::new(None),
    });
    core.register_driver(driver.clone()).await;
    let request = ChatRequest {
        task_id,
        request_id: RunId::new(),
        executor,
        text: "请直接修改代码".into(),
    };
    (core, repo, driver, request)
}
async fn finished(core: &Core, task: &TaskId) -> ChatState {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state = core.sidebar_chat(task).await.unwrap();
            if !state.running {
                return state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn chat_isolated_multiturn_idempotent_and_durable_without_changing_coding_session() {
    let events = vec![
        DriverEvent::TextDelta {
            item_id: "a".into(),
            text: "只能提供建议。".into(),
        },
        DriverEvent::TurnCompleted {
            summary: String::new(),
        },
    ];
    let (core, repo, driver, request) = setup(true, Some(events)).await;
    core.start_sidebar_chat(request.clone()).await.unwrap();
    core.start_sidebar_chat(request.clone()).await.unwrap();
    let state = finished(&core, &request.task_id).await;
    assert!(state.error.is_none());
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].text, "只能提供建议。");
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    assert_eq!(driver.stops.load(Ordering::SeqCst), 1);
    let restored = Arc::new(Core::new(repo));
    restored.register_driver(driver.clone()).await;
    assert_eq!(
        restored
            .sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .messages[1]
            .text,
        "只能提供建议。"
    );
    let mut next = request.clone();
    next.request_id = RunId::new();
    next.text = "解释上面的建议".into();
    restored.start_sidebar_chat(next).await.unwrap();
    assert_eq!(
        finished(&restored, &request.task_id).await.messages.len(),
        4
    );
    assert!(
        driver.prompts.lock().unwrap()[1]
            .prompt
            .contains("只能提供建议。")
    );
    assert!(
        restored
            .require_task(&request.task_id)
            .await
            .unwrap()
            .active_primary_run
            .is_none()
    );
    assert!(
        restored
            .clear_sidebar_chat(clear_request(&restored, &request.task_id).await)
            .await
            .unwrap()
            .state
            .messages
            .is_empty()
    );
}
#[tokio::test]
async fn unsupported_drivers_fail_closed_before_starting_any_session() {
    let (core, _, driver, request) = setup(false, None).await;
    assert!(core.chat_executors().await.unwrap().is_empty());
    assert!(
        core.start_sidebar_chat(request)
            .await
            .unwrap_err()
            .contains("只读问答")
    );
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sidebar_chat_rejects_unready_installations_and_unbound_executor_identities() {
    let (core, _, driver, request) = setup(true, None).await;
    for status in [
        InstallationStatus::Installed,
        InstallationStatus::NotConfigured,
        InstallationStatus::UnsupportedVersion,
        InstallationStatus::Broken,
    ] {
        core.execute(probe(status)).await.unwrap();
        assert!(core.chat_executors().await.unwrap().is_empty());
        assert!(
            core.start_sidebar_chat(request.clone())
                .await
                .unwrap_err()
                .contains("尚未就绪")
        );
        assert!(
            core.sidebar_chat_receipt(&request.task_id, &request.request_id)
                .await
                .unwrap()
                .is_none()
        );
    }
    core.execute(probe(InstallationStatus::Ready))
        .await
        .unwrap();
    assert_eq!(
        core.chat_executors().await.unwrap(),
        std::slice::from_ref(&request.executor)
    );
    for installation_id in [None, Some(agent_domain::InstallationId::new().to_string())] {
        let mut stale = request.clone();
        stale.executor.installation_id = installation_id;
        assert!(
            core.start_sidebar_chat(stale)
                .await
                .unwrap_err()
                .contains("尚未就绪")
        );
    }
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sidebar_chat_admission_waits_for_pending_installation_downgrade() {
    let (core, _, driver, request) = setup(true, None).await;
    let admission = core.agent_admission.read().await;
    let update = core.execute(probe(InstallationStatus::Broken));
    tokio::pin!(update);
    assert!(futures::poll!(&mut update).is_pending());
    let start = core.start_sidebar_chat(request.clone());
    tokio::pin!(start);
    assert!(futures::poll!(&mut start).is_pending());
    drop(admission);
    tokio::time::timeout(Duration::from_secs(3), async {
        update.await.unwrap();
        assert!(start.await.unwrap_err().contains("尚未就绪"));
    })
    .await
    .unwrap();
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert!(
        core.sidebar_chat_receipt(&request.task_id, &request.request_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sidebar_chat_admission_waits_for_pending_agent_disable() {
    let (core, _, driver, request) = setup(true, None).await;
    // Keep admission open while queueing a settings write, then a new request.
    // The queued disable must win before that new request can be accepted.
    let admission = core.agent_admission.read().await;
    let mut disable = CommandEnvelope::local(CoreCommand::SetAgentPreferences(
        agent_protocol::AgentPreferences {
            disabled: vec!["codex".into()],
            ..Default::default()
        },
    ));
    disable.expected_stream_version = Some(0);
    let update = core.execute(disable);
    tokio::pin!(update);
    assert!(futures::poll!(&mut update).is_pending());
    let start = core.start_sidebar_chat(request.clone());
    tokio::pin!(start);
    assert!(futures::poll!(&mut start).is_pending());
    drop(admission);
    tokio::time::timeout(Duration::from_secs(3), async {
        update.await.unwrap();
        assert!(start.await.unwrap_err().contains("已禁用"));
    })
    .await
    .unwrap();
    assert!(core.chat_executors().await.unwrap().is_empty());
    assert!(
        core.sidebar_chat_receipt(&request.task_id, &request.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sidebar_chat_owns_service_until_cancellation_shutdown_has_finished() {
    let (core, repo, driver, request) = setup(true, None).await;
    let release = Arc::new(tokio::sync::Notify::new());
    *driver.shutdown_pause.lock().unwrap() = Some(release.clone());
    assert!(!core.has_active_sidebar_chats().await);
    core.start_sidebar_chat(request.clone()).await.unwrap();
    assert!(core.has_active_sidebar_chats().await);
    // A new service must not mistake the saved acceptance for live ownership.
    let restarted = Core::new(repo);
    assert!(!restarted.has_active_sidebar_chats().await);
    assert!(
        !restarted
            .sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .running
    );
    assert!(!restarted.has_active_sidebar_chats().await);
    core.cancel_sidebar_chat(&request.task_id, &request.request_id)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while driver.stops.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(core.has_active_sidebar_chats().await);
    assert!(
        core.sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .cancelling
    );
    release.notify_one();
    assert!(!finished(&core, &request.task_id).await.running);
    assert!(!core.has_active_sidebar_chats().await);
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
    assert_eq!(driver.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cancellation_blocks_duplicate_runs_and_shuts_down_native_session() {
    let (core, _, driver, request) = setup(true, None).await;
    core.start_sidebar_chat(request.clone()).await.unwrap();
    let mut duplicate = request.clone();
    duplicate.request_id = RunId::new();
    assert!(core.start_sidebar_chat(duplicate).await.is_err());
    assert!(
        core.clear_sidebar_chat(clear_request(&core, &request.task_id).await)
            .await
            .is_err()
    );
    core.cancel_sidebar_chat(&request.task_id, &request.request_id)
        .await
        .unwrap();
    let state = finished(&core, &request.task_id).await;
    assert_eq!(state.error.as_deref(), Some("已停止回答"));
    assert_eq!(driver.stops.load(Ordering::SeqCst), 1);
    assert_eq!(driver.interrupts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn mutations_and_permission_requests_stop_the_chat() {
    for event in [
        DriverEvent::ApprovalRequested {
            native_request_id: "approval".into(),
            tool_name: "Write".into(),
            command_summary: "edit file".into(),
            options: vec![],
        },
        DriverEvent::ToolStarted {
            item_id: "write".into(),
            tool_name: "Write".into(),
            summary: "edit".into(),
        },
        DriverEvent::Presentation {
            item_id: "tool".into(),
            data: serde_json::json!({"type":"tool","name":"Bash"}),
        },
    ] {
        let (core, _, driver, request) = setup(true, Some(vec![event])).await;
        core.start_sidebar_chat(request.clone()).await.unwrap();
        assert!(
            finished(&core, &request.task_id)
                .await
                .error
                .unwrap()
                .contains("仅允许只读查看项目")
        );
        assert_eq!(driver.interrupts.load(Ordering::SeqCst), 1);
        assert_eq!(driver.stops.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn output_deduplicates_provider_mirrors_and_ignores_foreign_messages() {
    let mut output = ChatOutput::default();
    for data in [
        serde_json::json!({"type":"message","textDelta":"hello","sourceEventId":"same"}),
        serde_json::json!({"type":"message","textDelta":"hello","sourceEventId":"same"}),
        serde_json::json!({"type":"message","text":"wrong","sessionId":"foreign"}),
    ] {
        output
            .push(
                &DriverEvent::Presentation {
                    item_id: "a".into(),
                    data,
                },
                "chat",
            )
            .unwrap();
    }
    assert_eq!(output.text(), "hello");
    output
        .push(
            &DriverEvent::TextDelta {
                item_id: "a".into(),
                text: "hello".into(),
            },
            "chat",
        )
        .unwrap();
    assert_eq!(output.text(), "hello");
}

#[tokio::test]
async fn read_tools_continue_answering_in_the_selected_worktree_with_additional_roots() {
    let mut events = vec![];
    for name in ["Read", "Glob", "Grep"] {
        events.push(DriverEvent::ToolStarted {
            item_id: name.into(),
            tool_name: name.into(),
            summary: "reading".into(),
        });
        events.push(DriverEvent::Presentation {
            item_id: name.into(),
            data: serde_json::json!({"type":"tool","name":name,"status":"completed"}),
        });
    }
    events.push(DriverEvent::TextDelta {
        item_id: "a".into(),
        text: "当前 worktree 的 main.rs 包含未提交修改".into(),
    });
    events.push(DriverEvent::TurnCompleted {
        summary: String::new(),
    });
    let (core, _, driver, request) = setup(true, Some(events)).await;
    core.start_sidebar_chat(request.clone()).await.unwrap();
    let state = finished(&core, &request.task_id).await;
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(state.messages[1].text.contains("main.rs"));
    assert_eq!(driver.interrupts.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_to_string(driver.workspace.path().join("fork/main.rs")).unwrap(),
        "// uncommitted fork code"
    );
}

#[tokio::test]
async fn missing_workspace_fails_before_starting_a_session_or_recording_a_question() {
    let (core, _, driver, request) = setup(true, None).await;
    std::fs::remove_dir_all(driver.workspace.path().join("fork")).unwrap();
    assert!(
        core.start_sidebar_chat(request.clone())
            .await
            .unwrap_err()
            .contains("目录不可用")
    );
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
    assert!(
        core.sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .messages
            .is_empty()
    );
}

#[tokio::test]
async fn historical_chat_identity_survives_later_questions_clear_and_missing_driver() {
    let (core, repo, driver, first) = setup(
        true,
        Some(vec![DriverEvent::TurnCompleted {
            summary: String::new(),
        }]),
    )
    .await;
    core.start_sidebar_chat(first.clone()).await.unwrap();
    finished(&core, &first.task_id).await;
    let mut second = first.clone();
    second.request_id = RunId::new();
    second.text = "second".into();
    core.start_sidebar_chat(second.clone()).await.unwrap();
    finished(&core, &first.task_id).await;
    let before = driver.starts.load(Ordering::SeqCst);
    let replay = core.start_sidebar_chat(first.clone()).await.unwrap();
    assert_eq!(replay.request_id, Some(first.request_id.clone()));
    assert_eq!(replay.messages.len(), 2);
    assert_eq!(
        core.sidebar_chat(&first.task_id).await.unwrap().request_id,
        Some(second.request_id)
    );
    let mut changed = first.clone();
    changed.text = "changed".into();
    assert!(
        core.start_sidebar_chat(changed)
            .await
            .unwrap_err()
            .contains("不同")
    );
    let mut changed = first.clone();
    changed.executor.provider_id = ProviderId::parse("claude-code").unwrap();
    assert!(
        core.start_sidebar_chat(changed)
            .await
            .unwrap_err()
            .contains("不同")
    );
    core.clear_sidebar_chat(clear_request(&core, &first.task_id).await)
        .await
        .unwrap();
    let restored = Arc::new(Core::new(repo)); // Deliberately no connected driver.
    assert_eq!(
        restored
            .start_sidebar_chat(first.clone())
            .await
            .unwrap()
            .request_id,
        Some(first.request_id.clone())
    );
    assert!(
        restored
            .sidebar_chat(&first.task_id)
            .await
            .unwrap()
            .messages
            .is_empty()
    );
    let receipt = restored
        .sidebar_chat_receipt(&first.task_id, &first.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.request, Some(first));
    assert_eq!(driver.starts.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn chat_request_intent_is_durable_before_execution_and_interrupted_receipt_does_not_restart()
{
    let (core, repo, driver, request) = setup(true, None).await;
    core.start_sidebar_chat(request.clone()).await.unwrap();
    let live = core
        .sidebar_chat_receipt(&request.task_id, &request.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.request, Some(request.clone()));
    assert!(live.state.running);
    let restored = Arc::new(Core::new(repo));
    let recovered = restored.start_sidebar_chat(request.clone()).await.unwrap();
    assert!(!recovered.running);
    assert!(recovered.error.unwrap().contains("中断"));
    assert!(
        restored
            .sidebar_chat_receipt(&request.task_id, &RunId::new())
            .await
            .unwrap()
            .is_none()
    );
    core.cancel_sidebar_chat(&request.task_id, &request.request_id)
        .await
        .unwrap();
    finished(&core, &request.task_id).await;
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
}

async fn clear_request(core: &Core, task: &TaskId) -> ChatClearRequest {
    ChatClearRequest {
        task_id: task.clone(),
        operation_id: CommandId::new(),
        expected_revision: core.sidebar_chat(task).await.unwrap().revision,
    }
}

#[tokio::test]
async fn stale_clear_cannot_erase_new_question_and_replay_does_not_clear_twice() {
    let (core, repository, driver, first) = setup(
        true,
        Some(vec![DriverEvent::TurnCompleted {
            summary: String::new(),
        }]),
    )
    .await;
    core.start_sidebar_chat(first.clone()).await.unwrap();
    let original = finished(&core, &first.task_id).await;
    let clear = clear_request(&core, &first.task_id).await;
    let receipt = core.clear_sidebar_chat(clear.clone()).await.unwrap();
    assert!(receipt.state.messages.is_empty());
    assert_ne!(receipt.state.revision, original.revision);
    let mut next = first.clone();
    next.request_id = RunId::new();
    next.text = "new question".into();
    *driver.events.lock().unwrap() = Some(vec![DriverEvent::TurnCompleted {
        summary: String::new(),
    }]);
    core.start_sidebar_chat(next.clone()).await.unwrap();
    let latest = finished(&core, &first.task_id).await;
    let replay = core.clear_sidebar_chat(clear.clone()).await.unwrap();
    assert_eq!(replay.state.revision, receipt.state.revision);
    assert_eq!(
        core.sidebar_chat(&first.task_id).await.unwrap().revision,
        latest.revision
    );
    let stale = ChatClearRequest {
        operation_id: CommandId::new(),
        ..clear.clone()
    };
    assert!(
        core.clear_sidebar_chat(stale)
            .await
            .unwrap_err()
            .contains("已更新")
    );
    let changed = ChatClearRequest {
        expected_revision: latest.revision.clone(),
        ..clear.clone()
    };
    assert!(
        core.clear_sidebar_chat(changed)
            .await
            .unwrap_err()
            .contains("不同")
    );
    assert_eq!(
        core.sidebar_chat(&first.task_id).await.unwrap().messages[0].text,
        "new question"
    );
    let restored = Core::new(repository);
    assert_eq!(
        restored
            .clear_sidebar_chat(clear.clone())
            .await
            .unwrap()
            .request,
        clear
    );
    assert_eq!(
        restored
            .sidebar_chat(&first.task_id)
            .await
            .unwrap()
            .revision,
        latest.revision
    );
}

#[tokio::test]
async fn concurrent_clears_of_the_same_revision_have_only_one_winner() {
    let (core, _, _, request) = setup(
        true,
        Some(vec![DriverEvent::TurnCompleted {
            summary: String::new(),
        }]),
    )
    .await;
    core.start_sidebar_chat(request.clone()).await.unwrap();
    finished(&core, &request.task_id).await;
    let first = clear_request(&core, &request.task_id).await;
    let second = ChatClearRequest {
        operation_id: CommandId::new(),
        ..first.clone()
    };
    let (one, two) = tokio::join!(
        core.clear_sidebar_chat(first.clone()),
        core.clear_sidebar_chat(second.clone())
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let (winner, loser) = if one.is_ok() {
        (first, second)
    } else {
        (second, first)
    };
    assert!(
        core.sidebar_chat_clear_receipt(&request.task_id, &winner.operation_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        core.sidebar_chat_clear_receipt(&request.task_id, &loser.operation_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sidebar_chat_watch_tracks_stream_cancel_clear_and_releases_capacity() {
    let (core, _, driver, request) = setup(true, None).await;
    let (send, receiver) = tokio::sync::mpsc::unbounded_channel();
    *driver.streamed_events.lock().unwrap() = Some(receiver);
    let shutdown = Arc::new(tokio::sync::Notify::new());
    *driver.shutdown_pause.lock().unwrap() = Some(shutdown.clone());
    assert!(core.subscribe_sidebar_chat(TaskId::new()).await.is_err());
    let mut first = core
        .subscribe_sidebar_chat(request.task_id.clone())
        .await
        .unwrap();
    let mut second = core
        .subscribe_sidebar_chat(request.task_id.clone())
        .await
        .unwrap();
    let mut others = Vec::new();
    for _ in 0..62 {
        others.push(
            core.subscribe_sidebar_chat(request.task_id.clone())
                .await
                .unwrap(),
        );
    }
    assert!(
        core.subscribe_sidebar_chat(request.task_id.clone())
            .await
            .is_err()
    );
    drop(others);
    assert!(matches!(
        first.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    assert!(matches!(
        second.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    core.start_sidebar_chat(request.clone()).await.unwrap();
    assert!(matches!(
        first.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    assert!(matches!(
        second.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    send.send(Ok(DriverEvent::TextDelta {
        item_id: "answer".into(),
        text: "streamed answer".into(),
    }))
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), first.next())
            .await
            .unwrap()
            .unwrap(),
        ChatChange::Invalidated
    ));
    assert!(matches!(
        second.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    assert_eq!(
        core.sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .messages
            .last()
            .unwrap()
            .text,
        "streamed answer"
    );
    drop(first);
    assert!(core.sidebar_chat(&request.task_id).await.unwrap().running);
    assert_eq!(driver.interrupts.load(Ordering::SeqCst), 0);
    let mut reconnected = core
        .subscribe_sidebar_chat(request.task_id.clone())
        .await
        .unwrap();
    assert!(matches!(
        reconnected.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    core.cancel_sidebar_chat(&request.task_id, &request.request_id)
        .await
        .unwrap();
    assert!(matches!(
        second.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    assert!(
        core.sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .cancelling
    );
    shutdown.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), second.next())
            .await
            .unwrap()
            .unwrap(),
        ChatChange::Invalidated
    ));
    assert!(!core.sidebar_chat(&request.task_id).await.unwrap().running);
    let clear = clear_request(&core, &request.task_id).await;
    core.clear_sidebar_chat(clear).await.unwrap();
    assert!(matches!(
        second.next().await.unwrap(),
        ChatChange::Invalidated
    ));
    assert!(
        core.sidebar_chat(&request.task_id)
            .await
            .unwrap()
            .messages
            .is_empty()
    );
    drop(second);
    drop(reconnected);
    let mut all = Vec::new();
    for _ in 0..64 {
        all.push(
            core.subscribe_sidebar_chat(request.task_id.clone())
                .await
                .unwrap(),
        );
    }
    assert_eq!(driver.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sidebar_chat_rechecks_task_after_waiting_for_the_lifecycle_lock() {
    let (core, repo, driver, request) = setup(true, None).await;
    let chats = core.sidebar_chats.lock().await;
    let read = core.sidebar_chat(&request.task_id);
    let receipt = core.sidebar_chat_receipt(&request.task_id, &request.request_id);
    let start = core.start_sidebar_chat(request.clone());
    let cancel = core.cancel_sidebar_chat(&request.task_id, &request.request_id);
    let clear_request = ChatClearRequest {
        task_id: request.task_id.clone(),
        operation_id: CommandId::new(),
        expected_revision: String::new(),
    };
    let clear = core.clear_sidebar_chat(clear_request.clone());
    let clear_receipt =
        core.sidebar_chat_clear_receipt(&request.task_id, &clear_request.operation_id);
    let watch = core.subscribe_sidebar_chat(request.task_id.clone());
    tokio::pin!(read, receipt, start, cancel, clear, clear_receipt, watch);
    assert!(futures::poll!(&mut read).is_pending());
    assert!(futures::poll!(&mut receipt).is_pending());
    assert!(futures::poll!(&mut start).is_pending());
    assert!(futures::poll!(&mut cancel).is_pending());
    assert!(futures::poll!(&mut clear).is_pending());
    assert!(futures::poll!(&mut clear_receipt).is_pending());
    assert!(futures::poll!(&mut watch).is_pending());
    // The task existed when the operations arrived, but no longer exists when
    // they obtain the lifecycle lock. None may reconstruct a cached chat.
    *repo.task_override.lock().await = None;
    drop(chats);
    tokio::time::timeout(Duration::from_secs(3), async {
        assert!(read.await.is_err());
        assert!(receipt.await.is_err());
        assert!(start.await.is_err());
        assert!(cancel.await.is_err());
        assert!(clear.await.is_err());
        assert!(clear_receipt.await.is_err());
        assert!(watch.await.is_err());
    })
    .await
    .unwrap();
    assert!(core.sidebar_chats.lock().await.is_empty());
    assert_eq!(core.sidebar_chat_watch_slots.available_permits(), 64);
    assert_eq!(driver.starts.load(Ordering::SeqCst), 0);
}
