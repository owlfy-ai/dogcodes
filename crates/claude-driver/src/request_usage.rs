use driver_api::usage::{UsageRequest, UsageStore};
use serde_json::Value;

#[derive(Default)]
pub(super) struct Requests(
    std::collections::HashMap<String, (Value, bool, Option<std::time::Instant>)>,
);
impl Requests {
    pub fn observe(
        &mut self,
        raw: &Value,
        store: &UsageStore,
        session: &str,
    ) -> Option<UsageRequest> {
        let event = &raw["event"];
        let scope = raw["parent_tool_use_id"].as_str().unwrap_or("");
        if raw["type"] != "stream_event" {
            return None;
        }
        match event["type"].as_str() {
            Some("message_start") => {
                self.0
                    .insert(scope.into(), (event["message"].clone(), false, None));
            }
            Some("content_block_delta" | "content_block_start") => {
                let output = [
                    &event["delta"]["text"],
                    &event["delta"]["thinking"],
                    &event["delta"]["partial_json"],
                    &event["content_block"]["text"],
                    &event["content_block"]["thinking"],
                ]
                .into_iter()
                .any(|v| v.as_str().is_some_and(|s| !s.is_empty()));
                if output && let Some((_, _, start)) = self.0.get_mut(scope) {
                    start.get_or_insert_with(std::time::Instant::now);
                }
            }
            Some("message_delta") => {
                if let Some((message, final_usage, _)) = self.0.get_mut(scope)
                    && let Some(usage) = event["usage"].as_object()
                {
                    *final_usage |= usage.get("output_tokens").is_some_and(Value::is_u64);
                    for (key, value) in usage {
                        message["usage"][key] = value.clone();
                    }
                }
            }
            Some("message_stop") => {
                if let Some((message, true, started)) = self.0.remove(scope)
                    && let Some(mut request) = parse(&message)
                {
                    request.generation_ms = started
                        .and_then(|s| u64::try_from(s.elapsed().as_millis()).ok())
                        .filter(|ms| *ms > 0);
                    store.observe_request(session, request.clone());
                    // Background children can outlive the root and cannot be assigned to
                    // whichever turn happens to be active when their result arrives.
                    return scope.is_empty().then_some(request);
                }
            }
            _ => {}
        }
        None
    }
}
fn parse(message: &Value) -> Option<UsageRequest> {
    let model = message["model"].as_str()?;
    if model == "<synthetic>" {
        return None;
    }
    let u = &message["usage"];
    let read = u
        .get("cache_read_input_tokens")
        .map_or(Some(0), Value::as_u64);
    let write = u
        .get("cache_creation_input_tokens")
        .map_or(Some(0), Value::as_u64);
    Some(UsageRequest {
        request_id: message["id"].as_str()?.into(),
        model: Some(model.into()),
        source: "claude.stream".into(),
        input_tokens: u["input_tokens"]
            .as_u64()?
            .checked_add(read?)?
            .checked_add(write?),
        output_tokens: u["output_tokens"].as_u64(),
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: u["output_tokens_details"]["thinking_tokens"].as_u64(),
        ..UsageRequest::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn measures_generation_after_first_output_and_emits_only_settled_root_requests() {
        let store = UsageStore::default();
        let mut requests = Requests::default();
        requests.observe(&json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"root","model":"m","usage":{"input_tokens":10}}}}), &store,"s");
        assert!(requests.0[""].2.is_none());
        requests.observe(&json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"thinking":"not persisted"}}}), &store,"s");
        assert!(requests.0[""].2.is_some());
        requests.0.get_mut("").unwrap().2 =
            std::time::Instant::now().checked_sub(std::time::Duration::from_millis(150));
        requests.observe(&json!({"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":30}}}), &store,"s");
        let request = requests
            .observe(
                &json!({"type":"stream_event","event":{"type":"message_stop"}}),
                &store,
                "s",
            )
            .unwrap();
        assert!(request.generation_ms.unwrap() >= 150);
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains("not persisted")
        );
        assert!(
            requests
                .observe(
                    &json!({"type":"stream_event","event":{"type":"message_stop"}}),
                    &store,
                    "s"
                )
                .is_none()
        );
    }
    #[test]
    fn child_stream_does_not_replace_pending_root_usage() {
        let store = UsageStore::default();
        let mut requests = Requests::default();
        for (scope, id) in [("", "root"), ("tool-child", "child")] {
            requests.observe(&json!({"type":"stream_event","parent_tool_use_id":scope,"event":{"type":"message_start","message":{"id":id,"model":"m","usage":{"input_tokens":10,"output_tokens":3}}}}),&store,"s");
        }
        for scope in ["tool-child", ""] {
            requests.observe(&json!({"type":"stream_event","parent_tool_use_id":scope,"event":{"type":"message_delta","usage":{"output_tokens":3}}}),&store,"s");
            requests.observe(&json!({"type":"stream_event","parent_tool_use_id":scope,"event":{"type":"message_stop"}}),&store,"s");
        }
        let records = store.get(Some("s")).unwrap().request_usage.unwrap().records;
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.request_id == "root"));
    }
    #[test]
    fn only_settled_stream_counts_and_duplicate_message_is_replaced() {
        let store = UsageStore::default();
        let mut requests = Requests::default();
        for _ in 0..2 {
            requests.observe(&json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m","model":"claude","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":1}}}}), &store,"s");
            requests.observe(&json!({"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":20}}}), &store,"s");
            requests.observe(
                &json!({"type":"stream_event","event":{"type":"message_stop"}}),
                &store,
                "s",
            );
        }
        let r = store.get(Some("s")).unwrap().request_usage.unwrap();
        assert_eq!(r.records.len(), 1);
        assert_eq!(r.records[0].input_tokens, Some(100));
        assert_eq!(r.records[0].output_tokens, Some(20));
    }
}
