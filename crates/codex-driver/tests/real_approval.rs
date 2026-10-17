use codex_driver::CodexDriver;
use driver_api::{AgentDriver, DriverEvent, PromptRequest, SessionRequest};
use tokio_stream::StreamExt;

fn is_expected_command(command_summary: &str) -> bool {
    fn exact(command: &str) -> bool {
        matches!(
            command.trim(),
            "printf DOGCODES_APPROVAL_OK > approval.txt"
                | "printf 'DOGCODES_APPROVAL_OK' > approval.txt"
                | "printf '%s' 'DOGCODES_APPROVAL_OK' > approval.txt"
                | "printf %s DOGCODES_APPROVAL_OK > approval.txt"
                | "/bin/zsh -lc 'printf DOGCODES_APPROVAL_OK > approval.txt'"
                | "/bin/zsh -lc \"printf DOGCODES_APPROVAL_OK > approval.txt\""
        )
    }

    match serde_json::from_str::<serde_json::Value>(command_summary) {
        Ok(serde_json::Value::String(command)) => exact(&command),
        Ok(serde_json::Value::Array(arguments)) => {
            let arguments = arguments
                .iter()
                .map(serde_json::Value::as_str)
                .collect::<Option<Vec<_>>>();
            arguments.is_some_and(|arguments| {
                (arguments.len() == 3
                    && matches!(arguments[0], "/bin/zsh" | "/bin/bash" | "zsh" | "bash")
                    && arguments[1] == "-lc"
                    && exact(arguments[2]))
                    || (arguments.len() == 1 && exact(arguments[0]))
            })
        }
        _ => false,
    }
}

#[tokio::test]
#[ignore = "consumes two real model turns; explicit operator authorization required"]
async fn real_approval_deny_then_allow_only_in_temporary_workspace() {
    let executable = std::path::Path::new("/Applications/ChatGPT.app/Contents/Resources/codex");
    let version = tokio::process::Command::new(executable)
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout)
        .unwrap()
        .trim()
        .strip_prefix("codex-cli ")
        .unwrap()
        .to_owned();
    for option in ["decline", "allow"] {
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let driver =
            CodexDriver::new(executable, "ins_acceptance".into(), version.clone()).unwrap();
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd.clone(),
            })
            .await
            .unwrap();
        let mut stream = driver.prompt(&session, PromptRequest {
                        attachments: vec![],
            permission_mode: None,
            additional_directories: vec![],
            working_directory:cwd,
            prompt: "Approval integration test. Use a shell tool to run exactly: printf DOGCODES_APPROVAL_OK > approval.txt . Request approval if required. Do not use apply_patch or other tools. If denied, stop immediately and report denied. Do not access any other paths.".into(),
            context:vec![],
        }).await.unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut approvals = 0;
            let mut allowed = 0;
            let mut completed = false;
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    DriverEvent::ApprovalRequested {
                        native_request_id,
                        command_summary,
                        tool_name,
                        ..
                    } => {
                        approvals += 1;
                        let safe = tool_name == "commandExecution"
                            && is_expected_command(&command_summary);
                        let decision = if option == "allow" && !safe {
                            let shape = serde_json::from_str::<serde_json::Value>(&command_summary)
                                .map_or("invalid", |value| match value {
                                    serde_json::Value::String(_) => "string",
                                    serde_json::Value::Array(_) => "array",
                                    serde_json::Value::Object(_) => "object",
                                    _ => "scalar",
                                });
                            println!(
                                "allow fixture did not match: tool={tool_name}, shape={shape}, bytes={}",
                                command_summary.len()
                            );
                            "decline"
                        } else {
                            if option == "allow" {
                                allowed += 1;
                            }
                            option
                        };
                        driver
                            .respond_to_interaction(&session, &native_request_id, decision)
                            .await
                            .unwrap();
                    }
                    DriverEvent::TurnCompleted { .. } => completed = true,
                    DriverEvent::TurnFailed { .. }
                    | DriverEvent::TurnQuotaExhausted { .. } => {
                        return (approvals, allowed, false);
                    }
                    _ => {}
                }
            }
            (approvals, allowed, completed)
        })
        .await;
        driver.shutdown(session).await.unwrap();
        let (approvals, allowed, completed) = outcome.expect("approval turn timed out");
        assert!(approvals > 0, "approval was not requested");
        assert!(completed, "approval turn did not complete");
        let target = workspace.path().join("approval.txt");
        if option == "allow" {
            assert_eq!(allowed, 1, "safe allow was not sent to Codex");
            assert_eq!(
                std::fs::read_to_string(target).unwrap(),
                "DOGCODES_APPROVAL_OK"
            );
        } else {
            assert!(!target.exists(), "denied tool must not write");
        }
        println!("{option}: approval and filesystem outcome verified");
    }
}
