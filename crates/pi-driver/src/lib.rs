//! Pi's documented, long-lived RPC protocol (0.87+). No SDK subprocess wrapper.
mod events;
mod metadata;
#[cfg(all(test, unix))]
mod metadata_tests;
mod request_usage;
mod selection;
mod sessions;
mod transport;
use agent_domain::{ExecutorRef, ProviderId, QuestionAnswer};
use async_trait::async_trait;
use driver_api::{
    AgentDriver, DriverError, DriverEventStream, ModelCatalog, PermissionCatalog, PromptRequest,
    SessionHandle, SessionRequest, SessionResumeCapability, attachments,
};
use serde_json::{Value, json};
use sessions::stale_scope;
use std::{collections::HashMap, fmt::Write, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;
use transport::Connection;

#[derive(Clone)]
struct Session {
    connection: Arc<Connection>,
    workspace: SessionRequest,
}
struct PendingPromptDispatch {
    connection: Arc<Connection>,
    sent: bool,
}
impl Drop for PendingPromptDispatch {
    fn drop(&mut self) {
        if !self.sent {
            self.connection.request_close();
        }
    }
}
pub struct PiDriver {
    executor: ExecutorRef,
    executable: PathBuf,
    version: Option<String>,
    sessions: Mutex<HashMap<String, Session>>,
    stopping: watch::Sender<bool>,
    restore_gates: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    retired_handles: std::sync::Mutex<HashMap<String, SessionHandle>>,
    catalog_probe: Arc<Mutex<()>>,
}
impl Drop for PiDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}
fn permissions() -> PermissionCatalog {
    PermissionCatalog::new(
        "native",
        &[(
            "native",
            "Pi 原生权限",
            "Pi 不内置逐工具审批或沙箱；工具直接执行，扩展发起的询问会显示在对话中。",
            true,
        )],
    )
}
fn model_id(value: &Value) -> Option<String> {
    Some(format!(
        "{}/{}",
        value["provider"].as_str()?,
        value["id"].as_str()?
    ))
}
fn session_request(cwd: &str) -> SessionRequest {
    SessionRequest {
        working_directory: cwd.into(),
        additional_directories: vec![],
        app_mcps: vec![],
        permission_mode: None,
    }
}
impl PiDriver {
    pub fn new(
        installation: &str,
        executable: PathBuf,
        version: Option<String>,
    ) -> Result<Self, agent_domain::DomainError> {
        Ok(Self {
            executor: ExecutorRef {
                provider_id: ProviderId::parse("pi")?,
                installation_id: Some(installation.to_owned()),
            },
            executable,
            version,
            sessions: Mutex::new(HashMap::new()),
            stopping: watch::channel(false).0,
            restore_gates: std::sync::Mutex::new(HashMap::new()),
            retired_handles: std::sync::Mutex::new(HashMap::new()),
            catalog_probe: Arc::new(Mutex::new(())),
        })
    }
    async fn interrupt_with_timeout(
        &self,
        handle: &SessionHandle,
        timeout: std::time::Duration,
    ) -> Result<(), DriverError> {
        let connection = self.session_for(handle).await?.connection;
        if let Err(error) = connection.abort_turn(handle, timeout).await {
            // The result is uncertain. Retire this process and report failure;
            // never label an unconfirmed stop as a successful interruption.
            let retiring = connection.state.lock().expect("Pi state").disconnected;
            if retiring {
                let _ = self.retire(handle, &connection).await;
            }
            return Err(error);
        }
        Ok(())
    }
    pub async fn probe_ready(executable: &std::path::Path) -> Result<(), DriverError> {
        let connection = Connection::spawn(
            executable,
            &std::env::temp_dir().to_string_lossy(),
            None,
            true,
        )?;
        let result = async {
            connection.call(json!({"type":"get_state"})).await?;
            let models = connection
                .call(json!({"type":"get_available_models"}))
                .await?;
            if models["models"]
                .as_array()
                .is_none_or(std::vec::Vec::is_empty)
            {
                return Err(DriverError::NotConfigured(
                    "Pi has no configured models; use pi /login or configure a provider".into(),
                ));
            }
            Ok(())
        }
        .await;
        connection.close().await?;
        result
    }
    async fn connection(&self, id: &str) -> Result<Arc<Connection>, DriverError> {
        self.sessions
            .lock()
            .await
            .get(id)
            .map(|s| s.connection.clone())
            .ok_or_else(|| DriverError::SessionMissing("Pi session is not resident".into()))
    }
    async fn configure(
        connection: &Connection,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<Value, DriverError> {
        selection::configure(connection, model, thinking).await
    }
    async fn open(
        &self,
        request: SessionRequest,
        resume: Option<&str>,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        permissions().validate(request.permission_mode.as_deref())?;
        if !request.app_mcps.is_empty() {
            return Err(DriverError::CapabilityUnavailable(
                "Pi does not expose native MCP configuration".into(),
            ));
        }
        let gate = resume.map(|id| {
            self.restore_gates
                .lock()
                .expect("Pi restore gates")
                .entry(id.into())
                .or_default()
                .clone()
        });
        let _restoring = if let Some(gate) = &gate {
            Some(gate.lock().await)
        } else {
            None
        };
        let existing = if let Some(id) = resume {
            self.sessions.lock().await.get(id).cloned()
        } else {
            None
        };
        if let Some(session) = existing {
            let connection = &session.connection;
            let _admission = connection.admission.lock().await;
            if connection.state.lock().expect("Pi state").disconnected {
                let original = connection.handle().ok_or_else(stale_scope)?;
                connection.request_close();
                connection.confirmed().await;
                self.retire(&original, connection).await?;
            } else {
                if session.workspace != request {
                    return Err(DriverError::CapabilityUnavailable(
                        "Pi workspace changed; rebuild the session".into(),
                    ));
                }
                {
                    let state = connection.state.lock().expect("Pi state");
                    if state.turn.is_some() || state.cancelling {
                        return Err(DriverError::Protocol("Pi session is busy".into()));
                    }
                }
                let state = match Self::configure(connection, model, thinking).await {
                    Ok(state) => state,
                    Err(error) => {
                        let retiring = connection.state.lock().expect("Pi state").disconnected;
                        if retiring {
                            connection.confirmed().await;
                        }
                        return Err(error);
                    }
                };
                let id = resume.expect("existing session has a native id");
                if state["sessionFile"].as_str() != Some(id) {
                    connection.request_close();
                    connection.confirmed().await;
                    return Err(DriverError::SessionIncompatible(
                        "Pi changed the session during reuse".into(),
                    ));
                }
                return connection.handoff(id);
            }
        }
        if let Some(id) = resume
            && (!std::path::Path::new(id).is_absolute() || !std::path::Path::new(id).is_file())
        {
            return Err(DriverError::SessionMissing(
                "Pi session file is missing".into(),
            ));
        }
        let connection = Arc::new(Connection::spawn(
            &self.executable,
            &request.working_directory,
            resume,
            false,
        )?);
        connection.observe_owner(self.stopping.subscribe());
        let result = Self::configure(&connection, model, thinking).await;
        let state = match result {
            Ok(state) => state,
            Err(error) => {
                connection.request_close();
                connection.confirmed().await;
                return Err(error);
            }
        };
        let Some(id) = state["sessionFile"]
            .as_str()
            .filter(|path| std::path::Path::new(path).is_absolute())
            .map(str::to_owned)
        else {
            connection.request_close();
            connection.confirmed().await;
            return Err(DriverError::Protocol(
                "Pi did not provide a persistent session file".into(),
            ));
        };
        if resume.is_some_and(|expected| expected != id) {
            connection.request_close();
            connection.confirmed().await;
            return Err(DriverError::SessionIncompatible(
                "Pi restored a different session".into(),
            ));
        }
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(&id) {
            drop(sessions);
            connection.request_close();
            connection.confirmed().await;
            return Err(DriverError::SessionIncompatible(
                "Pi returned an already owned session file".into(),
            ));
        }
        let handle = match connection.handoff(&id) {
            Ok(handle) => handle,
            Err(error) => {
                drop(sessions);
                connection.request_close();
                connection.confirmed().await;
                return Err(error);
            }
        };
        sessions.insert(
            id,
            Session {
                connection,
                workspace: request,
            },
        );
        Ok(handle)
    }
}

#[async_trait]
impl AgentDriver for PiDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
    fn native_agent_version(&self) -> Option<&str> {
        self.version.as_deref()
    }
    fn resume_capability(&self) -> SessionResumeCapability {
        SessionResumeCapability::Supported
    }
    fn supports_live_session_reuse(&self) -> bool {
        true
    }
    fn supports_app_mcp(&self) -> bool {
        false
    }
    async fn permission_catalog(&self, _: Option<&str>) -> Result<PermissionCatalog, DriverError> {
        Ok(permissions())
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.open(request, None, None, None).await
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, None, model, thinking).await
    }
    async fn create_session_with_model(
        &self,
        request: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, None, model, None).await
    }
    async fn resume_session(&self, id: &str, cwd: &str) -> Result<SessionHandle, DriverError> {
        self.open(session_request(cwd), Some(id), None, None).await
    }
    async fn resume_selected_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, Some(id), model, thinking).await
    }
    async fn resume_workspace_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, Some(id), model, None).await
    }
    async fn model_catalog(&self) -> Result<ModelCatalog, DriverError> {
        self.model_catalog_in_workspace(None, None).await
    }
    async fn model_catalog_for(&self, model: Option<&str>) -> Result<ModelCatalog, DriverError> {
        self.model_catalog_in_workspace(model, None).await
    }
    async fn model_catalog_in_workspace(
        &self,
        model: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<ModelCatalog, DriverError> {
        metadata::read(self, model, workspace).await
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        permissions().validate(request.permission_mode.as_deref())?;
        let session = self.session_for(handle).await?;
        let connection = session.connection.clone();
        let _admission = connection.admission.lock().await;
        connection.validate_scope(handle)?;
        if session.workspace.working_directory != request.working_directory
            || session.workspace.additional_directories != request.additional_directories
        {
            return Err(DriverError::SessionIncompatible(
                "Pi workspace does not match the active session".into(),
            ));
        }
        attachments::validate(&request.attachments).await?;
        let mut message = String::new();
        for context in &request.context {
            let _ = write!(
                message,
                "[dogcodes {:?} {} sha256={}]\n{}\n\n",
                context.kind,
                context.artifact_id,
                context.payload_sha256,
                String::from_utf8_lossy(&context.content)
            );
        }
        if !request.additional_directories.is_empty() {
            let _ = write!(
                message,
                "Additional project directories (paths only): {}\n\n",
                json!(request.additional_directories)
            );
        }
        let mut images = vec![];
        for item in &request.attachments {
            if attachments::is_image(item) {
                images.push(json!({"type":"image","data":attachments::image_data(item).await?,"mimeType":item.mime_type}));
            } else {
                message.push_str(attachments::file_text(item)["text"].as_str().unwrap_or(""));
                message.push_str("\n\n");
            }
        }
        message.push_str(&request.prompt);
        let (turn_id, receiver) = connection.begin_turn(handle)?;
        // Cancellation while queued for stdin must not strand the local turn
        // claim, even if no byte of this prompt has reached the pipe yet.
        let mut dispatch = PendingPromptDispatch {
            connection: connection.clone(),
            sent: false,
        };
        let accepted = connection
            .request(&mut json!({"type":"prompt","message":message,"images":images}))
            .await?;
        dispatch.sent = true;
        // Return the event stream immediately. A preflight extension can ask a
        // question before the prompt response; waiting here would deadlock it.
        let dispatched = connection.clone();
        tokio::spawn(async move {
            if let Err(error) = accepted
                .await
                .unwrap_or_else(|_| Err(transport::disconnected()))
            {
                let retiring = {
                    let mut state = dispatched.state.lock().expect("Pi state");
                    if state.turn.as_ref().is_some_and(|turn| turn.id == turn_id) {
                        state.fail(&error);
                        true
                    } else {
                        false
                    }
                };
                if retiring {
                    dispatched.request_close();
                }
            }
        });
        Ok(Box::pin(UnboundedReceiverStream::new(receiver)))
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.interrupt_with_timeout(handle, std::time::Duration::from_secs(30))
            .await
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        id: &str,
        answer: QuestionAnswer,
    ) -> Result<(), DriverError> {
        let connection = self.session_for(handle).await?.connection;
        let _admission = connection.admission.lock().await;
        connection.validate_scope(handle)?;
        let response = {
            let mut state = connection.state.lock().expect("Pi state");
            let turn = state
                .turn
                .as_mut()
                .ok_or_else(|| DriverError::Protocol("Pi turn already ended".into()))?;
            let dialog = turn
                .dialogs
                .get(id)
                .ok_or_else(|| DriverError::Protocol("Pi question already closed".into()))?;
            dialog
                .form
                .validate(&answer)
                .map_err(DriverError::Protocol)?;
            let response = if answer.cancelled {
                json!({"type":"extension_ui_response","id":id,"cancelled":true})
            } else if dialog.method == "confirm" {
                json!({"type":"extension_ui_response","id":id,"confirmed":answer.values["answer"]})
            } else {
                json!({"type":"extension_ui_response","id":id,"value":answer.values["answer"]})
            };
            turn.dialogs.remove(id);
            response
        };
        connection.write(&response).await
    }
    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.sessions
            .lock()
            .await
            .get(id)
            .and_then(|s| s.connection.handle())
    }

    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&handle.native_session_id) {
            session.connection.validate_scope(handle).ok()?;
            let original = session.connection.clone();
            return Some(Box::pin(async move { original.confirmed().await }));
        }
        self.retired_handles
            .lock()
            .expect("Pi retired scopes")
            .get(&handle.native_session_id)
            .filter(|old| old.same_scope(handle))
            .map(|_| Box::pin(async {}) as driver_api::SessionShutdownProof)
    }

    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        let connection = {
            let sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get(&handle.native_session_id) {
                // Fence reuse synchronously, without waiting behind its write.
                let mut state = session.connection.state.lock().expect("Pi state");
                if !state
                    .scope
                    .as_ref()
                    .is_some_and(|old| old.same_scope(&handle))
                {
                    return Err(stale_scope());
                }
                state.fail(&transport::disconnected());
                session.connection.clone()
            } else {
                return if self
                    .retired_handles
                    .lock()
                    .expect("Pi retired scopes")
                    .get(&handle.native_session_id)
                    .is_some_and(|old| old.same_scope(&handle))
                {
                    Ok(())
                } else {
                    Err(stale_scope())
                };
            }
        };
        self.retire(&handle, &connection).await
    }
}

#[cfg(all(test, unix))]
mod cleanup_tests;
#[cfg(all(test, unix))]
mod scope_tests;
#[cfg(all(test, unix))]
mod tests;

#[async_trait]
impl driver_api::QuotaProvider for PiDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(
        &self,
        _id: Option<&str>,
    ) -> Result<driver_api::QuotaSnapshot, DriverError> {
        Err(DriverError::CapabilityUnavailable(
            "Pi does not report subscription quotas".into(),
        ))
    }
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        let connection = self.connection(id).await.ok()?;
        let value = connection.read_stats().await.ok()?;
        let mut usage = driver_api::usage::pi(&value);
        {
            let state = connection.state.lock().ok()?;
            usage.cache_hit_rate_percent = state.latest_cache_hit;
            usage.request_usage = Some(state.request_usage.clone());
        }
        usage.has_data().then_some(usage)
    }
}
