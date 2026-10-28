//! Daemon-owned "start repair" workflow. The service freezes the selected
//! findings, target Agent and context package, then drives context
//! confirmation and the repair turn with fixed command identities. Any client
//! can discover, verify, continue or abandon the original request; no client
//! has to replay the steps from its own storage.
use agent_core::Core;
use agent_domain::{CommandId, ExecutorRef, RunId, TaskId};
use agent_protocol::{
    ActorRef, BootstrapSnapshot, CommandAck, CommandEnvelope, ConfirmContextPackage,
    ContextPackageKind, ContextPreviewRequest, ContextStrength, CoreCommand, ResumeCapability,
    ReviewFixRef, SessionMapStatus, StreamType, SubmitTurn, TaskSummary,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const REPAIR_PROMPT: &str = "请处理我选中的审查意见。";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepairContext {
    pub operation_id: CommandId,
    pub request: ContextPreviewRequest,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepairIntent {
    #[serde(default = "local_actor")]
    pub actor: ActorRef,
    /// Also the command identity of the repair turn.
    pub operation_id: CommandId,
    pub task_id: TaskId,
    /// Task version observed when the request was frozen.
    pub base_version: u64,
    pub reference: ReviewFixRef,
    pub target: ExecutorRef,
    pub context: Option<RepairContext>,
    pub created_at: DateTime<Utc>,
}
fn local_actor() -> ActorRef {
    ActorRef::LocalUser
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
pub enum RepairState {
    /// Not yet proven submitted. `submission_version` is the verified task
    /// version after the original context step, when that step exists.
    Pending {
        submission_version: Option<u64>,
        context_confirmed: bool,
        last_error: Option<String>,
    },
    Submitted {
        acknowledgement: CommandAck,
    },
    /// Tracking ended without proof of submission. An accepted Agent switch is
    /// not undone.
    Abandoned,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepairEntry {
    pub intent: RepairIntent,
    #[serde(flatten)]
    pub state: RepairState,
    pub updated_at: DateTime<Utc>,
}
impl RepairEntry {
    fn pending(&self) -> bool {
        matches!(self.state, RepairState::Pending { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "action",
    deny_unknown_fields
)]
pub enum RepairCall {
    Start {
        operation_id: CommandId,
        task_id: TaskId,
        reference: ReviewFixRef,
        target: Option<ExecutorRef>,
    },
    /// Read only. Returns the latest record for the review run, if any.
    Get { task_id: TaskId, run_id: RunId },
    Resume {
        task_id: TaskId,
        run_id: RunId,
        operation_id: CommandId,
    },
    Abandon {
        task_id: TaskId,
        run_id: RunId,
        operation_id: CommandId,
    },
}
impl RepairCall {
    pub fn task_id(&self) -> &TaskId {
        match self {
            Self::Start { task_id, .. }
            | Self::Get { task_id, .. }
            | Self::Resume { task_id, .. }
            | Self::Abandon { task_id, .. } => task_id,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    schema_version: u8,
    entry: RepairEntry,
}

pub struct ReviewRepairs {
    root: PathBuf,
    // Serializes freezing, driving and abandoning so only one actor ever
    // issues the original commands.
    gate: tokio::sync::Mutex<()>,
}

fn regular(path: &Path, directory: bool) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if (directory && m.is_dir()) || (!directory && m.is_file()) => Ok(true),
        Ok(_) => Err("修复记录路径异常，未覆盖或重放操作".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn submit_envelope(intent: &RepairIntent, version: u64) -> CommandEnvelope<CoreCommand> {
    let mut envelope = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: Some(intent.reference.clone()),
        attachments: Vec::new(),
        permission_mode: None,
        thinking: None,
        model: None,
        task_id: intent.task_id.clone(),
        prompt: REPAIR_PROMPT.into(),
        executor_override: None,
    }));
    envelope.command_id = intent.operation_id.clone();
    envelope.idempotency_key = intent.operation_id.to_string();
    envelope.expected_stream_version = Some(version);
    envelope.actor = intent.actor.clone();
    envelope
}
fn context_envelope(
    intent: &RepairIntent,
    context: &RepairContext,
) -> CommandEnvelope<CoreCommand> {
    let mut envelope =
        CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
            request: context.request.clone(),
            expected_payload_sha256: context.hash.clone(),
            edited_package: None,
        }));
    envelope.command_id = context.operation_id.clone();
    envelope.idempotency_key = context.operation_id.to_string();
    envelope.expected_stream_version = Some(intent.base_version);
    envelope.actor = intent.actor.clone();
    envelope
}
fn accepted(ack: Option<CommandAck>) -> Option<CommandAck> {
    ack.filter(|ack| ack.accepted)
}

/// Same routing as a manual Agent switch: resume a reconcilable native session
/// of the target when one exists, otherwise hand off.
pub(crate) fn context_request(
    snapshot: &BootstrapSnapshot,
    task: &TaskSummary,
    target: &ExecutorRef,
) -> ContextPreviewRequest {
    let resumable = snapshot.session_maps.iter().find(|mapping| {
        mapping.task_id == task.task_id
            && &mapping.executor == target
            && matches!(
                mapping.status,
                SessionMapStatus::Resumable | SessionMapStatus::Active
            )
            && mapping.resume_capability == ResumeCapability::Supported
    });
    ContextPreviewRequest {
        task_id: task.task_id.clone(),
        kind: if resumable.is_some() {
            ContextPackageKind::Reconciliation
        } else {
            ContextPackageKind::Handoff
        },
        strength: ContextStrength::Standard,
        source_executor: task.preferred_executor.clone(),
        target_executor: target.clone(),
        after_sequence: resumable.map_or(0, |mapping| mapping.last_seen_sequence),
        through_sequence: snapshot.last_sequence,
    }
}

impl ReviewRepairs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: tokio::sync::Mutex::default(),
        }
    }
    fn path(&self, task: &TaskId, run: &RunId) -> PathBuf {
        // Both IDs are validated prefixed ULIDs, so they are safe path components.
        self.root.join(task.as_str()).join(format!("{run}.json"))
    }
    async fn read(&self, task: &TaskId, run: &RunId) -> Result<Option<RepairEntry>, String> {
        let (root, directory, path) = (
            self.root.clone(),
            self.root.join(task.as_str()),
            self.path(task, run),
        );
        tokio::task::spawn_blocking(move || {
            if !regular(&root, true)? || !regular(&directory, true)? || !regular(&path, false)? {
                return Ok(None);
            }
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            let stored: Stored = serde_json::from_slice(&bytes)
                .map_err(|_| "修复记录损坏，已保留原记录，请检查本机存储".to_string())?;
            if stored.schema_version != 1 {
                return Err("修复记录版本不受支持，已保留原记录".into());
            }
            Ok(Some(stored.entry))
        })
        .await
        .map_err(|_| "修复记录读取失败".to_string())?
    }
    async fn write(&self, entry: &RepairEntry) -> Result<(), String> {
        let directory = self.root.join(entry.intent.task_id.as_str());
        let path = self.path(&entry.intent.task_id, &entry.intent.reference.run_id);
        let root = self.root.clone();
        let bytes = serde_json::to_vec(&Stored {
            schema_version: 1,
            entry: entry.clone(),
        })
        .map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || {
            for dir in [&root, &directory] {
                if !regular(dir, true)? {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
            }
            regular(&path, false)?;
            let mut file = tempfile::Builder::new()
                .prefix(".pending-")
                .suffix(".tmp")
                .tempfile_in(&directory)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
            file.persist(&path).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::fs::File::open(&directory)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
        .map_err(|_| "修复记录保存失败".to_string())?
    }
    /// Permanent task deletion removes that task's repair records.
    pub fn forget_task(&self, task: &str) -> Result<(), String> {
        let task = TaskId::parse(task).map_err(|e| e.to_string())?;
        let directory = self.root.join(task.as_str());
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn call(&self, core: &Arc<Core>, call: RepairCall) -> Result<Value, String> {
        self.call_as(core, call, ActorRef::LocalUser).await
    }
    pub async fn call_as(
        &self,
        core: &Arc<Core>,
        call: RepairCall,
        actor: ActorRef,
    ) -> Result<Value, String> {
        let entry = match call {
            RepairCall::Get { task_id, run_id } => {
                return serde_json::to_value(self.read(&task_id, &run_id).await?)
                    .map_err(|e| e.to_string());
            }
            RepairCall::Start {
                operation_id,
                task_id,
                reference,
                target,
            } => {
                let _gate = self.gate.lock().await;
                let entry = self
                    .freeze_as(core, operation_id, task_id, reference, target, actor)
                    .await?;
                // A retried start of a finished request is read-only.
                if entry.pending() {
                    self.drive(core, entry).await?
                } else {
                    entry
                }
            }
            RepairCall::Resume {
                task_id,
                run_id,
                operation_id,
            } => {
                let _gate = self.gate.lock().await;
                let entry = self.original(&task_id, &run_id, &operation_id).await?;
                if entry.pending() {
                    self.drive(core, entry).await?
                } else {
                    entry
                }
            }
            RepairCall::Abandon {
                task_id,
                run_id,
                operation_id,
            } => {
                let _gate = self.gate.lock().await;
                let entry = self.original(&task_id, &run_id, &operation_id).await?;
                self.abandon(core, entry).await?
            }
        };
        serde_json::to_value(entry).map_err(|e| e.to_string())
    }

    async fn original(
        &self,
        task: &TaskId,
        run: &RunId,
        operation: &CommandId,
    ) -> Result<RepairEntry, String> {
        self.read(task, run)
            .await?
            .filter(|entry| &entry.intent.operation_id == operation)
            .ok_or_else(|| "原修复记录不存在或已被替换，请重新核实".into())
    }

    /// Records the frozen request without mutating the task. Callers hold the gate.
    #[cfg(test)]
    pub(crate) async fn freeze(
        &self,
        core: &Arc<Core>,
        operation_id: CommandId,
        task_id: TaskId,
        reference: ReviewFixRef,
        target: Option<ExecutorRef>,
    ) -> Result<RepairEntry, String> {
        self.freeze_as(
            core,
            operation_id,
            task_id,
            reference,
            target,
            ActorRef::LocalUser,
        )
        .await
    }
    async fn freeze_as(
        &self,
        core: &Arc<Core>,
        operation_id: CommandId,
        task_id: TaskId,
        mut reference: ReviewFixRef,
        target: Option<ExecutorRef>,
        actor: ActorRef,
    ) -> Result<RepairEntry, String> {
        if reference.input_sha256.trim().is_empty() || reference.finding_indices.is_empty() {
            return Err("请选择有效的审查问题。".into());
        }
        reference.finding_indices.sort_unstable();
        reference.finding_indices.dedup();
        if let Some(existing) = self.read(&task_id, &reference.run_id).await? {
            if existing.intent.operation_id == operation_id {
                // A retried start after a lost reply: same identity must mean same input.
                if existing.intent.reference != reference
                    || target
                        .as_ref()
                        .is_some_and(|target| target != &existing.intent.target)
                {
                    return Err("请求标识已用于不同的修复内容，未重复执行".into());
                }
                return Ok(existing);
            }
            if existing.pending() {
                return Err("请先核实原修复请求，不能更换问题或 Agent 重试。".into());
            }
        }
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .ok_or("原对话已不可用。")?;
        if task.active_primary_run.is_some() {
            return Err("请等待主对话当前回复结束后再提交修复。".into());
        }
        let target = target.unwrap_or_else(|| task.preferred_executor.clone());
        let context = if target == task.preferred_executor {
            None
        } else {
            let request = context_request(&snapshot, task, &target);
            let built = core
                .build_context_preview(request.clone())
                .await
                .map_err(|e| e.to_string())?;
            Some(RepairContext {
                operation_id: CommandId::new(),
                request,
                hash: built.payload_sha256,
            })
        };
        let now = Utc::now();
        let entry = RepairEntry {
            intent: RepairIntent {
                actor,
                operation_id,
                task_id,
                base_version: task.stream_version,
                reference,
                target,
                context,
                created_at: now,
            },
            state: RepairState::Pending {
                submission_version: None,
                context_confirmed: false,
                last_error: None,
            },
            updated_at: now,
        };
        // All original input is durable before the first mutation.
        self.write(&entry).await?;
        Ok(entry)
    }

    async fn submission_receipt(
        core: &Arc<Core>,
        intent: &RepairIntent,
        version: Option<u64>,
    ) -> Result<Option<CommandAck>, String> {
        let Some(version) = version else {
            return Ok(None);
        };
        core.command_receipt(&submit_envelope(intent, version))
            .await
            .map(accepted)
            .map_err(|e| e.to_string())
    }

    /// The version the repair turn must be submitted against.
    fn known_version(entry: &RepairEntry) -> Option<u64> {
        match (&entry.state, &entry.intent.context) {
            (_, None) => Some(entry.intent.base_version),
            (
                RepairState::Pending {
                    submission_version, ..
                },
                Some(_),
            ) => *submission_version,
            _ => None,
        }
    }

    async fn drive(&self, core: &Arc<Core>, mut entry: RepairEntry) -> Result<RepairEntry, String> {
        match self.advance(core, &mut entry).await {
            Ok(()) => {
                entry.updated_at = Utc::now();
                self.write(&entry).await?;
                Ok(entry)
            }
            Err(message) => {
                if let RepairState::Pending { last_error, .. } = &mut entry.state {
                    *last_error = Some(message.clone());
                }
                entry.updated_at = Utc::now();
                self.write(&entry).await?;
                Err(message)
            }
        }
    }

    async fn advance(&self, core: &Arc<Core>, entry: &mut RepairEntry) -> Result<(), String> {
        let intent = entry.intent.clone();
        if let Some(ack) =
            Self::submission_receipt(core, &intent, Self::known_version(entry)).await?
        {
            entry.state = RepairState::Submitted {
                acknowledgement: ack,
            };
            return Ok(());
        }
        let version = match (&intent.context, Self::known_version(entry)) {
            (_, Some(version)) => version,
            (None, None) => unreachable!("repairs without context always know their version"),
            (Some(context), None) => {
                let envelope = context_envelope(&intent, context);
                let mut receipt = accepted(
                    core.command_receipt(&envelope)
                        .await
                        .map_err(|e| e.to_string())?,
                );
                if receipt.is_none() {
                    // A lost response may follow successful confirmation. Never rebuild
                    // the package or switch again under a fresh identity.
                    match core.execute(envelope.clone()).await {
                        Ok(ack) => receipt = accepted(Some(ack)),
                        Err(error) => {
                            receipt = accepted(
                                core.command_receipt(&envelope)
                                    .await
                                    .map_err(|e| e.to_string())?,
                            );
                            if receipt.is_none() {
                                return Err(error.to_string());
                            }
                        }
                    }
                }
                let receipt = receipt.ok_or("上下文结果尚未确认，请先核实原操作。")?;
                if let RepairState::Pending {
                    context_confirmed, ..
                } = &mut entry.state
                {
                    *context_confirmed = true;
                }
                let version = Self::after_context(core, &intent, &receipt).await?;
                if let RepairState::Pending {
                    submission_version, ..
                } = &mut entry.state
                {
                    *submission_version = Some(version);
                }
                // The exact post-context version is durable before the submit.
                entry.updated_at = Utc::now();
                self.write(entry).await?;
                version
            }
        };
        let envelope = submit_envelope(&intent, version);
        let ack = match core.execute(envelope).await {
            Ok(ack) => accepted(Some(ack)),
            Err(error) => {
                let receipt = Self::submission_receipt(core, &intent, Some(version)).await?;
                if receipt.is_none() {
                    return Err(error.to_string());
                }
                receipt
            }
        }
        .ok_or("修复任务未被受理，请核实原操作。")?;
        entry.state = RepairState::Submitted {
            acknowledgement: ack,
        };
        Ok(())
    }

    /// Verifies that the original context command switched this task to the
    /// target and returns the resulting version; later changes stop the submit.
    async fn after_context(
        core: &Arc<Core>,
        intent: &RepairIntent,
        receipt: &CommandAck,
    ) -> Result<u64, String> {
        let last = receipt
            .last_sequence
            .filter(|sequence| *sequence > 0)
            .ok_or("上下文回执缺少版本边界，未提交修复。")?;
        let event = core
            .sync_events(last - 1, 1)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let verified = event.filter(|event| {
            event.sequence == last
                && event.stream.stream_type == StreamType::Task
                && event.stream.stream_id == intent.task_id.as_str()
                && event.causation_id.as_deref() == Some(receipt.command_id.as_str())
                && event.event_type == "task.preferred_executor_changed"
                && event.payload.get("taskId").and_then(Value::as_str)
                    == Some(intent.task_id.as_str())
                && event
                    .payload
                    .get("executor")
                    .and_then(|value| serde_json::from_value::<ExecutorRef>(value.clone()).ok())
                    .is_some_and(|executor| executor == intent.target)
                && event.stream_version > intent.base_version
        });
        let event = verified.ok_or("无法核实原上下文的会话版本，未提交修复。")?;
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == intent.task_id)
            .ok_or("原对话已不可用。")?;
        if task.active_primary_run.is_some()
            || task.stream_version != event.stream_version
            || task.preferred_executor != intent.target
        {
            return Err("上下文确认后会话已变化，未用新版本重新提交；请核对原操作。".into());
        }
        Ok(event.stream_version)
    }

    async fn abandon(
        &self,
        core: &Arc<Core>,
        mut entry: RepairEntry,
    ) -> Result<RepairEntry, String> {
        if !entry.pending() {
            return Ok(entry);
        }
        let intent = entry.intent.clone();
        if let Some(version) = Self::known_version(&entry) {
            // Fence the exact original submit so it cannot be accepted later.
            let envelope = submit_envelope(&intent, version);
            match core
                .invalidate_pending_command(&envelope)
                .await
                .map_err(|e| e.to_string())?
            {
                agent_core::command_invalidation::CommandInvalidation::Accepted {
                    acknowledgement,
                } => {
                    entry.state = RepairState::Submitted { acknowledgement };
                }
                agent_core::command_invalidation::CommandInvalidation::Invalidated => {
                    entry.state = RepairState::Abandoned;
                }
            }
        } else {
            entry.state = RepairState::Abandoned;
        }
        entry.updated_at = Utc::now();
        self.write(&entry).await?;
        Ok(entry)
    }
}

#[tauri::command]
pub async fn core_review_repair(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::DesktopState>,
    call: RepairCall,
) -> Result<Value, String> {
    crate::review_window::require_task_window(window.label(), call.task_id().as_str())?;
    state.core.review_repair(call).await
}
