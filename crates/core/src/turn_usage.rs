//! Freeze observed usage at the prompt boundary; never attach a live session total to history.
use super::{Core, EventSource, ExecutorRef, TaskId};
use agent_domain::TurnId;
use agent_protocol::TurnUsage;
use driver_api::{
    DriverEvent,
    usage::{RequestUsageReport, SessionUsageSnapshot, UsageRequest},
};
use serde_json::json;
use std::collections::BTreeSet;

impl Core {
    pub(crate) async fn read_turn_usage(
        &self,
        executor: &ExecutorRef,
        session: &str,
    ) -> Option<SessionUsageSnapshot> {
        let provider = self.quota_providers.read().await.get(executor).cloned()?;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            provider.read_session_usage(session),
        )
        .await
        .ok()
        .flatten()
    }
    pub(crate) async fn observe_turn_usage(
        &self,
        task: &TaskId,
        turn: &TurnId,
        meter: &mut TurnMeter,
        event: &DriverEvent,
    ) {
        if let DriverEvent::UsageRecorded { request } = event
            && request.has_data()
        {
            meter.requests.observe(request.clone());
            let _ = self
                .append_runtime_event(
                    task,
                    "usage.request_recorded",
                    json!({"taskId":task,"turnId":turn,"request":request}),
                    EventSource::Hub,
                )
                .await;
        }
        if meter.first_output_ms.is_none() && first_output(event) {
            let elapsed = meter.offset_ms.saturating_add(
                u64::try_from(meter.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            );
            meter.first_output_ms = Some(elapsed);
            let _ = self
                .append_runtime_event(
                    task,
                    "turn.first_output",
                    json!({"taskId":task,"turnId":turn,"elapsedMs":elapsed}),
                    EventSource::Hub,
                )
                .await;
        }
    }
    pub(crate) async fn record_turn_usage(
        &self,
        task: &TaskId,
        turn: &TurnId,
        input: TurnUsageInput<'_>,
    ) {
        let mut records = input.meter.requests.clone();
        if let Some(report) = input.after.and_then(|s| s.request_usage.as_ref()) {
            records.truncated |= report.truncated;
            let baseline = input.before.and_then(|s| s.request_usage.as_ref());
            if input.new_session || baseline.is_some() {
                let previous: BTreeSet<_> = baseline
                    .into_iter()
                    .flat_map(|r| &r.records)
                    .map(|r| &r.request_id)
                    .collect();
                for request in &report.records {
                    if let Some(old) = records
                        .records
                        .iter_mut()
                        .find(|r| r.request_id == request.request_id)
                    {
                        let mut corrected = request.clone();
                        corrected.generation_ms = corrected.generation_ms.or(old.generation_ms);
                        corrected.model = corrected.model.or_else(|| old.model.clone());
                        corrected.cost_usd = corrected.cost_usd.or(old.cost_usd);
                        if corrected != *old {
                            *old = corrected.clone();
                            let _ = self
                                .append_runtime_event(
                                    task,
                                    "usage.request_recorded",
                                    json!({"taskId":task,"turnId":turn,"request":corrected}),
                                    EventSource::Hub,
                                )
                                .await;
                        }
                        continue;
                    }
                    if request.source != "claude.stream"
                        && !(request.source == "zcode.session.messages"
                            && records
                                .records
                                .iter()
                                .any(|request| request.source == "zcode.session.debug"))
                        && request.has_data()
                        && !previous.contains(&request.request_id)
                        && !records
                            .records
                            .iter()
                            .any(|r| r.request_id == request.request_id)
                    {
                        records.observe(request.clone());
                        // Live facts were already persisted as they arrived. Persist native-history
                        // facts too, so a restart need not invent deltas from session totals.
                        let _ = self.append_runtime_event(task, "usage.request_recorded", json!({"taskId":task,"turnId":turn,"nativeSessionId":input.native_session_id,"request":request}), EventSource::Hub).await;
                    }
                }
            }
        }
        let mut usage = if records.records.is_empty() {
            summarize(input.before, input.after, input.new_session).unwrap_or_else(|| TurnUsage {
                scope: "unknown".into(),
                completeness: Some("unknown".into()),
                incomplete_reason: Some("未取得可确认归属本轮的用量".into()),
                ..TurnUsage::default()
            })
        } else {
            summarize_recorded_requests(&records).unwrap()
        };
        supplement_summary_cost(input.before, input.after, input.new_session, &mut usage);
        if usage.completeness.is_none() {
            usage.completeness = Some(
                if usage.scope == "native_turn" {
                    "complete"
                } else {
                    "partial"
                }
                .into(),
            );
        }
        if usage.models.is_empty() {
            usage.models.extend(
                input
                    .after
                    .and_then(|snapshot| snapshot.model.clone())
                    .filter(|model| !matches!(model.as_str(), "auto" | "default")),
            );
        }
        let _ = self
            .append_runtime_event(
                task,
                "turn.usage_recorded",
                json!({"taskId":task,"turnId":turn,"usage":usage}),
                EventSource::Hub,
            )
            .await;
    }
}

fn supplement_summary_cost(
    before: Option<&SessionUsageSnapshot>,
    after: Option<&SessionUsageSnapshot>,
    new_session: bool,
    usage: &mut TurnUsage,
) {
    // Some native protocols expose request tokens but only a settled turn cost. Never
    // apportion that cost among requests or substitute the accumulated session cost.
    if let Some(after) = after
        && (before.is_some() || new_session)
        && usage.cost_usd.is_none()
        && after.totals_scope.as_deref() == Some("turn")
        && after.native_summary_id.is_some()
        && after.native_summary_id != before.and_then(|s| s.native_summary_id.clone())
    {
        usage.cost_usd = after.cost_usd;
    }
}
/// Per-turn live observations. A new meter is required for every native continuation.
pub(crate) struct TurnMeter {
    started: std::time::Instant,
    offset_ms: u64,
    first_output_ms: Option<u64>,
    pub requests: RequestUsageReport,
}
impl TurnMeter {
    pub fn new(offset_ms: u64) -> Self {
        Self {
            started: std::time::Instant::now(),
            offset_ms,
            first_output_ms: None,
            requests: RequestUsageReport::default(),
        }
    }
}
pub(crate) struct TurnUsageInput<'a> {
    pub before: Option<&'a SessionUsageSnapshot>,
    pub after: Option<&'a SessionUsageSnapshot>,
    pub new_session: bool,
    pub native_session_id: &'a str,
    pub meter: &'a TurnMeter,
}
fn first_output(event: &DriverEvent) -> bool {
    match event {
        DriverEvent::FirstOutput => true,
        DriverEvent::TextDelta { text, .. } => !text.trim().is_empty(),
        DriverEvent::Presentation { data, .. }
            if data["parentId"].as_str().is_none_or(str::is_empty)
                && data["sessionId"].as_str().is_none_or(str::is_empty) =>
        {
            matches!(data["type"].as_str(), Some("message" | "thought"))
                && ["delta", "textDelta", "text"]
                    .iter()
                    .any(|k| data[*k].as_str().is_some_and(|s| !s.trim().is_empty()))
        }
        _ => false,
    }
}
/// Rebuild an observed turn from durable request facts, preserving unknown fields.
#[must_use]
pub fn summarize_recorded_requests(report: &RequestUsageReport) -> Option<TurnUsage> {
    let snapshot = SessionUsageSnapshot {
        request_usage: Some(report.clone()),
        ..SessionUsageSnapshot::new()
    };
    summarize(None, Some(&snapshot), true)
}

fn total(records: &[&UsageRequest], field: fn(&UsageRequest) -> Option<u64>) -> Option<u64> {
    records
        .iter()
        .try_fold(0_u64, |sum, r| sum.checked_add(field(r)?))
}
#[allow(clippy::cast_precision_loss)]
fn generation_speed(records: &[&UsageRequest]) -> Option<f64> {
    let ms = total(records, |r| r.generation_ms.filter(|ms| *ms > 0))?;
    let output = total(records, |r| r.output_tokens)?;
    (ms > 0).then_some(output as f64 * 1000.0 / ms as f64)
}

fn money(records: &[&UsageRequest], field: fn(&UsageRequest) -> Option<f64>) -> Option<f64> {
    let value = records
        .iter()
        .try_fold(0.0, |sum, r| Some(sum + field(r)?))?;
    (value.is_finite() && value >= 0.0).then_some(value)
}
fn summarize(
    before: Option<&SessionUsageSnapshot>,
    after: Option<&SessionUsageSnapshot>,
    new_session: bool,
) -> Option<TurnUsage> {
    let after = after?;
    if let Some(report) = &after.request_usage {
        let baseline = before.and_then(|s| s.request_usage.as_ref());
        if baseline.is_some() || new_session {
            let previous: BTreeSet<_> = baseline
                .into_iter()
                .flat_map(|r| &r.records)
                .map(|r| &r.request_id)
                .collect();
            let records: Vec<_> = report
                .records
                .iter()
                .filter(|r| r.has_data() && !previous.contains(&r.request_id))
                .collect();
            if !records.is_empty() {
                return Some(TurnUsage {
                    scope: "observed_requests".into(),
                    completeness: Some("partial".into()),
                    incomplete_reason: Some(
                        if report.truncated {
                            "已达到采集上限"
                        } else {
                            "已记录请求；原生协议未证明覆盖全部请求"
                        }
                        .into(),
                    ),
                    output_tokens_per_second: generation_speed(&records),
                    timed_request_count: generation_speed(&records).map(|_| records.len() as u64),
                    request_count: Some(records.len() as u64),
                    models: records
                        .iter()
                        .filter_map(|r| r.model.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    input_tokens: total(&records, |r| r.input_tokens),
                    output_tokens: total(&records, |r| r.output_tokens),
                    cache_read_tokens: total(&records, |r| r.cache_read_tokens),
                    cache_write_tokens: total(&records, |r| r.cache_write_tokens),
                    reasoning_tokens: total(&records, |r| r.reasoning_tokens),
                    cost_usd: money(&records, |r| r.cost_usd),
                    credits: money(&records, |r| r.credits),
                    partial: report.truncated || baseline.is_some_and(|r| r.truncated),
                });
            }
        }
    }
    // Accept explicitly turn-scoped summaries only when new evidence differs from
    // the pre-prompt reading. A fresh polling timestamp alone does not prove freshness.
    let counters = |s: &SessionUsageSnapshot| {
        (
            s.input_tokens,
            s.output_tokens,
            s.cache_read_tokens,
            s.cache_write_tokens,
            s.reasoning_tokens,
            s.cost_usd,
            s.credits,
        )
    };
    if after.totals_scope.as_deref() == Some("turn")
        && (new_session
            || before.is_some_and(|b| {
                counters(b) != counters(after)
                    || after.native_summary_id.is_some()
                        && b.native_summary_id != after.native_summary_id
            }))
        && (after.input_tokens.is_some()
            || after.output_tokens.is_some()
            || after.cost_usd.is_some()
            || after.credits.is_some())
    {
        return Some(TurnUsage {
            scope: "native_turn".into(),
            models: after.model.iter().cloned().collect(),
            input_tokens: after.input_tokens,
            output_tokens: after.output_tokens,
            cache_read_tokens: after.cache_read_tokens,
            cache_write_tokens: after.cache_write_tokens,
            reasoning_tokens: after.reasoning_tokens,
            cost_usd: after.cost_usd,
            credits: after.credits,
            ..TurnUsage::default()
        });
    }
    if after.totals_scope.as_deref() == Some("observed") {
        let previous = before
            .and_then(|b| {
                (b.totals_scope == after.totals_scope)
                    .then_some(b.credits)
                    .flatten()
            })
            .or(new_session.then_some(0.0));
        if let Some((end, start)) = after.credits.zip(previous) {
            let delta = end - start;
            if delta.is_finite() && delta > 0.0 {
                return Some(TurnUsage {
                    scope: "credits_delta".into(),
                    credits: Some(delta),
                    ..TurnUsage::default()
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn turn_cost_uses_a_new_native_summary_and_never_the_session_balance() {
        let before = super::SessionUsageSnapshot {
            native_summary_id: Some("old".into()),
            ..Default::default()
        };
        let mut after = super::SessionUsageSnapshot {
            native_summary_id: Some("new".into()),
            totals_scope: Some("turn".into()),
            cost_usd: Some(0.03),
            session_cost_usd: Some(100.0),
            ..Default::default()
        };
        let mut usage = super::TurnUsage::default();
        super::supplement_summary_cost(Some(&before), Some(&after), false, &mut usage);
        assert_eq!(usage.cost_usd, Some(0.03));
        usage.cost_usd = None;
        after.native_summary_id = before.native_summary_id.clone();
        super::supplement_summary_cost(Some(&before), Some(&after), false, &mut usage);
        assert!(usage.cost_usd.is_none());
        after.native_summary_id = Some("new".into());
        after.totals_scope = Some("session".into());
        super::supplement_summary_cost(Some(&before), Some(&after), false, &mut usage);
        assert!(usage.cost_usd.is_none());
        after.totals_scope = Some("turn".into());
        super::supplement_summary_cost(None, Some(&after), false, &mut usage);
        assert!(usage.cost_usd.is_none());
    }
    use super::*;
    use driver_api::usage::RequestUsageReport;
    #[test]
    fn identical_native_totals_are_a_new_turn_only_with_new_native_identity() {
        let before = SessionUsageSnapshot {
            native_summary_id: Some("before".into()),
            model: Some("native-model".into()),
            totals_scope: Some("turn".into()),
            input_tokens: Some(10),
            output_tokens: Some(5),
            ..SessionUsageSnapshot::new()
        };
        let mut after = before.clone();
        assert!(summarize(Some(&before), Some(&after), false).is_none());
        after.native_summary_id = Some("after".into());
        let summary = summarize(Some(&before), Some(&after), false).unwrap();
        assert_eq!(summary.models, vec!["native-model"]);
        assert_eq!(summary.output_tokens, Some(5));
    }
    #[test]
    fn speed_uses_only_complete_request_windows_and_first_output_excludes_tools() {
        let mut report = snapshot(&[("a", 20), ("b", 30)]).request_usage.unwrap();
        report.records[0].generation_ms = Some(100);
        report.records[1].generation_ms = Some(400);
        let usage = summarize_recorded_requests(&report).unwrap();
        assert_eq!(usage.output_tokens_per_second, Some(20.0));
        assert_eq!(usage.timed_request_count, Some(2));
        assert_eq!(usage.completeness.as_deref(), Some("partial"));
        report.records[1].generation_ms = None;
        assert_eq!(
            summarize_recorded_requests(&report)
                .unwrap()
                .output_tokens_per_second,
            None
        );
        assert!(first_output(&DriverEvent::FirstOutput));
        assert!(!first_output(&DriverEvent::ToolStarted {
            item_id: "t".into(),
            tool_name: "Bash".into(),
            summary: "output".into()
        }));
        assert!(first_output(&DriverEvent::Presentation {
            item_id: "thought".into(),
            data: json!({"type":"thought","delta":"text"})
        }));
        assert!(!first_output(&DriverEvent::Presentation {
            item_id: "child".into(),
            data: json!({"type":"message","parentId":"p","text":"child"})
        }));
    }
    fn snapshot(ids: &[(&str, u64)]) -> SessionUsageSnapshot {
        SessionUsageSnapshot {
            request_usage: Some(RequestUsageReport {
                records: ids
                    .iter()
                    .map(|(id, tokens)| UsageRequest {
                        request_id: (*id).into(),
                        input_tokens: Some(*tokens),
                        output_tokens: Some(5),
                        ..UsageRequest::default()
                    })
                    .collect(),
                ..RequestUsageReport::default()
            }),
            ..SessionUsageSnapshot::new()
        }
    }
    #[test]
    fn excludes_previous_requests_and_corrections_and_never_sums_session_snapshots() {
        let before = snapshot(&[("old", 100)]);
        let after = snapshot(&[("old", 120), ("new", 20)]);
        let stats = summarize(Some(&before), Some(&after), false).unwrap();
        assert_eq!(stats.request_count, Some(1));
        assert_eq!(stats.input_tokens, Some(20));
        assert_eq!(stats.cost_usd, None);
        assert!(summarize(None, Some(&after), false).is_none());
        assert_eq!(
            summarize(None, Some(&after), true).unwrap().request_count,
            Some(2)
        );
        assert!(summarize(Some(&after), Some(&after), false).is_none());
    }
    #[test]
    fn supports_turn_summary_and_credit_delta_but_rejects_stale_or_reset_values() {
        let mut before = SessionUsageSnapshot {
            totals_scope: Some("turn".into()),
            input_tokens: Some(50),
            ..SessionUsageSnapshot::new()
        };
        let mut after = before.clone();
        after.observed_at_unix_ms += 1000;
        assert!(summarize(Some(&before), Some(&after), false).is_none());
        after.input_tokens = Some(60);
        assert_eq!(
            summarize(Some(&before), Some(&after), false)
                .unwrap()
                .input_tokens,
            Some(60)
        );
        before.totals_scope = Some("observed".into());
        before.credits = Some(2.0);
        after.totals_scope = Some("observed".into());
        after.credits = Some(2.5);
        assert_eq!(
            summarize(Some(&before), Some(&after), false)
                .unwrap()
                .credits,
            Some(0.5)
        );
        after.credits = Some(1.0);
        assert!(summarize(Some(&before), Some(&after), false).is_none());
    }
    #[test]
    fn missing_fields_and_overflow_remain_unknown_and_truncation_is_visible() {
        let mut after = snapshot(&[("a", u64::MAX), ("b", 1)]);
        let report = after.request_usage.as_mut().unwrap();
        report.truncated = true;
        report.records[1].output_tokens = None;
        let stats = summarize(None, Some(&after), true).unwrap();
        assert_eq!(stats.input_tokens, None);
        assert_eq!(stats.output_tokens, None);
        assert!(stats.partial);
    }
}
