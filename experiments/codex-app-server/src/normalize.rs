//! Minimal, loss-aware normalization of stable Codex App Server frames.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::transport::IncomingFrame;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum NormalizedEvent {
    TurnStarted {
        native_turn_id: Option<String>,
    },
    TextDelta {
        item_id: Option<String>,
        text: String,
    },
    DiffUpdated {
        diff: String,
    },
    UsageUpdated {
        snapshot: Value,
    },
    InteractionRequested {
        native_request_id: Value,
        method: String,
        payload: Value,
    },
    TurnCompleted {
        outcome: Option<String>,
        error: Option<Value>,
    },
    VendorExtension {
        method: String,
        payload: Value,
    },
    ProtocolWarning {
        message: String,
        payload: Value,
    },
}

#[must_use]
pub fn normalize(frame: &IncomingFrame) -> Option<NormalizedEvent> {
    match frame {
        IncomingFrame::Notification { method, params, .. } => {
            Some(normalize_notification(method, params))
        }
        IncomingFrame::ServerRequest {
            id, method, params, ..
        } if is_interaction_method(method) => Some(NormalizedEvent::InteractionRequested {
            native_request_id: id.clone(),
            method: method.clone(),
            payload: params.clone(),
        }),
        IncomingFrame::ServerRequest { method, params, .. } => {
            Some(NormalizedEvent::VendorExtension {
                method: method.clone(),
                payload: params.clone(),
            })
        }
        IncomingFrame::Unknown { raw } => Some(NormalizedEvent::ProtocolWarning {
            message: "unrecognized app-server frame shape".into(),
            payload: raw.clone(),
        }),
        IncomingFrame::Response { .. } => None,
    }
}

fn normalize_notification(method: &str, params: &Value) -> NormalizedEvent {
    match method {
        "turn/started" => NormalizedEvent::TurnStarted {
            native_turn_id: params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
        "item/agentMessage/delta" => NormalizedEvent::TextDelta {
            item_id: params
                .get("itemId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            text: params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        "turn/diff/updated" => NormalizedEvent::DiffUpdated {
            diff: params
                .get("diff")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        "thread/tokenUsage/updated" => NormalizedEvent::UsageUpdated {
            snapshot: params.clone(),
        },
        "turn/completed" => {
            let turn = params.get("turn").unwrap_or(&Value::Null);
            NormalizedEvent::TurnCompleted {
                outcome: turn
                    .get("status")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                error: turn.get("error").filter(|value| !value.is_null()).cloned(),
            }
        }
        "configWarning" | "warning" | "error" => NormalizedEvent::ProtocolWarning {
            message: params
                .get("summary")
                .or_else(|| params.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("app-server warning")
                .to_owned(),
            payload: params.clone(),
        },
        _ => NormalizedEvent::VendorExtension {
            method: method.to_owned(),
            payload: params.clone(),
        },
    }
}

fn is_interaction_method(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::transport::classify_frame;

    fn normalized(frame: Value) -> NormalizedEvent {
        normalize(&classify_frame(frame).unwrap()).unwrap()
    }

    #[test]
    fn maps_turn_text_diff_usage_and_completion() {
        assert_eq!(
            normalized(json!({"method":"turn/started","params":{"turn":{"id":"turn-1"}}})),
            NormalizedEvent::TurnStarted {
                native_turn_id: Some("turn-1".into())
            }
        );
        assert_eq!(
            normalized(json!({
                "method":"item/agentMessage/delta",
                "params":{"itemId":"item-1","delta":"hello"}
            })),
            NormalizedEvent::TextDelta {
                item_id: Some("item-1".into()),
                text: "hello".into()
            }
        );
        assert!(matches!(
            normalized(json!({"method":"turn/diff/updated","params":{"diff":"--- a"}})),
            NormalizedEvent::DiffUpdated { diff } if diff == "--- a"
        ));
        assert!(matches!(
            normalized(
                json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"totalTokens":12}}})
            ),
            NormalizedEvent::UsageUpdated { .. }
        ));
        assert!(matches!(
            normalized(json!({"method":"turn/completed","params":{"turn":{"status":"completed","error":null}}})),
            NormalizedEvent::TurnCompleted { outcome: Some(outcome), error: None } if outcome == "completed"
        ));
    }

    #[test]
    fn preserves_interactions_and_unknown_extensions() {
        let approval = normalized(json!({
            "id":"approval-1",
            "method":"item/commandExecution/requestApproval",
            "params":{"threadId":"thread-1","itemId":"item-1"}
        }));
        assert!(matches!(
            approval,
            NormalizedEvent::InteractionRequested { native_request_id, method, .. }
                if native_request_id == json!("approval-1")
                    && method == "item/commandExecution/requestApproval"
        ));

        let extension = normalized(json!({
            "method":"future/newEvent",
            "params":{"newField":{"nested":true}}
        }));
        assert!(matches!(
            extension,
            NormalizedEvent::VendorExtension { method, payload }
                if method == "future/newEvent" && payload["newField"]["nested"] == true
        ));
    }
}
