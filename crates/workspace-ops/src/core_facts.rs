//! The answers on the machine hosting the tasks.
use crate::{MutationGuard, ReviewFacts, VerifiedArtifact};
use agent_core::Core;
use agent_domain::{ArtifactId, ExecutorRef, RunId, TaskId};
use agent_protocol::{ActorRef, ArtifactCursor, ArtifactPage, BootstrapSnapshot};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

#[async_trait]
impl ReviewFacts for Arc<Core> {
    fn on_hub(&self) -> bool {
        true
    }
    async fn bootstrap(&self) -> Result<BootstrapSnapshot, String> {
        Core::bootstrap(self).await.map_err(|e| e.to_string())
    }
    async fn try_workspace_mutation(&self) -> Result<MutationGuard, String> {
        let guard = Core::try_workspace_mutation(self).map_err(|e| e.to_string())?;
        Ok(Box::new(guard))
    }
    async fn task_artifacts(
        &self,
        task: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, String> {
        Core::task_artifacts(self, &ActorRef::LocalUser, task, before, limit)
            .await
            .map_err(|e| e.to_string())
    }
    async fn verified_artifact(
        &self,
        task: &TaskId,
        artifact: &ArtifactId,
        sha256: &str,
    ) -> Result<VerifiedArtifact, String> {
        Core::verified_local_artifact(self, &ActorRef::LocalUser, task, artifact, sha256)
            .await
            .map(|artifact| VerifiedArtifact {
                media_type: artifact.metadata.media_type,
                content: artifact.content,
            })
            .map_err(|e| e.to_string())
    }
    async fn summarize_commit(
        &self,
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    ) -> Result<String, String> {
        Core::summarize_commit(self, executor, changes, language.as_deref()).await
    }
    async fn review_run_exists(&self, task: &TaskId, run: &RunId) -> Result<bool, String> {
        let existing = Core::turn_reviews(self, task)
            .await
            .map_err(|e| e.to_string())?;
        Ok(existing.iter().any(|entry| &entry.request.run_id == run))
    }
    async fn start_project_review(
        &self,
        request: Value,
        input: Option<Value>,
    ) -> Result<Value, String> {
        let request = serde_json::from_value(request).map_err(|e| e.to_string())?;
        let run = Core::start_project_review(self, request, input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(run).map_err(|e| e.to_string())
    }
}
