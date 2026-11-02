//! Opt-in native acceptance; edits only a disposable fixture workspace.
use acp_driver::{AcpDriver, AcpLaunch};
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;
#[tokio::test]
#[ignore = "requires signed-in Cursor; runs one real edit in a temporary directory"]
async fn cursor_reports_confirmed_native_diff() {
    let executable = std::env::var("DOGCODES_ACP_BINARY").expect("DOGCODES_ACP_BINARY");
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("fixture.txt"),
        "first\nbefore\nlast\n",
    )
    .unwrap();
    let cwd = directory.path().to_string_lossy().into_owned();
    let driver = AcpDriver::new(
        "cursor",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: executable.into(),
            args: vec!["acp".into()],
        },
        None,
        SessionResumeCapability::Unknown,
    )
    .unwrap();
    let session = driver
        .create_selected_session(
            SessionRequest {
                app_mcps: vec![],
                working_directory: cwd.clone(),
                additional_directories: vec![],
                permission_mode: Some("full-access".into()),
            },
            None,
            None,
        )
        .await
        .unwrap();
    let run = async {
        let mut stream = driver.prompt(&session, PromptRequest { attachments: vec![], working_directory: cwd, additional_directories: vec![], permission_mode: Some("full-access".into()), context: vec![], prompt: "Use your native file edit tool to change exactly the line before to after in fixture.txt. Preserve the other two lines and final newline. Do not use a shell, do not modify other files, and do not delegate. Then say done.".into() }).await.unwrap();
        let mut patches = vec![];
        let mut completed = false;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                DriverEvent::Presentation { data, .. } => {
                    if let Some(changes) = data["changes"].as_array() {
                        patches.extend(changes.clone());
                    }
                }
                DriverEvent::TurnCompleted { .. } => completed = true,
                DriverEvent::ApprovalRequested {
                    native_request_id,
                    options,
                    ..
                } => {
                    // Unexpected interaction is declined, never broadly auto-approved.
                    let deny = options
                        .iter()
                        .find(|o| o.semantic_role == driver_api::ApprovalSemanticRole::Reject)
                        .unwrap();
                    driver
                        .respond_to_interaction(&session, &native_request_id, &deny.option_id)
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
        assert!(completed);
        assert_eq!(
            std::fs::read_to_string(directory.path().join("fixture.txt")).unwrap(),
            "first\nafter\nlast\n"
        );
        assert_eq!(patches.len(), 1, "expected one confirmed native diff");
        let patch = patches[0]["diff"].as_str().unwrap();
        assert!(
            patch.contains("-before\n+after\n"),
            "unexpected patch: {patch}"
        );
        println!("native edit and exactly one confirmed Diff verified");
    };
    let result = tokio::time::timeout(std::time::Duration::from_mins(3), run).await;
    driver.shutdown(session).await.unwrap();
    result.unwrap();
}
