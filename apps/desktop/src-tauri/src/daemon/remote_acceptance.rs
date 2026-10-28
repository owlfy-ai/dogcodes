//! Opt-in isolated end-to-end fixture; never starts a real agent or opens user data.
use super::{
    ActorRef, Arc, CommandEnvelope, Config, Core, CoreCommand, Duration, Mutex, PairingBook,
    PathBuf, Ready, Secrets, ServiceControl, Value, failure, json, validate_service,
};
use agent_domain::ExecutorRef;
use driver_api::FakeDriver;
use driver_api::{
    AgentDriver, ApprovalOption, ApprovalSemanticRole, DriverError, DriverEvent, DriverEventStream,
    PromptRequest, SessionHandle, SessionRequest,
};
struct ApprovalDriver {
    fake: FakeDriver,
    sender: Mutex<Option<tokio::sync::mpsc::Sender<Result<DriverEvent, DriverError>>>>,
    count: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl AgentDriver for ApprovalDriver {
    fn executor(&self) -> &ExecutorRef {
        self.fake.executor()
    }
    fn driver_version(&self) -> &'static str {
        "fixture"
    }
    async fn create_session(&self, r: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.fake.create_session(r).await
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        r: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tx.send(Ok(DriverEvent::TurnStarted {
            native_turn_id: Some("fixture-turn".into()),
        }))
        .await
        .map_err(|_| DriverError::Protocol("fixture closed".into()))?;
        tx.send(Ok(DriverEvent::TextDelta {
            item_id: "fixture-text".into(),
            text: format!("Synthetic task: {}", r.prompt),
        }))
        .await
        .map_err(|_| DriverError::Protocol("fixture closed".into()))?;
        tx.send(Ok(DriverEvent::ApprovalRequested {
            native_request_id: "fixture-approval".into(),
            tool_name: "fixture".into(),
            command_summary: "Synthetic approval; no command is executed".into(),
            options: vec![ApprovalOption {
                option_id: "allow-once".into(),
                label: "允许一次".into(),
                semantic_role: ApprovalSemanticRole::ApproveOnce,
            }],
        }))
        .await
        .map_err(|_| DriverError::Protocol("fixture closed".into()))?;
        *self.sender.lock().await = Some(tx);
        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|item| (item, rx)) },
        )))
    }
    async fn respond_to_interaction(
        &self,
        _: &SessionHandle,
        _: &str,
        _: &str,
    ) -> Result<(), DriverError> {
        if let Some(tx) = self.sender.lock().await.take() {
            let _ = tx
                .send(Ok(DriverEvent::ApprovalClosed {
                    native_request_id: "fixture-approval".into(),
                }))
                .await;
            let _ = tx
                .send(Ok(DriverEvent::TurnCompleted {
                    summary: "Synthetic approval completed".into(),
                }))
                .await;
        }
        Ok(())
    }
    /// Like a real driver, an interrupt ends the open turn.
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        if let Some(tx) = self.sender.lock().await.take() {
            let _ = tx
                .send(Ok(DriverEvent::TurnInterrupted {
                    summary: "Synthetic turn interrupted".into(),
                }))
                .await;
        }
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
pub(super) async fn run(service: String, directory: PathBuf) -> Result<(), String> {
    if !directory.join(".remote-acceptance").is_file() {
        return Err("isolated fixture marker required".into());
    }
    eprintln!("remote fixture: initializing");
    validate_service(&service)?;
    let core = Arc::new(Core::new(Arc::new(
        agent_store::SqliteStore::open(directory.join("fixture.db")).map_err(failure)?,
    )));
    let project = agent_domain::ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        agent_protocol::RegisterProject {
            project_id: project.clone(),
            display_name: "Remote fixture".into(),
            path: directory.to_string_lossy().into(),
            additional_directories: vec![],
        },
    )))
    .await
    .map_err(failure)?;
    let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        agent_protocol::RecordInstallationProbe {
            provider_id: "fake".into(),
            executable: "fixture:fake".into(),
            fingerprint: "isolated-remote-fixture".into(),
            status: agent_protocol::InstallationStatus::Ready,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    probe.actor = ActorRef::SystemRecovery;
    core.execute(probe).await.map_err(failure)?;
    let installed = core
        .bootstrap()
        .await
        .map_err(failure)?
        .installations
        .remove(0);
    let executor = ExecutorRef {
        provider_id: agent_domain::ProviderId::parse("fake").map_err(failure)?,
        installation_id: Some(installed.installation_id.to_string()),
    };
    let driver = Arc::new(ApprovalDriver {
        fake: FakeDriver::with_executor(executor.clone()),
        sender: Mutex::default(),
        count: std::sync::atomic::AtomicUsize::new(0),
    });
    core.register_driver(driver.clone()).await;
    // A known, pinned recipe avoids invoking an actual Agent during gateway
    // acceptance. Its only process prints a marker and exits.
    std::fs::create_dir_all(directory.join("project-runs")).map_err(failure)?;
    std::fs::write(
        directory.join("project-runs").join(format!("{project}-{}.json", std::env::consts::OS)),
        serde_json::to_vec(&json!({
            "projectId":project,"platform":std::env::consts::OS,"shell":"fixture",
            "generation":"fixture-generation","executor":null,
            "directories":[{"path":directory,"status":"ready","userModified":true,"error":null,
                "commands":[{"id":"fixture-run","label":"Fixture run","command":"echo fixture-project-run",
                    "cwd":".","source":"fixture","pinned":true,"userRenamed":false}]}],
            "operations":{}
        })).map_err(failure)?,
    ).map_err(failure)?;
    let control = Arc::new(ServiceControl::new(&directory));
    eprintln!("remote fixture: registering route");
    let registration: Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(failure)?
        .post(format!("{service}/v1/hosts"))
        .send()
        .await
        .map_err(failure)?
        .error_for_status()
        .map_err(failure)?
        .json()
        .await
        .map_err(failure)?;
    let keys = dogcodes_endpoint::keypair().map_err(failure)?;
    let config = Config {
        service: service.clone(),
        host_id: registration["hostId"].as_str().ok_or("missing id")?.into(),
        public_key: hex::encode(keys.public),
    };
    let secrets = Secrets {
        token: registration["hostToken"]
            .as_str()
            .ok_or("missing token")?
            .into(),
        private_key: hex::encode(keys.private),
    };
    control.remote.state.lock().await.ready = Some(Ready {
        config,
        secrets,
        book: PairingBook::load(directory.join("devices.json")).map_err(failure)?,
    });
    eprintln!("remote fixture: creating invitation");
    let invitation = control
        .remote
        .call(
            &core,
            &control,
            json!({"type":"invite","service":service,"dialogId":"fixture"}),
        )
        .await?;
    std::fs::write(
        directory.join("invitation.json"),
        serde_json::to_vec(&invitation).map_err(failure)?,
    )
    .map_err(failure)?;
    eprintln!("remote fixture: ready");
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>return Ok(()),
            _=tick.tick()=>{
                if directory.join("revoke").exists(){
                    let grants={let state=control.remote.state.lock().await;state.ready.as_ref().map(|r|r.book.grants()).unwrap_or_default()};
                    for grant in grants.into_iter().filter(|g|!g.revoked){control.remote.call(&core,&control,json!({"type":"revoke","deviceId":grant.device_id})).await?;}
                }
                let snapshot=core.bootstrap().await.map_err(failure)?;
                std::fs::write(directory.join("evidence.json"),serde_json::to_vec(&json!({"requests":driver.count.load(std::sync::atomic::Ordering::SeqCst),"tasks":snapshot.tasks.len()})).map_err(failure)?).map_err(failure)?;
            }
        }
    }
}
