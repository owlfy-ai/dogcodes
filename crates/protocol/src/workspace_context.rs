//! What a project's working directory can tell the application, wherever it lives.
//! Implementations read files and Git; callers only see these facts.
use crate::{
    ChangeSummary, GitWorkspaceSnapshotV1, RelevantFile, WorkspaceCollectionDiagnostic,
    WorkspaceDiagnosticKind,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceContextSnapshot {
    pub relevant_files: Vec<RelevantFile>,
    pub dependency_changes: Vec<RelevantFile>,
    pub change_summary: ChangeSummary,
    pub git_snapshot: Option<GitWorkspaceSnapshotV1>,
    pub diagnostic: Option<WorkspaceCollectionDiagnostic>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceDiffCapture {
    pub base: String,
    pub status_fingerprint: String,
    pub content: Vec<u8>,
}

#[async_trait]
pub trait WorkspaceTurnCapture: Send + Sync {
    async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
        Ok(None)
    }
    async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic>;
}

impl Default for WorkspaceContextSnapshot {
    fn default() -> Self {
        Self {
            relevant_files: vec![],
            dependency_changes: vec![],
            change_summary: ChangeSummary {
                base: "unknown".into(),
                head: "unknown".into(),
                diff_stat: "unknown".into(),
                artifact_ref: None,
            },
            git_snapshot: None,
            diagnostic: None,
        }
    }
}

#[async_trait]
pub trait WorkspaceContextProvider: Send + Sync {
    /// Persist the complete Git file state independently of bounded review diffs.
    async fn capture_fork_snapshot(&self, _paths: &[String]) -> Result<Value, String> {
        Err("当前环境不支持保存 worktree 快照".into())
    }

    /// Restore immutable turn snapshots into isolated checkouts, in root order.
    async fn fork_worktrees(&self, _snapshot: Value, _name: &str) -> Result<Vec<String>, String> {
        Err("当前环境不支持创建 Git worktree".into())
    }

    async fn delete_task_review_data(&self, _task_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn validate_workspace_roots(
        &self,
        _directories: &[String],
    ) -> Result<(), WorkspaceCollectionDiagnostic> {
        Ok(())
    }
    async fn recover_turn(
        &self,
        _workspace_path: &str,
        _checkpoint: Value,
    ) -> Result<Value, WorkspaceCollectionDiagnostic> {
        Err(WorkspaceCollectionDiagnostic {
            kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
        })
    }
    async fn begin_turn(
        &self,
        _workspace_path: &str,
    ) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
        Err(WorkspaceCollectionDiagnostic {
            kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
        })
    }
    async fn collect(&self, workspace_path: &str) -> WorkspaceContextSnapshot;

    async fn capture_diff(
        &self,
        _workspace_path: &str,
    ) -> Result<WorkspaceDiffCapture, WorkspaceCollectionDiagnostic> {
        Err(WorkspaceCollectionDiagnostic {
            kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
        })
    }
}
