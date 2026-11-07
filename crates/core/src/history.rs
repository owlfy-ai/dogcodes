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
/// A conversation's most recent message containing the searched text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMatch {
    pub task_id: TaskId,
    pub item_id: String,
    pub turn_id: Option<String>,
    /// Whether the person wrote it, rather than an agent.
    pub from_user: bool,
    pub snippet: String,
}

const MAX_SEARCH_LENGTH: usize = 1024;
const MAX_SEARCH_RESULTS: usize = 50;
const SNIPPET_LEAD: usize = 30;
const SNIPPET_LENGTH: usize = 140;

/// The part of a message around the match, on one line, for a row in a results list.
#[must_use]
pub fn search_snippet(body: &str, text: &str) -> String {
    let lowered = body.to_lowercase();
    let at = lowered
        .find(&text.to_lowercase())
        .map_or(0, |byte| lowered[..byte].chars().count());
    let start = at.saturating_sub(SNIPPET_LEAD);
    let excerpt: String = body.chars().skip(start).take(SNIPPET_LENGTH).collect();
    let line = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
    let more = body.chars().count() > start + SNIPPET_LENGTH;
    format!(
        "{}{line}{}",
        if start > 0 { "…" } else { "" },
        if more { "…" } else { "" }
    )
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
    /// Literal text across every conversation's messages, one result per conversation.
    pub async fn search_conversations(
        &self,
        text: &str,
    ) -> Result<Vec<ConversationMatch>, CoreError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if text.len() > MAX_SEARCH_LENGTH {
            return Err(CoreError::InvalidArgument("invalid search".into()));
        }
        Ok(self
            .repository
            .search_conversations(text, MAX_SEARCH_RESULTS)
            .await?)
    }

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

#[cfg(test)]
mod search_tests {
    use super::search_snippet;

    #[test]
    fn keeps_a_short_message_whole_on_one_line() {
        assert_eq!(search_snippet("fix\n  the  bug", "the"), "fix the bug");
    }

    #[test]
    fn cuts_a_long_message_around_the_match_on_character_boundaries() {
        let body = format!("{}目标{}", "前".repeat(100), "后".repeat(200));
        let snippet = search_snippet(&body, "目标");
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.contains("目标"));
        assert_eq!(snippet.chars().count(), 142);
    }

    #[test]
    fn starts_at_the_beginning_when_the_match_is_there_or_unfound() {
        let body = "a".repeat(200);
        assert!(search_snippet(&body, "AAA").starts_with("aaa"));
        assert!(!search_snippet(&body, "missing").starts_with('…'));
    }
}
