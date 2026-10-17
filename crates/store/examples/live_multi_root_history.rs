//! Explicit opt-in real provider acceptance. Uses an isolated temporary project/database.
#[path = "support/installation.rs"]
mod installation;
use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::{
    CommandEnvelope, ConfirmContextPackage, ContextPackageKind, ContextPreviewRequest,
    ContextStrength, CoreCommand, CreateTask, RegisterProject, SubmitTurn,
};
use agent_store::SqliteStore;
use driver_api::AgentDriver;
use std::{path::Path, sync::Arc, time::Duration};

async fn turn(
    core: &Arc<Core>,
    task: &TaskId,
    prompt: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let start = snapshot.last_sequence;
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        permission_mode: None,
        thinking: None,
        task_id: task.clone(),
        prompt: prompt.into(),
        model: None,
        executor_override: None,
    }));
    command.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(command).await?;
    tokio::time::timeout(Duration::from_mins(10), async {
        loop {
            let events = core.sync_events(start, 10_000).await?;
            if let Some(failed) = events.iter().find(|e| e.event_type == "turn.failed") {
                return Err(format!("turn failed: {}", failed.payload).into());
            }
            for approval in core.pending_approvals(task, 10).await? {
                println!("REVIEW APPROVAL {}", serde_json::to_string(&approval)?);
                println!("Enter an exact optionId to respond; empty input aborts acceptance.");
                let answer = tokio::task::spawn_blocking(|| {
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer).map(|_| answer)
                })
                .await??;
                let option = answer.trim();
                if !approval.options.iter().any(|o| o.option_id == option) {
                    return Err("approval not selected".into());
                }
                let mut command = CommandEnvelope::local(CoreCommand::RespondToApproval(
                    agent_protocol::RespondToApproval {
                        turn_id: approval.turn_id.clone(),
                        requested_at_sequence: approval.requested_at_sequence,
                        answer: None,
                        task_id: task.clone(),
                        approval_id: approval.approval_id,
                        option_id: option.into(),
                    },
                ));
                command.expected_stream_version =
                    Some(core.bootstrap().await?.tasks[0].stream_version);
                core.execute(command).await?;
            }
            if events.iter().any(|e| e.event_type == "turn.completed") {
                return Ok(events
                    .iter()
                    .filter(|e| e.event_type == "message.delta")
                    .filter_map(|e| e.payload["text"].as_str())
                    .collect::<String>());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?
}

async fn switch(
    core: &Arc<Core>,
    task: &TaskId,
    source: ExecutorRef,
    target: ExecutorRef,
    kind: ContextPackageKind,
    after: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let request = ContextPreviewRequest {
        task_id: task.clone(),
        kind,
        strength: ContextStrength::Standard,
        source_executor: source,
        target_executor: target,
        after_sequence: after,
        through_sequence: snapshot.last_sequence,
    };
    let built = core.build_context_preview(request.clone()).await?;
    assert!(!built.package.conversation_text.is_empty());
    let mut command =
        CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
            request,
            expected_payload_sha256: built.payload_sha256,
            edited_package: None,
        }));
    command.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(command).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if agent_core::history_bridge::run_query_if_requested() {
        return Ok(());
    }
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: live_relay ABSOLUTE_CODEX_PATH ABSOLUTE_KIMI_PATH (makes three real model turns)".into());
    }
    let directory = tempfile::tempdir()?;
    let workspace = directory.path().join("project");
    std::fs::create_dir(&workspace)?;
    let secondary = directory.path().join("secondary");
    std::fs::create_dir(&secondary)?;
    let store = Arc::new(SqliteStore::open(directory.path().join("acceptance.db"))?);
    let core = Arc::new(
        Core::with_workspace_context(
            store,
            Arc::new(
                agent_workspace::GitWorkspaceContextProvider::discover()
                    .ok_or("git unavailable")?,
            ),
        )
        .with_history_query_program(std::env::current_exe()?),
    );
    let version = std::process::Command::new(&args[1])
        .arg("--version")
        .output()?;
    if !version.status.success() {
        return Err("Codex version probe failed".into());
    }
    let version = String::from_utf8(version.stdout)?;
    let version = version
        .trim()
        .strip_prefix("codex-cli ")
        .ok_or("unexpected Codex version output")?;
    let a = Arc::new(codex_driver::CodexDriver::new(
        Path::new(&args[1]),
        installation::identity(&core, "codex").await?,
        version.into(),
    )?);
    let launch = acp_driver::AcpLaunch {
        executable: args[2].clone().into(),
        args: vec!["acp".into()],
    };
    let report = acp_driver::AcpDriver::probe_ready(&launch).await?;
    let b = Arc::new(acp_driver::AcpDriver::new(
        "kimi",
        &installation::identity(&core, "kimi").await?,
        launch,
        report.native_version,
        report.resume_capability,
    )?);
    let ae = a.executor().clone();
    let be = b.executor().clone();
    installation::register(&core, a).await?;
    installation::register(&core, b).await?;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            additional_directories: vec![secondary.to_string_lossy().into_owned()],
            project_id: project.clone(),
            display_name: "Live relay acceptance".into(),
            path: workspace.to_string_lossy().into_owned(),
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "Live relay acceptance".into(),
            preferred_executor: ae.clone(),
        },
    )))
    .await?;
    let first_prompt = format!(
        "Acceptance test in two isolated temporary project directories. Create same.txt in EACH project directory: primary file exactly ORCHID-731 followed by newline, secondary file exactly AMBER-419 followed by newline. Use normal tools and report denied access; never bypass approval. Reply ACK after writing both. Primary: {} Secondary: {}",
        workspace.display(),
        secondary.display()
    );
    turn(&core, &task, &first_prompt).await?;
    assert_eq!(
        std::fs::read_to_string(workspace.join("same.txt"))?,
        "ORCHID-731\n"
    );
    assert_eq!(
        std::fs::read_to_string(secondary.join("same.txt"))?,
        "AMBER-419\n"
    );
    let checkpoint = core
        .bootstrap()
        .await?
        .session_maps
        .into_iter()
        .find(|s| s.executor == ae)
        .ok_or("missing A session")?
        .last_seen_sequence;
    switch(
        &core,
        &task,
        ae.clone(),
        be.clone(),
        ContextPackageKind::Handoff,
        0,
    )
    .await?;
    let _second = turn(&core, &task, "Acceptance: use the supplied dogcodes read-only history MCP tools to call getChangesSinceCheckpoint, then getArtifact for the previous turn's changes. Do not read workspace files to discover their previous contents. Report the marker recorded for the SECONDARY directory. Then replace same.txt in the secondary directory with COBALT-842 followed by newline, leave primary untouched. Use normal approvals; do not bypass restrictions.").await?;
    let evidence = core.sync_events(0, 10_000).await?;
    assert!(
        evidence
            .iter()
            .filter(|e| e.event_type == "activity.updated")
            .any(|e| {
                let body = e.payload.to_string();
                body.contains("AMBER-419") && body.contains("representation")
            }),
        "B did not receive the secondary marker from the artifact query output"
    );
    assert_eq!(
        std::fs::read_to_string(secondary.join("same.txt"))?,
        "COBALT-842\n"
    );
    switch(
        &core,
        &task,
        be,
        ae,
        ContextPackageKind::Reconciliation,
        checkpoint,
    )
    .await?;
    let third = turn(&core, &task, "Do not read workspace files. From the supplied historical cross-directory changes, state the new marker written in the secondary same.txt while you were away, and the unchanged primary marker.").await?;
    assert!(
        third.contains("COBALT-842") && third.contains("ORCHID-731"),
        "A missed B directory changes: {third}"
    );
    println!(
        "PASS real two-root Codex → Kimi → Codex: files, secondary artifact lookup and reconciliation verified."
    );
    Ok(())
}
