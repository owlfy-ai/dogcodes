//! Provider token cache accounting and the unmodified model catalog.
use driver_api::usage::SessionUsageSnapshot;
use serde_json::Value;

pub(super) fn read(snapshot: &Value, catalog: &Value) -> SessionUsageSnapshot {
    let runtime = &snapshot["runtime"]["contextUsage"];
    let current = &snapshot["settings"]["model"]["current"];
    let window = catalog
        .pointer("/settings/model/available")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|row| {
            current["providerId"].is_string()
                && current["modelId"].is_string()
                && row["ref"]["providerId"] == current["providerId"]
                && row["ref"]["modelId"] == current["modelId"]
        })
        .and_then(|row| row["contextWindow"].as_u64())
        .or_else(|| {
            // Older native workspace/readState exposes the provider catalog.
            catalog
                .pointer("/modelCatalog/providers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|provider| {
                    current["providerId"].is_string()
                        && provider["providerId"] == current["providerId"]
                })
                .and_then(|provider| provider["models"].as_array())
                .into_iter()
                .flatten()
                .find(|model| {
                    current["modelId"].is_string() && model["modelId"] == current["modelId"]
                })
                .and_then(|model| model["contextWindow"].as_u64())
        })
        .or_else(|| snapshot["projection"]["contextWindow"].as_u64())
        .filter(|window| *window > 0);
    let cache = &runtime["cache"];
    SessionUsageSnapshot {
        request_usage: Some(requests(snapshot)),
        context_used_tokens: runtime["used"]
            .as_u64()
            .or_else(|| snapshot["projection"]["contextUsed"].as_u64()),
        context_window_tokens: window,
        cache_read_tokens: cache["totalCacheReadTokens"].as_u64(),
        cache_write_tokens: cache["totalCacheWriteTokens"].as_u64(),
        cache_hit_rate_percent: cache["latestHitRate"]
            .as_f64()
            .filter(|rate| rate.is_finite() && (0.0..=1.0).contains(rate))
            .map(|rate| rate * 100.0),
        ..SessionUsageSnapshot::new()
    }
}

/// Native request ledger, independent of stable message-projection identities.
pub(super) fn debug_requests(
    debug: &Value,
    session: &str,
) -> Option<driver_api::usage::RequestUsageReport> {
    use driver_api::usage::{RequestUsageReport, UsageRequest};
    if debug["sessionId"] != session {
        return None;
    }
    let rounds = debug["rounds"].as_array()?;
    let mut report = RequestUsageReport::default();
    for round in rounds {
        let Some(id) = round["requestId"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let network = debug["networkEntries"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| {
                entry["requestId"] == id
                    && entry["statusType"] == "model_request_completed"
                    && entry["querySource"] == "main_turn"
            });
        let Some(network) = network else {
            continue;
        };
        let tokens = &round["usage"];
        let (Some(input), Some(output), Some(total)) = (
            tokens["inputTokens"].as_u64(),
            tokens["outputTokens"].as_u64(),
            tokens["totalTokens"].as_u64(),
        ) else {
            continue;
        };
        if input.checked_add(output) != Some(total) {
            continue;
        }
        report.observe(UsageRequest {
            request_id: id.into(),
            model: network["modelId"].as_str().map(str::to_owned),
            source: "zcode.session.debug".into(),
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: tokens["cachedInputTokens"].as_u64(),
            cache_write_tokens: tokens["cachedWriteInputTokens"].as_u64(),
            reasoning_tokens: tokens["reasoningTokens"].as_u64(),
            generation_ms: round["generationDurationMs"]
                .as_u64()
                .filter(|duration| *duration > 0),
            ..UsageRequest::default()
        });
    }
    report.truncated = rounds.len() >= 200;
    Some(report)
}

fn requests(snapshot: &Value) -> driver_api::usage::RequestUsageReport {
    use driver_api::usage::{RequestUsageReport, UsageRequest};
    let mut report = RequestUsageReport::default();
    for message in snapshot["messages"].as_array().into_iter().flatten() {
        let info = &message["info"];
        if info["role"] != "assistant"
            || info["synthetic"] == true
            || info["semantics"]["origin"] == "system"
            || !info["time"]["completed"].is_number()
        {
            continue;
        }
        let Some(id) = info["messageId"].as_str() else {
            continue;
        };
        let tokens = &info["tokens"];
        let (Some(input), Some(output), Some(total)) = (
            tokens["input"].as_u64(),
            tokens["output"].as_u64(),
            tokens["total"].as_u64(),
        ) else {
            continue;
        };
        if input.checked_add(output) != Some(total) {
            continue;
        }
        report.observe(UsageRequest {
            request_id: id.into(),
            model: info["model"]["modelId"].as_str().map(str::to_owned),
            source: "zcode.session.messages".into(),
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: tokens["cache"]["read"].as_u64(),
            cache_write_tokens: tokens["cache"]["write"].as_u64(),
            reasoning_tokens: tokens["reasoning"].as_u64(),
            ..UsageRequest::default()
        });
    }
    if report.records.is_empty() {
        report.unavailable_reason = Some("原生快照未提供已完成请求的用量".into());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn debug_requests_are_bound_to_the_native_main_session_and_request() {
        let debug = json!({"sessionId":"session","rounds":[{"requestId":"request","generationDurationMs":100,"usage":{"inputTokens":100,"outputTokens":10,"totalTokens":110,"reasoningTokens":2}}],"networkEntries":[{"requestId":"request","statusType":"model_request_completed","querySource":"main_turn","modelId":"native-model"}]});
        let records = debug_requests(&debug, "session").unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "request");
        assert_eq!(records[0].generation_ms, Some(100));
        assert_eq!(records[0].model.as_deref(), Some("native-model"));
        assert!(debug_requests(&debug, "another").is_none());
        let mut child = debug;
        child["networkEntries"][0]["querySource"] = json!("subagent");
        assert!(
            debug_requests(&child, "session")
                .unwrap()
                .records
                .is_empty()
        );
        assert!(
            debug_requests(&json!({"sessionId":"session","rounds":[]}), "session")
                .unwrap()
                .records
                .is_empty()
        );
    }
    #[test]
    fn request_projection_requires_settlement_and_verified_token_identity() {
        let m = json!({"info":{"messageId":"r","role":"assistant","time":{"completed":1},"tokens":{"input":100,"output":10,"total":110,"cache":{"read":90},"reasoning":2}}});
        let report = requests(&json!({"messages":[m.clone(),m.clone()]}));
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].input_tokens, Some(100));
        let mut bad = m;
        bad["info"]["tokens"]["total"] = json!(200);
        assert!(requests(&json!({"messages":[bad]})).records.is_empty());
    }
    #[test]
    fn model_switch_uses_catalog_and_native_token_accounting() {
        let mut snapshot = json!({"settings":{"model":{"current":{"providerId":"p","modelId":"large"}}},
            "projection":{"contextUsed":10,"contextWindow":200_000},
            "runtime":{"contextUsage":{"used":1234,"cache":{"totalCacheReadTokens":900,"totalCacheWriteTokens":100,"latestHitRate":0.75}}},
            "cacheStats":{"totalMessages":8,"cachedMessages":1}});
        let catalog = json!({"settings":{"model":{"available":[
            {"ref":{"providerId":"p","modelId":"large"},"contextWindow":1_000_000},
            {"ref":{"providerId":"p","modelId":"small"},"contextWindow":64_000}]}}});
        let usage = read(&snapshot, &catalog);
        assert_eq!(usage.context_window_tokens, Some(1_000_000));
        assert_eq!(usage.context_used_tokens, Some(1234));
        assert_eq!(usage.cache_read_tokens, Some(900));
        assert_eq!(usage.cache_write_tokens, Some(100));
        assert_eq!(usage.cache_hit_rate_percent, Some(75.0));
        snapshot["settings"]["model"]["current"]["modelId"] = json!("small");
        assert_eq!(
            read(&snapshot, &catalog).context_window_tokens,
            Some(64_000)
        );
    }
    #[test]
    fn absent_or_invalid_cache_never_uses_message_counts() {
        let mut snapshot = json!({"projection":{"contextUsed":0,"contextWindow":200_000},"cacheStats":{"totalMessages":8,"cachedMessages":6}});
        let usage = read(&snapshot, &Value::Null);
        assert_eq!(usage.context_used_tokens, Some(0));
        assert_eq!(usage.cache_hit_rate_percent, None);
        assert_eq!(usage.cache_read_tokens, None);
        snapshot["runtime"] = json!({"contextUsage":{"cache":{"latestHitRate":1.5}}});
        assert_eq!(read(&snapshot, &Value::Null).cache_hit_rate_percent, None);
    }
}
