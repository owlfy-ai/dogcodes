use antigravity_driver::{AntigravityDriver, approval_bridge};
use driver_api::{
    AgentDriver, DriverEvent, PromptRequest, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;
fn main() {
    if approval_bridge::run_hook_if_requested() {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(run());
}
async fn run() {
    let mut passed = true;
    let workspace = tempfile::tempdir().unwrap();
    for allow in [false, true] {
        let started = std::time::Instant::now();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let command = format!(
            "printf DOGCODES_APPROVAL_OK > '{}'",
            workspace.path().join("approval.txt").display()
        );
        let prompt = format!(
            "Approval integration test. Use run_command to run exactly: {command} . Do not use other tools or access any other paths. If denied, stop immediately and report denied."
        );
        let driver = AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/agy"),
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: Some("desktop-approvals".into()),
                working_directory: cwd.clone(),
                additional_directories: vec![],
            })
            .await
            .expect("create approval session");
        println!(
            "approval probe session_ms={}",
            started.elapsed().as_millis()
        );
        let mut stream = driver
            .prompt(
                &session,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: Some("desktop-approvals".into()),
                    working_directory: cwd.clone(),
                    additional_directories: vec![],
                    context: vec![],
                    prompt,
                },
            )
            .await
            .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_mins(2), async {
            let mut approvals = 0;
            let mut approved = 0;
            let mut completed = false;
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    DriverEvent::ApprovalRequested {
                        native_request_id,
                        command_summary,
                        ..
                    } => {
                        approvals += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        assert!(
                            !workspace.path().join("approval.txt").exists(),
                            "tool ran before approval"
                        );
                        let raw: serde_json::Value =
                            serde_json::from_str(&command_summary).unwrap_or_default();
                        let requested_command = raw["command"]
                            .as_str()
                            .or_else(|| raw["CommandLine"].as_str())
                            .unwrap_or("");
                        let safe = requested_command.trim() == command
                            && raw["cwd"].as_str().is_none_or(|v| v == cwd);
                        let option = if allow && safe {
                            approved += 1;
                            "allow_once"
                        } else {
                            "deny"
                        };
                        println!(
                            "antigravity approval: allow={allow}, safe={safe}, response={option}"
                        );
                        driver
                            .respond_to_interaction(&session, &native_request_id, option)
                            .await
                            .unwrap();
                    }
                    DriverEvent::TurnCompleted { .. } => completed = true,
                    DriverEvent::TurnFailed { .. } | DriverEvent::TurnQuotaExhausted { .. } => {
                        println!("antigravity terminal failure");
                    }
                    event => println!("event: {event:?}"),
                }
            }
            (approvals, approved, completed)
        })
        .await;
        if result.is_err() {
            let _ = driver.interrupt(&session).await;
        }
        driver.shutdown(session).await.unwrap();
        let (approvals, approved, completed) = result.expect("approval timeout");
        let file = workspace.path().join("approval.txt");
        println!(
            "antigravity allow={allow}, approvals={approvals}, approved={approved}, completed={completed}, file={}",
            file.exists()
        );
        let marker = std::fs::read_to_string(&file).ok();
        println!(
            "marker_matches={}",
            marker.as_deref() == Some("DOGCODES_APPROVAL_OK")
        );
        passed &= approvals > 0
            && if allow {
                completed && approved > 0 && marker.as_deref() == Some("DOGCODES_APPROVAL_OK")
            } else {
                !file.exists()
            };
    }
    assert!(passed, "Antigravity approval gate failed");
}
