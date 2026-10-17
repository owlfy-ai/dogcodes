//! Synthetic stdio peer used only by the product-driver integration test.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut lines = stdin.lock().lines();
    let mut analysis = false;
    let mut full_review = false;
    while let Some(line) = lines.next() {
        let Ok(line) = line else { return };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = request.get("id").cloned();
        match method {
            "initialize" => respond(&mut stdout, id, json!({ "userAgent": "codex-fixture/1" })),
            "initialized" => {}
            "config/read" => respond(
                &mut stdout,
                id,
                json!({"config":{"mcp_servers":{"inherited":{"enabled":true}}}}),
            ),
            "account/read" => respond(
                &mut stdout,
                id,
                json!({ "account": { "type": "apiKey" }, "requiresOpenaiAuth": true }),
            ),
            "thread/start" | "thread/resume" => {
                if method == "thread/start" {
                    analysis = request.pointer("/params/ephemeral") == Some(&json!(true));
                    assert_eq!(
                        request.pointer("/params/approvalPolicy"),
                        Some(&json!(if analysis { "never" } else { "untrusted" }))
                    );
                    if analysis {
                        full_review = request["params"]["sandbox"] == "danger-full-access";
                        assert_eq!(
                            request["params"]["sandbox"],
                            if full_review {
                                "danger-full-access"
                            } else {
                                "read-only"
                            }
                        );
                        for key in [
                            "mcp_servers.inherited.enabled",
                            "features.multi_agent",
                            "apps._default.enabled",
                        ] {
                            assert_eq!(request["params"]["config"][key], false, "{key}");
                        }
                        assert_eq!(request["params"]["config"]["features.shell_tool"], true);
                        assert_eq!(request["params"]["config"]["web_search"], "disabled");
                    }
                }
                respond(
                    &mut stdout,
                    id,
                    json!({ "thread": { "id": "fixture-thread", "status": "idle" } }),
                );
            }
            "model/list" => respond(
                &mut stdout,
                id,
                json!({"data":[{"id":"fixture-default","model":"fixture-default","displayName":"Default","isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium"}]}],"nextCursor":null}),
            ),
            "turn/start" => {
                if request
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    == Some("CHAT_RESTRICTIONS")
                {
                    assert!(analysis && !full_review);
                    assert_eq!(request["params"]["sandboxPolicy"]["type"], "readOnly");
                    assert_eq!(request["params"]["approvalPolicy"], "never");
                    respond(
                        &mut stdout,
                        id,
                        json!({"turn":{"id":"chat-turn","status":"inProgress"}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"item/agentMessage/delta","params":{"threadId":"fixture-thread","turnId":"chat-turn","itemId":"chat","delta":"chat restrictions verified"}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"chat-turn","status":"completed"}}}),
                    );
                    continue;
                }
                if analysis {
                    assert_eq!(
                        request["params"]["sandboxPolicy"]["type"],
                        if full_review {
                            "dangerFullAccess"
                        } else {
                            "readOnly"
                        }
                    );
                    assert_eq!(request["params"]["approvalPolicy"], "never");
                    assert!(
                        request["params"]["input"][0]["text"]
                            .as_str()
                            .unwrap()
                            .len()
                            > 65_536
                    );
                    respond(
                        &mut stdout,
                        id,
                        json!({"turn":{"id":"analysis-turn","status":"inProgress"}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"item/agentMessage/delta","params":{"threadId":"fixture-thread","turnId":"analysis-turn","itemId":"analysis","delta":"historical analysis"}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"analysis-turn","status":"completed"}}}),
                    );
                    continue;
                }
                if request
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    == Some("settings-report")
                {
                    respond(
                        &mut stdout,
                        id,
                        json!({"turn":{"id":"settings-turn","status":"inProgress"}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"item/agentMessage/delta","params":{"threadId":"fixture-thread","turnId":"settings-turn","itemId":"report","delta":json!({"pid":std::process::id(),"model":request["params"]["model"],"effort":request["params"]["effort"]}).to_string()}}),
                    );
                    emit(
                        &mut stdout,
                        json!({"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"settings-turn","status":"completed"}}}),
                    );
                    continue;
                }

                let waits_for_interrupt = request
                    .pointer("/params/input")
                    .and_then(Value::as_array)
                    .and_then(|input| input.last())
                    .and_then(|input| input.get("text"))
                    .and_then(Value::as_str)
                    == Some("wait for interrupt");
                if waits_for_interrupt {
                    respond(
                        &mut stdout,
                        id,
                        json!({ "turn": { "id": "fixture-interrupt-turn", "status": "inProgress" } }),
                    );
                    continue;
                }
                let ordered = request
                    .pointer("/params/input")
                    .and_then(Value::as_array)
                    .is_some_and(|input| {
                        input.len() == 2
                            && input[0]
                                .get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| text.contains("sha256=fixture-hash"))
                            && input[1].get("text").and_then(Value::as_str) == Some("do work")
                    });
                if ordered {
                    let text = request["params"]["input"][0]["text"].as_str().unwrap();
                    assert!(text.len() > 65_536);
                    // Codex echoes the whole input before acknowledging turn/start.
                    emit(
                        &mut stdout,
                        json!({"method":"item/started","params":{
                            "threadId":"fixture-thread", "turnId":"fixture-turn",
                            "item":{"type":"userMessage","id":"handoff-input","content":request["params"]["input"]}
                        }}),
                    );
                    assert_eq!(
                        request.pointer("/params/sandboxPolicy/type"),
                        Some(&json!("workspaceWrite"))
                    );
                    assert_eq!(
                        request.pointer("/params/sandboxPolicy/networkAccess"),
                        Some(&json!(false))
                    );
                    let roots = request
                        .pointer("/params/sandboxPolicy/writableRoots")
                        .and_then(Value::as_array)
                        .expect("multi-root policy");
                    assert_eq!(roots.len(), 2);
                    assert_eq!(Some(&roots[0]), request.pointer("/params/cwd"));
                    assert!(std::path::Path::new(roots[1].as_str().unwrap()).is_dir());
                }
                respond(
                    &mut stdout,
                    id,
                    json!({ "turn": { "id": "fixture-turn", "status": "inProgress" } }),
                );
                emit(
                    &mut stdout,
                    json!({
                        "id": "approval-1",
                        "method": "item/commandExecution/requestApproval",
                        "params": { "command": "must not escape" }
                    }),
                );
                let approval = lines.next().and_then(Result::ok).unwrap_or_default();
                let declined = serde_json::from_str::<Value>(&approval)
                    .ok()
                    .and_then(|value| value.pointer("/result/decision").cloned())
                    == Some(json!("decline"));
                emit(
                    &mut stdout,
                    json!({
                        "method": "item/started",
                        "params": {
                            "item": { "id": "search-1", "type": "webSearch", "query": "fixture query" },
                            "startedAtMs": 1,
                            "threadId": "fixture-thread",
                            "turnId": "fixture-turn"
                        }
                    }),
                );
                emit(
                    &mut stdout,
                    json!({
                        "method": "item/completed",
                        "params": {
                            "completedAtMs": 2,
                            "item": { "id": "search-1", "type": "webSearch", "query": "fixture query" },
                            "threadId": "fixture-thread",
                            "turnId": "fixture-turn"
                        }
                    }),
                );
                emit(
                    &mut stdout,
                    json!({
                        "method": "item/agentMessage/delta",
                        "params": { "itemId": "message-1", "delta": if ordered && declined { "fixture reply" } else { "fixture failure" } }
                    }),
                );
                emit(
                    &mut stdout,
                    json!({ "method": "turn/completed", "params": { "turn": { "status": "completed" } } }),
                );
            }
            "turn/interrupt" => {
                emit(
                    &mut stdout,
                    json!({ "method": "turn/completed", "params": { "turn": { "status": "interrupted" } } }),
                );
                respond(&mut stdout, id, json!({}));
            }
            _ => {
                if let Some(id) = id {
                    emit(
                        &mut stdout,
                        json!({ "id": id, "error": { "code": -32601, "message": "unsupported" } }),
                    );
                }
            }
        }
    }
}

fn respond(stdout: &mut impl Write, id: Option<Value>, result: Value) {
    if let Some(id) = id {
        emit(stdout, json!({ "id": id, "result": result }));
    }
}

fn emit(stdout: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *stdout, &value).unwrap();
    stdout.write_all(b"\n").unwrap();
    stdout.flush().unwrap();
}
