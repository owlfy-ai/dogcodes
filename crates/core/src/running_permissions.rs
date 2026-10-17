use super::{
    AppendRequest, CommandId, Core, CoreError, ExecutorRef, NewEvent, RepositoryError,
    RespondToApproval, RunId, RunState, StreamRef, StreamType, TaskId, TaskSummary, Utc, json,
};
use agent_protocol::ActiveRunSummary;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct RunningPermission {
    task_id: TaskId,
    executor: ExecutorRef,
    native_full_access: bool,
    mode: String,
}

type Entries = Arc<Mutex<BTreeMap<RunId, Arc<tokio::sync::Mutex<RunningPermission>>>>>;

#[derive(Default)]
pub(super) struct RunningPermissions(Entries);

pub(super) struct PermissionLease(Entries, RunId);
impl Drop for PermissionLease {
    fn drop(&mut self) {
        self.0.lock().expect("running permissions").remove(&self.1);
    }
}

impl RunningPermissions {
    pub(super) fn track(
        &self,
        task_id: &TaskId,
        run: &ActiveRunSummary,
        mode: Option<&str>,
    ) -> Option<PermissionLease> {
        if run.executor.provider_id.as_str() != "cursor" {
            return None;
        }
        let mode = mode.unwrap_or("default");
        self.0.lock().expect("running permissions").insert(
            run.run_id.clone(),
            Arc::new(tokio::sync::Mutex::new(RunningPermission {
                task_id: task_id.clone(),
                executor: run.executor.clone(),
                native_full_access: mode == "full-access",
                mode: mode.into(),
            })),
        );
        Some(PermissionLease(Arc::clone(&self.0), run.run_id.clone()))
    }

    fn get(&self, run_id: &RunId) -> Option<Arc<tokio::sync::Mutex<RunningPermission>>> {
        self.0
            .lock()
            .expect("running permissions")
            .get(run_id)
            .cloned()
    }
}

impl Core {
    pub(super) async fn running_permission_mode(
        &self,
        task_id: &TaskId,
        executor: &ExecutorRef,
    ) -> Option<String> {
        let task = self.require_task(task_id).await.ok()?;
        let run = task.active_primary_run?;
        if &run.executor != executor {
            return None;
        }
        let entry = self.running_permissions.get(&run.run_id)?;
        Some(entry.lock().await.mode.clone())
    }

    /// A local picker action authorizes only this managed run, never a future
    /// run, another installation, user questions, or vendor-denied operations.
    pub async fn set_running_permission_mode(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        executor: &ExecutorRef,
        mode: &str,
    ) -> Result<(), CoreError> {
        if executor.provider_id.as_str() != "cursor" || !matches!(mode, "default" | "full-access") {
            return Err(CoreError::CapabilityUnavailable(
                "此 Agent 暂不支持运行中修改权限策略".into(),
            ));
        }
        let entry = self
            .running_permissions
            .get(run_id)
            .ok_or_else(|| CoreError::Conflict("当前运行已结束或尚未就绪".into()))?;
        // Serialize picker changes and automatic decisions for this run.
        let mut policy = entry.lock().await;
        if &policy.task_id != task_id || &policy.executor != executor {
            return Err(CoreError::Conflict("运行与当前 Agent 不匹配".into()));
        }
        self.require_permission_run(task_id, run_id, executor)
            .await?;
        if mode == "default" && policy.native_full_access {
            return Err(CoreError::CapabilityUnavailable(
                "本轮以完全访问启动，请停止后再收紧权限".into(),
            ));
        }
        let previous = std::mem::replace(&mut policy.mode, mode.to_owned());
        if let Err(error) = self
            .approve_running_requests(task_id, run_id, &policy)
            .await
        {
            policy.mode = previous;
            return Err(error);
        }
        Ok(())
    }

    async fn require_permission_run(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        executor: &ExecutorRef,
    ) -> Result<TaskSummary, CoreError> {
        let task = self.require_task(task_id).await?;
        let valid = task.active_primary_run.as_ref().is_some_and(|run| {
            &run.run_id == run_id
                && &run.executor == executor
                && matches!(
                    run.state,
                    RunState::Running | RunState::WaitingForApproval | RunState::WaitingForInput
                )
        }) && self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(run_id);
        if !valid {
            return Err(CoreError::Conflict(
                "当前运行已结束、正在停止或已被替换".into(),
            ));
        }
        Ok(task)
    }

    pub(super) async fn apply_running_permission(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
    ) -> Result<(), CoreError> {
        let Some(entry) = self.running_permissions.get(run_id) else {
            return Ok(());
        };
        let policy = entry.lock().await;
        self.approve_running_requests(task_id, run_id, &policy)
            .await
    }

    async fn approve_running_requests(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        policy: &RunningPermission,
    ) -> Result<(), CoreError> {
        if policy.mode != "full-access" {
            return Ok(());
        }
        let mut conflicts = 0;
        let mut applied = false;
        loop {
            // Read the version before the pending list: any concurrent manual
            // answer, cancellation or turn replacement then invalidates the append.
            let task = match self
                .require_permission_run(task_id, run_id, &policy.executor)
                .await
            {
                Err(CoreError::Conflict(_)) if applied => return Ok(()),
                result => result?,
            };
            let active = task
                .active_primary_run
                .as_ref()
                .expect("checked active run");
            let pending = self.repository.pending_approvals(task_id, 50).await?;
            let Some((approval, option)) = pending
                .iter()
                .filter(|a| a.turn_id == active.turn_id && a.question.is_none())
                .find_map(|a| {
                    a.options
                        .iter()
                        .find(|o| o.semantic_role == "approve_once")
                        .map(|o| (a, o))
                })
            else {
                return Ok(());
            };
            // Use only the vendor's allow-once option. Full access does not
            // invent an option or persist an allow rule in the native Agent.
            let event = NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: task_id.to_string(),
                },
                "approval.resolved",
                json!({"taskId":task_id,"turnId":active.turn_id,"approvalId":approval.approval_id,
                    "optionId":option.option_id,"answer":null,"requestedAtSequence":approval.requested_at_sequence,"resolvedAt":Utc::now(),"permissionMode":"full-access"}),
            );
            let outcome = self
                .repository
                .append(AppendRequest {
                    request_fingerprint: None,
                    command_id: CommandId::new(),
                    idempotency_key: event.event_id.to_string(),
                    expected_stream_version: Some(task.stream_version),
                    artifacts: vec![],
                    events: vec![event],
                })
                .await;
            let outcome = match outcome {
                Err(RepositoryError::Conflict { .. }) if conflicts < 8 => {
                    conflicts += 1;
                    continue;
                }
                result => result?,
            };
            self.publish(&outcome.events);
            applied = true;
            self.deliver_approval_response(
                RespondToApproval {
                    task_id: task_id.clone(),
                    turn_id: approval.turn_id.clone(),
                    requested_at_sequence: approval.requested_at_sequence,
                    approval_id: approval.approval_id.clone(),
                    option_id: option.option_id.clone(),
                    answer: None,
                },
                Some(active.turn_id.clone()),
                outcome.ack.command_id.clone(),
            )
            .await;
        }
    }
}
