//! Follow only output files identified by this session's native background Bash results.
use crate::child_events::ChildEvents;
use driver_api::DriverEvent;
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};
use tokio::{fs::File, io::AsyncReadExt, sync::mpsc::UnboundedSender};

// Match the presentation layer's bounded output, without retaining an unbounded log.
const OUTPUT_LIMIT: usize = 24_000;
#[derive(Default)]
pub(super) struct BackgroundOutputs {
    files: HashMap<String, FollowedCommand>,
}
struct FollowedCommand {
    task: String,
    path: Option<PathBuf>,
    sink: UnboundedSender<DriverEvent>,
    file: Option<File>,
    output: String,
    pending: Vec<u8>,
    opened: bool,
    truncated: bool,
}
impl BackgroundOutputs {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn observe(
        &mut self,
        raw: &Value,
        children: &ChildEvents,
        current: Option<&UnboundedSender<DriverEvent>>,
    ) {
        if raw["type"] != "user" {
            return;
        }
        let native = raw
            .get("tool_use_result")
            .or_else(|| raw.get("toolUseResult"));
        let Some(task) = native
            .and_then(|v| v["backgroundTaskId"].as_str())
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let results: Vec<_> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .collect();
        if results.len() != 1 || results[0]["is_error"] == true {
            return;
        }
        let block = results[0];
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        let Some(sink) = children.background_owner(id, current) else {
            return;
        };
        // Replayed native handoffs must not rewind the output file or change its owner.
        self.files
            .entry(id.into())
            .or_insert_with(|| FollowedCommand {
                task: task.into(),
                path: output_path(native.unwrap()).or_else(|| path_in_content(&block["content"])),
                sink,
                file: None,
                output: String::new(),
                pending: vec![],
                opened: false,
                truncated: false,
            });
    }

    pub async fn poll(&mut self) {
        for (id, output) in &mut self.files {
            output.read(id, false).await;
        }
    }

    // Read before the native terminal is published; no poll can append after completion.
    pub async fn settle(&mut self, raw: &mut Value) {
        if raw["type"] != "system"
            || raw["subtype"] != "task_notification"
            || !matches!(
                raw["status"].as_str(),
                Some("completed" | "failed" | "stopped" | "interrupted")
            )
        {
            return;
        }
        let id = self
            .files
            .iter()
            .find(|(id, o)| {
                let tool = raw["tool_use_id"].as_str().filter(|s| !s.is_empty());
                let task = raw["task_id"].as_str().filter(|s| !s.is_empty());
                (tool == Some(id.as_str()) || task == Some(o.task.as_str()))
                    && tool.is_none_or(|t| t == id.as_str())
                    && task.is_none_or(|t| t == o.task)
            })
            .map(|(id, _)| id.clone());
        let Some(id) = id else {
            return;
        };
        let mut output = self.files.remove(&id).expect("matched output");
        output.path = output.path.or_else(|| output_path(raw));
        output.read(&id, true).await;
        if output.opened {
            raw["summary"] = json!(output.output);
            raw["outputTruncated"] = json!(output.truncated);
        }
    }

    pub async fn finish(&mut self) {
        for (id, output) in &mut self.files {
            output.read(id, true).await;
        }
        self.files.clear();
    }
}

fn output_path(value: &Value) -> Option<PathBuf> {
    value
        .get("output_file")
        .or_else(|| value.get("outputFile"))?
        .as_str()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}
fn path_in_content(content: &Value) -> Option<PathBuf> {
    let text = content.as_str().map(str::to_owned).or_else(|| {
        content.as_array().map(|blocks| {
            blocks
                .iter()
                .filter_map(|v| v["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
    })?;
    let path = text
        .split_once("Output is being written to: ")?
        .1
        .lines()
        .next()?
        .trim();
    let path = path.find(".output").map_or(path, |end| &path[..end + 7]);
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

impl FollowedCommand {
    async fn read(&mut self, id: &str, final_read: bool) {
        if self.truncated {
            return;
        }
        if self.file.is_none() {
            let Some(path) = &self.path else {
                return;
            };
            // Never open a pipe, device or leaf symlink as a native output log.
            if !tokio::fs::symlink_metadata(path)
                .await
                .is_ok_and(|m| m.file_type().is_file())
            {
                return;
            }
            self.file = File::open(path).await.ok();
            self.opened = self.file.is_some();
        }
        let Some(file) = &mut self.file else {
            return;
        };
        let before = self.output.len();
        let mut buffer = [0_u8; 4096];
        for _ in 0..8 {
            let read = match file.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            self.pending.extend_from_slice(&buffer[..read]);
            loop {
                let (valid, skip) = match std::str::from_utf8(&self.pending) {
                    Ok(_) => (self.pending.len(), 0),
                    Err(e) => (e.valid_up_to(), e.error_len().unwrap_or(0)),
                };
                let text = std::str::from_utf8(&self.pending[..valid]).expect("valid UTF-8 prefix");
                let mut end = text
                    .len()
                    .min(OUTPUT_LIMIT.saturating_sub(self.output.len()));
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                self.output.push_str(&text[..end]);
                if end < text.len() {
                    self.truncated = true;
                    self.pending.clear();
                    break;
                }
                self.pending.drain(..valid + skip);
                if skip == 0 {
                    break;
                }
                if self.output.len() + 3 > OUTPUT_LIMIT {
                    self.truncated = true;
                    self.pending.clear();
                    break;
                }
                self.output.push('\u{fffd}');
            }
            if self.truncated {
                break;
            }
        }
        if final_read && !self.pending.is_empty() && !self.truncated {
            if self.output.len() + 3 <= OUTPUT_LIMIT {
                self.output.push('\u{fffd}');
            } else {
                self.truncated = true;
            }
            self.pending.clear();
        }
        if before != self.output.len() || self.truncated {
            let _ = self.sink.send(DriverEvent::Presentation { item_id: id.into(), data: json!({"type":"tool","name":"Bash","background":true,"output":self.output,"outputTruncated":self.truncated}) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    fn start(children: &mut ChildEvents, sink: &UnboundedSender<DriverEvent>, id: &str) {
        children.dispatch(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":"Bash"}]}}),Some(sink));
    }
    fn handoff(id: &str, path: &std::path::Path) -> Value {
        json!({"type":"user","tool_use_result":{"backgroundTaskId":format!("task-{id}")},"message":{"content":[{"type":"tool_result","tool_use_id":id,"content":format!("Started. Output is being written to: {}. You can read it later.",path.display())}]}})
    }
    fn terminal(id: &str) -> Value {
        json!({"type":"system","subtype":"task_notification","task_id":format!("task-{id}"),"status":"completed","summary":"native summary"})
    }
    #[tokio::test]
    async fn follows_split_utf8_preserves_owner_and_flushes_before_terminal() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("with space.output");
        let (old, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        let (new, mut new_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        let mut outputs = BackgroundOutputs::default();
        start(&mut children, &old, "shell");
        let _ = old_rx.try_recv().unwrap();
        let result = handoff("shell", &path);
        outputs.observe(&result, &children, Some(&old));
        children.dispatch(&result, Some(&old));
        let _ = old_rx.try_recv().unwrap();
        outputs.poll().await;
        assert!(old_rx.try_recv().is_err()); // File not created yet.
        tokio::fs::write(&path, [b'a', 0xe4, 0xbd]).await.unwrap();
        outputs.poll().await;
        assert!(
            matches!(old_rx.try_recv().unwrap(),DriverEvent::Presentation {data,..} if data["output"] == "a")
        );
        // Native handoff replay cannot reset the decoder, offset or original stream.
        outputs.observe(&result, &children, Some(&new));
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        file.write_all(&[0xa0, b'b']).await.unwrap();
        let mut done = terminal("shell");
        outputs.settle(&mut done).await;
        children.dispatch(&done, Some(&new));
        assert!(
            matches!(old_rx.try_recv().unwrap(),DriverEvent::Presentation{data,..} if data["output"] == "a你b" && data.get("status").is_none())
        );
        assert!(
            matches!(old_rx.try_recv().unwrap(),DriverEvent::Presentation{data,..} if data["output"] == "a你b" && data["status"] == "completed")
        );
        outputs.poll().await;
        assert!(outputs.is_empty());
        assert!(old_rx.try_recv().is_err());
        assert!(new_rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn late_paths_empty_files_and_bounded_output_keep_native_completion() {
        let root = tempfile::tempdir().unwrap();
        let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        let mut outputs = BackgroundOutputs::default();
        for (id, bytes) in [
            ("empty", vec![]),
            ("large", vec![b'x'; OUTPUT_LIMIT + 100]),
            ("partial", vec![0xe4, 0xbd]),
        ] {
            let path = root.path().join(id);
            tokio::fs::write(&path, bytes).await.unwrap();
            start(&mut children, &sink, id);
            let mut result = handoff(id, &path);
            result["message"]["content"][0]["content"] = json!("background");
            outputs.observe(&result, &children, Some(&sink));
            let mut done = terminal(id);
            done["output_file"] = json!(path);
            outputs.settle(&mut done).await;
            assert_eq!(
                done["summary"].as_str().unwrap().len(),
                match id {
                    "large" => OUTPUT_LIMIT,
                    "partial" => 3,
                    _ => 0,
                }
            );
            assert_eq!(done["outputTruncated"], id == "large");
        }
        while rx.try_recv().is_ok() {}
        // An unknown task, foreground result or failing tool cannot turn into a log reader.
        outputs.observe(
            &handoff("unknown", &root.path().join("large")),
            &children,
            Some(&sink),
        );
        start(&mut children, &sink, "failed");
        let mut failed = handoff("failed", &root.path().join("large"));
        failed["message"]["content"][0]["is_error"] = json!(true);
        outputs.observe(&failed, &children, Some(&sink));
        assert!(outputs.is_empty());
    }
    #[tokio::test]
    async fn independent_commands_missing_logs_and_shutdown_are_isolated() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a.output");
        tokio::fs::write(&path, "first").await.unwrap();
        let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut children = ChildEvents::default();
        let mut outputs = BackgroundOutputs::default();
        for id in ["a", "b"] {
            start(&mut children, &sink, id);
            outputs.observe(
                &handoff(
                    id,
                    &if id == "a" {
                        path.clone()
                    } else {
                        root.path().join("missing.output")
                    },
                ),
                &children,
                Some(&sink),
            );
        }
        let mut unknown = terminal("unrelated");
        outputs.settle(&mut unknown).await;
        assert_eq!(outputs.files.len(), 2);
        let mut done = terminal("b");
        outputs.settle(&mut done).await;
        assert_eq!(done["summary"], "native summary");
        outputs.finish().await;
        assert!(outputs.is_empty());
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|e|matches!(e,DriverEvent::Presentation{item_id,data} if item_id == "a" && data["output"] == "first")));
        assert!(!events.iter().any(|e|matches!(e,DriverEvent::Presentation{item_id,data} if item_id == "b" && data.get("output").is_some())));
    }
}
