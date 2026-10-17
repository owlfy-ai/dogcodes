//! Legacy workflow records and frozen source reader; no new workflow is executed.
use super::{ReviewSource, add_source, sources};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewGroup {
    pub id: String,
    pub file_ids: Vec<String>,
    pub status: String,
    pub sessions: Vec<String>,
    pub context_requests: usize,
    #[serde(default)]
    pub context_log: Vec<Value>,
    pub initial_output: String,
    pub verification_output: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewWorkflow {
    pub groups: Vec<ReviewGroup>,
    pub context_scope: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub verdict: String,
    pub reason: String,
    #[serde(default)]
    pub citations: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_review: Option<super::security::SecurityReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_evidence: Option<super::security::Assessment>,
}

pub(crate) fn corpus(packet: &Value) -> Vec<ReviewSource> {
    let mut all = sources(packet);
    let mut add = |files: &Value, root_id: &str, label: &str| {
        for file in files.as_array().into_iter().flatten() {
            if file["path"].as_str().is_none() {
                continue;
            }
            add_source(&mut all, file, root_id, label);
            if let Some(last) = all.last_mut() {
                last.file_id = format!("context-{}", last.file_id);
            }
        }
    };
    if let Some(roots) = packet["roots"].as_array() {
        for (i, root) in roots.iter().enumerate() {
            add(
                &root["contextFiles"],
                root["rootId"]
                    .as_str()
                    .unwrap_or(&format!("root-{}", i + 1)),
                root["path"].as_str().unwrap_or(""),
            );
        }
    } else {
        add(&packet["changes"]["contextFiles"], "turn-root", "");
        for root in packet["changes"]["roots"].as_array().into_iter().flatten() {
            add(
                &root["changes"]["contextFiles"],
                root["workspacePath"].as_str().unwrap_or("turn-root"),
                root["workspacePath"].as_str().unwrap_or(""),
            );
        }
    }
    all
}

pub(crate) fn parse(text: &str) -> Option<Value> {
    parse_with_prose(text).map(|(value, _)| value)
}

/// Accept one complete JSON object or one complete JSON fence with prose.
/// Never choose between multiple candidates or recover a truncated block.
pub(crate) fn parse_with_prose(text: &str) -> Option<(Value, bool)> {
    let text = text.trim();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return value.is_object().then_some((value, false));
    }
    let lines: Vec<_> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| matches!(line.trim(), "```json" | "```"))?;
    let end = start
        + 1
        + lines[start + 1..]
            .iter()
            .position(|line| line.trim() == "```")?;
    let outside = lines[..start].iter().chain(lines[end + 1..].iter());
    let mut prose = false;
    for line in outside {
        if line.contains("```") || line.contains(['{', '}']) {
            return None;
        }
        prose |= !line.trim().is_empty();
    }
    let value: Value = serde_json::from_str(&lines[start + 1..end].join("\n")).ok()?;
    value.is_object().then_some((value, prose))
}

/// Optional context must never crowd the actual changed code out of a packet.
pub(crate) fn discard_context(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.remove("contextFiles").is_some() {
                map.insert(
                    "contextScope".into(),
                    json!("上下文因总材料预算省略；仅可检索已保存变更"),
                );
            }
            for value in map.values_mut() {
                discard_context(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                discard_context(value);
            }
        }
        _ => {}
    }
}
