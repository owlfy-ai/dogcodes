use super::{AppendRequest, Core, CoreError, NewEvent, require_non_empty};
use agent_domain::RunState;
use agent_protocol::SteerTurn;
use agent_protocol::{
    CommandAck, CommandEnvelope, CoreCommand, EventSource, StreamRef, StreamType,
};
use serde_json::json;
use std::sync::Arc;

impl Core {
    pub(super) async fn execute_steer(
        self: &Arc<Self>,
        envelope: &CommandEnvelope<CoreCommand>,
        command: &SteerTurn,
        fingerprint: String,
    ) -> Result<CommandAck, CoreError> {
        require_non_empty("text", &command.text)?;
        let control = self
            .owned_run_control(&command.run_id)
            .ok_or_else(|| CoreError::Conflict("backendText.m3ff23090".into()))?;
        // Order durable requests and native writes together. Cancel never takes this lock.
        let order = control.steering_order.clone().lock_owned().await;
        if let Some(ack) = self.command_receipt(envelope).await? {
            return Ok(ack);
        }
        let task = self.require_task(&command.task_id).await?;
        let Some(run) = task.active_primary_run.filter(|run| {
            run.run_id == command.run_id
                && run.turn_id == command.turn_id
                && run.state == RunState::Running
        }) else {
            return Err(CoreError::Conflict("backendText.mae76c0bb".into()));
        };
        if !control.ready_for_followup().await {
            return Err(CoreError::Conflict("backendText.mae76c0bb".into()));
        }
        let sender = control.steering.get().cloned();
        let message_id = envelope.command_id.to_string();
        let payload = json!({"taskId":command.task_id,"turnId":command.turn_id,"runId":command.run_id,"messageId":message_id,"executor":run.executor});
        let stream = StreamRef {
            stream_type: StreamType::Task,
            stream_id: command.task_id.to_string(),
        };
        let mut request = NewEvent::hub(
            stream,
            if sender.is_some() {
                "turn.steer_requested"
            } else {
                "turn.followup_queued"
            },
            payload.clone(),
        );
        request.payload["text"] = json!(command.text);
        if sender.is_none() {
            request.payload["actor"] = json!(envelope.actor);
            request.payload["settings"] =
                json!(control.followup_settings.get().cloned().unwrap_or_default());
        }
        request.causation_id = Some(message_id.clone());
        let outcome = self
            .repository
            .append(AppendRequest {
                request_fingerprint: Some(fingerprint),
                command_id: envelope.command_id.clone(),
                idempotency_key: envelope.idempotency_key.clone(),
                expected_stream_version: envelope.expected_stream_version,
                artifacts: vec![],
                events: vec![request],
            })
            .await?;
        if !outcome.duplicate {
            if sender.is_some() {
                control
                    .steering_boundary
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            self.publish(&outcome.events);
            let Some(sender) = sender else {
                self.schedule_followups(command.task_id.clone());
                return Ok(outcome.ack);
            };
            let core = self.clone();
            let task = command.task_id.clone();
            let text = command.text.clone();
            tokio::spawn(async move {
                let _order = order;
                let result = if control.stopping() || !control.accepts_interactions() {
                    Ok(driver_api::SteerDelivery::Rejected)
                } else {
                    sender.send(&message_id, &text).await
                };
                let kind = match result {
                    Ok(driver_api::SteerDelivery::Accepted) => "turn.steer_accepted",
                    Ok(driver_api::SteerDelivery::Rejected) => "turn.steer_rejected",
                    // An ambiguous native write stays unconfirmed, including across a restart.
                    Err(_) => return,
                };
                let mut delivery = payload;
                if kind == "turn.steer_accepted" {
                    delivery["prompt"] = json!(text);
                }
                let _ = core
                    .append_runtime_event(&task, kind, delivery, EventSource::Hub)
                    .await;
            });
        }
        Ok(outcome.ack)
    }
}
