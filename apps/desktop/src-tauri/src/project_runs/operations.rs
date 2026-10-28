//! Operation receipts live in the same atomic file as the catalog they change.
use super::{RunCatalog, RunEdit};
use agent_domain::ExecutorRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnalyzeRequest {
    pub operation_id: String,
    pub task_id: String,
    pub expected_generation: String,
    pub executor: Option<ExecutorRef>,
    pub overwrite_generation: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    pub operation_id: String,
    pub task_id: String,
    pub generation: String,
    pub directory_index: usize,
    pub command_id: String,
    pub edit: RunEdit,
}
/// Only explicit pre-commit validation failures are rejected. Storage and
/// transport failures remain unknown, including a write whose reply was lost.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MutationError {
    Rejected { code: String, message: String },
    Unknown { message: String },
}
impl MutationError {
    pub fn rejected(code: &str, message: &str) -> Self {
        Self::Rejected {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl From<String> for MutationError {
    fn from(message: String) -> Self {
        Self::Unknown { message }
    }
}
impl From<&str> for MutationError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { message, .. } | Self::Unknown { message } => f.write_str(message),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOperation {
    pub operation_id: String,
    pub generation: String,
    pub status: String,
    pub error: Option<String>,
    /// Included on reads so remote clients verify the frozen original intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Receipt {
    pub request: serde_json::Value,
    pub result: RunOperation,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct StoredCatalog {
    #[serde(flatten)]
    pub catalog: RunCatalog,
    #[serde(default)]
    pub operations: BTreeMap<String, Receipt>,
}
impl From<RunCatalog> for StoredCatalog {
    fn from(catalog: RunCatalog) -> Self {
        Self {
            catalog,
            operations: BTreeMap::new(),
        }
    }
}
impl std::ops::Deref for StoredCatalog {
    type Target = RunCatalog;
    fn deref(&self) -> &RunCatalog {
        &self.catalog
    }
}
impl std::ops::DerefMut for StoredCatalog {
    fn deref_mut(&mut self) -> &mut RunCatalog {
        &mut self.catalog
    }
}
impl StoredCatalog {
    pub fn replay(&self, id: &str, request: &serde_json::Value) -> Result<bool, MutationError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| MutationError::rejected("invalid_request", "无效的运行操作标识"))?;
        if let Some(receipt) = self.operations.get(id) {
            if &receipt.request != request {
                return Err(MutationError::rejected(
                    "operation_conflict",
                    "运行操作标识已用于不同请求，请核实原操作",
                ));
            }
            return Ok(true);
        }
        Ok(false)
    }
    pub fn record(&mut self, id: &str, request: serde_json::Value, status: &str) {
        self.operations.insert(
            id.into(),
            Receipt {
                request,
                result: RunOperation {
                    operation_id: id.into(),
                    generation: self.generation.clone(),
                    status: status.into(),
                    error: None,
                    request: None,
                },
            },
        );
    }
    pub fn settle_analysis(&mut self) {
        let status = if self.directories.iter().any(|d| d.status == "analyzing") {
            return;
        } else if self.directories.iter().any(|d| d.status == "error") {
            "failed"
        } else {
            "succeeded"
        };
        let error = self.directories.iter().find_map(|d| d.error.clone());
        let generation = self.generation.clone();
        for receipt in self
            .operations
            .values_mut()
            .filter(|r| r.result.generation == generation && r.result.status == "running")
        {
            receipt.result.status = status.into();
            receipt.result.error.clone_from(&error);
        }
    }
    pub fn interrupt_analysis(&mut self, status: &str, message: &str) -> bool {
        let mut changed = false;
        for directory in &mut self.directories {
            if directory.status == "analyzing" {
                directory.status = "error".into();
                directory.error = Some(message.into());
                changed = true;
            }
        }
        let generation = self.generation.clone();
        for receipt in self
            .operations
            .values_mut()
            .filter(|r| r.result.generation == generation && r.result.status == "running")
        {
            receipt.result.status = status.into();
            receipt.result.error = Some(message.into());
            changed = true;
        }
        changed
    }
}
