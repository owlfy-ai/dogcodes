//! One supervised, persistent stdio connection. Unknown actions are never replayed.
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
    pub async fn spawn(program: &Path, home: &Path) -> Result<Self, String> {
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
        self.request(method, json!({"name":name,"arguments":args}))
            .await
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
                return Err("unsupported_computer_batch_step".into());
            }
            if tool.is_none() && step.get("wait_for").is_none() && step.get("expect").is_none() {
                return Err("unsupported_computer_batch_step".into());
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
