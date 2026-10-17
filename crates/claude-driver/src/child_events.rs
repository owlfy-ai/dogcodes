//! Route background children back to the stream that observed their creation.
use driver_api::{DriverEvent, presentation::presentation_events};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
pub(super) struct ChildEvents {
    owners: HashMap<String, UnboundedSender<DriverEvent>>,
    tasks: HashMap<String, String>,
    tool_names: HashMap<String, String>,
    settled: std::collections::HashSet<String>,
}
impl ChildEvents {
    pub fn dispatch(&mut self, raw: &Value, current: Option<&UnboundedSender<DriverEvent>>) {
        // Claude uses task_* notifications for background Bash as well as Agents.
        // Keep the originating tool identity instead of turning every task into a child Agent.
        if raw["type"] == "assistant"
            && let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array)
        {
            for block in blocks {
                if block["type"] == "tool_use"
                    && let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                {
                    self.tool_names.insert(id.to_owned(), name.to_owned());
                }
            }
        }
        // A successful Bash result can hand work to a native background task
        // before task_started arrives. Retain its original stream immediately.
        if raw["type"] == "user"
            && let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array)
        {
            let results: Vec<_> = blocks
                .iter()
                .filter(|b| b["type"] == "tool_result")
                .collect();
            if results.len() == 1 {
                let block = results[0];
                let id = block["tool_use_id"].as_str().unwrap_or_default();
                let native = raw
                    .get("tool_use_result")
                    .or_else(|| raw.get("toolUseResult"));
                if block["is_error"] != true
                    && self.tool_names.get(id).is_some_and(|name| name == "Bash")
                    && let Some(task) = native
                        .and_then(|v| v["backgroundTaskId"].as_str())
                        .filter(|id| !id.is_empty())
                    && let Some(sink) = current
                    && !self.settled.contains(id)
                {
                    self.owners.entry(id.into()).or_insert_with(|| sink.clone());
                    self.tasks.insert(task.into(), id.into());
                }
            }
        }
        let mut raw = raw.clone();
        if raw["status"] == "stopped" {
            raw["status"] = json!("interrupted");
        }
        let subtype = raw["subtype"].as_str().unwrap_or("").to_owned();
        let task = raw["task_id"].as_str().unwrap_or("").to_owned();
        let mut tool = raw["tool_use_id"].as_str().unwrap_or("").to_owned();
        if tool.is_empty()
            && let Some(known) = self.tasks.get(&task)
        {
            tool = known.clone();
            raw["tool_use_id"] = json!(tool);
        }
        if subtype == "task_notification"
            && (self.settled.contains(&tool) || self.settled.contains(&task))
        {
            return;
        }
        if subtype == "task_started"
            && !tool.is_empty()
            && let Some(sink) = current
        {
            self.owners
                .entry(tool.clone())
                .or_insert_with(|| sink.clone());
            if !task.is_empty() {
                self.tasks.insert(task.clone(), tool.clone());
            }
        }
        let parent = raw["parent_tool_use_id"].as_str().unwrap_or("");
        let key = if parent.is_empty() { &tool } else { parent };
        if let Some(sink) = self.owners.get(key).or(current) {
            for mut event in presentation_events("claude", "", &raw) {
                if subtype.starts_with("task_")
                    && let DriverEvent::Presentation { data, .. } = &mut event
                {
                    preserve_tool_kind(data, self.tool_names.get(&tool));
                }
                if raw["type"] == "user"
                    && let DriverEvent::Presentation { item_id, data } = &mut event
                {
                    if self.settled.contains(item_id) {
                        continue;
                    }
                    if self.defers_tool_completion(item_id) {
                        preserve_tool_kind(data, self.tool_names.get(item_id));
                        data["status"] = json!("running");
                    }
                }
                let destination = if let DriverEvent::Presentation { item_id, .. } = &event {
                    self.owners.get(item_id).unwrap_or(sink)
                } else {
                    sink
                };
                let _ = destination.send(event);
            }
        }
        if subtype == "task_notification"
            && matches!(
                raw["status"].as_str(),
                Some("completed" | "failed" | "stopped" | "interrupted")
            )
        {
            if !tool.is_empty() {
                self.settled.insert(tool.clone());
            }
            if !task.is_empty() {
                self.settled.insert(task.clone());
            }
            self.owners.remove(&tool);
            self.tasks.retain(|_, owner| owner != &tool);
            self.tool_names.remove(&tool);
        }
        if raw["type"] == "user"
            && let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array)
        {
            for block in blocks {
                if block["type"] == "tool_result"
                    && let Some(id) = block["tool_use_id"].as_str()
                    && !self.owners.contains_key(id)
                {
                    self.tool_names.remove(id);
                }
            }
        }
    }
    pub fn defers_tool_completion(&self, id: &str) -> bool {
        self.settled.contains(id)
            || (self.owners.contains_key(id)
                && self
                    .tool_names
                    .get(id)
                    .is_some_and(|name| !matches!(name.as_str(), "Agent" | "Task")))
    }
    pub fn close(&mut self) {
        self.close_with_status("interrupted");
    }
    pub fn fail(&mut self) {
        self.close_with_status("failed");
    }
    fn close_with_status(&mut self, status: &str) {
        for (id, sink) in self.owners.drain() {
            let mut data = json!({"type":"subagent","status":status,"details":"available"});
            preserve_tool_kind(&mut data, self.tool_names.get(&id));
            let _ = sink.send(DriverEvent::Presentation { item_id: id, data });
        }
        self.tasks.clear();
        self.tool_names.clear();
        self.settled.clear();
    }
}

fn preserve_tool_kind(data: &mut Value, name: Option<&String>) {
    if let Some(name) = name.filter(|name| !matches!(name.as_str(), "Agent" | "Task")) {
        data["type"] = json!("tool");
        data["name"] = json!(name);
        data["background"] = json!(true);
        if let Some(object) = data.as_object_mut() {
            object.remove("childId");
            object.remove("details");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_shell_stays_a_tool_in_its_original_turn() {
        for name in ["Bash", "Agent", "Task"] {
            let (old, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
            let (new, mut new_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut children = ChildEvents::default();
            children.dispatch(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"call","name":name,"input":{"command":"sleep 60"}}]}}), Some(&old));
            let _ = old_rx.try_recv().unwrap();
            children.dispatch(&json!({"type":"system","subtype":"task_started","task_id":"background","tool_use_id":"call","description":"long operation"}), Some(&old));
            children.dispatch(&json!({"type":"system","subtype":"task_notification","task_id":"background","status":"stopped","summary":"interrupted"}), Some(&new));
            for expected_status in ["running", "interrupted"] {
                let DriverEvent::Presentation { item_id, data } = old_rx.try_recv().unwrap() else {
                    panic!("expected presentation")
                };
                assert_eq!(item_id, "call");
                assert_eq!(data["status"], expected_status);
                assert_eq!(
                    data["type"],
                    if name == "Bash" { "tool" } else { "subagent" }
                );
                if name == "Bash" {
                    assert_eq!(data["name"], "Bash");
                    assert_eq!(data["background"], true);
                    assert!(data.get("childId").is_none());
                }
            }
            assert!(new_rx.try_recv().is_err());
            assert!(children.tool_names.is_empty());
        }
    }

    #[test]
    fn native_background_result_is_not_completion_and_stays_on_original_stream() {
        let (old, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        let (new, mut new_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        children.dispatch(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"shell","name":"Bash"}]}}), Some(&old));
        let _ = old_rx.try_recv().unwrap();
        let result = json!({"type":"user","tool_use_result":{"backgroundTaskId":"native-task"},"message":{"content":[{"type":"tool_result","tool_use_id":"shell","content":"started"}]}});
        children.dispatch(&result, Some(&old));
        assert!(children.defers_tool_completion("shell"));
        assert!(
            matches!(old_rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["status"] == "running" && data["background"] == true)
        );
        children.dispatch(&result, Some(&new));
        assert!(
            matches!(old_rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["status"] == "running")
        );
        assert!(new_rx.try_recv().is_err());
        children.dispatch(&json!({"type":"system","subtype":"task_notification","task_id":"native-task","status":"completed","summary":"done"}), Some(&new));
        assert!(
            matches!(old_rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["type"] == "tool" && data["status"] == "completed" && data["background"] == true)
        );
        children.dispatch(&result, Some(&new));
        assert!(new_rx.try_recv().is_err());
    }

    #[test]
    fn closing_background_shell_does_not_create_a_subagent() {
        let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        children.dispatch(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"shell","name":"Bash"}]}}), Some(&sink));
        children.dispatch(&json!({"type":"system","subtype":"task_started","task_id":"background","tool_use_id":"shell"}), Some(&sink));
        children.close();
        let _ = rx.try_recv().unwrap();
        let _ = rx.try_recv().unwrap();
        assert!(
            matches!(rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["type"] == "tool" && data["name"] == "Bash")
        );
        assert!(children.tool_names.is_empty());
    }

    #[test]
    fn settlement_keeps_original_turn_and_resolves_task_alias() {
        let (old, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        let (new, mut new_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        children.dispatch(&json!({"type":"system","subtype":"task_started","task_id":"child","tool_use_id":"call"}), Some(&old));
        drop(old);
        children.dispatch(&json!({"type":"system","subtype":"task_notification","task_id":"child","status":"completed","summary":"done"}), Some(&new));
        assert!(
            matches!(old_rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["status"] == "running")
        );
        assert!(
            matches!(old_rx.try_recv().unwrap(), DriverEvent::Presentation { item_id, data } if item_id == "call" && data["status"] == "completed")
        );
        assert!(new_rx.try_recv().is_err());
        children.dispatch(&json!({"type":"system","subtype":"task_notification","task_id":"child","tool_use_id":"call","status":"completed"}), Some(&new));
        assert!(new_rx.try_recv().is_err());
        assert!(children.owners.is_empty());
    }
    #[test]
    fn idle_settlements_do_not_require_a_new_parent_turn() {
        for status in ["completed", "failed", "stopped"] {
            let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut children = ChildEvents::default();
            children.dispatch(&json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"call"}), Some(&sink));
            drop(sink);
            children.dispatch(&json!({"type":"system","subtype":"task_notification","task_id":"task","status":status}), None);
            let _ = rx.try_recv().unwrap();
            assert!(
                matches!(rx.try_recv().unwrap(),DriverEvent::Presentation { data, .. } if data["status"] == if status == "stopped" { "interrupted" } else { status })
            );
            assert!(children.owners.is_empty());
        }
    }
    #[test]
    fn process_close_interrupts_unconfirmed_children() {
        let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        children.dispatch(
            &json!({"type":"system","subtype":"task_started","tool_use_id":"call"}),
            Some(&sink),
        );
        children.close();
        let _ = rx.try_recv().unwrap();
        assert!(
            matches!(rx.try_recv().unwrap(), DriverEvent::Presentation { data, .. } if data["status"] == "interrupted")
        );
    }
}
