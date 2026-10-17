//! Exact upstream completions own request identities and timing. Counter checkpoints are fallback only.
use driver_api::usage::{SessionUsageSnapshot, UsageRequest};
use serde_json::Value;
use std::{collections::BTreeSet, time::Instant};

#[derive(Default)]
pub(super) struct LiveUsage {
    thread: String,
    turn: String,
    first_output: Option<Instant>,
    timing_unavailable: bool,
    raw_seen: bool,
    seen: BTreeSet<String>,
    checkpoints: Vec<UsageRequest>,
}

impl LiveUsage {
    pub fn begin(&mut self, thread: &str, turn: &str) {
        *self = Self {
            thread: thread.into(),
            turn: turn.into(),
            ..Self::default()
        };
    }
    pub fn accepts(&self, params: &Value) -> bool {
        params["threadId"]
            .as_str()
            .is_none_or(|id| id == self.thread)
            && params["turnId"]
                .as_str()
                .or_else(|| params["turn"]["id"].as_str())
                .is_none_or(|id| id == self.turn)
    }

    pub fn observe(
        &mut self,
        method: &str,
        params: &Value,
        previous: Option<&SessionUsageSnapshot>,
    ) -> Option<UsageRequest> {
        if self.turn.is_empty()
            || params["threadId"] != self.thread
            || params["turnId"] != self.turn
        {
            return None;
        }
        if method == "error" {
            // Native retry boundaries are not exposed. Preserve usage, omit uncertain timing.
            self.first_output = None;
            self.timing_unavailable = true;
        }
        if method == "thread/tokenUsage/updated"
            && let Some(record) = super::request_usage::observe(params, previous)
        {
            self.checkpoints.push(record);
        }
        let output_start = method == "item/started"
            && matches!(
                params["item"]["type"].as_str(),
                Some("agentMessage" | "reasoning" | "plan")
            );
        let output_delta = matches!(
            method,
            "item/agentMessage/delta"
                | "item/reasoning/textDelta"
                | "item/reasoning/summaryTextDelta"
                | "item/plan/delta"
        ) && params["delta"].as_str().is_some_and(|s| !s.is_empty());
        if (output_start || output_delta) && !self.timing_unavailable {
            self.first_output.get_or_insert_with(Instant::now);
        }
        if method != "rawResponse/completed" {
            return None;
        }
        self.raw_seen = true;
        let id = params["responseId"].as_str()?;
        if self.seen.contains(&format!("response:{id}")) {
            return None;
        }
        let duration = self
            .first_output
            .take()
            .and_then(|start| u64::try_from(start.elapsed().as_millis()).ok())
            .filter(|ms| *ms > 0);
        let mut record = response(&params["usage"], id, "codex.response")?;
        if !self.seen.insert(record.request_id.clone()) {
            return None;
        }
        record.generation_ms = (!self.timing_unavailable).then_some(duration).flatten();
        record.model = previous.and_then(|s| s.model.clone());
        Some(record)
    }

    pub fn finish(&mut self, model: Option<String>) -> Vec<UsageRequest> {
        let mut records = std::mem::take(&mut self.checkpoints);
        if self.raw_seen {
            records.clear();
        }
        for record in &mut records {
            record.model.clone_from(&model);
        }
        self.turn.clear();
        self.first_output = None;
        records
    }
}

pub(super) fn response(usage: &Value, id: &str, source: &str) -> Option<UsageRequest> {
    if id.is_empty() {
        return None;
    }
    let optional = |key: &str| -> Option<Option<u64>> {
        match usage.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(value) => Some(Some(value.as_u64()?)),
        }
    };
    let input = usage["inputTokens"].as_u64()?;
    let output = usage["outputTokens"].as_u64()?;
    if (input == 0 && output == 0)
        || optional("totalTokens")?.is_some_and(|n| input.checked_add(output) != Some(n))
    {
        return None;
    }
    let record = UsageRequest {
        request_id: format!("response:{id}"),
        source: source.into(),
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_tokens: optional("cachedInputTokens")?,
        cache_write_tokens: optional("cacheWriteInputTokens")?,
        reasoning_tokens: optional("reasoningOutputTokens")?,
        ..UsageRequest::default()
    };
    record.has_data().then_some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn completed(id: &str) -> Value {
        json!({"threadId":"s","turnId":"t","responseId":id,"usage":{"inputTokens":100,"outputTokens":20,"cachedInputTokens":50,"reasoningOutputTokens":10,"totalTokens":120}})
    }
    #[test]
    fn legacy_counter_fallback_waits_for_settlement_and_resets_between_turns() {
        let mut live = LiveUsage::default();
        live.begin("s", "t");
        let params = json!({"threadId":"s","turnId":"t","tokenUsage":{"total":{"inputTokens":100,"outputTokens":20},"last":{"inputTokens":100,"outputTokens":20}}});
        assert!(
            live.observe("thread/tokenUsage/updated", &params, None)
                .is_none()
        );
        let records = live.finish(Some("actual".into()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model.as_deref(), Some("actual"));
        assert!(records[0].generation_ms.is_none());
        live.begin("s", "next");
        assert!(live.finish(None).is_empty());
        assert!(!live.accepts(&json!({"threadId":"child","turnId":"next"})));
    }
    #[test]
    fn exact_completions_deduplicate_checkpoints_and_exclude_tools_and_other_threads() {
        let mut live = LiveUsage::default();
        live.begin("s", "t");
        live.observe("thread/tokenUsage/updated", &json!({"threadId":"s","turnId":"t","tokenUsage":{"total":{"inputTokens":100,"outputTokens":20},"last":{"inputTokens":100,"outputTokens":20}}}), None);
        live.observe(
            "item/started",
            &json!({"threadId":"s","turnId":"t","item":{"type":"commandExecution"}}),
            None,
        );
        let mut other = completed("other");
        other["threadId"] = json!("child");
        assert!(
            live.observe("rawResponse/completed", &other, None)
                .is_none()
        );
        assert!(
            live.observe("rawResponse/completed", &completed("r1"), None)
                .unwrap()
                .generation_ms
                .is_none()
        );
        assert!(
            live.observe("rawResponse/completed", &completed("r1"), None)
                .is_none()
        );
        live.first_output = Instant::now().checked_sub(std::time::Duration::from_millis(100));
        assert!(
            live.observe("rawResponse/completed", &completed("r2"), None)
                .unwrap()
                .generation_ms
                .unwrap()
                >= 100
        );
        assert!(live.finish(None).is_empty());
        live.begin("s", "next");
        assert!(
            live.observe("rawResponse/completed", &completed("late"), None)
                .is_none()
        );
    }
    #[test]
    fn retries_do_not_guess_timing_and_invalid_usage_is_not_zero() {
        let mut live = LiveUsage::default();
        live.begin("s", "t");
        live.first_output = Instant::now().checked_sub(std::time::Duration::from_millis(100));
        live.observe("error", &json!({"threadId":"s","turnId":"t"}), None);
        assert!(
            live.observe("rawResponse/completed", &completed("r"), None)
                .unwrap()
                .generation_ms
                .is_none()
        );
        let mut invalid = completed("bad");
        invalid["usage"]["cachedInputTokens"] = json!(101);
        assert!(
            live.observe("rawResponse/completed", &invalid, None)
                .is_none()
        );
        assert!(
            response(
                &json!({"inputTokens":1,"outputTokens":1,"reasoningOutputTokens":-1}),
                "r",
                "test"
            )
            .is_none()
        );
    }
}
