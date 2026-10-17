use std::path::Path;

use agent_domain::ArtifactId;
use codex_driver::CodexDriver;
use driver_api::{
    AgentDriver, ContextDelivery, ContextDeliveryKind, DriverEvent, PromptRequest, SessionRequest,
};
use tokio_stream::StreamExt;

fn fixture_executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_codex-driver-fixture"))
}

#[tokio::test]
async fn analysis_allows_local_shell_with_readonly_sandbox_and_disables_remote_tools() {
    let driver =
        CodexDriver::new(fixture_executable(), "ins_review".into(), "fixture".into()).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    assert!(driver.supports_readonly_analysis());
    let cwd = workspace.path().to_str().unwrap();
    let session = driver.create_analysis_session(cwd).await.unwrap();
    let mut stream = driver
        .prompt(
            &session,
            PromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                prompt: "review scope\n".repeat(10_000),
                working_directory: cwd.into(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    let mut result = String::new();
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            DriverEvent::TextDelta { text, .. } => result.push_str(&text),
            DriverEvent::TurnStarted { .. } | DriverEvent::TurnCompleted { .. } => {}
            other => panic!("unexpected analysis event: {other:?}"),
        }
    }
    assert_eq!(result, "historical analysis");
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn review_uses_full_native_permissions_in_an_independent_session() {
    let driver =
        CodexDriver::new(fixture_executable(), "ins_review".into(), "fixture".into()).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    assert!(driver.supports_readonly_analysis());
    let cwd = workspace.path().to_str().unwrap();
    let session = driver.create_review_session(cwd, &[]).await.unwrap();
    let mut stream = driver
        .prompt(
            &session,
            PromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                prompt: "review scope\n".repeat(10_000),
                working_directory: cwd.into(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    let mut result = String::new();
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            DriverEvent::TextDelta { text, .. } => result.push_str(&text),
            DriverEvent::TurnStarted { .. } | DriverEvent::TurnCompleted { .. } => {}
            other => panic!("unexpected analysis event: {other:?}"),
        }
    }
    assert_eq!(result, "historical analysis");
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn real_process_path_accepts_large_handoff_and_streams_a_turn() {
    CodexDriver::probe_ready(fixture_executable())
        .await
        .unwrap();
    let driver =
        CodexDriver::new(fixture_executable(), "ins_fixture".into(), "0.153.0".into()).unwrap();
    assert!(driver.supports_live_session_reuse());
    assert_eq!(
        driver.executor().installation_id.as_deref(),
        Some("ins_fixture")
    );
    let workspace = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();
    let additional = vec![extra.path().to_string_lossy().into_owned()];
    let session = driver
        .create_session(SessionRequest {
            history_mcp: None,
            permission_mode: None,
            additional_directories: additional.clone(),
            working_directory: workspace.path().to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    assert_eq!(session.native_session_id, "fixture-thread");

    let mut stream = driver
        .prompt(
            &session,
            PromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: additional,
                prompt: "do work".into(),
                working_directory: workspace.path().to_string_lossy().into_owned(),
                context: vec![ContextDelivery {
                    artifact_id: ArtifactId::new(),
                    kind: ContextDeliveryKind::Handoff,
                    payload_sha256: "fixture-hash".into(),
                    content: serde_json::to_vec(&serde_json::json!({ "objective": "continue", "history": "交接上下文\n\"".repeat(8_000) })).unwrap(),
                }],
            },
        )
        .await
        .unwrap();
    let mut events = vec![];
    let mut answered = false;
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        // The fixture asks for a command approval; the hub must surface the
        // vendor request and can only answer with the vendor decision ids.
        if let DriverEvent::ApprovalRequested {
            native_request_id, ..
        } = &event
        {
            assert_eq!(native_request_id, "codex-approval-1");
            driver
                .respond_to_interaction(&session, native_request_id, "decline")
                .await
                .unwrap();
            answered = true;
        }
        events.push(event);
    }
    assert!(answered);
    assert!(matches!(events[0], DriverEvent::TurnStarted { .. }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DriverEvent::ApprovalRequested { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        DriverEvent::TextDelta { text, .. } if text == "fixture reply"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        DriverEvent::ToolStarted { item_id, tool_name, summary }
            if item_id == "search-1" && tool_name == "web_search" && summary == "搜索网页"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        DriverEvent::ToolCompleted { item_id, tool_name, summary, success }
            if item_id == "search-1" && tool_name == "web_search" && summary == "搜索网页" && *success
    )));
    assert!(matches!(
        events.last(),
        Some(DriverEvent::TurnCompleted { .. })
    ));
    assert_eq!(
        driver
            .resume_session(&session.native_session_id, "")
            .await
            .unwrap(),
        session
    );
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn interrupt_preserves_a_terminal_notification_interleaved_before_its_response() {
    let driver = CodexDriver::new(
        fixture_executable(),
        "ins_interrupt".into(),
        "0.153.0".into(),
    )
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = driver
        .create_session(SessionRequest {
            history_mcp: None,
            permission_mode: None,
            additional_directories: vec![],
            working_directory: workspace.path().to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    let mut stream = driver
        .prompt(
            &session,
            PromptRequest {
                attachments: vec![],
                permission_mode: None,
                additional_directories: vec![],
                prompt: "wait for interrupt".into(),
                working_directory: workspace.path().to_string_lossy().into_owned(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        DriverEvent::TurnStarted { .. }
    ));

    driver.interrupt(&session).await.unwrap();
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        DriverEvent::TurnInterrupted { .. }
    ));
    assert!(stream.next().await.is_none());
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn changed_selection_reuses_the_process_and_applies_on_turn_start() {
    let driver =
        CodexDriver::new(fixture_executable(), "ins_fixture".into(), "0.153.4".into()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let request = || SessionRequest {
        history_mcp: None,
        permission_mode: None,
        additional_directories: vec![],
        working_directory: directory.path().to_string_lossy().into_owned(),
    };
    let first = driver.create_session(request()).await.unwrap();
    let mut pid = None;
    for (model, thinking) in [
        (Some("fixture-switched"), Some("high")),
        (Some("fixture-switched"), Some("low")),
        (None, None),
    ] {
        let updated = driver
            .reconfigure_session(&first.native_session_id, request(), model, thinking)
            .await
            .unwrap();
        assert_eq!(updated.native_session_id, first.native_session_id);
        let mut stream = driver
            .prompt(
                &updated,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    working_directory: directory.path().to_string_lossy().into_owned(),
                    prompt: "settings-report".into(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let mut reported = false;
        while let Some(event) = stream.next().await {
            if let DriverEvent::TextDelta { text, .. } = event.unwrap() {
                let report: serde_json::Value = serde_json::from_str(&text).unwrap();
                if let Some(pid) = pid {
                    assert_eq!(report["pid"].as_u64(), Some(pid));
                }
                pid = report["pid"].as_u64();
                assert_eq!(report["model"], model.unwrap_or("fixture-default"));
                assert_eq!(report["effort"], thinking.unwrap_or("medium"));
                reported = true;
            }
        }
        assert!(reported);
    }
    driver.shutdown(first).await.unwrap();
}

#[tokio::test]
async fn sidebar_chat_allows_sandboxed_reads_but_disables_remote_tools_and_permission_escalation() {
    let driver =
        CodexDriver::new(fixture_executable(), "ins_chat".into(), "fixture".into()).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path().to_str().unwrap();
    assert!(driver.supports_readonly_chat());
    assert!(driver.allows_readonly_chat_tool("command_execution"));
    assert!(driver.allows_readonly_chat_tool("terminal"));
    assert!(!driver.allows_readonly_chat_tool("file_change"));
    assert!(!driver.allows_readonly_chat_tool("edit"));
    assert!(!driver.allows_readonly_chat_tool("web_search"));
    let session = driver
        .create_chat_session(cwd, &[cwd.into()])
        .await
        .unwrap();
    // Even an accidentally supplied full-access prompt cannot widen this session.
    let mut stream = driver
        .prompt(
            &session,
            PromptRequest {
                working_directory: cwd.into(),
                additional_directories: vec![cwd.into()],
                permission_mode: Some("full-access".into()),
                attachments: vec![],
                context: vec![],
                prompt: "CHAT_RESTRICTIONS".into(),
            },
        )
        .await
        .unwrap();
    let text = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                DriverEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                DriverEvent::TurnCompleted { .. } => break,
                _ => {}
            }
        }
        text
    })
    .await
    .unwrap();
    assert_eq!(text, "chat restrictions verified");
    driver.shutdown(session).await.unwrap();
}
