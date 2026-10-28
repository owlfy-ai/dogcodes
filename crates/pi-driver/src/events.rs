use agent_domain::QuestionForm;
use driver_api::{DriverError, DriverEvent, explicitly_reports_quota_exhaustion};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;

pub(crate) struct Dialog {
    pub method: String,
    pub form: QuestionForm,
}
pub(crate) struct Turn {
    pub id: uuid::Uuid,
    pub sender: mpsc::UnboundedSender<Result<DriverEvent, DriverError>>,
    pub interrupted: bool,
    pub dialogs: HashMap<String, Dialog>,
    message: u64,
    text: HashMap<u64, String>,
    failure: Option<DriverEvent>,
}
impl Turn {
    pub fn new(sender: mpsc::UnboundedSender<Result<DriverEvent, DriverError>>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            sender,
            interrupted: false,
            dialogs: HashMap::new(),
            message: 0,
            text: HashMap::new(),
            failure: None,
        }
    }
    fn emit(&self, event: DriverEvent) {
        let _ = self.sender.send(Ok(event));
    }
    fn append(&mut self, index: u64, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.text.entry(index).or_default().push_str(delta);
        self.emit(DriverEvent::TextDelta {
            item_id: format!("pi-message-{}-{index}", self.message),
            text: delta.into(),
        });
    }
    pub fn observe(&mut self, value: &Value) -> bool {
        for event in driver_api::presentation::presentation_events("pi", "", value) {
            self.emit(event);
        }
        match value["type"].as_str().unwrap_or("") {
            "message_start" if value["message"]["role"] == "assistant" => {
                self.message += 1;
                self.text.clear();
                // A subsequent successful retry supersedes a prior provider error.
                self.failure = None;
            }
            "message_update" => {
                let update = &value["assistantMessageEvent"];
                if update["type"] == "text_delta" {
                    self.append(
                        update["contentIndex"].as_u64().unwrap_or(0),
                        update["delta"].as_str().unwrap_or(""),
                    );
                }
            }
            "message_end" if value["message"]["role"] == "assistant" => {
                let message = &value["message"];
                for (index, block) in message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if block["type"] != "text" {
                        continue;
                    }
                    let complete = block["text"].as_str().unwrap_or("");
                    let seen = self.text.get(&(index as u64)).map_or("", String::as_str);
                    // Some models only emit their final message; do not duplicate
                    // content for models that already sent incremental deltas.
                    if let Some(rest) = complete.strip_prefix(seen) {
                        self.append(index as u64, rest);
                    }
                }
                match message["stopReason"].as_str() {
                    Some("aborted") => self.interrupted = true,
                    Some("error") => {
                        self.failure = Some(
                            if explicitly_reports_quota_exhaustion(
                                message["errorMessage"].as_str().unwrap_or(""),
                            ) {
                                DriverEvent::TurnQuotaExhausted {
                                    resets_at_unix: None,
                                }
                            } else {
                                DriverEvent::TurnFailed {
                                    reason: "Pi 模型调用失败，请检查原生客户端的模型授权与网络。"
                                        .into(),
                                }
                            },
                        );
                    }
                    _ => {}
                }
            }
            "tool_execution_start" => self.emit(DriverEvent::ToolStarted {
                item_id: value["toolCallId"].as_str().unwrap_or("pi-tool").into(),
                tool_name: value["toolName"].as_str().unwrap_or("tool").into(),
                summary: format!("正在使用 {}", value["toolName"].as_str().unwrap_or("工具")),
            }),
            "tool_execution_end" => self.emit(DriverEvent::ToolCompleted {
                item_id: value["toolCallId"].as_str().unwrap_or("pi-tool").into(),
                tool_name: value["toolName"].as_str().unwrap_or("tool").into(),
                summary: if value["isError"] == true {
                    "工具执行失败"
                } else {
                    "工具执行完成"
                }
                .into(),
                success: value["isError"] != true,
            }),
            "extension_ui_request" => {
                if let Some((id, dialog)) = dialog(value) {
                    self.emit(DriverEvent::QuestionRequested {
                        native_request_id: id.clone(),
                        form: dialog.form.clone(),
                    });
                    self.dialogs.insert(id, dialog);
                }
            }
            "agent_settled" => {
                for id in self.dialogs.keys() {
                    self.emit(DriverEvent::QuestionClosed {
                        native_request_id: id.clone(),
                    });
                }
                let terminal = if self.interrupted {
                    DriverEvent::TurnInterrupted {
                        summary: "已停止".into(),
                    }
                } else {
                    self.failure.take().unwrap_or(DriverEvent::TurnCompleted {
                        summary: "完成".into(),
                    })
                };
                self.emit(terminal);
                return true;
            }
            // agent_end is NOT terminal: Pi may retry or compact afterwards.
            _ => {}
        }
        false
    }
}
fn dialog(value: &Value) -> Option<(String, Dialog)> {
    let id = value["id"].as_str()?.to_owned();
    let method = value["method"].as_str()?;
    let mut property = match method {
        "confirm" => json!({"type":"boolean","description":value["message"]}),
        "select" => json!({"type":"string","enum":value["options"]}),
        "input" | "editor" => json!({"type":"string"}),
        _ => return None,
    };
    property["title"] = value["title"].clone();
    if method == "editor" && value["prefill"].is_string() {
        property["default"] = value["prefill"].clone();
    }
    Some((
        id,
        Dialog {
            method: method.into(),
            form: QuestionForm {
                title: value["title"].as_str().unwrap_or("Pi 请求输入").into(),
                schema: json!({"type":"object","properties":{"answer":property},"required":["answer"]}),
            },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_is_not_completed_at_agent_end_and_text_is_not_duplicated() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut turn = Turn::new(sender);
        turn.observe(&json!({"type":"message_start","message":{"role":"assistant"}}));
        turn.observe(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello"}}));
        turn.observe(&json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}],"stopReason":"stop"}}));
        assert!(!turn.observe(&json!({"type":"agent_end","willRetry":false})));
        assert!(turn.observe(&json!({"type":"agent_settled"})));
        let mut text = String::new();
        let mut terminals = 0;
        while let Ok(event) = receiver.try_recv() {
            match event.unwrap() {
                DriverEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                DriverEvent::TurnCompleted { .. } => terminals += 1,
                _ => panic!("unexpected event"),
            }
        }
        assert_eq!(text, "Hello world");
        assert_eq!(terminals, 1);
    }
    #[test]
    fn native_dialogs_keep_exact_values_and_do_not_invent_permission_options() {
        let (_, dialog) = dialog(&json!({"type":"extension_ui_request","id":"a","method":"select","title":"Choose","options":["Allow once","Block"]})).unwrap();
        assert_eq!(
            dialog.form.schema["properties"]["answer"]["enum"],
            json!(["Allow once", "Block"])
        );
        assert!(
            dialog
                .form
                .validate(&agent_domain::QuestionAnswer {
                    cancelled: false,
                    values: json!({"answer":"invented"})
                })
                .is_err()
        );
    }
}
