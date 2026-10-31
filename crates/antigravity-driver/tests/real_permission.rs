use antigravity_driver::AntigravityDriver;
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;

#[tokio::test]
#[ignore = "consumes one real model turn in a temporary workspace; requires authorization"]
async fn real_headless_write_permission_observation() {
    let executable =
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/agy");
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_string_lossy().into_owned();
    let driver = AntigravityDriver::new(
        "antigravity",
        &agent_domain::InstallationId::new().to_string(),
        executable,
        None,
        SessionResumeCapability::Unknown,
    )
    .unwrap();
    let session = driver
        .create_session(SessionRequest {
            app_mcps: vec![],
            permission_mode: None,
            additional_directories: vec![],
            working_directory: cwd.clone(),
        })
        .await
        .unwrap();
    let mut stream = driver.prompt(&session,PromptRequest {
                attachments: vec![],
        permission_mode: None,
        additional_directories: vec![],
        prompt:"Permission integration test. In the current directory only, use a shell tool to run exactly: printf DOGCODES_APPROVAL_OK > approval.txt . Do not access any other paths or change permission settings. If permission is denied or unavailable, stop and explain.".into(),
        working_directory:cwd,context:vec![],
    }).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let mut completed = false;
        let mut failed = false;
        let mut approvals = 0;
        let mut tool_events = 0;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                DriverEvent::TurnCompleted { .. } => completed = true,
                DriverEvent::TurnFailed { .. } | DriverEvent::TurnQuotaExhausted { .. } => {
                    failed = true;
                }
                DriverEvent::ApprovalRequested { .. } => {
                    approvals += 1;
                    driver.interrupt(&session).await.unwrap();
                }
                DriverEvent::ToolStarted { .. } => tool_events += 1,
                _ => {}
            }
        }
        (completed, failed, approvals, tool_events)
    })
    .await;
    driver.shutdown(session).await.unwrap();
    let (completed, failed, approvals, tool_events) =
        result.expect("permission observation timed out");
    let written = std::fs::read_to_string(workspace.path().join("approval.txt"))
        .is_ok_and(|text| text == "DOGCODES_APPROVAL_OK");
    println!(
        "completed={completed}, failed={failed}, approvals={approvals}, tool_events={tool_events}, marker_written={written}"
    );
    assert!(
        completed && !failed,
        "runtime failed; permission behavior not verified"
    );
    assert!(
        tool_events > 0 || approvals > 0,
        "no observable tool attempt; permission behavior not verified"
    );
}
