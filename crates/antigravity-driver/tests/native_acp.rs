#![cfg(unix)]

use antigravity_driver::AntigravityDriver;
use driver_api::{
    AgentDriver, ApprovalSemanticRole, DriverEvent, PromptRequest, SessionHandle, SessionRequest,
};
use futures::StreamExt;
use serde_json::{Value, json};

#[tokio::test]
#[ignore = "uses a signed-in local agy and real model turns in a temporary workspace"]
async fn native_approvals_and_questions_survive_policy_changes() {
    let executable =
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/bin/agy");
    let version = std::process::Command::new(&executable)
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    let driver = AntigravityDriver::connect_ready(
        "antigravity",
        &agent_domain::InstallationId::new().to_string(),
        executable.into(),
        Some(version.trim().into()),
    )
    .await
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().canonicalize().unwrap();
    let cwd = path.to_str().unwrap();
    let request = |mode: &str| SessionRequest {
        app_mcps: vec![],
        permission_mode: Some(mode.into()),
        additional_directories: vec![],
        working_directory: cwd.into(),
    };
    let mut session = driver
        .create_selected_session(
            request("desktop-approvals"),
            Some("gemini-4-neon-low"),
            None,
        )
        .await
        .unwrap();
    let id = session.native_session_id.clone();
    let test_cases = [
        (
            "desktop-approvals",
            "approved",
            Some(ApprovalSemanticRole::ApproveOnce),
            true,
        ),
        ("dangerously-skip-permissions", "skipped", None, true),
        (
            "desktop-approvals",
            "rejected",
            Some(ApprovalSemanticRole::Reject),
            false,
        ),
    ];
    for (index, (mode, name, decision, exists)) in test_cases.into_iter().enumerate() {
        if index > 0 {
            session = driver
                .reconfigure_session(&id, request(mode), Some("gemini-4-neon"), Some("low"))
                .await
                .unwrap();
            assert_eq!(session.native_session_id, id);
        }
        let marker = path.join(name);
        let command = format!("/usr/bin/touch {}", marker.display());
        let prompt = format!(
            "Call run_command once with exactly `{command}`. If denied, stop. Use no other tools."
        );
        let (approvals, _, _) = run_turn(&driver, &session, cwd, prompt, decision).await;
        assert_eq!(approvals, usize::from(decision.is_some()), "{mode}");
        assert_eq!(marker.is_file(), exists, "{mode}");
        eprintln!("native ACP {mode} {name} passed");
        if mode == "dangerously-skip-permissions" {
            let (_, questions, answer) = run_turn(
                &driver,
                &session,
                cwd,
                "Use the native ask_question tool to ask one single-select question: Which token? Choices DOGCODESRED and DOGCODESBLUE. When the tool returns the answer, reply with only the chosen token. Do not call other tools.".into(),
                None,
            )
            .await;
            assert_eq!(questions, 1);
            assert!(answer.contains("DOGCODESBLUE"), "model replied {answer:?}");
            eprintln!("native ACP question passed");
        }
    }
    session = driver
        .reconfigure_session(&id, request("plan"), Some("gemini-4-neon"), Some("low"))
        .await
        .unwrap();
    assert_eq!(session.native_session_id, id);
    driver.shutdown(session).await.unwrap();
}

async fn run_turn(
    driver: &AntigravityDriver,
    session: &SessionHandle,
    cwd: &str,
    prompt: String,
    decision: Option<ApprovalSemanticRole>,
) -> (usize, usize, String) {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut events = driver
            .prompt(
                session,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt,
                    working_directory: cwd.into(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let mut approvals = 0;
        let mut questions = 0;
        let mut answer = String::new();
        while let Some(event) = events.next().await {
            match event.unwrap() {
                DriverEvent::ApprovalRequested {
                    native_request_id,
                    options,
                    command_summary,
                    ..
                } => {
                    approvals += 1;
                    assert!(command_summary.contains("/usr/bin/touch"));
                    let role = decision.expect("unexpected native approval");
                    let option = options
                        .iter()
                        .find(|option| option.semantic_role == role)
                        .unwrap_or_else(|| panic!("missing {role:?} in {options:?}"));
                    driver
                        .respond_to_interaction(session, &native_request_id, &option.option_id)
                        .await
                        .unwrap();
                }
                DriverEvent::QuestionRequested {
                    native_request_id,
                    form,
                } => {
                    questions += 1;
                    let (field, option) = form.schema["properties"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .find_map(|(field, property)| {
                            property["oneOf"]
                                .as_array()?
                                .iter()
                                .find(|option| {
                                    option["title"].as_str().is_some_and(|title| {
                                        title.eq_ignore_ascii_case("DOGCODESBLUE")
                                    })
                                })
                                .map(|option| (field, option["const"].clone()))
                        })
                        .expect("native question had no DOGCODESBLUE option");
                    assert_ne!(option, Value::Null);
                    driver
                        .respond_to_question(
                            session,
                            &native_request_id,
                            agent_domain::QuestionAnswer {
                                cancelled: false,
                                values: json!({field: option}),
                            },
                        )
                        .await
                        .unwrap();
                }
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                    if let Some(text) = data["textDelta"].as_str() {
                        answer.push_str(text);
                    }
                }
                DriverEvent::TurnCompleted { .. } => return (approvals, questions, answer),
                DriverEvent::TurnInterrupted { .. }
                    if decision == Some(ApprovalSemanticRole::Reject) =>
                {
                    return (approvals, questions, answer);
                }
                DriverEvent::TurnFailed { reason } => panic!("native ACP turn failed: {reason}"),
                _ => {}
            }
        }
        panic!("native ACP turn ended without completion")
    })
    .await
    .expect("native ACP turn timed out")
}
