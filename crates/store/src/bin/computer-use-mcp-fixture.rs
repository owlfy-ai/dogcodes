//! Hermetic MCP fixture: no desktop capture or native input.
use serde_json::{Value, json};
use std::io::{BufRead, Write};
fn main() {
    let home =
        std::path::PathBuf::from(std::env::var_os("CUA_DRIVER_RS_HOME").expect("fixture home"));
    std::fs::create_dir_all(&home).unwrap();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let request: Value = serde_json::from_str(&line).unwrap();
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":(["get_window_state","click","run_script"].iter().map(|name|json!({"name":name,"description":"fixture","inputSchema":{"type":"object","properties":{}}})).collect::<Vec<_>>())})
            }
            "tools/call" => {
                let args = &request["params"]["arguments"];
                if request["params"]["name"] == "click" {
                    std::fs::write(home.join("in-flight"), "yes").unwrap();
                    if let Some(delay) = args["delay_ms"].as_u64() {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                    let path = home.join("effects");
                    let count = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.parse::<u64>().ok())
                        .unwrap_or(0);
                    std::fs::write(path, (count + 1).to_string()).unwrap();
                    json!({"content":[{"type":"text","text":"clicked"}],"structuredContent":{"effect":"confirmed"},"isError":false})
                } else {
                    json!({"content":[{"type":"image","mimeType":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a1XcAAAAASUVORK5CYII="},{"type":"text","text":"fixture state"}],"structuredContent":{"snapshot_id":"fixture:1"},"isError":false})
                }
            }
            _ => json!({}),
        };
        println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
        std::io::stdout().flush().unwrap();
    }
}
