//! Classify native errors without retaining vendor text, paths or credentials.
use crate::DriverError;

#[must_use]
pub fn classify(message: &str) -> Option<&'static str> {
    // Adapters may already have reduced a native error to one of these codes.
    for kind in [
        "selection_rejected",
        "selection_unverifiable",
        "authentication_failed",
        "subscription_required",
        "access_denied",
        "agent_rate_limited",
        "model_unavailable",
        "database_incompatible",
        "agent_timeout",
        "agent_connection_failed",
        "agent_no_response",
        "driver_not_configured",
        "driver_disconnected",
        "driver_protocol_error",
        "driver_capability_unavailable",
        "target_session_missing",
        "target_session_incompatible",
    ] {
        if message == kind || message == user_message(kind) {
            return Some(kind);
        }
    }
    let text = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if has(&[
        "subscription_required",
        "subscription required",
        "subscription expired",
        "no active subscription",
        "not subscribed",
        "to use codex with your chatgpt plan, upgrade to",
    ]) {
        Some("subscription_required")
    } else if has(&[
        "not eligible",
        "eligibility check failed",
        "access denied",
        "access_denied",
    ]) {
        Some("access_denied")
    } else if has(&[
        "authentication_failed",
        "authentication_error",
        "failed to authenticate",
        "authentication failed",
        "authentication is missing",
        "authentication required",
        "authentication_required",
        "authentication is required",
        "auth_required",
        "invalid api key",
        "unauthorized",
        "not logged in",
        "not authenticated",
        "login required",
        "login_required",
        "requires login",
        "身份验证失败",
    ]) {
        Some("authentication_failed")
    } else if text.contains("model")
        && has(&[
            "has been deprecated",
            "has been retired",
            "model not found",
            "model is unavailable",
            "model is not available",
            "model_not_found",
        ])
    {
        Some("model_unavailable")
    } else if has(&[
        "not null constraint failed: session_message.seq",
        "failed to run data migrations",
        "database_incompatible",
    ]) {
        Some("database_incompatible")
    } else if has(&[
        "keepalive ping timed out",
        "keepalive timeout",
        "http/2 stream closed",
        "http/2 stream reset",
        "socket hang up",
        "connection refused",
        "connection reset",
        "connection is closed",
        "connection closed",
        "network error",
        "fetch failed",
        "enotfound",
        "econnreset",
        "etimedout",
        "eai_again",
        "service unavailable",
    ]) {
        Some("agent_connection_failed")
    } else if has(&["timed out", "timeout", "超时"]) {
        Some("agent_timeout")
    } else if has(&["未返回任何内容或工具活动", "agent_no_response"]) {
        Some("agent_no_response")
    } else {
        None
    }
}

/// Some agents emit a transport error as plain assistant text before `end_turn`.
/// Only recognize explicit error lines outside code fences, not quoted evidence.
#[must_use]
pub fn reported_transport_error(output: &str) -> Option<&str> {
    let mut fence: Option<char> = None;
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            let Some(marker) = line.chars().next() else {
                continue;
            };
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if [
            "Error:",
            "RetriableError:",
            "ConnectError:",
            "NetworkError:",
            "TypeError:",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
            && matches!(
                classify(line),
                Some("agent_connection_failed" | "agent_timeout" | "driver_disconnected")
            )
        {
            return Some(line);
        }
    }
    None
}

#[must_use]
pub fn kind(error: &DriverError) -> &'static str {
    let (reason, fallback) = match error {
        DriverError::AutonomousTurnBusy => return "autonomous_turn_busy",
        DriverError::NotConfigured(reason) => (reason, "driver_not_configured"),
        DriverError::Disconnected(reason) => (reason, "driver_disconnected"),
        DriverError::Protocol(reason) => (reason, "driver_protocol_error"),
        DriverError::CapabilityUnavailable(_) => return "driver_capability_unavailable",
        DriverError::SessionMissing(_) => return "target_session_missing",
        DriverError::SessionIncompatible(_) => return "target_session_incompatible",
        DriverError::QuotaExhausted { .. } => return "quota_exhausted",
    };
    classify(reason).unwrap_or(fallback)
}

/// Account rejection must not be masked by a cached quota or session context.
#[must_use]
pub fn invalidates_account_quota(error: &DriverError) -> bool {
    matches!(
        kind(error),
        "authentication_failed"
            | "subscription_required"
            | "access_denied"
            | "driver_not_configured"
    )
}

/// Reduce quota lookup failures to safe UI diagnostics, preserving account state.
#[must_use]
pub fn quota_diagnostic(error: &DriverError) -> &'static str {
    if let DriverError::CapabilityUnavailable(reason) = error {
        match reason.as_str() {
            "quota_login_required" => return "quota_login_required",
            "quota_timeout" => return "quota_timeout",
            "quota_network_error" => return "quota_network_error",
            "quota_unavailable" => return "quota_unavailable",
            "claude_quota_custom_provider" => return "claude_quota_custom_provider",
            "claude_quota_login_required" => return "claude_quota_login_required",
            "claude_quota_timeout" => return "claude_quota_timeout",
            "claude_quota_transport_unavailable" => return "claude_quota_transport_unavailable",
            "claude_quota_network_error" => return "claude_quota_network_error",
            "claude_quota_rate_limited" => return "claude_quota_rate_limited",
            "claude_quota_token_expired" => return "claude_quota_token_expired",
            "claude_quota_invalid_response" => return "claude_quota_invalid_response",
            "claude_quota_no_windows" => return "claude_quota_no_windows",
            _ => {}
        }
    }
    match kind(error) {
        "authentication_failed" => "quota_login_required",
        "subscription_required" => "quota_subscription_required",
        "access_denied" => "quota_access_denied",
        "driver_not_configured" => "quota_not_configured",
        "quota_exhausted" => "quota_exhausted",
        "agent_timeout" => "quota_timeout",
        "agent_rate_limited" => "quota_rate_limited",
        "agent_connection_failed" | "driver_disconnected" => "quota_network_error",
        _ => "quota_unavailable",
    }
}

#[must_use]
pub fn user_message(kind: &str) -> &'static str {
    match kind {
        "selection_rejected" => {
            "模型或思考强度切换未生效，已恢复原选择。本条消息尚未发送，请确认选择后重试。"
        }
        "selection_unverifiable" => {
            "无法确认模型或思考强度是否生效，已停止本次发送。请重新选择后重试，必要时更新原生 Agent。"
        }
        "autonomous_turn_busy" => {
            "Agent 正在继续处理后台任务。这条消息尚未发送，请等待续答完成后重试。"
        }
        "model_unavailable" => "所选模型已下线或不可用，请在模型列表中切换模型后重试。",
        "authentication_failed" => {
            "Agent 登录或认证失败。请在原生客户端重新登录，或检查 API Key 是否有效及账号是否有访问权限，然后重试。"
        }
        "subscription_required" => {
            "当前账号没有有效订阅或套餐已到期。请在原生客户端确认订阅、切换有权限的账号或模型后重试。"
        }
        "access_denied" => {
            "当前账号没有访问所选服务或模型的权限。请在原生客户端检查账号资格、服务权限或模型配置后重试。"
        }
        "agent_rate_limited" => "Agent 请求暂时受限，请稍后重试。",
        "database_incompatible" => {
            "Agent 的本地数据与当前客户端版本不兼容。请将命令行和桌面客户端更新到兼容版本，再重新检测 Agent；无需删除历史会话。"
        }
        "agent_timeout" => "Agent 响应超时。请检查网络和原生客户端是否正常，稍后重试。",
        "agent_connection_failed" | "driver_disconnected" | "driver_stream_ended" => {
            "与 Agent 的连接已中断。请检查网络和原生客户端是否正常，再重试。"
        }
        "agent_no_response" => {
            "Agent 已结束运行，但没有返回回复。请在原生客户端确认能否正常对话，并检查客户端版本后重试。"
        }
        "driver_not_configured" => {
            "Agent 尚未配置好。请在原生客户端确认登录、模型及服务配置，然后重新检测 Agent。"
        }
        "target_session_missing" => {
            "Agent 中的原会话已不存在。请重新选择 Agent，并通过上下文交接继续对话。"
        }
        "target_session_incompatible"
        | "driver_capability_unavailable"
        | "capability_unavailable" => {
            "Agent 无法继续原会话或不支持本次操作。请重新选择 Agent，并通过上下文交接继续对话。"
        }
        "driver_protocol_error" => {
            "Agent 返回了无法处理的响应。请检查原生客户端能否正常对话，更新客户端后重新检测 Agent。"
        }
        "driver_unavailable" => "当前 Agent 尚未连接。请在“连接 Agent”中重新检测后重试。",
        "interrupted" => "本次执行已停止，可以发送消息继续。",
        _ => {
            "Agent 未能完成本次请求，未提供可识别的具体原因。请先确认原生客户端能否正常对话；仍有问题时可查看“帮助与恢复”中的诊断信息。"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_text_is_not_confused_with_a_review_of_error_handling() {
        let error =
            "Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms";
        assert_eq!(classify(error), Some("agent_connection_failed"));
        assert_eq!(
            reported_transport_error(&format!("Checking callers.\n\n{error}")),
            Some(error)
        );
        for quoted in [
            format!("```text\n{error}\n```"),
            format!("> {error}"),
            format!("The test should handle `{error}`."),
            json_error(error),
        ] {
            assert!(reported_transport_error(&quoted).is_none());
        }
        assert!(reported_transport_error("Error: invalid test fixture").is_none());
    }

    fn json_error(error: &str) -> String {
        serde_json::json!({"summary":error,"findings":[]}).to_string()
    }

    #[test]
    fn native_errors_become_fixed_categories_without_private_details() {
        for (reason, expected) in [
            (
                "Failed to authenticate. API Error: 403 secret-token",
                "authentication_failed",
            ),
            (
                "NOT NULL constraint failed: session_message.seq /private/database",
                "database_incompatible",
            ),
            ("request timed out https://private", "agent_timeout"),
            ("fetch failed private-host", "agent_connection_failed"),
            (
                "Transport channel closed, when UnexpectedServerResponse(\"HTTP 503 Service Unavailable: \")",
                "agent_connection_failed",
            ),
            ("OpenCode 未返回任何内容或工具活动", "agent_no_response"),
            (
                "Internal error: Model exo-free has been deprecated.",
                "model_unavailable",
            ),
        ] {
            assert_eq!(classify(reason), Some(expected));
            assert_eq!(kind(&DriverError::Protocol(reason.into())), expected);
            assert!(!user_message(expected).contains("private"));
            assert!(!user_message(expected).contains("secret-token"));
        }
        assert_eq!(classify("unknown private failure"), None);
        assert_eq!(classify("permission denied"), None);
        assert_eq!(
            kind(&DriverError::NotConfigured("no models".into())),
            "driver_not_configured"
        );
    }

    #[test]
    fn login_required_probe_errors_keep_login_and_quota_diagnostics() {
        for reason in [
            "agent requires login before sessions can be created",
            "Kimi quota requires login; please sign in again",
            "Authentication required; private token",
            "authentication_required",
            "authentication is required",
            "LOGIN REQUIRED",
            "login_required",
            "not authenticated",
        ] {
            let error = DriverError::NotConfigured(reason.into());
            assert_eq!(kind(&error), "authentication_failed");
            assert_eq!(quota_diagnostic(&error), "quota_login_required");
            assert!(invalidates_account_quota(&error));
        }
        assert_eq!(
            kind(&DriverError::NotConfigured("no models configured".into())),
            "driver_not_configured"
        );
    }

    #[test]
    fn quota_diagnostics_preserve_categories_without_provider_text() {
        for (error, expected) in [
            (
                DriverError::NotConfigured("authentication_failed".into()),
                "quota_login_required",
            ),
            (
                DriverError::NotConfigured("subscription_required".into()),
                "quota_subscription_required",
            ),
            (
                DriverError::NotConfigured("access_denied".into()),
                "quota_access_denied",
            ),
            (
                DriverError::NotConfigured("runtime missing private-path".into()),
                "quota_not_configured",
            ),
            (
                DriverError::Disconnected("request timed out private-host".into()),
                "quota_timeout",
            ),
            (
                DriverError::Disconnected("private-host".into()),
                "quota_network_error",
            ),
            (
                DriverError::Protocol("agent_rate_limited".into()),
                "quota_rate_limited",
            ),
            (
                DriverError::CapabilityUnavailable("private-token".into()),
                "quota_unavailable",
            ),
            (
                DriverError::CapabilityUnavailable("claude_quota_custom_provider".into()),
                "claude_quota_custom_provider",
            ),
        ] {
            assert_eq!(quota_diagnostic(&error), expected);
        }
        for kind in [
            "subscription_required",
            "access_denied",
            "agent_rate_limited",
        ] {
            assert_eq!(classify(user_message(kind)), Some(kind));
        }
    }
}
