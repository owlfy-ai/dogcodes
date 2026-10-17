//! Approval previews are structured user-facing data, not short diagnostic messages.
use serde_json::{Value, json};

const TEXT_BUDGET: usize = 256 * 1024;
const NODE_BUDGET: usize = 4096;

pub(crate) fn summary(input: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        // Native drivers may provide plain command text. Never truncate a JSON serialization.
        let text_value = input.to_owned();
        if text_value.len() <= TEXT_BUDGET {
            return text_value;
        }
        return json!({"detailText": prefix(&text_value, TEXT_BUDGET), "detailsTruncated": true})
            .to_string();
    };
    if let Value::String(text) = value {
        let text_value = text;
        if text_value.len() <= TEXT_BUDGET {
            return serde_json::to_string(&text_value).expect("string JSON");
        }
        return json!({"detailText": prefix(&text_value, TEXT_BUDGET), "detailsTruncated": true})
            .to_string();
    }
    let mut budget = Budget {
        bytes: TEXT_BUDGET,
        nodes: NODE_BUDGET,
        truncated: false,
    };
    let value = limit_details(value, &mut budget);
    // Metadata lives outside native input and cannot be overwritten by it.
    json!({"input": value, "detailsTruncated": budget.truncated}).to_string()
}

struct Budget {
    bytes: usize,
    nodes: usize,
    truncated: bool,
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn bounded(text: String, budget: &mut Budget) -> String {
    let result = prefix(&text, budget.bytes.min(64 * 1024)).to_owned();
    budget.truncated |= result.len() != text.len();
    budget.bytes -= result.len();
    result
}
fn limit_details(value: Value, budget: &mut Budget) -> Value {
    if budget.nodes == 0 {
        budget.truncated = true;
        return Value::Null;
    }
    budget.nodes -= 1;
    match value {
        Value::String(text) => Value::String(bounded(text, budget)),
        Value::Array(values) => {
            let mut result = Vec::new();
            for value in values {
                if budget.nodes == 0 {
                    budget.truncated = true;
                    break;
                }
                result.push(limit_details(value, budget));
            }
            Value::Array(result)
        }
        Value::Object(values) => {
            let mut result = serde_json::Map::new();
            for (key, value) in values {
                if budget.nodes == 0 {
                    budget.truncated = true;
                    break;
                }
                let key = bounded(key, budget);
                result.insert(key, limit_details(value, budget));
            }
            Value::Object(result)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redaction::DeterministicRedactor;
    #[test]
    fn long_edits_preserve_paths_code_and_json_without_masking_values() {
        let code = format!(
            "{}\napi_key = \"sk-abcdefgh12345678\";",
            "const x = 1;\n".repeat(100)
        );
        let output = summary( &json!({"file_path":"src/service.ts", "old_string":code,"new_string":"const model = \"deepseek-flash\";", "headers":{"Authorization":"Bearer private-value"}}).to_string());
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["input"]["file_path"], "src/service.ts");
        assert!(parsed["input"]["old_string"].as_str().unwrap().len() > 512);
        assert_eq!(
            parsed["input"]["headers"]["Authorization"],
            "Bearer private-value"
        );
        assert_eq!(parsed["detailsTruncated"], false);
        assert!(output.contains("sk-abcdefgh12345678"));
        assert!(output.contains("private-value"));
    }
    #[test]
    fn oversized_unicode_remains_valid_json_with_explicit_incompleteness() {
        let output = summary(
            &json!({"file_path":"a.ts", "new_string":"改".repeat(TEXT_BUDGET)}).to_string(),
        );
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["input"]["file_path"], "a.ts");
        assert_eq!(parsed["detailsTruncated"], true);
    }
    #[test]
    fn approval_event_does_not_apply_the_diagnostic_512_character_limit() {
        use agent_domain::{ExecutorRef, TaskId, TurnId};
        use driver_api::DriverEvent;
        let input = json!({"file_path":"src/model.ts", "old_string":"let model = 1;\n".repeat(100), "new_string":"let model = 2;"});
        let executor: ExecutorRef =
            serde_json::from_value(json!({"providerId":"fake", "installationId":null})).unwrap();
        let (kind, payload) = crate::map_driver_event(
            &TaskId::new(),
            &TurnId::new(),
            &executor,
            DriverEvent::ApprovalRequested {
                native_request_id: "approval-1".into(),
                tool_name: "Edit".into(),
                command_summary: input.to_string(),
                options: vec![],
            },
            &DeterministicRedactor,
        );
        assert_eq!(kind, "approval.requested");
        let preview: Value =
            serde_json::from_str(payload["commandSummary"].as_str().unwrap()).unwrap();
        assert_eq!(preview["input"], input);
    }

    #[test]
    fn plain_commands_keep_their_existing_shape() {
        assert_eq!(summary("echo hello"), "echo hello");
        assert_eq!(summary("\"echo hello\""), "\"echo hello\"");
    }
}
