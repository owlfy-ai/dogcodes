//! Persist exact file-mutation requests and their original (possibly partial) result.
use super::operation_journal::{
    JournalOutcome, JournalRequest, OperationJournal, OperationReceipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MutationRequest {
    pub operation_id: String,
    pub task_id: String,
    pub command: String,
    pub arguments: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum MutationOutcome {
    Running,
    Succeeded { result: Value },
    Failed { message: String },
    Unknown,
}
pub(crate) type MutationReceipt = OperationReceipt<MutationRequest, MutationOutcome>;
pub(crate) type MutationOperations = OperationJournal<MutationRequest, MutationOutcome>;
impl JournalRequest for MutationRequest {
    fn task_id(&self) -> &str {
        &self.task_id
    }
    fn operation_id(&self) -> &str {
        &self.operation_id
    }
}
impl JournalOutcome for MutationOutcome {
    type Output = Value;
    fn running() -> Self {
        Self::Running
    }
    fn unknown() -> Self {
        Self::Unknown
    }
    fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
    fn from_result(result: &Result<Value, String>) -> Self {
        match result {
            Ok(result) => Self::Succeeded {
                result: result.clone(),
            },
            Err(message) => Self::Failed {
                message: message.clone(),
            },
        }
    }
    fn result(self) -> Result<Value, String> {
        match self {
            Self::Succeeded { result } => Ok(result),
            Self::Failed { message } => Err(message),
            Self::Running => Err("文件操作仍在执行，请核实原操作".into()),
            Self::Unknown => {
                Err("文件操作曾中断，结果未知；请核实文件及恢复记录，未自动重发".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn file_operation_partial_result_survives_restart_without_reapplying_completed_files() {
        let directory = tempfile::tempdir().unwrap();
        let request = MutationRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            task_id: agent_domain::TaskId::new().to_string(),
            command: "core_review_batch_files".into(),
            arguments: serde_json::json!({"paths":["a","b"]}),
        };
        let partial = serde_json::json!({"completed":["a"],"error":"b changed after preview"});
        let service = MutationOperations::new(directory.path().into());
        assert_eq!(
            service
                .execute(request.clone(), || async { Ok(partial.clone()) })
                .await
                .unwrap(),
            partial
        );
        let reopened = MutationOperations::new(directory.path().into());
        assert_eq!(
            reopened
                .execute(request.clone(), || async {
                    panic!("do not reapply a partial batch")
                })
                .await
                .unwrap(),
            partial
        );
        let mut changed = request.clone();
        changed.arguments = serde_json::json!({"paths":["b"]});
        assert!(
            reopened
                .execute(changed, || async {
                    panic!("cannot reuse ID for remaining files")
                })
                .await
                .is_err()
        );
        reopened.forget_task(&request.task_id).unwrap();
        assert!(
            reopened
                .get(&request.task_id, &request.operation_id)
                .unwrap()
                .is_none()
        );
    }
}
