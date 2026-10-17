//! Native session telemetry. Context occupancy is independent of billed totals.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Mutex};
mod requests;
pub use requests::{RequestUsageReport, UsageRequest};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageSnapshot {
    /// Native session/request model, never inferred from the picker label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_summary_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_usage: Option<RequestUsageReport>,
    pub context_used_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub context_used_percent: Option<f64>,
    pub cache_hit_rate_percent: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub credits: Option<f64>,
    pub session_cost_usd: Option<f64>,
    /// Native accounting scope: session, turn, request, or native (unspecified). Never sum snapshots.
    pub totals_scope: Option<String>,
    pub observed_at_unix_ms: i64,
}
impl SessionUsageSnapshot {
    #[must_use]
    pub fn new() -> Self {
        Self {
            observed_at_unix_ms: now(),
            ..Self::default()
        }
    }
    #[must_use]
    pub fn has_data(&self) -> bool {
        self.model.is_some()
            || self.request_usage.is_some()
            || self.context_used_tokens.is_some()
            || self.context_window_tokens.is_some()
            || self.context_used_percent.is_some()
            || self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.total_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_write_tokens.is_some()
            || self.reasoning_tokens.is_some()
            || self.cost_usd.is_some()
            || self.credits.is_some()
            || self.session_cost_usd.is_some()
    }
}
#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
#[must_use]
pub fn token(v: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| v.get(k).and_then(Value::as_u64))
}
#[must_use]
pub fn number(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| {
        v.get(k)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0)
    })
}
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn ratio(cached: Option<u64>, input: Option<u64>) -> Option<f64> {
    cached
        .zip(input)
        .filter(|(c, i)| *i > 0 && c <= i)
        .map(|(c, i)| c as f64 / i as f64 * 100.0)
}

#[derive(Default)]
pub struct UsageStore(Mutex<HashMap<String, SessionUsageSnapshot>>);
impl UsageStore {
    /// Return the latest snapshot for a session.
    ///
    /// # Panics
    /// Panics if another thread panicked while holding the usage store lock.
    pub fn get(&self, id: Option<&str>) -> Option<SessionUsageSnapshot> {
        self.0.lock().unwrap().get(id?).cloned()
    }
    /// Revise a snapshot atomically, retaining observations that arrived during I/O.
    /// The callback must not perform I/O or re-enter this store.
    ///
    /// # Panics
    /// Panics if the usage store mutex was poisoned.
    pub fn update(
        &self,
        id: &str,
        update: impl FnOnce(&mut SessionUsageSnapshot),
    ) -> SessionUsageSnapshot {
        if id.is_empty() {
            return SessionUsageSnapshot::default();
        }
        let mut guard = self.0.lock().unwrap();
        let usage = guard.entry(id.into()).or_default();
        update(usage);
        usage.clone()
    }
    /// Replace the snapshot for a session when it contains usage data.
    ///
    /// # Panics
    /// Panics if another thread panicked while holding the usage store lock.
    pub fn set(&self, id: &str, usage: SessionUsageSnapshot) {
        if !id.is_empty() && usage.has_data() {
            let mut guard = self.0.lock().unwrap();
            let mut usage = usage;
            if usage.request_usage.is_none() {
                usage.request_usage = guard.get(id).and_then(|s| s.request_usage.clone());
            }
            if usage.model.is_none() {
                usage.model = guard.get(id).and_then(|s| s.model.clone());
            }
            guard.insert(id.into(), usage);
        }
    }
    /// Retain a request fact scoped to exactly one native session.
    ///
    /// # Panics
    /// Panics if the usage store mutex was poisoned.
    pub fn observe_request(&self, id: &str, request: UsageRequest) {
        if id.is_empty() || !request.has_data() {
            return;
        }
        self.0
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .request_usage
            .get_or_insert_with(RequestUsageReport::default)
            .observe(request);
    }
    /// Merge separate native context/cost/token reports; counters are absolute.
    ///
    /// # Panics
    /// Panics if another thread panicked while holding the usage store lock.
    pub fn merge(&self, id: &str, usage: SessionUsageSnapshot) {
        if id.is_empty() || !usage.has_data() {
            return;
        }
        let mut guard = self.0.lock().unwrap();
        let old = guard.entry(id.into()).or_default();
        // A missing cost on a new native turn must not inherit the previous turn's amount.
        if usage.totals_scope.as_deref() == Some("turn")
            && usage
                .native_summary_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            && usage.native_summary_id != old.native_summary_id
        {
            old.cost_usd = None;
        }
        macro_rules! merge { ($($field:ident),*) => { $(if usage.$field.is_some() { old.$field = usage.$field; })* }; }
        if usage.context_used_tokens.is_some()
            || usage.context_window_tokens.is_some()
            || usage.context_used_percent.is_some()
        {
            old.context_used_tokens = usage.context_used_tokens;
            old.context_window_tokens = usage.context_window_tokens;
            old.context_used_percent = usage.context_used_percent;
        }
        merge!(
            model,
            native_summary_id,
            request_usage,
            cache_hit_rate_percent,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            total_tokens,
            cost_usd,
            credits,
            session_cost_usd,
            totals_scope
        );
        old.observed_at_unix_ms = usage.observed_at_unix_ms;
    }
}

#[must_use]
pub fn codex(value: &Value) -> SessionUsageSnapshot {
    let mut s = SessionUsageSnapshot::new();
    let last = &value["last"];
    let total = &value["total"];
    s.context_used_tokens = token(last, &["totalTokens"]);
    s.context_window_tokens = token(value, &["modelContextWindow"]).filter(|n| *n > 0);
    s.cache_hit_rate_percent = ratio(
        token(last, &["cachedInputTokens"]),
        token(last, &["inputTokens"]),
    );
    s.input_tokens = token(total, &["inputTokens"]);
    s.output_tokens = token(total, &["outputTokens"]);
    s.cache_read_tokens = token(total, &["cachedInputTokens"]);
    s.reasoning_tokens = token(total, &["reasoningOutputTokens"]);
    s.total_tokens = token(total, &["totalTokens"]);
    s.totals_scope = Some("session".into());
    s
}
#[must_use]
pub fn claude(value: &Value) -> SessionUsageSnapshot {
    let mut s = SessionUsageSnapshot::new();
    let u = &value["usage"];
    s.input_tokens = token(u, &["input_tokens"]);
    s.output_tokens = token(u, &["output_tokens"]);
    s.cache_read_tokens = token(u, &["cache_read_input_tokens"]);
    s.cache_write_tokens = token(u, &["cache_creation_input_tokens"]);
    // Result token counters and cumulative cost have distinct native scopes.
    s.session_cost_usd = number(value, &["total_cost_usd"]);
    s.totals_scope = Some("turn".into());
    s
}
#[must_use]
pub fn pi(value: &Value) -> SessionUsageSnapshot {
    let mut s = SessionUsageSnapshot::new();
    let t = &value["tokens"];
    s.input_tokens = token(t, &["input"]);
    s.output_tokens = token(t, &["output"]);
    s.cache_read_tokens = token(t, &["cacheRead"]);
    s.cache_write_tokens = token(t, &["cacheWrite"]);
    s.total_tokens = token(t, &["total"]);
    s.cost_usd = number(value, &["cost"]);
    s.context_used_tokens = token(&value["contextUsage"], &["tokens"]);
    s.context_window_tokens = token(&value["contextUsage"], &["contextWindow"]).filter(|n| *n > 0);
    s.totals_scope = Some("session".into());
    s
}
/// ACP `usage_update` provides context; prompt counters retain the vendor's native scope.
#[must_use]
pub fn acp(value: &Value) -> SessionUsageSnapshot {
    let mut s = SessionUsageSnapshot::new();
    s.context_used_tokens = token(value, &["used"]);
    s.context_window_tokens = token(value, &["size"]).filter(|n| *n > 0);
    s.input_tokens = token(value, &["inputTokens"]);
    s.output_tokens = token(value, &["outputTokens"]);
    s.cache_read_tokens = token(value, &["cachedReadTokens"]);
    s.cache_write_tokens = token(value, &["cachedWriteTokens", "cacheCreationTokens"]);
    s.reasoning_tokens = token(value, &["thoughtTokens", "reasoningTokens"]);
    s.total_tokens = token(value, &["totalTokens"]);
    if value["cost"]["currency"] == "USD" {
        s.cost_usd = number(&value["cost"], &["amount"]);
    }
    s.cost_usd = s
        .cost_usd
        .or_else(|| number(value, &["costUsdTicks"]).map(|ticks| ticks / 10_000_000_000.0));
    s.totals_scope = Some("native".into());
    s
}
#[must_use]
pub fn antigravity(value: &Value) -> SessionUsageSnapshot {
    let mut s = SessionUsageSnapshot::new();
    s.context_used_tokens = token(value, &["context_used_tokens", "estimated_tokens_used"]);
    s.context_window_tokens =
        token(value, &["context_window_tokens", "max_context_tokens"]).filter(|n| *n > 0);
    s.input_tokens = token(value, &["input_tokens"]);
    s.output_tokens = token(value, &["output_tokens"]);
    s.cache_read_tokens = token(value, &["cache_read_tokens"]);
    s.reasoning_tokens = token(value, &["thinking_tokens"]);
    s.total_tokens = token(value, &["total_tokens"]);
    s.totals_scope = Some("native".into());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn history_revision_retains_live_facts_and_context_received_after_initial_snapshot() {
        let store = UsageStore::default();
        let initial = store.get(Some("s")).unwrap_or_default();
        store.observe_request(
            "s",
            UsageRequest {
                request_id: "live".into(),
                output_tokens: Some(7),
                generation_ms: Some(20),
                ..UsageRequest::default()
            },
        );
        store.merge(
            "s",
            SessionUsageSnapshot {
                context_used_tokens: Some(123),
                ..SessionUsageSnapshot::new()
            },
        );
        assert!(initial.request_usage.is_none());
        let revised = store.update("s", |usage| {
            usage
                .request_usage
                .get_or_insert_with(Default::default)
                .observe(UsageRequest {
                    request_id: "history".into(),
                    input_tokens: Some(100),
                    ..UsageRequest::default()
                });
            usage.model = Some("actual".into());
        });
        let records = revised.request_usage.unwrap().records;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].generation_ms, Some(20));
        assert_eq!(revised.context_used_tokens, Some(123));
        assert_eq!(
            store.get(Some("s")).unwrap().model.as_deref(),
            Some("actual")
        );
        store.update("", |_| panic!("empty identity must not be stored"));
        assert!(store.get(Some("")).is_none());
    }
    #[test]
    fn new_native_turn_does_not_inherit_previous_cost() {
        let store = UsageStore::default();
        store.merge(
            "s",
            SessionUsageSnapshot {
                native_summary_id: Some("old".into()),
                totals_scope: Some("turn".into()),
                cost_usd: Some(0.1),
                session_cost_usd: Some(1.0),
                ..SessionUsageSnapshot::new()
            },
        );
        store.merge(
            "s",
            SessionUsageSnapshot {
                native_summary_id: Some("new".into()),
                totals_scope: Some("turn".into()),
                input_tokens: Some(20),
                ..SessionUsageSnapshot::new()
            },
        );
        let snapshot = store.get(Some("s")).unwrap();
        assert!(snapshot.cost_usd.is_none());
        assert_eq!(snapshot.session_cost_usd, Some(1.0));
        store.merge(
            "s",
            SessionUsageSnapshot {
                cost_usd: Some(0.2),
                ..SessionUsageSnapshot::new()
            },
        );
        store.merge(
            "s",
            SessionUsageSnapshot {
                input_tokens: Some(30),
                ..SessionUsageSnapshot::new()
            },
        );
        assert_eq!(store.get(Some("s")).unwrap().cost_usd, Some(0.2));
    }
    #[test]
    fn snapshot_updates_preserve_request_identity_and_session_isolation() {
        let store = UsageStore::default();
        let request = UsageRequest {
            request_id: "r".into(),
            input_tokens: Some(100),
            ..UsageRequest::default()
        };
        store.observe_request("a", request.clone());
        store.observe_request("a", request);
        store.set("a", codex(&json!({"total":{"inputTokens":100}})));
        assert_eq!(
            store
                .get(Some("a"))
                .unwrap()
                .request_usage
                .unwrap()
                .records
                .len(),
            1
        );
        assert!(store.get(Some("b")).is_none());
        store.merge(
            "a",
            SessionUsageSnapshot {
                request_usage: Some(RequestUsageReport::default()),
                ..SessionUsageSnapshot::new()
            },
        );
        assert!(
            store
                .get(Some("a"))
                .unwrap()
                .request_usage
                .unwrap()
                .records
                .is_empty()
        );
    }
    #[test]
    fn context_is_last_request_not_total_and_cache_uses_latest() {
        let s = codex(
            &json!({"last":{"totalTokens":120,"inputTokens":100,"cachedInputTokens":90},"total":{"totalTokens":9_000_000,"inputTokens":8_000_000,"cachedInputTokens":3_000_000},"modelContextWindow":200_000}),
        );
        assert_eq!(s.context_used_tokens, Some(120));
        assert_eq!(s.total_tokens, Some(9_000_000));
        assert_eq!(s.cache_hit_rate_percent, Some(90.0));
    }
    #[test]
    fn pi_compaction_unknown_does_not_become_zero() {
        let s = pi(
            &json!({"tokens":{"input":50,"total":90},"contextUsage":{"tokens":null,"contextWindow":200_000}}),
        );
        assert_eq!(s.context_used_tokens, None);
        assert_eq!(s.context_window_tokens, Some(200_000));
    }
    #[test]
    fn snapshots_do_not_accumulate_or_cross_sessions() {
        let store = UsageStore::default();
        let s = acp(&json!({"used":100,"size":1000,"cost":{"currency":"USD","amount":0.1}}));
        store.merge("a", s.clone());
        store.merge("a", s);
        store.merge("a", acp(&json!({"totalTokens":10_000})));
        assert_eq!(store.get(Some("a")).unwrap().cost_usd, Some(0.1));
        assert!(store.get(Some("b")).is_none());
        assert!(store.get(None).is_none());
    }
    #[test]
    fn missing_negative_and_foreign_currency_are_not_invented() {
        let s = acp(&json!({"used":-1,"size":0,"cost":{"currency":"EUR","amount":2}}));
        assert!(!s.has_data());
        assert_eq!(ratio(Some(2), Some(0)), None);
    }
}
