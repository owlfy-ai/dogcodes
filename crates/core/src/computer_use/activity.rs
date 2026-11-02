//! Transient execution state, independent of the agent's claimed progress.
use agent_domain::TaskId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityMode {
    #[default]
    Idle,
    Observing,
    Background,
    Foreground,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerActivity {
    pub operation_id: Option<String>,
    pub task_id: Option<TaskId>,
    pub tool: Option<String>,
    pub mode: ActivityMode,
    pub preparing: bool,
    /// A fallback-capable batch or app launch may front without necessarily doing so.
    pub foreground_possible: bool,
}

pub(super) struct ActivityHub {
    pub state: watch::Sender<ComputerActivity>,
    presented: watch::Sender<Option<(String, bool)>>,
    pub notice_required: AtomicBool,
}
impl Default for ActivityHub {
    fn default() -> Self {
        Self {
            state: watch::channel(ComputerActivity::default()).0,
            presented: watch::channel(None).0,
            notice_required: AtomicBool::new(false),
        }
    }
}
impl ActivityHub {
    pub fn begin(self: &Arc<Self>, task: &TaskId, tool: &str, args: &Value) -> ActivityGuard {
        let (mode, foreground_possible) = classify(tool, args);
        let id = uuid::Uuid::new_v4().to_string();
        self.state.send_replace(ComputerActivity {
            operation_id: Some(id.clone()),
            task_id: Some(task.clone()),
            tool: Some(tool.into()),
            mode,
            preparing: mode == ActivityMode::Foreground,
            foreground_possible,
        });
        ActivityGuard {
            hub: self.clone(),
            id,
        }
    }
    pub fn presented(&self, id: &str) -> bool {
        let state = self.state.borrow();
        if state.operation_id.as_deref() != Some(id) || state.mode != ActivityMode::Foreground {
            return false;
        }
        self.presented.send_replace(Some((id.into(), true)));
        true
    }
    pub fn unavailable(&self, id: &str) -> bool {
        if self.state.borrow().operation_id.as_deref() != Some(id) {
            return false;
        }
        self.presented.send_replace(Some((id.into(), false)));
        true
    }
}

pub(super) struct ActivityGuard {
    hub: Arc<ActivityHub>,
    id: String,
}
impl ActivityGuard {
    pub async fn before_input(&self) -> Result<(), String> {
        if self.hub.state.borrow().mode != ActivityMode::Foreground {
            return Ok(());
        }
        if self.hub.notice_required.load(Ordering::Acquire) {
            let mut presented = self.hub.presented.subscribe();
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if let Some((id, ready)) = presented.borrow_and_update().as_ref()
                        && id == &self.id {
                        if !ready { return Err("computer_escape_unavailable; foreground input was not sent; background delivery remains available".into()); }
                        break;
                    }
                    presented
                        .changed()
                        .await
                        .map_err(|_| "computer_notice_disconnected")?;
                }
                // Give the human time to see the painted warning before input starts.
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                Ok::<_, String>(())
            })
            .await
            .map_err(|_| {
                "computer_notice_unavailable; foreground input was not sent".to_string()
            })??;
        }
        self.hub.state.send_if_modified(|state| {
            if state.operation_id.as_deref() != Some(&self.id) {
                return false;
            }
            state.preparing = false;
            true
        });
        Ok(())
    }
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.hub.state.send_if_modified(|state| {
            if state.operation_id.as_deref() != Some(&self.id) {
                return false;
            }
            *state = ComputerActivity::default();
            true
        });
    }
}

fn foreground(args: &Value) -> bool {
    args["delivery_mode"] == "foreground"
        || args["scope"] == "desktop"
        || args["target"]["kind"] == "desktop"
}

/// Route-based classification, not a claim that a background write was verified.
fn classify(tool: &str, args: &Value) -> (ActivityMode, bool) {
    if matches!(
        tool,
        "list_apps"
            | "list_windows"
            | "get_window_state"
            | "get_desktop_state"
            | "zoom"
            | "verify_state"
    ) {
        return (ActivityMode::Observing, false);
    }
    if tool == "launch_app" {
        return (ActivityMode::Foreground, true);
    }
    if foreground(args) {
        return (ActivityMode::Foreground, false);
    }
    if tool == "run_steps" {
        if args["foreground_fallback"] == true {
            return (ActivityMode::Foreground, true);
        }
        if args["steps"].as_array().is_some_and(|steps| {
            steps.iter().any(|step| {
                if step.get("tool").is_some() {
                    foreground(&step["args"])
                } else {
                    step.as_object()
                        .is_some_and(|object| object.values().any(foreground))
                }
            })
        }) {
            return (ActivityMode::Foreground, false);
        }
    }
    (ActivityMode::Background, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn classifications_cover_both_batch_shapes_fallback_and_desktop_aliases() {
        assert_eq!(
            classify("get_desktop_state", &json!({})),
            (ActivityMode::Observing, false)
        );
        assert_eq!(
            classify("click", &json!({"x":1,"y":2})),
            (ActivityMode::Background, false)
        );
        for args in [
            json!({"scope":"desktop"}),
            json!({"target":{"kind":"desktop"}}),
            json!({"delivery_mode":"foreground"}),
        ] {
            assert_eq!(classify("click", &args).0, ActivityMode::Foreground);
        }
        for step in [
            json!({"tool":"hotkey","args":{"delivery_mode":"foreground"}}),
            json!({"hotkey":{"delivery_mode":"foreground"}}),
        ] {
            assert_eq!(
                classify("run_steps", &json!({"steps":[step]})).0,
                ActivityMode::Foreground
            );
        }
        assert_eq!(
            classify(
                "run_steps",
                &json!({"foreground_fallback":true,"steps":[{"click":{}}]})
            ),
            (ActivityMode::Foreground, true)
        );
        assert_eq!(
            classify("launch_app", &json!({"start_minimized":true})),
            (ActivityMode::Foreground, true)
        );
    }
    #[tokio::test(start_paused = true)]
    async fn foreground_waits_for_matching_painted_notice_and_lead_time() {
        let hub = Arc::new(ActivityHub::default());
        hub.notice_required.store(true, Ordering::Release);
        let guard = hub.begin(
            &TaskId::new(),
            "click",
            &json!({"delivery_mode":"foreground"}),
        );
        let id = hub.state.borrow().operation_id.clone().unwrap();
        let pending = tokio::spawn(async move { guard.before_input().await });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        assert!(!hub.presented("old-notice"));
        assert!(hub.presented(&id));
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(pending.await.unwrap().is_ok());
        assert_eq!(hub.state.borrow().mode, ActivityMode::Idle);
    }
    #[tokio::test(start_paused = true)]
    async fn a_missing_notice_refuses_input_and_releases_transient_state() {
        let hub = Arc::new(ActivityHub::default());
        hub.notice_required.store(true, Ordering::Release);
        let guard = hub.begin(
            &TaskId::new(),
            "click",
            &json!({"delivery_mode":"foreground"}),
        );
        assert!(
            guard
                .before_input()
                .await
                .unwrap_err()
                .contains("foreground input was not sent")
        );
        drop(guard);
        assert_eq!(hub.state.borrow().mode, ActivityMode::Idle);
    }

    #[tokio::test]
    async fn a_missing_escape_observer_refuses_foreground_but_allows_background() {
        let hub = Arc::new(ActivityHub::default());
        hub.notice_required.store(true, Ordering::Release);
        let guard = hub.begin(
            &TaskId::new(),
            "click",
            &json!({"delivery_mode":"foreground"}),
        );
        assert!(!hub.unavailable("expired-operation"));
        let id = hub.state.borrow().operation_id.clone().unwrap();
        assert!(hub.unavailable(&id));
        assert!(
            guard
                .before_input()
                .await
                .unwrap_err()
                .contains("computer_escape_unavailable")
        );
        drop(guard);
        let background = hub.begin(&TaskId::new(), "click", &json!({}));
        assert!(background.before_input().await.is_ok());
    }
}
