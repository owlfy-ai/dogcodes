use acp_driver::{AcpDriver, AcpLaunch};
use driver_api::{
    AgentDriver, ApprovalSemanticRole, DriverEvent, PromptRequest, SessionHandle, SessionRequest,
};
use futures::StreamExt;
async fn run(
    driver: &AcpDriver,
    session: &SessionHandle,
    cwd: &str,
    prompt: &str,
) -> (String, usize, usize) {
    let mut stream = driver
        .prompt(
            session,
            PromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd.into(),
                context: vec![],
                prompt: prompt.into(),
            },
        )
        .await
        .unwrap();
    let mut text = String::new();
    let mut children = 0;
    let mut steps = 0;
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            DriverEvent::Presentation { data, .. } => {
                for key in ["text", "textDelta"] {
                    if let Some(t) = data[key].as_str() {
                        text.push_str(t);
                    }
                }
                if data["type"] == "subagent" {
                    children += 1;
                }
                if data["parentId"].as_str().is_some_and(|s| !s.is_empty()) {
                    steps += 1;
                }
            }
            DriverEvent::TextDelta { text: t, .. } => text.push_str(&t),
            DriverEvent::ApprovalRequested {
                native_request_id,
                tool_name,
                options,
                ..
            } => {
                let safe = matches!(tool_name.to_lowercase().as_str(), "agent" | "task" | "read");
                let role = if safe {
                    ApprovalSemanticRole::ApproveOnce
                } else {
                    ApprovalSemanticRole::Reject
                };
                let choice = options
                    .iter()
                    .find(|o| o.semantic_role == role)
                    .expect("native option");
                driver
                    .respond_to_interaction(session, &native_request_id, &choice.option_id)
                    .await
                    .unwrap();
            }
            DriverEvent::TurnCompleted { .. } => completed = true,
            _ => {}
        }
    }
    assert!(completed, "turn did not complete");
    (text, children, steps)
}
#[tokio::test]
#[ignore = "requires signed-in CodeBuddy; real resume and read-only delegation in a temporary directory"]
async fn codebuddy_resume_and_child_activity() {
    let launch = AcpLaunch {
        executable: std::env::var("DOGCODES_ACP_BINARY").unwrap().into(),
        args: vec!["--acp".into()],
    };
    let report = AcpDriver::probe_ready(&launch).await.unwrap();
    let make = || {
        AcpDriver::new(
            "codebuddy",
            &agent_domain::InstallationId::new().to_string(),
            launch.clone(),
            report.native_version.clone(),
            report.resume_capability,
        )
        .unwrap()
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fixture.txt"), "CHILD_RECORD_842\n").unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let request = || SessionRequest {
        history_mcp: None,
        permission_mode: None,
        additional_directories: vec![],
        working_directory: cwd.clone(),
    };
    let driver = make();
    let session = driver.create_session(request()).await.unwrap();
    let first=tokio::time::timeout(std::time::Duration::from_mins(2),run(&driver,&session,&cwd,"Remember the token CODEBUDDY_RESUME_731 for the next message. Reply only OK. Do not use tools.")).await;
    driver.shutdown(session.clone()).await.unwrap();
    first.unwrap();
    drop(driver);
    let driver = make();
    let session = driver
        .resume_selected_session(&session.native_session_id, request(), None, None)
        .await
        .unwrap();
    let result=tokio::time::timeout(std::time::Duration::from_mins(3),run(&driver,&session,&cwd,"Report the exact token I gave you in the previous message. Also use the Agent tool to delegate reading fixture.txt in this directory to exactly one subagent; include its file contents in your reply. Do not use shell commands, change files, or access other directories.")).await;
    driver.shutdown(session).await.unwrap();
    let (text, children, steps) = result.unwrap();
    assert!(
        text.contains("CODEBUDDY_RESUME_731"),
        "resume lost prior context"
    );
    assert!(
        text.contains("CHILD_RECORD_842"),
        "child fixture result missing"
    );
    assert!(children > 0, "no native child activity observed");
    println!(
        "resume identity/context verified; child events={children}; correlated internal events={steps}"
    );
}
