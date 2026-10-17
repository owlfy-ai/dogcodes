//! Legacy safety-report data, retained for reading saved results.
use super::Side;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub source_id: String,
    pub side: Side,
    pub start_line: usize,
    pub end_line: usize,
    pub evidence: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub root_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceStep {
    pub role: String,
    pub explanation: String,
    #[serde(flatten)]
    pub citation: Citation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub attack_class: String,
    pub attacker: String,
    pub boundary: String,
    pub preconditions: String,
    pub controls: String,
    pub impact: String,
    pub trace: Vec<TraceStep>,
    pub blockers: Vec<String>,
    pub validation_plan: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    SourceChecked,
    NeedsValidation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeValidation {
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub evidence: Option<Evidence>,
    pub evidence_status: EvidenceStatus,
    pub runtime_validation: RuntimeValidation,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Reviewed,
    NotApplicable,
    NeedsContext,
    NotReported,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityCheck {
    pub attack_class: String,
    pub label: String,
    pub status: CheckStatus,
    pub reason: String,
    pub citations: Vec<Citation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityReview {
    pub boundary_analysis: String,
    pub control_analysis: String,
    pub impact_analysis: String,
    pub unresolved: Vec<String>,
}
