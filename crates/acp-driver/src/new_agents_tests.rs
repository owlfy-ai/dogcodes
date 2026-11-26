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
fn only_a_script_inside_the_apps_own_resources_selects_electron_node_mode() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Qoder CN");
    let executable = app.join("Qoder CN.exe");
    let resources = agent_runtime::electron_resources_directory(&executable).unwrap();
    let launch = |args: Vec<String>| AcpLaunch {
        executable: executable.clone().into(),
        args,
    };
    let script = resources
        .join("app.asar.unpacked/runtime.mjs")
        .to_string_lossy()
        .into_owned();
    assert!(electron_node_script(&launch(vec![script, "--acp".into()])));
    // A plain CLI keeps its normal launch, even with a script argument elsewhere.
    assert!(!electron_node_script(&launch(vec!["--acp".into()])));
    assert!(!electron_node_script(&launch(vec![])));
    let foreign = root
        .path()
        .join("Other/resources/runtime.mjs")
        .to_string_lossy()
        .into_owned();
    assert!(!electron_node_script(&launch(vec![foreign])));
}
#[test]
fn deepseek_grouped_catalog_preserves_opaque_model_identifiers_and_effort() {
    let state = json!({"configOptions":[{"id":"model","category":"model","type":"select","currentValue":"[\"provider\",\"flash\"]","options":[{"group":"provider","options":[{"value":"[\"provider\",\"flash\"]","name":"Flash"}]}]},{"id":"reasoning_effort","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"off","name":"Off"},{"value":"high","name":"High"}]}]});
    let catalog = models::catalog(&state);
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(catalog.models[0].id, "[\"provider\",\"flash\"]");
    assert_eq!(catalog.models[0].thinking_options.len(), 2);
}
/// Qoder CN runs its bundled CLI script through the desktop app's Electron
/// binary; the path matches `bundled_script` in drivers/manifests/qoder-cn.toml.
fn qoder_cn_args(binary: &str) -> Vec<String> {
    let script = agent_runtime::electron_resources_directory(std::path::Path::new(binary))
        .unwrap()
        .join("app.asar.unpacked/node_modules/@qoder-ai/qoder-cn-agent-sdk/dist/_worker/qoder-worker-runtime.obf.mjs");
    vec![script.to_string_lossy().into_owned(), "--acp".into()]
}
#[tokio::test]
#[ignore = "metadata-only native new/load/resume/config acceptance for one local ACP installation"]
async fn real_new_agent_metadata() {
    let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
    let binary = std::env::var("DOGCODES_ACP_BINARY").unwrap();
    let args: Vec<String> = match provider.as_str() {
        "qoder-cn" => qoder_cn_args(&binary),
        "opencode" => vec!["acp".into()],
        "kiro" => ["acp", "--agent-engine", "v3", "--auth-method", "cli"]
            .map(Into::into)
            .into(),
        "deepseek-harness" => vec!["--profile".into(), "acp".into()],
        _ => panic!("unsupported acceptance provider"),
    };
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
        app_mcps: vec![],
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
    // Every restore returns a newly scoped handle; the earlier one cannot close it.
    let resumed = driver
        .resume_selected_session(&handle.native_session_id, request.clone(), model, thinking)
        .await
        .unwrap();
    println!(
        "{provider} models={} permissions={} warm_ms={}",
        catalog.models.len(),
        permissions.modes.len(),
        warm.elapsed().as_millis()
    );
    driver.shutdown(resumed).await.unwrap();
    let restored = driver
        .resume_selected_session(&handle.native_session_id, request, model, thinking)
        .await
        .unwrap();
    driver.shutdown(restored).await.unwrap();
}

#[tokio::test]
#[ignore = "runs a short native model turn in a temporary workspace"]
async fn real_new_agent_turn() {
    let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
    let binary = std::env::var("DOGCODES_ACP_BINARY").unwrap();
    let args: Vec<String> = match provider.as_str() {
        "qoder-cn" => qoder_cn_args(&binary),
        "kiro" => ["acp", "--agent-engine", "v3", "--auth-method", "cli"]
            .map(Into::into)
            .into(),
        "deepseek-harness" => vec!["--profile".into(), "acp".into()],
        "opencode" => vec!["acp".into()],
        _ => panic!("unknown provider"),
    };
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
        app_mcps: vec![],
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

#[tokio::test]
#[ignore = "sends two real messages through a locally authenticated agent"]
async fn real_active_turn_steering() {
    let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
    let args = match provider.as_str() {
        "codebuddy" => vec!["--acp"],
        "grok" => vec!["agent", "stdio"],
        "kiro" => vec!["acp", "--agent-engine", "v3", "--auth-method", "cli"],
        "opencode" => vec!["acp"],
        _ => panic!("unsupported provider"),
    };
    let driver = AcpDriver::connect_ready(
        &provider,
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: std::env::var("DOGCODES_ACP_BINARY").unwrap().into(),
            args: args.into_iter().map(String::from).collect(),
        },
        None,
    )
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let handle = driver
        .create_session(SessionRequest {
            working_directory: cwd.clone(),
            additional_directories: vec![],
            app_mcps: vec![],
            permission_mode: None,
        })
        .await
        .unwrap();
    let mut stream = driver.prompt(&handle, HubPromptRequest {
        working_directory: cwd, additional_directories: vec![], attachments: vec![], context: vec![], permission_mode: None,
        prompt: "Use the shell tool to run exactly `sleep 7` once. Then reply ORIGINAL unless I send a correction. Do not read or write any files.".into(),
    }).await.unwrap();
    let sender = driver
        .turn_steering(&handle)
        .await
        .expect("advertised steering");
    let marker = format!("STEER_{}", uuid::Uuid::now_v7().simple());
    let mut answer = String::new();
    let mut sent = false;
    tokio::time::timeout(std::time::Duration::from_mins(2), async {
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            let tool = matches!(
                &event,
                DriverEvent::ApprovalRequested { .. } | DriverEvent::ToolStarted { .. }
            ) || matches!(&event, DriverEvent::Presentation {data,..} if data["type"]=="tool");
            if tool && !sent {
                assert_eq!(
                    sender
                        .send(
                            "live-steering",
                            &format!("After the wait reply exactly {marker}.")
                        )
                        .await
                        .unwrap(),
                    driver_api::SteerDelivery::Accepted
                );
                sent = true;
            }
            match event {
                DriverEvent::ApprovalRequested {
                    native_request_id,
                    command_summary,
                    options,
                    ..
                } => {
                    assert!(
                        command_summary.contains("sleep 7"),
                        "unexpected approval {command_summary}"
                    );
                    let option = options
                        .iter()
                        .find(|o| o.semantic_role == driver_api::ApprovalSemanticRole::ApproveOnce)
                        .unwrap();
                    driver
                        .respond_to_interaction(&handle, &native_request_id, &option.option_id)
                        .await
                        .unwrap();
                }
                DriverEvent::TextDelta { text, .. } => answer.push_str(&text),
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                    answer.push_str(data["textDelta"].as_str().unwrap_or_default());
                }
                DriverEvent::TurnCompleted { .. } => break,
                DriverEvent::TurnFailed { reason } => panic!("native failure {reason}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        sender
            .send("after-completion", "Late message")
            .await
            .unwrap(),
        driver_api::SteerDelivery::Rejected
    );
    driver.shutdown(handle).await.unwrap();
    assert!(
        sent && answer.contains(&marker),
        "{provider} final response: {answer}"
    );
}
