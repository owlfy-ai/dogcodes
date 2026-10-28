//! Recovery inventory and startup repair, including pending app-owned task cleanup.
use crate::{Core, CoreError};
use agent_domain::{RunId, RunState};
use agent_protocol::{
    ActorRef, InstallationStatus, RecoveryCode, RecoveryItem, RecoverySnapshot, SessionMapStatus,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskDeletion {
    pub task_id: agent_domain::TaskId,
    pub deletion_sequence: u64,
}

pub(crate) type OwnedRuns = Arc<Mutex<BTreeMap<RunId, Arc<dyn driver_api::AgentDriver>>>>;
pub(crate) type OwnedSessions = Arc<Mutex<BTreeMap<RunId, driver_api::SessionHandle>>>;
pub(crate) type OwnedControls = Arc<Mutex<BTreeMap<RunId, Arc<crate::run_control::Control>>>>;

pub(crate) struct RunOwnership {
    pub runs: OwnedRuns,
    pub sessions: OwnedSessions,
    pub controls: OwnedControls,
    pub control: Arc<crate::run_control::Control>,
    pub run_id: RunId,
}

impl Drop for RunOwnership {
    fn drop(&mut self) {
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runs.remove(&self.run_id);
        self.control.abandon();
        self.controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.run_id);
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.run_id);
    }
}

fn provider_name(value: &str) -> String {
    match value {
        "codex" | "claude-code" | "kimi" | "grok" | "cursor" | "antigravity" | "zcode" | "fake" => {
            value.into()
        }
        _ => "other".into(),
    }
}

impl Core {
    #[cfg(test)]
    pub(crate) fn own_run(
        &self,
        run_id: RunId,
        driver: Arc<dyn driver_api::AgentDriver>,
    ) -> RunOwnership {
        self.own_run_with_usage(run_id, driver, None)
    }
    pub(crate) fn own_run_with_usage(
        &self,
        run_id: RunId,
        driver: Arc<dyn driver_api::AgentDriver>,
        usage_provider: Option<Arc<dyn driver_api::QuotaProvider>>,
    ) -> RunOwnership {
        let mut runs = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let control = Arc::new(crate::run_control::Control::with_usage_provider(
            run_id.clone(),
            usage_provider,
        ));
        self.owned_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run_id.clone(), control.clone());
        runs.insert(run_id.clone(), driver);
        RunOwnership {
            runs: Arc::clone(&self.owned_runs),
            sessions: Arc::clone(&self.owned_sessions),
            controls: Arc::clone(&self.owned_controls),
            control,
            run_id,
        }
    }

    pub(crate) fn run_driver(
        &self,
        run_id: &RunId,
        executor: &agent_domain::ExecutorRef,
    ) -> Result<Arc<dyn driver_api::AgentDriver>, CoreError> {
        self.owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .filter(|driver| driver.executor() == executor)
            .cloned()
            .ok_or_else(|| CoreError::DriverUnavailable(executor.provider_id.to_string()))
    }

    pub(crate) fn owned_run_control(
        &self,
        run: &RunId,
    ) -> Option<Arc<crate::run_control::Control>> {
        let runs = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !runs.contains_key(run) {
            return None;
        }
        self.owned_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run)
            .cloned()
    }

    pub(crate) fn bind_run_session(
        &self,
        run: &RunId,
        driver: &Arc<dyn driver_api::AgentDriver>,
        handle: driver_api::SessionHandle,
    ) -> Result<(), CoreError> {
        let runs = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !runs
            .get(run)
            .is_some_and(|owned| Arc::ptr_eq(owned, driver))
        {
            return Err(CoreError::Conflict("run no longer owns this driver".into()));
        }
        let mut sessions = self
            .owned_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions.contains_key(run) {
            return Err(CoreError::Conflict(
                "run already has a native session".into(),
            ));
        }
        sessions.insert(run.clone(), handle);
        Ok(())
    }

    pub(crate) fn run_session(
        &self,
        run: &RunId,
        executor: &agent_domain::ExecutorRef,
    ) -> Result<driver_api::SessionHandle, CoreError> {
        let runs = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runs
            .get(run)
            .is_none_or(|driver| driver.executor() != executor)
        {
            return Err(CoreError::DriverUnavailable(
                executor.provider_id.to_string(),
            ));
        }
        self.owned_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run)
            .cloned()
            .ok_or_else(|| {
                CoreError::CapabilityUnavailable(
                    "active native session is not ready for interaction".into(),
                )
            })
    }

    /// Read-only proof for clients retiring task-owned recovery records.
    /// Missing projections and failed lookups are not permanent deletion.
    pub async fn task_deletion(
        &self,
        task: &agent_domain::TaskId,
    ) -> Result<Option<TaskDeletion>, CoreError> {
        match self.submission_task_deletion(task).await? {
            None => Ok(None),
            Some(agent_protocol::submission::SubmissionResolution::TaskDeleted {
                task_id,
                deletion_sequence,
            }) => Ok(Some(TaskDeletion {
                task_id,
                deletion_sequence,
            })),
            Some(_) => Err(CoreError::Internal(
                "invalid permanent task deletion evidence".into(),
            )),
        }
    }
    pub(crate) async fn cleanup_deleted_tasks(
        &self,
        only: Option<&agent_domain::TaskId>,
    ) -> Result<(), CoreError> {
        let mut failure = None;
        for task in self.repository.pending_task_cleanups().await? {
            if only.is_some_and(|id| id != &task) {
                continue;
            }
            // This queue is created by durable permanent deletion, not a missing
            // task projection. Release task-owned MCP listeners and temp configs
            // even if the independent review-file cleanup needs another attempt.
            self.forget_deleted_history(&task).await;
            match self
                .workspace_context
                .delete_task_review_data(task.as_str())
                .await
            {
                Ok(()) => self.repository.complete_task_cleanup(&task).await?,
                Err(error) => {
                    failure = Some(CoreError::Internal(format!(
                        "会话已永久删除，快照清理未完成，重启后将重试：{error}"
                    )));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Called exclusively during startup, before accepting commands or registering drivers.
    /// Does not replay prompts, kill processes, or restore workspace files.
    pub async fn recover_interrupted_turns(&self) -> Result<(), CoreError> {
        use agent_protocol::EventSource;
        use serde_json::json;
        // A busy/corrupt review store must not prevent unrelated conversations
        // from starting. The durable pending marker is retried at next startup.
        if let Err(error) = self.cleanup_deleted_tasks(None).await {
            eprintln!("pending conversation cleanup: {error}");
        }
        self.recover_reviews().await?;
        let snapshot = self.repository.bootstrap().await?;
        for task in &snapshot.tasks {
            let Some(run) = &task.active_primary_run else {
                continue;
            };
            if self
                .owned_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&run.run_id)
            {
                continue;
            }
            let scope = self.turn_workspace_scope(&task.task_id).await;
            self.recover_run_workspace(
                &task.task_id,
                &run.turn_id,
                scope.as_ref(),
                snapshot.last_sequence,
            )
            .await?;
            for approval in self
                .repository
                .pending_approvals(&task.task_id, 1000)
                .await?
                .into_iter()
                .filter(|a| a.turn_id == run.turn_id)
            {
                self.append_runtime_event(&task.task_id, "approval.expired", json!({"taskId":task.task_id,"turnId":run.turn_id,"approvalId":approval.approval_id,"reason":"host_interrupted"}), EventSource::Hub).await?;
            }
            self.append_runtime_event(&task.task_id, "turn.connection_lost", json!({"taskId":task.task_id,"turnId":run.turn_id,"runId":run.run_id,"executor":run.executor,"reason":"host_interrupted","result":"unknown","continuationAllowed":true,"message":"Execution connection lost. The local turn is closed for continuation; external tools may have continued. Inspect current results before retrying side effects. No prompt was replayed."}), EventSource::Hub).await?;
        }
        Ok(())
    }

    /// Shared checkpoint recovery for startup and an abandoned live worker.
    pub(super) async fn recover_run_workspace(
        &self,
        task_id: &agent_domain::TaskId,
        turn_id: &agent_domain::TurnId,
        scope: Option<&crate::TurnWorkspaceScope>,
        through: u64,
    ) -> Result<(), CoreError> {
        use agent_domain::ArtifactId;
        use sha2::{Digest, Sha256};
        let mut cursor = 0;
        let mut checkpoint = None;
        let mut changes_recorded = false;
        loop {
            let events = self
                .repository
                .task_events(task_id, cursor, through, 1000)
                .await?;
            if events.is_empty() {
                break;
            }
            for event in &events {
                if event.payload["turnId"].as_str() != Some(turn_id.as_str()) {
                    continue;
                }
                if event.event_type == "workspace.turn_checkpoint_captured" {
                    checkpoint = Some(event.payload.clone());
                }
                if event.event_type == "workspace.turn_changes_captured" {
                    changes_recorded = true;
                }
            }
            if let Some(event) = events.last() {
                cursor = event.sequence;
            }
        }
        if !changes_recorded {
            let mut result = Err(agent_protocol::WorkspaceCollectionDiagnostic {
                kind: agent_protocol::WorkspaceDiagnosticKind::WorkspaceUnavailable,
            });
            if let Some(payload) = checkpoint
                && let Some(scope) = scope
            {
                let id: ArtifactId =
                    serde_json::from_value(payload["artifact"]["artifactId"].clone())
                        .map_err(|e| CoreError::Internal(e.to_string()))?;
                if let Some(artifact) = self.repository.artifact(&id).await?
                    && hex::encode(Sha256::digest(&artifact.content))
                        == artifact.metadata.payload_sha256
                    && let Ok(value) = serde_json::from_slice(&artifact.content)
                {
                    result = crate::multi_workspace::recover(
                        self.workspace_context.as_ref(),
                        &scope.project_paths,
                        value,
                    )
                    .await;
                }
            }
            if let Some(scope) = scope {
                self.record_turn_changes(scope, result).await?;
            }
        }
        Ok(())
    }

    pub async fn recovery_snapshot(&self, actor: &ActorRef) -> Result<RecoverySnapshot, CoreError> {
        if *actor != ActorRef::LocalUser {
            return Err(CoreError::InvalidArgument(
                "recovery requires the local user".into(),
            ));
        }
        let snapshot = self.bootstrap().await?;
        let drivers = self
            .drivers
            .read()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let owned = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut items = Vec::new();
        for installation in snapshot.installations.iter().take(100) {
            let code = match installation.status {
                InstallationStatus::NotConfigured => Some(RecoveryCode::InstallationUnconfigured),
                InstallationStatus::Broken => Some(RecoveryCode::InstallationBroken),
                InstallationStatus::UnsupportedVersion => {
                    Some(RecoveryCode::InstallationUnsupported)
                }
                InstallationStatus::Installed => Some(RecoveryCode::InstallationNotReady),
                InstallationStatus::Ready => {
                    let registered = drivers.iter().any(|executor| {
                        executor.installation_id.as_deref()
                            == Some(installation.installation_id.as_str())
                    });
                    (!registered).then_some(RecoveryCode::DriverUnavailable)
                }
            };
            if let Some(code) = code {
                items.push(RecoveryItem {
                    code,
                    provider_id: Some(provider_name(&installation.provider_id)),
                    task_id: None,
                    run_id: None,
                    turn_id: None,
                    expected_stream_version: None,
                    can_cancel: false,
                });
            }
        }
        for task in snapshot.tasks.iter().take(100) {
            let make = |code, can_cancel| RecoveryItem {
                code,
                provider_id: Some(provider_name(
                    task.active_primary_run
                        .as_ref()
                        .map_or(task.preferred_executor.provider_id.as_str(), |run| {
                            run.executor.provider_id.as_str()
                        }),
                )),
                task_id: Some(task.task_id.clone()),
                run_id: task
                    .active_primary_run
                    .as_ref()
                    .map(|run| run.run_id.clone()),
                turn_id: task
                    .active_primary_run
                    .as_ref()
                    .map(|run| run.turn_id.clone()),
                expected_stream_version: Some(task.stream_version),
                can_cancel,
            };
            if let Some(run) = &task.active_primary_run {
                if !owned.contains(&run.run_id) {
                    items.push(make(RecoveryCode::RunOwnershipUnknown, false));
                } else if run.state == RunState::Interrupting {
                    items.push(make(RecoveryCode::RunInterrupting, false));
                } else if !self
                    .repository
                    .pending_approvals(&task.task_id, 1)
                    .await?
                    .is_empty()
                {
                    items.push(make(RecoveryCode::ApprovalPending, true));
                }
            }
            if snapshot.session_maps.iter().any(|session| {
                session.task_id == task.task_id
                    && session.executor == task.preferred_executor
                    && matches!(
                        session.status,
                        SessionMapStatus::Missing
                            | SessionMapStatus::Incompatible
                            | SessionMapStatus::Detached
                    )
            }) {
                items.push(make(RecoveryCode::SessionUnavailable, false));
            }
        }
        Ok(RecoverySnapshot {
            schema_version: 1,
            generated_at: chrono::Utc::now(),
            last_sequence: snapshot.last_sequence,
            truncated: snapshot.tasks.len() > 100 || snapshot.installations.len() > 100,
            items,
        })
    }
}

#[cfg(test)]
mod ownership_tests;
