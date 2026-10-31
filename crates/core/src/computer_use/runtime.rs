//! One supervised, persistent stdio connection. Unknown actions are never replayed.
use base64::Engine as _;
use process_scope::OwnedChild;
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub(super) const MAX_FRAME: usize = 24 * 1024 * 1024;
pub(super) const TOOLS: &[&str] = &[
    "list_apps",
    "list_windows",
    "launch_app",
    "get_window_state",
    "get_desktop_state",
    "zoom",
    "click",
    "double_click",
    "right_click",
    "type_text",
    "set_value",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
    "move_cursor",
    "invoke_menu",
    "set_window_frame",
    "verify_state",
    "run_steps",
];
pub(super) const INSTRUCTIONS: &str = include_str!("workflow.md");

pub(super) struct Driver {
    child: OwnedChild,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
    catalog: Value,
}

impl Driver {
    pub async fn spawn(
        program: &Path,
        home: &Path,
        bundle_id: Option<&str>,
    ) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(["mcp", "--direct"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("DO_NOT_TRACK", "1")
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .env("CUA_DRIVER_RS_HOME", home)
            // The task switch authorizes ordinary host-desktop work. Do not inherit
            // a user's unrelated Cua manifest, recording or browser configuration.
            .env("CUA_DRIVER_PERMISSION_MODE", "standard")
            .env_remove("CUA_DRIVER_ALLOW_BRIEF_FOREGROUND")
            .env_remove("CUA_DRIVER_CAPABILITY_MANIFEST_FILE")
            .env_remove("CUA_DRIVER_SESSION_POLICY_FILE")
            .env_remove("CUA_DRIVER_POLICY_FILE")
            .env_remove("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS");
        // Direct MCP stays in the spawning application's responsibility chain.
        // Only the macOS desktop host supplies an advisory bundle identity.
        command
            .env_remove("CUA_DRIVER_EMBEDDED")
            .env_remove("CUA_DRIVER_HOST_BUNDLE_ID");
        if cfg!(target_os = "macos")
            && let Some(bundle_id) = bundle_id
        {
            command
                .env("CUA_DRIVER_EMBEDDED", "1")
                .env("CUA_DRIVER_HOST_BUNDLE_ID", bundle_id);
        }
        let mut child =
            OwnedChild::spawn(command).map_err(|e| format!("computer_driver_start: {e}"))?;
        let input = child.stdin.take().ok_or("computer_driver_stdin")?;
        let output = BufReader::new(child.stdout.take().ok_or("computer_driver_stdout")?);
        let mut driver = Self {
            child,
            input,
            output,
            next_id: 1,
            catalog: Value::Null,
        };
        let initialized = async {
            driver.request("initialize", json!({"protocolVersion":"2025-03-26", "capabilities":{},"clientInfo":{"name":"dogcodes","version":"1"}})).await?;
            driver.write(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
            let mut catalog = driver.request("tools/list", json!({})).await?;
            let tools = catalog["tools"].as_array_mut().ok_or("computer_driver_invalid_catalog")?;
            tools.retain(|tool| tool["name"].as_str().is_some_and(|name| TOOLS.contains(&name)));
            for tool in tools.iter_mut() { compact_schema(tool); }
            driver.catalog = catalog;
            Ok::<_,String>(())
        }.await;
        if let Err(error) = initialized {
            let _ = driver.close().await;
            return Err(error);
        }
        Ok(driver)
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.child
            .terminate()
            .await
            .map_err(|e| format!("computer_driver_cleanup_pending: {e}"))
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if method == "tools/list" {
            return Ok(self.catalog.clone());
        }
        if method != "tools/call" {
            return Err("unsupported_computer_method".into());
        }
        let name = params["name"]
            .as_str()
            .ok_or("computer_tool_name_required")?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        validate_call(name, &args)?;
        let mut result = self
            .request(method, json!({"name":name,"arguments":args}))
            .await?;
        describe_images(&mut result);
        Ok(result)
    }

    async fn write(&mut self, value: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| "computer_encode_failed")?;
        if bytes.len() > MAX_FRAME {
            return Err("computer_request_too_large".into());
        }
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| "computer_driver_disconnected".into())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(Duration::from_mins(2), async {
            loop {
                let mut line = String::new();
                let length = (&mut self.output).take(MAX_FRAME as u64 + 1).read_line(&mut line).await
                    .map_err(|_| "computer_driver_read_failed")?;
                if length == 0 { return Err("computer_driver_disconnected; action outcome may be unknown; observe before retrying".into()); }
                if length > MAX_FRAME { return Err("computer_driver_frame_too_large".into()); }
                let reply: Value = serde_json::from_str(&line).map_err(|_| "computer_driver_invalid_json")?;
                if reply["id"] == id {
                    if let Some(error) = reply.get("error") { return Err(format!("computer_driver_error: {error}")); }
                    return reply.get("result").cloned().ok_or_else(|| "computer_driver_missing_result".into());
                }
                // Drivers may issue notifications. Never auto-answer an interactive
                // server request as if the operator had supplied an answer.
                if let Some(request_id) = reply.get("id") {
                    self.write(&json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Host does not support interactive driver requests"}})).await?;
                }
            }
        }).await.map_err(|_| "computer_driver_timeout; action outcome unknown; observe before retrying".to_owned())?
    }
}

fn describe_images(result: &mut Value) {
    // Some clients omit structuredContent when constructing model messages.
    let grounding: serde_json::Map<String, Value> = [
        "capture_id",
        "snapshot_id",
        "pid",
        "window_id",
        "window_bounds",
        "screenshot_width",
        "screenshot_height",
        "screenshot_mime_type",
    ]
    .into_iter()
    .filter_map(|key| {
        result["structuredContent"]
            .get(key)
            .map(|value| (key.to_owned(), value.clone()))
    })
    .collect();
    let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let mut descriptions = vec![];
    if !grounding.is_empty() {
        descriptions.push(json!({"type":"text","text":format!("Computer-use capture metadata (use for coordinate grounding): {}", Value::Object(grounding))}));
    }
    descriptions.extend(content.iter().filter(|item| item["type"] == "image")
        .enumerate().filter_map(|(index,item)| {
            if item["mimeType"] != "image/png" { return None; }
            // PNG's signature and IHDR dimensions fit in the first 32 base64
            // characters. Do not decode/copy the entire screenshot just for size.
            let prefix = item["data"].as_str()?.get(..32)?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(prefix).ok()?;
            if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR" { return None; }
            let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
            let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
            if width == 0 || height == 0 { return None; }
            Some(json!({"type":"text","text":format!("Returned image {}: {width} x {height} pixels. Use the original capture/coordinate metadata for input.",index+1)}))
        }).collect::<Vec<_>>());
    // Some MCP clients preserve images while dropping tool-result metadata.
    // A short text caption keeps dimensions available to their image viewers.
    content.splice(0..0, descriptions);
}

pub(super) fn validate_call(name: &str, args: &Value) -> Result<(), String> {
    if !TOOLS.contains(&name) || !args.is_object() {
        return Err("unknown_computer_tool_or_arguments".into());
    }
    if name == "run_steps" {
        let steps = args["steps"].as_array().ok_or("computer_steps_required")?;
        if steps.is_empty() || steps.len() > 32 {
            return Err("computer_batch_requires_1_to_32_steps".into());
        }
        let allowed = [
            "click",
            "double_click",
            "right_click",
            "set_value",
            "type_text",
            "press_key",
            "hotkey",
            "scroll",
            "drag",
            "move_cursor",
        ];
        for step in steps {
            let tool = step["tool"].as_str().or_else(|| {
                step.as_object()?
                    .keys()
                    .find(|key| allowed.contains(&key.as_str()))
                    .map(String::as_str)
            });
            if tool.is_some_and(|tool| !allowed.contains(&tool)) {
                return Err("unsupported_computer_batch_step; steps must be {tool,args} or {click:{...}}, using the published action enum; use observe:{} for the final read, or call an individual tool".into());
            }
            if tool.is_none() && step.get("wait_for").is_none() && step.get("expect").is_none() {
                return Err("unsupported_computer_batch_step; steps must be {tool,args} or {click:{...}}, or a wait_for/expect check; use observe:{} for the final read".into());
            }
        }
    }
    Ok(())
}

fn compact_schema(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("_meta");
            if let Some(description) = object
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                object.insert(
                    "description".into(),
                    Value::String(description.chars().take(240).collect()),
                );
            }
            for nested in object.values_mut() {
                compact_schema(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                compact_schema(nested);
            }
        }
        _ => (),
    }
    if value["name"] == "run_steps" {
        value["description"] = json!(
            "Run 1–32 known actions, stopping on failure. Steps: {tool,args} or {click:{...}}; only the listed action tools. Use observe:{} for one final read, include_screenshot:true there for an image. Split at decisions; inspect partial results before retrying."
        );
        value["inputSchema"]["properties"]["steps"]["maxItems"] = json!(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_dimensions_are_portable_without_changing_pixels_or_capture_metadata() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend(1280_u32.to_be_bytes());
        header.extend(720_u32.to_be_bytes());
        let image = json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(header)});
        let mut result = json!({"content":[{"type":"text","text":"tree"},image],"_meta":{"scale":2},"structuredContent":{"capture_id":"original","snapshot_id":"s1","window_bounds":{"x":10,"y":20,"width":1600,"height":900},"screenshot_width":1280,"screenshot_height":720,"tree_markdown":"large tree"}});
        let original = result.clone();
        describe_images(&mut result);
        assert_eq!(result["content"][2], original["content"][0]);
        assert_eq!(result["content"][3], original["content"][1]);
        assert_eq!(result["_meta"], original["_meta"]);
        assert_eq!(result["structuredContent"], original["structuredContent"]);
        let grounding = result["content"][0]["text"].as_str().unwrap();
        assert!(grounding.contains("original") && grounding.contains("window_bounds"));
        assert!(!grounding.contains("large tree"));
        assert!(
            result["content"][1]["text"]
                .as_str()
                .unwrap()
                .contains("1280 x 720")
        );
        let mut malformed =
            json!({"content":[{"type":"image","mimeType":"image/png","data":"invalid"}]});
        let original = malformed.clone();
        describe_images(&mut malformed);
        assert_eq!(malformed, original);
        let mut malformed_result = json!("invalid tool result");
        describe_images(&mut malformed_result);
        assert_eq!(malformed_result, json!("invalid tool result"));
    }
    #[test]
    fn batches_are_validated_before_any_native_action() {
        assert!(
            validate_call(
                "run_steps",
                &json!({"steps":[{"tool":"click","args":{}},{"tool":"run_script","args":{}}]})
            )
            .is_err()
        );
        assert!(
            validate_call(
                "run_steps",
                &json!({"steps":[{"run_script":{"script":"x"}}]})
            )
            .is_err()
        );
        assert!(validate_call("run_steps", &json!({"steps":[]})).is_err());
        assert!(
            validate_call("run_steps", &json!({"steps":vec![json!({"click":{}});33]})).is_err()
        );
        assert!(validate_call("run_steps", &json!({"steps":[{"click":{"x":12,"y":34}},{"wait_for":{"text":"Ready"}}],"observe":{}})).is_ok());
    }
    #[test]
    fn compacting_preserves_grounding_and_postcondition_schema() {
        let mut schema = json!({"description":"x".repeat(1000),"inputSchema":{"properties":{"capture_id":{"type":"string"},"expect":{"oneOf":[{"type":"object"},{"type":"array"}]},"target":{"required":["kind"]}}}});
        let original = schema["inputSchema"].clone();
        compact_schema(&mut schema);
        assert_eq!(schema["inputSchema"], original);
        assert_eq!(schema["description"].as_str().unwrap().len(), 240);
    }
    #[test]
    fn batch_catalog_advertises_the_limit_enforced_by_the_proxy() {
        let mut schema =
            json!({"name":"run_steps","inputSchema":{"properties":{"steps":{"maxItems":64}}}});
        compact_schema(&mut schema);
        assert_eq!(schema["inputSchema"]["properties"]["steps"]["maxItems"], 32);
        assert!(
            schema["description"]
                .as_str()
                .unwrap()
                .contains("{tool,args}")
        );
    }
}
