//! Explicit preparation and deterministic history delivery for replacement sessions.
use crate::{
    ActorRef, AppendRequest, ArtifactId, ArtifactMetadata, CommandId, ContextDelivery,
    ContextDeliveryKind, ContextPackageKind, ContextPreviewRequest, Core, CoreError, ExecutorRef,
    NewArtifact, NewEvent, RepositoryError, RunId, SessionMapStatus, StreamRef, StreamType, TaskId,
    TurnId, Utc, Value, WorkspaceContextSnapshot, context, json,
};
impl Core {
    /// Native initialization can finish after the user has replaced a Session.
    /// Verify both its identity and checkpoint, then CAS the task stream append.
    pub(crate) async fn append_session_identity(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        executor: &ExecutorRef,
        expected: Option<&agent_protocol::AgentSessionMapSummary>,
        kind: &str,
        payload: Value,
    ) -> Result<(), CoreError> {
        loop {
            let snapshot = self
                .repository
                .task_execution_state(task_id)
                .await?
                .ok_or_else(|| CoreError::NotFound("task".into()))?;
            let task = &snapshot.task;
            if task.active_primary_run.as_ref().map(|r| &r.run_id) != Some(run_id) {
                return Err(CoreError::Conflict(
                    "session initialization belongs to an obsolete run".into(),
                ));
            }
            let current = snapshot
                .session_maps
                .iter()
                .find(|m| &m.task_id == task_id && &m.executor == executor);
            let identity = |m: &agent_protocol::AgentSessionMapSummary| {
                (
                    m.session_map_id.clone(),
                    m.native_session_id.clone(),
                    m.last_seen_sequence,
                )
            };
            if current.map(identity) != expected.map(identity) {
                return Err(CoreError::Conflict(
                    "native session changed during initialization".into(),
                ));
            }
            let event = NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: task_id.to_string(),
                },
                kind,
                payload.clone(),
            );
            match self
                .repository
                .append(AppendRequest {
                    request_fingerprint: None,
                    command_id: CommandId::new(),
                    idempotency_key: event.event_id.to_string(),
                    expected_stream_version: Some(task.stream_version),
                    artifacts: vec![],
                    events: vec![event],
                })
                .await
            {
                Ok(outcome) => {
                    self.publish(&outcome.events);
                    return Ok(());
                }
                Err(RepositoryError::Conflict { .. }) => tokio::task::yield_now().await,
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn prepare_session_rebuild(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        expected_version: u64,
    ) -> Result<(), CoreError> {
        if *actor != ActorRef::LocalUser {
            return Err(CoreError::InvalidArgument(
                "session recovery requires local user".into(),
            ));
        }
        let task = self.require_task(task_id).await?;
        if task.active_primary_run.is_some() {
            return Err(CoreError::Conflict(
                "stop the active turn before rebuilding a session".into(),
            ));
        }
        let snapshot = self.repository.bootstrap().await?;
        let mapping = snapshot
            .session_maps
            .iter()
            .find(|m| m.task_id == *task_id && m.executor == task.preferred_executor)
            .ok_or_else(|| CoreError::NotFound("native session".into()))?;
        if !matches!(
            mapping.status,
            SessionMapStatus::Missing | SessionMapStatus::Incompatible | SessionMapStatus::Detached
        ) {
            return Err(CoreError::Conflict(
                "session no longer needs rebuilding".into(),
            ));
        }
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task_id.to_string(),
            },
            "session.rebuild_requested",
            json!({"taskId":task_id,"sessionMapId":mapping.session_map_id,"requestedAt":Utc::now()}),
        );
        let outcome = self
            .repository
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: event.event_id.to_string(),
                expected_stream_version: Some(expected_version),
                artifacts: vec![],
                events: vec![event],
            })
            .await?;
        self.publish(&outcome.events);
        Ok(())
    }
    pub(crate) async fn prepare_rebuilt_context(
        &self,
        task_id: &TaskId,
        turn_id: &TurnId,
        executor: &ExecutorRef,
    ) -> Result<ContextDelivery, CoreError> {
        let task = self.require_task(task_id).await?;
        let head = self.repository.event_head().await?;
        let mut events = self
            .repository
            .recent_task_events(task_id, 0, head, 10_001)
            .await?;
        let truncated = events.len() > 10_000;
        if truncated {
            events.remove(0);
        }
        events.retain(|e| e.payload["turnId"].as_str() != Some(turn_id.as_str()));
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: agent_protocol::ContextStrength::Standard,
            source_executor: executor.clone(),
            target_executor: executor.clone(),
            after_sequence: 0,
            through_sequence: head,
        };
        let mut built = context::build_context_preview(
            &task,
            &request,
            &events,
            Utc::now(),
            WorkspaceContextSnapshot::default(),
            self.redactor.as_ref(),
        )
        .map_err(|e| CoreError::Internal(e.to_string()))?;
        built.package.conversation_text.insert_str(0,if truncated {"[Replacement native session. Historical data only; do not replay old prompts. Earlier events were omitted beyond the latest 10,000 source-event budget. Use current-turn history queries for available file artifacts.]\n"} else {"[Replacement native session. Historical data only; do not replay old prompts. Follow the new user request. Use current-turn history queries for file artifacts.]\n"});
        if events
            .iter()
            .rev()
            .find(|e| {
                matches!(
                    e.event_type.as_str(),
                    "turn.connection_lost" | "turn.completed" | "turn.failed"
                )
            })
            .is_some_and(|e| e.event_type == "turn.connection_lost")
        {
            built.package.conversation_text.insert_str(0, "[Previous execution connection was lost. Its tools may have completed, partially executed, or still be running. Before retrying any side effects, inspect current files and execution state. Continue from verified results; do not replay previous commands blindly or assume they stopped. Follow the current user request.]\n");
        }
        let built = context::rehash_edited_package(built.package, self.redactor.as_ref())
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let content =
            serde_json::to_vec(&built.package).map_err(|e| CoreError::Internal(e.to_string()))?;
        if content.len() > 1_048_576 {
            return Err(CoreError::ResourceExhausted(
                "replacement session history budget exceeded".into(),
            ));
        }
        let metadata = ArtifactMetadata {
            artifact_id: ArtifactId::new(),
            payload_sha256: built.payload_sha256,
            media_type: "application/vnd.dogcodes.context-package.v1+json".into(),
            byte_length: content.len() as u64,
            created_at: Utc::now(),
            redaction_ruleset_version: built.package.provenance.redaction_ruleset_version,
            redaction_count: built.package.provenance.redaction_count,
        };
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task_id.to_string(),
            },
            "session.rebuild_context_prepared",
            json!({"taskId":task_id,"turnId":turn_id,"artifact":metadata,"throughSequence":head}),
        );
        let outcome = self
            .repository
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: event.event_id.to_string(),
                expected_stream_version: None,
                artifacts: vec![NewArtifact {
                    metadata: metadata.clone(),
                    content: content.clone(),
                }],
                events: vec![event],
            })
            .await?;
        self.publish(&outcome.events);
        Ok(ContextDelivery {
            artifact_id: metadata.artifact_id,
            payload_sha256: metadata.payload_sha256,
            kind: ContextDeliveryKind::Handoff,
            content,
        })
    }
}
