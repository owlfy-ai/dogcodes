use super::*;

#[test]
fn native_cancel_and_failure_override_end_turn_but_partial_success_remains_success() {
    for (outcome, expected) in [
        ("CANCELLED", Some(StopReason::Cancelled)),
        ("FAILURE", None),
        ("SUCCESS", Some(StopReason::EndTurn)),
        ("PARTIAL_SUCCESS", Some(StopReason::EndTurn)),
    ] {
        let response = serde_json::from_value(serde_json::json!({
            "stopReason":"end_turn", "_meta":{"codebuddy.ai/outcome":outcome}
        }))
        .unwrap();
        assert_eq!(prompt_stop_reason(response).ok(), expected);
    }
}

#[test]
fn native_permission_failure_preserves_the_vendors_reason() {
    let response = serde_json::from_value(serde_json::json!({"stopReason":"cancelled","_meta":{
        "codebuddy.ai/outcome":"PERMISSION_DENIED",
        "codebuddy.ai/errorMessage":"Permission denied for tool(s): unknown"
    }}))
    .unwrap();
    assert!(
        matches!(prompt_stop_reason(response), Err(DriverError::Protocol(reason))
        if reason == "CodeBuddy native turn failed: Permission denied for tool(s): unknown")
    );
}

// Use the real driver's public prompt/resume path with a controllable transport
// boundary. Reconnecting must retire that boundary before attempting a spawn.
#[tokio::test]
async fn normal_turns_reuse_but_cancel_and_failure_retire_the_connection() {
    for (result, interrupted, late_interrupt, should_reconnect) in [
        (Ok(StopReason::EndTurn), false, false, false),
        (Ok(StopReason::Cancelled), false, false, true),
        (Ok(StopReason::EndTurn), true, false, true),
        (Ok(StopReason::EndTurn), false, true, true),
        (Err(driver_error_from_channel()), false, false, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let driver = AcpDriver::new(
            "codebuddy",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: root.path().join("missing-agent"),
                args: vec![],
            },
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        assert!(driver.supports_live_session_reuse());
        let (commands, mut receiver) = mpsc::unbounded_channel();
        let (cancels, _cancel_receiver) = mpsc::unbounded_channel();
        let finished = Arc::new(AtomicBool::new(false));
        *driver.shared.connection.lock().await = Some(ConnectionHandle {
            native: None,
            commands,
            cancels,
            finished: finished.clone(),
            cursor_force: false,
            codebuddy_cwd: Some(root.path().to_owned()),
        });
        let resident = sessions::Resident::new(
            "session".into(),
            driver.shared.connection.lock().await.as_ref().unwrap(),
            false,
        );
        driver
            .shared
            .sessions
            .lock()
            .unwrap()
            .insert("session".into(), resident);
        let shared = driver.shared.clone();
        let exited = finished.clone();
        tokio::spawn(async move {
            let Command::Prompt { reply, .. } = receiver.recv().await.unwrap() else {
                panic!("expected prompt");
            };
            shared
                .interrupt_requested
                .store(interrupted, Ordering::Release);
            reply.send(result).unwrap();
            while let Some(command) = receiver.recv().await {
                let Command::LoadSession {
                    session_id, reply, ..
                } = command
                else {
                    panic!("expected session restoration");
                };
                reply.send(Ok(session_id)).unwrap();
            }
            exited.store(true, Ordering::Release);
        });
        let handle = driver
            .shared
            .sessions
            .lock()
            .unwrap()
            .get("session")
            .unwrap()
            .handle
            .clone();
        let request = SessionRequest {
            working_directory: root.path().to_string_lossy().into_owned(),
            app_mcps: vec![],
            permission_mode: None,
            additional_directories: vec![],
        };
        let mut events = driver
            .prompt(
                &handle,
                HubPromptRequest {
                    prompt: "test".into(),
                    context: vec![],
                    attachments: vec![],
                    working_directory: request.working_directory.clone(),
                    additional_directories: vec![],
                    permission_mode: None,
                },
            )
            .await
            .unwrap();
        let event = events.next().await.unwrap().unwrap();
        assert!(matches!(
            event,
            DriverEvent::TurnCompleted { .. }
                | DriverEvent::TurnInterrupted { .. }
                | DriverEvent::TurnFailed { .. }
        ));
        // Reuse state must be settled before consumers receive the terminal.
        assert!(!driver.shared.turn_active.load(Ordering::Acquire));
        if late_interrupt {
            driver.interrupt(&handle).await.unwrap();
        }
        assert_eq!(
            driver.shared.reconnect_required.load(Ordering::Acquire),
            should_reconnect
        );
        let resumed = driver
            .resume_selected_session("session", request, None, None)
            .await;
        if should_reconnect {
            // A new spawn fails deliberately; returning the old handle would
            // incorrectly reuse CodeBuddy's sticky cancellation state.
            assert!(resumed.is_err());
            assert!(finished.load(Ordering::Acquire));
        } else {
            let resumed = resumed.unwrap();
            assert_eq!(resumed.native_session_id, "session");
            assert!(!finished.load(Ordering::Acquire));
            driver.shutdown(resumed).await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "runs short model turns against the local CodeBuddy installation"]
async fn local_codebuddy_reuses_success_and_recovers_after_cancel() {
    let executable = std::env::var_os("DOGCODES_ACP_BINARY")
        .map(PathBuf::from)
        .expect("DOGCODES_ACP_BINARY");
    let driver = AcpDriver::new(
        "codebuddy",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable,
            args: vec!["--acp".into()],
        },
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let request = SessionRequest {
        working_directory: workspace.path().to_string_lossy().into_owned(),
        app_mcps: vec![],
        permission_mode: None,
        additional_directories: vec![],
    };
    let started = std::time::Instant::now();
    let handle = driver.create_session(request.clone()).await.unwrap();
    eprintln!(
        "codebuddy first_session_ms={}",
        started.elapsed().as_millis()
    );
    let original = driver
        .shared
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .finished
        .clone();
    let token = agent_domain::InstallationId::new().to_string();
    for index in 0..4 {
        if index > 0 {
            let started = std::time::Instant::now();
            driver
                .resume_selected_session(&handle.native_session_id, request.clone(), None, None)
                .await
                .unwrap();
            eprintln!(
                "codebuddy resume_before_turn_{}_ms={}",
                index + 1,
                started.elapsed().as_millis()
            );
            let current = driver
                .shared
                .connection
                .lock()
                .await
                .as_ref()
                .unwrap()
                .finished
                .clone();
            assert_eq!(Arc::ptr_eq(&original, &current), index < 3);
        }
        let prompt = match index {
            0 => format!("Remember this token: {token}. Reply only OK. Do not use tools."),
            2 => "Count from 1 to 10000 in your response. Do not use tools.".into(),
            _ => "Reply only with the token I asked you to remember. Do not use tools.".into(),
        };
        let mut stream = driver
            .prompt(
                &handle,
                HubPromptRequest {
                    prompt,
                    working_directory: request.working_directory.clone(),
                    context: vec![],
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                },
            )
            .await
            .unwrap();
        if index == 2 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            driver.interrupt(&handle).await.unwrap();
        }
        let events = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut events = vec![];
            while let Some(event) = stream.next().await {
                events.push(event.unwrap());
            }
            events
        })
        .await
        .expect("turn timeout");
        if index == 2 {
            assert!(driver.shared.reconnect_required.load(Ordering::Acquire));
        } else {
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
            );
            if index > 0 {
                let text = events
                    .iter()
                    .filter_map(|event| match event {
                        DriverEvent::TextDelta { text, .. } => Some(text.as_str()),
                        DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                            data["textDelta"].as_str().or_else(|| data["text"].as_str())
                        }
                        _ => None,
                    })
                    .collect::<String>();
                assert!(
                    text.contains(&token),
                    "native context must survive reuse and cancellation"
                );
            }
        }
    }
    driver.shutdown(handle).await.unwrap();
}
