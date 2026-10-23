use super::*;
fn fixture(root: &Path) -> (ProjectRunHost, Arc<Core>, ProjectSummary) {
    let host = ProjectRunHost::new(root.join("catalogs"));
    let core = Arc::new(Core::new(Arc::new(
        agent_store::SqliteStore::open_in_memory().unwrap(),
    )));
    let project = ProjectSummary {
        project_id: ProjectId::new(),
        path: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        display_name: "test".into(),
        task_count: 0,
        stream_version: 1,
        updated_at: None,
    };
    let catalog = RunCatalog {
        project_id: project.project_id.to_string(),
        platform: std::env::consts::OS.into(),
        shell: shell().into(),
        generation: "v1".into(),
        executor: None,
        directories: vec![RunDirectory {
            path: project.path.clone(),
            status: "ready".into(),
            commands: vec![],
            error: None,
            user_modified: false,
        }],
    };
    host.save(&catalog.into()).unwrap();
    (host, core, project)
}
fn request() -> AnalyzeRequest {
    AnalyzeRequest {
        operation_id: uuid::Uuid::new_v4().to_string(),
        task_id: "task".into(),
        expected_generation: "v1".into(),
        executor: None,
        overwrite_generation: None,
    }
}
async fn run(
    host: &ProjectRunHost,
    core: &Arc<Core>,
    project: &ProjectSummary,
    request: AnalyzeRequest,
) -> Result<RunCatalog, MutationError> {
    analyze_recorded(
        host.clone(),
        core.clone(),
        project.clone(),
        request.executor.clone(),
        true,
        request.overwrite_generation.clone(),
        Some(request),
    )
    .await
}
async fn analysis_driver(core: &Arc<Core>) -> (ExecutorRef, Arc<driver_api::FakeDriver>) {
    use agent_domain::ProviderId;
    use agent_protocol::{
        ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, RecordInstallationProbe,
    };
    use driver_api::FakeDriver;
    let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        RecordInstallationProbe {
            provider_id: "analysis-test".into(),
            executable: "fixture:analysis".into(),
            fingerprint: "test".into(),
            status: InstallationStatus::Ready,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    probe.actor = ActorRef::SystemRecovery;
    core.execute(probe).await.unwrap();
    let installation = core.bootstrap().await.unwrap().installations.remove(0);
    let executor = ExecutorRef {
        provider_id: ProviderId::parse("analysis-test").unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    };
    let driver = Arc::new(FakeDriver::with_executor(executor.clone()));
    core.register_driver(driver.clone()).await;
    (executor, driver)
}

#[tokio::test]
async fn project_runs_analysis_receipt_prevents_duplicate_jobs_and_binds_payload() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, project) = fixture(root.path());
    std::fs::write(root.path().join("README.md"), "npm run dev").unwrap();
    analysis_driver(&core).await;
    let pause = host.gate.acquire_many(2).await.unwrap();
    let request = request();
    let accepted = run(&host, &core, &project, request.clone()).await.unwrap();
    assert_eq!(
        run(&host, &core, &project, request.clone())
            .await
            .unwrap()
            .generation,
        accepted.generation
    );
    assert_eq!(host.active.load(std::sync::atomic::Ordering::Acquire), 1);
    let mut changed = request.clone();
    changed.expected_generation = accepted.generation.clone();
    assert!(
        run(&host, &core, &project, changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("不同请求")
    );
    let stale = AnalyzeRequest {
        operation_id: uuid::Uuid::new_v4().to_string(),
        ..request.clone()
    };
    assert!(
        run(&host, &core, &project, stale)
            .await
            .unwrap_err()
            .to_string()
            .contains("配置已更新")
    );
    drop(pause);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while host.has_active() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let saved = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert_eq!(
        saved.operations[&request.operation_id].result.status,
        "failed"
    ); // fixture Agent returns no valid command JSON
    let restarted = ProjectRunHost::new(root.path().join("catalogs"));
    assert_eq!(
        run(&restarted, &core, &project, request.clone())
            .await
            .unwrap()
            .generation,
        accepted.generation
    );
    assert!(!restarted.has_active());
    assert_eq!(
        restarted
            .load(project.project_id.as_str())
            .unwrap()
            .unwrap()
            .operations[&request.operation_id]
            .result
            .status,
        "failed"
    );
}
#[tokio::test]
async fn project_runs_restart_recovers_unresolved_receipt_without_relaunch() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, project) = fixture(root.path());
    let request = request();
    let mut saved = host.load(project.project_id.as_str()).unwrap().unwrap();
    saved.generation = "accepted".into();
    saved.directories[0].status = "analyzing".into();
    saved.record(&request.operation_id, serde_json::json!(request), "running");
    host.save(&saved).unwrap();
    let restarted = ProjectRunHost::new(root.path().join("catalogs"));
    let recovered = run(&restarted, &core, &project, request.clone())
        .await
        .unwrap();
    assert_eq!(recovered.generation, "accepted");
    assert_eq!(recovered.directories[0].status, "error");
    assert!(!restarted.has_active());
    assert_eq!(
        restarted
            .load(project.project_id.as_str())
            .unwrap()
            .unwrap()
            .operations[&request.operation_id]
            .result
            .status,
        "interrupted"
    );
}

#[tokio::test]
async fn project_runs_rejections_are_structured_and_do_not_accept_operations() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, project) = fixture(root.path());
    let stale = AnalyzeRequest {
        expected_generation: "stale".into(),
        ..request()
    };
    let error = run(&host, &core, &project, stale.clone())
        .await
        .unwrap_err();
    assert!(matches!(&error, MutationError::Rejected { code, .. } if code == "version_conflict"));
    assert_eq!(serde_json::to_value(&error).unwrap()["kind"], "rejected");
    let saved = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert_eq!(saved.generation, "v1");
    assert!(!saved.operations.contains_key(&stale.operation_id));
    assert!(!host.has_active());

    let mut saved = saved;
    saved.directories[0].commands.push(RunCommand {
        id: "dev".into(),
        label: "Run".into(),
        command: "echo run".into(),
        cwd: ".".into(),
        source: "test".into(),
        pinned: false,
        user_renamed: false,
    });
    host.save(&saved).unwrap();
    let host = ProjectRunHost::new(root.path().join("catalogs"));
    let edit = EditRequest {
        operation_id: uuid::Uuid::new_v4().to_string(),
        task_id: "task".into(),
        generation: "v1".into(),
        directory_index: 0,
        command_id: "dev".into(),
        edit: RunEdit::Rename { label: "\n".into() },
    };
    let execute = || {
        host.edit_recorded(
            project.project_id.as_str(),
            &edit.generation,
            0,
            "dev",
            edit.edit.clone(),
            Some((&edit.operation_id, serde_json::json!(edit))),
        )
    };
    assert!(
        matches!(execute().unwrap_err(), MutationError::Rejected { code, .. } if code == "invalid_request")
    );
    assert_eq!(
        host.load(project.project_id.as_str())
            .unwrap()
            .unwrap()
            .generation,
        "v1"
    );
    // An indeterminate storage write must never be downgraded to a rejection.
    host.uncertain_storage
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        execute().unwrap_err(),
        MutationError::Unknown { .. }
    ));
}

#[tokio::test]
async fn project_runs_accepted_batch_keeps_original_driver_after_disable_and_replacement() {
    use agent_protocol::{AgentPreferences, CommandEnvelope, CoreCommand};
    use driver_api::FakeDriver;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("README.md"), "Run with npm run dev").unwrap();
    let extra = root.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    std::fs::write(extra.join("README.md"), "Run with npm run dev").unwrap();
    let (host, core, mut project) = fixture(root.path());
    project.additional_directories = vec![extra.to_string_lossy().into_owned()];
    let (executor, driver) = analysis_driver(&core).await;
    let pause = host.gate.acquire_many(2).await.unwrap();
    let request = AnalyzeRequest {
        executor: Some(executor.clone()),
        ..request()
    };
    let accepted = run(&host, &core, &project, request.clone()).await.unwrap();
    let mut disable = CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
        disabled: vec!["analysis-test".into()],
        ..Default::default()
    }));
    disable.expected_stream_version = Some(0);
    core.execute(disable).await.unwrap();
    core.select_installation("analysis-test", None).await;
    let replacement = Arc::new(FakeDriver::with_executor(executor));
    core.register_driver(replacement.clone()).await;
    assert_eq!(
        run(&host, &core, &project, request.clone())
            .await
            .unwrap()
            .generation,
        accepted.generation
    );
    drop(pause);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while host.has_active() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(driver.received_requests().await.len(), 2);
    assert!(replacement.received_requests().await.is_empty());
    let next = AnalyzeRequest {
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_generation: accepted.generation,
        ..request
    };
    assert!(matches!(
        run(&host, &core, &project, next).await,
        Err(MutationError::Rejected { .. })
    ));
}

#[tokio::test]
async fn project_runs_no_ready_agent_rejects_before_durable_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    let (host, core, project) = fixture(directory.path());
    let request = request();
    assert!(matches!(run(&host, &core, &project, request.clone()).await,
        Err(MutationError::Rejected { code, .. }) if code == "executor_unavailable"));
    let stored = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert_eq!(stored.generation, "v1");
    assert!(!stored.operations.contains_key(&request.operation_id));
    assert!(!host.has_active());
}

fn accepted_catalog(
    host: &ProjectRunHost,
    project: &ProjectSummary,
) -> (StoredCatalog, AnalyzeRequest) {
    let mut catalog = host.load(project.project_id.as_str()).unwrap().unwrap();
    let request = request();
    catalog.generation = "accepted-analysis".into();
    catalog.directories[0].status = "analyzing".into();
    catalog.directories[0].commands.push(RunCommand {
        id: "saved-command".into(),
        label: "Previously saved".into(),
        command: "echo saved".into(),
        cwd: ".".into(),
        source: "README.md".into(),
        pinned: false,
        user_renamed: false,
    });
    let mut second = catalog.directories[0].clone();
    second.path.push_str("/extra");
    catalog.directories.push(second);
    catalog.record(&request.operation_id, serde_json::json!(request), "running");
    host.save(&catalog).unwrap();
    host.cache
        .lock()
        .unwrap()
        .insert(project.project_id.to_string(), catalog.clone());
    (catalog, request)
}

#[test]
fn project_runs_failed_analysis_save_never_publishes_unsaved_commands_and_recovers_after_repair() {
    let root = tempfile::tempdir().unwrap();
    let (host, _, project) = fixture(root.path());
    let (catalog, request) = accepted_catalog(&host, &project);
    let mut unrelated = catalog.clone();
    unrelated.project_id = "unrelated-project".into();
    unrelated
        .directories
        .iter_mut()
        .for_each(|d| d.status = "ready".into());
    unrelated.operations.clear();
    host.save(&unrelated).unwrap();
    host.cache
        .lock()
        .unwrap()
        .insert("unrelated-project".into(), unrelated);
    let backup = root.path().join("last-durable-catalogs");
    std::fs::rename(&host.directory, &backup).unwrap();
    std::fs::write(&host.directory, b"blocked storage").unwrap();
    host.finish(
        project.project_id.as_str(),
        &catalog.generation,
        0,
        Ok(vec![RunCommand {
            id: "unsaved-command".into(),
            label: "Must never be offered".into(),
            command: "echo unsaved".into(),
            cwd: ".".into(),
            source: "README.md".into(),
            pinned: false,
            user_renamed: false,
        }]),
    );
    assert!(
        host.load(project.project_id.as_str()).is_err(),
        "Reads and launches must wait for the failed finalization to be saved"
    );
    assert_eq!(
        host.cache.lock().unwrap()[project.project_id.as_str()].directories[0].commands[0].id,
        "saved-command"
    );
    assert!(
        host.load("unrelated-project")
            .unwrap()
            .unwrap()
            .directories
            .iter()
            .all(|d| d.status == "ready")
    );
    std::fs::remove_file(&host.directory).unwrap();
    std::fs::rename(&backup, &host.directory).unwrap();
    let recovered = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert!(recovered.directories.iter().all(|d| d.status == "error"));
    assert_eq!(recovered.directories[0].commands[0].id, "saved-command");
    assert_eq!(
        recovered.operations[&request.operation_id].result.status,
        "failed"
    );
    let restarted = ProjectRunHost::new(host.directory.clone());
    let durable = restarted
        .load(project.project_id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(durable).unwrap(),
        serde_json::to_value(recovered).unwrap()
    );
}

async fn wait_finished(host: &ProjectRunHost) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while host.has_active() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn project_runs_closed_queue_settles_every_accepted_directory_and_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, mut project) = fixture(root.path());
    project
        .additional_directories
        .push(root.path().join("extra").to_string_lossy().into_owned());
    let (_, driver) = analysis_driver(&core).await;
    let pause = host.gate.acquire_many(2).await.unwrap();
    let request = request();
    let accepted = run(&host, &core, &project, request.clone()).await.unwrap();
    host.gate.close();
    drop(pause);
    wait_finished(&host).await;
    let saved = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert!(saved.directories.iter().all(|d| d.status == "error"));
    assert_eq!(
        saved.operations[&request.operation_id].result.status,
        "interrupted"
    );
    assert!(driver.received_requests().await.is_empty());
    let restarted = ProjectRunHost::new(host.directory.clone());
    assert_eq!(
        run(&restarted, &core, &project, request)
            .await
            .unwrap()
            .generation,
        accepted.generation
    );
    assert!(!restarted.has_active());
}

#[tokio::test]
async fn project_runs_storage_failure_stops_remaining_agent_calls_in_the_batch() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, mut project) = fixture(root.path());
    std::fs::write(root.path().join("README.md"), "npm run dev").unwrap();
    let extra = root.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    std::fs::write(extra.join("README.md"), "npm run dev").unwrap();
    project
        .additional_directories
        .push(extra.to_string_lossy().into_owned());
    let (_, driver) = analysis_driver(&core).await;
    let pause = host.gate.acquire_many(2).await.unwrap();
    let request = request();
    run(&host, &core, &project, request.clone()).await.unwrap();
    let backup = root.path().join("last-durable-catalogs");
    std::fs::rename(&host.directory, &backup).unwrap();
    std::fs::write(&host.directory, b"blocked storage").unwrap();
    drop(pause);
    wait_finished(&host).await;
    assert_eq!(
        driver.received_requests().await.len(),
        1,
        "Do not invoke another Agent after a result cannot be saved"
    );
    assert!(host.load(project.project_id.as_str()).is_err());
    std::fs::remove_file(&host.directory).unwrap();
    std::fs::rename(&backup, &host.directory).unwrap();
    let saved = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert!(saved.directories.iter().all(|d| d.status == "error"));
    assert_eq!(
        saved.operations[&request.operation_id].result.status,
        "failed"
    );
    assert_eq!(driver.received_requests().await.len(), 1);
}

#[tokio::test]
async fn project_runs_aborted_or_panicking_worker_persists_interruption_before_becoming_idle() {
    for panic in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (host, _, project) = fixture(root.path());
        let (catalog, request) = accepted_catalog(&host, &project);
        let running = ActiveAnalysis::new(
            host.clone(),
            project.project_id.to_string(),
            catalog.generation.clone(),
        );
        let (started, ready) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let _running = running;
            started.send(()).unwrap();
            assert!(!panic, "synthetic worker panic");
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        if !panic {
            worker.abort();
        }
        let error = worker.await.unwrap_err();
        assert_eq!(error.is_panic(), panic);
        assert!(!host.has_active());
        let raw: StoredCatalog =
            serde_json::from_slice(&std::fs::read(host.file(project.project_id.as_str())).unwrap())
                .unwrap();
        assert!(raw.directories.iter().all(|d| d.status == "error"));
        assert_eq!(
            raw.operations[&request.operation_id].result.status,
            "interrupted"
        );
        assert_eq!(raw.directories[0].commands[0].id, "saved-command");
    }
}

#[test]
fn project_runs_old_worker_cleanup_cannot_interrupt_its_replacement() {
    let root = tempfile::tempdir().unwrap();
    let (host, _, project) = fixture(root.path());
    let (mut catalog, old_request) = accepted_catalog(&host, &project);
    let old = ActiveAnalysis::new(
        host.clone(),
        project.project_id.to_string(),
        catalog.generation.clone(),
    );
    catalog
        .operations
        .get_mut(&old_request.operation_id)
        .unwrap()
        .result
        .status = "superseded".into();
    catalog.generation = "replacement".into();
    let replacement_request = request();
    catalog.record(
        &replacement_request.operation_id,
        serde_json::json!(replacement_request),
        "running",
    );
    host.save(&catalog).unwrap();
    host.cache
        .lock()
        .unwrap()
        .insert(project.project_id.to_string(), catalog);
    let replacement = ActiveAnalysis::new(
        host.clone(),
        project.project_id.to_string(),
        "replacement".into(),
    );
    drop(old);
    assert!(host.has_active());
    let current = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert_eq!(current.generation, "replacement");
    assert!(current.directories.iter().all(|d| d.status == "analyzing"));
    assert_eq!(
        current.operations[&replacement_request.operation_id]
            .result
            .status,
        "running"
    );
    drop(replacement);
    assert!(!host.has_active());
}

#[test]
fn project_runs_indeterminate_commit_blocks_additional_writes_until_restart() {
    let root = tempfile::tempdir().unwrap();
    let (host, _, project) = fixture(root.path());
    let (mut catalog, request) = accepted_catalog(&host, &project);
    let before = std::fs::read(host.file(project.project_id.as_str())).unwrap();
    host.uncertain_storage
        .store(true, std::sync::atomic::Ordering::Release);
    catalog.directories[0].commands[0].label = "Must not overwrite an uncertain commit".into();
    assert!(host.save(&catalog).is_err());
    assert_eq!(
        std::fs::read(host.file(project.project_id.as_str())).unwrap(),
        before
    );
    assert!(host.load(project.project_id.as_str()).is_err());
    let restarted = ProjectRunHost::new(host.directory.clone());
    let recovered = restarted
        .load(project.project_id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.directories[0].commands[0].label,
        "Previously saved"
    );
    assert_eq!(
        recovered.operations[&request.operation_id].result.status,
        "interrupted"
    );
    let durable: StoredCatalog = serde_json::from_slice(
        &std::fs::read(restarted.file(project.project_id.as_str())).unwrap(),
    )
    .unwrap();
    assert_eq!(
        durable.operations[&request.operation_id].result.status,
        "interrupted"
    );
}

#[tokio::test]
async fn project_runs_pending_native_cleanup_blocks_new_analysis_but_not_original_receipt() {
    use agent_core::AnalysisLifetime;
    let root = tempfile::tempdir().unwrap();
    let (host, core, project) = fixture(root.path());
    let (_, driver) = analysis_driver(&core).await;
    let (catalog, original) = accepted_catalog(&host, &project);
    let lifetime = Arc::new(ActiveAnalysis::new(
        host.clone(),
        project.project_id.to_string(),
        catalog.generation.clone(),
    ));
    lifetime.cleanup_pending("fixture: native close unconfirmed");
    let pending = host.load(project.project_id.as_str()).unwrap().unwrap();
    assert_eq!(
        pending.operations[&original.operation_id].result.status,
        "interrupted"
    );
    assert!(pending.directories.iter().all(|d| d.status == "error"));
    assert!(host.has_active());
    assert_eq!(
        run(&host, &core, &project, original)
            .await
            .unwrap()
            .generation,
        catalog.generation
    );
    let retry = AnalyzeRequest {
        expected_generation: catalog.generation.clone(),
        ..request()
    };
    assert!(matches!(run(&host, &core, &project, retry.clone()).await,
        Err(MutationError::Rejected { code, .. }) if code == "analysis_cleanup_pending"));
    assert!(driver.received_requests().await.is_empty());
    drop(lifetime); // The original native cleanup confirms shutdown.
    assert!(!host.has_active());
    assert_ne!(
        run(&host, &core, &project, retry).await.unwrap().generation,
        catalog.generation
    );
    wait_finished(&host).await;
}

struct CloseFailureDriver {
    inner: Arc<driver_api::FakeDriver>,
    fail: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl driver_api::AgentDriver for CloseFailureDriver {
    fn executor(&self) -> &ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "fixture"
    }
    async fn create_session(
        &self,
        request: driver_api::SessionRequest,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        self.inner.create_session(request).await
    }
    async fn create_analysis_session(
        &self,
        directory: &str,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        self.inner.create_analysis_session(directory).await
    }
    async fn prompt(
        &self,
        handle: &driver_api::SessionHandle,
        request: driver_api::PromptRequest,
    ) -> Result<driver_api::DriverEventStream, driver_api::DriverError> {
        self.inner.prompt(handle, request).await
    }
    async fn interrupt(
        &self,
        handle: &driver_api::SessionHandle,
    ) -> Result<(), driver_api::DriverError> {
        self.inner.interrupt(handle).await
    }
    async fn shutdown(
        &self,
        handle: driver_api::SessionHandle,
    ) -> Result<(), driver_api::DriverError> {
        if self.fail.load(std::sync::atomic::Ordering::Acquire) {
            return Err(driver_api::DriverError::Disconnected(
                "fixture: close unconfirmed".into(),
            ));
        }
        self.inner.shutdown(handle).await
    }
}

#[tokio::test]
async fn project_runs_actual_native_close_failure_halts_batch_and_prevents_idle_retirement() {
    let root = tempfile::tempdir().unwrap();
    let (host, core, mut project) = fixture(root.path());
    std::fs::write(root.path().join("README.md"), "npm run dev").unwrap();
    let extra = root.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    std::fs::write(extra.join("README.md"), "npm run dev").unwrap();
    project
        .additional_directories
        .push(extra.to_string_lossy().into_owned());
    let (_, inner) = analysis_driver(&core).await;
    let driver = Arc::new(CloseFailureDriver {
        inner,
        fail: std::sync::atomic::AtomicBool::new(true),
    });
    core.register_driver(driver.clone()).await;
    let request = request();
    let accepted = run(&host, &core, &project, request.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let current = host.load(project.project_id.as_str()).unwrap().unwrap();
            if current.operations[&request.operation_id].result.status == "interrupted" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        host.has_active(),
        "The exact native cleanup still owns the service retirement lease"
    );
    assert_eq!(driver.inner.received_requests().await.len(), 1);
    let next = AnalyzeRequest {
        expected_generation: accepted.generation.clone(),
        ..self::request()
    };
    assert!(matches!(run(&host, &core, &project, next).await,
        Err(MutationError::Rejected { code, .. }) if code == "analysis_cleanup_pending"));
    driver
        .fail
        .store(false, std::sync::atomic::Ordering::Release);
    wait_finished(&host).await;
    assert_eq!(driver.inner.received_requests().await.len(), 1);
    let restarted = ProjectRunHost::new(host.directory.clone());
    let receipt = restarted
        .load(project.project_id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.operations[&request.operation_id].result.status,
        "interrupted"
    );
    assert!(receipt.directories.iter().all(|d| d.status == "error"));
}
