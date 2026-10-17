//! Durable delivery evidence. Checkpoints describe execution, never historical coverage.
use crate::{CoreRepository, RepositoryError};
use agent_domain::{SessionMapId, TaskId};
use agent_protocol::{EventEnvelope, HistoryFactDelivery};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[must_use]
pub fn event_fact(event: &EventEnvelope) -> Option<(&'static str, String)> {
    let field = match event.event_type.as_str() {
        "turn.queued" => "prompt",
        "message.delta" => "text",
        "decision.recorded" => "statement",
        "review.updated"
        | "task.forked"
        | "test.completed"
        | "workspace.turn_changes_captured"
        | "workspace.turn_changes_failed" => {
            return Some(("payload", event.payload.to_string()));
        }
        _ => return None,
    };
    Some((field, event.payload[field].as_str()?.to_owned()))
}
#[must_use]
pub fn fact(
    sequence: u64,
    field: &str,
    text: &str,
    offset: usize,
    end: usize,
) -> HistoryFactDelivery {
    HistoryFactDelivery {
        sequence,
        field: field.into(),
        sha256: hex::encode(Sha256::digest(text.as_bytes())),
        offset,
        end,
        total_bytes: text.len(),
    }
}

/// Bounded event pages and a bounded result tail; receipts are replayed from canonical events.
/// The frozen receipt boundary makes preview/confirmation hashes reproducible.
pub async fn missing_events(
    repository: &dyn CoreRepository,
    task_id: &TaskId,
    session_id: Option<&SessionMapId>,
    through: u64,
    limit: usize,
) -> Result<Vec<EventEnvelope>, RepositoryError> {
    let mut delivered: BTreeMap<(u64, String, String), Vec<(usize, usize)>> = BTreeMap::new();
    let mut own_turns = BTreeSet::new();
    if let Some(session_id) = session_id {
        let mut cursor = 0;
        loop {
            let page = repository
                .task_events(task_id, cursor, through, 100)
                .await?;
            if page.is_empty() {
                break;
            }
            let Some(last) = page.last() else {
                break;
            };
            cursor = last.sequence;
            for event in page {
                if event.payload["sessionMapId"].as_str() != Some(session_id.as_str()) {
                    continue;
                }
                if event.event_type == "history.native_prompt_accepted"
                    && let Some(id) = event.payload["turnId"].as_str()
                {
                    own_turns.insert(id.to_owned());
                }
                if event.event_type == "history.facts_delivered" {
                    let facts: Vec<HistoryFactDelivery> =
                        serde_json::from_value(event.payload["facts"].clone()).unwrap_or_default();
                    for f in facts {
                        if f.offset <= f.end && f.end <= f.total_bytes {
                            delivered
                                .entry((f.sequence, f.field, f.sha256))
                                .or_default()
                                .push((f.offset, f.end));
                        }
                    }
                }
            }
        }
    }
    let mut cursor = 0;
    let mut tail = VecDeque::new();
    loop {
        let page = repository
            .task_events(task_id, cursor, through, 100)
            .await?;
        if page.is_empty() {
            break;
        }
        let Some(last) = page.last() else {
            break;
        };
        cursor = last.sequence;
        for event in page {
            let Some((field, text)) = event_fact(&event) else {
                continue;
            };
            let own = matches!(event.event_type.as_str(), "turn.queued" | "message.delta")
                && event.payload["turnId"]
                    .as_str()
                    .is_some_and(|t| own_turns.contains(t));
            let key = (
                event.sequence,
                field.to_owned(),
                hex::encode(Sha256::digest(text.as_bytes())),
            );
            let covered = delivered.get_mut(&key).is_some_and(|ranges| {
                ranges.sort_unstable();
                let mut end = 0;
                for &(start, next) in ranges.iter() {
                    if start > end {
                        return false;
                    }
                    end = end.max(next);
                }
                end >= text.len()
            });
            if !own && !covered {
                tail.push_back(event);
                if tail.len() > limit {
                    tail.pop_front();
                }
            }
        }
    }
    Ok(tail.into_iter().collect())
}
