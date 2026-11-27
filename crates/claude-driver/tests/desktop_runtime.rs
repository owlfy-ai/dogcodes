//! Explicit, local acceptance only: one real no-tool model turn in a temporary workspace.
use agent_domain::InstallationId;
use claude_driver::ClaudeDriver;
use driver_api::{AgentDriver, DriverEvent, PromptRequest, QuotaProvider, SessionResumeCapability};
use futures::StreamExt;
use std::{path::PathBuf, time::Duration};

#[tokio::test]
#[ignore = "requires desktop runtime/login and explicit authorization for one real model turn"]
async fn desktop_runtime_conversation_and_quota() {
    let executable = PathBuf::from(
        std::env::var_os("DOGCODES_TEST_CLAUDE_EXECUTABLE").expect("desktop executable"),
    );
    let version = std::env::var("DOGCODES_TEST_CLAUDE_VERSION").expect("verified runtime version");
    assert!(
        executable
            .ancestors()
            .any(|p| p.ends_with("Claude/claude-code")
                || (p.ends_with("claude-code")
                    && p.parent()
                        .and_then(std::path::Path::parent)
                        .is_some_and(|data| data.ends_with("Claude-Data")))),
        "must explicitly test the desktop-managed runtime"
    );
    let workspace = tempfile::tempdir().unwrap();
    let driver = ClaudeDriver::connect_ready(
        "claude-code",
        &InstallationId::new().to_string(),
        &executable,
        Some(version.clone()),
        SessionResumeCapability::Supported,
    )
    .await
    .expect("desktop initialize must succeed");
    println!("desktop runtime {version}: initialize OK");
    let quota = driver.read_quota_with_refresh(None, true).await;
    if let Ok(quota) = &quota {
        println!(
            "desktop runtime: {} quota windows, source={}",
            quota.windows.len(),
            quota.source
        );
    }
    let session = driver
        .create_analysis_session(workspace.path().to_str().unwrap())
        .await
        .expect("isolated no-MCP/no-hooks session");
    let result = tokio::time::timeout(Duration::from_mins(3), async {
        let mut events = driver.prompt(&session, PromptRequest {
            working_directory: workspace.path().to_string_lossy().into_owned(),
            prompt: "Reply with exactly: DOGCODES_DESKTOP_RUNTIME_OK. Do not use tools or inspect files.".into(),
            permission_mode: Some("plan".into()),
            additional_directories: vec![], attachments: vec![], context: vec![],
        }).await?;
        let mut text = String::new();
        let mut complete = false;
        while let Some(event) = events.next().await {
            match event? {
                DriverEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                DriverEvent::TurnCompleted { .. } => complete = true,
                DriverEvent::TurnFailed { reason } => return Err(driver_api::DriverError::Protocol(reason)),
                DriverEvent::ToolStarted { .. } | DriverEvent::ApprovalRequested { .. } => return Err(driver_api::DriverError::Protocol("acceptance prompt unexpectedly requested a tool".into())),
                _ => {}
            }
        }
        Ok::<_, driver_api::DriverError>((complete, text))
    }).await;
    driver
        .shutdown(session)
        .await
        .expect("desktop session cleanup");
    let (complete, text) = result
        .expect("desktop turn timeout")
        .expect("desktop model turn");
    assert!(complete, "desktop turn must finish");
    assert_eq!(text.trim(), "DOGCODES_DESKTOP_RUNTIME_OK");
    println!("desktop runtime: conversation OK, no tools requested");
    assert!(
        quota.is_ok(),
        "desktop native quota must succeed: {quota:?}"
    );
}
