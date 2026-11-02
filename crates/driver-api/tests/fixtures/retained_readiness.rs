//! Private stdio fixture compiled by driver tests. No installed Agent/account.
use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(&format!("\"{name}\":"))? + name.len() + 3;
    let value = line[start..].trim_start();
    if let Some(value) = value.strip_prefix('"') {
        Some(&value[..value.find('"')?])
    } else {
        Some(value.split([',', '}']).next()?.trim())
    }
}
fn append(path: &std::path::Path, line: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}
fn emit(line: &str) {
    println!("{line}");
    io::stdout().flush().unwrap();
}
fn main() {
    let root: PathBuf = std::env::current_exe().unwrap().parent().unwrap().into();
    let mode = fs::read_to_string(root.join("mode")).unwrap();
    append(&root.join("starts"), &std::process::id().to_string());
    let mut model_reads = 0;
    let mut sessions = 0;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        append(&root.join("requests"), &line);
        let method = field(&line, "method")
            .or_else(|| field(&line, "type"))
            .unwrap_or("");
        let request_id = field(&line, "request_id").unwrap_or("");
        let catalog = method == "model/list"
            || request_id == "model-catalog"
            || (method == "get_available_models" && model_reads > 0);
        if method == "get_available_models" {
            model_reads += 1;
        }
        if (mode == "blocked-catalog" && catalog)
            || (mode == "blocked-init"
                && (method == "initialize" || request_id == "dogcodes-probe"))
        {
            while !root.join("release").exists() {
                thread::sleep(Duration::from_millis(10));
            }
        }
        if method == "control_request" {
            let id = if mode == "wrong-id" {
                "unrelated"
            } else {
                request_id
            };
            emit(&format!(
                r#"{{"type":"control_response","response":{{"request_id":"{id}","subtype":"success","response":{{"models":[{{"value":"fixture","displayName":"Fixture","supportsAutoMode":true}}]}}}}}}"#
            ));
        } else if let Some(id) = field(&line, "id") {
            if line.contains("\"method\"") {
                let id = if line.contains(&format!("\"id\":\"{id}\"")) {
                    format!("\"{id}\"")
                } else {
                    id.to_owned()
                };
                if method == "session/new" {
                    sessions += 1;
                    emit(&format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"sessionId":"session-{sessions}","models":{{"currentModelId":"fixture","availableModels":[{{"modelId":"fixture","name":"Fixture"}}]}},"modes":{{"currentModeId":"default","availableModes":[{{"id":"default","name":"Ask"}}]}}}}}}"#
                    ));
                    continue;
                }
                let data = match method {
                    "initialize" => {
                        r#"{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}"#
                    }
                    "account/read" if mode == "unauthenticated" => r#"{"requiresOpenaiAuth":true}"#,
                    "account/read" => r#"{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}"#,
                    "model/list" => {
                        r#"{"data":[{"id":"fixture","model":"fixture","displayName":"Fixture","isDefault":true}],"nextCursor":null}"#
                    }
                    _ => "{}",
                };
                emit(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{data}}}"#));
            } else if id != "null" && !method.is_empty() {
                let data = match method {
                    "get_state" => {
                        r#"{"isStreaming":false,"sessionFile":null,"model":{"provider":"fixture","id":"base"},"thinkingLevel":"medium"}"#
                    }
                    "get_available_models" if mode == "empty" => r#"{"models":[]}"#,
                    "get_available_models" => {
                        r#"{"models":[{"provider":"fixture","id":"base","name":"Base"}]}"#
                    }
                    "get_available_thinking_levels" => r#"{"levels":["medium","high"]}"#,
                    _ => "{}",
                };
                emit(&format!(
                    r#"{{"type":"response","id":"{id}","success":true,"data":{data}}}"#
                ));
            }
        }
    }
}
