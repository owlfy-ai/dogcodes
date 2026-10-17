//! Native root continuations have their own persisted run, without a user prompt.
use crate::{
    AgentDriver, AppendRequest, CommandId, Core, CoreError, DriverEvent, EventSource, NewEvent,
    RepositoryError, RunId, SessionHandle, SessionMapId, SessionMapStatus, StreamExt, StreamRef,
    StreamType, TaskId, TurnId, Utc, map_driver_event, recovery, text_segments::TextSegments,
};
use driver_api::DriverEventStream;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

struct AutonomousSession {
    task: TaskId,
    mapping: SessionMapId,
    handle: SessionHandle,
    driver: Arc<dyn AgentDriver>,
}

impl Core {
    pub(crate) fn observe_autonomous(
        self: &Arc<Self>,
        mut stream: DriverEventStream,
        task: TaskId,
        mapping: SessionMapId,
        session: SessionHandle,
        driver: Arc<dyn AgentDriver>,
    ) {
        let identity = AutonomousSession {
            task,
            mapping,
            handle: session,
            driver,
        };
        let core = Arc::clone(self);
        tokio::spawn(async move {
            let mut background_owners = BTreeMap::new();
            while let Some(event) = stream.next().await {
                let Ok(DriverEvent::TurnStarted { native_turn_id }) = event else {
                    // Detached child tools keep their originating turn after the root ends.
                    if let Ok(event) = event {
                        let _ = core
                            .autonomous_child_event(&identity, &background_owners, event)
                            .await;
                    }
                    continue;
                };
                let outcome = core
                    .consume_autonomous(
                        &mut stream,
                        &identity,
                        native_turn_id,
                        &mut background_owners,
                    )
                    .await;
                identity
                    .driver
                    .acknowledge_autonomous_turn(&identity.handle.native_session_id)
                    .await;
                if outcome.is_err() {
                    // A broken consumer cannot leave native work silently reusable.
                    let _ = identity.driver.interrupt(&identity.handle).await;
                    let _ = identity.driver.shutdown(identity.handle.clone()).await;
                    break;
                }
            }
        });
    }

    async fn admit_autonomous(
        &self,
        identity: &AutonomousSession,
        native_turn: Option<String>,
    ) -> Result<(TurnId, recovery::RunOwnership), CoreError> {
        let AutonomousSession {
            task,
            mapping,
            handle: session,
            driver,
        } = identity;
        let mut notifications = self.events.subscribe();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let turn = TurnId::new();
        let run = RunId::new();
        loop {
            let state = self
                .repository
                .task_execution_state(task)
                .await?
                .ok_or_else(|| CoreError::NotFound("autonomous task disappeared".into()))?;
            if !state.session_maps.iter().any(|m| {
                m.session_map_id == *mapping
                    && m.native_session_id == session.native_session_id
                    && m.executor == *driver.executor()
                    && m.status == SessionMapStatus::Active
            }) {
                return Err(CoreError::Conflict(
                    "autonomous session was replaced".into(),
                ));
            }
            if state.task.active_primary_run.is_some() {
                // Native result precedes the originating run's workspace capture/commit.
                // Never overwrite that run, or a user request that won admission.
                tokio::time::timeout_at(deadline, notifications.recv())
                    .await
                    .map_err(|_| CoreError::Conflict("previous run has not settled".into()))?
                    .ok();
                continue;
            }
            let payload = json!({"taskId":task,"turnId":turn,"runId":run,
                "executor":driver.executor(),"nativeTurnId":native_turn,"startedAt":Utc::now()});
            let events = ["turn.autonomous.started", "driver.turn_started"].map(|kind| {
                let mut event = NewEvent::hub(
                    StreamRef {
                        stream_type: StreamType::Task,
                        stream_id: task.to_string(),
                    },
                    kind,
                    payload.clone(),
                );
                event.source = EventSource::Driver {
                    provider_id: driver.executor().provider_id.to_string(),
                    source_sequence: None,
                };
                event
            });
            let ownership = self.own_run(run.clone(), Arc::clone(driver));
            match self
                .repository
                .append(AppendRequest {
                    request_fingerprint: None,
                    command_id: CommandId::new(),
                    idempotency_key: format!("autonomous-{run}"),
                    expected_stream_version: Some(state.task.stream_version),
                    artifacts: vec![],
                    events: events.into(),
                })
                .await
            {
                Ok(outcome) => {
                    self.publish(&outcome.events);
                    return Ok((turn, ownership));
                }
                Err(RepositoryError::Conflict { .. }) => {
                    drop(ownership);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    async fn consume_autonomous(
        &self,
        stream: &mut DriverEventStream,
        identity: &AutonomousSession,
        native_turn: Option<String>,
        background_owners: &mut BTreeMap<String, TurnId>,
    ) -> Result<(), CoreError> {
        let task = &identity.task;
        let mut meter = crate::turn_usage::TurnMeter::new(0);
        let driver = identity.driver.as_ref();
        let (turn, _ownership) = self.admit_autonomous(identity, native_turn).await?;
        let _workspace = self.workspace_access.read().await;
        let scope = self.turn_workspace_scope(task).await;
        let capture = if let Some(scope) = &scope {
            Some(
                crate::multi_workspace::begin(
                    self.workspace_context.as_ref(),
                    &scope.project_paths,
                )
                .await,
            )
        } else {
            None
        };
        let mut segments = TextSegments::default();
        let mut seen = std::collections::BTreeSet::new();
        let mut thoughts = BTreeMap::new();
        let mut terminal = None;
        let result: Result<(), CoreError> = async {
            while let Some(event) = stream.next().await {
                let mut event = event?;
                if let DriverEvent::Presentation { data, .. } = &event
                    && let Some(id) = data["sourceEventId"].as_str()
                    && !seen.insert(id.to_owned())
                {
                    continue;
                }
                if let Some(owner) = child_owner(&event, background_owners)
                    && owner != &turn
                {
                    self.persist_autonomous_event(task, owner, driver, event)
                        .await?;
                    continue;
                }
                self.observe_turn_usage(task, &turn, &mut meter, &event)
                    .await;
                if matches!(
                    event,
                    DriverEvent::UsageRecorded { .. } | DriverEvent::FirstOutput
                ) {
                    continue;
                }
                if let DriverEvent::Presentation { item_id, data } = &event
                    && (data["type"] == "subagent" || data["background"] == true)
                {
                    background_owners
                        .entry(item_id.clone())
                        .or_insert_with(|| turn.clone());
                }
                if matches!(event, DriverEvent::TurnStarted { .. }) {
                    return Err(CoreError::Internal("overlapping autonomous roots".into()));
                }
                if matches!(
                    event,
                    DriverEvent::TurnCompleted { .. }
                        | DriverEvent::TurnFailed { .. }
                        | DriverEvent::TurnInterrupted { .. }
                        | DriverEvent::TurnQuotaExhausted { .. }
                ) {
                    terminal = Some(event);
                    break;
                }
                // Match foreground privacy: reasoning deltas are never persisted.
                if let DriverEvent::Presentation { data, .. } = &mut event
                    && data["type"] == "thought"
                    && let Some(object) = data.as_object_mut()
                {
                    object.remove("delta");
                }
                if let DriverEvent::Presentation { item_id, data } = &event
                    && data["type"] == "thought"
                {
                    thoughts
                        .entry(item_id.clone())
                        .or_insert_with(std::time::Instant::now);
                }
                segments.apply(&mut event);
                self.persist_autonomous_event(task, &turn, driver, event)
                    .await?;
            }
            Ok(())
        }
        .await;
        // Native continuation events carry their own request facts. A live session
        // snapshot may already contain a later root, so never use a snapshot delta here.
        self.record_turn_usage(
            task,
            &turn,
            crate::turn_usage::TurnUsageInput {
                before: None,
                after: None,
                new_session: false,
                native_session_id: &identity.handle.native_session_id,
                meter: &meter,
            },
        )
        .await;
        if let (Some(scope), Some(capture)) = (&scope, capture) {
            self.record_fork_snapshot(scope).await;
            let change = match capture {
                Ok(capture) => capture.finish().await,
                Err(error) => Err(error),
            };
            if self.record_turn_changes(scope, change).await.is_err() {
                let _ = self
                    .append_runtime_event(
                        task,
                        "workspace.turn_changes_failed",
                        json!({"taskId":task,"turnId":turn,"failureKind":"storage_failed"}),
                        EventSource::Hub,
                    )
                    .await;
            }
        }
        let terminal = terminal.unwrap_or(DriverEvent::TurnFailed {
            reason: "autonomous stream ended before its terminal".into(),
        });
        for (item_id, start) in thoughts {
            self.persist_autonomous_event(task, &turn, driver, DriverEvent::Presentation {
                item_id, data: json!({"type":"thought",
                    "status":if matches!(terminal, DriverEvent::TurnCompleted { .. }) { "completed" } else { "interrupted" },
                    "durationMs":u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)}),
            }).await?;
        }
        self.persist_autonomous_event(task, &turn, driver, terminal)
            .await?;
        result
    }

    async fn persist_autonomous_event(
        &self,
        task: &TaskId,
        turn: &TurnId,
        driver: &dyn AgentDriver,
        event: DriverEvent,
    ) -> Result<(), CoreError> {
        let reusable = matches!(&event, DriverEvent::TurnInterrupted { .. })
            && driver.supports_live_session_reuse();
        let (kind, mut payload) =
            map_driver_event(task, turn, driver.executor(), event, self.redactor.as_ref());
        if reusable {
            payload["sessionReusable"] = json!(true);
        }
        self.append_runtime_event(
            task,
            kind,
            payload,
            EventSource::Driver {
                provider_id: driver.executor().provider_id.to_string(),
                source_sequence: None,
            },
        )
        .await
    }

    async fn autonomous_child_event(
        &self,
        identity: &AutonomousSession,
        owners: &BTreeMap<String, TurnId>,
        event: DriverEvent,
    ) -> Result<(), CoreError> {
        let AutonomousSession {
            task,
            mapping,
            handle: session,
            driver,
        } = identity;
        let Some(turn) = child_owner(&event, owners) else {
            return Ok(());
        };
        let Some(state) = self.repository.task_execution_state(task).await? else {
            return Ok(());
        };
        if state.session_maps.iter().any(|m| {
            m.session_map_id == *mapping
                && m.native_session_id == session.native_session_id
                && m.status == SessionMapStatus::Active
        }) {
            self.persist_autonomous_event(task, turn, driver.as_ref(), event)
                .await?;
        }
        Ok(())
    }
}

fn child_owner<'a>(
    event: &DriverEvent,
    owners: &'a BTreeMap<String, TurnId>,
) -> Option<&'a TurnId> {
    let DriverEvent::Presentation { item_id, data } = event else {
        return None;
    };
    if data["type"] == "thought" {
        return None;
    }
    let parent = data["parentId"].as_str().filter(|id| !id.is_empty());
    let id = parent.unwrap_or(item_id);
    owners.get(id)
}
