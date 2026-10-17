//! Immutable submission intent and read-only resolution from existing Core receipts.
//! This is not a second task projection; the host service drives the commands.
use crate::{Core, CoreError, command_invalidation::CommandInvalidation};
use agent_domain::{CommandId, TaskId, TurnId};
use agent_protocol::{
    CommandAck, CommandEnvelope, ConfirmContextPackage, ContextStrength, CoreCommand, CreateTask,
    EventEnvelope, StreamType, SubmitTurn,
    submission::{SubmissionIntent, SubmissionResolution},
};
use sha2::{Digest, Sha256};

pub const MAX_INTENT_BYTES: usize = 32 * 1024 * 1024;

/// Progress of the creation/context step that precedes the final message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionStage {
    /// The original step has neither been accepted nor fenced.
    Unresolved,
    /// The original step was fenced and can never be accepted.
    Invalidated,
    /// The final message must be submitted against this task version.
    Ready { version: u64 },
}
fn invalid() -> CoreError {
    CoreError::InvalidArgument("原发送记录无效，请保留记录并核实".into())
}
fn conflict() -> CoreError {
    CoreError::Conflict("原发送回执与事件不一致，未结束恢复记录".into())
}

/// UI checkpoint and display metadata are derived, never part of immutable intent.
/// Original target, content, identities and version fences remain bound.
pub fn normalize(mut intent: SubmissionIntent) -> Result<SubmissionIntent, CoreError> {
    let task = &intent.task;
    if intent.schema_version != 1
        || intent.scope.trim().is_empty()
        || intent.scope.len() > 4096
        || chrono::DateTime::parse_from_rfc3339(&intent.requested_at).is_err()
        || task.active_primary_run.is_some()
        || (intent.input.prompt.trim().is_empty() && intent.input.attachments.is_empty())
        || intent.input.attachments.len() > 16
        || intent.input.attachments.iter().any(|a| {
            a.path.is_empty()
                || a.name.is_empty()
                || a.mime_type.is_empty()
                || a.integrity.as_ref().is_some_and(|i| {
                    i.byte_length > 25 * 1024 * 1024
                        || i.sha256.len() != 64
                        || !i
                            .sha256
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
        })
    {
        return Err(invalid());
    }
    if let Some(creation) = &intent.creation
        && (intent.context.is_some()
            || task.stream_version != 0
            || task.title != "新对话"
            || task.preferred_executor != intent.target
            || creation.operation_id == intent.operation_id)
    {
        return Err(invalid());
    }
    if let Some(context) = &intent.context {
        let request = &context.request;
        if context.operation_id == intent.operation_id
            || context.hash.is_empty()
            || request.task_id != task.task_id
            || request.source_executor != task.preferred_executor
            || request.target_executor != intent.target
            || request.strength != ContextStrength::Standard
            || request.after_sequence > request.through_sequence
        {
            return Err(invalid());
        }
    }
    if let Some(checkpoint) = &intent.submission
        && (checkpoint.task_id != task.task_id
            || checkpoint.project_id != task.project_id
            || checkpoint.active_primary_run.is_some()
            || checkpoint.preferred_executor != intent.target
            || checkpoint.stream_version < task.stream_version
            || (intent.creation.is_some() && checkpoint.stream_version != 1)
            || (intent.context.is_some() && checkpoint.stream_version <= task.stream_version)
            || (intent.creation.is_none()
                && intent.context.is_none()
                && checkpoint.stream_version != task.stream_version))
    {
        return Err(invalid());
    }
    if intent.creation.is_none()
        && intent.context.is_none()
        && (intent.submission.is_none() || task.preferred_executor != intent.target)
    {
        return Err(invalid());
    }
    intent.task.updated_at = None;
    intent.task.last_turn_executor = None;
    intent.submission = if intent.creation.is_none() && intent.context.is_none() {
        Some(intent.task.clone())
    } else {
        None
    };
    if serde_json::to_vec(&intent).map_err(|_| invalid())?.len() > MAX_INTENT_BYTES {
        return Err(invalid());
    }
    Ok(intent)
}

pub fn fingerprint(intent: &SubmissionIntent) -> Result<String, CoreError> {
    let normalized = normalize(intent.clone())?;
    let bytes = serde_json::to_vec(&normalized).map_err(|_| invalid())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn envelope(id: &CommandId, version: u64, payload: CoreCommand) -> CommandEnvelope<CoreCommand> {
    let mut envelope = CommandEnvelope::local(payload);
    envelope.command_id = id.clone();
    envelope.idempotency_key = id.to_string();
    envelope.expected_stream_version = Some(version);
    envelope
}
#[must_use]
pub fn stage_command(intent: &SubmissionIntent) -> Option<CommandEnvelope<CoreCommand>> {
    if let Some(creation) = &intent.creation {
        Some(envelope(
            &creation.operation_id,
            0,
            CoreCommand::CreateTask(CreateTask {
                task_id: intent.task.task_id.clone(),
                project_id: intent.task.project_id.clone(),
                title: intent.task.title.clone(),
                preferred_executor: intent.target.clone(),
            }),
        ))
    } else {
        intent.context.as_ref().map(|context| {
            envelope(
                &context.operation_id,
                intent.task.stream_version,
                CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                    request: context.request.clone(),
                    expected_payload_sha256: context.hash.clone(),
                    edited_package: None,
                }),
            )
        })
    }
}
#[must_use]
pub fn submission_command(intent: &SubmissionIntent, version: u64) -> CommandEnvelope<CoreCommand> {
    envelope(
        &intent.operation_id,
        version,
        CoreCommand::SubmitTurn(SubmitTurn {
            task_id: intent.task.task_id.clone(),
            prompt: intent.input.prompt.clone(),
            model: intent.input.model.clone(),
            thinking: intent.input.thinking.clone(),
            permission_mode: intent.input.permission_mode.clone(),
            attachments: intent.input.attachments.clone(),
            executor_override: Some(intent.target.clone()),
            review_fix: None,
        }),
    )
}
impl Core {
    /// The permanent tombstone survives purging task events. Absence of a task
    /// projection or receipt alone is never deletion evidence.
    pub async fn submission_task_deletion(
        &self,
        task: &TaskId,
    ) -> Result<Option<SubmissionResolution>, CoreError> {
        let head = self.repository.event_head().await?;
        let mut cursor = 0;
        loop {
            let events = self
                .repository
                .task_events_of_types(task, cursor, head, &["task.deleted"], 100)
                .await?;
            if events.is_empty() {
                return Ok(None);
            }
            for event in &events {
                if event.event_type != "task.deleted"
                    || event.sequence <= cursor
                    || event.sequence > head
                    || event.stream.stream_type != StreamType::Task
                    || event.stream.stream_id != task.as_str()
                    || event.payload["taskId"].as_str() != Some(task.as_str())
                {
                    return Err(conflict());
                }
                cursor = event.sequence;
                if event.payload["permanent"] == true {
                    if event.stream_version == 0 {
                        return Err(conflict());
                    }
                    return Ok(Some(SubmissionResolution::TaskDeleted {
                        task_id: task.clone(),
                        deletion_sequence: event.sequence,
                    }));
                }
            }
        }
    }

    async fn submission_receipt_event(
        &self,
        intent: &SubmissionIntent,
        ack: &CommandAck,
    ) -> Result<EventEnvelope, CoreError> {
        let last = ack
            .last_sequence
            .filter(|s| *s > 0 && ack.accepted)
            .ok_or_else(conflict)?;
        let event = self
            .sync_events(last - 1, 1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(conflict)?;
        if event.sequence != last
            || event.stream.stream_type != StreamType::Task
            || event.stream.stream_id != intent.task.task_id.as_str()
            || event.causation_id.as_deref() != Some(ack.command_id.as_str())
            || event.payload["taskId"].as_str() != Some(intent.task.task_id.as_str())
        {
            return Err(conflict());
        }
        Ok(event)
    }
    /// The task version the final message must use, proven only by the original
    /// creation/context event. Read only; never executes or fences anything.
    pub async fn submission_stage(
        &self,
        intent: &SubmissionIntent,
    ) -> Result<SubmissionStage, CoreError> {
        let intent = normalize(intent.clone())?;
        Ok(if let Some(command) = stage_command(&intent) {
            match self.pending_command_invalidation_receipt(&command).await? {
                None => SubmissionStage::Unresolved,
                Some(CommandInvalidation::Invalidated) => SubmissionStage::Invalidated,
                Some(CommandInvalidation::Accepted { acknowledgement }) => {
                    let event = self
                        .submission_receipt_event(&intent, &acknowledgement)
                        .await?;
                    let field = if intent.creation.is_some() {
                        "preferredExecutor"
                    } else {
                        "executor"
                    };
                    if event.payload[field]
                        != serde_json::to_value(&intent.target).map_err(|_| conflict())?
                        || (intent.creation.is_some()
                            && (event.event_type != "task.created"
                                || event.stream_version != 1
                                || event.payload["projectId"].as_str()
                                    != Some(intent.task.project_id.as_str())
                                || event.payload["title"].as_str()
                                    != Some(intent.task.title.as_str())))
                        || (intent.context.is_some()
                            && (event.event_type != "task.preferred_executor_changed"
                                || event.stream_version <= intent.task.stream_version))
                    {
                        return Err(conflict());
                    }
                    SubmissionStage::Ready {
                        version: event.stream_version,
                    }
                }
            }
        } else {
            SubmissionStage::Ready {
                version: intent.task.stream_version,
            }
        })
    }
    /// A missing receipt means unresolved, never permission to close or replay.
    /// Checkpoints and current bootstrap versions cannot replace the original event fence.
    pub async fn submission_intent_resolution(
        &self,
        intent: &SubmissionIntent,
    ) -> Result<Option<SubmissionResolution>, CoreError> {
        let intent = normalize(intent.clone())?;
        if let Some(deleted) = self.submission_task_deletion(&intent.task.task_id).await? {
            return Ok(Some(deleted));
        }
        let invalidated = || {
            Some(SubmissionResolution::Invalidated {
                task_id: intent.task.task_id.clone(),
            })
        };
        let version = match self.submission_stage(&intent).await? {
            SubmissionStage::Unresolved => return Ok(None),
            SubmissionStage::Invalidated => return Ok(invalidated()),
            SubmissionStage::Ready { version } => version,
        };
        match self
            .pending_command_invalidation_receipt(&submission_command(&intent, version))
            .await?
        {
            None => Ok(None),
            Some(CommandInvalidation::Invalidated) => Ok(invalidated()),
            Some(CommandInvalidation::Accepted { acknowledgement }) => {
                let event = self
                    .submission_receipt_event(&intent, &acknowledgement)
                    .await?;
                if acknowledgement.first_sequence != acknowledgement.last_sequence
                    || event.event_type != "turn.queued"
                    || version.checked_add(1) != Some(event.stream_version)
                {
                    return Err(conflict());
                }
                let turn_id = TurnId::parse(event.payload["turnId"].as_str().ok_or_else(conflict)?)
                    .map_err(|_| conflict())?;
                Ok(Some(SubmissionResolution::Submitted {
                    task_id: intent.task.task_id,
                    sequence: event.sequence,
                    turn_id,
                }))
            }
        }
    }
}
