//! Preserve native account failures while discarding vendor text and credentials.
use driver_api::{DriverError, explicitly_reports_quota_exhaustion, failure};
use serde_json::Value;

pub(super) fn map(error: &Value) -> DriverError {
    // JSON-RPC errors may wrap the typed TurnError in data.
    let candidates = [error, &error["data"], &error["data"]["error"]];
    let codes = candidates.iter().flat_map(|value| {
        ["code", "type", "codexErrorInfo"]
            .into_iter()
            .filter_map(|field| value[field].as_str())
    });
    let messages = candidates.iter().flat_map(|value| {
        ["message", "reason"]
            .into_iter()
            .filter_map(|field| value[field].as_str())
    });
    // Native UsageNotIncluded also maps to usageLimitExceeded. An explicit
    // plan requirement must take precedence over that shared umbrella code.
    if codes.clone().any(|code| {
        matches!(
            code,
            "subscription_required" | "subscription_expired" | "usage_not_included"
        )
    }) || messages
        .clone()
        .any(|message| failure::classify(message) == Some("subscription_required"))
    {
        return DriverError::NotConfigured("subscription_required".into());
    }
    if codes.clone().any(|code| {
        matches!(
            code,
            "usageLimitExceeded" | "usage_limit_reached" | "quota_exceeded" | "insufficient_quota"
        )
    }) || messages.clone().any(explicitly_reports_quota_exhaustion)
    {
        return DriverError::QuotaExhausted {
            resets_at_unix: candidates.iter().find_map(|value| {
                value["resetsAt"]
                    .as_i64()
                    .or_else(|| value["reset_at"].as_i64())
            }),
        };
    }
    let explicit_kind = codes.clone().find_map(|code| match code {
        "unauthorized" | "authentication_failed" | "auth_required" => Some("authentication_failed"),
        "subscription_required" | "subscription_expired" => Some("subscription_required"),
        "access_denied" | "forbidden" => Some("access_denied"),
        "rateLimitExceeded" | "rate_limit_exceeded" => Some("agent_rate_limited"),
        _ => None,
    });
    let from_message = messages.filter_map(failure::classify).find(|kind| {
        matches!(
            *kind,
            "authentication_failed" | "subscription_required" | "access_denied"
        )
    });
    let http_status = candidates.iter().find_map(|value| {
        value["httpStatusCode"].as_u64().or_else(|| {
            [
                "httpConnectionFailed",
                "responseStreamConnectionFailed",
                "responseStreamDisconnected",
                "responseTooManyFailedAttempts",
            ]
            .into_iter()
            .find_map(|field| value["codexErrorInfo"][field]["httpStatusCode"].as_u64())
        })
    });
    match explicit_kind.or(from_message).or(match http_status {
        Some(401) => Some("authentication_failed"),
        Some(403) => Some("access_denied"),
        Some(429) => Some("agent_rate_limited"),
        _ => None,
    }) {
        Some(kind @ ("authentication_failed" | "subscription_required" | "access_denied")) => {
            DriverError::NotConfigured(kind.into())
        }
        Some(kind) => DriverError::Protocol(kind.into()),
        None => DriverError::Protocol("codex rejected a protocol request".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_account_errors_without_retaining_native_text() {
        for (error, expected) in [
            (
                json!({"code":"usage_limit_reached", "message":"private-token"}),
                "quota_exhausted",
            ),
            (
                json!({"code":-32000,"message":"The usage limit has been reached"}),
                "quota_exhausted",
            ),
            (
                json!({"data":{"codexErrorInfo":"usageLimitExceeded", "resetsAt":1_800_000_000}}),
                "quota_exhausted",
            ),
            (
                json!({"message":"subscription required private-token"}),
                "subscription_required",
            ),
            (
                json!({"data":{"codexErrorInfo":"usageLimitExceeded"},"message":"To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus."}),
                "subscription_required",
            ),
            (
                json!({"data":{"error":{"codexErrorInfo":"unauthorized"}}, "message":"private-token"}),
                "authentication_failed",
            ),
            (
                json!({"data":{"codexErrorInfo":{"httpConnectionFailed":{"httpStatusCode":403}}}}),
                "access_denied",
            ),
            (
                json!({"data":{"codexErrorInfo":"rateLimitExceeded"}}),
                "agent_rate_limited",
            ),
            (
                json!({"message":"HTTP 429 rate limit; retry in 2 seconds"}),
                "driver_protocol_error",
            ),
            (
                json!({"message":"unknown private-token"}),
                "driver_protocol_error",
            ),
            (
                json!({"data":{"codexErrorInfo":"sessionBudgetExceeded"}}),
                "driver_protocol_error",
            ),
        ] {
            let mapped = map(&error);
            assert_eq!(failure::kind(&mapped), expected);
            assert!(!mapped.to_string().contains("private-token"));
        }
        assert!(matches!(
            map(&json!({"data":{"codexErrorInfo":"usageLimitExceeded", "reset_at":1_800_000_000}})),
            DriverError::QuotaExhausted {
                resets_at_unix: Some(1_800_000_000)
            }
        ));
    }
}
