//! Only native error codes determine authentication state, never message text.
use driver_api::DriverEvent;
use serde_json::Value;

const AUTH_FAILURE: &str =
    "Claude Code 身份验证失败，请检查原生客户端登录状态、API Key 或服务权限后重试。";
const EXECUTION_FAILURE: &str = "Claude Code turn ended with an error result";

fn authentication_code(code: &str) -> bool {
    matches!(code, "authentication_failed" | "oauth_org_not_allowed")
}

pub fn result_failure(event: &Value) -> String {
    if ["error_code", "errorCode", "error_type"]
        .iter()
        .any(|key| event[*key].as_str().is_some_and(authentication_code))
    {
        AUTH_FAILURE.into()
    } else {
        EXECUTION_FAILURE.into()
    }
}

#[derive(Default)]
pub struct TurnErrors {
    authentication: bool,
    assistant_error: bool,
}
impl TurnErrors {
    pub fn observe(&mut self, event: &Value) {
        if event["type"] == "assistant"
            && event["parent_tool_use_id"]
                .as_str()
                .is_none_or(str::is_empty)
            && let Some(code) = event["error"].as_str().filter(|code| !code.is_empty())
        {
            self.assistant_error = true;
            self.authentication |= authentication_code(code);
        }
    }
    pub fn finish(&mut self, event: DriverEvent) -> DriverEvent {
        let errors = std::mem::take(self);
        if errors.authentication {
            DriverEvent::TurnFailed {
                reason: AUTH_FAILURE.into(),
            }
        } else if errors.assistant_error && matches!(event, DriverEvent::TurnCompleted { .. }) {
            DriverEvent::TurnFailed {
                reason: EXECUTION_FAILURE.into(),
            }
        } else {
            event
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn success() -> DriverEvent {
        DriverEvent::TurnCompleted {
            summary: "done".into(),
        }
    }

    #[test]
    fn native_auth_error_is_retained_until_result_and_reset_for_next_turn() {
        let mut errors = TurnErrors::default();
        for code in ["authentication_failed", "oauth_org_not_allowed"] {
            errors.observe(&json!({"type":"assistant","error":code}));
            assert!(
                matches!(errors.finish(success()), DriverEvent::TurnFailed { reason } if reason == AUTH_FAILURE)
            );
            assert!(matches!(
                errors.finish(success()),
                DriverEvent::TurnCompleted { .. }
            ));
        }
    }
    #[test]
    fn message_text_and_child_failures_do_not_change_parent_auth_state() {
        let mut errors = TurnErrors::default();
        errors.observe(&json!({"type":"assistant","message":{"content":[{"type":"text","text":"invalid api key, not logged in, unauthorized"}]}}));
        errors.observe(&json!({"type":"assistant","parent_tool_use_id":"child","error":"authentication_failed"}));
        assert!(matches!(
            errors.finish(success()),
            DriverEvent::TurnCompleted { .. }
        ));
        errors.observe(&json!({"type":"assistant","error":"unknown_native_error"}));
        assert!(
            matches!(errors.finish(success()), DriverEvent::TurnFailed { reason } if reason == EXECUTION_FAILURE)
        );
    }
}
