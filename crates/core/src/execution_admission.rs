//! Shared availability checks for new work. Callers hold `agent_admission`'s
//! read guard through durable acceptance; existing receipts bypass these checks.
use crate::{Core, CoreError};
use agent_domain::ExecutorRef;
use agent_protocol::{InstallationStatus, InstallationSummary};
use std::sync::Arc;

/// A driver captured at acceptance, never resolved again from the registry.
#[derive(Clone)]
pub struct AcceptedExecution {
    pub(crate) driver: Arc<dyn driver_api::AgentDriver>,
}

/// Keep this admission alive through the caller's durable acceptance and dispatch.
/// Execution itself must not hold the guard: disabling an Agent remains immediate.
pub struct ExecutionAdmission<'a> {
    _guard: tokio::sync::RwLockReadGuard<'a, ()>,
    execution: AcceptedExecution,
}
impl ExecutionAdmission<'_> {
    #[must_use]
    pub fn execution(&self) -> AcceptedExecution {
        self.execution.clone()
    }
}

pub(crate) fn installation_is_ready(
    executor: &ExecutorRef,
    installations: &[InstallationSummary],
) -> bool {
    installations.iter().any(|installation| {
        installation.status == InstallationStatus::Ready
            && installation.provider_id == executor.provider_id.as_str()
            && executor.installation_id.as_deref() == Some(installation.installation_id.as_str())
    })
}

impl Core {
    pub async fn available_execution_candidates(&self) -> Result<Vec<ExecutorRef>, CoreError> {
        let _guard = self.agent_admission.read().await;
        let preferences = self.agent_preferences().await?.preferences;
        let snapshot = self.bootstrap().await?;
        let mut candidates: Vec<_> = self
            .drivers
            .read()
            .await
            .keys()
            .filter(|executor| {
                !preferences
                    .disabled
                    .iter()
                    .any(|id| id == executor.provider_id.as_str())
                    && installation_is_ready(executor, &snapshot.installations)
            })
            .cloned()
            .collect();
        candidates.sort_by_key(|executor| {
            preferences
                .order
                .iter()
                .position(|id| id == executor.provider_id.as_str())
                .unwrap_or(usize::MAX)
        });
        Ok(candidates)
    }

    pub async fn admit_execution(
        &self,
        executor: &ExecutorRef,
    ) -> Result<ExecutionAdmission<'_>, CoreError> {
        let guard = self.agent_admission.read().await;
        self.require_available_executor(executor).await?;
        let driver = self
            .drivers
            .read()
            .await
            .get(executor)
            .cloned()
            .ok_or_else(|| {
                CoreError::CapabilityUnavailable("所选 Agent 驱动不可用，请重新检测".into())
            })?;
        Ok(ExecutionAdmission {
            _guard: guard,
            execution: AcceptedExecution { driver },
        })
    }

    pub(crate) async fn require_enabled_executor(
        &self,
        executor: &ExecutorRef,
    ) -> Result<(), CoreError> {
        let preferences = self.agent_preferences().await?;
        if preferences
            .preferences
            .disabled
            .iter()
            .any(|id| id == executor.provider_id.as_str())
        {
            return Err(CoreError::CapabilityUnavailable(
                "所选 Agent 已禁用，请启用后重试或选择其他 Agent".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn require_available_executor(
        &self,
        executor: &ExecutorRef,
    ) -> Result<(), CoreError> {
        self.require_enabled_executor(executor).await?;
        let snapshot = self.bootstrap().await?;
        if !installation_is_ready(executor, &snapshot.installations) {
            return Err(CoreError::CapabilityUnavailable(
                "所选 Agent 安装尚未就绪或已更换，请重新检测并选择可用的 Agent".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::MemoryRepository;
    use agent_domain::{ProjectId, ProviderId, TaskId};
    use agent_protocol::{AgentPreferences, CommandEnvelope, CoreCommand, SubmitTurn, TaskSummary};
    use std::sync::Arc;

    #[tokio::test]
    async fn primary_admission_waits_for_disable_and_checks_actual_executor() {
        for override_provider in [None, Some("other")] {
            let repository = Arc::new(MemoryRepository::default());
            let core = Arc::new(Core::new(repository.clone()));
            let task_id = TaskId::new();
            let executor = |provider| ExecutorRef {
                provider_id: ProviderId::parse(provider).unwrap(),
                installation_id: None,
            };
            *repository.task_override.lock().await = Some(TaskSummary {
                task_id: task_id.clone(),
                project_id: ProjectId::new(),
                title: "test".into(),
                preferred_executor: executor("fake"),
                last_turn_executor: None,
                updated_at: None,
                stream_version: 1,
                active_primary_run: None,
            });
            let held = core.agent_admission.read().await;
            let mut disable =
                CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
                    disabled: vec![override_provider.unwrap_or("fake").into()],
                    ..Default::default()
                }));
            disable.expected_stream_version = Some(0);
            let update = core.execute(disable);
            tokio::pin!(update);
            assert!(futures::poll!(&mut update).is_pending());
            let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
                task_id,
                prompt: "do work".into(),
                executor_override: override_provider.map(executor),
                model: None,
                thinking: None,
                permission_mode: None,
                attachments: vec![],
                review_fix: None,
            }));
            submit.expected_stream_version = Some(1);
            let send = core.execute(submit);
            tokio::pin!(send);
            assert!(futures::poll!(&mut send).is_pending());
            drop(held);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                update.await.unwrap();
                assert!(send.await.unwrap_err().to_string().contains("已禁用"));
            })
            .await
            .unwrap();
            assert!(
                core.sync_events(0, 1000)
                    .await
                    .unwrap()
                    .iter()
                    .all(|event| event.event_type != "turn.queued")
            );
        }
    }
}
