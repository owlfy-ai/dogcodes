use acp_driver::{AcpDriver, AcpLaunch};
use driver_api::{AgentDriver, ApprovalSemanticRole, DriverEvent, PromptRequest, SessionRequest};
use futures::StreamExt;

fn exact_fixture_command(command: &str) -> bool {
    matches!(
        command.trim(),
        "printf DOGCODES_APPROVAL_OK > approval.txt"
            | "printf 'DOGCODES_APPROVAL_OK' > approval.txt"
            | "printf '%s' 'DOGCODES_APPROVAL_OK' > approval.txt"
            | "printf %s DOGCODES_APPROVAL_OK > approval.txt"
            | "/bin/zsh -lc 'printf DOGCODES_APPROVAL_OK > approval.txt'"
            | "/bin/zsh -lc \"printf DOGCODES_APPROVAL_OK > approval.txt\""
            | "/bin/bash -lc 'printf DOGCODES_APPROVAL_OK > approval.txt'"
            | "/bin/bash -lc \"printf DOGCODES_APPROVAL_OK > approval.txt\""
    )
}

fn safe_fixture_command(raw: &serde_json::Value, fallback: &str) -> bool {
    if let Some(nested) = raw.get("rawInput").or_else(|| raw.get("input")) {
        return safe_fixture_command(nested, "");
    }
    if let Some(command) = raw
        .get("command")
        .or_else(|| raw.get("cmd"))
        .and_then(serde_json::Value::as_str)
    {
        return exact_fixture_command(command);
    }
    if let Some(command) = raw.as_str() {
        return exact_fixture_command(command);
    }
    if let Some(arguments) = raw.as_array() {
        let arguments = arguments
            .iter()
            .map(serde_json::Value::as_str)
            .collect::<Option<Vec<_>>>();
        return arguments.is_some_and(|arguments| {
            (arguments.len() == 3
                && matches!(arguments[0], "/bin/zsh" | "/bin/bash" | "zsh" | "bash")
                && arguments[1] == "-lc"
                && exact_fixture_command(arguments[2]))
                || (arguments.len() == 1 && exact_fixture_command(arguments[0]))
        });
    }
    let mut leaves = Vec::new();
    collect_string_leaves(raw, &mut leaves);
    if leaves.iter().any(|text| displayed_fixture_command(text)) {
        return true;
    }
    exact_fixture_command(fallback)
}

fn collect_string_leaves<'a>(value: &'a serde_json::Value, output: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => output.push(text),
        serde_json::Value::Array(values) => {
            for value in values {
                collect_string_leaves(value, output);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_string_leaves(value, output);
            }
        }
        _ => {}
    }
}

fn displayed_fixture_command(text: &str) -> bool {
    if text.matches("printf").count() != 1
        || text.matches("DOGCODES_APPROVAL_OK").count() != 1
        || text.matches("approval.txt").count() != 1
        || text.matches('>').count() != 1
        || text.contains(';')
        || text.contains('&')
        || text.contains('|')
        || text.contains('$')
    {
        return false;
    }
    let Some(start) = text.find("printf") else {
        return false;
    };
    let Some(target_start) = text[start..].find("approval.txt") else {
        return false;
    };
    let end = start + target_start + "approval.txt".len();
    exact_fixture_command(&text[start..end])
}

fn string_leaf_facts(value: &serde_json::Value, output: &mut Vec<(usize, bool, bool, bool)>) {
    match value {
        serde_json::Value::String(text) => output.push((
            text.len(),
            text.contains("printf"),
            text.contains("DOGCODES_APPROVAL_OK"),
            text.contains("approval.txt"),
        )),
        serde_json::Value::Array(values) => {
            for value in values {
                string_leaf_facts(value, output);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                string_leaf_facts(value, output);
            }
        }
        _ => {}
    }
}

async fn verify(provider: &str, relative: &str, args: &[&str]) {
    let launch = AcpLaunch {
        executable: std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(relative),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    };
    let report = AcpDriver::probe_ready(&launch)
        .await
        .expect("Agent readiness failed");
    let cases: Vec<bool> = match std::env::var("DOGCODES_APPROVAL_CASE").as_deref() {
        Ok("deny") => vec![false],
        Ok("allow") => vec![true],
        _ => vec![false, true],
    };
    for allow in cases {
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let driver = AcpDriver::new(
            provider,
            &agent_domain::InstallationId::new().to_string(),
            launch.clone(),
            report.native_version.clone(),
            report.resume_capability,
        )
        .unwrap();
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd.clone(),
            })
            .await
            .unwrap();
        let mut stream = driver.prompt(&session,PromptRequest {
                        attachments: vec![],
            permission_mode: None,
            additional_directories: vec![],
            working_directory:cwd,
            prompt:"Approval integration test. Use a shell tool to run exactly: printf DOGCODES_APPROVAL_OK > approval.txt . Request approval if required. Do not use other tools. If denied, stop immediately and report denied. Do not access any other paths.".into(),context:vec![],
        }).await.unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut count = 0;
            let mut allowed = 0;
            let mut completed = false;
            let mut terminal = false;
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    DriverEvent::ApprovalRequested {
                        native_request_id,
                        options,
                        command_summary,
                        ..
                    } => {
                        count += 1;
                        let raw = serde_json::from_str::<serde_json::Value>(&command_summary)
                            .unwrap_or_default();
                        let safe = safe_fixture_command(&raw, &command_summary)
                            && raw
                            .get("cwd")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|path| path == workspace.path().to_string_lossy());
                        let role = if allow && safe {
                            allowed += 1;
                            ApprovalSemanticRole::ApproveOnce
                        } else {
                            ApprovalSemanticRole::Reject
                        };
                        let option = options
                            .iter()
                            .find(|option| option.semantic_role == role)
                            .expect("vendor did not supply required one-time decision");
                        if allow && !safe {
                            // Fixture-only diagnostics for nonmatching shapes.
                            let words_are_fixture_only = command_summary
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .filter(|word| !word.is_empty())
                                .all(|word| {
                                    matches!(
                                        word,
                                        "printf"
                                            | "s"
                                            | "DOGCODES_APPROVAL_OK"
                                            | "approval"
                                            | "txt"
                                            | "title"
                                            | "content"
                                            | "type"
                                            | "text"
                                            | "Shell"
                                            | "Bash"
                                            | "Execute"
                                            | "command"
                                            | "sh"
                                            | "bash"
                                            | "code"
                                            | "language"
                                    )
                                });
                            if words_are_fixture_only {
                                println!("nonmatching fixture command: {command_summary:?}");
                            } else {
                                let (shape, entries) = match &raw {
                                    serde_json::Value::Array(values) => ("array", values.len()),
                                    serde_json::Value::Object(values) => ("object", values.len()),
                                    serde_json::Value::String(_) => ("string", 1),
                                    serde_json::Value::Null => ("invalid_or_null", 0),
                                    _ => ("scalar", 1),
                                };
                                let keys = raw
                                    .as_object()
                                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                                    .unwrap_or_default();
                                let mut leaf_facts = Vec::new();
                                string_leaf_facts(&raw, &mut leaf_facts);
                                println!(
                                    "nonmatching command: shape={shape}, entries={entries}, keys={keys:?}, leaves(len,printf,marker,target)={leaf_facts:?}, command_string={}, cwd_present={}, bytes={}",
                                    raw.get("command").is_some_and(serde_json::Value::is_string),
                                    raw.get("cwd").is_some(),
                                    command_summary.len()
                                );
                            }
                        }
                        driver
                            .respond_to_interaction(&session, &native_request_id, &option.option_id)
                            .await
                            .unwrap();
                    }
                    DriverEvent::TurnCompleted { summary } => {
                        println!("completed: {summary}");
                        completed = true;
                        terminal = true;
                    }
                    event @ (DriverEvent::TurnFailed { .. }
                    | DriverEvent::TurnQuotaExhausted { .. }
                    | DriverEvent::TurnInterrupted { .. }) => {
                        println!("terminal: {event:?}");
                        terminal = true;
                    }
                    _ => {}
                }
            }
            (count, allowed, completed, terminal)
        })
        .await;
        driver.shutdown(session).await.unwrap();
        let (count, allowed, completed, terminal) = outcome.expect("approval turn timeout");
        let target = workspace.path().join("approval.txt");
        println!(
            "{provider}: allow={allow}, approvals={count}, allowed={allowed}, completed={completed}, terminal={terminal}, marker_written={}",
            target.exists()
        );
        assert!(count > 0, "{provider}: no approval request (allow={allow})");
        assert!(terminal, "{provider}: approval turn had no terminal");
        if allow {
            assert_eq!(allowed, 1, "{provider}: safe allow was not sent");
            assert!(
                completed,
                "{provider}: allowed approval turn did not complete"
            );
            assert_eq!(
                std::fs::read_to_string(target).unwrap(),
                "DOGCODES_APPROVAL_OK"
            );
        } else {
            if provider == "grok" {
                assert!(
                    completed,
                    "Grok rejection must end normally, not fail or interrupt"
                );
            }
            assert!(!target.exists(), "{provider}: denied tool wrote the file");
        }
        println!("{provider}: allow={allow}, approval and filesystem outcome verified");
    }
}

#[tokio::test]
#[ignore = "consumes two real model turns; requires operator authorization"]
async fn real_approval_kimi() {
    verify("kimi", ".kimi-code/bin/kimi", &["acp"]).await;
}
#[tokio::test]
#[ignore = "consumes two real model turns; requires operator authorization"]
async fn real_approval_grok() {
    verify(
        "grok",
        ".local/bin/grok",
        &["--permission-mode", "default", "agent", "stdio"],
    )
    .await;
}
#[tokio::test]
#[ignore = "consumes two real model turns; requires operator authorization"]
async fn real_approval_cursor() {
    verify("cursor", ".local/bin/cursor-agent", &["acp"]).await;
}

#[tokio::test]
#[ignore = "requires signed-in CodeBuddy; real approve and deny in isolated directories"]
async fn real_approval_codebuddy() {
    let binary = std::env::var("DOGCODES_ACP_BINARY").expect("DOGCODES_ACP_BINARY");
    verify("codebuddy", &binary, &["--acp"]).await;
}
