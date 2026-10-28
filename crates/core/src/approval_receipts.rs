//! Approval recovery and explicit version invalidation without another answer copy.
use crate::{AppendRequest, Core, CoreError, RepositoryError, command_identity};
use agent_domain::{CommandId, TaskId, TurnId};
use agent_protocol::{
    ActorRef, CommandAck, CommandEnvelope, CoreCommand, NewEvent, SCHEMA_VERSION, StreamRef,
    StreamType,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalReceiptQuery {
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub approval_id: String,
    pub requested_at_sequence: u64,
    pub operation_id: CommandId,
    pub request_fingerprint: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDelivery {
    Unknown,
    Sent,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalReceipt {
    pub acknowledgement: CommandAck,
    pub delivery: ApprovalDelivery,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalInvalidation {
    Accepted { receipt: ApprovalReceipt },
    Invalidated,
}

impl Core {
    /// Explicitly retire the ORIGINAL version-bound response, never its approval
    /// request. A competing acceptance wins and is returned instead of withdrawn.
    pub async fn invalidate_approval_response(
        &self,
        query: &ApprovalReceiptQuery,
        original_version: u64,
    ) -> Result<ApprovalInvalidation, CoreError> {
        use sha2::{Digest, Sha256};
        if let Some(receipt) = self.approval_receipt(query).await? {
            return Ok(ApprovalInvalidation::Accepted { receipt });
        }
        let key = format!("approval-invalidate:{}", query.operation_id);
        let identity = format!(
            "approval-invalidate:v1:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&(query, original_version))
                    .map_err(|e| CoreError::Internal(e.to_string()))?
            ))
        );
        if let Some(ack) = self.repository.command_ack(&key).await? {
            command_identity::verify(&ack, &identity)?;
            // A previous successful barrier is durable. Do not increment a newer
            // task version when the reply to that same invalidation was lost.
            return Ok(match self.approval_receipt(query).await? {
                Some(receipt) => ApprovalInvalidation::Accepted { receipt },
                None => ApprovalInvalidation::Invalidated,
            });
        }
        let task = self.require_task(&query.task_id).await?;
        if task.stream_version < original_version {
            return Err(CoreError::Conflict(
                "后台版本早于原审批版本，无法安全作废".into(),
            ));
        }
        // Fence the observed CURRENT version, never rely on a caller-supplied
        // lower version as proof. Every previously observed original version is
        // then obsolete. Stable identity makes retries of this barrier harmless.
        let command_id = CommandId::new();
        let mut event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: query.task_id.to_string(),
            },
            "approval.response_invalidated",
            serde_json::json!({"taskId":query.task_id,"turnId":query.turn_id,
                "approvalId":query.approval_id,"requestedAtSequence":query.requested_at_sequence,
                "operationId":query.operation_id,"requestFingerprint":query.request_fingerprint,"originalVersion":original_version}),
        );
        event.causation_id = Some(command_id.to_string());
        let outcome = self
            .repository
            .append(AppendRequest {
                command_id,
                idempotency_key: key,
                request_fingerprint: Some(identity),
                expected_stream_version: Some(task.stream_version),
                artifacts: vec![],
                events: vec![event],
            })
            .await;
        match outcome {
            Ok(outcome) => self.publish(&outcome.events),
            Err(RepositoryError::Conflict { .. }) => {
                return match self.approval_receipt(query).await? {
                    Some(receipt) => Ok(ApprovalInvalidation::Accepted { receipt }),
                    None => Err(CoreError::Conflict(
                        "会话状态已变化，请核实原审批结果后再修改决定".into(),
                    )),
                };
            }
            Err(error) => return Err(error.into()),
        }
        Ok(match self.approval_receipt(query).await? {
            Some(receipt) => ApprovalInvalidation::Accepted { receipt },
            None => ApprovalInvalidation::Invalidated,
        })
    }

    /// Computes the same typed-command identity as execute, without admitting,
    /// persisting, or delivering it. The returned query contains no answer text.
    pub fn prepare_approval_response(
        &self,
        envelope: &CommandEnvelope<CoreCommand>,
    ) -> Result<ApprovalReceiptQuery, CoreError> {
        if envelope.schema_version != SCHEMA_VERSION {
            return Err(CoreError::UnsupportedSchema(envelope.schema_version));
        }
        let CoreCommand::RespondToApproval(command) = &envelope.payload else {
            return Err(CoreError::InvalidArgument(
                "expected an approval response".into(),
            ));
        };
        if !matches!(
            envelope.actor,
            ActorRef::LocalUser | ActorRef::PairedDevice(_)
        ) || envelope.expected_stream_version.is_none()
            || command.approval_id.trim().is_empty()
            || command.requested_at_sequence == 0
        {
            return Err(CoreError::InvalidArgument(
                "approval source, authenticated actor and original version are required".into(),
            ));
        }
        let operation_id = CommandId::parse(envelope.idempotency_key.clone()).map_err(|_| {
            CoreError::InvalidArgument("approval operationId must be a command ID".into())
        })?;
        Ok(ApprovalReceiptQuery {
            task_id: command.task_id.clone(),
            turn_id: command.turn_id.clone(),
            approval_id: command.approval_id.clone(),
            requested_at_sequence: command.requested_at_sequence,
            operation_id,
            request_fingerprint: command_identity::fingerprint(envelope)?,
        })
    }

    pub async fn approval_receipt(
        &self,
        query: &ApprovalReceiptQuery,
    ) -> Result<Option<ApprovalReceipt>, CoreError> {
        let Some(ack) = self
            .repository
            .command_ack(query.operation_id.as_str())
            .await?
        else {
            return Ok(None);
        };
        command_identity::verify(&ack, &query.request_fingerprint)?;
        let sequence = ack
            .first_sequence
            .filter(|value| *value > 0 && Some(*value) == ack.last_sequence)
            .ok_or_else(|| CoreError::Conflict("审批回执的事件范围无法核实".into()))?;
        let events = self
            .repository
            .task_events(&query.task_id, sequence - 1, sequence, 1)
            .await?;
        let event = events.first().ok_or_else(|| {
            CoreError::NotFound("原审批已受理，但记录已不可用；未重复发送".into())
        })?;
        if event.sequence != sequence
            || event.event_type != "approval.resolved"
            || event.stream.stream_id != query.task_id.as_str()
            || event.causation_id.as_deref() != Some(ack.command_id.as_str())
            || event.payload["taskId"].as_str() != Some(query.task_id.as_str())
            || event.payload["turnId"].as_str() != Some(query.turn_id.as_str())
            || event.payload["approvalId"].as_str() != Some(&query.approval_id)
            || event.payload["requestedAtSequence"].as_u64() != Some(query.requested_at_sequence)
        {
            return Err(CoreError::Conflict(
                "审批回执不属于原请求，未重复发送".into(),
            ));
        }
        let through = self.repository.event_head().await?;
        let mut after = sequence;
        let mut delivery = None;
        loop {
            let page = self
                .repository
                .task_events_of_types(
                    &query.task_id,
                    after,
                    through,
                    &["approval.delivery_sent", "approval.delivery_failed"],
                    256,
                )
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.sequence;
            for event in page {
                if event.payload["commandId"].as_str() != Some(ack.command_id.as_str()) {
                    continue;
                }
                if event.payload["turnId"].as_str() != Some(query.turn_id.as_str())
                    || event.payload["approvalId"].as_str() != Some(&query.approval_id)
                    || event.payload["requestedAtSequence"].as_u64()
                        != Some(query.requested_at_sequence)
                {
                    return Err(CoreError::Conflict("审批投递记录的归属无法核实".into()));
                }
                let status = if event.event_type == "approval.delivery_sent" {
                    ApprovalDelivery::Sent
                } else {
                    ApprovalDelivery::Failed
                };
                if delivery.is_some_and(|old| old != status) {
                    return Err(CoreError::Conflict(
                        "审批投递记录存在冲突，请检查原任务".into(),
                    ));
                }
                delivery = Some(status);
            }
        }
        Ok(Some(ApprovalReceipt {
            acknowledgement: ack,
            delivery: delivery.unwrap_or(ApprovalDelivery::Unknown),
        }))
    }
}
