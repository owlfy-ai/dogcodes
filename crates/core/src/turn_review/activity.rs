//! Review-local presentation items. Never project them onto the main conversation.
use driver_api::DriverEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewActivity {
    pub item_id: String,
    pub data: Value,
    #[serde(default)]
    pub legacy: bool,
}

pub(super) fn apply(items: &mut Vec<ReviewActivity>, updates: Vec<ReviewActivity>) {
    for update in updates {
        if let Some(old) = items.iter_mut().find(|old| old.item_id == update.item_id) {
            *old = update;
        } else {
            items.push(update);
        }
    }
}

#[derive(Default)]
pub(super) struct ActivityOutput {
    items: Vec<ReviewActivity>,
    dirty: BTreeSet<String>,
    seen: BTreeSet<String>,
}
impl ActivityOutput {
    pub fn push(&mut self, event: &DriverEvent, session: &str) {
        let (id, patch, legacy) = match event {
            DriverEvent::Presentation { item_id, data } => {
                if data["parentId"].as_str().is_some_and(|id| !id.is_empty())
                    || data["sessionId"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id != session)
                {
                    return;
                }
                if let Some(source) = data["sourceEventId"].as_str()
                    && !self.seen.insert(source.to_owned())
                {
                    return;
                }
                (item_id.clone(), data.clone(), false)
            }
            DriverEvent::TextDelta { item_id, text } => (
                format!("legacy-message:{item_id}"),
                json!({"type":"message", "textDelta":text}),
                true,
            ),
            DriverEvent::ToolStarted {
                item_id,
                tool_name,
                summary,
            } => (
                item_id.clone(),
                json!({"type":"tool", "name":tool_name,"input":summary,"status":"running"}),
                true,
            ),
            DriverEvent::ToolCompleted {
                item_id,
                tool_name,
                summary,
                success,
            } => (
                item_id.clone(),
                json!({"type":"tool", "name":tool_name,"output":summary,"status":if *success {"completed"} else {"failed"}}),
                true,
            ),
            _ => return,
        };
        let Some(patch) = patch.as_object() else {
            return;
        };
        let position = self.items.iter().position(|item| item.item_id == id);
        let index = position.unwrap_or_else(|| {
            self.items.push(ReviewActivity {
                item_id: id.clone(),
                data: json!({}),
                legacy,
            });
            self.items.len() - 1
        });
        let item = &mut self.items[index];
        // Normalized presentations carry richer tool input/output than legacy mirrors.
        if legacy && !item.legacy {
            return;
        }
        item.legacy = legacy;
        for (key, value) in patch {
            if value.is_null() || key == "sourceEventId" {
                continue;
            }
            if key == "textDelta" {
                item.data["text"] = json!(format!(
                    "{}{}",
                    item.data["text"].as_str().unwrap_or(""),
                    value.as_str().unwrap_or("")
                ));
            } else if key == "status"
                && ["completed", "failed", "interrupted", "cancelled"]
                    .contains(&item.data[key].as_str().unwrap_or(""))
                && ["pending", "running", "in_progress"].contains(&value.as_str().unwrap_or(""))
            {
                // A delayed mirror must not move a finished tool back to running.
            } else {
                item.data[key] = value.clone();
            }
        }
        self.dirty.insert(id);
    }
    pub fn changes(&mut self) -> Vec<ReviewActivity> {
        self.items
            .iter()
            .filter(|item| self.dirty.contains(&item.item_id))
            .cloned()
            .collect()
    }
    pub fn committed(&mut self) {
        self.dirty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_snapshots_deltas_and_mirrored_tools_merge_without_regressing() {
        let mut output = ActivityOutput::default();
        let presentation = |data| DriverEvent::Presentation {
            item_id: "tool".into(),
            data,
        };
        output.push(
            &DriverEvent::ToolStarted {
                item_id: "tool".into(),
                tool_name: "Read".into(),
                summary: "legacy".into(),
            },
            "root",
        );
        output.push(
            &presentation(json!({"type":"tool","input":{"path":"a.rs"},"status":"running"})),
            "root",
        );
        output.push(
            &presentation(json!({"status":"completed","output":"contents"})),
            "root",
        );
        output.push(&presentation(json!({"status":"running"})), "root");
        output.push(
            &DriverEvent::ToolCompleted {
                item_id: "tool".into(),
                tool_name: "Read".into(),
                summary: "mirror".into(),
                success: true,
            },
            "root",
        );
        let items = output.changes();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].data["status"], "completed");
        assert_eq!(items[0].data["output"], "contents");
        assert_eq!(items[0].data["input"]["path"], "a.rs");
        output.committed();
        assert!(output.changes().is_empty());
    }
    #[test]
    fn deduplicates_native_events_and_keeps_child_output_out_of_root_report() {
        let mut output = ActivityOutput::default();
        let event = DriverEvent::Presentation {
            item_id: "reply".into(),
            data: json!({"type":"message","sourceEventId":"one","textDelta":"part"}),
        };
        output.push(&event, "root");
        output.push(&event, "root");
        output.push(
            &DriverEvent::Presentation {
                item_id: "reply".into(),
                data: json!({"type":"message","text":"complete"}),
            },
            "root",
        );
        output.push(
            &DriverEvent::Presentation {
                item_id: "child".into(),
                data: json!({"type":"message","parentId":"tool","text":"child"}),
            },
            "root",
        );
        let items = output.changes();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].data["text"], "complete");
    }
}
