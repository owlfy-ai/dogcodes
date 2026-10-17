//! A control acknowledgement is not proof that a requested model/effort took effect.
use super::{
    DriverError, Ordering, PROBE_TIMEOUT, SessionProcess, Value, json, lifecycle, oneshot,
};

fn readable(value: &Value) -> bool {
    value["applied"]["model"].is_string() && value["applied"]["effort"].is_string()
}
fn model_matches(selected: &str, actual: &str) -> bool {
    actual == selected
        || ["sonnet", "opus", "haiku"].contains(&selected)
            && actual.starts_with(&format!("claude-{selected}-"))
}
fn matches(value: &Value, model: Option<&str>, thinking: Option<&str>) -> bool {
    readable(value)
        && model.filter(|v| *v != "default").is_none_or(|selected| {
            value["applied"]["model"]
                .as_str()
                .is_some_and(|actual| model_matches(selected, actual))
        })
        && thinking.filter(|v| *v != "auto").is_none_or(|selected| {
            if selected == "off" {
                value["effective"]["alwaysThinkingEnabled"] == false
            } else {
                value["applied"]["effort"] == selected
                    && value["effective"]["alwaysThinkingEnabled"] != false
            }
        })
}
fn settings(model: Option<&str>, thinking: Option<&str>) -> Value {
    json!({"model": model.filter(|v| *v != "default"),
        "effortLevel": thinking.filter(|v| !["off", "auto"].contains(v)),
        "alwaysThinkingEnabled": match thinking { Some("off") => Some(false), Some("auto") | None => None, Some(_) => Some(true) }})
}
fn selection(value: &Value) -> Value {
    json!({"model":value["applied"]["model"], "effort":value["applied"]["effort"], "thinking":value["effective"]["alwaysThinkingEnabled"]})
}
impl SessionProcess {
    async fn selection_control(&self, request: Value) -> Result<Value, DriverError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, reply) = oneshot::channel();
        self.pending_controls
            .lock()
            .expect("controls")
            .insert(id.clone(), sender);
        let result = async {
            self.send_line(&json!({"type":"control_request","request_id":id,"request":request}))
                .await?;
            tokio::time::timeout(PROBE_TIMEOUT, reply)
                .await
                .map_err(|_| DriverError::Disconnected("selection_unverifiable".into()))?
                .map_err(|_| DriverError::Disconnected("selection_unverifiable".into()))?
        }
        .await;
        self.pending_controls.lock().expect("controls").remove(&id);
        result
    }
    async fn read_selection(&self) -> Result<Value, DriverError> {
        let state = self
            .selection_control(json!({"subtype":"get_settings"}))
            .await
            .map_err(|_| DriverError::Protocol("selection_unverifiable".into()))?;
        if !readable(&state) {
            return Err(DriverError::Protocol("selection_unverifiable".into()));
        }
        Ok(state)
    }
    pub(super) async fn verify_selection(
        &self,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<(), DriverError> {
        if matches(&self.read_selection().await?, model, thinking) {
            Ok(())
        } else {
            Err(DriverError::Protocol("selection_unverifiable".into()))
        }
    }
    pub(super) async fn configure(
        &self,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<(), DriverError> {
        if self.autonomous_pending.load(Ordering::Acquire) > 0 {
            return Err(DriverError::AutonomousTurnBusy);
        }
        if self.turn_active.load(Ordering::Acquire) {
            return Err(DriverError::Protocol(
                "cannot configure an active Claude turn".into(),
            ));
        }
        let before = match self.read_selection().await {
            Ok(state) => state,
            Err(error) => {
                self.closing.store(true, Ordering::Release);
                let _ = self.commands.send(lifecycle::Command::Close);
                return Err(error);
            }
        };
        let apply = self
            .selection_control(
                json!({"subtype":"apply_flag_settings","settings":settings(model, thinking)}),
            )
            .await;
        if apply.is_ok() && self.verify_selection(model, thinking).await.is_ok() {
            return Ok(());
        }
        // Restore explicit native state, then verify the rollback too. If the native
        // process cannot prove its state, retire it before another prompt can run.
        let rollback = json!({"model":before["applied"]["model"],"effortLevel":before["applied"]["effort"],"alwaysThinkingEnabled":before["effective"]["alwaysThinkingEnabled"]});
        let restored = self
            .selection_control(json!({"subtype":"apply_flag_settings","settings":rollback}))
            .await
            .is_ok()
            && self
                .read_selection()
                .await
                .is_ok_and(|state| selection(&state) == selection(&before));
        if restored {
            Err(DriverError::Protocol("selection_rejected".into()))
        } else {
            self.closing.store(true, Ordering::Release);
            let _ = self.commands.send(lifecycle::Command::Close);
            Err(DriverError::Protocol("selection_unverifiable".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_applied_state_instead_of_requested_flags() {
        let mut state = json!({"effective":{"model":"sonnet","effortLevel":"high"},"applied":{"model":"claude-opus-5","effort":"low"}});
        assert!(!matches(&state, Some("sonnet"), Some("high")));
        state["applied"] = json!({"model":"claude-sonnet-5","effort":"high"});
        assert!(matches(&state, Some("sonnet"), Some("high")));
        assert!(!matches(&state, Some("claude-sonnet-4"), Some("high")));
        assert!(!matches(&Value::Null, None, None));
        state["effective"]["alwaysThinkingEnabled"] = json!(false);
        assert!(matches(&state, None, Some("off")));
        assert!(!matches(&state, None, Some("high")));
    }
    #[tokio::test]
    #[ignore = "requires local Claude CLI; uses only native control requests, no model prompts"]
    async fn native_selection_round_trip_without_model_requests() {
        use driver_api::{AgentDriver, SessionRequest, SessionResumeCapability};
        let executable = std::env::var_os("DOGCODES_CLAUDE_SELECTION_PROBE")
            .expect("explicit executable required");
        let directory = tempfile::tempdir().unwrap();
        let driver = crate::ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable.into(),
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let request = || SessionRequest {
            working_directory: directory.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            history_mcp: None,
            permission_mode: None,
        };
        let session = driver
            .create_selected_session(request(), Some("sonnet"), Some("high"))
            .await
            .unwrap();
        let result = driver
            .reconfigure_session(
                &session.native_session_id,
                request(),
                Some("opus"),
                Some("low"),
            )
            .await;
        driver.shutdown(session).await.unwrap();
        assert!(
            result.is_ok(),
            "native selection round trip failed: {result:?}"
        );
    }
}
