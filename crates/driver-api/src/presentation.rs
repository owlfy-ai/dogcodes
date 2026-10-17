//! Allowlisted, displayable native content. Never forwards entire vendor frames.
use crate::DriverEvent;
use serde_json::{Value, json};
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn bounded(v: &Value) -> Value {
    match v {
        Value::String(t) => Value::String(t.chars().take(24000).collect()),
        Value::Array(a) => Value::Array(a.iter().take(80).map(bounded).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| {
                    !["signature", "authorization", "api_key", "token", "secret"]
                        .iter()
                        .any(|x| k.to_lowercase().contains(x))
                })
                .take(80)
                .map(|(k, v)| (k.clone(), bounded(v)))
                .collect(),
        ),
        _ => v.clone(),
    }
}
fn event(id: &str, mut data: Value) -> DriverEvent {
    let original = serde_json::to_vec(&data).map_or(0, |v| v.len());
    for key in ["title", "name"] {
        if let Some(text) = data.get(key).and_then(Value::as_str) {
            let short: String = text.chars().take(200).collect();
            data[key] = json!(short);
        }
    }
    let mut data = bounded(&data);
    if original > serde_json::to_vec(&data).map_or(0, |v| v.len()) {
        data["partial"] = json!(true);
    }
    DriverEvent::Presentation {
        item_id: id.into(),
        data,
    }
}
fn tool(
    id: &str,
    name: &str,
    state: &str,
    input: Value,
    output: Value,
    parent: &str,
) -> DriverEvent {
    let mut data = json!({"type":"tool","parentId":parent});
    if !name.is_empty() {
        data["name"] = json!(name);
    }
    if !state.is_empty() {
        data["status"] = json!(state);
    }
    if !input.is_null() {
        data["input"] = input;
    }
    if !output.is_null() {
        data["output"] = output;
    }
    event(id, data)
}
/// Only documented display text, tool inputs/results and explicit relationships.
pub fn presentation_events(provider: &str, method: &str, v: &Value) -> Vec<DriverEvent> {
    let mut out = vec![];
    match provider {
        "pi" => match s(v, "type") {
            "tool_execution_start" => out.push(tool(s(v,"toolCallId"), s(v,"toolName"), "running", v["args"].clone(), Value::Null, "")),
            "tool_execution_update" => out.push(tool(s(v,"toolCallId"), s(v,"toolName"), "running", Value::Null, v["partialResult"]["content"].clone(), "")),
            "tool_execution_end" => out.push(tool(s(v,"toolCallId"), s(v,"toolName"), if v["isError"] == true {"failed"} else {"completed"}, Value::Null, v["result"]["content"].clone(), "")),
            "message_update" => {
                let update = &v["assistantMessageEvent"];
                if update["type"] == "thinking_delta" {
                    out.push(event("pi-thought",json!({"type":"thought","delta":s(update,"delta"),"status":"running"})));
                }
            }
            _ => {}
        },
        "claude" => {
            let parent=s(v,"parent_tool_use_id");
            match s(v,"type") {
                "assistant"|"user" => {
                    if let Some(blocks)=v.pointer("/message/content").and_then(Value::as_array){for (index,b) in blocks.iter().enumerate() {
                        match s(b,"type") {
                            "tool_use" => out.push(tool(s(b,"id"),s(b,"name"),"running",b["input"].clone(),Value::Null,parent)),
                            "tool_result" => out.push(tool(s(b,"tool_use_id"),"",if b["is_error"]==true{"failed"}else{"completed"},Value::Null,b["content"].clone(),parent)),
                            "text" if !parent.is_empty()=>out.push(event(&format!("{}:text:{index}",s(&v["message"],"id")),json!({"type":"message","parentId":parent,"text":s(b,"text")}))),
                            _=>{}
                        }
                    }}
                }
                "system" if s(v,"subtype").starts_with("task_")=> {
                    let id=s(v,"tool_use_id");
                    if !id.is_empty(){let mut data=json!({"type":"subagent","childId":s(v,"task_id"),"details":"available"});
                        for (src,dst) in [("description","title"),("status","status"),("summary","output")] {if let Some(x)=v.get(src){data[dst]=x.clone();}}
                        if s(v,"subtype")=="task_started"{data["status"]=json!("running");}
                        if let Some(n)=v.pointer("/usage/duration_ms"){data["durationMs"]=n.clone();}
                        out.push(event(id,data));}
                }
                "stream_event"=>{
                    let e=&v["event"];let d=&e["delta"];
                    if s(d,"type")=="thinking_delta" {out.push(event(&format!("thought:{parent}"),json!({"type":"thought","parentId":parent,"delta":s(d,"thinking"),"status":"running"})));}
                }
                _=>{}
            }
        }
        "codex"=>{
            let i=&v["item"];let id=s(i,"id");
            if method=="item/started" || method=="item/completed" {
                let state=if method=="item/started"{"running"}else if ["failed","declined"].contains(&s(i,"status")){"failed"}else{"completed"};
                match s(i,"type") {
                    "commandExecution"=>out.push(tool(id,"terminal",state,i["command"].clone(),i["aggregatedOutput"].clone(),"")),
                    "fileChange"=>out.push(event(id,json!({"type":"tool","name":"edit","status":state,"changes":i["changes"]}))),
                    "mcpToolCall"|"dynamicToolCall"=>out.push(tool(id,s(i,"tool"),state,i["arguments"].clone(),i["result"].clone(),"")),
                    "webSearch"=>out.push(tool(id,"web_search",state,i["query"].clone(),i["action"].clone(),"")),
                    "collabAgentToolCall"=>{
                        out.push(tool(id,s(i,"tool"),state,i["prompt"].clone(),i["agentsStates"].clone(),""));
                        if let Some(ids)=i["receiverThreadIds"].as_array(){for child in ids.iter().filter_map(Value::as_str){
                            let reported=i["agentsStates"][child]["status"].as_str().unwrap_or("unknown");
                            out.push(event(child,json!({"type":"subagent","childId":child,"details":"history","status":reported})));
                        }}
                    },
                    "subAgentActivity"=>out.push(event(s(i,"agentThreadId"),json!({"type":"subagent","childId":s(i,"agentThreadId"),"title":s(i,"agentPath"),"status":s(i,"kind"),"details":"history"}))),
                    _=>{}
                }
            } else if method=="item/reasoning/summaryTextDelta" {
                out.push(event(&format!("thought:{}",s(v,"itemId")),json!({"type":"thought","delta":s(v,"delta"),"status":"running"})));
            }
        }
        "zcode"=>{
            let p=v.get("payload").unwrap_or(v);
            let method=if method=="session/event"{s(v,"type")}else{method};
            let parent=s(p,"parentToolCallId");
            if method=="tool.updated" {
                let state=match s(p,"kind"){"scheduled"=>"pending","started"=>"running","result"|"completed"=>if p.pointer("/result/success")==Some(&Value::Bool(false)){"failed"}else{"completed"},"failed"|"error"=>"failed",_=>""};
                out.push(tool(s(p,"toolCallId"),s(p,"toolName"),state,p.get("input").or_else(||p.get("args")).cloned().unwrap_or(Value::Null),p["result"].clone(),parent));
                if !parent.is_empty(){out.push(event(parent,json!({"type":"subagent","childId":s(p,"childSessionId"),"title":s(p,"description"),"details":"available"})));}
            }else if method=="stream.delta" && s(p,"kind")=="text_delta" && !parent.is_empty() {
                out.push(event(&format!("child-message:{parent}"),json!({"type":"message","parentId":parent,"textDelta":s(p,"delta")})));
            }else if method=="stream.delta" && s(p,"kind")=="reasoning_delta" {
                out.push(event(&format!("thought:{parent}"),json!({"type":"thought","parentId":parent,"delta":s(p,"delta"),"status":"running"})));
            }
        }
        "acp"=>{
            let u=v.get("update").unwrap_or(v); let session=s(v,"sessionId");
            let id=format!("{session}:{}",s(u,"toolCallId"));
            match s(u,"sessionUpdate") {
                "tool_call"|"tool_call_update"=>{
                    let mut data=json!({"type":"tool","sessionId":session,"nativeToolId":s(u,"toolCallId")});
                    for (src,dst) in [("title","name"),("kind","category"),("status","status"),("rawInput","input"),("rawOutput","output"),("content","content"),("locations","locations")] {if let Some(x)=u.get(src)&& !x.is_null(){data[dst]=x.clone();}}
                    if let Some(name)=u.pointer("/_meta/x.ai~1tool/name").and_then(Value::as_str){data["name"]=json!(name);}
                    out.push(event(&id,data));
                }
                "agent_message_chunk"=>out.push(event(&format!("message:{session}"),json!({"type":"message","sessionId":session,"textDelta":s(&u["content"],"text")}))),
                "agent_thought_chunk"=>out.push(event(&format!("thought:{session}"),json!({"type":"thought","sessionId":session,"delta":s(&u["content"],"text"),"status":"running"}))),
                _=>{}
            }
        }
        "grok"=>{
            let u=&v["update"];let id=s(u,"child_session_id");
            if s(u,"sessionUpdate").starts_with("subagent_") {
                let mut data=json!({"type":"subagent","childId":id,"childSessionId":id,"parentSessionId":s(u,"parent_session_id"),"details":"available"});
                for (src,dst) in [("description","title"),("status","status"),("output","output"),("duration_ms","durationMs")] {if let Some(x)=u.get(src){data[dst]=x.clone();}}
                if s(u,"sessionUpdate")=="subagent_spawned"{data["status"]=json!("running");}
                out.push(event(id,data));
            }
        }
        "cursor"=>out.push(event(s(v,"toolCallId"),json!({"type":"subagent","childId":s(v,"agentId"),"title":s(v,"description"),"details":"unavailable","durationMs":v["durationMs"]}))),
        "antigravity"=>{
            let p=&v["step_update"];
            if s(p,"step_type")=="tool" {
                out.push(tool(&format!("step:{}:{}",s(p,"conversation_id"),p["step_index"]),s(p,"tool_name"),match s(p,"state"){"DONE"=>"completed","ERROR"|"FAILED"=>"failed",_=>"running"},p.pointer("/tool_info/input").cloned().unwrap_or(Value::Null),p.pointer("/tool_info/output").or_else(||p.pointer("/tool_info/error")).cloned().unwrap_or(Value::Null),""));
            }
            if let Some(a)=p.pointer("/subagent_info/subagents").and_then(Value::as_array){for child in a {let id=s(child,"conversation_id");if !id.is_empty(){out.push(event(id,json!({"type":"subagent","childId":id,"title":s(child,"role"),"details":if s(child,"log_uri").is_empty(){"unavailable"}else{"history_partial"},"status":"unknown"})));}}}
        }
        _=>{}
    }
    let source = v
        .get("eventId")
        .or_else(|| v.pointer("/_meta/eventId"))
        .or_else(|| v.get("uuid"))
        .and_then(Value::as_str);
    if let Some(source) = source {
        for (index, event) in out.iter_mut().enumerate() {
            if let DriverEvent::Presentation { data, .. } = event {
                data["sourceEventId"] = json!(format!("{source}:{index}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn data(event: &DriverEvent) -> &Value {
        match event {
            DriverEvent::Presentation { data, .. } => data,
            _ => panic!("display event"),
        }
    }
    #[test]
    fn claude_child_tools_have_explicit_parent_and_no_signature() {
        let events = presentation_events(
            "claude",
            "",
            &json!({"type":"assistant","parent_tool_use_id":"task1","message":{"content":[{"type":"tool_use","id":"read1","name":"Read","input":{"file_path":"/fixture.txt","signature":"opaque"}}]}}),
        );
        assert_eq!(data(&events[0])["parentId"], "task1");
        assert_eq!(data(&events[0])["input"]["file_path"], "/fixture.txt");
        assert!(data(&events[0])["input"].get("signature").is_none());
    }
    #[test]
    fn zcode_failed_result_does_not_become_success_and_links_child_session() {
        let events = presentation_events(
            "zcode",
            "session/event",
            &json!({"type":"tool.updated","payload":{"kind":"result","toolCallId":"r","parentToolCallId":"p","childSessionId":"child","result":{"success":false,"content":"denied"}}}),
        );
        assert_eq!(data(&events[0])["status"], "failed");
        assert_eq!(data(&events[1])["childId"], "child");
    }
    #[test]
    fn acp_preserves_session_scopes_and_partial_updates() {
        let events = presentation_events(
            "acp",
            "session/update",
            &json!({"sessionId":"child","update":{"sessionUpdate":"tool_call_update","toolCallId":"r","rawOutput":{"result":"ok"}}}),
        );
        assert_eq!(data(&events[0])["sessionId"], "child");
        assert!(data(&events[0]).get("status").is_none());
    }
    #[test]
    fn codex_start_activity_completion_is_not_child_completion() {
        let events = presentation_events(
            "codex",
            "item/completed",
            &json!({"item":{"type":"subAgentActivity","id":"i","agentThreadId":"child","kind":"started","agentPath":"/root/reader"}}),
        );
        assert_eq!(data(&events[0])["status"], "started");
        assert_eq!(data(&events[0])["details"], "history");
    }
    #[test]
    fn grok_explicit_relationship_is_preserved() {
        let events = presentation_events(
            "grok",
            "_x.ai/session_notification",
            &json!({"update":{"sessionUpdate":"subagent_spawned","parent_session_id":"parent","child_session_id":"child","description":"Read"}}),
        );
        assert_eq!(data(&events[0])["parentSessionId"], "parent");
        assert_eq!(data(&events[0])["childSessionId"], "child");
    }
}
