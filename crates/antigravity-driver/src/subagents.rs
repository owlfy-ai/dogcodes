//! Read only the child transcript identified by the native delegation event.
use agent_runtime::SharedTarget;
use driver_api::{DriverError, DriverEvent};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

async fn transcript(target: &SharedTarget, id: &str) -> Result<String, DriverError> {
    if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(DriverError::Protocol("invalid child id".into()));
    }
    let home = target
        .inherited("HOME")
        .ok_or_else(|| DriverError::CapabilityUnavailable("home unavailable".into()))?;
    let path = std::path::PathBuf::from(home)
        .join(".gemini/antigravity-cli/brain")
        .join(id)
        .join(".system_generated/logs/transcript.jsonl");
    let bytes = super::scratch::read_file(target, &path, 1_048_577)
        .await
        .ok_or_else(|| DriverError::Protocol("log unavailable".into()))?;
    if bytes.len() > 1_048_576 {
        return Err(DriverError::Protocol("log unavailable".into()));
    }
    String::from_utf8(bytes).map_err(|_| DriverError::Protocol("log unavailable".into()))
}

// A DONE tool step (including send_message) can be followed by more work.
// A finished model response with content and no pending calls ends the child's turn.
fn status(text: &str) -> Option<&'static str> {
    let last = text.lines().rev().find(|line| !line.trim().is_empty())?;
    let row: Value = serde_json::from_str(last).ok()?;
    match row["status"].as_str()? {
        "ERROR" | "FAILED" if row["type"] == "PLANNER_RESPONSE" => Some("failed"),
        "CANCELLED" | "CANCELED" | "INTERRUPTED" if row["type"] == "PLANNER_RESPONSE" => {
            Some("interrupted")
        }
        "DONE"
            if row["type"] == "PLANNER_RESPONSE"
                && row["content"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
                && (row["tool_calls"].is_null()
                    || row["tool_calls"].as_array().is_some_and(Vec::is_empty)) =>
        {
            Some("completed")
        }
        _ => None,
    }
}

// The parent can finish first. Keep the original turn's sender and observe
// only this child's log until the child settles or the native session closes.
pub(super) async fn observe_completion(
    target: SharedTarget,
    id: String,
    updates: mpsc::UnboundedSender<DriverEvent>,
    mut closing: watch::Receiver<bool>,
) {
    let poll = async {
        let mut delay = Duration::from_millis(250);
        let mut read_errors = 0;
        loop {
            match transcript(&target, &id).await {
                Ok(text) => {
                    read_errors = 0;
                    if let Some(status) = status(&text) {
                        return status;
                    }
                }
                Err(_) => {
                    read_errors += 1;
                    if read_errors >= 8 {
                        return "unknown";
                    }
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(5));
        }
    };
    let status = tokio::select! {
        () = updates.closed() => return,
        _ = closing.wait_for(|closed| *closed) => "unknown",
        status = poll => status,
    };
    let _ = updates.send(DriverEvent::Presentation {
        item_id: id,
        data: json!({"type":"subagent","status":status}),
    });
}

pub(super) async fn history(target: &SharedTarget, id: &str) -> Result<Vec<Value>, DriverError> {
    let text = transcript(target, id).await?;
    let mut output = Vec::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(calls) = row["tool_calls"].as_array() {
            for call in calls {
                output.push(serde_json::json!({"type":"tool","name":call["name"],"input":call["args"],"status":"unknown"}));
            }
        }
        if row["type"] == "TOOL_RESULT" {
            output.push(serde_json::json!({"type":"tool","name":row["tool_name"],"output":row["content"],"status":"unknown"}));
        }
    }
    if let Some(status) = status(&text) {
        output.push(json!({"type":"status","status":status}));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::status;
    #[test]
    fn only_the_latest_child_step_can_confirm_completion() {
        let reply = r#"{"type":"PLANNER_RESPONSE","status":"DONE","content":"result"}"#;
        assert_eq!(status(reply), Some("completed"));
        for tail in [
            r#"{"type":"GENERIC","status":"DONE"}"#,
            r#"{"type":"PLANNER_RESPONSE","status":"DONE","content":"checking","tool_calls":[{"name":"read_file"}]}"#,
            r#"{"type":"SYSTEM_MESSAGE","status":"DONE"}"#,
            "{",
        ] {
            assert_eq!(status(&format!("{reply}\n{tail}")), None);
        }
    }
}
