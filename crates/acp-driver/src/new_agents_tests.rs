use super::*;
use serde_json::json;
#[test]
fn resume_negotiation_uses_the_advertised_native_method() {
    let current = json!({"agentCapabilities":{"sessionCapabilities":{"resume":{}}}});
    assert!(supports_native_resume(&current));
    assert_eq!(resume_method(&current), "session/resume");
    let legacy =
        json!({"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"resume":{}}}});
    assert!(supports_native_resume(&legacy));
    assert_eq!(resume_method(&legacy), "session/load");
    assert!(!supports_native_resume(
        &json!({"agentCapabilities":{"sessionCapabilities":{"resume":null}}})
    ));
}
#[test]
fn deepseek_grouped_catalog_preserves_opaque_model_identifiers_and_effort() {
    let state = json!({"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"[\"provider\",\"flash\"]","options":[{"group":"provider","options":[{"value":"[\"provider\",\"flash\"]","name":"Flash"}]}]},{"id":"reasoning_effort","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"off","name":"Off"},{"value":"high","name":"High"}]}]});
    let catalog = models::catalog(&state);
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].id, "[\"provider\",\"flash\"]");
    assert_eq!(catalog.models[0].thinking_options.len(), 2);
}
#[tokio::test]
#[ignore = "metadata-only native new/load/resume/config acceptance for one local ACP installation"]
async fn real_new_agent_metadata() {
    let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
    let binary = std::env::var("DOGCODES_ACP_BINARY").unwrap();
    let args: Vec<String> = match provider.as_str() {
        "opencode" => vec!["acp"],
        "kiro" => vec!["acp", "--agent-engine", "v3", "--auth-method", "cli"],
        "deepseek-harness" => vec!["--profile", "acp"],
        _ => panic!("unsupported acceptance provider"),
    }
    .into_iter()
    .map(Into::into)
    .collect();
    let started = std::time::Instant::now();
    let driver = AcpDriver::connect_ready(
        &provider,
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: binary.into(),
            args,
        },
        None,
    )
    .await
    .unwrap();
    println!("{provider} readiness_ms={}", started.elapsed().as_millis());
    assert_eq!(
        driver.resume_capability(),
        SessionResumeCapability::Supported
    );
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let catalog = driver
        .model_catalog_in_workspace(None, Some(&cwd))
        .await
        .unwrap();
    assert!(!catalog.models.is_empty());
    let permissions = driver.permission_catalog(Some(&cwd)).await.unwrap();
    let request = SessionRequest {
        working_directory: cwd,
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: permissions.default_mode_id.clone(),
    };
    let model = catalog.default_model.as_deref();
    let thinking = catalog
        .models
        .iter()
        .find(|m| Some(m.id.as_str()) == model)
        .and_then(|m| m.default_thinking.as_deref());
    let handle = driver
        .create_selected_session(request.clone(), model, thinking)
        .await
        .unwrap();
    let warm = std::time::Instant::now();
    driver
        .resume_selected_session(&handle.native_session_id, request.clone(), model, thinking)
        .await
        .unwrap();
    println!(
        "{provider} models={} permissions={} warm_ms={}",
        catalog.models.len(),
        permissions.modes.len(),
        warm.elapsed().as_millis()
    );
    driver.shutdown(handle.clone()).await.unwrap();
    driver
        .resume_selected_session(&handle.native_session_id, request, model, thinking)
        .await
        .unwrap();
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
#[ignore = "runs a short native model turn in a temporary workspace"]
async fn real_new_agent_turn() {
    let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
    let binary = std::env::var("DOGCODES_ACP_BINARY").unwrap();
    let args = match provider.as_str() {
        "kiro" => vec!["acp", "--agent-engine", "v3", "--auth-method", "cli"],
        "deepseek-harness" => vec!["--profile", "acp"],
        "opencode" => vec!["acp"],
        _ => panic!("unknown provider"),
    }
    .into_iter()
    .map(Into::into)
    .collect();
    let driver = AcpDriver::connect_ready(
        &provider,
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: binary.into(),
            args,
        },
        None,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let request = SessionRequest {
        working_directory: cwd.clone(),
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: None,
    };
    let handle = driver.create_session(request).await.unwrap();
    let mut stream = driver
        .prompt(
            &handle,
            HubPromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd,
                context: vec![],
                prompt: "Reply with exactly DOGCODES_OK. Do not use any tools or modify files."
                    .into(),
            },
        )
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let mut text = String::new();
    let mut deltas = 0;
    let mut completed = false;
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                DriverEvent::TextDelta { text: delta, .. } => {
                    if deltas == 0 {
                        println!(
                            "{provider} first_delta_ms={}",
                            started.elapsed().as_millis()
                        );
                    }
                    deltas += 1;
                    text.push_str(&delta);
                }
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                    if let Some(delta) = data["textDelta"].as_str().filter(|s| !s.is_empty()) {
                        if deltas == 0 {
                            println!(
                                "{provider} first_delta_ms={}",
                                started.elapsed().as_millis()
                            );
                        }
                        deltas += 1;
                        text.push_str(delta);
                    }
                }
                DriverEvent::TurnCompleted { .. } => {
                    completed = true;
                    break;
                }
                DriverEvent::TurnFailed { reason } => panic!("native turn failed: {reason}"),
                DriverEvent::TurnQuotaExhausted { .. } => panic!("native quota exhausted"),
                DriverEvent::ApprovalRequested { .. } => {
                    driver.interrupt(&handle).await.unwrap();
                    panic!("unexpected tool approval for no-tool test");
                }
                _ => {}
            }
        }
    })
    .await;
    driver.shutdown(handle).await.unwrap();
    result.unwrap();
    assert!(completed);
    assert!(text.contains("DOGCODES_OK"));
    assert!(deltas > 0);
    println!("{provider} stream_deltas={deltas} completed=true");
}
