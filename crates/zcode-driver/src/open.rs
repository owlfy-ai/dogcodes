//! Admission and publication of current-format native sessions.
use super::{
    DriverError, SessionHandle, SessionRequest, SessionResumeCapability, SessionState, ZcodeDriver,
    context_from_snapshot, json, models, resume, scopes, session_id_from_snapshot, session_usage,
};
use std::sync::{Arc, atomic::Ordering};

impl ZcodeDriver {
    pub(super) async fn open(
        &self,
        id: Option<&str>,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let gate = id.map(|id| self.restore_gate(id));
        let _restore = match gate.as_ref() {
            Some(gate) => Some(gate.lock().await),
            None => None,
        };
        let mut rebind_native = None;
        if let Some(id) = id {
            let resident = self.sessions.lock().expect("sessions").get(id).cloned();
            if let Some(state) = resident {
                let _admission = state.admission.lock().await;
                let original = state.handle();
                let closing = {
                    let _gate = state.close_result.lock().expect("session lifecycle");
                    let closing = state.closing.load(Ordering::Acquire)
                        || state.native.as_ref().is_some_and(|n| n.stopping());
                    if !closing && state.turn_active.load(Ordering::Acquire) {
                        return Err(DriverError::AutonomousTurnBusy);
                    }
                    closing
                };
                if !same_workspace(&state.workspace, &request.working_directory) {
                    return Err(DriverError::SessionIncompatible(
                        "ZCode session belongs to a different workspace".into(),
                    ));
                }
                let binding_changed = state
                    .binding
                    .lock()
                    .expect("session binding")
                    .as_ref()
                    .is_some_and(|old| {
                        old.app_mcps != request.app_mcps
                            || old.additional_directories != request.additional_directories
                    });
                // MCP changes require reopening this logical session. The shared
                // native worker and its other tasks remain alive.
                if closing || binding_changed {
                    if binding_changed && !closing {
                        rebind_native = state.native.clone();
                    }
                    self.retire(&state, &original).await?;
                } else {
                    if !same_workspace(&state.workspace, &request.working_directory) {
                        return Err(DriverError::SessionIncompatible(
                            "ZCode session belongs to a different workspace".into(),
                        ));
                    }
                    if model.is_some() || thinking.is_some() {
                        let mut attempt = self.attempt(&state, &original);
                        let mut guard = self.connection.lock().await;
                        let result = match guard.as_mut() {
                            Some(connection)
                                if state
                                    .native
                                    .as_ref()
                                    .is_some_and(|n| Arc::ptr_eq(n, &connection.native))
                                    && !connection.native.stopping() =>
                            {
                                models::transaction(
                                    connection,
                                    id,
                                    &state.workspace,
                                    model,
                                    thinking,
                                )
                                .await
                            }
                            _ => Err(DriverError::Disconnected(
                                "ZCode original worker is unavailable".into(),
                            )),
                        };
                        drop(guard);
                        match result {
                            Ok(()) => attempt.committed = true,
                            Err(error)
                                if driver_api::failure::kind(&error) == "selection_rejected" =>
                            {
                                attempt.committed = true;
                                return Err(error);
                            }
                            Err(error) => {
                                self.retire(&state, &original).await?;
                                attempt.committed = true;
                                return Err(error);
                            }
                        }
                    }
                    let _gate = state.close_result.lock().expect("session lifecycle");
                    state.validate(&original)?;
                    if state.closing.load(Ordering::Acquire)
                        || state.native.as_ref().is_some_and(|n| n.stopping())
                    {
                        return Err(DriverError::Disconnected("ZCode session is closing".into()));
                    }
                    let handle = SessionHandle::scoped(id.into());
                    *state.scope.lock().expect("session scope") = handle.clone();
                    state.turn_done.lock().expect("turn receipt").take();
                    return Ok(handle);
                }
            }
            // A live session's configuration refresh stays on the same owned
            // worker. Cold history restoration remains independently gated.
            if rebind_native.is_none()
                && self.resume_capability != SessionResumeCapability::Supported
            {
                return Err(DriverError::CapabilityUnavailable(
                    "zcode session/resume has not been runtime verified".into(),
                ));
            }
        }
        let mut guard = self.ensure_connection().await?;
        let connection = guard.as_mut().expect("connection ensured");
        if id.is_some()
            && self.resume_capability != SessionResumeCapability::Supported
            && rebind_native
                .as_ref()
                .is_none_or(|native| !Arc::ptr_eq(native, &connection.native))
        {
            return Err(DriverError::CapabilityUnavailable(
                "ZCode original worker ended before configuration refresh; cold resume is unverified".into(),
            ));
        }
        let mut native_attempt = scopes::NativeAttempt {
            native: connection.native.clone(),
            committed: false,
        };
        let workspace = &request.working_directory;
        let method = if id.is_some() {
            "session/resume"
        } else {
            "session/create"
        };
        let mut params = session_params(&request);
        if let Some(id) = id {
            params["sessionId"] = json!(id);
        }
        let created = match connection.request(method, params, None).await {
            Ok(created) => created,
            Err(error)
                if matches!(error, DriverError::SessionMissing(_))
                    && connection.request_confirmed =>
            {
                // An explicit missing-session reply is positive evidence that
                // nothing was attached. Preserve other sessions on this worker.
                native_attempt.committed = true;
                return Err(error);
            }
            Err(error) => {
                connection.native.request_close();
                connection.native.confirmed().await;
                native_attempt.committed = true;
                return Err(error);
            }
        };
        let Some(native_id) =
            session_id_from_snapshot(&created).filter(|id| !id.is_empty() && id.len() <= 512)
        else {
            connection.native.request_close();
            connection.native.confirmed().await;
            native_attempt.committed = true;
            return Err(DriverError::Protocol(
                "ZCode did not report a native session identity".into(),
            ));
        };
        // A response identifying foreign workspace/native work grants no authority
        // to configure or close it. Do not mutate it on validation rejection.
        native_attempt.committed = true;
        resume::validate(&created, id.unwrap_or(native_id), workspace)?;
        let handle = SessionHandle::scoped(native_id.into());
        let state = SessionState::new(
            handle.clone(),
            workspace.clone(),
            created.clone(),
            connection.native.clone(),
        );
        *state.binding.lock().expect("session binding") = Some(request.clone());
        {
            let mut sessions = self.sessions.lock().expect("sessions");
            if sessions.contains_key(native_id) {
                return Err(DriverError::SessionIncompatible(
                    "ZCode native session is already owned".into(),
                ));
            }
            // No await between receiving the owned identity and this reservation.
            sessions.insert(native_id.into(), state.clone());
        }
        let mut attempt = self.attempt(&state, &handle);
        let result = async {
            connection
                .request(
                    "session/subscribe",
                    json!({"sessionId":native_id,"deliveryKind":"desktop-continuous"}),
                    None,
                )
                .await?;
            models::transaction(connection, native_id, workspace, model, thinking).await?;
            if model.is_some() || thinking.is_some() {
                connection
                    .request("session/read", json!({"sessionId":native_id}), None)
                    .await
            } else {
                Ok(created.clone())
            }
        }
        .await;
        drop(guard);
        match result {
            Ok(snapshot) => {
                if let Some(context) = context_from_snapshot(&snapshot, &created) {
                    self.latest_context
                        .lock()
                        .expect("context")
                        .insert(native_id.into(), context);
                }
                self.usage
                    .set(native_id, session_usage::read(&snapshot, &created));
                attempt.committed = true;
                Ok(handle)
            }
            Err(error) => {
                self.retire(&state, &handle).await?;
                attempt.committed = true;
                Err(error)
            }
        }
    }
    async fn retire(
        &self,
        state: &Arc<SessionState>,
        handle: &SessionHandle,
    ) -> Result<(), DriverError> {
        let receipt = super::lifecycle::close_session(
            self.connection.clone(),
            self.sessions.clone(),
            handle.clone(),
            state.clone(),
            self.retired.clone(),
        )?;
        super::lifecycle::wait(receipt, super::REQUEST_TIMEOUT).await?;
        // Installation waits for the original table reservation to retire too.
        // Public passive proof may independently use positive original OS exit.
        state.logical_confirmed().await;
        Ok(())
    }
}
fn session_params(request: &SessionRequest) -> serde_json::Value {
    // ZCode 0.17 supports a session-local denylist. Its bundled JS computer-use
    // entry would bypass the app's activity indicator, cancellation and audit.
    // An empty list on the next resume restores the native session defaults.
    let denied: &[&str] = if request.app_mcps.iter().any(driver_api::AppMcp::is_computer) {
        &["mcp__node_repl__js"]
    } else {
        &[]
    };
    let workspace = &request.working_directory;
    json!({
        "workspace":{"workspacePath":workspace,"workspaceKey":workspace},
        "additionalDirectories":request.additional_directories,
        "mcpServers":driver_api::AppMcp::native_servers(&request.app_mcps),
        "toolDenylist":denied
    })
}

fn same_workspace(a: &str, b: &str) -> bool {
    a == b
        || std::path::Path::new(a)
            .canonicalize()
            .ok()
            .zip(std::path::Path::new(b).canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn computer_tools_are_selected_per_session_and_can_be_removed() {
        let mut request = SessionRequest {
            app_mcps: vec![],
            permission_mode: None,
            additional_directories: vec![],
            working_directory: "/work".into(),
        };
        assert_eq!(session_params(&request)["toolDenylist"], json!([]));
        request.app_mcps.push(driver_api::AppMcp {
            name: "dogcodes_cu_test".into(),
            command: "app".into(),
            args: vec!["--computer-mcp".into()],
            directory: "/work".into(),
        });
        let params = session_params(&request);
        assert_eq!(params["toolDenylist"], json!(["mcp__node_repl__js"]));
        assert_eq!(params["mcpServers"][0]["name"], "dogcodes_cu_test");
        request.app_mcps.clear();
        assert_eq!(session_params(&request)["toolDenylist"], json!([]));
    }
}
