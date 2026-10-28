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
                if closing {
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
            if self.resume_capability != SessionResumeCapability::Supported {
                return Err(DriverError::CapabilityUnavailable(
                    "zcode session/resume has not been runtime verified".into(),
                ));
            }
        }
        let mut guard = self.ensure_connection().await?;
        let connection = guard.as_mut().expect("connection ensured");
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
        let mut params = json!({"workspace":{"workspacePath":workspace,"workspaceKey":workspace},"mcpServers":driver_api::HistoryMcp::native_servers(request.history_mcp.as_ref())});
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
        let _ = super::lifecycle::close_session(
            self.connection.clone(),
            self.sessions.clone(),
            handle.clone(),
            state.clone(),
            self.retired.clone(),
        )?;
        // Installation waits for the original table reservation to retire too.
        // Public passive proof may independently use positive original OS exit.
        state.logical_confirmed().await;
        Ok(())
    }
}
fn same_workspace(a: &str, b: &str) -> bool {
    a == b
        || std::path::Path::new(a)
            .canonicalize()
            .ok()
            .zip(std::path::Path::new(b).canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}
