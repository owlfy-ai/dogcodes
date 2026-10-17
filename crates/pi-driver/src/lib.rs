//! Pi's documented, long-lived RPC protocol (0.87+). No SDK subprocess wrapper.
mod events;
mod request_usage;
mod transport;
use agent_domain::{ExecutorRef, ProviderId, QuestionAnswer};
use async_trait::async_trait;
use driver_api::{
    AgentDriver, DriverError, DriverEventStream, ModelCatalog, ModelOption, PermissionCatalog,
    PromptRequest, SessionHandle, SessionRequest, SessionResumeCapability, ThinkingOption,
    attachments,
};
use serde_json::{Value, json};
use std::{collections::HashMap, fmt::Write, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_stream::wrappers::UnboundedReceiverStream;
use transport::Connection;

struct Session {
    connection: Arc<Connection>,
    workspace: SessionRequest,
}
pub struct PiDriver {
    executor: ExecutorRef,
    executable: PathBuf,
    version: Option<String>,
    sessions: Mutex<HashMap<String, Session>>,
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
        history_mcp: None,
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
        })
    }
    async fn interrupt_with_timeout(
        &self,
        handle: &SessionHandle,
        timeout: std::time::Duration,
    ) -> Result<(), DriverError> {
        let connection = self.connection(&handle.native_session_id).await?;
        if let Err(error) = connection.abort_turn(timeout).await {
            // The result is uncertain. Retire this process and report failure;
            // never label an unconfirmed stop as a successful interruption.
            connection.close().await;
            self.sessions.lock().await.remove(&handle.native_session_id);
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
        connection.close().await;
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
        if let Some(model) = model {
            let (provider, id) = model
                .split_once('/')
                .filter(|(p, m)| !p.is_empty() && !m.is_empty())
                .ok_or_else(|| {
                    DriverError::CapabilityUnavailable("Invalid Pi model selection".into())
                })?;
            connection
                .call(json!({"type":"set_model","provider":provider,"modelId":id}))
                .await?;
        }
        if let Some(level) = thinking {
            let levels = connection
                .call(json!({"type":"get_available_thinking_levels"}))
                .await?;
            if !levels["levels"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == level))
            {
                return Err(DriverError::CapabilityUnavailable(
                    "Pi model does not support this thinking level".into(),
                ));
            }
            connection
                .call(json!({"type":"set_thinking_level","level":level}))
                .await?;
        }
        let state = connection.call(json!({"type":"get_state"})).await?;
        if model.is_some_and(|selected| model_id(&state["model"]).as_deref() != Some(selected))
            || thinking.is_some_and(|selected| state["thinkingLevel"] != selected)
        {
            return Err(DriverError::Protocol(
                "Pi did not confirm model/thinking selection".into(),
            ));
        }
        Ok(state)
    }
    async fn open(
        &self,
        request: SessionRequest,
        resume: Option<&str>,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        permissions().validate(request.permission_mode.as_deref())?;
        if request.history_mcp.is_some() {
            return Err(DriverError::CapabilityUnavailable(
                "Pi does not expose native MCP configuration".into(),
            ));
        }
        let mut sessions = self.sessions.lock().await;
        if let Some(id) = resume
            && sessions.get(id).is_some_and(|session| {
                session
                    .connection
                    .state
                    .lock()
                    .expect("Pi state")
                    .disconnected
            })
            && let Some(session) = sessions.remove(id)
        {
            session.connection.close().await;
        }
        if let Some(id) = resume
            && let Some(session) = sessions.get(id)
        {
            if session.workspace != request {
                return Err(DriverError::CapabilityUnavailable(
                    "Pi workspace changed; rebuild the session".into(),
                ));
            }
            if session
                .connection
                .state
                .lock()
                .expect("Pi state")
                .turn
                .is_some()
            {
                return Err(DriverError::Protocol("Pi session is busy".into()));
            }
            Self::configure(&session.connection, model, thinking).await?;
            return Ok(SessionHandle {
                native_session_id: id.into(),
            });
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
        let result = Self::configure(&connection, model, thinking).await;
        let state = match result {
            Ok(state) => state,
            Err(error) => {
                connection.close().await;
                return Err(error);
            }
        };
        let id = state["sessionFile"]
            .as_str()
            .filter(|path| std::path::Path::new(path).is_absolute())
            .ok_or_else(|| {
                DriverError::Protocol("Pi did not provide a persistent session file".into())
            })?
            .to_owned();
        if resume.is_some_and(|expected| expected != id) {
            connection.close().await;
            return Err(DriverError::SessionIncompatible(
                "Pi restored a different session".into(),
            ));
        }
        sessions.insert(
            id.clone(),
            Session {
                connection,
                workspace: request,
            },
        );
        Ok(SessionHandle {
            native_session_id: id,
        })
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
    fn supports_history_mcp(&self) -> bool {
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
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        let connection =
            Connection::spawn(&self.executable, workspace.unwrap_or(&temp), None, true)?;
        let result = async {
            let initial = connection.call(json!({"type":"get_state"})).await?;
            let state = Self::configure(&connection, model, None).await?;
            let models = connection
                .call(json!({"type":"get_available_models"}))
                .await?;
            let levels = connection
                .call(json!({"type":"get_available_thinking_levels"}))
                .await?;
            let selected = model_id(&state["model"]);
            Ok(ModelCatalog {
                default_model: model_id(&initial["model"]),
                models: models["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|row| {
                        let id = model_id(row)?;
                        let mut option = ModelOption::new(
                            &id,
                            &format!(
                                "{} · {}",
                                row["name"].as_str().unwrap_or(&id),
                                row["provider"].as_str().unwrap_or("")
                            ),
                        );
                        if Some(&id) == selected.as_ref() {
                            option.thinking_options = levels["levels"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(|v| ThinkingOption {
                                    id: v.into(),
                                    label: v.into(),
                                })
                                .collect();
                            option.default_thinking =
                                state["thinkingLevel"].as_str().map(Into::into);
                        }
                        Some(option)
                    })
                    .collect(),
            })
        }
        .await;
        connection.close().await;
        result
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        permissions().validate(request.permission_mode.as_deref())?;
        attachments::validate(&request.attachments).await?;
        let connection = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(&handle.native_session_id)
                .ok_or_else(|| DriverError::SessionMissing("Pi session is not resident".into()))?;
            if session.workspace.working_directory != request.working_directory
                || session.workspace.additional_directories != request.additional_directories
            {
                return Err(DriverError::SessionIncompatible(
                    "Pi workspace does not match the active session".into(),
                ));
            }
            session.connection.clone()
        };
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
        let receiver = connection.begin_turn()?;
        let accepted = connection
            .request(&mut json!({"type":"prompt","message":message,"images":images}))
            .await?;
        // Return the event stream immediately. A preflight extension can ask a
        // question before the prompt response; waiting here would deadlock it.
        tokio::spawn(async move {
            if let Err(error) = accepted
                .await
                .unwrap_or_else(|_| Err(transport::disconnected()))
                && let Some(turn) = connection.state.lock().expect("Pi state").turn.take()
            {
                let _ = turn.sender.send(Err(error));
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
        let connection = self.connection(&handle.native_session_id).await?;
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
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        if let Some(session) = self.sessions.lock().await.remove(&handle.native_session_id) {
            session.connection.close().await;
        }
        Ok(())
    }
}

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
        let value = connection
            .call(json!({"type":"get_session_stats"}))
            .await
            .ok()?;
        let mut usage = driver_api::usage::pi(&value);
        {
            let state = connection.state.lock().ok()?;
            usage.cache_hit_rate_percent = state.latest_cache_hit;
            usage.request_usage = Some(state.request_usage.clone());
        }
        usage.has_data().then_some(usage)
    }
}
