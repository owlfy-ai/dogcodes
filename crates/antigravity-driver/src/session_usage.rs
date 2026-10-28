//! Read context metadata from this session's own local language server.
use super::SessionProcess;
use driver_api::usage::{SessionUsageSnapshot, token};
use serde_json::{Value, json};
use std::io::Read;

/// CLI result.usage is cumulative, while step usage describes a request.
/// Neither input counter is an authoritative context-window measurement.
pub(super) fn observe(previous: &mut Option<SessionUsageSnapshot>, event: &Value) {
    let (payload, scope) = match event["event"].as_str() {
        Some("result") => (&event["result"], "session"),
        Some("step_update") if event["step_update"]["step_type"] == "agent_response" => {
            (&event["step_update"], "request")
        }
        _ => return,
    };
    let mut next = driver_api::usage::antigravity(&payload["usage"]);
    if !next.has_data() {
        return;
    }
    next.totals_scope = Some(scope.into());
    let mut report = previous
        .as_ref()
        .and_then(|s| s.request_usage.clone())
        .unwrap_or_default();
    if scope == "request"
        && payload["state"] == "DONE"
        && let (Some(conversation), Some(step)) = (
            payload["conversation_id"].as_str(),
            payload["step_index"].as_u64(),
        )
    {
        report.observe(driver_api::usage::UsageRequest {
            request_id: format!("{conversation}:{step}"),
            source: "antigravity.step".into(),
            input_tokens: next.input_tokens,
            output_tokens: next.output_tokens,
            cache_read_tokens: next.cache_read_tokens,
            reasoning_tokens: next.reasoning_tokens,
            ..driver_api::usage::UsageRequest::default()
        });
    }
    next.request_usage = Some(report);
    if scope == "session"
        && let Some(old) = previous.as_ref()
        && next.context_used_tokens.is_none()
        && next.context_window_tokens.is_none()
    {
        next.context_used_tokens = old.context_used_tokens;
        next.context_window_tokens = old.context_window_tokens;
    }
    *previous = Some(next);
}

fn port(log: &str) -> Option<u16> {
    log.split("Language server listening on random port at ")
        .skip(1)
        .find_map(|tail| {
            let (port, _) = tail.split_once(" for HTTPS (gRPC)")?;
            port.parse::<u16>().ok().filter(|p| *p > 0)
        })
}
fn context(value: &Value) -> Option<SessionUsageSnapshot> {
    let trajectory = value.get("trajectory").unwrap_or(value);
    let metadata = trajectory
        .pointer("/generatorMetadata/0/chatModel/chatStartMetadata/contextWindowMetadata")?;
    let used = token(metadata, &["estimatedTokensUsed", "estimated_tokens_used"]).or_else(|| {
        token(
            &metadata["tokenBreakdown"],
            &["totalTokens", "total_tokens"],
        )
    });
    let window = token(metadata, &["maxContextTokens", "max_context_tokens"]).filter(|n| *n > 0);
    // No guessed model capacities. The native estimate can be shown on its own.
    used.map(|used| SessionUsageSnapshot {
        context_used_tokens: Some(used),
        context_window_tokens: window,
        ..SessionUsageSnapshot::new()
    })
}
pub(super) async fn read(process: &SessionProcess) -> Option<SessionUsageSnapshot> {
    let id = process.native_id.lock().ok()?.clone()?;
    let path = process.usage_log.as_ref()?.path().join("session.log");
    let log = tokio::task::spawn_blocking(move || {
        let mut log = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(512 * 1024)
            .read_to_string(&mut log)
            .ok()?;
        Some(log)
    })
    .await
    .ok()??;
    let port = port(&log)?;
    // Self-signed TLS is accepted only for the owned loopback service. No proxy,
    // redirects, credentials, or model prompt; bounded time and response size.
    let mut command = tokio::process::Command::new("curl");
    command
        .args(["--disable", "--silent", "--fail", "--insecure", "--noproxy", "*", "--max-time", "2", "--max-filesize", "2097152", "--proto", "=https", "-H", "Content-Type: application/json", "--data-binary"])
        .arg(json!({"cascadeId":id,"generatorMetadataOffset":0,"includeMessages":false}).to_string())
        .arg(format!("https://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/GetCascadeTrajectoryGeneratorMetadata"))
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let output = agent_runtime::OwnedChild::spawn(command)
        .ok()?
        .wait_with_output(2 * 1024 * 1024)
        .await
        .ok()?;
    if !output.status.success() || output.stdout.len() > 2 * 1024 * 1024 {
        return None;
    }
    context(&serde_json::from_slice::<Value>(&output.stdout).ok()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_step_and_session_result_do_not_double_count() {
        let mut state = None;
        let event = json!({"event":"step_update","step_update":{"conversation_id":"s","step_index":1,"state":"DONE","step_type":"agent_response","usage":{"input_tokens":100,"output_tokens":10,"thinking_tokens":8}}});
        observe(&mut state, &event);
        observe(&mut state, &event);
        observe(
            &mut state,
            &json!({"event":"result","result":{"usage":{"input_tokens":100,"output_tokens":10}}}),
        );
        let report = state.unwrap().request_usage.unwrap();
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].request_id, "s:1");
        assert_eq!(report.records[0].output_tokens, Some(10));
    }
    #[test]
    fn cumulative_input_is_never_context_occupancy() {
        let mut state = None;
        observe(
            &mut state,
            &json!({"event":"result","result":{"usage":{"input_tokens":81239,"output_tokens":488,"cache_read_tokens":50000,"total_tokens":81727}}}),
        );
        let s = state.unwrap();
        assert_eq!(s.context_used_tokens, None);
        assert_eq!(s.context_window_tokens, None);
        assert_eq!(s.input_tokens, Some(81239));
        assert_eq!(s.cache_read_tokens, Some(50000));
        assert_eq!(s.totals_scope.as_deref(), Some("session"));
    }
    #[test]
    fn result_preserves_only_explicit_context_from_latest_request() {
        let mut state = None;
        observe(
            &mut state,
            &json!({"event":"step_update","step_update":{"step_type":"agent_response","usage":{"input_tokens":12,"context_used_tokens":1000,"context_window_tokens":200_000}}}),
        );
        observe(
            &mut state,
            &json!({"event":"result","result":{"usage":{"input_tokens":100_000,"total_tokens":101_000}}}),
        );
        let s = state.unwrap();
        assert_eq!(s.context_used_tokens, Some(1000));
        assert_eq!(s.context_window_tokens, Some(200_000));
        assert_eq!(s.total_tokens, Some(101_000));
    }
    #[test]
    fn accepts_only_native_capacity_and_session_https_port() {
        assert_eq!(
            port("Language server listening on random port at 12345 for HTTPS (gRPC)"),
            Some(12345)
        );
        assert_eq!(
            port("Language server listening on random port at 0 for HTTPS (gRPC)"),
            None
        );
        let s = context(&json!({"generatorMetadata":[{"chatModel":{"chatStartMetadata":{"contextWindowMetadata":{"estimatedTokensUsed":500}}}}]})).unwrap();
        assert_eq!(s.context_used_tokens, Some(500));
        assert_eq!(s.context_window_tokens, None);
    }
}
