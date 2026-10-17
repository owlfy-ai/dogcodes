use driver_api::usage::UsageRequest;
use serde_json::Value;

pub(super) fn parse(message: &Value) -> Option<UsageRequest> {
    if message["role"] != "assistant"
        || matches!(message["stopReason"].as_str(), Some("error" | "aborted"))
    {
        return None;
    }
    let usage = &message["usage"];
    let id = message["responseId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            message["timestamp"]
                .as_u64()
                .map(|t| format!("timestamp:{t}"))
        })?;
    let read = usage["cacheRead"].as_u64();
    let write = usage["cacheWrite"].as_u64();
    Some(UsageRequest {
        request_id: id,
        model: message["model"].as_str().map(str::to_owned),
        source: "pi.message_end".into(),
        input_tokens: usage["input"]
            .as_u64()
            .and_then(|n| n.checked_add(read?)?.checked_add(write?)),
        output_tokens: usage["output"].as_u64(),
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: usage["reasoning"].as_u64(),
        cost_usd: usage["cost"]["total"].as_f64(),
        ..UsageRequest::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cache_is_included_once_and_failed_messages_are_not_requests() {
        let mut m = json!({"role":"assistant","responseId":"r","model":"m","usage":{"input":10,"cacheRead":90,"cacheWrite":0,"output":20,"reasoning":15}});
        let r = parse(&m).unwrap();
        assert_eq!(r.input_tokens, Some(100));
        assert_eq!(r.output_tokens, Some(20));
        m["stopReason"] = json!("error");
        assert!(parse(&m).is_none());
    }
}
