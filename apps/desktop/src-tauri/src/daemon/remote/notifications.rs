//! Durable event cursor, generic reminders only; never cloud-store task contents.
use super::{
    Arc, Core, Duration, PathBuf, RemoteHost, ServiceControl, Value, failure, http_client, json,
};
use agent_protocol::{EventEnvelope, StreamType};
use std::io::Write;

impl RemoteHost {
    pub(super) fn start_notifications(
        self: &Arc<Self>,
        core: &Arc<Core>,
        control: std::sync::Weak<ServiceControl>,
    ) {
        if self
            .notifications_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let host = Arc::downgrade(self);
        let core = Arc::downgrade(core);
        tokio::spawn(async move {
            let mut cursor = None;
            loop {
                if control.upgrade().is_none() {
                    return;
                }
                let (Some(host), Some(core)) = (host.upgrade(), core.upgrade()) else {
                    return;
                };
                let failed = host.notification_batch(&core, &mut cursor).await.is_err();
                drop(host);
                drop(core);
                if failed {
                    // Raw vendor responses, tokens and event contents never enter diagnostics.
                    crate::service_diagnostics::record(
                        "mobile_notification_pending",
                        "手机提醒尚未同步，将从原事件继续",
                    );
                    tokio::time::sleep(Duration::from_secs(10)).await;
                } else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        });
    }
    async fn notification_batch(
        &self,
        core: &Arc<Core>,
        cursor: &mut Option<u64>,
    ) -> Result<(), String> {
        let (config, secrets) = {
            let state = self.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            (ready.config.clone(), ready.secrets.clone())
        };
        let path = self.directory.join("notification-cursor.json");
        if cursor.is_none() {
            let initial = match std::fs::read(&path) {
                Ok(bytes) => serde_json::from_slice::<String>(&bytes)
                    .map_err(|_| "通知同步位置损坏")?
                    .parse::<u64>()
                    .map_err(|_| "通知同步位置损坏")?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    core.bootstrap().await.map_err(failure)?.last_sequence
                }
                Err(_) => return Err("通知同步位置不可读取".into()),
            };
            save_cursor(path.clone(), initial).await?;
            *cursor = Some(initial);
        }
        let events = core
            .sync_events(cursor.ok_or("missing cursor")?, 500)
            .await
            .map_err(failure)?;
        let mut accepted = cursor.ok_or("missing cursor")?;
        let live = core
            .bootstrap()
            .await
            .map_err(failure)?
            .tasks
            .into_iter()
            .map(|task| task.task_id.to_string())
            .collect::<std::collections::HashSet<_>>();
        for event in events {
            if let Some(payload) = notification(&event) {
                let pending = if !live.contains(&event.stream.stream_id) {
                    false
                } else if event.event_type == "approval.requested" {
                    let task = agent_domain::TaskId::parse(event.stream.stream_id.clone())
                        .map_err(failure)?;
                    core.pending_approvals(&task, 50)
                        .await
                        .map_err(failure)?
                        .iter()
                        .any(|p| p.requested_at_sequence == event.sequence)
                } else {
                    true
                };
                if pending
                    && live.contains(&event.stream.stream_id)
                    && (chrono::Utc::now() - event.occurred_at).num_hours() < 24
                {
                    http_client(&config.service)?
                        .post(format!(
                            "{}/v1/hosts/{}/notifications",
                            config.service, config.host_id
                        ))
                        .bearer_auth(&secrets.token)
                        .json(&payload)
                        .send()
                        .await
                        .map_err(|_| "通知接入暂时不可用")?
                        .error_for_status()
                        .map_err(|_| "通知接入尚未受理")?;
                }
            }
            accepted = event.sequence;
        }
        // Batch streaming text events into one disk write. On a lost HTTP or
        // disk receipt the same eventIds are replayed, never duplicated remotely.
        if Some(accepted) != *cursor {
            save_cursor(path, accepted).await?;
            *cursor = Some(accepted);
        }
        Ok(())
    }
}
async fn save_cursor(path: PathBuf, sequence: u64) -> Result<(), String> {
    tokio::task::spawn_blocking(move || write_cursor(&path, sequence))
        .await
        .map_err(|_| "通知同步位置保存失败")?
}
fn notification(event: &EventEnvelope) -> Option<Value> {
    if event.stream.stream_type != StreamType::Task {
        return None;
    }
    let kind = match event.event_type.as_str() {
        "approval.requested" if event.payload.get("question").is_some_and(|v| !v.is_null()) => {
            "question"
        }
        "approval.requested" => "approval",
        "turn.completed" => "completed",
        "turn.failed" => "failed",
        _ => return None,
    };
    Some(json!({"eventId":event.event_id,"taskId":event.stream.stream_id,"kind":kind}))
}
fn write_cursor(path: &std::path::Path, sequence: u64) -> Result<(), String> {
    let directory = path.parent().ok_or("通知同步目录无效")?;
    std::fs::create_dir_all(directory).map_err(|_| "通知同步目录不可写入")?;
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "通知同步位置不可写入")?;
    file.write_all(
        serde_json::to_string(&sequence.to_string())
            .map_err(failure)?
            .as_bytes(),
    )
    .and_then(|()| file.as_file_mut().sync_all())
    .map_err(|_| "通知同步位置保存失败")?;
    file.persist(path).map_err(|_| "通知同步位置保存失败")?;
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| "通知同步位置保存结果待核实")?;
    Ok(())
}
