//! Durable next-turn delivery for providers without native mid-turn input.
use crate::{Core, CoreError, RepositoryError};
use agent_domain::{ExecutorRef, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, EventSource, SubmitTurn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Settings {
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub permission_mode: Option<String>,
}

// One consumer per conversation. A concurrent enqueue marks an existing worker
// dirty so it cannot exit between the final empty read and the enqueue.
#[derive(Default)]
pub(super) struct Workers(Mutex<BTreeMap<TaskId, bool>>);

impl Core {
    #[must_use]
    pub fn has_queued_followups(&self) -> bool {
        !self
            .followups
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub async fn recover_queued_followups(self: &Arc<Self>) -> Result<(), CoreError> {
        for entry in self.repository.pending_followups(None).await? {
            self.schedule_followups(
                TaskId::parse(&entry.stream.stream_id)
                    .map_err(|e| CoreError::Internal(e.to_string()))?,
            );
        }
        Ok(())
    }

    pub(super) fn schedule_followups(self: &Arc<Self>, task: TaskId) {
        if self
            .followups
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(task.clone(), true)
            .is_some()
        {
            return;
        }
        let weak = Arc::downgrade(self);
        let mut events = self.events.subscribe();
        tokio::spawn(async move {
            while let Some(core) = weak.upgrade() {
                core.followups
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(task.clone(), false);
                match core.dispatch_next_followup(&task).await {
                    Ok(Progress::Dispatched) => continue,
                    Ok(Progress::Empty) => {
                        let mut workers = core
                            .followups
                            .0
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if workers.get(&task) == Some(&true) {
                            continue;
                        }
                        workers.remove(&task);
                        return;
                    }
                    Ok(Progress::Waiting) | Err(_) => {}
                }
                drop(core);
                // Native cleanup and driver registration can finish without publishing a task event.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while let Ok(event) = events.recv().await {
                        if event.stream.stream_id == task.as_str()
                            && matches!(
                                event.event_type.as_str(),
                                "turn.completed"
                                    | "turn.failed"
                                    | "turn.connection_lost"
                                    | "task.deleted"
                            )
                        {
                            break;
                        }
                    }
                })
                .await;
            }
        });
    }

    async fn dispatch_next_followup(
        self: &Arc<Self>,
        task_id: &TaskId,
    ) -> Result<Progress, CoreError> {
        let Some(entry) = self
            .repository
            .pending_followups(Some(task_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Progress::Empty);
        };
        let task = self.require_task(task_id).await?;
        if task.active_primary_run.is_some() || self.require_task_cleanup_clear(task_id).is_err() {
            return Ok(Progress::Waiting);
        }
        let data = &entry.payload;
        let executor: ExecutorRef = serde_json::from_value(data["executor"].clone())
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        if !self.drivers.read().await.contains_key(&executor) {
            return Ok(Progress::Waiting);
        }
        let settings: Settings = serde_json::from_value(data["settings"].clone())
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let message_id = data["messageId"]
            .as_str()
            .ok_or_else(|| CoreError::Internal("missing follow-up identity".into()))?;
        let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            task_id: task_id.clone(),
            prompt: data["text"].as_str().unwrap_or_default().into(),
            executor_override: Some(executor),
            model: settings.model,
            thinking: settings.thinking,
            permission_mode: settings.permission_mode,
            attachments: vec![],
            review_fix: None,
        }));
        request.actor = serde_json::from_value(data["actor"].clone())
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        request.expected_stream_version = Some(task.stream_version);
        match self.execute_with_followup(request, Some(message_id)).await {
            Ok(_) => Ok(Progress::Dispatched),
            Err(
                CoreError::Conflict(_)
                | CoreError::Repository(RepositoryError::Conflict { .. })
                | CoreError::DriverUnavailable(_),
            ) => Ok(Progress::Waiting),
            Err(error @ CoreError::Repository(_)) => Err(error),
            Err(_) => {
                self.append_runtime_event(
                    task_id,
                    "turn.followup_failed",
                    json!({"taskId":task_id,"messageId":message_id}),
                    EventSource::Hub,
                )
                .await?;
                Ok(Progress::Dispatched)
            }
        }
    }
}

enum Progress {
    Dispatched,
    Empty,
    Waiting,
}
