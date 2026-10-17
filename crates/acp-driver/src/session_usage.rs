use super::AcpQuotaState;
use driver_api::usage::{SessionUsageSnapshot, UsageStore, number, token};
use serde_json::Value;

impl AcpQuotaState {
    pub(super) fn observe_live_grok(&self, params: &Value) -> Vec<driver_api::usage::UsageRequest> {
        let Some(id) = params["sessionId"].as_str() else {
            return vec![];
        };
        let update = params.get("update").unwrap_or(params);
        if params["_meta"]["subagentId"].is_string() || update["_meta"]["subagentId"].is_string() {
            return vec![];
        }
        let model = self.usage.get(Some(id)).and_then(|s| s.model);
        let (own_turn, records) = {
            let mut meter = self.grok_usage.lock().expect("grok usage");
            (meter.is_active(id), meter.observe(params, model))
        };
        for record in &records {
            self.usage.observe_request(id, record.clone());
        }
        if update["sessionUpdate"] == "turn_completed" && own_turn {
            self.record_grok_summary(id, update["prompt_id"].as_str(), &update["usage"]);
        }
        records
    }
    pub(super) fn settle_live_grok(
        &self,
        id: &str,
        prompt: Option<&str>,
        usage: &Value,
    ) -> Vec<driver_api::usage::UsageRequest> {
        let (own_turn, records) = {
            let mut meter = self.grok_usage.lock().expect("grok usage");
            (meter.is_active(id), meter.settle(id, prompt, usage))
        };
        if own_turn {
            self.record_grok_summary(id, prompt, usage);
        }
        records
    }
    fn record_grok_summary(&self, id: &str, prompt: Option<&str>, usage: &Value) {
        let Some(prompt) = prompt.filter(|s| !s.is_empty()) else {
            return;
        };
        if !usage.is_object() {
            return;
        }
        self.usage.merge(
            id,
            SessionUsageSnapshot {
                native_summary_id: Some(prompt.into()),
                totals_scope: Some("turn".into()),
                cost_usd: super::grok_usage::cost(usage),
                ..driver_api::usage::acp(usage)
            },
        );
    }
    pub(super) fn observe_model(&self, id: &str, state: &Value) {
        let model = super::models::model_config(state)
            .and_then(|config| config["currentValue"].as_str())
            .or_else(|| {
                state
                    .pointer("/models/currentModelId")
                    .and_then(Value::as_str)
            })
            .filter(|value| !value.is_empty());
        if let Some(model) = model {
            self.usage.merge(
                id,
                SessionUsageSnapshot {
                    model: Some(model.into()),
                    ..SessionUsageSnapshot::new()
                },
            );
        }
    }
    pub(super) fn observe_kiro_context(&self, id: &str, value: &Value) {
        let value = value.get("contextUsage").unwrap_or(value);
        self.usage.merge(
            id,
            SessionUsageSnapshot {
                context_used_percent: number(value, &["usagePercentage"]),
                ..SessionUsageSnapshot::new()
            },
        );
    }
    pub(super) fn observe_kiro(&self, id: &str, value: &Value) {
        if value["kind"] == "context_usage" {
            self.observe_kiro_context(id, value);
        }
        if value["kind"] != "turn_completion" && value["type"] != "usage_summary" {
            return;
        }
        let key = value["requestIds"]
            .as_array()
            .filter(|ids| !ids.is_empty())
            .and_then(|ids| {
                let mut ids = ids.iter().map(Value::as_str).collect::<Option<Vec<_>>>()?;
                if ids.iter().any(|id| id.is_empty()) {
                    return None;
                }
                ids.sort_unstable();
                ids.dedup();
                Some(format!("requests:{}", serde_json::to_string(&ids).ok()?))
            })
            .or_else(|| {
                value["executionId"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|id| format!("execution:{id}"))
            });
        let Some(key) = key else {
            return;
        };
        let Some(entries) = value["promptTurnSummaries"].as_array() else {
            return;
        };
        let entries = entries
            .iter()
            .filter(|e| e["unit"] == "credit")
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return;
        }
        let Some(values) = entries
            .iter()
            .map(|e| number(e, &["usage"]))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let mut all = self.kiro_credits.lock().unwrap();
        let session = all.entry(id.into()).or_default();
        session.insert(key, values.iter().sum());
        let credits = session.values().sum::<f64>();
        if credits.is_finite() {
            self.usage.merge(
                id,
                SessionUsageSnapshot {
                    credits: Some(credits),
                    totals_scope: Some("observed".into()),
                    ..SessionUsageSnapshot::new()
                },
            );
        }
    }
}

pub(super) fn observe_grok(store: &UsageStore, params: &Value) {
    let Some(id) = params["sessionId"].as_str() else {
        return;
    };
    // Native signals have context semantics; cumulative metadata.totalTokens does not.
    let signals = params.get("signals").unwrap_or(params);
    store.merge(
        id,
        SessionUsageSnapshot {
            context_used_tokens: token(signals, &["contextTokensUsed"]),
            context_window_tokens: token(signals, &["contextWindowTokens"]).filter(|n| *n > 0),
            ..SessionUsageSnapshot::new()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn grok_summary_without_requests_clears_previous_turn_cost_but_ignores_other_sessions() {
        let state = AcpQuotaState::default();
        state.record_grok_summary(
            "s",
            Some("old"),
            &json!({"inputTokens":10,"costUsdTicks":100_000_000}),
        );
        state.grok_usage.lock().unwrap().begin("s", None);
        let event = json!({"sessionId":"other","update":{"sessionUpdate":"turn_completed","prompt_id":"new","usage":{"inputTokens":20,"costUsdTicks":200_000_000,"costIsPartial":true}}});
        assert!(state.observe_live_grok(&event).is_empty());
        assert!(state.usage.get(Some("other")).is_none());
        assert_eq!(state.usage.get(Some("s")).unwrap().cost_usd, Some(0.01));
        let mut event = event;
        event["sessionId"] = json!("s");
        assert!(state.observe_live_grok(&event).is_empty());
        let summary = state.usage.get(Some("s")).unwrap();
        assert_eq!(summary.native_summary_id.as_deref(), Some("new"));
        assert_eq!(summary.totals_scope.as_deref(), Some("turn"));
        assert!(summary.cost_usd.is_none());
        state.settle_live_grok("s", Some("next"), &json!({"inputTokens":30}));
        assert!(state.usage.get(Some("s")).unwrap().cost_usd.is_none());
    }
    #[test]
    fn kiro_percent_and_deduplicated_credits_are_session_scoped() {
        let state = AcpQuotaState::default();
        state.observe_kiro(
            "a",
            &json!({"kind":"context_usage","contextUsage":{"usagePercentage":61.5}}),
        );
        let event = json!({"kind":"turn_completion","requestIds":["x","y"],"promptTurnSummaries":[{"unit":"credit","usage":0.5}]});
        state.observe_kiro("a", &event);
        state.observe_kiro("a", &event);
        let s = state.usage.get(Some("a")).unwrap();
        assert_eq!(s.credits, Some(0.5));
        assert_eq!(s.context_used_percent, Some(61.5));
        assert!(s.context_used_tokens.is_none());
        assert!(state.usage.get(Some("b")).is_none());
    }
}

/// Optional on-disk telemetry from the exact bound native session. Never scan
/// other workspaces, create a session, or read credentials to fill missing fields.
pub(super) fn read_native(
    provider: &str,
    cwd: &std::path::Path,
    id: &str,
) -> Option<SessionUsageSnapshot> {
    use std::{io::Read, path::PathBuf};
    if !matches!(provider, "grok" | "codebuddy")
        || id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return None;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let cwd = cwd.canonicalize().ok()?;
    let path = if provider == "grok" {
        use std::fmt::Write;
        let root = std::env::var_os("GROK_HOME")
            .map_or_else(|| PathBuf::from(&home).join(".grok"), PathBuf::from);
        let mut encoded = String::new();
        for b in cwd.to_str()?.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                encoded.push(char::from(b));
            } else {
                let _ = write!(encoded, "%{b:02X}");
            }
        }
        root.join("sessions")
            .join(encoded)
            .join(id)
            .join("signals.json")
    } else {
        let root = std::env::var_os("CODEBUDDY_CONFIG_DIR")
            .map_or_else(|| PathBuf::from(&home).join(".codebuddy"), PathBuf::from);
        // Native CodeBuddy preserves case, spaces and dots in workspace names.
        // Only path separators become hyphens; sanitizing every punctuation
        // character made Application Support and ordinary project paths unreadable.
        let slug = codebuddy_workspace_slug(&cwd)?;
        root.join("projects").join(slug).join(format!("{id}.jsonl"))
    };
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    let mut content = String::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut content)
        .ok()?;
    if content.len() > 16 * 1024 * 1024 {
        return None;
    }
    if provider == "grok" {
        let v: Value = serde_json::from_str(&content).ok()?;
        let s = SessionUsageSnapshot {
            context_used_tokens: token(&v, &["contextTokensUsed"]),
            context_window_tokens: token(&v, &["contextWindowTokens"]).filter(|n| *n > 0),
            ..SessionUsageSnapshot::new()
        };
        return s.has_data().then_some(s);
    }
    codebuddy_history(&content, id, &cwd)
}

fn codebuddy_workspace_slug(cwd: &std::path::Path) -> Option<String> {
    Some(
        cwd.to_str()?
            .replace(['/', '\\', ':'], "-")
            .trim_start_matches('-')
            .to_owned(),
    )
}

fn codebuddy_history(
    content: &str,
    session_id: &str,
    cwd: &std::path::Path,
) -> Option<SessionUsageSnapshot> {
    use std::collections::{HashMap, HashSet};
    let mut entries = HashMap::<String, Value>::new();
    let mut leaf = String::new();
    for line in content.split_inclusive('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) if !line.ends_with('\n') => break, // concurrent append
            Err(_) => return None,
        };
        if row["sessionId"].as_str().is_some_and(|id| id != session_id) {
            return None;
        }
        if let Some(dir) = row["cwd"].as_str()
            && std::path::Path::new(dir).canonicalize().ok().as_deref() != Some(cwd)
        {
            return None;
        }
        if row["type"] == "resend-fork-notice" {
            leaf = row["parentId"].as_str().unwrap_or("").into();
            continue;
        }
        if !matches!(
            row["type"].as_str(),
            Some("message" | "reasoning" | "function_call" | "function_call_result")
        ) {
            continue;
        }
        leaf = row["id"].as_str()?.into();
        entries.insert(leaf.clone(), row);
    }
    let mut seen = HashSet::new();
    let mut requests = HashMap::new();
    while !leaf.is_empty() {
        if !seen.insert(leaf.clone()) {
            return None;
        }
        let row = entries.get(&leaf)?;
        let data = &row["providerData"];
        if let Some(id) = data["messageId"].as_str().filter(|id| !id.is_empty())
            && data["rawUsage"].is_object()
        {
            requests
                .entry(id)
                .or_insert((&data["rawUsage"], &data["model"]));
        }
        leaf = row["parentId"].as_str().unwrap_or("").into();
    }
    if requests.is_empty() {
        return None;
    }
    let sum = |key| {
        requests
            .values()
            .try_fold(0_u64, |sum, v| sum.checked_add(token(v.0, &[key])?))
    };
    let credits = requests
        .values()
        .try_fold(0.0, |sum, v| Some(sum + number(v.0, &["credit"])?))
        .filter(|n| f64::is_finite(*n));
    let mut report = driver_api::usage::RequestUsageReport::default();
    for (id, (raw, model)) in &requests {
        report.observe(driver_api::usage::UsageRequest {
            request_id: (*id).into(),
            model: model.as_str().map(str::to_owned),
            source: "codebuddy.jsonl".into(),
            input_tokens: token(raw, &["prompt_tokens"]),
            output_tokens: token(raw, &["completion_tokens"]),
            cache_read_tokens: token(&raw["prompt_tokens_details"], &["cached_tokens"]),
            cache_write_tokens: token(raw, &["prompt_cache_write_tokens"]),
            reasoning_tokens: token(raw, &["completion_thinking_tokens"]),
            credits: number(raw, &["credit"]),
            ..driver_api::usage::UsageRequest::default()
        });
    }
    report
        .records
        .sort_by(|a, b| a.request_id.cmp(&b.request_id));
    let s = SessionUsageSnapshot {
        request_usage: Some(report),
        input_tokens: sum("prompt_tokens"),
        output_tokens: sum("completion_tokens"),
        total_tokens: sum("total_tokens"),
        cache_read_tokens: requests.values().try_fold(0_u64, |sum, v| {
            sum.checked_add(token(&v.0["prompt_tokens_details"], &["cached_tokens"])?)
        }),
        cache_write_tokens: sum("prompt_cache_write_tokens"),
        reasoning_tokens: sum("completion_thinking_tokens"),
        credits,
        totals_scope: Some("branch".into()),
        ..SessionUsageSnapshot::new()
    };
    s.has_data().then_some(s)
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_workspace_names_preserve_spaces_case_and_punctuation() {
        assert_eq!(
            codebuddy_workspace_slug(std::path::Path::new(
                "/Users/Test/Library/Application Support/codes.dog.desktop/scratch"
            ))
            .as_deref(),
            Some("Users-Test-Library-Application Support-codes.dog.desktop-scratch")
        );
    }
    #[test]
    fn history_counts_each_request_once_and_only_on_active_branch() {
        let cwd = tempfile::tempdir().unwrap();
        let path = cwd.path().canonicalize().unwrap();
        let row = |id, parent, request, count| {
            json!({"type":"message","sessionId":"s","cwd":path,"id":id,"parentId":parent,"providerData":{"messageId":request,"rawUsage":{"prompt_tokens":count,"credit":0.5}}}).to_string()
        };
        let content = [
            row("a", "", "r1", 100),
            row("old", "a", "r2", 10000),
            row("b", "a", "r1", 100),
            row("c", "b", "r3", 50),
        ]
        .join("\n");
        let s = codebuddy_history(&content, "s", &path).unwrap();
        assert_eq!(s.input_tokens, Some(150));
        assert_eq!(s.credits, Some(1.0));
        assert_eq!(s.output_tokens, None);
        assert!(codebuddy_history(&content, "other", &path).is_none());
        let rewind = format!(
            "{content}\n{}",
            json!({"type":"resend-fork-notice","parentId":"a"})
        );
        assert_eq!(
            codebuddy_history(&rewind, "s", &path).unwrap().input_tokens,
            Some(100)
        );
    }
}
