//! Continue the originating Turn's child stream without keeping its run active.
use crate::{
    AppendRequest, CommandId, Core, DriverEvent, EventSource, ExecutorRef, NewEvent,
    RepositoryError, SessionMapId, StreamExt, StreamRef, StreamType, TaskId, TurnId,
    map_driver_event,
};
impl Core {
    pub(crate) fn observe_background(
        &self,
        mut stream: driver_api::DriverEventStream,
        task_id: TaskId,
        turn_id: TurnId,
        executor: ExecutorRef,
        mapping_id: SessionMapId,
        native_id: String,
    ) {
        let repository = self.repository.clone();
        let redactor = self.redactor.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut seen = std::collections::BTreeSet::new();
            while let Some(Ok(event)) = stream.next().await {
                let DriverEvent::Presentation { data, .. } = &event else {
                    continue;
                };
                // Only explicit child activity is valid outside a parent Turn.
                if data["type"] == "thought" {
                    continue;
                }
                if data["type"] != "subagent"
                    && data["parentId"].as_str().unwrap_or("").is_empty()
                    && !(data["type"] == "tool" && data["background"] == true)
                {
                    continue;
                }
                if let Some(id) = data["sourceEventId"].as_str()
                    && !seen.insert(id.to_owned())
                {
                    continue;
                }
                let (kind, payload) =
                    map_driver_event(&task_id, &turn_id, &executor, event, redactor.as_ref());
                let mut pending = NewEvent::hub(
                    StreamRef {
                        stream_type: StreamType::Task,
                        stream_id: task_id.to_string(),
                    },
                    kind,
                    payload,
                );
                pending.source = EventSource::Driver {
                    provider_id: executor.provider_id.to_string(),
                    source_sequence: None,
                };
                // Read and append against the same task version. Never let an old
                // native process write activity into a replacement Session.
                loop {
                    let Ok(snapshot) = repository.bootstrap().await else {
                        return;
                    };
                    if !snapshot.session_maps.iter().any(|m| {
                        m.session_map_id == mapping_id
                            && m.native_session_id == native_id
                            && m.task_id == task_id
                            && m.executor == executor
                    }) {
                        return;
                    }
                    let Some(task) = snapshot.tasks.iter().find(|t| t.task_id == task_id) else {
                        return;
                    };
                    let append = repository
                        .append(AppendRequest {
                            request_fingerprint: None,
                            command_id: CommandId::new(),
                            idempotency_key: pending.event_id.to_string(),
                            expected_stream_version: Some(task.stream_version),
                            artifacts: vec![],
                            events: vec![pending.clone()],
                        })
                        .await;
                    match append {
                        Ok(outcome) => {
                            for event in outcome.events {
                                let _ = events.send(event);
                            }
                            break;
                        }
                        Err(RepositoryError::Conflict { .. }) => {
                            tokio::task::yield_now().await;
                        }
                        Err(_) => return,
                    }
                }
            }
        });
    }
}
