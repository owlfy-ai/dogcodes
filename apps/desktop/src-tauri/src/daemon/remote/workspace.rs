//! Paired-owner workspace API. Reuses daemon services, never desktop window commands.
use super::{Arc, Core, ServiceControl, Value, failure, json, parse_sequence};
use crate::workspace_service::watch::WatchMessage;
use tokio::sync::mpsc;

pub(super) const METHODS: &[&str] = &[
    "workspace_roots",
    "workspace_directory",
    "workspace_preview",
    "workspace_find_files",
    "review",
    "terminal_start",
    "terminal_launch_intents",
    "terminal_list",
    "terminal_get",
    "terminal_read",
    "terminal_input",
    "terminal_resize",
    "terminal_stop",
    "project_runs",
    "project_runs_analyze",
    "project_run_edit",
    "project_run_operation",
    "project_run_start",
];
pub(super) const STREAMS: &[&str] = &["workspace_watch", "review_watch", "project_runs_watch"];

pub(super) fn capabilities() -> Value {
    json!({
        "version": 1, "platform": std::env::consts::OS, "workspaceMethods": METHODS, "workspaceStreams": STREAMS,
        "commandRecoveryMethods":["command_receipt"],
        "terminalLaunchRecovery": true, "terminalStopEpoch": true, "terminalInputStreams":true, "runOperationRequest": true,
        "businessMethods": super::business::METHODS,
        "desktopMethods": super::native::METHODS,
        "clipboardTransfer": {"version":1,"chunkBytes":super::clipboard::CHUNK,"maxImageBytes":dogcodes_remote_desktop::MAX_CLIPBOARD_IMAGE_BYTES},
        "powerMethods": super::power::METHODS,
        "previewMethods": super::preview::METHODS,
        "transportMethods": ["direct_open","pairing_receipt","transport_status","transport_options"], "transports":["webrtc-datachannel","wss-relay"],
        "unsubscribe": "unsubscribe", "maxSubscriptions": super::MAX_SUBSCRIPTIONS,
        "maxInFlightRequests": 16, "maxFrameBytes": super::MAX_FRAME_BYTES,"transportFragments":true,
        "maxTerminalInputBytes": 16384, "terminalOffsets": "decimal-string",
        "reviewExcludedOperations": ["reveal_path", "workspace_path"]
    })
}

pub(super) async fn call(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    mut args: Value,
) -> Result<Value, String> {
    if !METHODS.contains(&method) {
        return Err("remote method unavailable".into());
    }
    if method == "review" {
        use crate::review::service::ReviewCall as R;
        let request: R = serde_json::from_value(args).map_err(failure)?;
        // Exhaustive allowlist: adding a local operation does not expose it remotely.
        match &request {
            R::Query(_)
            | R::Operations(_)
            | R::Lease(_)
            | R::File { .. }
            | R::History { .. }
            | R::Branches { .. }
            | R::Index { .. }
            | R::Discard { .. }
            | R::GitAction(_)
            | R::GitClose { .. }
            | R::GitAdvance { .. }
            | R::GitOperation { .. }
            | R::CommitSummary { .. }
            | R::BatchFiles { .. }
            | R::FileOperation { .. }
            | R::FileClose { .. }
            | R::Preview { .. }
            | R::Ignore { .. }
            | R::UndoStatus { .. }
            | R::Undo { .. }
            | R::Workspaces { .. }
            | R::StartProjectReview(_) => {}
            R::RevealPath { .. } | R::WorkspacePath { .. } => {
                return Err("remote review operation unavailable".into());
            }
        }
        return crate::review::service::dispatch(core, &control.review, request).await;
    }
    if method == "terminal_read" {
        parse_sequence(args.get_mut(1))?;
    }
    let target = if method == "project_run_start" {
        "project_run_open"
    } else {
        method
    };
    let mut value = super::super::dispatch_service(core, control, target, args).await?;
    if method == "terminal_read" {
        for field in ["from", "next"] {
            if let Some(number) = value.get(field).and_then(Value::as_u64) {
                value[field] = json!(number.to_string());
            }
        }
    }
    Ok(value)
}

enum Subscription {
    Workspace(crate::workspace_service::watch::Subscription),
    Changes(crate::review::watch::Subscription),
}
impl Subscription {
    async fn next(&mut self) -> Result<WatchMessage, String> {
        match self {
            Self::Workspace(stream) => stream.next().await,
            Self::Changes(stream) => stream.next().await,
        }
    }
}

pub(super) async fn subscribe(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    args: Value,
    out: &mpsc::Sender<Value>,
    id: &str,
) -> Result<(), String> {
    let mut subscription = match method {
        "workspace_watch" => Subscription::Workspace(
            control
                .workspace_watchers
                .open(core.clone(), serde_json::from_value(args).map_err(failure)?)
                .await?,
        ),
        "review_watch" | "project_runs_watch" => Subscription::Changes(
            control
                .review
                .changes
                .open(core.clone(), serde_json::from_value(args).map_err(failure)?)
                .await?,
        ),
        _ => return Err("remote stream unavailable".into()),
    };
    out.send(super::response(
        id,
        Ok(json!({"subscribed":true,"subscriptionId":id})),
    ))
    .await
    .map_err(|_| "connection closed")?;
    loop {
        let event = subscription
            .next()
            .await
            .unwrap_or_else(|message| WatchMessage::Error {
                code: "disconnected".into(),
                message,
            });
        let ended = matches!(event, WatchMessage::Error { .. });
        out.send(json!({"type":"notification","subscriptionId":id,"method":method,"event":event}))
            .await
            .map_err(|_| "connection closed")?;
        if ended {
            return Ok(());
        }
    }
}

/// Connection-owned tasks also get aborted on early errors and revoked grants.
#[derive(Default)]
pub(super) struct Subscriptions(
    std::collections::HashMap<String, (String, tokio::task::JoinHandle<()>)>,
);
impl Subscriptions {
    pub(super) async fn remove(&mut self, id: &str) -> bool {
        if let Some((_, task)) = self.0.remove(id) {
            task.abort();
            let _ = task.await;
            true
        } else {
            false
        }
    }
    pub(super) async fn prepare(&mut self, id: &str, scope: &str) -> Result<(), String> {
        self.0.retain(|_, (_, task)| !task.is_finished());
        let replaced: Vec<_> = self
            .0
            .iter()
            .filter(|(key, (old_scope, _))| key.as_str() == id || old_scope == scope)
            .map(|(key, _)| key.clone())
            .collect();
        for key in replaced {
            self.remove(&key).await;
        }
        if self.0.len() >= super::MAX_SUBSCRIPTIONS {
            return Err("too many subscriptions".into());
        }
        Ok(())
    }
    pub(super) fn insert(&mut self, id: String, scope: String, task: tokio::task::JoinHandle<()>) {
        self.0.insert(id, (scope, task));
    }
}
impl Drop for Subscriptions {
    fn drop(&mut self) {
        for (_, task) in self.0.values() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests;
