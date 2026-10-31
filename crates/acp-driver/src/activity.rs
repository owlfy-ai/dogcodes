//! Stateful native updates: replacement content and confirmed tool outcomes.
use driver_api::{DriverEvent, presentation::presentation_events};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(super) struct Activities {
    tools: HashMap<String, Value>,
    first_output: HashSet<String>,
    active_session: Option<String>,
}
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}
pub(super) fn permission_tool_name(request: &Value, fallback: &str) -> String {
    // Direct CodeBuddy MCP permission frames omit title, but identify the
    // actual tool in namespaced protocol metadata (not in its arguments).
    request["toolCall"]["_meta"]["codebuddy.ai/toolName"]
        .as_str()
        .filter(|name| name.starts_with("mcp__"))
        .unwrap_or(fallback)
        .to_owned()
}
fn emit(id: &str, data: Value) -> DriverEvent {
    DriverEvent::Presentation {
        item_id: id.into(),
        data,
    }
}
fn child(id: &str, u: &Value, status: &str) -> DriverEvent {
    let output = u["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|c| c.pointer("/content/text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let output = if output.is_empty() {
        u["rawOutput"].clone()
    } else {
        json!(output)
    };
    emit(
        id,
        json!({"type":"subagent","title":u["rawInput"]["description"],"input":u["rawInput"]["prompt"],"output":output,"details":"result_only","status":status}),
    )
}
impl Activities {
    pub fn begin_turn(&mut self, session: &str) {
        self.first_output.clear();
        self.active_session = Some(session.into());
    }
    pub fn reject(&mut self, session: &str, call: &str) {
        self.tools
            .entry(format!("{session}:{call}"))
            .or_insert_with(|| json!({}))["_rejected"] = json!(true);
    }
    /// Permission frames may contain only a vendor-truncated title. Reuse input
    /// already received for this exact session/tool, never another call's data.
    pub fn approval_input(&self, session: &str, call: &str) -> Option<Value> {
        let state = self.tools.get(&format!("{session}:{call}"))?;
        if state["_done"] == true {
            return None;
        }
        if let Some(input) = state.get("rawInput").filter(|input| !input.is_null()) {
            return Some(input.clone());
        }
        // Kimi streams the complete Bash argument JSON as replacement content.
        // An incomplete fragment or a prose approval summary is not input.
        let content = state["content"].as_array()?;
        if content.len() != 1 {
            return None;
        }
        let text = content[0].pointer("/content/text")?.as_str()?;
        let input: Value = serde_json::from_str(text).ok()?;
        input.get("command")?.as_str()?;
        Some(input)
    }

    pub fn permission_input(
        &self,
        session: &str,
        call: &str,
        input: Option<&Value>,
    ) -> Option<Value> {
        let known = self.approval_input(session, call);
        // CodeBuddy's deferred MCP permission frame sometimes contains just
        // params, while the exact unfinished tool already supplied its name.
        // Retain that wrapper only when its params equal the permission input.
        if let (Some(input), Some(known)) = (input, known.as_ref())
            && known["toolName"]
                .as_str()
                .is_some_and(|name| name.starts_with("mcp__"))
            && known.get("params") == Some(input)
            && known.get("command").is_none()
        {
            return Some(known.clone());
        }
        input.cloned().or(known)
    }

    pub fn update(&mut self, raw: &Value) -> Vec<DriverEvent> {
        let mut events = presentation_events("acp", "session/update", raw);
        let u = &raw["update"];
        if let Some(parent) = u["_meta"]["codebuddy.ai/parentToolCallId"]
            .as_str()
            .filter(|id| !id.is_empty())
        {
            let parent = format!("{}:{parent}", text(raw, "sessionId"));
            for event in &mut events {
                if let DriverEvent::Presentation { data, .. } = event {
                    data["parentId"] = json!(parent);
                }
            }
            return events;
        }
        if matches!(
            u["sessionUpdate"].as_str(),
            Some("agent_message_chunk" | "agent_thought_chunk")
        ) && u["content"]["text"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
            && self.active_session.as_deref() == raw["sessionId"].as_str()
            && self.first_output.insert(text(raw, "sessionId"))
        {
            // A root session ID is not a child ID. Core receives a content-free
            // signal, so private thought text need not become a timing event.
            events.insert(0, DriverEvent::FirstOutput);
        }
        if !matches!(
            u["sessionUpdate"].as_str(),
            Some("tool_call" | "tool_call_update")
        ) {
            return events;
        }
        let id = format!("{}:{}", text(raw, "sessionId"), text(u, "toolCallId"));
        let state = self.tools.entry(id.clone()).or_insert_with(|| json!({}));
        let already_done = state["_done"] == true;
        if let Some(fields) = u.as_object() {
            for (key, value) in fields {
                if !value.is_null() {
                    state[key] = value.clone();
                }
            }
        }
        // Cursor can report completed without a result after rejecting a tool.
        // That closes its protocol call; it is not successful command execution.
        if state["_rejected"] == true && state["status"] == "completed" {
            state["status"] = json!("failed");
            for event in &mut events {
                if let DriverEvent::Presentation { item_id, data } = event
                    && item_id == &id
                    && data["type"] == "tool"
                {
                    data["status"] = json!("failed");
                    data["output"] = json!("Rejected by the operator in dog.codes");
                }
            }
        }
        let codebuddy_child = state["_meta"]["codebuddy.ai/toolName"] == "Agent"
            || state["_meta"]["codebuddy.ai/isSubagent"] == true;
        if (codebuddy_child && state["_meta"]["codebuddy.ai/toolArgumentsComplete"] == true)
            || state["_codebuddyChild"] == true
        {
            let background = state["rawInput"]["run_in_background"] == true;
            let status = match state["status"].as_str() {
                Some("failed") => "failed",
                Some("completed") if !background => "completed",
                _ => "running",
            };
            let mut event = child(&id, state, status);
            if let DriverEvent::Presentation { data, .. } = &mut event {
                data["details"] = json!("live_only");
            }
            events.push(event);
            state["_codebuddyChild"] = json!(true);
            state["_settled"] = json!(status != "running");
        }
        if state["rawInput"]["_toolName"] == "task" {
            let status = if state["_settled"] == true {
                text(state, "_childStatus")
            } else {
                match state["status"].as_str() {
                    Some("pending") => "pending",
                    Some("failed") => "failed",
                    _ => "running",
                }
                .into()
            };
            events.push(child(&id, state, &status));
        }
        if !already_done && matches!(state["status"].as_str(), Some("completed" | "failed")) {
            state["_done"] = json!(true);
            if state["status"] == "completed" {
                let mut budget = 100_000;
                let changes: Vec<Value> = state["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| patch(c, &mut budget))
                    .collect();
                if !changes.is_empty() {
                    events.push(emit(
                        &id,
                        json!({"type":"tool","name":"edit","status":"completed","changes":changes}),
                    ));
                }
            }
        }
        events
    }
    pub fn cursor_task(&mut self, params: &Value) -> (Value, Option<DriverEvent>) {
        let call = text(params, "toolCallId");
        let found = self
            .tools
            .iter_mut()
            .find(|(_, u)| u["toolCallId"] == call && u["rawInput"]["_toolName"] == "task");
        let Some((id, u)) = found else {
            return (
                json!({"outcome":{"outcome":"rejected","reason":"Unknown Task"}}),
                None,
            );
        };
        if !matches!(u["status"].as_str(), Some("completed" | "failed")) {
            return (
                json!({"outcome":{"outcome":"rejected","reason":"Task has not completed"}}),
                None,
            );
        }
        let status = if u["status"] == "failed" {
            "failed"
        } else if u["rawOutput"]["isBackground"] == true {
            "running"
        } else {
            "completed"
        };
        u["_settled"] = json!(status != "running");
        u["_childStatus"] = json!(status);
        (
            json!({"outcome":{"outcome":"completed","agentId":params["agentId"],"durationMs":params["durationMs"]}}),
            Some(child(id, u, status)),
        )
    }
    pub fn finish(&mut self) -> Vec<DriverEvent> {
        self.tools
            .drain()
            .filter_map(|(id, u)| {
                if (u["rawInput"]["_toolName"] != "task" && u["_codebuddyChild"] != true)
                    || u["_settled"] == true
                {
                    return None;
                }
                let mut event = child(
                    &id,
                    &u,
                    if u["status"] == "failed" {
                        "failed"
                    } else {
                        "interrupted"
                    },
                );
                if u["_codebuddyChild"] == true
                    && let DriverEvent::Presentation { data, .. } = &mut event
                {
                    data["details"] = json!("live_only");
                }
                Some(event)
            })
            .collect()
    }
}
/// Bounded native patch; preserve exact EOLs and omit rather than truncate.
fn patch(c: &Value, budget: &mut usize) -> Option<Value> {
    if c["type"] != "diff" {
        return None;
    }
    let path = c["path"].as_str()?;
    if path.contains(['\n', '\r', '\t', '"', '\\']) {
        return None;
    }
    let old = if c["oldText"].is_null() {
        ""
    } else {
        c["oldText"].as_str()?
    };
    let new = c["newText"].as_str()?;
    if old == new
        || (old == "-- /dev/null" && new.starts_with("++ b/"))
        || old.len() + new.len() > *budget
    {
        return None;
    }
    let before = if c["oldText"].is_null() {
        "/dev/null".into()
    } else {
        format!("a/{path}")
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    let diff = similar::TextDiff::configure()
        .deadline(deadline)
        .diff_lines(old, new)
        .unified_diff()
        .header(&before, &format!("b/{path}"))
        .to_string();
    if std::time::Instant::now() >= deadline {
        return None;
    }
    if diff.len() > *budget {
        return None;
    }
    *budget -= diff.len();
    Some(json!({"path":path,"diff":diff}))
}
#[cfg(test)]
mod tests {
    #[test]
    fn first_output_is_per_root_turn_and_excludes_children() {
        use super::*;
        let mut activity = Activities::default();
        let root = json!({"sessionId":"s","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private"}}});
        let mut child = root.clone();
        child["update"]["_meta"] = json!({"codebuddy.ai/parentToolCallId":"child"});
        activity.begin_turn("s");
        let mut separate_child = root.clone();
        separate_child["sessionId"] = json!("child-session");
        assert!(
            !activity
                .update(&separate_child)
                .iter()
                .any(|event| matches!(event, DriverEvent::FirstOutput))
        );

        assert!(
            !activity
                .update(&child)
                .iter()
                .any(|event| matches!(event, DriverEvent::FirstOutput))
        );
        assert!(
            activity
                .update(&root)
                .iter()
                .any(|event| matches!(event, DriverEvent::FirstOutput))
        );
        assert!(
            !activity
                .update(&root)
                .iter()
                .any(|event| matches!(event, DriverEvent::FirstOutput))
        );
        activity.begin_turn("s");
        assert!(
            activity
                .update(&root)
                .iter()
                .any(|event| matches!(event, DriverEvent::FirstOutput))
        );
    }
    #[test]
    fn rejected_cursor_tool_completion_is_not_execution_success() {
        let mut activity = super::Activities::default();
        activity.reject("rejected-session", "call");
        for session in ["rejected-session", "other-session"] {
            let events = activity.update(&serde_json::json!({"sessionId":session,"update":{"sessionUpdate":"tool_call_update","toolCallId":"call","status":"completed"}}));
            let driver_api::DriverEvent::Presentation { data, .. } = &events[0] else {
                panic!("expected presentation")
            };
            assert_eq!(
                data["status"],
                if session == "rejected-session" {
                    "failed"
                } else {
                    "completed"
                }
            );
        }
        let events = activity.update(&serde_json::json!({"sessionId":"rejected-session","update":{"sessionUpdate":"tool_call_update","toolCallId":"different-call","status":"completed"}}));
        assert!(
            matches!(&events[0], driver_api::DriverEvent::Presentation { data, .. } if data["status"] == "completed")
        );
    }

    #[test]
    fn approval_input_requires_complete_matching_unfinished_tool() {
        let mut activity = super::Activities::default();
        let command = serde_json::json!({"command":"printf START > start; sleep 60; printf END > end","cwd":"/tmp/acceptance"});
        activity.update(&serde_json::json!({"sessionId":"session-a","update":{"sessionUpdate":"tool_call","toolCallId":"call","title":"Bash","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":command.to_string()}}]}}));
        assert_eq!(activity.approval_input("session-a", "call"), Some(command));
        assert_eq!(activity.approval_input("session-b", "call"), None);
        assert_eq!(activity.approval_input("session-a", "other"), None);
        activity.update(&serde_json::json!({"sessionId":"session-a","update":{"sessionUpdate":"tool_call_update","toolCallId":"call","content":[{"type":"content","content":{"type":"text","text":"{\"command\":"}}]}}));
        assert_eq!(activity.approval_input("session-a", "call"), None);
        activity.update(&serde_json::json!({"sessionId":"session-a","update":{"sessionUpdate":"tool_call_update","toolCallId":"call","rawInput":{"command":"true"}}}));
        assert_eq!(
            activity.approval_input("session-a", "call"),
            Some(serde_json::json!({"command":"true"}))
        );
        activity.update(&serde_json::json!({"sessionId":"session-a","update":{"sessionUpdate":"tool_call_update","toolCallId":"call","status":"completed"}}));
        assert_eq!(activity.approval_input("session-a", "call"), None);
    }

    #[test]
    fn deferred_mcp_permission_keeps_the_exact_calls_matching_wrapper() {
        let mut activity = super::Activities::default();
        let params = json!({"pid":33332,"session":"fixture"});
        let input = json!({"toolName":"mcp__dogcodes_cu_test__list_windows","params":params});
        activity.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"call","title":"DeferExecuteTool","rawInput":input}}));
        assert_eq!(
            activity.permission_input("s", "call", Some(&params)),
            Some(input)
        );
        let other = json!({"pid":7});
        assert_eq!(
            activity.permission_input("s", "call", Some(&other)),
            Some(other)
        );
        assert_eq!(
            activity.permission_input("other-session", "call", Some(&params)),
            Some(params.clone())
        );
        activity.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"call","status":"completed"}}));
        assert_eq!(
            activity.permission_input("s", "call", Some(&params)),
            Some(params)
        );
    }

    #[test]
    fn direct_mcp_approval_uses_native_tool_identity_without_promoting_arguments() {
        let request = json!({"toolCall":{"toolCallId":"call_9476e867b51744d881f78e2f","rawInput":{"pid":7},"_meta":{"codebuddy.ai/toolName":"mcp__dogcodes_cu_test__list_windows"}}});
        assert_eq!(
            permission_tool_name(&request, "call_9476e867b51744d881f78e2f"),
            "mcp__dogcodes_cu_test__list_windows"
        );
        let deferred = json!({"toolCall":{"rawInput":{"toolName":"mcp__dogcodes_cu_test__click","params":{}},"_meta":{"codebuddy.ai/toolName":"DeferExecuteTool"}}});
        assert_eq!(
            permission_tool_name(&deferred, "call_9476e867b51744d881f78e2f"),
            "call_9476e867b51744d881f78e2f"
        );
        let shell = json!({"toolCall":{"rawInput":{"_meta":{"codebuddy.ai/toolName":"mcp__dogcodes_cu_test__click"}},"_meta":{"codebuddy.ai/toolName":"Bash"}}});
        assert_eq!(permission_tool_name(&shell, "Bash"), "Bash");
    }

    use super::*;
    #[test]
    fn native_diff_waits_for_success_and_replaces_proposals() {
        let mut a = Activities::default();
        let raw = |update: Value| json!({"sessionId":"s","update":update});
        a.update(&raw(json!({"sessionUpdate":"tool_call","toolCallId":"t","status":"pending","content":[{"type":"diff","path":"a","oldText":"x\n","newText":"proposal\n"}]})));
        a.update(&raw(json!({"sessionUpdate":"tool_call_update","toolCallId":"t","content":[{"type":"diff","path":"a","oldText":"x\n","newText":"final\n"}]})));
        let done =
            raw(json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed"}));
        let changes = a.update(&done);
        assert!(
            matches!(changes.last().unwrap(),DriverEvent::Presentation { data, .. } if data["changes"][0]["diff"].as_str().unwrap().contains("+final\n"))
        );
        assert_eq!(a.update(&done).len(), 1);
    }
    #[test]
    fn malformed_creation_is_omitted_and_missing_newline_preserved() {
        assert!(
            patch(
                &json!({"type":"diff","path":"a","oldText":"-- /dev/null","newText":"++ b/a"}),
                &mut 100_000
            )
            .is_none()
        );
        let p = patch(
            &json!({"type":"diff","path":"a","oldText":null,"newText":"hello"}),
            &mut 100_000,
        )
        .unwrap();
        assert!(
            p["diff"]
                .as_str()
                .unwrap()
                .contains("\\ No newline at end of file")
        );
    }
    #[test]
    fn codebuddy_child_relationship_and_terminal_metadata_are_preserved() {
        let mut a = Activities::default();
        a.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"agent","status":"in_progress","_meta":{"codebuddy.ai/toolName":"Agent","codebuddy.ai/toolArgumentsComplete":true},"rawInput":{"prompt":"inspect"}}}));
        let child = a.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"read","_meta":{"codebuddy.ai/parentToolCallId":"agent"}}}));
        assert!(
            matches!(&child[0],DriverEvent::Presentation { data, .. } if data["parentId"] == "s:agent")
        );
        let done = a.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"agent","status":"completed","rawOutput":"found","_meta":{}}}));
        assert!(done.iter().any(|e| matches!(e,DriverEvent::Presentation { data, .. } if data["type"] == "subagent" && data["status"] == "completed" && data["output"] == "found")));
        assert!(a.finish().is_empty());
    }
    #[test]
    fn task_background_launch_is_not_completion() {
        let mut a = Activities::default();
        a.update(&json!({"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"t","status":"completed","rawInput":{"_toolName":"task","prompt":"inspect"},"rawOutput":{"isBackground":true}}}));
        let (_, e) = a.cursor_task(&json!({"toolCallId":"t"}));
        assert!(
            matches!(e.unwrap(),DriverEvent::Presentation { data, .. } if data["status"] == "running")
        );
        assert!(
            matches!(a.finish()[0],DriverEvent::Presentation { ref data, .. } if data["status"] == "interrupted")
        );
    }
}
