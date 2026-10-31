use serde_json::Value;

/// Task bookkeeping and child output do not start root execution. Approval
/// requests do: they must have an active turn before the user can answer.
pub(super) fn is_root_output(raw: &Value) -> bool {
    if raw["parent_tool_use_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty())
    {
        return false;
    }
    match raw["type"].as_str() {
        Some("assistant") => raw
            .pointer("/message/content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|b| matches!(b["type"].as_str(), Some("text" | "tool_use" | "thinking")))
            }),
        Some("stream_event") => matches!(
            raw["event"]["type"].as_str(),
            Some("content_block_start" | "content_block_delta")
        ),
        Some("control_request") => {
            raw["request"]["subtype"] == "can_use_tool"
                && raw["request"]["agent_id"]
                    .as_str()
                    .is_none_or(str::is_empty)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::*;
    use serde_json::json;
    #[test]
    fn only_root_execution_opens_a_continuation() {
        for raw in [
            json!({"type":"system","subtype":"task_started"}),
            json!({"type":"system","subtype":"task_notification"}),
            json!({"type":"result","subtype":"success"}),
            json!({"type":"stream_event","event":{"type":"message_start"}}),
            json!({"type":"assistant","parent_tool_use_id":"child","message":{"content":[{"type":"text","text":"child"}]}}),
        ] {
            assert!(!is_root_output(&raw), "{raw}");
        }
        assert!(is_root_output(
            &json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"answer"}}})
        ));
        assert!(is_root_output(
            &json!({"type":"control_request","request":{"subtype":"can_use_tool"}})
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn continuation_admission_streaming_interrupt_and_reuse() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("claude-fixture");
        std::fs::write(&executable, r"#!/usr/bin/python3
import sys,json
def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
    req=json.loads(line)
    if req.get('fixture') == 'continue':
        emit({'type':'system','subtype':'task_notification','status':'completed','task_id':'old'})
        emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'live continuation'}}})
    elif req.get('fixture') == 'finish':
        emit({'type':'result','subtype':'success'})
    elif req.get('type') == 'control_request':
        emit({'type':'result','subtype':'success'})
    else:
        emit({'type':'result','subtype':'success'})
").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            None,
            SessionResumeCapability::Unsupported,
        )
        .unwrap();
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: None,
                additional_directories: vec![],
                working_directory: directory.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let request = || HubPromptRequest {
            attachments: vec![],
            permission_mode: None,
            additional_directories: vec![],
            prompt: "go".into(),
            working_directory: directory.path().to_string_lossy().into_owned(),
            context: vec![],
        };
        let mut autonomous = driver.take_autonomous_events(&session).await.unwrap();
        assert!(driver.take_autonomous_events(&session).await.is_none());
        let mut first = driver.prompt(&session, request()).await.unwrap();
        assert!(matches!(
            first.next().await.unwrap().unwrap(),
            DriverEvent::TurnCompleted { .. }
        ));
        assert!(first.next().await.is_none());
        let process = driver.processes.lock().await[&session.native_session_id].clone();
        process
            .send_line(&json!({"fixture":"continue"}))
            .await
            .unwrap();
        let marker = tokio::time::timeout(std::time::Duration::from_secs(3), autonomous.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let DriverEvent::TurnStarted {
            native_turn_id: first_id,
        } = marker
        else {
            panic!("missing autonomous start")
        };
        assert!(matches!(
            autonomous.next().await.unwrap().unwrap(),
            DriverEvent::FirstOutput
        ));
        assert!(
            matches!(autonomous.next().await.unwrap().unwrap(), DriverEvent::TextDelta { text, .. } if text == "live continuation")
        );
        assert!(driver.has_autonomous_turn(&session.native_session_id).await);
        assert!(driver.prompt(&session, request()).await.is_err());
        assert!(process.configure(None, None).await.is_err());
        driver.interrupt(&session).await.unwrap();
        assert!(matches!(
            autonomous.next().await.unwrap().unwrap(),
            DriverEvent::TurnInterrupted { .. }
        ));
        // Native idle is not enough: Core must first finish its durable terminal.
        assert!(driver.prompt(&session, request()).await.is_err());
        driver.acknowledge_autonomous_turn(&session).await;
        let mut next = driver.prompt(&session, request()).await.unwrap();
        assert!(matches!(
            next.next().await.unwrap().unwrap(),
            DriverEvent::TurnCompleted { .. }
        ));
        process
            .send_line(&json!({"fixture":"continue"}))
            .await
            .unwrap();
        let DriverEvent::TurnStarted {
            native_turn_id: second_id,
        } = autonomous.next().await.unwrap().unwrap()
        else {
            panic!("missing second start")
        };
        assert!(first_id.is_none() && second_id.is_none());
        assert!(matches!(
            autonomous.next().await.unwrap().unwrap(),
            DriverEvent::FirstOutput
        ));
        assert!(matches!(
            autonomous.next().await.unwrap().unwrap(),
            DriverEvent::TextDelta { .. }
        ));
        process
            .send_line(&json!({"fixture":"finish"}))
            .await
            .unwrap();
        assert!(matches!(
            autonomous.next().await.unwrap().unwrap(),
            DriverEvent::TurnCompleted { .. }
        ));
        driver.acknowledge_autonomous_turn(&session).await;
        driver.shutdown(session).await.unwrap();
    }
}
