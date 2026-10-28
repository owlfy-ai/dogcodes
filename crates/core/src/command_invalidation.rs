//! Explicitly retire a frozen, unaccepted task command without replaying it.
use crate::{AppendRequest, Core, CoreError, RepositoryError, command_identity};
use agent_domain::CommandId;
use agent_protocol::{
    ActorRef, CommandAck, CommandEnvelope, CoreCommand, NewEvent, StreamRef, StreamType,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandInvalidation {
    Accepted { acknowledgement: CommandAck },
    Invalidated,
}

impl Core {
    /// Read-only evidence for finishing a persisted submission workflow. Never
    /// creates a barrier merely because a receipt is missing.
    pub async fn pending_command_invalidation_receipt(
        &self,
        envelope: &CommandEnvelope<CoreCommand>,
    ) -> Result<Option<CommandInvalidation>, CoreError> {
        if !matches!(
            envelope.actor,
            ActorRef::LocalUser | ActorRef::PairedDevice(_)
        ) || envelope.expected_stream_version.is_none()
            || !matches!(
                envelope.payload,
                CoreCommand::CreateTask(_)
                    | CoreCommand::SubmitTurn(_)
                    | CoreCommand::ConfirmContextPackage(_)
            )
            || (matches!(envelope.payload, CoreCommand::CreateTask(_))
                && envelope.expected_stream_version != Some(0))
        {
            return Err(CoreError::InvalidArgument(
                "invalid submission recovery query".into(),
            ));
        }
        if let Some(acknowledgement) = self.command_receipt(envelope).await? {
            return Ok(Some(CommandInvalidation::Accepted { acknowledgement }));
        }
        let (task_id, command_type) = match &envelope.payload {
            CoreCommand::CreateTask(c) => (&c.task_id, "create_task"),
            CoreCommand::SubmitTurn(c) => (&c.task_id, "submit_turn"),
            CoreCommand::ConfirmContextPackage(c) => {
                (&c.request.task_id, "confirm_context_package")
            }
            _ => unreachable!("validated above"),
        };
        let fingerprint = command_identity::fingerprint(envelope)?;
        let Some(ack) = self
            .repository
            .command_ack(&format!("command-invalidate:{}", envelope.idempotency_key))
            .await?
        else {
            return Ok(None);
        };
        command_identity::verify(&ack, &fingerprint)?;
        let sequence = ack
            .last_sequence
            .filter(|sequence| {
                ack.accepted && *sequence > 0 && ack.first_sequence == Some(*sequence)
            })
            .ok_or_else(|| {
                CoreError::Conflict("invalidation receipt has no event boundary".into())
            })?;
        let events = self.sync_events(sequence - 1, 1).await?;
        let event = events.first().filter(|event| {
            event.sequence == sequence
                && event.event_type == "task.command_invalidated"
                && event.stream.stream_type == StreamType::Task
                && event.stream.stream_id == task_id.as_str()
                && event.payload["taskId"].as_str() == Some(task_id.as_str())
                && event.payload["originalCommandType"].as_str() == Some(command_type)
                && event.payload["originalVersion"].as_u64() == envelope.expected_stream_version
                && event.causation_id.as_deref() == Some(ack.command_id.as_str())
                && event.payload["originalOperationId"].as_str()
                    == Some(envelope.idempotency_key.as_str())
                && event.payload["requestFingerprint"].as_str() == Some(fingerprint.as_str())
        });
        if event.is_none() {
            return Err(CoreError::Conflict(
                "invalidation evidence does not match original command".into(),
            ));
        }
        Ok(Some(match self.command_receipt(envelope).await? {
            Some(acknowledgement) => CommandInvalidation::Accepted { acknowledgement },
            None => CommandInvalidation::Invalidated,
        }))
    }

    pub async fn invalidate_pending_command(
        &self,
        envelope: &CommandEnvelope<CoreCommand>,
    ) -> Result<CommandInvalidation, CoreError> {
        let (task_id, may_be_absent, command_type) = match &envelope.payload {
            CoreCommand::CreateTask(command) => (&command.task_id, true, "create_task"),
            CoreCommand::SubmitTurn(command) => (&command.task_id, false, "submit_turn"),
            CoreCommand::ConfirmContextPackage(command) => {
                (&command.request.task_id, false, "confirm_context_package")
            }
            _ => {
                return Err(CoreError::InvalidArgument(
                    "this command cannot be retired through task submission recovery".into(),
                ));
            }
        };
        if !matches!(
            envelope.actor,
            ActorRef::LocalUser | ActorRef::PairedDevice(_)
        ) {
            return Err(CoreError::InvalidArgument(
                "only an authenticated user may retire a task submission".into(),
            ));
        }
        let original_version = envelope
            .expected_stream_version
            .ok_or_else(|| CoreError::InvalidArgument("original version is required".into()))?;
        if may_be_absent && original_version != 0 {
            return Err(CoreError::InvalidArgument(
                "task creation must use original version zero".into(),
            ));
        }
        if let Some(acknowledgement) = self.command_receipt(envelope).await? {
            return Ok(CommandInvalidation::Accepted { acknowledgement });
        }
        let fingerprint = command_identity::fingerprint(envelope)?;
        let key = format!("command-invalidate:{}", envelope.idempotency_key);
        if let Some(ack) = self.repository.command_ack(&key).await? {
            command_identity::verify(&ack, &fingerprint)?;
            return Ok(match self.command_receipt(envelope).await? {
                Some(acknowledgement) => CommandInvalidation::Accepted { acknowledgement },
                None => CommandInvalidation::Invalidated,
            });
        }
        let version = match self.repository.task_summary(task_id).await? {
            Some(task) => task.stream_version,
            None if may_be_absent => 0,
            None => return Err(CoreError::NotFound("原会话已不可用，请核实原操作".into())),
        };
        if version < original_version {
            return Err(CoreError::Conflict("后台版本早于原请求，未作废操作".into()));
        }
        let command_id = CommandId::new();
        let mut event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task_id.to_string(),
            },
            "task.command_invalidated",
            serde_json::json!({"taskId":task_id,"originalCommandType":command_type,
                "originalOperationId":envelope.idempotency_key,"originalVersion":original_version,"requestFingerprint":fingerprint}),
        );
        event.causation_id = Some(command_id.to_string());
        // An uncreated task has no projection. Its stream barrier still prevents
        // the original version-zero creation from arriving late and taking effect.
        match self
            .repository
            .append(AppendRequest {
                command_id,
                idempotency_key: key,
                request_fingerprint: Some(fingerprint),
                expected_stream_version: Some(version),
                artifacts: vec![],
                events: vec![event],
            })
            .await
        {
            Ok(outcome) => self.publish(&outcome.events),
            Err(RepositoryError::Conflict { .. }) => {
                return match self.command_receipt(envelope).await? {
                    Some(acknowledgement) => Ok(CommandInvalidation::Accepted { acknowledgement }),
                    None => Err(CoreError::Conflict(
                        "会话状态已变化，请先核实原操作结果".into(),
                    )),
                };
            }
            Err(error) => return Err(error.into()),
        }
        Ok(match self.command_receipt(envelope).await? {
            Some(acknowledgement) => CommandInvalidation::Accepted { acknowledgement },
            None => CommandInvalidation::Invalidated,
        })
    }
}
