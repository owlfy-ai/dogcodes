//! Durable Git request identities. An interrupted intent is never executed again
//! automatically: Git and this journal cannot share an atomic transaction.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GitRequest {
    pub operation_id: String,
    pub task_id: String,
    pub root_id: String,
    pub action: String,
    pub message: String,
    pub stage_all: Option<bool>,
    pub snapshot_id: Option<String>,
    pub workflow: Option<super::git_workflow::GitWorkflow>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum GitOutcome {
    Running,
    Succeeded {
        completed: bool,
        #[serde(default)]
        target: Option<super::git_push_target::PushTarget>,
        #[serde(default, rename = "targetError")]
        target_error: Option<String>,
    },
    Failed {
        message: String,
    },
    Unknown,
}
#[derive(Clone, Debug)]
pub(crate) struct GitCompletion {
    pub completed: bool,
    pub target: Option<super::git_push_target::PushTarget>,
    pub target_error: Option<String>,
}
impl From<bool> for GitCompletion {
    fn from(completed: bool) -> Self {
        Self {
            completed,
            target: None,
            target_error: None,
        }
    }
}
pub(crate) type GitReceipt = super::operation_journal::OperationReceipt<GitRequest, GitOutcome>;
pub(crate) type GitOperations = super::operation_journal::OperationJournal<GitRequest, GitOutcome>;
impl super::operation_journal::JournalRequest for GitRequest {
    fn task_id(&self) -> &str {
        &self.task_id
    }
    fn operation_id(&self) -> &str {
        &self.operation_id
    }
}
impl super::operation_journal::JournalOutcome for GitOutcome {
    type Output = GitCompletion;
    fn running() -> Self {
        Self::Running
    }
    fn unknown() -> Self {
        Self::Unknown
    }
    fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
    fn from_result(result: &Result<GitCompletion, String>) -> Self {
        match result {
            Ok(completed) => Self::Succeeded {
                completed: completed.completed,
                target: completed.target.clone(),
                target_error: completed.target_error.clone(),
            },
            Err(message) if message.contains(super::UNKNOWN_GIT_EFFECT) => Self::Unknown,
            Err(message) => Self::Failed {
                message: message.clone(),
            },
        }
    }
    fn result(self) -> Result<GitCompletion, String> {
        outcome(self)
    }
}

fn outcome(value: GitOutcome) -> Result<GitCompletion, String> {
    match value {
        GitOutcome::Succeeded {
            completed,
            target,
            target_error,
        } => Ok(GitCompletion {
            completed,
            target,
            target_error,
        }),
        GitOutcome::Failed { message } => Err(message),
        GitOutcome::Running => Err("Git 操作仍在执行，请核实原操作结果".into()),
        GitOutcome::Unknown => {
            Err("Git 操作曾中断，结果未知；请检查仓库后再操作，未自动重发".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn request() -> GitRequest {
        GitRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            task_id: agent_domain::TaskId::new().to_string(),
            root_id: "repo".into(),
            action: "commit".into(),
            message: "change".into(),
            stage_all: Some(true),
            snapshot_id: Some("snapshot".into()),
            workflow: None,
        }
    }
    #[tokio::test]
    async fn git_operation_receipt_replays_exact_request_after_restart_without_new_effect() {
        let temp = tempfile::tempdir().unwrap();
        let host = GitOperations::new(temp.path().join("operations"));
        let req = request();
        let count = AtomicUsize::new(0);
        assert!(
            host.execute(req.clone(), || async {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(true.into())
            })
            .await
            .unwrap()
            .completed
        );
        drop(host);
        let reopened = GitOperations::new(temp.path().join("operations"));
        assert!(
            reopened
                .execute(req.clone(), || async {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(true.into())
                })
                .await
                .unwrap()
                .completed
        );
        let mut changed = req.clone();
        changed.message = "other".into();
        assert!(
            reopened
                .execute(changed, || async {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(true.into())
                })
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            reopened
                .get(&req.task_id, &req.operation_id)
                .unwrap()
                .unwrap()
                .outcome,
            GitOutcome::Succeeded {
                completed: true,
                target: None,
                target_error: None
            }
        );
        reopened.forget_task(&req.task_id).unwrap();
        assert!(
            reopened
                .get(&req.task_id, &req.operation_id)
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn git_operation_interruption_preserves_unknown_and_never_reexecutes() {
        let temp = tempfile::tempdir().unwrap();
        let host = Arc::new(GitOperations::new(temp.path().join("operations")));
        let req = request();
        let (started, received) = tokio::sync::oneshot::channel();
        let worker = {
            let host = host.clone();
            let req = req.clone();
            tokio::spawn(async move {
                host.execute(req, || async {
                    started.send(()).unwrap();
                    std::future::pending::<Result<GitCompletion, String>>().await
                })
                .await
            })
        };
        received.await.unwrap();
        assert_eq!(
            host.get(&req.task_id, &req.operation_id)
                .unwrap()
                .unwrap()
                .outcome,
            GitOutcome::Running
        );
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        let reopened = GitOperations::new(temp.path().join("operations"));
        assert_eq!(
            reopened
                .get(&req.task_id, &req.operation_id)
                .unwrap()
                .unwrap()
                .outcome,
            GitOutcome::Unknown
        );
        assert!(
            reopened
                .execute(req, || async {
                    panic!("interrupted Git must never be replayed")
                })
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn git_operation_failure_confirmation_and_storage_errors_do_not_repeat_effects() {
        let temp = tempfile::tempdir().unwrap();
        let host = GitOperations::new(temp.path().join("operations"));
        let req = request();
        assert!(
            !host
                .execute(req.clone(), || async { Ok(false.into()) })
                .await
                .unwrap()
                .completed
        );
        assert!(
            !host
                .execute(req, || async { panic!("confirmation receipt must replay") })
                .await
                .unwrap()
                .completed
        );
        let req = request();
        assert_eq!(
            host.execute(req.clone(), || async { Err("push rejected".into()) })
                .await
                .unwrap_err(),
            "push rejected"
        );
        assert_eq!(
            host.execute(req, || async {
                panic!("failed operation must not run again")
            })
            .await
            .unwrap_err(),
            "push rejected"
        );
        let req = request();
        let file = host.file(&req.task_id, &req.operation_id).unwrap();
        assert!(
            host.execute(req.clone(), || async {
                std::fs::remove_file(&file).unwrap();
                std::fs::create_dir(&file).unwrap();
                Ok(true.into())
            })
            .await
            .is_err()
        );
        assert!(
            host.execute(req, || async {
                panic!("unreadable receipt must not be treated as absent")
            })
            .await
            .is_err()
        );
        let blocked = temp.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        let host = GitOperations::new(blocked);
        assert!(
            host.execute(request(), || async {
                panic!("intent must persist before Git runs")
            })
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn git_operation_history_pages_bound_records_and_report_damage_without_reexecution() {
        let temp = tempfile::tempdir().unwrap();
        let host = GitOperations::new(temp.path().into());
        let mut req = request();
        for n in 1..=3 {
            req.operation_id = uuid::Uuid::from_u128(n).to_string();
            host.execute(req.clone(), || async { Ok(true.into()) })
                .await
                .unwrap();
        }
        let first = host.page(&req.task_id, None, 2).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_eq!(
            first.entries[0].operation_id,
            uuid::Uuid::from_u128(1).to_string()
        );
        assert_eq!(
            first.next_cursor,
            Some(uuid::Uuid::from_u128(2).to_string())
        );
        std::fs::write(
            host.file(&req.task_id, &req.operation_id).unwrap(),
            "broken JSON",
        )
        .unwrap();
        let next = host
            .page(&req.task_id, first.next_cursor.as_deref(), 2)
            .unwrap();
        assert_eq!(next.entries.len(), 1);
        assert!(next.next_cursor.is_none());
        assert!(next.entries[0].receipt.is_none());
        assert!(next.entries[0].error.as_ref().unwrap().contains("损坏"));
        assert!(host.page(&req.task_id, Some("../other"), 2).is_err());
        assert!(host.page(&req.task_id, None, 0).is_err());
        assert!(host.page(&req.task_id, None, 51).is_err());
        assert!(
            host.page(&agent_domain::TaskId::new().to_string(), None, 20)
                .unwrap()
                .entries
                .is_empty()
        );
        std::fs::write(
            host.file(&req.task_id, &req.operation_id).unwrap(),
            vec![b' '; 256 * 1024 + 1],
        )
        .unwrap();
        let large = host
            .page(&req.task_id, first.next_cursor.as_deref(), 2)
            .unwrap();
        assert!(large.entries[0].error.as_ref().unwrap().contains("较大"));
        std::fs::write(temp.path().join(&req.task_id).join("bad-id.json"), "{}").unwrap();
        assert!(host.page(&req.task_id, None, 2).is_err());
    }
    #[tokio::test]
    async fn git_operation_timeout_is_unknown_and_cannot_be_reexecuted() {
        let directory = tempfile::tempdir().unwrap();
        let host = GitOperations::new(directory.path().into());
        let req = request();
        let count = AtomicUsize::new(0);
        assert!(
            host.execute(req.clone(), || async {
                count.fetch_add(1, Ordering::SeqCst);
                Err(format!(
                    "REVIEW_SYNC_PUSH_FAILED\n{}等待超时",
                    super::super::UNKNOWN_GIT_EFFECT
                ))
            })
            .await
            .is_err()
        );
        assert_eq!(
            host.get(&req.task_id, &req.operation_id)
                .unwrap()
                .unwrap()
                .outcome,
            GitOutcome::Unknown
        );
        drop(host);
        let restored = GitOperations::new(directory.path().into());
        assert!(
            restored
                .execute(req.clone(), || async {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(true.into())
                })
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            restored
                .get(&req.task_id, &req.operation_id)
                .unwrap()
                .unwrap()
                .outcome,
            GitOutcome::Unknown
        );
    }
}
