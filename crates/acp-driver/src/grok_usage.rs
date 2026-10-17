//! Grok's ACP response events are disjoint token buckets, not turn aggregates.
use driver_api::usage::{RequestUsageReport, SessionUsageSnapshot, UsageRequest};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    time::Instant,
};

#[derive(Default)]
pub(super) struct GrokUsage {
    generation: u64,
    active: Option<Active>,
    covered: BTreeSet<(String, String)>,
    history_cost: HashMap<String, Option<f64>>,
    settled: Vec<UsageRequest>,
}
struct Active {
    session: String,
    generation: u64,
    first_output: Option<Instant>,
    stream_key: Option<Value>,
    compacting: bool,
    timing_unavailable: bool,
    seen: BTreeSet<String>,
    responses: Vec<UsageRequest>,
    last_cost_sequence: usize,
    last_cost_requested: usize,
}
#[derive(Clone)]
pub(super) struct CostQuery {
    pub session: String,
    generation: u64,
    sequence: usize,
}

impl GrokUsage {
    pub fn is_active(&self, session: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.session == session)
    }
    pub fn reset_connection(&mut self) {
        self.active = None;
        self.settled.clear();
        self.history_cost.clear();
    }
    pub fn begin(&mut self, session: &str, before: Option<&SessionUsageSnapshot>) {
        self.settled.clear();
        self.generation = self.generation.saturating_add(1);
        self.history_cost
            .entry(session.into())
            .or_insert_with(|| before.and_then(|s| s.session_cost_usd));
        self.active = Some(Active {
            session: session.into(),
            generation: self.generation,
            first_output: None,
            stream_key: None,
            compacting: false,
            timing_unavailable: false,
            seen: BTreeSet::new(),
            responses: vec![],
            last_cost_sequence: 0,
            last_cost_requested: 0,
        });
    }
    pub fn observe(&mut self, params: &Value, model: Option<String>) -> Vec<UsageRequest> {
        let Some(active) = &mut self.active else {
            return vec![];
        };
        if params["sessionId"] != active.session {
            return vec![];
        }
        let update = params.get("update").unwrap_or(params);
        if params["_meta"]["subagentId"].is_string() || update["_meta"]["subagentId"].is_string() {
            return vec![];
        }
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        if matches!(
            kind,
            "compaction_started" | "compaction_completed" | "error"
        ) {
            active.first_output = None;
            active.stream_key = None;
            active.compacting = kind == "compaction_started";
            active.timing_unavailable |= kind == "error";
        }
        if active.compacting {
            return vec![];
        }
        if matches!(
            kind,
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call_delta_chunk"
        ) {
            let text = update["content"]["text"]
                .as_str()
                .or_else(|| update["arguments_delta"].as_str());
            if text.is_some_and(|s| !s.is_empty()) && !active.timing_unavailable {
                let key = update["_meta"]
                    .get("streamStartMs")
                    .or_else(|| params["_meta"].get("streamStartMs"));
                if key.is_some() && active.stream_key.is_some() && key != active.stream_key.as_ref()
                {
                    active.first_output = None;
                }
                if let Some(key) = key {
                    active.stream_key = Some(key.clone());
                }
                active.first_output.get_or_insert_with(Instant::now);
            }
        }
        if kind == "response_completed" {
            if active.responses.len() >= 10_000 {
                return vec![];
            }
            let native_id = update["message_id"].as_str().filter(|s| !s.is_empty());
            // Signatures are used only to reject retransmissions, never as invented request IDs.
            let dedup = native_id.map(|id| format!("id:{id}")).or_else(|| {
                update["signature"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 8192)
                    .map(|s| format!("signature:{s}"))
            });
            if dedup.as_ref().is_some_and(|key| active.seen.contains(key)) {
                return vec![];
            }
            let duration = active
                .first_output
                .take()
                .and_then(|s| u64::try_from(s.elapsed().as_millis()).ok())
                .filter(|ms| *ms > 0);
            active.stream_key = None;
            let Some(mut record) = response(&update["usage"]) else {
                active.timing_unavailable = true;
                return vec![];
            };
            if let Some(key) = dedup {
                active.seen.insert(key);
            }
            record.model = update["model_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or(model);
            record.generation_ms = (!active.timing_unavailable).then_some(duration).flatten();
            if let Some(id) = native_id {
                record.request_id = format!("response:{id}");
            }
            active.responses.push(record.clone());
            // v1.0.50 omits message_id. Wait for its native prompt ID and validated call count.
            return native_id.map_or_else(Vec::new, |_| vec![record]);
        }
        if kind != "turn_completed" {
            return vec![];
        }
        self.settle(
            params["sessionId"].as_str().unwrap_or(""),
            update["prompt_id"].as_str(),
            &update["usage"],
        )
    }

    pub fn settle(
        &mut self,
        session: &str,
        prompt: Option<&str>,
        usage: &Value,
    ) -> Vec<UsageRequest> {
        let Some(active) = self.active.as_ref().filter(|a| a.session == session) else {
            return vec![];
        };
        let Some(prompt) = prompt.filter(|s| !s.is_empty()) else {
            return vec![];
        };
        if active.responses.is_empty() || !matches_summary(&active.responses, usage) {
            return vec![];
        }
        let active = self.active.take().expect("active");
        self.covered.insert((session.into(), prompt.into()));
        let sole_model = usage["modelUsage"]
            .as_object()
            .filter(|m| m.len() == 1)
            .and_then(|m| m.keys().next())
            .cloned();
        let only_one = active.responses.len() == 1;
        let records: Vec<_> = active
            .responses
            .into_iter()
            .enumerate()
            .map(|(index, mut record)| {
                if record.request_id.is_empty() {
                    record.request_id = format!("prompt:{prompt}:response:{index}");
                }
                // A native single-model ledger confirms the actual model, including automatic routing.
                record.model = sole_model.clone().or(record.model);
                if only_one {
                    record.cost_usd = cost(usage);
                }
                record
            })
            .collect();
        self.settled.clone_from(&records);
        records
    }
    pub fn finish(&mut self, session: &str) -> Vec<UsageRequest> {
        if self.active.as_ref().is_some_and(|a| a.session == session) {
            return self
                .active
                .take()
                .expect("active")
                .responses
                .into_iter()
                .filter(UsageRequest::has_data)
                .collect();
        }
        std::mem::take(&mut self.settled)
    }
    pub fn cost_query(&mut self, session: &str) -> Option<CostQuery> {
        let active = self.active.as_mut().filter(|a| a.session == session)?;
        if active.responses.len() <= active.last_cost_requested {
            return None;
        }
        active.last_cost_requested = active.responses.len();
        Some(CostQuery {
            session: active.session.clone(),
            generation: active.generation,
            sequence: active.responses.len(),
        })
    }
    pub fn apply_cost(&mut self, query: &CostQuery, value: &Value) -> Option<f64> {
        if ["costIsPartial", "usageIsIncomplete"]
            .iter()
            .any(|key| value.get(key).is_some_and(|v| *v != false))
        {
            return None;
        }
        let base = self.history_cost.get(&query.session).copied().flatten()?;
        let active = self.active.as_mut().filter(|a| {
            a.session == query.session
                && a.generation == query.generation
                && query.sequence >= a.last_cost_sequence
        })?;
        let amount = cost(value.get("usage").unwrap_or(value))?;
        active.last_cost_sequence = query.sequence;
        let total = base + amount;
        total.is_finite().then_some(total)
    }
    pub fn reconcile(
        &self,
        session: &str,
        mut history: RequestUsageReport,
        live: Option<&RequestUsageReport>,
    ) -> RequestUsageReport {
        history.records.retain(|r| {
            !self.covered.iter().any(|(s, p)| {
                s == session
                    && r.source == "grok.single_model_call"
                    && r.request_id.starts_with(&format!("{p}:"))
            })
        });
        for record in live
            .into_iter()
            .flat_map(|r| &r.records)
            .filter(|r| r.source == "grok.response")
        {
            history.observe(record.clone());
        }
        history
    }
}

fn matches_summary(records: &[UsageRequest], usage: &Value) -> bool {
    usage["modelCalls"].as_u64() == Some(records.len() as u64)
        && records
            .iter()
            .try_fold(0_u64, |sum, r| sum.checked_add(r.input_tokens?))
            == usage["inputTokens"].as_u64()
        && records
            .iter()
            .try_fold(0_u64, |sum, r| sum.checked_add(r.output_tokens?))
            == usage["outputTokens"].as_u64()
}
#[allow(clippy::cast_precision_loss)]
pub(super) fn cost(usage: &Value) -> Option<f64> {
    if ["costIsPartial", "usageIsIncomplete"]
        .iter()
        .any(|k| usage.get(k).is_some_and(|v| *v != false))
    {
        return None;
    }
    usage["costUsdTicks"]
        .as_u64()
        .map(|ticks| ticks as f64 / 10_000_000_000.0)
}
fn response(usage: &Value) -> Option<UsageRequest> {
    let optional = |key: &str| -> Option<Option<u64>> {
        match usage.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(v) => Some(Some(v.as_u64()?)),
        }
    };
    let input = usage["input_tokens"].as_u64()?;
    let read = optional("cache_read_input_tokens")?;
    let write = optional("cache_creation_input_tokens")?;
    // Without both disjoint cache buckets, the normalized input cannot be determined.
    let normalized = read
        .zip(write)
        .and_then(|(r, w)| input.checked_add(r)?.checked_add(w));
    let record = UsageRequest {
        request_id: String::new(),
        source: "grok.response".into(),
        input_tokens: normalized,
        output_tokens: optional("output_tokens")?,
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: optional("reasoning_tokens")?,
        ..UsageRequest::default()
    };
    if record
        .output_tokens
        .zip(record.reasoning_tokens)
        .is_some_and(|(o, r)| r > o)
    {
        return None;
    }
    (record.input_tokens.is_some()
        && record.output_tokens.is_some()
        && (record.input_tokens != Some(0) || record.output_tokens != Some(0)))
    .then_some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn completion(id: Option<&str>) -> Value {
        json!({"sessionId":"s","update":{"sessionUpdate":"response_completed","message_id":id,"usage":{"input_tokens":80,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":10,"reasoning_tokens":5}}})
    }
    fn summary(calls: u64) -> Value {
        json!({"modelCalls":calls,"inputTokens":100*calls,"outputTokens":10*calls,"costUsdTicks":200_000_000,"modelUsage":{"actual":{}}})
    }
    #[test]
    fn anonymous_responses_require_native_prompt_and_matching_call_count() {
        let mut meter = GrokUsage::default();
        meter.begin("s", None);
        assert!(meter.observe(&completion(None), None).is_empty());
        assert!(meter.observe(&completion(None), None).is_empty());
        assert!(meter.settle("s", Some("p"), &summary(1)).is_empty());
        let requests = meter.settle("s", Some("p"), &summary(2));
        assert_eq!(requests.len(), 2);
        assert_ne!(requests[0].request_id, requests[1].request_id);
        assert_eq!(requests[0].input_tokens, Some(100));
        assert!(requests[0].cost_usd.is_none());
        assert_eq!(requests[0].model.as_deref(), Some("actual"));
        assert!(meter.settle("s", Some("p"), &summary(2)).is_empty());
    }
    #[test]
    fn foreign_late_duplicate_tool_and_cost_events_are_isolated() {
        let mut meter = GrokUsage::default();
        meter.begin(
            "s",
            Some(&SessionUsageSnapshot {
                session_cost_usd: Some(1.0),
                ..SessionUsageSnapshot::default()
            }),
        );
        let mut other = completion(Some("foreign"));
        other["sessionId"] = json!("child");
        assert!(meter.observe(&other, None).is_empty());
        meter.observe(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","rawOutput":"tool output"}}),None);
        let requests = meter.observe(&completion(Some("r")), None);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].generation_ms.is_none());
        assert!(meter.observe(&completion(Some("r")), None).is_empty());
        let query = meter.cost_query("s").unwrap();
        assert!(meter.cost_query("s").is_none());
        assert!(meter.cost_query("child").is_none());
        assert_eq!(meter.apply_cost(&query, &summary(1)), Some(1.02));
        meter.finish("s");
        assert!(meter.apply_cost(&query, &summary(1)).is_none());
        meter.begin("s", None);
        assert!(meter.apply_cost(&query, &summary(1)).is_none());
        let mut invalid = completion(Some("bad"));
        invalid["update"]["usage"]["reasoning_tokens"] = json!(11);
        assert!(meter.observe(&invalid, None).is_empty());
    }
    #[test]
    fn settlement_keeps_native_cost_and_prevents_history_double_counting() {
        let mut meter = GrokUsage::default();
        meter.begin("s", None);
        meter.active.as_mut().unwrap().first_output =
            Instant::now().checked_sub(std::time::Duration::from_millis(50));
        meter.observe(&completion(None), None);
        let live = meter.settle("s", Some("p"), &summary(1));
        assert!(live[0].generation_ms.unwrap() >= 50);
        assert_eq!(live[0].cost_usd, Some(0.02));
        let mut history = RequestUsageReport::default();
        super::super::request_usage::grok_report(
            &json!({"sessionUpdate":"turn_completed","prompt_id":"p","usage":{"modelUsage":{"actual":{"modelCalls":1,"inputTokens":100,"outputTokens":10}}}}),
            &mut history,
        );
        let result = meter.reconcile(
            "s",
            history,
            Some(&RequestUsageReport {
                records: live,
                ..RequestUsageReport::default()
            }),
        );
        assert_eq!(result.records.len(), 1);
    }
    #[test]
    fn connection_reset_rebases_cost_and_finish_returns_only_current_turn() {
        let mut meter = GrokUsage::default();
        let baseline = |amount| SessionUsageSnapshot {
            session_cost_usd: Some(amount),
            ..SessionUsageSnapshot::default()
        };
        meter.begin("s", Some(&baseline(1.0)));
        meter.observe(&completion(Some("r1")), None);
        let old_query = meter.cost_query("s").unwrap();
        assert_eq!(
            meter.apply_cost(&old_query, &json!({"usage":summary(1)})),
            Some(1.02)
        );
        assert!(
            meter
                .apply_cost(
                    &old_query,
                    &json!({"usage":summary(1),"costIsPartial":true})
                )
                .is_none()
        );
        assert!(
            meter
                .apply_cost(&old_query, &json!({"usage":{"costUsdTicks":-1}}))
                .is_none()
        );
        meter.settle("s", Some("p1"), &summary(1));
        assert_eq!(meter.finish("s").len(), 1);
        assert!(meter.finish("s").is_empty());
        meter.reset_connection();
        meter.begin("s", Some(&baseline(2.0)));
        meter.observe(&completion(Some("r2")), None);
        assert!(meter.apply_cost(&old_query, &summary(1)).is_none());
        let query = meter.cost_query("s").unwrap();
        assert_eq!(meter.apply_cost(&query, &summary(1)), Some(2.02));
        let current = meter.finish("s");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].request_id, "response:r2");
        meter.begin("s", None);
        assert!(meter.finish("s").is_empty());
    }
}
