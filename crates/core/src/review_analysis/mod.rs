//! Report types and readers retained for historical reports. New reviews use `turn_review::workspace`.
pub mod security;
pub mod workflow;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSource {
    pub file_id: String,
    pub root_id: String,
    #[serde(default)]
    pub root_label: String,
    pub path: String,
    // None means unavailable, whereas an absent side of an addition/deletion is "".
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Before,
    After,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub file_id: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub suggestion: String,
    pub side: Side,
    pub start_line: usize,
    pub end_line: usize,
    pub evidence: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedFinding {
    #[serde(flatten)]
    pub finding: Finding,
    pub path: Option<String>,
    pub root_id: Option<String>,
    pub root_label: Option<String>,
    pub location_verified: bool,
    pub location_note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<workflow::Verification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<security::Assessment>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCoverage {
    pub file_id: String,
    pub root_id: String,
    #[serde(default)]
    pub root_label: String,
    pub path: String,
    pub status: CoverageStatus,
    pub rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_checks: Vec<security::SecurityCheck>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Reported,
    Partial,
    NotReported,
    Unavailable,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReport {
    pub schema_version: u8,
    pub structured: bool,
    pub findings: Vec<CheckedFinding>,
    pub files: Vec<FileCoverage>,
    pub warnings: Vec<String>,
}

pub(crate) fn sources(packet: &Value) -> Vec<ReviewSource> {
    let mut sources = vec![];
    if let Some(roots) = packet["roots"].as_array() {
        for (index, root) in roots.iter().enumerate() {
            let root_id = root["rootId"]
                .as_str()
                .map_or_else(|| format!("root-{}", index + 1), str::to_owned);
            for file in root["files"].as_array().into_iter().flatten() {
                add_source(
                    &mut sources,
                    file,
                    &root_id,
                    root["path"].as_str().unwrap_or(""),
                );
            }
        }
    } else {
        for file in packet["changes"]["files"].as_array().into_iter().flatten() {
            add_source(
                &mut sources,
                file,
                file["workspacePath"].as_str().unwrap_or("turn-root"),
                file["workspacePath"].as_str().unwrap_or(""),
            );
        }
    }
    sources
}
fn add_source(sources: &mut Vec<ReviewSource>, file: &Value, root_id: &str, root_label: &str) {
    let Some(path) = file["path"].as_str() else {
        return;
    };
    let version = |side: &str| {
        if file.get("omitted").is_some() || file["binary"] == true {
            return None;
        }
        match file.get(side) {
            Some(Value::Null) => Some(String::new()),
            Some(Value::String(text)) => Some(text.clone()),
            Some(Value::Object(value)) => {
                value.get("text").and_then(Value::as_str).map(str::to_owned)
            }
            _ => None,
        }
    };
    sources.push(ReviewSource {
        file_id: format!("file-{}", sources.len() + 1),
        root_id: root_id.into(),
        root_label: root_label.into(),
        path: path.into(),
        before: version("before"),
        after: version("after"),
    });
}
