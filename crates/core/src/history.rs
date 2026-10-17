//! Task-scoped, bounded canonical-history queries. Cursors freeze the upper bound.
use crate::{Core, CoreError};
use agent_domain::TaskId;
use agent_protocol::{EventEnvelope, TimelineItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    pub task_id: TaskId,
    pub after_sequence: u64,
    pub through_sequence: Option<u64>,
    pub limit: usize,
    pub kind: String,
    #[serde(default)]
    pub text: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<Value>,
    pub next_cursor: Option<u64>,
    pub through_sequence: u64,
    pub coverage: String,
}
pub(crate) fn matches_event(event: &EventEnvelope, kind: &str) -> bool {
    match kind {
        "decisions" => event.event_type == "decision.recorded",
        "changes" => {
            event.event_type == "workspace.turn_changes_captured"
                || event.event_type == "workspace.turn_changes_failed"
        }
        _ => false,
    }
}
pub(crate) fn validate(query: &HistoryQuery) -> Result<(), CoreError> {
    if !(1..=100).contains(&query.limit)
        || query.text.len() > 1024
        || !["turns", "search", "decisions", "changes"].contains(&query.kind.as_str())
    {
        return Err(CoreError::InvalidArgument("invalid history query".into()));
    }
    Ok(())
}
pub(crate) fn timeline_value(item: TimelineItem) -> Value {
    json!({"sequence":item.sequence,"turnId":item.turn_id,"kind":item.kind,"text":item.text,"occurredAt":item.occurred_at,"attachments":item.attachments})
}
impl Core {
    pub async fn query_history(&self, query: &HistoryQuery) -> Result<HistoryPage, CoreError> {
        validate(query)?;
        self.require_task(&query.task_id).await?;
        let head = self.repository.bootstrap().await?.last_sequence;
        let through = query.through_sequence.unwrap_or(head);
        if through > head || query.after_sequence > through {
            return Err(CoreError::InvalidArgument("invalid history range".into()));
        }
        if query.kind == "turns" || query.kind == "search" {
            let mut items = self
                .repository
                .history_items(
                    &query.task_id,
                    query.after_sequence,
                    through,
                    &query.text,
                    query.limit + 1,
                )
                .await?;
            let more = items.len() > query.limit;
            items.truncate(query.limit);
            return Ok(HistoryPage { next_cursor: if more { items.last().map(|i| i.sequence) } else { None }, items: items.into_iter().map(timeline_value).collect(), through_sequence: through, coverage: "Persisted conversation text only; live thoughts and artifact bodies excluded. Pages contain timeline records with turnId; a turn can span pages.".into() });
        }
        let events = self
            .repository
            .task_events(&query.task_id, query.after_sequence, through, 1001)
            .await?;
        let mut cursor = query.after_sequence;
        let mut items = Vec::new();
        for event in events.iter().take(1000) {
            cursor = event.sequence;
            if matches_event(event, &query.kind) {
                items.push(json!({"sequence":event.sequence,"turnId":event.payload["turnId"],"kind":event.event_type,"text":event.payload.to_string(),"occurredAt":event.occurred_at}));
                if items.len() == query.limit {
                    break;
                }
            }
        }
        let more = events.last().is_some_and(|e| e.sequence > cursor);
        Ok(HistoryPage { items, next_cursor: more.then_some(cursor), through_sequence: through, coverage: "Canonical recorded facts only; no inferred decisions. Changes include failure records. At most 1000 source events scanned per page; follow nextCursor even for empty pages.".into() })
    }
}

/// Reconstruct one message at a canonical upper bound, retaining exact source spans.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrozenMessage {
    pub sequence: u64,
    pub turn_id: Option<String>,
    pub kind: String,
    pub text: String,
    pub occurred_at: i64,
    pub fragments: Vec<FrozenFragment>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrozenFragment {
    pub sequence: u64,
    pub field: String,
    pub length: usize,
}

/// Retain persisted tool/approval/artifact evidence alongside conversation messages.
#[must_use]
pub fn message_fact(event: &EventEnvelope) -> Option<(&'static str, String)> {
    match event.event_type.as_str() {
        "turn.queued" | "message.delta" | "test.completed" => {
            crate::history_delivery::event_fact(event)
        }
        "review.updated"
        | "task.forked"
        | "activity.updated"
        | "tool.started"
        | "tool.completed"
        | "approval.requested"
        | "approval.resolved"
        | "workspace.diff_captured"
        | "workspace.operation_changes_captured"
        | "session.rebuild_context_prepared"
        | "handoff.package_confirmed"
        | "reconciliation.package_confirmed"
        | "handoff.sent"
        | "reconciliation.sent"
        | "turn.failed"
        | "turn.interrupted" => Some(("payload", event.payload.to_string())),
        _ => None,
    }
}
