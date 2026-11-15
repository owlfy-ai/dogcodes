//! Native error codes establish failure; only then refine known authentication diagnostics.
use driver_api::DriverEvent;
use serde_json::Value;

const AUTH_FAILURE: &str = "backendText.me34dd651";
const EXECUTION_FAILURE: &str = "Claude Code turn ended with an error result";
fn authentication_code(code: &str) -> bool {
    matches!(code, "authentication_failed" | "oauth_org_not_allowed")
}
fn root(event: &Value) -> bool {
    event["type"] == "assistant"
        && event["parent_tool_use_id"]
            .as_str()
            .is_none_or(str::is_empty)
}
fn native_text(event: &Value) -> String {
    event["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            (block["type"] == "text")
                .then(|| block["text"].as_str())
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub(super) fn quota_failure(event: &Value) -> Option<&'static str> {
    if !root(event) {
        return None;
    }
    let code = event["error"].as_str()?;
    if code == "oauth_org_not_allowed" {
        return Some("claude_quota_access_denied");
    }
    if code == "authentication_failed" {
        let text = native_text(event);
        return Some(
            if text.starts_with("Failed to authenticate: this login reached its time limit")
                || text.starts_with(
                    "Failed to authenticate: OAuth session expired and could not be refreshed",
                )
            {
                "claude_quota_login_expired"
            } else {
                "claude_quota_login_required"
            },
        );
    }
    (code == "server_error"
        && native_text(event)
            .starts_with("Failed to refresh OAuth token: another Claude Code process"))
    .then_some("claude_quota_refresh_busy")
}
pub(super) fn control_failure(response: &Value) -> &'static str {
    let text = response["error"].as_str().unwrap_or("");
    if text.starts_with("Failed to authenticate: this login reached its time limit")
        || text
            .starts_with("Failed to authenticate: OAuth session expired and could not be refreshed")
    {
        return "claude_quota_login_expired";
    }
    if text.starts_with("Failed to refresh OAuth token: another Claude Code process") {
        return "claude_quota_refresh_busy";
    }
    match response["error_code"].as_str() {
        Some("authentication_failed") => "claude_quota_login_required",
        Some("oauth_org_not_allowed") => "claude_quota_access_denied",
        Some("rate_limit_error") => "claude_quota_rate_limited",
        _ if text.starts_with("Not logged in") => "claude_quota_login_required",
        _ => "claude_quota_unavailable",
    }
}
pub(super) fn successful_response(event: &Value) -> bool {
    root(event)
        && event["error"].is_null()
        && event["message"]["model"]
            .as_str()
            .is_some_and(|model| model != "<synthetic>")
        && event["message"]["usage"].is_object()
}
fn reason(code: &str) -> &'static str {
    match code {
        "claude_quota_login_expired" => "backendText.claudeLoginExpired",
        "claude_quota_refresh_busy" => "backendText.claudeRefreshBusy",
        "claude_quota_access_denied" => "backendText.claudeAccessDenied",
        _ => AUTH_FAILURE,
    }
}
pub fn result_failure(event: &Value) -> String {
    if let Some(code) = ["error_code", "errorCode", "error_type"]
        .iter()
        .filter_map(|key| event[*key].as_str())
        .find(|code| authentication_code(code))
    {
        let response = serde_json::json!({"error_code":code,"error":event["result"]});
        reason(control_failure(&response)).into()
    } else {
        EXECUTION_FAILURE.into()
    }
}

#[derive(Default)]
pub struct TurnErrors {
    authentication: Option<&'static str>,
    assistant_error: bool,
}
impl TurnErrors {
    pub fn observe(&mut self, event: &Value) {
        if root(event) && event["error"].as_str().is_some_and(|code| !code.is_empty()) {
            self.assistant_error = true;
            if let Some(code) = quota_failure(event)
                && (self.authentication.is_none() || self.authentication == Some(AUTH_FAILURE))
            {
                self.authentication = Some(reason(code));
            }
        }
    }
    pub fn finish(&mut self, event: DriverEvent) -> DriverEvent {
        let errors = std::mem::take(self);
        if let Some(reason) = errors.authentication {
            DriverEvent::TurnFailed {
                reason: reason.into(),
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
        errors.observe(&json!({"type":"assistant","error":"authentication_failed"}));
        assert!(
            matches!(errors.finish(success()), DriverEvent::TurnFailed { reason } if reason == AUTH_FAILURE)
        );
        assert!(matches!(
            errors.finish(success()),
            DriverEvent::TurnCompleted { .. }
        ));
    }
    #[test]
    fn known_native_failures_have_actionable_safe_recovery_without_replaying_the_turn() {
        for (code, text, expected) in [
            (
                "authentication_failed",
                "Failed to authenticate: this login reached its time limit and cannot be renewed. Bearer private-token",
                "backendText.claudeLoginExpired",
            ),
            (
                "authentication_failed",
                "Failed to authenticate: OAuth session expired and could not be refreshed",
                "backendText.claudeLoginExpired",
            ),
            (
                "server_error",
                "Failed to refresh OAuth token: another Claude Code process is refreshing it or exited mid-refresh",
                "backendText.claudeRefreshBusy",
            ),
            (
                "oauth_org_not_allowed",
                "private organization",
                "backendText.claudeAccessDenied",
            ),
        ] {
            let mut errors = TurnErrors::default();
            errors.observe(&json!({"type":"assistant","error":code,"message":{"content":[{"type":"text","text":text}]}}));
            assert!(
                matches!(errors.finish(success()),DriverEvent::TurnFailed { reason } if reason == expected)
            );
        }
    }
    #[test]
    fn message_text_and_child_failures_do_not_change_parent_auth_state() {
        let mut errors = TurnErrors::default();
        errors.observe(&json!({"type":"assistant","message":{"content":[{"type":"text","text":"Failed to authenticate: this login reached its time limit"}]}}));
        errors.observe(&json!({"type":"assistant","parent_tool_use_id":"child","error":"authentication_failed"}));
        assert!(matches!(
            errors.finish(success()),
            DriverEvent::TurnCompleted { .. }
        ));
        errors.observe(&json!({"type":"assistant","error":"unknown_native_error"}));
        assert!(
            matches!(errors.finish(success()),DriverEvent::TurnFailed { reason } if reason == EXECUTION_FAILURE)
        );
    }
}
