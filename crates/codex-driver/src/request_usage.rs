//! Live counter checkpoints are admissible only when their delta proves one request.
#[cfg(test)]
use driver_api::usage::RequestUsageReport;
use driver_api::usage::{SessionUsageSnapshot, UsageRequest};
use serde_json::Value;
#[cfg(test)]
fn model_from_rollout(text: &str, thread: &str, turn: &str) -> Option<String> {
    let file = tempfile::NamedTempFile::new().ok()?;
    std::fs::write(file.path(), format!("{text}\n")).ok()?;
    super::rollout_usage::Reader::default()
        .read(file.path(), thread, Some(turn))?
        .model
}
#[cfg(test)]
fn requests_from_rollout(
    text: &str,
    thread: &str,
) -> Option<(RequestUsageReport, std::collections::BTreeSet<String>)> {
    let file = tempfile::NamedTempFile::new().ok()?;
    std::fs::write(file.path(), text).ok()?;
    let value = super::rollout_usage::Reader::default().read(file.path(), thread, None)?;
    Some((value.report, value.exact_turns))
}
pub(super) fn observe(
    params: &Value,
    previous: Option<&SessionUsageSnapshot>,
) -> Option<UsageRequest> {
    let usage = &params["tokenUsage"];
    let total = &usage["total"];
    let last = &usage["last"];
    let ti = total["inputTokens"].as_u64()?;
    let to = total["outputTokens"].as_u64()?;
    let input = last["inputTokens"].as_u64()?;
    let output = last["outputTokens"].as_u64()?;
    let pi = previous.and_then(|p| p.input_tokens).unwrap_or(0);
    let po = previous.and_then(|p| p.output_tokens).unwrap_or(0);
    if ti.checked_sub(pi) != Some(input)
        || to.checked_sub(po) != Some(output)
        || (input == 0 && output == 0)
    {
        return None;
    }
    Some(UsageRequest {
        request_id: format!("checkpoint:{}:{ti}:{to}", params["turnId"].as_str()?),
        model: None,
        source: "codex.verified_counter_delta".into(),
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_tokens: last["cachedInputTokens"].as_u64(),
        reasoning_tokens: last["reasoningOutputTokens"].as_u64(),
        ..UsageRequest::default()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_response_identity_deduplicates_compaction_and_excludes_copied_threads() {
        let usage = json!({"input_tokens":100,"output_tokens":10,"cached_input_tokens":50,"reasoning_output_tokens":2});
        let request =
            json!({"thread_id":"s","session_id":"s","turn_id":"t","response_id":"r","usage":usage});
        let text = [json!({"type":"session_meta","payload":{"id":"s"}}),json!({"type":"turn_context","payload":{"turn_id":"t","model":"actual"}}),json!({"type":"token_usage_record","payload":request}),json!({"type":"compacted","payload":{"latest_token_usage_record":request}}),json!({"type":"token_usage_record","payload":{"thread_id":"parent","response_id":"copied","usage":usage}})]
            .into_iter().map(|row| row.to_string()).collect::<Vec<_>>().join("\n") + "\n";
        let (report, turns) = requests_from_rollout(&text, "s").unwrap();
        assert!(turns.contains("t"));
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].request_id, "response:r");
        assert_eq!(report.records[0].model.as_deref(), Some("actual"));
        assert!(requests_from_rollout(&text, "wrong").is_none());
    }
    #[test]
    fn native_model_is_bound_to_the_exact_thread_and_turn() {
        let text = [
            json!({"type":"session_meta","payload":{"id":"thread"}}),
            json!({"type":"turn_context","payload":{"turn_id":"old","model":"previous"}}),
            json!({"type":"turn_context","payload":{"turn_id":"current","model":"actual"}}),
        ]
        .into_iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!(
            model_from_rollout(&text, "thread", "current").as_deref(),
            Some("actual")
        );
        assert_eq!(
            model_from_rollout(&text, "thread", "old").as_deref(),
            Some("previous")
        );
        assert!(model_from_rollout(&text, "other", "current").is_none());
        assert!(model_from_rollout(&text, "thread", "missing").is_none());
    }
    #[test]
    fn cumulative_jump_or_duplicate_is_not_one_request() {
        let p = SessionUsageSnapshot {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..SessionUsageSnapshot::new()
        };
        let mut event = json!({"turnId":"t","tokenUsage":{"total":{"inputTokens":150,"outputTokens":15},"last":{"inputTokens":50,"outputTokens":5}}});
        assert!(observe(&event, Some(&p)).is_some());
        let current = driver_api::usage::codex(&event["tokenUsage"]);
        assert!(observe(&event, Some(&current)).is_none());
        event["tokenUsage"]["total"]["inputTokens"] = json!(200);
        assert!(observe(&event, Some(&p)).is_none());
    }
}
