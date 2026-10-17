//! Reply boundaries follow new root activity, never late progress of an old tool.
use driver_api::DriverEvent;
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct TextSegments {
    tools: BTreeSet<String>,
    boundary: bool,
    ordinal: u64,
}

impl TextSegments {
    pub(crate) fn apply(&mut self, event: &mut DriverEvent) {
        match event {
            DriverEvent::Presentation { item_id, data } => {
                if data["parentId"].as_str().is_some_and(|id| !id.is_empty()) {
                    return;
                }
                match data["type"].as_str() {
                    Some("message") => self.text(item_id),
                    Some("tool" | "subagent") => {
                        let native = data["nativeToolId"].as_str().unwrap_or(item_id);
                        // Remember both identities: the canonical tool event can precede
                        // its richer, session-scoped presentation (or follow it).
                        let known = self.tools.contains(native) || self.tools.contains(item_id);
                        self.tools.insert(native.to_owned());
                        self.tools.insert(item_id.clone());
                        self.boundary |= !known;
                    }
                    Some("thought") => self.boundary = true,
                    _ => {}
                }
            }
            DriverEvent::TextDelta { item_id, .. } => self.text(item_id),
            DriverEvent::ToolStarted { item_id, .. }
            | DriverEvent::ToolCompleted { item_id, .. } => {
                self.boundary |= self.tools.insert(item_id.clone());
            }
            _ => {}
        }
    }

    fn text(&mut self, id: &mut String) {
        if self.boundary {
            self.ordinal += 1;
            self.boundary = false;
        }
        *id = format!("{id}:segment-{}", self.ordinal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(segments: &mut TextSegments) -> String {
        let mut event = DriverEvent::TextDelta {
            item_id: "answer".into(),
            text: "x".into(),
        };
        segments.apply(&mut event);
        let DriverEvent::TextDelta { item_id, .. } = event else {
            unreachable!()
        };
        item_id
    }

    #[test]
    fn late_progress_and_completion_do_not_split_the_answer() {
        let mut segments = TextSegments::default();
        let before = text(&mut segments);
        segments.apply(&mut DriverEvent::ToolStarted {
            item_id: "t".into(),
            tool_name: "Bash".into(),
            summary: String::new(),
        });
        let answer = text(&mut segments);
        assert_ne!(before, answer);
        segments.apply(&mut DriverEvent::Presentation {
            item_id: "session:t".into(),
            data: json!({"type":"tool","nativeToolId":"t","status":"running"}),
        });
        segments.apply(&mut DriverEvent::ToolCompleted {
            item_id: "session:t".into(),
            tool_name: "Bash".into(),
            summary: String::new(),
            success: true,
        });
        assert_eq!(answer, text(&mut segments));
        segments.apply(&mut DriverEvent::Presentation {
            item_id: "new".into(),
            data: json!({"type":"tool","status":"completed"}),
        });
        assert_ne!(answer, text(&mut segments));
    }

    #[test]
    fn child_activity_and_repeated_subagents_preserve_root_answer() {
        let mut segments = TextSegments::default();
        segments.apply(&mut DriverEvent::Presentation {
            item_id: "child".into(),
            data: json!({"type":"subagent","status":"running"}),
        });
        let answer = text(&mut segments);
        for data in [
            json!({"type":"thought","parentId":"child"}),
            json!({"type":"tool","parentId":"child"}),
            json!({"type":"subagent","status":"completed"}),
        ] {
            segments.apply(&mut DriverEvent::Presentation {
                item_id: "child".into(),
                data,
            });
            assert_eq!(answer, text(&mut segments));
        }
    }
}
