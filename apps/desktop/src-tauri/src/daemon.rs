//! Versioned local desktop RPC. Execution ownership stays in the service.
mod access;
mod remote;
mod review_watch;
mod startup;
mod workspace;
use super::{
    Arc, BUILTIN_AGENT_MANIFESTS, Core, ExecutorRef, FakeDriver, GitWorkspaceContextProvider,
    ProviderManifest, RecvError, TaskId, acquire_host_lease, ensure_initial_conversation,
    prepare_desktop_storage, scan_builtin_agent, scan_builtin_agents,
};
use agent_core::turn_review::{ReviewRequest, ReviewRun};
use agent_core::{
    QuotaSnapshotView, StoredArtifact,
    history::{HistoryPage, HistoryQuery},
};
use agent_domain::ArtifactId;
use agent_domain::RunId;
use agent_protocol::{
    ActorRef, ArtifactChunk, ArtifactCursor, ArtifactPage, ArtifactReadRequest, BootstrapSnapshot,
    BuiltContextPackage, CommandAck, CommandEnvelope, ContextEditRequest, ContextPreviewRequest,
    CoreCommand, EventEnvelope, PendingApproval, RecoverySnapshot, TimelineItem,
    WorkspaceSnapshotSummary,
};
use driver_api::{ModelCatalog, PermissionCatalog};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
// This is the compatibility boundary, not the executable's build identity.
// Bump it when changing existing RPC arguments, replies or semantics incompatibly:
// an updated desktop may reconnect to a compatible service that still owns work.
const VERSION: u32 = 1;
const MAX_FRAME: usize = 64 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Endpoint {
    version: u32,
    address: std::net::SocketAddr,
    token: String,
    pid: u32,
    build: String,
}
#[derive(Serialize, Deserialize)]
struct Request {
    version: u32,
    token: String,
    method: String,
    args: Value,
}
#[derive(Clone)]
pub enum CoreClient {
    Remote(PathBuf),
    #[cfg(any(test, feature = "review-acceptance"))]
    Local(Arc<Core>, Arc<EmbeddedServices>),
}
#[cfg(any(test, feature = "review-acceptance"))]
pub struct EmbeddedServices {
    control: ServiceControl,
    _directory: Option<tempfile::TempDir>,
}

impl CoreClient {
    /// # Panics
    /// If an isolated temporary service directory cannot be created.
    #[cfg(any(test, feature = "review-acceptance"))]
    pub fn local(core: Arc<Core>) -> Arc<Self> {
        let directory = tempfile::tempdir().expect("isolated service directory");
        let control = ServiceControl::new(directory.path());
        Arc::new(Self::Local(
            core,
            Arc::new(EmbeddedServices {
                control,
                _directory: Some(directory),
            }),
        ))
    }
    #[cfg(any(test, feature = "review-acceptance"))]
    pub fn local_at(core: Arc<Core>, directory: &Path) -> Arc<Self> {
        Arc::new(Self::Local(
            core,
            Arc::new(EmbeddedServices {
                control: ServiceControl::new(directory),
                _directory: None,
            }),
        ))
    }
    pub async fn remote_control(&self, call: Value) -> Result<Value, String> {
        self.call("remote_control", call).await
    }
    async fn connect(&self, method: &str, args: Value) -> Result<TcpStream, String> {
        let directory = match self {
            Self::Remote(directory) => directory,
            #[cfg(any(test, feature = "review-acceptance"))]
            Self::Local(..) => return Err("not a remote client".into()),
        };
        let endpoint: Endpoint = serde_json::from_slice(
            &std::fs::read(directory.join("service.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if endpoint.version != VERSION || !endpoint.address.ip().is_loopback() {
            return Err("incompatible local service".into());
        }
        let mut stream = TcpStream::connect(endpoint.address)
            .await
            .map_err(|e| e.to_string())?;
        write_frame(
            &mut stream,
            &Request {
                version: VERSION,
                token: endpoint.token,
                method: method.into(),
                args,
            },
        )
        .await?;
        Ok(stream)
    }
    async fn call<T: DeserializeOwned>(&self, method: &str, args: Value) -> Result<T, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, services) = self {
            return serde_json::from_value(
                dispatch_service(core, &services.control, method, args).await?,
            )
            .map_err(|e| e.to_string());
        }
        // Never retry a mutation: its reply may have been lost after committing.
        tokio::time::timeout(Duration::from_mins(3), async {
            let mut stream = self.connect(method, args).await?;
            let result: Result<T, String> = read_frame(&mut stream).await?;
            result
        })
        .await
        .map_err(|_| "后台服务响应超时，请重新连接检查结果；未自动重发操作".to_string())?
    }
    pub(crate) async fn review<T: DeserializeOwned>(
        &self,
        request: crate::review::service::ReviewCall,
    ) -> Result<T, String> {
        self.call("review", json!(request)).await
    }
    pub async fn submission_intents(
        &self,
        call: agent_protocol::submission::SubmissionIntentCall,
    ) -> Result<Value, String> {
        self.call("submission_intents", json!(call)).await
    }
    pub async fn attachment_save(
        &self,
        request: agent_protocol::AttachmentUploadRequest,
        encoded: String,
    ) -> Result<agent_domain::PromptAttachment, String> {
        self.call("attachment_save", json!((request, encoded)))
            .await
    }
    pub async fn attachment_receipt(
        &self,
        request: agent_protocol::AttachmentUploadRequest,
    ) -> Result<Option<agent_domain::PromptAttachment>, String> {
        self.call("attachment_receipt", json!(request)).await
    }
    pub async fn attachment_read(&self, path: String) -> Result<String, String> {
        self.call("attachment_read", json!(path)).await
    }
    pub async fn project_runs(
        &self,
        task_id: &str,
    ) -> Result<crate::project_runs::RunCatalog, String> {
        self.call("project_runs", json!(task_id)).await
    }
    pub async fn project_runs_analyze(
        &self,
        request: crate::project_runs::AnalyzeRequest,
    ) -> Result<crate::project_runs::RunCatalog, crate::project_runs::MutationError> {
        self.call("project_runs_analyze", json!(request))
            .await
            .map_err(crate::project_runs::MutationError::from)?
    }
    pub async fn project_run_edit(
        &self,
        request: crate::project_runs::EditRequest,
    ) -> Result<crate::project_runs::RunCatalog, crate::project_runs::MutationError> {
        self.call("project_run_edit", json!(request))
            .await
            .map_err(crate::project_runs::MutationError::from)?
    }
    pub async fn project_run_operation(
        &self,
        task_id: &str,
        operation_id: &str,
    ) -> Result<Option<crate::project_runs::RunOperation>, String> {
        self.call("project_run_operation", json!((task_id, operation_id)))
            .await
    }
    pub async fn project_run_open(
        &self,
        request: crate::project_runs::LaunchRequest,
    ) -> Result<crate::terminal_host::TerminalInfo, String> {
        self.call("project_run_open", json!(request)).await
    }
    pub async fn terminal_start(
        &self,
        intent: crate::terminal_host::LaunchIntent,
    ) -> Result<crate::terminal_host::TerminalInfo, String> {
        self.call("terminal_start", json!(intent)).await
    }
    pub async fn terminal_get(
        &self,
        id: &str,
    ) -> Result<crate::terminal_host::TerminalInfo, String> {
        self.call("terminal_get", json!(id)).await
    }
    pub async fn terminal_list(
        &self,
        project_id: &str,
    ) -> Result<Vec<crate::terminal_host::TerminalInfo>, String> {
        self.call("terminal_list", json!(project_id)).await
    }
    pub async fn terminal_read(
        &self,
        id: &str,
        after: u64,
    ) -> Result<crate::terminal_host::OutputBatch, String> {
        self.call("terminal_read", json!((id, after))).await
    }
    pub async fn terminal_input(&self, window: &str, request: Value) -> Result<Value, String> {
        // A busy old Core remains reachable for approvals/output. Never fall
        // back to its unsequenced writer when the new window protocol is absent.
        let caps: Value = self.call("ping", json!([])).await?;
        if caps["terminalInputWindowProtocol"] != 1 {
            return Err("后台仍在运行旧版本，当前输入已暂停；输出仍可查看，请在任务安全结束后重新打开应用。".into());
        }
        self.call("terminal_input", json!((window, request))).await
    }
    #[cfg(test)]
    async fn terminal_write(&self, id: &str, bytes: Vec<u8>) -> Result<(), String> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let info = self.terminal_get(id).await?;
        let source = self
            .terminal_input(
                "main",
                json!({"type":"inspect","terminalId":id,"expectedEpoch":info.epoch}),
            )
            .await?;
        let stream = uuid::Uuid::new_v4().to_string();
        let opened=self.terminal_input("main",json!({"type":"open","terminalId":id,"expectedEpoch":info.epoch,"streamId":stream,"expectedRevision":source["revision"]})).await?;
        if opened["state"] != "ready" {
            return Err("test terminal input not ready".into());
        }
        let applied=self.terminal_input("main",json!({"type":"write","terminalId":id,"expectedEpoch":info.epoch,"streamId":stream,"revision":opened["revision"],"sequence":"1","data":STANDARD.encode(bytes)})).await?;
        if applied["state"] != "applied" {
            return Err("test input was not confirmed".into());
        }
        Ok(())
    }
    pub async fn terminal_resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        self.call("terminal_resize", json!((id, cols, rows))).await
    }
    pub async fn terminal_stop(
        &self,
        id: &str,
    ) -> Result<crate::terminal_host::TerminalInfo, String> {
        self.call("terminal_stop", json!(id)).await
    }
    pub async fn subagent_history(
        &self,
        task_id: &TaskId,
        item_id: &str,
    ) -> Result<Vec<Value>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .subagent_history(task_id, item_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("subagent_history", json!((task_id, item_id)))
            .await
    }
    pub async fn scan_agent(&self, provider_id: &str) -> Result<BootstrapSnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return rescan_agent(core, provider_id).await;
        }
        self.call("scan_agent", json!(provider_id)).await
    }
    pub async fn bootstrap(&self) -> Result<BootstrapSnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.bootstrap().await.map_err(|e| e.to_string());
        }
        self.call("bootstrap", json!(())).await
    }
    pub async fn history_preferences(
        &self,
    ) -> Result<agent_protocol::HistoryPreferencesSnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.history_preferences().await.map_err(|e| e.to_string());
        }
        self.call("history_preferences", json!(())).await
    }
    pub async fn agent_preferences(
        &self,
    ) -> Result<agent_protocol::AgentPreferencesSnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.agent_preferences().await.map_err(|e| e.to_string());
        }
        self.call("agent_preferences", json!(())).await
    }
    pub async fn execute(
        &self,
        envelope: CommandEnvelope<CoreCommand>,
    ) -> Result<CommandAck, String> {
        self.call("execute", json!((envelope,))).await
    }
    pub async fn review_repair(
        &self,
        call: crate::review_repairs::RepairCall,
    ) -> Result<Value, String> {
        self.call("review_repair", json!((call,))).await
    }
    pub async fn register_project(
        &self,
        request: crate::project_registry::ProjectRegistration,
    ) -> Result<crate::project_registry::Registered, String> {
        self.call("register_project", json!((request,))).await
    }
    pub async fn update_project(
        &self,
        request: crate::project_registry::ProjectEdit,
    ) -> Result<(), String> {
        self.call("update_project", json!((request,))).await
    }
    pub async fn task_deletion(
        &self,
        task: TaskId,
    ) -> Result<Option<agent_core::recovery::TaskDeletion>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.task_deletion(&task).await.map_err(|e| e.to_string());
        }
        self.call("task_deletion", json!(task)).await
    }
    pub async fn invalidate_pending_command(
        &self,
        envelope: CommandEnvelope<CoreCommand>,
    ) -> Result<agent_core::command_invalidation::CommandInvalidation, String> {
        self.call("invalidate_pending_command", json!((envelope,)))
            .await
    }
    pub async fn command_receipt(
        &self,
        envelope: CommandEnvelope<CoreCommand>,
    ) -> Result<Option<CommandAck>, String> {
        self.call("command_receipt", json!((envelope,))).await
    }
    pub async fn invalidate_approval_response(
        &self,
        query: agent_core::approval_receipts::ApprovalReceiptQuery,
        original_version: u64,
    ) -> Result<agent_core::approval_receipts::ApprovalInvalidation, String> {
        self.call(
            "invalidate_approval_response",
            json!((query, original_version)),
        )
        .await
    }
    pub async fn prepare_approval_response(
        &self,
        envelope: CommandEnvelope<CoreCommand>,
    ) -> Result<agent_core::approval_receipts::ApprovalReceiptQuery, String> {
        self.call("prepare_approval_response", json!((envelope,)))
            .await
    }
    pub async fn approval_receipt(
        &self,
        query: agent_core::approval_receipts::ApprovalReceiptQuery,
    ) -> Result<Option<agent_core::approval_receipts::ApprovalReceipt>, String> {
        self.call("approval_receipt", json!((query,))).await
    }
    pub async fn sync_events(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .sync_events(after_sequence, limit)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("sync_events", json!((after_sequence, limit,)))
            .await
    }
    pub async fn timeline(&self, task_id: &TaskId) -> Result<Vec<TimelineItem>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.timeline(task_id).await.map_err(|e| e.to_string());
        }
        self.call("timeline", json!((task_id,))).await
    }
    pub async fn timeline_snapshot(
        &self,
        task_id: &TaskId,
    ) -> Result<agent_protocol::TimelineSnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .timeline_snapshot(task_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("timeline_snapshot", json!((task_id,))).await
    }

    pub async fn pending_approvals(
        &self,
        task_id: &TaskId,
        limit: usize,
    ) -> Result<Vec<PendingApproval>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .pending_approvals(task_id, limit)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("pending_approvals", json!((task_id, limit,)))
            .await
    }
    pub async fn workspace_snapshots(
        &self,
        task_id: &TaskId,
        limit: usize,
    ) -> Result<Vec<WorkspaceSnapshotSummary>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .workspace_snapshots(task_id, limit)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("workspace_snapshots", json!((task_id, limit,)))
            .await
    }
    pub async fn build_context_preview(
        &self,
        request: ContextPreviewRequest,
    ) -> Result<BuiltContextPackage, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .build_context_preview(request)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("build_context_preview", json!((request,))).await
    }
    pub async fn build_edited_context_preview(
        &self,
        edit: ContextEditRequest,
    ) -> Result<BuiltContextPackage, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .build_edited_context_preview(edit)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("build_edited_context_preview", json!((edit,)))
            .await
    }
    pub async fn query_history(&self, query: &HistoryQuery) -> Result<HistoryPage, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.query_history(query).await.map_err(|e| e.to_string());
        }
        self.call("query_history", json!((query,))).await
    }
    pub async fn recovery_snapshot(&self, actor: &ActorRef) -> Result<RecoverySnapshot, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .recovery_snapshot(actor)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("recovery_snapshot", json!((actor,))).await
    }
    pub async fn prepare_session_rebuild(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        expected_version: u64,
    ) -> Result<(), String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .prepare_session_rebuild(actor, task_id, expected_version)
                .await
                .map_err(|e| e.to_string());
        }
        self.call(
            "prepare_session_rebuild",
            json!((actor, task_id, expected_version,)),
        )
        .await
    }
    pub async fn task_artifacts(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .task_artifacts(actor, task_id, before, limit)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("task_artifacts", json!((actor, task_id, before, limit,)))
            .await
    }
    pub async fn read_artifact(
        &self,
        actor: &ActorRef,
        request: &ArtifactReadRequest,
    ) -> Result<ArtifactChunk, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .read_artifact(actor, request)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("read_artifact", json!((actor, request,))).await
    }
    pub async fn verified_local_artifact(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        artifact_id: &ArtifactId,
        expected_sha256: &str,
    ) -> Result<StoredArtifact, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .verified_local_artifact(actor, task_id, artifact_id, expected_sha256)
                .await
                .map_err(|e| e.to_string());
        }
        self.call(
            "verified_local_artifact",
            json!((actor, task_id, artifact_id, expected_sha256,)),
        )
        .await
    }
    pub async fn permission_catalog(
        &self,
        executor: &ExecutorRef,
        task_id: Option<&TaskId>,
    ) -> Result<PermissionCatalog, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .permission_catalog(executor, task_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("permission_catalog", json!((executor, task_id,)))
            .await
    }
    pub async fn model_catalog_for_task(
        &self,
        executor: &ExecutorRef,
        model: Option<&str>,
        task_id: Option<&TaskId>,
    ) -> Result<ModelCatalog, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .model_catalog_for_task(executor, model, task_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("model_catalog_for_task", json!((executor, model, task_id,)))
            .await
    }
    pub async fn review_executors(&self) -> Result<Vec<ExecutorRef>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.review_executors().await.map_err(|e| e.to_string());
        }
        self.call("review_executors", json!([])).await
    }
    pub async fn fork_receipt(
        &self,
        source: TaskId,
        target: TaskId,
    ) -> Result<Option<agent_core::task_fork::ForkReceipt>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .fork_receipt(&source, &target)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("fork_receipt", json!((source, target))).await
    }
    pub async fn fork_task(
        &self,
        request: agent_core::task_fork::ForkRequest,
    ) -> Result<TaskId, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.fork_task(request).await.map_err(|e| e.to_string());
        }
        self.call("fork_task", json!((request,))).await
    }
    pub async fn turn_reviews(&self, task_id: &TaskId) -> Result<Vec<ReviewRun>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.turn_reviews(task_id).await.map_err(|e| e.to_string());
        }
        self.call("turn_reviews", json!((task_id,))).await
    }
    pub async fn review_source(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        file_id: &str,
    ) -> Result<agent_core::review_analysis::ReviewSource, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .review_source(task_id, run_id, file_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("review_source", json!((task_id, run_id, file_id)))
            .await
    }
    pub async fn start_turn_review(&self, request: ReviewRequest) -> Result<ReviewRun, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .start_turn_review(request)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("start_turn_review", json!((request,))).await
    }
    pub async fn start_project_review(&self, request: ReviewRequest) -> Result<ReviewRun, String> {
        self.review(crate::review::service::ReviewCall::StartProjectReview(
            request,
        ))
        .await
    }
    pub async fn cancel_turn_review(&self, task_id: &TaskId, run_id: &RunId) -> Result<(), String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .cancel_turn_review(task_id, run_id)
                .await
                .map_err(|e| e.to_string());
        }
        self.call("cancel_turn_review", json!((task_id, run_id)))
            .await
    }
    pub async fn analyze_project_commands(
        &self,
        executor: ExecutorRef,
        packet: String,
    ) -> Result<String, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.analyze_project_commands(executor, packet).await;
        }
        self.call("analyze_project_commands", json!((executor, packet)))
            .await
    }
    pub async fn summarize_commit(
        &self,
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    ) -> Result<String, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .summarize_commit(executor, changes, language.as_deref())
                .await;
        }
        self.call("summarize_commit", json!((executor, changes, language)))
            .await
    }
    pub async fn registered_executors(&self) -> Result<Vec<ExecutorRef>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return Ok(core.registered_executors().await);
        }
        self.call("registered_executors", json!(())).await
    }
    pub async fn set_running_permission_mode(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        executor: &ExecutorRef,
        mode: &str,
    ) -> Result<(), String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .set_running_permission_mode(task_id, run_id, executor, mode)
                .await
                .map_err(|e| e.to_string());
        }
        self.call(
            "set_running_permission_mode",
            json!((task_id, run_id, executor, mode)),
        )
        .await
    }
    pub async fn quota_snapshots_for(
        &self,
        task_id: Option<&TaskId>,
        installation_id: Option<&str>,
        force: bool,
    ) -> Result<Vec<QuotaSnapshotView>, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return Ok(core
                .quota_snapshots_with_refresh(task_id, installation_id, force)
                .await);
        }
        self.call(
            "quota_snapshots_for",
            json!((task_id, installation_id, force)),
        )
        .await
    }

    pub async fn try_workspace_mutation(&self) -> Result<WorkspaceLease, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core
                .try_workspace_mutation()
                .map(WorkspaceLease::Local)
                .map_err(|e| e.to_string());
        }
        let mut stream = self.connect("workspace_lease", json!([])).await?;
        let reply: Result<(), String> = read_frame(&mut stream).await?;
        reply?;
        Ok(WorkspaceLease::Remote(stream))
    }
    /// Events of one project plus host-wide streams, for clients scoped to a project.
    pub async fn subscribe_project(
        &self,
        after: u64,
        project: &agent_domain::ProjectId,
    ) -> Result<agent_core::EventSubscription, String> {
        self.event_stream("subscribe_project", json!((after, project)))
            .await
    }
    pub async fn subscribe(&self, after: u64) -> Result<agent_core::EventSubscription, String> {
        #[cfg(any(test, feature = "review-acceptance"))]
        if let Self::Local(core, _) = self {
            return core.subscribe(after).await.map_err(|e| e.to_string());
        }
        self.event_stream("subscribe", json!(after)).await
    }
    async fn event_stream(
        &self,
        method: &str,
        args: Value,
    ) -> Result<agent_core::EventSubscription, String> {
        let mut stream = self.connect(method, args).await?;
        let replay: Result<Vec<EventEnvelope>, String> = read_frame(&mut stream).await?;
        let replay = replay?;
        let (sender, live) = tokio::sync::broadcast::channel(1024);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = sender.closed() => break,
                    result = read_frame::<EventEnvelope>(&mut stream) => match result {
                        Ok(event) => { if sender.send(event).is_err() { break; } },
                        Err(_) => break,
                    }
                }
            }
        });
        Ok(agent_core::EventSubscription { replay, live })
    }
}
// Closing the connection releases the service-owned lock, including desktop SIGKILL.
#[allow(dead_code)]
pub enum WorkspaceLease {
    Remote(TcpStream),
    #[cfg(any(test, feature = "review-acceptance"))]
    Local(tokio::sync::OwnedRwLockWriteGuard<()>),
}
async fn rescan_agent(core: &Arc<Core>, provider_id: &str) -> Result<BootstrapSnapshot, String> {
    let source = BUILTIN_AGENT_MANIFESTS
        .iter()
        .find(|source| {
            ProviderManifest::parse_toml(source).is_ok_and(|m| m.id.as_str() == provider_id)
        })
        .ok_or("unsupported agent")?;
    let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
    if snapshot
        .tasks
        .iter()
        .any(|task| task.active_primary_run.is_some())
    {
        return Err("请等待当前对话完成后重新检测 Agent".into());
    }
    scan_builtin_agent(Arc::clone(core), source).await?;
    core.bootstrap().await.map_err(|e| e.to_string())
}

async fn dispatch(core: &Arc<Core>, method: &str, args: Value) -> Result<Value, String> {
    match method {
        "scan_agent" => {
            let provider_id: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(rescan_agent(core, &provider_id).await?).map_err(|e| e.to_string())
        }
        "subagent_history" => {
            let (task_id, item_id): (TaskId, String) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.subagent_history(&task_id, &item_id)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "ping" => {
            Ok(json!({"version":VERSION,"pid":std::process::id(),"terminalInputWindowProtocol":1}))
        }
        "bootstrap" => {
            let (): () = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core.bootstrap().await.map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "history_preferences" => {
            let (): () = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.history_preferences()
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "task_deletion" => {
            let task: TaskId = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(core.task_deletion(&task).await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "agent_preferences" => {
            let (): () = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(core.agent_preferences().await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "invalidate_approval_response" => {
            let (query, original_version): (
                agent_core::approval_receipts::ApprovalReceiptQuery,
                u64,
            ) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.invalidate_approval_response(&query, original_version)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "approval_receipt" => {
            let (query,): (agent_core::approval_receipts::ApprovalReceiptQuery,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.approval_receipt(&query)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "execute"
        | "command_receipt"
        | "prepare_approval_response"
        | "invalidate_pending_command" => {
            let (envelope,): (CommandEnvelope<CoreCommand>,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let mut envelope = envelope;
            if matches!(
                &envelope.payload,
                CoreCommand::RecordTestResult(_)
                    | CoreCommand::RecordInstallationProbe(_)
                    | CoreCommand::ReconcileMissingInstallation(_)
            ) {
                return Err("internal command unavailable over desktop RPC".into());
            }
            envelope.actor = ActorRef::LocalUser;
            if method == "invalidate_pending_command" {
                return serde_json::to_value(
                    core.invalidate_pending_command(&envelope)
                        .await
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string());
            }
            if method == "prepare_approval_response" {
                return serde_json::to_value(
                    core.prepare_approval_response(&envelope)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string());
            }
            if method == "command_receipt" {
                return serde_json::to_value(
                    core.command_receipt(&envelope)
                        .await
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string());
            }
            let result = core.execute(envelope).await.map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "sync_events" => {
            let (after_sequence, limit): (u64, usize) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .sync_events(after_sequence, limit)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "timeline" => {
            let (task_id,): (TaskId,) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core.timeline(&task_id).await.map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "timeline_snapshot" => {
            let (task_id,): (TaskId,) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .timeline_snapshot(&task_id)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "pending_approvals" => {
            let (task_id, limit): (TaskId, usize) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .pending_approvals(&task_id, limit)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "workspace_snapshots" => {
            let (task_id, limit): (TaskId, usize) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .workspace_snapshots(&task_id, limit)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "build_context_preview" => {
            let (request,): (ContextPreviewRequest,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .build_context_preview(request)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "build_edited_context_preview" => {
            let (edit,): (ContextEditRequest,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .build_edited_context_preview(edit)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "query_history" => {
            let (query,): (HistoryQuery,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .query_history(&query)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "recovery_snapshot" => {
            let (actor,): (ActorRef,) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .recovery_snapshot(&actor)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "prepare_session_rebuild" => {
            let (actor, task_id, expected_version): (ActorRef, TaskId, u64) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            core.prepare_session_rebuild(&actor, &task_id, expected_version)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(()).map_err(|e| e.to_string())
        }
        "task_artifacts" => {
            let (actor, task_id, before, limit): (ActorRef, TaskId, Option<ArtifactCursor>, usize) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .task_artifacts(&actor, &task_id, before.as_ref(), limit)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "read_artifact" => {
            let (actor, request): (ActorRef, ArtifactReadRequest) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .read_artifact(&actor, &request)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "verified_local_artifact" => {
            let (actor, task_id, artifact_id, expected_sha256): (
                ActorRef,
                TaskId,
                ArtifactId,
                String,
            ) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .verified_local_artifact(&actor, &task_id, &artifact_id, &expected_sha256)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "permission_catalog" => {
            let (executor, task_id): (ExecutorRef, Option<TaskId>) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .permission_catalog(&executor, task_id.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "model_catalog_for_task" => {
            let (executor, model, task_id): (ExecutorRef, Option<String>, Option<TaskId>) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .model_catalog_for_task(&executor, model.as_deref(), task_id.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "review_executors" => {
            serde_json::to_value(core.review_executors().await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "fork_receipt" => {
            let (source, target) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.fork_receipt(&source, &target)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "fork_task" => {
            let (request,): (agent_core::task_fork::ForkRequest,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(core.fork_task(request).await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "turn_reviews" => {
            let (task_id,): (TaskId,) = serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.turn_reviews(&task_id)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "review_source" => {
            let (task_id, run_id, file_id): (TaskId, RunId, String) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.review_source(&task_id, &run_id, &file_id)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "start_turn_review" => {
            let (request,): (ReviewRequest,) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(
                core.start_turn_review(request)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        }
        "cancel_turn_review" => {
            let (task_id, run_id): (TaskId, RunId) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            core.cancel_turn_review(&task_id, &run_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "analyze_project_commands" => {
            let (executor, packet): (ExecutorRef, String) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            serde_json::to_value(core.analyze_project_commands(executor, packet).await?)
                .map_err(|e| e.to_string())
        }
        "summarize_commit" => {
            let (executor, changes, language): (ExecutorRef, String, Option<String>) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .summarize_commit(executor, changes, language.as_deref())
                .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "registered_executors" => {
            let (): () = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core.registered_executors().await;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "set_running_permission_mode" => {
            let (task_id, run_id, executor, mode): (TaskId, RunId, ExecutorRef, String) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            core.set_running_permission_mode(&task_id, &run_id, &executor, &mode)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "quota_snapshots_for" => {
            let (task_id, installation_id, force): (Option<TaskId>, Option<String>, bool) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let result = core
                .quota_snapshots_with_refresh(task_id.as_ref(), installation_id.as_deref(), force)
                .await;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        _ => Err("unknown service method".into()),
    }
}
async fn write_frame<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("service frame exceeds limit".into());
    }
    stream
        .write_u32(u32::try_from(bytes.len()).map_err(|_| "service frame exceeds limit")?)
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(&bytes).await.map_err(|e| e.to_string())
}
async fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> Result<T, String> {
    let length = stream.read_u32().await.map_err(|e| e.to_string())? as usize;
    if length > MAX_FRAME {
        return Err("service frame exceeds limit".into());
    }
    let mut bytes = vec![0; length];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
struct ServiceControl {
    remote: Arc<remote::RemoteHost>,
    submissions: crate::submission_intents::SubmissionIntents,
    attachments: Arc<crate::attachment_service::AttachmentStore>,
    review: crate::review::ReviewHost,
    terminals: crate::terminal_host::TerminalHost,
    project_runs: crate::project_runs::ProjectRunHost,
    workspace_watchers: crate::workspace_service::watch::WatchService,
    // Serializes project registration/editing across all clients.
    projects: tokio::sync::Mutex<()>,
    repairs: crate::review_repairs::ReviewRepairs,
    startup_work: startup::Work,
    requests: tokio::sync::RwLock<()>,
    stopping: std::sync::atomic::AtomicBool,
    shutdown: tokio::sync::Notify,
}
impl ServiceControl {
    fn new(directory: &Path) -> Self {
        let review = crate::review::ReviewHost::new(directory.join("review"));
        let changes = review.changes.source();
        let mut project_runs =
            crate::project_runs::ProjectRunHost::new(directory.join("project-runs"));
        project_runs.changes = changes.clone();
        Self {
            remote: Arc::new(remote::RemoteHost::new(directory.join("remote"))),
            submissions: crate::submission_intents::SubmissionIntents::new(
                directory.join("submission-intents"),
            ),
            attachments: Arc::new(crate::attachment_service::AttachmentStore::new(
                directory.join("attachments"),
            )),
            review,
            terminals: crate::terminal_host::TerminalHost::persistent(
                directory.join("terminal-operations"),
            )
            .with_changes(changes),
            project_runs,
            workspace_watchers: crate::workspace_service::watch::WatchService::default(),
            projects: tokio::sync::Mutex::default(),
            repairs: crate::review_repairs::ReviewRepairs::new(directory.join("review-repairs")),
            startup_work: startup::Work::default(),
            requests: tokio::sync::RwLock::default(),
            stopping: std::sync::atomic::AtomicBool::default(),
            shutdown: tokio::sync::Notify::default(),
        }
    }
}
async fn dispatch_service(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    if method == "submission_intents" {
        let call = serde_json::from_value(args).map_err(|e| e.to_string())?;
        return control.submissions.call(core, call).await;
    }
    if method.starts_with("attachment_") {
        return crate::attachment_service::dispatch(control.attachments.clone(), method, args)
            .await;
    }
    if method == "review" {
        let request = serde_json::from_value(args).map_err(|e| e.to_string())?;
        return crate::review::service::dispatch(core, &control.review, request).await;
    }
    if method == "review_repair" {
        let (call,): (crate::review_repairs::RepairCall,) =
            serde_json::from_value(args).map_err(|e| e.to_string())?;
        return control.repairs.call(core, call).await;
    }
    if method == "register_project" {
        let (request,): (crate::project_registry::ProjectRegistration,) =
            serde_json::from_value(args).map_err(|e| e.to_string())?;
        let registered = {
            let _projects = control.projects.lock().await;
            crate::project_registry::register(core, request).await?
        };
        if registered.created {
            // Registration already committed; analysis failures are reported by the run catalogue.
            let _ = crate::project_runs::schedule(
                &control.project_runs,
                core.clone(),
                registered.project_id.clone(),
            )
            .await;
        }
        return serde_json::to_value(registered).map_err(|e| e.to_string());
    }
    if method == "update_project" {
        let (request,): (crate::project_registry::ProjectEdit,) =
            serde_json::from_value(args).map_err(|e| e.to_string())?;
        let project = request.project_id.clone();
        {
            let _projects = control.projects.lock().await;
            // Same gate as frozen snapshot reads/writes: scope cannot change mid-operation.
            let _scope = control.review.gate.lock().await;
            crate::project_registry::update(core, request).await?;
        }
        let _ = crate::project_runs::schedule(&control.project_runs, core.clone(), project).await;
        return Ok(Value::Null);
    }
    // Project scope changes and deletion use the same gate as frozen snapshot
    // reads/writes, so a checked scope cannot change midway through an operation.
    let mut scope_guard = None;
    if method == "execute" {
        let (envelope,): (CommandEnvelope<CoreCommand>,) =
            serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        if matches!(&envelope.payload, CoreCommand::DeleteTask(_)) {
            let mut envelope = envelope;
            // This boundary is the desktop RPC; remote callers use their own
            // authenticated actor and call the shared cleanup service directly.
            envelope.actor = ActorRef::LocalUser;
            return execute_task_delete(core, control, envelope).await;
        }
        if matches!(envelope.payload, CoreCommand::UpdateProject(_)) {
            scope_guard = Some(control.review.gate.lock().await);
        }
    }
    if matches!(
        method,
        "workspace_roots" | "workspace_directory" | "workspace_preview" | "workspace_find_files"
    ) {
        return crate::workspace_service::dispatch(core, method, args).await;
    }
    if method.starts_with("terminal_") {
        return crate::terminal_host::rpc::dispatch(core, &control.terminals, method, args).await;
    }
    if method.starts_with("project_run") {
        return crate::project_runs::dispatch(
            core,
            &control.project_runs,
            &control.terminals,
            method,
            args,
        )
        .await;
    }
    let changed_project = if method == "execute" {
        serde_json::from_value::<(CommandEnvelope<CoreCommand>,)>(args.clone())
            .ok()
            .and_then(|(envelope,)| match envelope.payload {
                CoreCommand::RegisterProject(project) | CoreCommand::UpdateProject(project) => {
                    Some(project.project_id)
                }
                _ => None,
            })
    } else {
        None
    };
    let result = dispatch(core, method, args).await?;
    drop(scope_guard);
    if let Some(project) = changed_project {
        // Registration already committed. Analysis failures are reported by the
        // run catalogue and must not turn a successful registration into failure.
        let _ = crate::project_runs::schedule(&control.project_runs, core.clone(), project).await;
    }
    Ok(result)
}

async fn execute_task_delete(
    core: &Arc<Core>,
    control: &ServiceControl,
    envelope: CommandEnvelope<CoreCommand>,
) -> Result<Value, String> {
    let CoreCommand::DeleteTask(command) = &envelope.payload else {
        return Err("task deletion required".into());
    };
    let task = command.task_id.clone();
    let _guard = control.review.gate.lock().await;
    let result = core
        .execute(envelope)
        .await
        .map_err(|error| error.to_string());
    if result.is_ok()
        || core
            .bootstrap()
            .await
            .is_ok_and(|snapshot| !snapshot.tasks.iter().any(|current| current.task_id == task))
    {
        control.review.forget_task(task.as_str())?;
        control.repairs.forget_task(task.as_str())?;
        control
            .submissions
            .reconcile_deleted(core, Some(&task))
            .await?;
    }
    serde_json::to_value(result?).map_err(|error| error.to_string())
}

fn executable_build() -> Result<String, String> {
    let metadata = std::fs::metadata(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}
async fn serve_connection(
    core: Arc<Core>,
    mut stream: TcpStream,
    token: Arc<String>,
    control: Arc<ServiceControl>,
) -> Result<(), String> {
    let request: Request = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await
        .map_err(|_| "request timeout")??;
    if request.version != VERSION || request.token != *token {
        return Err("unauthorized service connection".into());
    }
    if request.method == "remote_control" {
        let result = control
            .remote
            .clone()
            .call(&core, &control, request.args)
            .await;
        return write_frame(&mut stream, &result).await;
    }
    // Only classified methods are served; see access.rs.
    if access::method_access(&request.method).is_none() {
        return write_frame(
            &mut stream,
            &Err::<Value, _>("unknown service method".to_string()),
        )
        .await;
    }
    if request.method == "retire_if_idle" {
        // Exclude new commands while checking execution and workspace ownership.
        let _requests = control.requests.write().await;
        let workspace = core.try_workspace_mutation();
        let idle = !control.startup_work.active()
            && workspace.is_ok()
            && !control.terminals.has_active()
            && !control.project_runs.has_active()
            && !control.remote.has_active_sessions()
            && !core.has_active_reviews().await
            && !core
                .bootstrap()
                .await
                .map_err(|e| e.to_string())?
                .tasks
                .iter()
                .any(|t| t.active_primary_run.is_some());
        if idle {
            control
                .stopping
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let result = write_frame(&mut stream, &Ok::<_, String>(idle)).await;
        if idle {
            control.shutdown.notify_one();
        }
        return result;
    }
    let request_guard = control.requests.read().await;
    if control.stopping.load(std::sync::atomic::Ordering::Acquire) {
        return Err("service is restarting while idle".into());
    }
    if request.method == "workspace_lease" {
        let guard = core.try_workspace_mutation().map_err(|e| e.to_string());
        write_frame(
            &mut stream,
            &guard.as_ref().map(|_| ()).map_err(Clone::clone),
        )
        .await?;
        let _guard = guard?;
        drop(request_guard);
        let _ = stream.read_u8().await;
        return Ok(());
    }
    if request.method == "submission_watch" {
        drop(request_guard);
        return review_watch::serve_submissions(&control.submissions, &mut stream, request.args)
            .await;
    }
    if matches!(
        request.method.as_str(),
        "review_watch" | "project_runs_watch"
    ) {
        drop(request_guard);
        return review_watch::serve(&core, &control.review.changes, &mut stream, request.args)
            .await;
    }
    if request.method == "workspace_watch" {
        drop(request_guard);
        return workspace::serve_watch(
            &core,
            &control.workspace_watchers,
            &mut stream,
            request.args,
        )
        .await;
    }
    if request.method == "subscribe" {
        let after: u64 = serde_json::from_value(request.args).map_err(|e| e.to_string())?;
        let subscription = core.subscribe(after).await.map_err(|e| e.to_string())?;
        drop(request_guard);
        return stream_events(&core, &mut stream, after, subscription, |_| true).await;
    }
    if request.method == "subscribe_project" {
        // A remote client sees one project's tasks plus host-wide streams
        // (installations, quotas, shared settings), never other projects.
        let (after, project): (u64, agent_domain::ProjectId) =
            serde_json::from_value(request.args).map_err(|e| e.to_string())?;
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        if !snapshot.projects.iter().any(|p| p.project_id == project) {
            drop(request_guard);
            return write_frame(
                &mut stream,
                &Err::<Vec<EventEnvelope>, _>("项目不存在".to_string()),
            )
            .await;
        }
        let mut tasks: std::collections::HashSet<String> = snapshot
            .tasks
            .iter()
            .filter(|task| task.project_id == project)
            .map(|task| task.task_id.to_string())
            .collect();
        let subscription = core.subscribe(after).await.map_err(|e| e.to_string())?;
        drop(request_guard);
        return stream_events(&core, &mut stream, after, subscription, move |event| {
            project_scoped(event, &project, &mut tasks)
        })
        .await;
    }
    // A disconnected UI must not cancel a command already accepted by the service.
    let result = dispatch_service(&core, &control, &request.method, request.args).await;
    write_frame(&mut stream, &result).await
}

/// Whether a project-scoped subscriber may see this event. Tasks created in the
/// project after subscribing join the scope through their creation event.
fn project_scoped(
    event: &EventEnvelope,
    project: &agent_domain::ProjectId,
    tasks: &mut std::collections::HashSet<String>,
) -> bool {
    match event.stream.stream_type {
        agent_protocol::StreamType::Project => event.stream.stream_id == project.as_str(),
        agent_protocol::StreamType::Task => {
            if event.event_type == "task.created"
                && event.payload["projectId"].as_str() == Some(project.as_str())
            {
                tasks.insert(event.stream.stream_id.clone());
            }
            tasks.contains(&event.stream.stream_id)
        }
        _ => true,
    }
}

/// Replays then follows events, recovering from a lagging broadcast through the
/// durable log. Filtered streams have sequence gaps; the last delivered
/// sequence remains a valid resume cursor.
async fn stream_events(
    core: &Arc<Core>,
    stream: &mut TcpStream,
    after: u64,
    mut subscription: agent_core::EventSubscription,
    mut keep: impl FnMut(&EventEnvelope) -> bool,
) -> Result<(), String> {
    let mut cursor = subscription.replay.last().map_or(after, |e| e.sequence);
    let replay: Vec<_> = std::mem::take(&mut subscription.replay)
        .into_iter()
        .filter(|event| keep(event))
        .collect();
    write_frame(stream, &Ok::<_, String>(replay)).await?;
    loop {
        tokio::select! {
            _ = stream.read_u8() => return Ok(()),
            event = subscription.live.recv() => match event {
                Ok(event) => {
                    if event.sequence > cursor {
                        cursor = event.sequence;
                        if keep(&event) { write_frame(stream, &event).await?; }
                    }
                },
                Err(RecvError::Lagged(_)) => {
                    loop {
                        let events = core.sync_events(cursor, 1000).await.map_err(|e| e.to_string())?;
                        let count = events.len();
                        for event in events {
                            cursor = event.sequence;
                            if keep(&event) { write_frame(stream, &event).await?; }
                        }
                        if count < 1000 { break; }
                    }
                },
                Err(RecvError::Closed) => return Ok(()),
            }
        }
    }
}

// Record stage names and timings only; never endpoint tokens or conversation data.
pub(crate) struct StartupTiming {
    directory: PathBuf,
    stage: &'static str,
    started: std::time::Instant,
}
impl StartupTiming {
    pub(crate) fn new(directory: &Path, stage: &'static str) -> Self {
        let timing = Self {
            directory: directory.to_owned(),
            stage,
            started: std::time::Instant::now(),
        };
        timing.record("begin", 0);
        timing
    }
    fn record(&self, event: &str, elapsed_ms: u128) {
        use std::io::Write;
        crate::service_diagnostics::record(
            event,
            &format!("stage={} elapsed_ms={elapsed_ms}", self.stage),
        );
        let path = self.directory.join("startup-timing.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            if file
                .metadata()
                .is_ok_and(|metadata| metadata.len() > 256 * 1024)
            {
                let _ = file.set_len(0);
            }
            let _ = writeln!(
                file,
                "{} pid={} stage={} event={} elapsed_ms={}",
                chrono::Utc::now().to_rfc3339(),
                std::process::id(),
                self.stage,
                event,
                elapsed_ms
            );
        }
    }
}
impl Drop for StartupTiming {
    fn drop(&mut self) {
        self.record("end", self.started.elapsed().as_millis());
    }
}

pub async fn start_or_connect(directory: &Path) -> Result<Arc<CoreClient>, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let _startup = StartupTiming::new(directory, "desktop.connect_core");
    let client = Arc::new(CoreClient::Remote(directory.to_owned()));
    let probe = StartupTiming::new(directory, "desktop.probe_existing_core");
    let existing = matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            client.call::<Value>("ping", json!([]))
        )
        .await,
        Ok(Ok(_))
    );
    drop(probe);
    if existing {
        let endpoint: Endpoint = serde_json::from_slice(
            &std::fs::read(directory.join("service.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if endpoint.build == executable_build()? {
            return Ok(client);
        }
        // Prefer the new build when idle, but keep a compatible service reachable
        // while it owns work. Refusing to retire is a normal lifecycle state, not
        // a connection failure; hiding the UI would also hide its live approvals.
        let _retirement = StartupTiming::new(directory, "desktop.retire_old_core");
        if !client.call::<bool>("retire_if_idle", json!([])).await? {
            let _reuse = StartupTiming::new(directory, "desktop.reuse_busy_compatible_core");
            return Ok(client);
        }
        for _ in 0..100 {
            if let Ok(lease) = acquire_host_lease(directory) {
                drop(lease);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let _launch = StartupTiming::new(directory, "desktop.launch_and_wait_core");
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--core-service")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The child detaches its entire session before starting any native agents.
    // A separate process group alone still inherits the dev terminal: a child
    // opening /dev/tty can stop the whole service group with SIGTTIN/SIGTTOU.
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    // Reap without owning execution lifetime. Dropping a std Child does not terminate it.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    for _ in 0..150 {
        if matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                client.call::<Value>("ping", json!([]))
            )
            .await,
            Ok(Ok(_))
        ) {
            return Ok(client);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("后台服务未启动，请查看启动诊断".into())
}

#[must_use]
pub fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--core-service")) {
        return false;
    }
    let Some(directory) = args.next() else {
        return true;
    };
    let directory = PathBuf::from(directory);
    crate::service_diagnostics::install(&directory);
    #[cfg(unix)]
    if let Err(error) = nix::unistd::setsid() {
        crate::service_diagnostics::record("failed", &format!("terminal detach: {error}"));
        eprintln!("Core service could not detach from terminal: {error}");
        return true;
    }
    let result = tokio::runtime::Runtime::new()
        .map_err(|error| error.to_string())
        .and_then(|runtime| runtime.block_on(run_service(directory)));
    match result {
        Err(error) => {
            crate::service_diagnostics::record("failed", &error);
            eprintln!("Core service: {error}");
        }
        Ok(()) => crate::service_diagnostics::record("stopped", "service shutdown completed"),
    }
    true
}
async fn run_service(directory: PathBuf) -> Result<(), String> {
    use std::io::Write;
    let startup = StartupTiming::new(&directory, "service.start_to_ready");
    let _lease = acquire_host_lease(&directory)
        .map_err(|_| "Core service already owns this directory or lease unavailable")?;
    let storage = StartupTiming::new(&directory, "service.open_database");
    let (store, scratch) =
        prepare_desktop_storage(&directory).map_err(|_| "service storage unavailable")?;
    drop(storage);
    let core = Arc::new(
        Core::with_workspace_context(
            Arc::new(store),
            Arc::new(
                GitWorkspaceContextProvider::default()
                    .with_review_directory(directory.join("review")),
            ),
        )
        .with_history_query_program(std::env::current_exe().map_err(|e| e.to_string())?)
        .with_turn_timing_sink(crate::turn_timing::file_sink(&directory)),
    );
    let recovery = StartupTiming::new(&directory, "service.recover_interrupted_turns");
    core.recover_interrupted_turns()
        .await
        .map_err(|e| e.to_string())?;
    drop(recovery);
    core.register_driver(Arc::new(
        FakeDriver::new("fake").map_err(|e| e.to_string())?,
    ))
    .await;
    for source in BUILTIN_AGENT_MANIFESTS {
        if let Ok(manifest) = ProviderManifest::parse_toml(source) {
            core.select_installation(manifest.id.as_str(), None).await;
        }
    }
    let control = Arc::new(ServiceControl::new(&directory));
    remote::restore(
        control.remote.clone(),
        core.clone(),
        Arc::downgrade(&control),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let token = Arc::new(uuid::Uuid::new_v4().to_string());
    let endpoint = Endpoint {
        version: VERSION,
        address: listener.local_addr().map_err(|e| e.to_string())?,
        token: (*token).clone(),
        pid: std::process::id(),
        build: executable_build()?,
    };
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(&endpoint).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.persist(directory.join("service.json"))
        .map_err(|e| e.to_string())?;
    let discovery = Arc::clone(&core);
    let discovery_directory = directory.clone();
    let _discovery = control.startup_work.spawn(async move {
        let _discovery = StartupTiming::new(&discovery_directory, "service.scan_agents_background");
        scan_builtin_agents(discovery.clone(), None).await;
        ensure_initial_conversation(discovery, scratch).await;
    });
    let cleanup_core = core.clone();
    let cleanup_control = control.clone();
    let cleanup_directory = directory.clone();
    let _cleanup = control.startup_work.spawn(async move {
        let _timing =
            StartupTiming::new(&cleanup_directory, "service.submission_cleanup_background");
        if cleanup_control
            .submissions
            .reconcile_deleted(&cleanup_core, None)
            .await
            .is_err()
        {
            crate::service_diagnostics::record(
                "submission_cleanup_incomplete",
                "发送恢复记录清理未完成，请核实本机存储后重试",
            );
        }
    });
    // No awaited maintenance remains between this marker and accepting clients.
    drop(startup);
    crate::service_diagnostics::record(
        "ready",
        "local service accepting requests; startup maintenance may still be running",
    );
    accept_connections(core, listener, token, control).await
}
async fn accept_connections(
    core: Arc<Core>,
    listener: TcpListener,
    token: Arc<String>,
    control: Arc<ServiceControl>,
) -> Result<(), String> {
    let connections = Arc::new(tokio::sync::Semaphore::new(128));
    loop {
        let (stream, _) = tokio::select! {
            () = control.shutdown.notified() => return Ok(()),
            accepted = listener.accept() => accepted.map_err(|e|e.to_string())?,
        };
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            continue;
        };
        let core = core.clone();
        let token = token.clone();
        let control = control.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = serve_connection(core, stream, token, control).await;
        });
    }
}

pub(crate) fn record_frontend_startup(directory: &Path, stage: &str, elapsed_ms: u64) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("startup-timing.log"))
    {
        let _ = writeln!(
            file,
            "{} pid={} stage={} event=mark elapsed_ms={}",
            chrono::Utc::now().to_rfc3339(),
            std::process::id(),
            stage,
            elapsed_ms
        );
    }
}

#[cfg(feature = "remote-acceptance")]
pub fn run_remote_acceptance_if_requested() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--remote-acceptance") {
        return false;
    }
    let result = (|| {
        let service = args.get(2).ok_or("missing service")?.clone();
        let directory = PathBuf::from(args.get(3).ok_or("missing directory")?);
        tokio::runtime::Runtime::new()
            .map_err(|e| e.to_string())?
            .block_on(remote::acceptance_run(service, directory))
    })();
    if let Err(error) = result {
        eprintln!("isolated remote acceptance: {error}");
        std::process::exit(1);
    }
    true
}

#[cfg(test)]
mod tests {
    mod primary_admission_tests;
    mod remote_client_tests;
    mod repair_tests;
    mod review_tests;
    mod thought_history_tests;
    use super::*;
    use agent_core::CoreRepository;
    use agent_domain::ProjectId;
    use agent_protocol::RegisterProject;
    use agent_store::SqliteStore;
    #[cfg(unix)]
    #[test]
    fn service_session_detaches_from_parent() {
        const CHILD: &str = "DOGCODES_SESSION_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let pid = nix::unistd::getpid();
            assert_ne!(nix::unistd::getpgrp(), pid);
            nix::unistd::setsid().unwrap();
            assert_eq!(nix::unistd::getsid(None).unwrap(), pid);
            assert!(std::fs::File::open("/dev/tty").is_err());
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::tests::service_session_detaches_from_parent",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }
    struct Fixture {
        repository: Arc<SqliteStore>,
        client: CoreClient,
        core: Arc<Core>,
        directory: tempfile::TempDir,
        worker: tokio::task::JoinHandle<()>,
        control: Arc<ServiceControl>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.worker.abort();
        }
    }
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let repository = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(repository.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            version: VERSION,
            address: listener.local_addr().unwrap(),
            token: uuid::Uuid::new_v4().to_string(),
            pid: std::process::id(),
            build: executable_build().unwrap(),
        };
        std::fs::write(
            dir.path().join("service.json"),
            serde_json::to_vec(&endpoint).unwrap(),
        )
        .unwrap();
        let service = core.clone();
        let control = Arc::new(ServiceControl::new(dir.path()));
        let fixture_control = control.clone();
        let token = Arc::new(endpoint.token);
        let worker = tokio::spawn(async move {
            accept_connections(service, listener, token, control)
                .await
                .unwrap();
        });
        Fixture {
            repository,
            client: CoreClient::Remote(dir.path().into()),
            core,
            directory: dir,
            control: fixture_control,
            worker,
        }
    }
    #[tokio::test]
    async fn project_registration_is_validated_and_serialized_by_the_service() {
        use crate::project_registry::{ProjectEdit, ProjectRegistration};
        let f = fixture().await;
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let canonical = project.path().canonicalize().unwrap();
        let request = ProjectRegistration {
            display_name: String::new(),
            path: project.path().join(".").to_str().unwrap().into(),
            additional_directories: vec![],
        };
        // Two independent clients racing the same directory must share one project.
        let second = CoreClient::Remote(f.directory.path().into());
        let (a, b) = tokio::join!(
            f.client.register_project(request.clone()),
            second.register_project(request.clone()),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.project_id, b.project_id);
        assert_ne!(a.created, b.created);
        let projects = f.core.bootstrap().await.unwrap().projects;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, canonical.to_str().unwrap());

        let relative = ProjectRegistration {
            path: "relative".into(),
            ..request.clone()
        };
        assert!(
            f.client
                .register_project(relative)
                .await
                .unwrap_err()
                .contains("完整的本地目录路径")
        );
        let edit = ProjectEdit {
            project_id: a.project_id.clone(),
            expected_stream_version: 1,
            display_name: " Renamed ".into(),
            path: other.path().to_str().unwrap().into(),
            additional_directories: vec![project.path().to_str().unwrap().into()],
        };
        f.client.update_project(edit.clone()).await.unwrap();
        let saved = f.core.bootstrap().await.unwrap().projects.remove(0);
        assert_eq!(saved.display_name, "Renamed");
        assert_eq!(
            saved.path,
            other.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            saved.additional_directories,
            vec![canonical.to_str().unwrap()]
        );
        assert!(
            f.client
                .update_project(edit)
                .await
                .unwrap_err()
                .contains("项目已被修改")
        );
    }
    #[tokio::test]
    async fn submission_intents_rpc_recovers_registration_after_a_lost_reply() {
        use crate::workspace_service::watch::WatchMessage;
        use agent_domain::CommandId;
        use agent_protocol::submission::*;
        let f = fixture().await;
        let intent: SubmissionIntent = serde_json::from_value(json!({
            "schemaVersion": 1, "operationId": CommandId::new(), "scope": "draft:rpc", "requestedAt": "2026-10-16T12:00:00Z",
            "task": { "taskId": TaskId::new(), "projectId": ProjectId::new(), "title": "新对话", "preferredExecutor": {"providerId":"fake","installationId":null}, "activePrimaryRun":null, "streamVersion":0 },
            "target": {"providerId":"fake","installationId":null}, "input": {"prompt":"original", "model":null, "thinking":null, "permissionMode":null,"attachments":[]},
            "creation": {"operationId": CommandId::new()}, "context":null, "submission":null
        })).unwrap();
        let mut subscription = f.client.submission_watch().await.unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        assert_eq!(f.control.submissions.active_subscriptions(), 1);
        let stream = f
            .client
            .connect(
                "submission_intents",
                json!(SubmissionIntentCall::Put {
                    intent: Box::new(intent.clone())
                }),
            )
            .await
            .unwrap();
        drop(stream);
        let other = CoreClient::Remote(f.directory.path().into());
        let found = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let found = other
                    .submission_intents(SubmissionIntentCall::Get {
                        operation_id: intent.operation_id.clone(),
                        expected: Some(Box::new(intent.clone())),
                    })
                    .await
                    .unwrap();
                if !found.is_null() {
                    break found;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), subscription.next())
                .await
                .unwrap()
                .unwrap(),
            WatchMessage::Invalidated
        ));
        drop(subscription);
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.control.submissions.active_subscriptions() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut reconnected = other.submission_watch().await.unwrap();
        assert!(matches!(
            reconnected.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        assert_eq!(found["state"], "pending");
        assert_eq!(
            other
                .submission_intents(SubmissionIntentCall::Put {
                    intent: Box::new(intent.clone())
                })
                .await
                .unwrap(),
            found
        );
        assert_eq!(
            other
                .submission_intents(SubmissionIntentCall::List {
                    after: None,
                    limit: 50
                })
                .await
                .unwrap()["operationIds"],
            json!([intent.operation_id])
        );
        assert!(f.core.bootstrap().await.unwrap().tasks.is_empty());
    }
    #[tokio::test]
    async fn attachment_rpc_recovers_a_lost_upload_reply_across_clients() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use sha2::{Digest, Sha256};
        let f = fixture().await;
        let request = agent_protocol::AttachmentUploadRequest {
            upload_id: uuid::Uuid::new_v4().to_string(),
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            byte_length: 8,
            sha256: format!("{:x}", Sha256::digest(b"original")),
        };
        assert!(
            f.client
                .attachment_receipt(request.clone())
                .await
                .unwrap()
                .is_none()
        );
        let stream = f
            .client
            .connect(
                "attachment_save",
                json!((request.clone(), STANDARD.encode(b"original"))),
            )
            .await
            .unwrap();
        drop(stream); // Request was written; its response is deliberately lost.
        let other = CoreClient::Remote(f.directory.path().into());
        let saved = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(value) = other.attachment_receipt(request.clone()).await.unwrap() {
                    break value;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            other
                .attachment_save(request.clone(), STANDARD.encode(b"original"))
                .await
                .unwrap(),
            saved
        );
        assert_eq!(
            STANDARD
                .decode(other.attachment_read(saved.path.clone()).await.unwrap())
                .unwrap(),
            b"original"
        );
        let mut changed = request;
        changed.name = "different.txt".into();
        assert!(other.attachment_receipt(changed).await.is_err());
        assert_eq!(
            std::fs::read_dir(f.directory.path().join("attachments/uploads"))
                .unwrap()
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn manual_quota_refresh_flag_survives_service_rpc() {
        let f = fixture().await;
        for force in [false, true] {
            let views = f
                .client
                .quota_snapshots_for(None, Some("ins_missing"), force)
                .await
                .unwrap();
            assert!(views.is_empty());
        }
    }

    #[tokio::test]
    async fn agent_preferences_are_shared_through_service_rpc_and_replay() {
        let f = fixture().await;
        let other = CoreClient::Remote(f.directory.path().into());
        assert_eq!(f.client.agent_preferences().await.unwrap().revision, 0);
        let mut request = CommandEnvelope::local(CoreCommand::SetAgentPreferences(
            agent_protocol::AgentPreferences {
                order: vec!["kimi".into()],
                disabled: vec!["codex".into()],
            },
        ));
        request.expected_stream_version = Some(0);
        let ack = f.client.execute(request.clone()).await.unwrap();
        let current = other.agent_preferences().await.unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(current.preferences.disabled, ["codex"]);
        assert_eq!(other.execute(request).await.unwrap(), ack);
        let events = other.sync_events(0, 100).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event_type,
            agent_core::agent_preferences::EVENT_TYPE
        );
        assert_eq!(events[0].stream_version, current.revision);
    }

    #[tokio::test]
    async fn history_preferences_are_shared_through_service_rpc_and_replay() {
        let f = fixture().await;
        let other = CoreClient::Remote(f.directory.path().into());
        assert_eq!(f.client.history_preferences().await.unwrap().revision, 0);
        let mut request = CommandEnvelope::local(CoreCommand::SetHistoryPreferences(
            agent_protocol::HistoryPreferences {
                tasks: [(
                    "tsk_example".into(),
                    agent_protocol::HistoryTaskState::Archived,
                )]
                .into(),
                ..Default::default()
            },
        ));
        request.expected_stream_version = Some(0);
        let ack = f.client.execute(request.clone()).await.unwrap();
        let current = other.history_preferences().await.unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(
            current.preferences.tasks["tsk_example"],
            agent_protocol::HistoryTaskState::Archived
        );
        assert_eq!(other.execute(request).await.unwrap(), ack);
        let events = other.sync_events(0, 100).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event_type,
            agent_core::history_preferences::EVENT_TYPE
        );
        assert_eq!(events[0].sequence, current.last_sequence);
    }

    fn project() -> CommandEnvelope<CoreCommand> {
        CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            project_id: ProjectId::new(),
            display_name: "RPC fixture".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        }))
    }
    #[tokio::test]
    async fn disconnected_reply_does_not_cancel_or_duplicate_committed_command() {
        let f = fixture().await;
        let command = project();
        let stream = f
            .client
            .connect("execute", json!((command.clone(),)))
            .await
            .unwrap();
        drop(stream); // Client disappears before receiving its acknowledgement.
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.core.bootstrap().await.unwrap().projects.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // Explicit replay with the SAME idempotency key cannot create a second project.
        f.client.execute(command).await.unwrap();
        assert_eq!(f.client.bootstrap().await.unwrap().projects.len(), 1);
    }
    #[tokio::test]
    async fn workspace_lock_is_owned_by_service_and_released_on_disconnect() {
        let f = fixture().await;
        let lease = f.client.try_workspace_mutation().await.unwrap();
        assert!(f.core.try_workspace_mutation().is_err());
        assert!(f.client.try_workspace_mutation().await.is_err());
        drop(lease);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if f.core.try_workspace_mutation().is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn event_reconnect_replays_from_cursor_and_keeps_live_updates() {
        let f = fixture().await;
        f.client.execute(project()).await.unwrap();
        let cursor = f.client.bootstrap().await.unwrap().last_sequence;
        let mut subscription = f.client.subscribe(cursor).await.unwrap();
        assert!(subscription.replay.is_empty());
        f.client.execute(project()).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), subscription.live.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(event.sequence > cursor);
        drop(subscription);
        f.client.execute(project()).await.unwrap();
        let resumed = f.client.subscribe(event.sequence).await.unwrap();
        assert!(!resumed.replay.is_empty());
        assert!(resumed.replay.iter().all(|e| e.sequence > event.sequence));
    }
    #[tokio::test]
    async fn authentication_version_and_frame_limits_fail_closed() {
        let f = fixture().await;
        let endpoint: Endpoint = serde_json::from_slice(
            &std::fs::read(f.directory.path().join("service.json")).unwrap(),
        )
        .unwrap();
        for (version, token) in [
            (VERSION, "wrong".into()),
            (VERSION + 1, endpoint.token.clone()),
        ] {
            let mut stream = TcpStream::connect(endpoint.address).await.unwrap();
            write_frame(
                &mut stream,
                &Request {
                    version,
                    token,
                    method: "bootstrap".into(),
                    args: json!([]),
                },
            )
            .await
            .unwrap();
            assert!(read_frame::<Value>(&mut stream).await.is_err());
        }
        let mut stream = TcpStream::connect(endpoint.address).await.unwrap();
        stream
            .write_u32(u32::try_from(MAX_FRAME).unwrap() + 1)
            .await
            .unwrap();
        assert!(read_frame::<Value>(&mut stream).await.is_err());
        assert!(
            f.client
                .call::<Value>("not_a_method", json!([]))
                .await
                .is_err()
        );
        assert!(f.client.bootstrap().await.is_ok());
    }
    #[tokio::test]
    async fn rebuilt_desktop_reconnects_without_interrupting_busy_compatible_service() {
        let f = fixture().await;
        let path = f.directory.path().join("service.json");
        let mut endpoint: Endpoint =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        endpoint.build = "previous-build".into();
        std::fs::write(&path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        let work = f.control.startup_work.spawn(async move {
            waiting.await.unwrap();
        });

        let client =
            tokio::time::timeout(Duration::from_secs(3), start_or_connect(f.directory.path()))
                .await
                .unwrap()
                .expect("busy compatible service must not prevent the desktop from opening");
        client.bootstrap().await.unwrap();
        assert!(!work.is_finished());
        assert!(!f.worker.is_finished());
        assert!(
            !f.control
                .stopping
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let unchanged: Endpoint = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(unchanged.pid, endpoint.pid);
        assert_eq!(unchanged.token, endpoint.token);
        assert_eq!(unchanged.build, "previous-build");
        assert!(
            std::fs::read_to_string(f.directory.path().join("startup-timing.log"))
                .unwrap()
                .contains("desktop.reuse_busy_compatible_core")
        );

        release.send(()).unwrap();
        work.await.unwrap();
        // Reuse does not pin the service forever: normal idle retirement still works.
        assert!(
            client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn client_rejects_incompatible_endpoint_even_when_build_matches() {
        let f = fixture().await;
        let path = f.directory.path().join("service.json");
        let mut endpoint: Endpoint =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        endpoint.version = VERSION + 1;
        std::fs::write(path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
        assert_eq!(
            f.client.bootstrap().await.unwrap_err(),
            "incompatible local service"
        );
        assert!(
            !f.control
                .stopping
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }
    #[tokio::test]
    async fn startup_maintenance_keeps_queries_responsive_and_prevents_early_retirement() {
        let f = fixture().await;
        let root = f.directory.path().join("submission-intents");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("unrecognized.json"), b"keep original").unwrap();
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        let control = f.control.clone();
        let core = f.core.clone();
        let cleanup = f.control.startup_work.spawn(async move {
            waiting.await.unwrap();
            control.submissions.reconcile_deleted(&core, None).await
        });
        assert!(f.control.startup_work.active());
        tokio::time::timeout(Duration::from_secs(3), async {
            f.client.call::<Value>("ping", json!([])).await.unwrap();
            f.client.bootstrap().await.unwrap();
            assert!(
                !f.client
                    .call::<bool>("retire_if_idle", json!([]))
                    .await
                    .unwrap()
            );
        })
        .await
        .unwrap();
        assert!(!cleanup.is_finished());
        release.send(()).unwrap();
        assert!(cleanup.await.unwrap().unwrap_err().contains("部分"));
        assert!(!f.control.startup_work.active());
        assert_eq!(
            std::fs::read(root.join("unrecognized.json")).unwrap(),
            b"keep original"
        );
        assert!(
            f.client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while !f.worker.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(f.client.execute(project()).await.is_err());
    }
    #[tokio::test]
    async fn startup_cleanup_runs_behind_live_rpc_and_publishes_verified_deletion() {
        use crate::workspace_service::watch::WatchMessage;
        use agent_domain::CommandId;
        use agent_protocol::submission::*;
        let f = fixture().await;
        let command = project();
        let project_id = match &command.payload {
            CoreCommand::RegisterProject(p) => p.project_id.clone(),
            _ => unreachable!(),
        };
        f.core.execute(command).await.unwrap();
        let intent: SubmissionIntent = serde_json::from_value(json!({
            "schemaVersion":1, "operationId":CommandId::new(), "scope":"draft:startup", "requestedAt":"2026-10-16T12:00:00Z",
            "task":{"taskId":TaskId::new(),"projectId":project_id,"title":"新对话","preferredExecutor":{"providerId":"fake","installationId":null},"activePrimaryRun":null,"streamVersion":0},
            "target":{"providerId":"fake","installationId":null},"input":{"prompt":"private startup fixture","model":null,"thinking":null,"permissionMode":null,"attachments":[]},
            "creation":{"operationId":CommandId::new()},"context":null,"submission":null
        })).unwrap();
        f.client
            .submission_intents(SubmissionIntentCall::Put {
                intent: Box::new(intent.clone()),
            })
            .await
            .unwrap();
        f.core
            .execute(agent_core::submission_intents::stage_command(&intent).unwrap())
            .await
            .unwrap();
        let mut deletion =
            CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
                task_id: intent.task.task_id.clone(),
            }));
        deletion.expected_stream_version = Some(1);
        f.core.execute(deletion).await.unwrap(); // Simulate committed deletion before journal cleanup.
        let mut subscription = f.client.submission_watch().await.unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        let control = f.control.clone();
        let core = f.core.clone();
        let cleanup = f.control.startup_work.spawn(async move {
            waiting.await.unwrap();
            control.submissions.reconcile_deleted(&core, None).await
        });
        assert!(f.client.bootstrap().await.unwrap().tasks.is_empty());
        let query = SubmissionIntentCall::Get {
            operation_id: intent.operation_id.clone(),
            expected: Some(Box::new(intent.clone())),
        };
        assert_eq!(
            f.client.submission_intents(query.clone()).await.unwrap()["state"],
            "pending"
        );
        assert!(
            !f.client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
        release.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), subscription.next())
                .await
                .unwrap()
                .unwrap(),
            WatchMessage::Invalidated
        ));
        assert_eq!(
            f.client.submission_intents(query).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        let bytes = std::fs::read_to_string(
            f.directory
                .path()
                .join(format!("submission-intents/{}.json", intent.operation_id)),
        )
        .unwrap();
        assert!(!bytes.contains("private startup fixture"));
        assert!(!f.control.startup_work.active());
        assert!(
            f.client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
    }
    #[tokio::test]
    async fn idle_retirement_respects_workspace_ownership_and_closes_command_gate() {
        let f = fixture().await;
        let lease = f.client.try_workspace_mutation().await.unwrap();
        assert!(
            !f.client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
        assert!(f.client.bootstrap().await.is_ok());
        drop(lease);
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.core.try_workspace_mutation().is_err() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            f.client
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
        assert!(f.client.execute(project()).await.is_err());
        assert!(f.core.bootstrap().await.unwrap().projects.is_empty());
    }

    #[tokio::test]
    async fn terminal_input_rejects_busy_old_core_before_sending_any_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            version: VERSION,
            address: listener.local_addr().unwrap(),
            token: uuid::Uuid::new_v4().to_string(),
            pid: std::process::id(),
            build: "old-input-protocol".into(),
        };
        std::fs::write(
            directory.path().join("service.json"),
            serde_json::to_vec(&endpoint).unwrap(),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request: Request = read_frame(&mut socket).await.unwrap();
            assert_eq!(request.method, "ping");
            assert_eq!(request.token, endpoint.token);
            write_frame(
                &mut socket,
                &Ok::<_, String>(json!({"version":VERSION,"pid":1})),
            )
            .await
            .unwrap();
            // A second connection would carry a mutation to this old service.
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client = CoreClient::Remote(directory.path().into());
        let error = client
            .terminal_input("main", json!({"type":"write","terminalId":"not-sent"}))
            .await
            .unwrap_err();
        assert!(error.contains("旧版本"));
        server.await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_input_rpc_windows_and_phone_share_cas_without_replaying_old_bytes() {
        use crate::terminal_host::LaunchIntent;
        use agent_protocol::CreateTask;
        use base64::{Engine, engine::general_purpose::STANDARD};
        let f = fixture().await;
        let project_id = ProjectId::new();
        let task = TaskId::new();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                RegisterProject {
                    project_id: project_id.clone(),
                    display_name: "Input fixture".into(),
                    path: f.directory.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                },
            )))
            .await
            .unwrap();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::CreateTask(
                CreateTask {
                    task_id: task.clone(),
                    project_id: project_id.clone(),
                    title: "Input".into(),
                    preferred_executor: ExecutorRef {
                        provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                        installation_id: None,
                    },
                },
            )))
            .await
            .unwrap();
        let info=f.client.terminal_start(LaunchIntent{project_launch:None,operation_id:uuid::Uuid::new_v4().to_string(),project_id:project_id.to_string(),directory_index:0,task_id:Some(task.to_string()),command_id:None,label:"isolated input".into(),cwd:f.directory.path().into(),command:Some("stty -echo; printf 'READY\\n'; read text; printf 'INPUT:%s\\n' \"$text\"; sleep 60".into()),cols:80,rows:24}).await.unwrap();
        let mut cursor = 0;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !String::from_utf8_lossy(&bytes).contains("READY") {
                let batch = f.client.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                bytes.extend(batch.bytes);
            }
        })
        .await
        .unwrap();
        let inspect = json!({"type":"inspect","terminalId":info.id,"expectedEpoch":info.epoch});
        assert!(
            f.client
                .terminal_input("unknown-window", inspect.clone())
                .await
                .is_err()
        );
        let opening = |revision: &str, stream: &str| json!({"type":"open","terminalId":info.id,"expectedEpoch":info.epoch,"expectedRevision":revision,"streamId":stream});
        let writing = |opened: &Value, sequence: &str, data: &[u8]| json!({"type":"write","terminalId":info.id,"expectedEpoch":info.epoch,"revision":opened["revision"],"streamId":opened["streamId"],"sequence":sequence,"data":STANDARD.encode(data)});
        let first = f
            .client
            .terminal_input("main", opening("0", &uuid::Uuid::new_v4().to_string()))
            .await
            .unwrap();
        let a = writing(&first, "1", b"A");
        let applied = f.client.terminal_input("main", a.clone()).await.unwrap();
        assert_eq!(applied["state"], "applied");
        assert_eq!(
            f.client.terminal_input("main", a.clone()).await.unwrap(),
            applied
        );
        let review = format!("review-{task}");
        assert!(f.client.terminal_input(&review, a).await.is_err());
        let second = f
            .client
            .terminal_input(&review, opening("1", &uuid::Uuid::new_v4().to_string()))
            .await
            .unwrap();
        f.client
            .terminal_input(&review, writing(&second, "1", b"B"))
            .await
            .unwrap();
        assert!(
            f.client
                .terminal_input("main", writing(&first, "2", b"BAD-main"))
                .await
                .is_err()
        );
        let actor = ActorRef::PairedDevice(agent_domain::DeviceId::new());
        let third = remote::remote_call(
            &f.core,
            &f.control,
            "terminal_input",
            opening("2", &uuid::Uuid::new_v4().to_string()),
            actor.clone(),
        )
        .await
        .unwrap();
        assert!(
            f.client
                .terminal_input(&review, writing(&second, "2", b"BAD-review"))
                .await
                .is_err()
        );
        assert_eq!(
            f.client.terminal_input("main", inspect).await.unwrap()["owned"],
            false
        );
        remote::remote_call(
            &f.core,
            &f.control,
            "terminal_input",
            writing(&third, "1", b"C\n"),
            actor,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !String::from_utf8_lossy(&bytes).contains("INPUT:ABC") {
                let batch = f.client.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                bytes.extend(batch.bytes);
            }
        })
        .await
        .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("BAD-"));
        f.client.terminal_stop(&info.id).await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_host_rpc_disconnect_reconnect_and_idle_retirement() {
        use crate::terminal_host::{LaunchIntent, TerminalStatus};
        let f = fixture().await;
        let project_id = ProjectId::new();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                RegisterProject {
                    project_id: project_id.clone(),
                    display_name: "Terminal RPC".into(),
                    path: f.directory.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                },
            )))
            .await
            .unwrap();
        let intent = LaunchIntent {
            project_launch: None,
            operation_id: uuid::Uuid::new_v4().to_string(), project_id: project_id.to_string(), directory_index: 0,
            task_id: None, command_id: None, label: "RPC test".into(), cwd: f.directory.path().into(),
            command: Some("printf x >> rpc-count; printf 'READY\\n'; read text; printf 'RECEIVED:%s\\n' \"$text\"; sleep 60".into()),
            cols: 80, rows: 24,
        };
        // Send the real authenticated request, then discard its reply channel.
        drop(
            f.client
                .connect("terminal_start", json!(intent))
                .await
                .unwrap(),
        );
        let other = CoreClient::Remote(f.directory.path().into());
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if other
                    .terminal_get(&intent.operation_id)
                    .await
                    .is_ok_and(|info| info.status == TerminalStatus::Running)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let info = other.terminal_start(intent.clone()).await.unwrap();
        let mut cursor = 0;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !String::from_utf8_lossy(&bytes).contains("READY") {
                let batch = other.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                bytes.extend(batch.bytes);
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(f.directory.path().join("rpc-count")).unwrap(),
            b"x"
        );
        assert!(
            !other
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
        assert_eq!(
            other.terminal_list(&intent.project_id).await.unwrap().len(),
            1
        );
        // Losing an output subscriber does not own or stop the PTY.
        drop(
            other
                .connect("terminal_read", json!((&info.id, cursor)))
                .await
                .unwrap(),
        );
        let retired_write = "terminal_write";
        assert!(
            other
                .call::<Value>(retired_write, json!((&info.id, b"must-not-write".to_vec())))
                .await
                .is_err()
        );
        other.terminal_resize(&info.id, 100, 30).await.unwrap();
        other
            .terminal_write(&info.id, b"answer\n".to_vec())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !String::from_utf8_lossy(&bytes).contains("RECEIVED:answer") {
                let batch = other.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                bytes.extend(batch.bytes);
            }
        })
        .await
        .unwrap();
        other.terminal_stop(&info.id).await.unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let batch = other.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                if batch.info.status == TerminalStatus::Exited {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            other.terminal_start(intent).await.unwrap().status,
            TerminalStatus::Exited
        );
        assert_eq!(
            std::fs::read(f.directory.path().join("rpc-count")).unwrap(),
            b"x"
        );
        assert!(
            other
                .call::<bool>("retire_if_idle", json!([]))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn terminal_host_rpc_rejects_unauthorized_and_out_of_project_launches() {
        use crate::terminal_host::LaunchIntent;
        let f = fixture().await;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let project_id = ProjectId::new();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                RegisterProject {
                    project_id: project_id.clone(),
                    display_name: "Terminal scope".into(),
                    path: root.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                },
            )))
            .await
            .unwrap();
        let mut intent = LaunchIntent {
            project_launch: None,
            operation_id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            directory_index: 0,
            task_id: None,
            command_id: None,
            label: "Rejected".into(),
            cwd: outside.path().into(),
            command: None,
            cols: 80,
            rows: 24,
        };
        assert!(
            f.client
                .terminal_start(intent.clone())
                .await
                .unwrap_err()
                .contains("项目范围")
        );
        intent.cwd = root.path().into();
        intent.task_id = Some("tsk_missing".into());
        assert!(
            f.client
                .terminal_start(intent.clone())
                .await
                .unwrap_err()
                .contains("会话不属于")
        );
        intent.task_id = None;
        intent.directory_index = 1;
        assert!(
            f.client
                .terminal_start(intent.clone())
                .await
                .unwrap_err()
                .contains("项目目录不存在")
        );
        intent.directory_index = 0;
        let endpoint: Endpoint = serde_json::from_slice(
            &std::fs::read(f.directory.path().join("service.json")).unwrap(),
        )
        .unwrap();
        let mut stream = TcpStream::connect(endpoint.address).await.unwrap();
        write_frame(
            &mut stream,
            &Request {
                version: VERSION,
                token: "wrong".into(),
                method: "terminal_start".into(),
                args: json!(intent),
            },
        )
        .await
        .unwrap();
        assert!(read_frame::<Value>(&mut stream).await.is_err());
        assert!(
            f.client
                .terminal_list(&intent.project_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(f.client.terminal_get(&intent.operation_id).await.is_err());
    }

    fn test_agent_probe(
        status: agent_protocol::InstallationStatus,
    ) -> CommandEnvelope<CoreCommand> {
        let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                provider_id: "fake".into(),
                executable: "test-chat-agent".into(),
                fingerprint: "test-chat-agent-fingerprint".into(),
                status,
                version: None,
                version_text: None,
                diagnostic: None,
                elapsed_millis: 0,
            },
        ));
        command.actor = ActorRef::SystemRecovery;
        command
    }

    async fn readonly_test_driver(core: &Arc<Core>) -> Arc<ReadonlyTestDriver> {
        core.execute(test_agent_probe(agent_protocol::InstallationStatus::Ready))
            .await
            .unwrap();
        let installation = core
            .bootstrap()
            .await
            .unwrap()
            .installations
            .into_iter()
            .find(|installation| installation.executable == "test-chat-agent")
            .unwrap();
        Arc::new(ReadonlyTestDriver {
            executor: ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                installation_id: Some(installation.installation_id.to_string()),
            },
            fake: FakeDriver::new("fake").unwrap(),
            starting: tokio::sync::Notify::new(),
            allow_start: tokio::sync::Notify::new(),
            shutting_down: tokio::sync::Notify::new(),
            allow_shutdown: tokio::sync::Notify::new(),
            starts: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    struct ReadonlyTestDriver {
        executor: ExecutorRef,
        fake: FakeDriver,
        starting: tokio::sync::Notify,
        allow_start: tokio::sync::Notify,
        shutting_down: tokio::sync::Notify,
        allow_shutdown: tokio::sync::Notify,
        starts: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl driver_api::AgentDriver for ReadonlyTestDriver {
        fn executor(&self) -> &ExecutorRef {
            &self.executor
        }
        fn driver_version(&self) -> &'static str {
            "test"
        }
        fn supports_readonly_chat(&self) -> bool {
            true
        }
        async fn create_review_session(
            &self,
            directory: &str,
            additional: &[String],
        ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
            driver_api::AgentDriver::create_review_session(&self.fake, directory, additional).await
        }
        async fn create_session(
            &self,
            _: driver_api::SessionRequest,
        ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
            panic!("readonly test driver must not start a coding session")
        }
        async fn create_chat_session(
            &self,
            _: &str,
            _: &[String],
        ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.starting.notify_one();
            self.allow_start.notified().await;
            Ok(driver_api::SessionHandle {
                native_session_id: "fake_chat".into(),
            })
        }
        async fn prompt(
            &self,
            handle: &driver_api::SessionHandle,
            request: driver_api::PromptRequest,
        ) -> Result<driver_api::DriverEventStream, driver_api::DriverError> {
            driver_api::AgentDriver::prompt(&self.fake, handle, request).await
        }
        async fn interrupt(
            &self,
            _: &driver_api::SessionHandle,
        ) -> Result<(), driver_api::DriverError> {
            Ok(())
        }
        async fn shutdown(
            &self,
            _: driver_api::SessionHandle,
        ) -> Result<(), driver_api::DriverError> {
            self.shutting_down.notify_one();
            self.allow_shutdown.notified().await;
            Ok(())
        }
    }
    #[tokio::test]
    async fn fork_receipt_rpc_preserves_origin_and_does_not_create_another_task() {
        use agent_core::{AppendRequest, CoreRepository};
        use agent_protocol::{NewEvent, StreamRef, StreamType};
        let (f, source, project) = project_runs_fixture().await;
        let request = agent_core::task_fork::ForkRequest {
            task_id: source.clone(),
            turn_id: agent_domain::TurnId::new(),
            new_task_id: TaskId::new(),
            environment: agent_core::task_fork::ForkEnvironment::Workspace,
        };
        let stream = StreamRef {
            stream_type: StreamType::Task,
            stream_id: request.new_task_id.to_string(),
        };
        f.repository.append(AppendRequest {
            request_fingerprint: None,
            command_id: agent_domain::CommandId::new(), idempotency_key: format!("fork:{}", request.new_task_id), expected_stream_version: Some(0), artifacts: vec![],
            events: vec![
                NewEvent::hub(stream.clone(), "task.created", json!({"taskId":request.new_task_id,"projectId":project,"title":"Fork","preferredExecutor":{"providerId":"fake","installationId":null}})),
                NewEvent::hub(stream, "task.forked", json!({"taskId":request.new_task_id,"sourceTaskId":source,"sourceTurnId":request.turn_id,"sourceSequence":0,"environment":"workspace","text":"[]"})),
            ],
        }).await.unwrap();
        let head = f.repository.event_head().await.unwrap();
        let receipt = f
            .client
            .fork_receipt(source.clone(), request.new_task_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receipt.request, request);
        assert!(receipt.task_available);
        assert!(
            f.client
                .fork_receipt(source, TaskId::new())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            f.client
                .fork_receipt(request.new_task_id.clone(), request.new_task_id.clone())
                .await
                .is_err()
        );
        assert_eq!(
            f.client.fork_task(request.clone()).await.unwrap(),
            request.new_task_id
        );
        assert_eq!(f.repository.event_head().await.unwrap(), head);
    }

    #[tokio::test]
    async fn approval_receipt_rpc_prepares_identity_without_accepting_or_exposing_answers() {
        let f = fixture().await;
        let mut command = CommandEnvelope::local(CoreCommand::RespondToApproval(
            agent_protocol::RespondToApproval {
                task_id: TaskId::new(),
                turn_id: agent_domain::TurnId::new(),
                requested_at_sequence: 10,
                approval_id: "native-id".into(),
                option_id: String::new(),
                answer: Some(agent_domain::QuestionAnswer {
                    cancelled: false,
                    values: json!({"password":"sensitive"}),
                }),
            },
        ));
        command.actor = ActorRef::SystemRecovery; // Host normalizes the local caller.
        command.expected_stream_version = Some(5);
        let query = f
            .client
            .prepare_approval_response(command.clone())
            .await
            .unwrap();
        command.actor = ActorRef::LocalUser;
        assert_eq!(
            query.request_fingerprint,
            agent_core::command_identity::fingerprint(&command).unwrap()
        );
        assert!(!serde_json::to_string(&query).unwrap().contains("sensitive"));
        assert!(
            f.client
                .approval_receipt(query.clone())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            f.client
                .invalidate_approval_response(query, 5)
                .await
                .is_err()
        );
        assert_eq!(f.repository.event_head().await.unwrap(), 0);
        assert!(
            f.repository
                .command_ack(&command.idempotency_key)
                .await
                .unwrap()
                .is_none()
        );
        let wrong_command = CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            project_id: ProjectId::new(),
            display_name: "must not register".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        }));
        assert!(
            f.client
                .prepare_approval_response(wrong_command)
                .await
                .is_err()
        );
        assert_eq!(f.repository.event_head().await.unwrap(), 0);
        let project_id = ProjectId::new();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                RegisterProject {
                    project_id: project_id.clone(),
                    display_name: "fixture".into(),
                    path: f.directory.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                },
            )))
            .await
            .unwrap();
        let CoreCommand::RespondToApproval(response) = &command.payload else {
            unreachable!()
        };
        f.client
            .execute(CommandEnvelope::local(CoreCommand::CreateTask(
                agent_protocol::CreateTask {
                    task_id: response.task_id.clone(),
                    project_id,
                    title: "fixture".into(),
                    preferred_executor: agent_domain::ExecutorRef {
                        provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                        installation_id: None,
                    },
                },
            )))
            .await
            .unwrap();
        command.expected_stream_version = Some(1);
        let query = f.client.prepare_approval_response(command).await.unwrap();
        assert!(matches!(
            f.client
                .invalidate_approval_response(query.clone(), 1)
                .await
                .unwrap(),
            agent_core::approval_receipts::ApprovalInvalidation::Invalidated
        ));
        let head = f.repository.event_head().await.unwrap();
        assert_eq!(head, 3);
        f.client
            .invalidate_approval_response(query, 1)
            .await
            .unwrap();
        assert_eq!(f.repository.event_head().await.unwrap(), head);
    }

    #[tokio::test]
    async fn pending_command_invalidation_rpc_is_explicit_and_idempotent() {
        let f = fixture().await;
        let project_id = ProjectId::new();
        let registration = CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            project_id: project_id.clone(),
            display_name: "fixture".into(),
            path: f.directory.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        }));
        assert!(
            f.client
                .invalidate_pending_command(registration.clone())
                .await
                .is_err()
        );
        f.client.execute(registration).await.unwrap();
        let mut creation =
            CommandEnvelope::local(CoreCommand::CreateTask(agent_protocol::CreateTask {
                task_id: TaskId::new(),
                project_id,
                title: "private-title".into(),
                preferred_executor: agent_domain::ExecutorRef {
                    provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            }));
        creation.expected_stream_version = Some(0);
        creation.actor = ActorRef::SystemRecovery; // Desktop identity is fixed to local user.
        assert!(
            f.client
                .command_receipt(creation.clone())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(f.repository.event_head().await.unwrap(), 1);
        assert!(matches!(
            f.client
                .invalidate_pending_command(creation.clone())
                .await
                .unwrap(),
            agent_core::command_invalidation::CommandInvalidation::Invalidated
        ));
        assert!(f.client.bootstrap().await.unwrap().tasks.is_empty());
        let head = f.repository.event_head().await.unwrap();
        f.client
            .invalidate_pending_command(creation.clone())
            .await
            .unwrap();
        assert!(f.client.execute(creation.clone()).await.is_err());
        assert_eq!(f.repository.event_head().await.unwrap(), head);
        if let CoreCommand::CreateTask(command) = &mut creation.payload {
            command.title = "changed".into();
        }
        assert!(f.client.invalidate_pending_command(creation).await.is_err());
        assert_eq!(f.repository.event_head().await.unwrap(), head);
    }

    #[tokio::test]
    async fn command_identity_rpc_returns_original_ack_and_rejects_changed_payload() {
        let f = fixture().await;
        let original = CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            project_id: ProjectId::new(),
            display_name: "identity".into(),
            path: f.directory.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        }));
        assert!(
            f.client
                .command_receipt(original.clone())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(f.repository.event_head().await.unwrap(), 0);
        let ack = f.client.execute(original.clone()).await.unwrap();
        assert!(ack.request_fingerprint.is_some());
        let mut retry = original.clone();
        retry.command_id = agent_domain::CommandId::new();
        assert_eq!(f.client.execute(retry).await.unwrap(), ack);
        let head = f.repository.event_head().await.unwrap();
        let mut lookup = original.clone();
        lookup.command_id = agent_domain::CommandId::new();
        lookup.actor = ActorRef::SystemRecovery; // RPC callers cannot select their actor.
        assert_eq!(f.client.command_receipt(lookup).await.unwrap(), Some(ack));
        assert_eq!(f.repository.event_head().await.unwrap(), head);
        let mut changed = original;
        if let CoreCommand::RegisterProject(project) = &mut changed.payload {
            project.display_name = "changed".into();
        }
        assert!(
            f.client
                .command_receipt(changed.clone())
                .await
                .unwrap_err()
                .contains("不同")
        );
        assert!(
            f.client
                .execute(changed)
                .await
                .unwrap_err()
                .contains("不同")
        );
        assert_eq!(
            f.client.bootstrap().await.unwrap().projects[0].display_name,
            "identity"
        );
    }

    #[tokio::test]
    async fn command_receipt_rpc_rejects_internal_commands_without_writes() {
        let f = fixture().await;
        let commands = [
            CoreCommand::RecordTestResult(agent_protocol::RecordTestResult {
                task_id: TaskId::new(),
                command: "test".into(),
                outcome: "passed".into(),
                report: None,
            }),
            CoreCommand::RecordInstallationProbe(agent_protocol::RecordInstallationProbe {
                provider_id: "fake".into(),
                executable: "fake".into(),
                fingerprint: "fake".into(),
                status: agent_protocol::InstallationStatus::Ready,
                version: None,
                version_text: None,
                diagnostic: None,
                elapsed_millis: 0,
            }),
            CoreCommand::ReconcileMissingInstallation(
                agent_protocol::ReconcileMissingInstallation {
                    installation_id: agent_domain::InstallationId::new(),
                    provider_id: "fake".into(),
                    executable: "fake".into(),
                },
            ),
        ];
        for command in commands {
            assert!(
                f.client
                    .command_receipt(CommandEnvelope::local(command))
                    .await
                    .unwrap_err()
                    .contains("internal command")
            );
        }
        assert_eq!(f.repository.event_head().await.unwrap(), 0);
    }

    async fn project_runs_fixture() -> (Fixture, TaskId, ProjectId) {
        let f = fixture().await;
        let project_id = ProjectId::new();
        let task_id = TaskId::new();
        let directory = f.directory.path().join("project-runs");
        std::fs::create_dir_all(&directory).unwrap();
        let catalog = json!({ "projectId": project_id, "platform": std::env::consts::OS, "shell": "sh", "generation": "v1", "executor": null,
            "directories": [{ "path": f.directory.path(), "status": "ready", "error": null,
                "commands": [{ "id": "dev", "label": "Dev", "command": "printf x >> launches; printf 'RUN_OK\\n'", "cwd": ".", "source": "package.json" }] }] });
        std::fs::write(
            directory.join(format!("{project_id}-{}.json", std::env::consts::OS)),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                RegisterProject {
                    project_id: project_id.clone(),
                    display_name: "Run RPC".into(),
                    path: f.directory.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                },
            )))
            .await
            .unwrap();
        f.client
            .execute(CommandEnvelope::local(CoreCommand::CreateTask(
                agent_protocol::CreateTask {
                    task_id: task_id.clone(),
                    project_id: project_id.clone(),
                    title: "Run".into(),
                    preferred_executor: ExecutorRef {
                        provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                        installation_id: None,
                    },
                },
            )))
            .await
            .unwrap();
        (f, task_id, project_id)
    }

    #[tokio::test]
    async fn project_runs_shared_rpc_conflicts_and_lost_edit_reply_recover_from_disk() {
        use crate::project_runs::RunEdit;
        let (f, task, _) = project_runs_fixture().await;
        let other = CoreClient::Remote(f.directory.path().into());
        let initial = other.project_runs(task.as_str()).await.unwrap();
        assert_eq!(initial.generation, "v1");
        let rename = crate::project_runs::EditRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            task_id: task.to_string(),
            generation: "v1".into(),
            directory_index: 0,
            command_id: "dev".into(),
            edit: RunEdit::Rename {
                label: "Renamed".into(),
            },
        };
        let edited = f.client.project_run_edit(rename.clone()).await.unwrap();
        assert_ne!(edited.generation, "v1");
        assert!(matches!(
            other
                .project_run_edit(crate::project_runs::EditRequest {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    edit: RunEdit::Delete,
                    ..rename.clone()
                })
                .await
                .unwrap_err(), crate::project_runs::MutationError::Rejected { code, .. } if code == "version_conflict"
        ));
        assert_eq!(
            other.project_runs(task.as_str()).await.unwrap().directories[0].commands[0].label,
            "Renamed"
        );
        let pin = crate::project_runs::EditRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            generation: edited.generation.clone(),
            edit: RunEdit::Pin { pinned: true },
            ..rename.clone()
        };
        drop(other.connect("project_run_edit", json!(pin)).await.unwrap());
        let saved = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let current = other.project_runs(task.as_str()).await.unwrap();
                if current.generation != edited.generation {
                    break current;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(saved.directories[0].commands[0].pinned);
        assert_eq!(saved.directories[0].commands[0].label, "Renamed");
        assert_eq!(
            other
                .project_run_edit(pin.clone())
                .await
                .unwrap()
                .generation,
            saved.generation
        );
        assert_eq!(
            other
                .project_run_edit(rename.clone())
                .await
                .unwrap()
                .generation,
            saved.generation
        );
        assert_eq!(
            other
                .project_run_operation(task.as_str(), &pin.operation_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "succeeded"
        );
        let mut changed = pin.clone();
        changed.edit = RunEdit::Delete;
        assert!(
            other
                .project_run_edit(changed)
                .await
                .unwrap_err()
                .to_string()
                .contains("不同请求")
        );
        let restored =
            crate::project_runs::ProjectRunHost::new(f.directory.path().join("project-runs"));
        let value = crate::project_runs::dispatch(
            &f.core,
            &restored,
            &crate::terminal_host::TerminalHost::default(),
            "project_runs",
            json!(task),
        )
        .await
        .unwrap();
        assert_eq!(value["generation"], saved.generation);
        assert_eq!(value["directories"][0]["commands"][0]["pinned"], true);
        assert!(
            value.get("operations").is_none(),
            "private receipts must not leak through the catalog response"
        );
        let replay = crate::project_runs::dispatch(
            &f.core,
            &restored,
            &crate::terminal_host::TerminalHost::default(),
            "project_run_edit",
            json!(pin),
        )
        .await
        .unwrap();
        assert_eq!(replay["Ok"]["generation"], saved.generation);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn project_runs_service_validates_generation_before_launching_and_reuses_identity() {
        let (f, task, _) = project_runs_fixture().await;
        let request = crate::project_runs::LaunchRequest {
            task_id: task.to_string(),
            generation: "stale".into(),
            directory_index: 0,
            command_id: "dev".into(),
            cols: 80,
            rows: 24,
            operation_id: uuid::Uuid::new_v4().to_string(),
        };
        assert!(
            f.client
                .project_run_open(request.clone())
                .await
                .unwrap_err()
                .contains("配置已更新")
        );
        assert!(!f.directory.path().join("launches").exists());
        let request = crate::project_runs::LaunchRequest {
            generation: "v1".into(),
            ..request
        };
        drop(
            f.client
                .connect("project_run_open", json!(request))
                .await
                .unwrap(),
        );
        let other = CoreClient::Remote(f.directory.path().into());
        let info = other.project_run_open(request.clone()).await.unwrap();
        let mut saw_marker = false;
        let completed = tokio::time::timeout(Duration::from_secs(8), async {
            let mut cursor = 0;
            loop {
                let batch = other.terminal_read(&info.id, cursor).await.unwrap();
                cursor = batch.next;
                saw_marker |= String::from_utf8_lossy(&batch.bytes).contains("RUN_OK");
                assert!(
                    batch.info.status != crate::terminal_host::TerminalStatus::Failed
                        && batch.info.status != crate::terminal_host::TerminalStatus::Unknown,
                    "unexpected process outcome: {:?} {:?}",
                    batch.info.status,
                    batch.info.error
                );
                if batch.info.status == crate::terminal_host::TerminalStatus::Exited {
                    break;
                }
            }
        })
        .await;
        assert!(
            completed.is_ok(),
            "process did not exit: {:?}, marker={saw_marker}, side_effect_present={}",
            other.terminal_get(&info.id).await.unwrap().status,
            f.directory.path().join("launches").exists()
        );
        assert_eq!(
            std::fs::read(f.directory.path().join("launches")).unwrap(),
            b"x"
        );
        let edited = other
            .project_run_edit(crate::project_runs::EditRequest {
                operation_id: uuid::Uuid::new_v4().to_string(),
                task_id: task.to_string(),
                generation: "v1".into(),
                directory_index: 0,
                command_id: "dev".into(),
                edit: crate::project_runs::RunEdit::Rename {
                    label: "New label".into(),
                },
            })
            .await
            .unwrap();
        let replay = other.project_run_open(request.clone()).await.unwrap();
        assert_eq!(replay.id, info.id);
        assert_eq!(replay.label, "Dev"); // The accepted resource keeps its original snapshot.
        for changed in [
            crate::project_runs::LaunchRequest {
                generation: edited.generation.clone(),
                ..request.clone()
            },
            crate::project_runs::LaunchRequest {
                task_id: "other-task".into(),
                ..request.clone()
            },
            crate::project_runs::LaunchRequest {
                command_id: "other-command".into(),
                ..request.clone()
            },
            crate::project_runs::LaunchRequest {
                directory_index: 1,
                ..request.clone()
            },
            crate::project_runs::LaunchRequest {
                cols: 100,
                ..request.clone()
            },
            crate::project_runs::LaunchRequest {
                rows: 30,
                ..request.clone()
            },
        ] {
            assert!(
                other
                    .project_run_open(changed)
                    .await
                    .unwrap_err()
                    .contains("不同请求")
            );
        }
        // A reload reads the binding from disk, even when current task/catalog
        // state no longer exists. This only returns the accepted resource.
        let terminals = crate::terminal_host::TerminalHost::persistent(
            f.directory.path().join("terminal-operations"),
        );
        let empty_core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let host =
            crate::project_runs::ProjectRunHost::new(f.directory.path().join("missing-catalog"));
        let replay = crate::project_runs::dispatch(
            &empty_core,
            &host,
            &terminals,
            "project_run_open",
            json!(request),
        )
        .await
        .unwrap();
        assert_eq!(replay["id"], info.id);
        assert_eq!(replay["status"], "exited");
        assert!(!terminals.has_active());
        assert!(
            crate::project_runs::dispatch(
                &empty_core,
                &host,
                &terminals,
                "project_run_open",
                json!(crate::project_runs::LaunchRequest {
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    ..request.clone()
                })
            )
            .await
            .unwrap_err()
            .contains("会话不存在")
        );
        let journal = f
            .directory
            .path()
            .join("terminal-operations")
            .join(format!("{}.json", info.id));
        let stored: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        let bound: crate::terminal_host::LaunchIntent =
            serde_json::from_value(stored["intent"].clone()).unwrap();
        assert!(
            other
                .terminal_start(bound)
                .await
                .unwrap_err()
                .contains("项目启动入口")
        );
        assert_eq!(
            std::fs::read(f.directory.path().join("launches")).unwrap(),
            b"x"
        );
    }

    #[tokio::test]
    async fn workspace_service_rpc_queries_watch_disconnect_and_scope_changes() {
        use crate::workspace_service::{DirectoryRequest, watch::WatchMessage};
        let (f, task, project) = project_runs_fixture().await;
        let request = DirectoryRequest {
            task_id: task.to_string(),
            root: 0,
            path: String::new(),
        };
        std::fs::write(f.directory.path().join("hello.txt"), "hello").unwrap();
        let roots = f.client.workspace_roots(task.to_string()).await.unwrap();
        assert_eq!(roots, vec![f.directory.path().to_owned()]);
        let preview = f
            .client
            .workspace_preview(DirectoryRequest {
                path: "hello.txt".into(),
                ..request.clone()
            })
            .await
            .unwrap();
        assert_eq!(preview.text.as_deref(), Some("hello"));
        assert!(
            f.client
                .workspace_directory(DirectoryRequest {
                    path: "../".into(),
                    ..request.clone()
                })
                .await
                .is_err()
        );
        assert!(
            f.client
                .workspace_watch(DirectoryRequest {
                    task_id: TaskId::new().to_string(),
                    ..request.clone()
                })
                .await
                .is_err()
        );
        let results = serde_json::to_value(
            f.client
                .workspace_find_files(task.to_string(), "hello".into(), false)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(results["files"][0]["path"], "hello.txt");
        std::fs::create_dir(f.directory.path().join("nested")).unwrap();
        let watching = DirectoryRequest {
            path: "nested".into(),
            ..request.clone()
        };
        let mut first = f.client.workspace_watch(watching.clone()).await.unwrap();
        let mut second = f.client.workspace_watch(watching).await.unwrap();
        assert_eq!(f.control.workspace_watchers.active_count(), 2);
        std::fs::write(f.directory.path().join("nested/change.txt"), "new").unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), first.next())
                .await
                .unwrap()
                .unwrap(),
            WatchMessage::Invalidated
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), second.next())
                .await
                .unwrap()
                .unwrap(),
            WatchMessage::Invalidated
        ));
        drop(first);
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.control.workspace_watchers.active_count() != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // Changing a project's directory invalidates the old subscription's scope.
        let changed = f.directory.path().join("changed-root");
        std::fs::create_dir(&changed).unwrap();
        let revision = f
            .core
            .bootstrap()
            .await
            .unwrap()
            .projects
            .iter()
            .find(|value| value.project_id == project)
            .unwrap()
            .stream_version;
        let mut update = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
            project_id: project,
            display_name: "Changed".into(),
            path: changed.to_string_lossy().into_owned(),
            additional_directories: vec![],
        }));
        update.expected_stream_version = Some(revision);
        f.core.execute(update).await.unwrap();
        std::fs::write(f.directory.path().join("nested/trigger.txt"), "old scope").unwrap();
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(5), second.next()).await.unwrap().unwrap(), WatchMessage::Error { code, .. } if code == "scope_changed")
        );
        drop(second);
        tokio::time::timeout(Duration::from_secs(3), async {
            while f.control.workspace_watchers.active_count() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let local = CoreClient::local_at(f.core.clone(), f.directory.path());
        assert_eq!(
            local.workspace_roots(task.to_string()).await.unwrap(),
            vec![changed]
        );
        let local_watch = local.workspace_watch(request).await.unwrap();
        drop(local_watch);
        // Existing similarly named Core query must keep its route.
        assert!(f.client.workspace_snapshots(&task, 1).await.is_ok());
    }

    #[tokio::test]
    async fn project_runs_embedded_acceptance_uses_the_same_host_contract() {
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let directory = tempfile::tempdir().unwrap();
        let client = CoreClient::local_at(core, directory.path());
        let info = client.terminal_list("none").await.unwrap();
        assert!(info.is_empty());
        assert!(
            client
                .project_runs("missing")
                .await
                .unwrap_err()
                .contains("会话不存在")
        );
    }

    #[test]
    fn service_lease_prevents_two_owners() {
        let dir = tempfile::tempdir().unwrap();
        let lease = acquire_host_lease(dir.path()).unwrap_or_else(|_| panic!("lease"));
        assert!(acquire_host_lease(dir.path()).is_err());
        drop(lease);
        assert!(acquire_host_lease(dir.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn service_lease_release_does_not_wait_for_inherited_descriptors() {
        let dir = tempfile::tempdir().unwrap();
        let lease = acquire_host_lease(dir.path()).unwrap_or_else(|_| panic!("lease"));
        // A fork before exec holds the same open file description, just like dup.
        // Keep that description alive deterministically instead of racing a PTY spawn.
        let inherited = lease.file.try_clone().unwrap();
        assert!(acquire_host_lease(dir.path()).is_err());
        drop(lease);
        let replacement = acquire_host_lease(dir.path())
            .unwrap_or_else(|_| panic!("released owner still holds the lease"));
        drop(inherited);
        assert!(acquire_host_lease(dir.path()).is_err());
        drop(replacement);
        assert!(acquire_host_lease(dir.path()).is_ok());
    }

    #[tokio::test]
    async fn service_lease_excludes_other_processes_and_recovers_after_kill() {
        use std::io::Write;
        use tokio::io::AsyncBufReadExt;

        const CHILD: &str = "DOGCODES_HOST_LEASE_TEST_CHILD";
        const READY: &str = "HOST_LEASE_READY";
        if let Some(directory) = std::env::var_os(CHILD) {
            let _lease =
                acquire_host_lease(Path::new(&directory)).unwrap_or_else(|_| panic!("child lease"));
            println!("{READY}");
            std::io::stdout().flush().unwrap();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::tests::service_lease_excludes_other_processes_and_recovers_after_kill",
                "--nocapture",
            ])
            .env(CHILD, dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("child exited early");
                if line.trim() == READY {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(acquire_host_lease(dir.path()).is_err());
        child.kill().await.unwrap();
        assert!(acquire_host_lease(dir.path()).is_ok());
    }
}
