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
