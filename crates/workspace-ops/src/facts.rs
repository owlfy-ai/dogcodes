//! What workspace operations ask of whoever owns tasks and artifacts. The service
//! answers from its core; a shim asks the desktop over the connection.
use agent_domain::{ArtifactId, ExecutorRef, RunId, TaskId};
use agent_protocol::{ArtifactCursor, ArtifactPage, BootstrapSnapshot};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::Any;

/// Held while a mutation runs, so an agent cannot start in the same workspace.
pub type MutationGuard = Box<dyn Any + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerifiedArtifact {
    pub media_type: String,
    pub content: Vec<u8>,
}

#[async_trait]
pub trait ReviewFacts: Send + Sync {
    /// Whether these operations are running on the machine hosting the tasks. There,
    /// a project on another machine has paths that must never be read as local ones.
    fn on_hub(&self) -> bool;
    async fn bootstrap(&self) -> Result<BootstrapSnapshot, String>;
    async fn try_workspace_mutation(&self) -> Result<MutationGuard, String>;
    async fn task_artifacts(
        &self,
        task: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, String>;
    async fn verified_artifact(
        &self,
        task: &TaskId,
        artifact: &ArtifactId,
        sha256: &str,
    ) -> Result<VerifiedArtifact, String>;
    async fn summarize_commit(
        &self,
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    ) -> Result<String, String>;
    async fn review_run_exists(&self, task: &TaskId, run: &RunId) -> Result<bool, String>;
    /// `request` is the core's own review request, which only it interprets.
    async fn start_project_review(
        &self,
        request: Value,
        input: Option<Value>,
    ) -> Result<Value, String>;
    /// What Git on another machine needs someone to answer. `None` means declined.
    /// On the hub, the question goes straight to the review host instead.
    async fn ask_git_credential(
        &self,
        _task: &TaskId,
        _prompt: &str,
    ) -> Result<Option<String>, String> {
        Err("not available here".into())
    }
}
