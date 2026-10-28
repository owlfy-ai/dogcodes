mod agent_login;
mod agent_scan;
mod agent_updates;
mod artifact_export;
mod attachment_service;
mod attachments;
pub mod daemon;
mod external_links;
mod project_registry;
mod project_runs;
mod review;
#[cfg(feature = "review-acceptance")]
mod review_acceptance;
mod review_file_actions;
mod review_repairs;
mod review_window;
mod service_diagnostics;
mod state_changes;
mod submission_intents;
mod task_deletion;
mod terminal;
pub mod terminal_host;
mod terminal_installation;
mod turn_review;
mod turn_timing;
mod turn_undo;
mod workspace_browser;
mod workspace_service;

use agent_protocol::{
    ArtifactChunk, ArtifactCursor, ArtifactExportRequest, ArtifactPage, ArtifactReadRequest,
};
use tauri_plugin_dialog::DialogExt;

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, ProviderId, TaskId};
use agent_protocol::{
    ActorRef, BootstrapSnapshot, BuiltContextPackage, CommandAck, CommandEnvelope,
    ContextEditRequest, ContextPreviewRequest, CoreCommand, CreateTask, EventEnvelope,
    EventStreamMessage, InstallationStatus, ReconcileMissingInstallation, RecordInstallationProbe,
    RegisterProject, TimelineItem, WorkspaceSnapshotSummary,
};
use agent_registry::{
    DiscoveryContext, InstallationStatus as ProbeStatus, ProviderManifest, probe_installation,
};
use agent_store::SqliteStore;
use agent_workspace::GitWorkspaceContextProvider;
use claude_driver::ClaudeDriver;
use codex_driver::CodexDriver;
use driver_api::FakeDriver;
use tauri::{Emitter, Manager, ipc::Channel};
use tokio::sync::{broadcast::error::RecvError, oneshot};
use zcode_driver::ZcodeDriver;

#[tauri::command]
async fn core_remote_control(
    state: tauri::State<'_, DesktopState>,
    call: serde_json::Value,
) -> Result<serde_json::Value, String> {
    state.core.remote_control(call).await
}

struct DesktopState {
    core: Arc<daemon::CoreClient>,
    subscriptions: Mutex<HashMap<String, oneshot::Sender<()>>>,
    export_gate: tokio::sync::Mutex<()>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum StartupStatus {
    Ready,
    #[cfg(not(feature = "review-acceptance"))]
    ServiceUnavailable,
    StorageRecoveryRequired,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum StartupFailureCode {
    StorageDirectoryUnavailable,
    HostLeaseUnavailable,
    DatabaseCheckFailed,
    /// Written by a newer dogcodes; left untouched until that version is installed.
    DatabaseFromNewerVersion,
    ScratchDirectoryUnavailable,
    #[cfg(feature = "review-acceptance")]
    HistoryRecoveryFailed,
    #[cfg(not(feature = "review-acceptance"))]
    ServiceUnavailable,
}
struct StartupFailure(Option<StartupFailureCode>);
struct StartupExportGate(tokio::sync::Mutex<()>);
fn startup_diagnostic_value(failure: &StartupFailure) -> serde_json::Value {
    serde_json::json!({"schemaVersion":1,"component":"startup","code":failure.0,"applicationVersion":env!("CARGO_PKG_VERSION"),"generatedAt":chrono::Utc::now(),"automaticReplay":false})
}
struct HostLease {
    file: std::fs::File,
}

impl Drop for HostLease {
    fn drop(&mut self) {
        // Closing alone can leave a Unix flock held by a concurrent fork's
        // inherited descriptor until exec. Ownership ends with this guard.
        let _ = self.file.unlock();
    }
}

fn acquire_host_lease(app_data: &std::path::Path) -> Result<HostLease, StartupFailureCode> {
    std::fs::create_dir_all(app_data)
        .map_err(|_| StartupFailureCode::StorageDirectoryUnavailable)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(app_data.join("host.lock"))
        .map_err(|_| StartupFailureCode::HostLeaseUnavailable)?;
    file.try_lock()
        .map_err(|_| StartupFailureCode::HostLeaseUnavailable)?;
    Ok(HostLease { file })
}

fn prepare_desktop_storage(
    app_data: &std::path::Path,
) -> Result<(SqliteStore, std::path::PathBuf), StartupFailureCode> {
    std::fs::create_dir_all(app_data)
        .map_err(|_| StartupFailureCode::StorageDirectoryUnavailable)?;
    let store = SqliteStore::open(app_data.join("app-v5.db")).map_err(|error| {
        if error.to_string().contains(agent_store::NEWER_FORMAT_ERROR) {
            StartupFailureCode::DatabaseFromNewerVersion
        } else {
            StartupFailureCode::DatabaseCheckFailed
        }
    })?;
    let scratch = app_data.join("scratch");
    std::fs::create_dir_all(&scratch)
        .map_err(|_| StartupFailureCode::ScratchDirectoryUnavailable)?;
    Ok((store, scratch))
}

#[cfg(test)]
async fn register_local_project(
    core: &Arc<daemon::CoreClient>,
    display_name: String,
    path: String,
) -> Result<ProjectId, String> {
    register_local_multi_project(core, display_name, path, vec![]).await
}

// Validation, duplicate reuse and ID allocation run in the service so every
// client shares them; the desktop only adapts the window entry point.
async fn register_local_multi_project(
    core: &Arc<daemon::CoreClient>,
    display_name: String,
    path: String,
    additional_directories: Vec<String>,
) -> Result<ProjectId, String> {
    core.register_project(project_registry::ProjectRegistration {
        display_name,
        path,
        additional_directories,
    })
    .await
    .map(|registered| registered.project_id)
}

#[tauri::command]
async fn core_register_local_project(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    display_name: String,
    path: String,
    additional_directories: Option<Vec<String>>,
) -> Result<ProjectId, String> {
    require_main_artifact_window(window.label())?;
    register_local_multi_project(
        &state.core,
        display_name,
        path,
        additional_directories.unwrap_or_default(),
    )
    .await
}

async fn update_local_project(
    core: &Arc<daemon::CoreClient>,
    project_id: ProjectId,
    expected_stream_version: u64,
    display_name: String,
    path: String,
    additional_directories: Vec<String>,
) -> Result<(), String> {
    core.update_project(project_registry::ProjectEdit {
        project_id,
        expected_stream_version,
        display_name,
        path,
        additional_directories,
    })
    .await
}

#[tauri::command]
async fn core_update_local_project(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    project_id: ProjectId,
    expected_stream_version: u64,
    display_name: String,
    path: String,
    additional_directories: Vec<String>,
) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    update_local_project(
        &state.core,
        project_id,
        expected_stream_version,
        display_name,
        path,
        additional_directories,
    )
    .await
}

#[tauri::command]
async fn core_reveal_local_project(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    project_id: ProjectId,
) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    let project = state
        .core
        .bootstrap()
        .await?
        .projects
        .into_iter()
        .find(|project| project.project_id == project_id)
        .ok_or("项目不存在")?;
    let target = tokio::fs::canonicalize(&project.path)
        .await
        .map_err(|_| "项目目录不存在或无法访问")?;
    if !target.is_dir() {
        return Err("项目路径不是文件夹".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = tokio::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(&target);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("explorer.exe");
        command.arg(&target);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(&target);
        command
    };
    let status = command.status().await.map_err(|_| "无法打开文件管理器")?;
    if !status.success() {
        return Err("无法打开文件管理器".into());
    }
    Ok(())
}

#[tauri::command]
async fn core_pick_project_directory(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    require_main_artifact_window(window.label())?;
    let (sender, receiver) = oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("选择项目目录");
    // Without a starting folder the system picker reopens wherever it was last
    // left, often the filesystem root. Hidden entries stay governed by the
    // picker's own toggle (⌘⇧. on macOS), which is off by default.
    if let Some(directory) = project_picker_start(&app) {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_folder(move |path| {
        let _ = sender.send(path);
    });
    let Some(path) = receiver.await.map_err(|_| "目录选择窗口已关闭")? else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| "请选择本地目录")?;
    if !path.is_dir() {
        return Err("所选路径不是目录".into());
    }
    let path = path.to_str().ok_or("目录路径不是有效的 UTF-8")?;
    Ok(Some(path.to_owned()))
}

/// Windows opens on "This PC" so every drive is listed; elsewhere, the home directory.
fn project_picker_start(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        return Some("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}".into());
    }
    app.path().home_dir().ok().filter(|home| home.is_dir())
}

#[tauri::command]
fn restart_application(window: tauri::WebviewWindow) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    // The independent service retains ownership of running tasks.
    window.app_handle().restart();
}

// Reveal only after the frontend has painted its themed shell.
#[tauri::command]
fn show_main_window(window: tauri::WebviewWindow) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    window.show().map_err(|error| error.to_string())
}

#[tauri::command]
fn core_startup_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, StartupStatus>,
) -> Result<StartupStatus, String> {
    require_main_artifact_window(window.label())?;
    Ok(state.inner().clone())
}

#[tauri::command]
async fn core_prepare_session_rebuild(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    expected_version: u64,
) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    state
        .core
        .prepare_session_rebuild(&ActorRef::LocalUser, &task_id, expected_version)
        .await
        .map_err(|_| "会话状态已变化，请刷新后重试".into())
}

#[tauri::command]
async fn core_recovery_snapshot(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<agent_protocol::RecoverySnapshot, String> {
    require_main_artifact_window(window.label())?;
    state
        .core
        .recovery_snapshot(&ActorRef::LocalUser)
        .await
        .map_err(|_| "恢复信息读取失败，请保留数据库并重试".into())
}

#[tauri::command]
fn core_startup_diagnostics(
    window: tauri::WebviewWindow,
    failure: tauri::State<'_, StartupFailure>,
) -> Result<serde_json::Value, String> {
    require_main_artifact_window(window.label())?;
    Ok(startup_diagnostic_value(&failure))
}

#[tauri::command]
async fn core_export_startup_diagnostics(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    failure: tauri::State<'_, StartupFailure>,
    gate: tauri::State<'_, StartupExportGate>,
) -> Result<&'static str, String> {
    require_main_artifact_window(window.label())?;
    let _guard = gate.0.try_lock().map_err(|_| "另一个导出窗口已打开")?;
    let mut report = startup_diagnostic_value(&failure);
    if let Some(directory) = app.try_state::<service_diagnostics::DiagnosticDirectory>() {
        report["serviceDiagnostics"] = service_diagnostics::report(&directory.0);
    }
    let content = serde_json::to_vec_pretty(&report).map_err(|_| "诊断信息序列化失败")?;
    let (sender, receiver) = oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("导出诊断报告")
        .set_file_name("startup-diagnostics.json")
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver.await.map_err(|_| "导出窗口已关闭")? else {
        return Ok("cancelled");
    };
    let path = path.into_path().map_err(|_| "请选择本地文件")?;
    tauri::async_runtime::spawn_blocking(move || {
        artifact_export::save_new_artifact(&path, &content)
    })
    .await
    .map_err(|_| "导出失败")??;
    Ok("saved")
}

#[tauri::command]
async fn core_export_diagnostics(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
) -> Result<&'static str, String> {
    require_main_artifact_window(window.label())?;
    let _guard = state
        .export_gate
        .try_lock()
        .map_err(|_| "另一个导出窗口已打开")?;
    let snapshot = state
        .core
        .recovery_snapshot(&ActorRef::LocalUser)
        .await
        .map_err(|_| "诊断信息读取失败")?;
    let content = serde_json::to_vec_pretty(&snapshot).map_err(|_| "诊断信息序列化失败")?;
    let (sender, receiver) = oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("导出诊断摘要（不含对话或凭据）")
        .set_file_name("diagnostics.json")
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver.await.map_err(|_| "导出窗口已关闭")? else {
        return Ok("cancelled");
    };
    let path = path.into_path().map_err(|_| "请选择本地文件")?;
    tauri::async_runtime::spawn_blocking(move || {
        artifact_export::save_new_artifact(&path, &content)
    })
    .await
    .map_err(|_| "导出失败")??;
    Ok("saved")
}

#[tauri::command]
async fn core_task_artifacts(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    before: Option<ArtifactCursor>,
    limit: Option<usize>,
) -> Result<ArtifactPage, String> {
    require_main_artifact_window(window.label())?;
    let task_id = TaskId::parse(task_id).map_err(|error| error.to_string())?;
    state
        .core
        .task_artifacts(
            &ActorRef::LocalUser,
            &task_id,
            before.as_ref(),
            limit.unwrap_or(20),
        )
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_read_artifact(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: ArtifactReadRequest,
) -> Result<ArtifactChunk, String> {
    require_main_artifact_window(window.label())?;
    state
        .core
        .read_artifact(&ActorRef::LocalUser, &request)
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_export_artifact(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    request: ArtifactExportRequest,
) -> Result<&'static str, String> {
    require_main_artifact_window(window.label())?;
    if !request.confirmed_sensitive_export {
        return Err("confirm the artifact's sensitive content before exporting".into());
    }
    let _guard = state
        .export_gate
        .try_lock()
        .map_err(|_| "another export dialog is open")?;
    let artifact = state
        .core
        .verified_local_artifact(
            &ActorRef::LocalUser,
            &request.task_id,
            &request.artifact_id,
            &request.expected_payload_sha256,
        )
        .await
        .map_err(|error| error.clone())?;
    let extension = if artifact.metadata.media_type == "application/vnd.dogcodes.git-diff.v1.patch"
    {
        "patch"
    } else {
        "json"
    };
    let (sender, receiver) = oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("导出已审阅的原始 Artifact（请使用新文件名）")
        .set_file_name(format!("{}.{extension}", artifact.metadata.artifact_id))
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver
        .await
        .map_err(|_| "export dialog closed unexpectedly")?
    else {
        return Ok("cancelled");
    };
    let path = path
        .into_path()
        .map_err(|_| "export destination is not a local path")?;
    tauri::async_runtime::spawn_blocking(move || {
        artifact_export::save_new_artifact(&path, &artifact.content)
    })
    .await
    .map_err(|_| "artifact export worker failed")??;
    Ok("saved")
}

fn require_main_artifact_window(label: &str) -> Result<(), String> {
    if label != "main" {
        return Err("artifact access is only available in the main window".into());
    }
    Ok(())
}

#[tauri::command]
fn core_startup_trace(window: tauri::WebviewWindow, stage: String, elapsed_ms: u64) {
    if !cfg!(debug_assertions) || window.label() != "main" {
        return;
    }
    if ![
        "frontend.module_ready",
        "frontend.app_mounted",
        "frontend.bootstrap_ready",
        "frontend.timeline_ready",
        "frontend.event_loop_stall",
    ]
    .contains(&stage.as_str())
    {
        return;
    }
    if let Ok(directory) = window.app_handle().path().app_data_dir() {
        daemon::record_frontend_startup(&directory, &stage, elapsed_ms);
    }
}

#[tauri::command]
async fn core_history_preferences(
    state: tauri::State<'_, DesktopState>,
) -> Result<agent_protocol::HistoryPreferencesSnapshot, String> {
    state.core.history_preferences().await
}

#[tauri::command]
async fn core_agent_preferences(
    state: tauri::State<'_, DesktopState>,
) -> Result<agent_protocol::AgentPreferencesSnapshot, String> {
    state.core.agent_preferences().await
}

#[tauri::command]
async fn core_bootstrap(
    state: tauri::State<'_, DesktopState>,
) -> Result<BootstrapSnapshot, String> {
    state.core.bootstrap().await.map_err(|error| error.clone())
}

#[tauri::command]
async fn core_quota_snapshots(
    state: tauri::State<'_, DesktopState>,
    task_id: Option<String>,
    installation_id: Option<String>,
    force: Option<bool>,
) -> Result<Vec<agent_core::QuotaSnapshotView>, String> {
    let task_id = task_id
        .map(|value| value.parse::<TaskId>().map_err(|error| error.to_string()))
        .transpose()?;
    state
        .core
        .quota_snapshots_for(
            task_id.as_ref(),
            installation_id.as_deref(),
            force.unwrap_or(false),
        )
        .await
}

#[tauri::command]
async fn core_set_running_permission_mode(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: agent_domain::TaskId,
    run_id: agent_domain::RunId,
    executor: agent_domain::ExecutorRef,
    mode: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("权限策略只能在主窗口修改".into());
    }
    state
        .core
        .set_running_permission_mode(&task_id, &run_id, &executor, &mode)
        .await
}

#[tauri::command]
async fn core_permission_catalog(
    state: tauri::State<'_, DesktopState>,
    executor: agent_domain::ExecutorRef,
    task_id: Option<agent_domain::TaskId>,
) -> Result<driver_api::PermissionCatalog, String> {
    state
        .core
        .permission_catalog(&executor, task_id.as_ref())
        .await
}

#[tauri::command]
async fn core_scan_agent(
    state: tauri::State<'_, DesktopState>,
    provider_id: String,
) -> Result<agent_protocol::BootstrapSnapshot, String> {
    state.core.scan_agent(&provider_id).await
}

#[tauri::command]
async fn core_model_ready_executors(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<agent_domain::ExecutorRef>, String> {
    state.core.registered_executors().await
}

#[tauri::command]
async fn core_available_models(
    state: tauri::State<'_, DesktopState>,
    executor: agent_domain::ExecutorRef,
    selected_model: Option<String>,
    task_id: Option<agent_domain::TaskId>,
) -> Result<driver_api::ModelCatalog, String> {
    state
        .core
        .model_catalog_for_task(&executor, selected_model.as_deref(), task_id.as_ref())
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_pending_approvals(
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    limit: Option<usize>,
) -> Result<Vec<agent_protocol::PendingApproval>, String> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|error| error.to_string())?;
    state
        .core
        .pending_approvals(&task_id, limit.unwrap_or(20))
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_query_history(
    state: tauri::State<'_, DesktopState>,
    query: agent_core::history::HistoryQuery,
) -> Result<agent_core::history::HistoryPage, String> {
    state.core.query_history(&query).await
}

#[tauri::command]
async fn core_timeline(
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<Vec<TimelineItem>, String> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|error| error.to_string())?;
    state
        .core
        .timeline(&task_id)
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_timeline_snapshot(
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<agent_protocol::TimelineSnapshot, String> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|error| error.to_string())?;
    state.core.timeline_snapshot(&task_id).await
}

#[tauri::command]
async fn core_workspace_snapshots(
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    limit: Option<usize>,
) -> Result<Vec<WorkspaceSnapshotSummary>, String> {
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|error| error.to_string())?;
    state
        .core
        .workspace_snapshots(&task_id, limit.unwrap_or(20))
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_build_context_preview(
    state: tauri::State<'_, DesktopState>,
    request: ContextPreviewRequest,
) -> Result<BuiltContextPackage, String> {
    state
        .core
        .build_context_preview(request)
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_build_edited_context_preview(
    state: tauri::State<'_, DesktopState>,
    edit: ContextEditRequest,
) -> Result<BuiltContextPackage, String> {
    state
        .core
        .build_edited_context_preview(edit)
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_execute(
    state: tauri::State<'_, DesktopState>,
    mut envelope: CommandEnvelope<CoreCommand>,
) -> Result<CommandAck, String> {
    if matches!(
        &envelope.payload,
        CoreCommand::RecordTestResult(_)
            | CoreCommand::RecordInstallationProbe(_)
            | CoreCommand::ReconcileMissingInstallation(_)
    ) {
        return Err("internal commands are not available to the UI".into());
    }
    envelope.actor = ActorRef::LocalUser;
    state.core.execute(envelope).await
}

#[tauri::command]
async fn core_invalidate_pending_command(
    state: tauri::State<'_, DesktopState>,
    mut envelope: CommandEnvelope<CoreCommand>,
) -> Result<agent_core::command_invalidation::CommandInvalidation, String> {
    envelope.actor = ActorRef::LocalUser;
    state.core.invalidate_pending_command(envelope).await
}

#[tauri::command]
async fn core_command_receipt(
    state: tauri::State<'_, DesktopState>,
    mut envelope: CommandEnvelope<CoreCommand>,
) -> Result<Option<CommandAck>, String> {
    if matches!(
        &envelope.payload,
        CoreCommand::RecordTestResult(_)
            | CoreCommand::RecordInstallationProbe(_)
            | CoreCommand::ReconcileMissingInstallation(_)
    ) {
        return Err("internal commands are not available to the UI".into());
    }
    envelope.actor = ActorRef::LocalUser;
    state.core.command_receipt(envelope).await
}

#[tauri::command]
async fn core_invalidate_approval_response(
    state: tauri::State<'_, DesktopState>,
    query: agent_core::approval_receipts::ApprovalReceiptQuery,
    original_version: u64,
) -> Result<agent_core::approval_receipts::ApprovalInvalidation, String> {
    state
        .core
        .invalidate_approval_response(query, original_version)
        .await
}

#[tauri::command]
async fn core_prepare_approval_response(
    state: tauri::State<'_, DesktopState>,
    mut envelope: CommandEnvelope<CoreCommand>,
) -> Result<agent_core::approval_receipts::ApprovalReceiptQuery, String> {
    envelope.actor = ActorRef::LocalUser;
    state.core.prepare_approval_response(envelope).await
}

#[tauri::command]
async fn core_approval_receipt(
    state: tauri::State<'_, DesktopState>,
    query: agent_core::approval_receipts::ApprovalReceiptQuery,
) -> Result<Option<agent_core::approval_receipts::ApprovalReceipt>, String> {
    state.core.approval_receipt(query).await
}

#[tauri::command]
async fn core_sync_events(
    state: tauri::State<'_, DesktopState>,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<EventEnvelope>, String> {
    state
        .core
        .sync_events(after_sequence, limit)
        .await
        .map_err(|error| error.clone())
}

#[tauri::command]
async fn core_subscribe(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    after_sequence: u64,
    subscription_id: String,
    on_event: Channel<EventStreamMessage>,
) -> Result<(), String> {
    if subscription_id.trim().is_empty() {
        return Err("subscriptionId must not be empty".into());
    }
    let subscription = state
        .core
        .subscribe(after_sequence)
        .await
        .map_err(|error| error.clone())?;
    let core = Arc::clone(&state.core);
    let (cancel_tx, mut cancel_rx) = oneshot::channel();
    let old = state
        .subscriptions
        .lock()
        .map_err(|error| error.to_string())?
        .insert(subscription_id.clone(), cancel_tx);
    if let Some(old) = old {
        let _ = old.send(());
    }

    tauri::async_runtime::spawn(async move {
        tokio::select! {
            () = pump_event_stream(core, subscription, after_sequence, on_event) => {}
            _ = &mut cancel_rx => {}
        }
        // The pump may exit on its own (e.g. after a terminal stream error);
        // drop the cancel entry so the map does not grow across reconnects.
        if let Some(subscriptions) = app.try_state::<DesktopState>()
            && let Ok(mut subscriptions) = subscriptions.subscriptions.lock()
        {
            subscriptions.remove(&subscription_id);
        }
    });
    Ok(())
}

#[tauri::command]
fn core_unsubscribe(
    state: tauri::State<'_, DesktopState>,
    subscription_id: String,
) -> Result<(), String> {
    if let Some(cancel) = state
        .subscriptions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&subscription_id)
    {
        let _ = cancel.send(());
    }
    Ok(())
}

async fn pump_event_stream(
    core: Arc<daemon::CoreClient>,
    mut subscription: agent_core::EventSubscription,
    after_sequence: u64,
    channel: Channel<EventStreamMessage>,
) {
    let mut last_sequence = after_sequence;
    if channel
        .send(EventStreamMessage::ReplayStarted {
            after_sequence: last_sequence,
        })
        .is_err()
    {
        return;
    }
    if send_ordered(&channel, &subscription.replay, &mut last_sequence).is_err() {
        return;
    }
    if channel
        .send(EventStreamMessage::CaughtUp { last_sequence })
        .is_err()
    {
        return;
    }

    loop {
        match subscription.live.recv().await {
            Ok(event) if event.sequence <= last_sequence => {}
            Ok(event) if event.sequence == last_sequence + 1 => {
                if send_event(&channel, event, &mut last_sequence).is_err() {
                    return;
                }
            }
            Ok(_) | Err(RecvError::Lagged(_)) => {
                // A transient storage failure must not kill the ordered stream;
                // retry with backoff before handing the error to the UI, which
                // will then resubscribe from its last applied sequence.
                let mut attempt = 0_u32;
                loop {
                    match sync_all_events(&core, last_sequence).await {
                        Ok(events) => {
                            if send_ordered(&channel, &events, &mut last_sequence).is_err() {
                                return;
                            }
                            break;
                        }
                        Err(message) => {
                            attempt += 1;
                            if attempt >= 3 {
                                let _ = channel.send(EventStreamMessage::Error {
                                    message,
                                    retryable: true,
                                    last_sequence,
                                });
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(
                                200_u64 << attempt,
                            ))
                            .await;
                        }
                    }
                }
            }
            Err(RecvError::Closed) => {
                let _ = channel.send(EventStreamMessage::Error {
                    message: "Core event stream closed".into(),
                    retryable: true,
                    last_sequence,
                });
                return;
            }
        }
    }
}

async fn sync_all_events(
    core: &daemon::CoreClient,
    after_sequence: u64,
) -> Result<Vec<EventEnvelope>, String> {
    let mut cursor = after_sequence;
    let mut events = Vec::new();
    loop {
        let page = core
            .sync_events(cursor, 1_000)
            .await
            .map_err(|error| error.clone())?;
        let page_len = page.len();
        if let Some(last) = page.last() {
            cursor = last.sequence;
        }
        events.extend(page);
        if page_len < 1_000 {
            return Ok(events);
        }
    }
}

fn send_ordered(
    channel: &Channel<EventStreamMessage>,
    events: &[EventEnvelope],
    last_sequence: &mut u64,
) -> tauri::Result<()> {
    for event in events {
        if event.sequence <= *last_sequence {
            continue;
        }
        send_event(channel, event.clone(), last_sequence)?;
    }
    Ok(())
}

fn send_event(
    channel: &Channel<EventStreamMessage>,
    event: EventEnvelope,
    last_sequence: &mut u64,
) -> tauri::Result<()> {
    let sequence = event.sequence;
    channel.send(EventStreamMessage::Event(Box::new(event)))?;
    *last_sequence = sequence;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the desktop UI and connects to the independently owned Core service.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the platform event loop.
pub fn run() {
    tauri::Builder::default()
        .manage(agent_updates::AgentUpdates::default())
        .manage(workspace_browser::WorkspaceWatchers::default())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                window
                    .state::<workspace_browser::WorkspaceWatchers>()
                    .close_window(window.label());
            }
        })
        .on_page_load(|window, payload| {
            // Keep the static recovery screen reachable if the JS bundle fails.
            if window.label() == "main"
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
            {
                let window = window.window();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                });
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(not(feature = "review-acceptance"))]
            {
                let app_data = app.path().app_data_dir()?;
                #[cfg(debug_assertions)]
                let app_data = match std::env::var_os("DOGCODES_DESKTOP_DATA_DIR") {
                    Some(value) => {
                        let path = std::path::PathBuf::from(value);
                        if !path.is_absolute() {
                            return Err("isolated data directory must be absolute".into());
                        }
                        path
                    }
                    None => app_data,
                };
                app.manage(service_diagnostics::DiagnosticDirectory(app_data.clone()));
                app.manage(StartupExportGate(tokio::sync::Mutex::new(())));
                if let Ok(core) =
                    tauri::async_runtime::block_on(daemon::start_or_connect(&app_data))
                {
                    app.manage(StartupFailure(None));
                    app.manage(StartupStatus::Ready);
                    app.manage(DesktopState {
                        core: core.clone(),
                        subscriptions: Mutex::new(HashMap::new()),
                        export_gate: tokio::sync::Mutex::new(()),
                    });
                } else if SqliteStore::written_by_newer_version(app_data.join("app-v5.db")) {
                    // The service refuses newer data read-only; tell the user why.
                    app.manage(StartupFailure(Some(
                        StartupFailureCode::DatabaseFromNewerVersion,
                    )));
                    app.manage(StartupStatus::StorageRecoveryRequired);
                } else {
                    app.manage(StartupFailure(Some(StartupFailureCode::ServiceUnavailable)));
                    app.manage(StartupStatus::ServiceUnavailable);
                }
            }
            #[cfg(feature = "review-acceptance")]
            {
                #[cfg(not(feature = "review-acceptance"))]
                let app_data = app.path().app_data_dir()?;
                #[cfg(feature = "review-acceptance")]
                let app_data = review_acceptance::new_data_directory()?;
                #[cfg(feature = "review-acceptance")]
                app.manage(review_acceptance::AcceptanceDirectory(app_data.clone()));
                // Recovery may finalize abandoned turns only while this host owns
                // the database's lifetime lease. A second app must not recover a live host.
                app.manage(service_diagnostics::DiagnosticDirectory(app_data.clone()));
                app.manage(StartupExportGate(tokio::sync::Mutex::new(())));
                let lease = match acquire_host_lease(&app_data) {
                    Ok(lease) => lease,
                    Err(code) => {
                        app.manage(StartupFailure(Some(code)));
                        app.manage(StartupStatus::StorageRecoveryRequired);
                        return Ok(());
                    }
                };
                app.manage(lease);
                let (store, scratch_path) = match prepare_desktop_storage(&app_data) {
                    Ok(storage) => storage,
                    Err(code) => {
                        app.manage(StartupFailure(Some(code)));
                        app.manage(StartupStatus::StorageRecoveryRequired);
                        return Ok(());
                    }
                };
                let store = Arc::new(store);
                let core = Arc::new(
                    Core::with_workspace_context(
                        store,
                        Arc::new(
                            GitWorkspaceContextProvider::default()
                                .with_review_directory(app_data.join("review")),
                        ),
                    )
                    .with_history_query_program(std::env::current_exe()?),
                );
                if tauri::async_runtime::block_on(core.recover_interrupted_turns()).is_err() {
                    app.manage(StartupFailure(Some(
                        StartupFailureCode::HistoryRecoveryFailed,
                    )));
                    app.manage(StartupStatus::StorageRecoveryRequired);
                    return Ok(());
                }
                app.manage(StartupFailure(None));
                tauri::async_runtime::block_on(
                    core.register_driver(Arc::new(FakeDriver::new("fake")?)),
                );
                app.manage(DesktopState {
                    core: daemon::CoreClient::local_at(Arc::clone(&core), &app_data),
                    subscriptions: Mutex::new(HashMap::new()),
                    export_gate: tokio::sync::Mutex::new(()),
                });
                app.manage(StartupStatus::Ready);
                #[cfg(feature = "review-acceptance")]
                if let Some(window) = app.get_webview_window("main") {
                    window.set_title("dog.codes · ISOLATED REVIEW ACCEPTANCE")?;
                }
                #[cfg(feature = "review-acceptance")]
                let acceptance_app = app.handle().clone();
                let discovery_app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if !cfg!(feature = "review-acceptance")
                        || (cfg!(feature = "review-acceptance")
                            && std::env::var_os("DOGCODES_RECOVERY_GUI").is_some())
                    {
                        scan_builtin_agents(Arc::clone(&core), Some(discovery_app)).await;
                    }
                    ensure_initial_conversation(Arc::clone(&core), scratch_path.clone()).await;
                    #[cfg(feature = "review-acceptance")]
                    match review_acceptance::seed(&core, &scratch_path).await {
                        Ok(false) if !review_acceptance::restart_test() => return,
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("isolated review fixture failed: {error}");
                            return;
                        }
                    }
                    #[cfg(feature = "review-acceptance")]
                    if let Some(window) = acceptance_app.get_webview_window("main")
                        && std::env::var_os("DOGCODES_REVIEW_MANUAL").is_none()
                    {
                        let script = if review_acceptance::history_test() {
                            include_str!("review_acceptance_history.js")
                        } else if review_acceptance::live::mixed_test() {
                            include_str!("review_acceptance_live_mixed.js")
                        } else if review_acceptance::live_test() {
                            include_str!("review_acceptance_live.js")
                        } else if review_acceptance::multi_test() {
                            include_str!("review_acceptance_multi.js")
                        } else {
                            include_str!("review_acceptance.js")
                        };
                        if let Err(error) = window.eval(script) {
                            eprintln!("isolated review injection failed: {error}");
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_remote_control,
            external_links::open_website,
            agent_login::open_agent_login,
            attachments::core_save_attachment,
            attachments::core_attachment_upload_receipt,
            attachments::core_read_attachment,
            attachments::core_export_attachment,
            project_runs::core_project_runs,
            project_runs::watch::core_project_runs_watch,
            project_runs::watch::core_project_runs_unwatch,
            project_runs::core_project_runs_analyze,
            project_runs::core_project_run_edit,
            project_runs::core_project_run_operation,
            project_runs::core_project_run_open,
            terminal::core_terminal_open,
            terminal::core_terminal_list,
            terminal::core_terminal_get,
            terminal::core_terminal_read,
            terminal::core_terminal_input,
            terminal::core_terminal_resize,
            terminal::core_terminal_close,
            #[cfg(feature = "review-acceptance")]
            review_acceptance::report,
            #[cfg(feature = "review-acceptance")]
            review_acceptance::mutate_review_fixture,
            review_window::core_review_window_open,
            review_window::core_review_window_close,
            review_window::core_review_window_is_open,
            review_window::core_review_window_running,
            review::core_review_query,
            review::workspaces::core_review_workspaces,
            task_deletion::core_task_deletion,
            review::core_open_workspace_file,
            review::core_workspace_file_applications,
            review::core_reveal_workspace_file,
            workspace_browser::bridges::core_workspace_watch,
            workspace_browser::bridges::core_workspace_unwatch,
            workspace_browser::core_workspace_find_files,
            workspace_browser::core_workspace_roots,
            workspace_browser::core_workspace_directory,
            workspace_browser::core_workspace_preview,
            core_subagent_history,
            review::core_review_snapshot_lease,
            review::core_review_file,
            review::core_review_git_action,
            review::core_review_git_operation,
            review::git_workflow::core_review_git_advance,
            review::git_workflow::core_review_git_close,
            review::core_review_file_operation,
            review::core_review_file_close,
            review_repairs::core_review_repair,
            review::operation_history::core_review_operations,
            review::watch::core_review_watch,
            review::watch::core_review_unwatch,
            review::core_review_commit_summary,
            review::core_review_history,
            review::core_review_branches,
            review::core_review_index,
            review::core_review_discard,
            review::batch_file_actions::core_review_batch_files,
            review_file_actions::core_review_preview,
            review_file_actions::core_review_reveal,
            review_file_actions::core_review_ignore,
            core_pick_project_directory,
            core_reveal_local_project,
            core_register_local_project,
            core_update_local_project,
            restart_application,
            show_main_window,
            core_startup_status,
            core_startup_diagnostics,
            core_export_startup_diagnostics,
            core_startup_trace,
            core_bootstrap,
            core_agent_preferences,
            core_history_preferences,
            turn_review::core_review_executors,
            turn_review::core_turn_reviews,
            turn_review::core_fork_task,
            turn_review::core_fork_receipt,
            turn_review::core_review_source,
            turn_review::core_start_turn_review,
            turn_review::core_start_project_review,
            turn_review::core_cancel_turn_review,
            core_recovery_snapshot,
            core_prepare_session_rebuild,
            core_export_diagnostics,
            core_quota_snapshots,
            core_available_models,
            core_model_ready_executors,
            agent_updates::core_check_agent_update,
            agent_updates::core_upgrade_agent,
            agent_updates::core_open_agent_updater,
            core_scan_agent,
            core_permission_catalog,
            core_set_running_permission_mode,
            core_timeline,
            core_timeline_snapshot,
            core_query_history,
            core_pending_approvals,
            core_workspace_snapshots,
            core_task_artifacts,
            turn_undo::core_turn_undo,
            turn_undo::core_turn_undo_status,
            core_read_artifact,
            core_export_artifact,
            core_build_context_preview,
            core_build_edited_context_preview,
            core_execute,
            core_command_receipt,
            core_invalidate_pending_command,
            submission_intents::core_submission_intents,
            submission_intents::watch::core_submission_watch,
            submission_intents::watch::core_submission_unwatch,
            core_prepare_approval_response,
            core_invalidate_approval_response,
            core_approval_receipt,
            core_sync_events,
            core_subscribe,
            core_unsubscribe
        ])
        .run(tauri::generate_context!())
        .expect("failed to run dogcodes desktop");
}

async fn ensure_initial_conversation(core: Arc<Core>, scratch_path: std::path::PathBuf) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    if !snapshot.tasks.is_empty() {
        return;
    }

    let preferred_executor = snapshot
        .installations
        .iter()
        .filter(|installation| {
            installation.status == InstallationStatus::Ready
                && installation.provider_id.as_str() != "fake"
        })
        .min_by_key(|installation| {
            let priority = match installation.provider_id.as_str() {
                "codex" => 0,
                "claude-code" => 1,
                "kimi" => 2,
                "grok" => 3,
                _ => 10,
            };
            (priority, installation.provider_id.as_str())
        })
        .and_then(|installation| {
            ProviderId::parse(&installation.provider_id)
                .ok()
                .map(|provider_id| ExecutorRef {
                    provider_id,
                    installation_id: Some(installation.installation_id.to_string()),
                })
        })
        .unwrap_or_else(|| ExecutorRef {
            provider_id: ProviderId::parse("fake").expect("fake provider id is valid"),
            installation_id: None,
        });

    let project_id = if let Some(project) = snapshot.projects.first() {
        project.project_id.clone()
    } else {
        let project_id = ProjectId::new();
        let mut register = CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            additional_directories: vec![],
            project_id: project_id.clone(),
            display_name: "快速对话".into(),
            path: scratch_path.to_string_lossy().into_owned(),
        }));
        register.actor = ActorRef::SystemRecovery;
        if core.execute(register).await.is_err() {
            return;
        }
        project_id
    };

    let Ok(latest) = core.bootstrap().await else {
        return;
    };
    if !latest.tasks.is_empty() {
        return;
    }
    let mut create = CommandEnvelope::local(CoreCommand::CreateTask(CreateTask {
        task_id: TaskId::new(),
        project_id,
        title: "新对话".into(),
        preferred_executor,
    }));
    create.actor = ActorRef::SystemRecovery;
    let _ = core.execute(create).await;
}

async fn scan_builtin_agents(core: Arc<Core>, app: Option<tauri::AppHandle>) {
    let mut scans = tokio::task::JoinSet::new();
    for source in BUILTIN_AGENT_MANIFESTS {
        let core = Arc::clone(&core);
        let app = app.clone();
        scans.spawn(async move {
            let _ = scan_builtin_agent(core, source).await;
            if let Some(app) = app {
                let _ = app.emit("quota-provider-ready", ());
            }
        });
    }
    while scans.join_next().await.is_some() {}
}

const BUILTIN_AGENT_MANIFESTS: &[&str] = &[
    include_str!("../../../../drivers/manifests/codex.toml"),
    include_str!("../../../../drivers/manifests/claude-code.toml"),
    include_str!("../../../../drivers/manifests/kimi.toml"),
    include_str!("../../../../drivers/manifests/grok.toml"),
    include_str!("../../../../drivers/manifests/zcode.toml"),
    include_str!("../../../../drivers/manifests/cursor.toml"),
    include_str!("../../../../drivers/manifests/codebuddy.toml"),
    include_str!("../../../../drivers/manifests/antigravity.toml"),
    include_str!("../../../../drivers/manifests/pi.toml"),
    include_str!("../../../../drivers/manifests/opencode.toml"),
    include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
    include_str!("../../../../drivers/manifests/kiro.toml"),
];

async fn scan_builtin_agent(core: Arc<Core>, source: &'static str) -> Result<(), String> {
    static SCANS: std::sync::OnceLock<agent_scan::ScanCoordinator> = std::sync::OnceLock::new();
    let manifest =
        ProviderManifest::parse_toml(source).map_err(|_| "invalid agent manifest".to_string())?;
    let key = format!("{:p}:{}", Arc::as_ptr(&core), manifest.id);
    SCANS
        .get_or_init(Default::default)
        .run(key, async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(90),
                Box::pin(scan_builtin_agent_inner(core, source)),
            )
            .await
            .map_err(|_| "agent detection timed out".to_string())?
        })
        .await
}

async fn scan_builtin_agent_inner(core: Arc<Core>, source: &str) -> Result<(), String> {
    let manifest =
        ProviderManifest::parse_toml(source).map_err(|_| "invalid agent manifest".to_string())?;
    let context = DiscoveryContext::from_current_process();
    let preferred = match terminal_installation::preferred_executable(&manifest, &context).await {
        Ok(path) => path,
        Err(error) => {
            core.select_installation(manifest.id.as_str(), None).await;
            return Err(error);
        }
    };
    core.select_installation(
        manifest.id.as_str(),
        preferred
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )
    .await;
    let executables = preferred.into_iter().collect::<Vec<_>>();
    scan_discovered_agents(core, &manifest, executables).await
}

async fn scan_discovered_agents(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    executables: Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let snapshot = core
        .bootstrap()
        .await
        .map_err(|_| "installation snapshot unavailable".to_string())?;
    let discovered_paths = executables
        .iter()
        .map(|executable| executable.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    let registered = core.registered_executors().await;
    let mut readiness_paths = discovered_paths.clone();
    for executable in executables {
        let report = probe_installation(manifest, executable)
            .await
            .map_err(|_| "agent installation probe failed".to_string())?;
        // agy holds its credential lock for the entire live session, including
        // the idle time between turns. Re-probing a verified, unchanged driver
        // would wait behind that session and downgrade Ready until scan timeout.
        // Keep the same driver too: replacing it would orphan its live sessions.
        let reuse_antigravity = manifest.protocol.kind == "antigravity_stream_json"
            && matches!(report.status, ProbeStatus::Installed | ProbeStatus::Ready)
            && snapshot.installations.iter().any(|previous| {
                previous.provider_id == report.provider_id.as_str()
                    && std::path::Path::new(&previous.executable) == report.executable
                    && previous.fingerprint == report.fingerprint
                    && previous.status == InstallationStatus::Ready
                    && registered.iter().any(|executor| {
                        executor.provider_id.as_str() == previous.provider_id
                            && executor.installation_id.as_deref()
                                == Some(previous.installation_id.as_str())
                    })
            });
        let status = match report.status {
            ProbeStatus::Installed | ProbeStatus::Ready if reuse_antigravity => {
                readiness_paths.remove(&report.executable.to_string_lossy().into_owned());
                InstallationStatus::Ready
            }
            ProbeStatus::Installed => InstallationStatus::Installed,
            ProbeStatus::NotConfigured => InstallationStatus::NotConfigured,
            ProbeStatus::Ready => InstallationStatus::Ready,
            ProbeStatus::UnsupportedVersion => InstallationStatus::UnsupportedVersion,
            ProbeStatus::Broken => InstallationStatus::Broken,
        };
        let mut envelope = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            RecordInstallationProbe {
                provider_id: report.provider_id.to_string(),
                executable: report.executable.to_string_lossy().into_owned(),
                fingerprint: report.fingerprint,
                status,
                version: report.version.map(|version| version.to_string()),
                version_text: report.version_text,
                diagnostic: report.diagnostic,
                elapsed_millis: report.elapsed_millis,
            },
        ));
        envelope.actor = ActorRef::SystemRecovery;
        let _ = core.execute(envelope).await;
    }

    for installation in snapshot.installations.iter().filter(|installation| {
        is_missing_from_successful_scan(
            &installation.provider_id,
            &installation.executable,
            manifest.id.as_str(),
            &discovered_paths,
        )
    }) {
        let mut envelope = CommandEnvelope::local(CoreCommand::ReconcileMissingInstallation(
            ReconcileMissingInstallation {
                installation_id: installation.installation_id.clone(),
                provider_id: installation.provider_id.clone(),
                executable: installation.executable.clone(),
            },
        ));
        envelope.actor = ActorRef::SystemRecovery;
        let _ = core.execute(envelope).await;
    }

    match manifest.protocol.kind.as_str() {
        "app_server_json_rpc" => register_ready_codex_drivers(core, &discovered_paths).await,
        "acp" => register_ready_acp_drivers(core, manifest, &discovered_paths).await,
        "pi_rpc" => register_ready_pi_drivers(core, manifest, &discovered_paths).await,
        "claude_stream_json" => {
            register_ready_claude_drivers(core, manifest, &discovered_paths).await;
        }
        "antigravity_stream_json" => {
            register_ready_antigravity_drivers(core, manifest, &readiness_paths).await;
        }
        "zcode_app_server" => {
            register_ready_zcode_drivers(core, manifest, &discovered_paths).await;
        }
        // Custom protocols (e.g. ZCode app-server) stay discovery-only until a
        // dedicated driver crate lands behind an M0 fact probe.
        _ => {}
    }
    Ok(())
}

/// Runs the ACP initialize handshake for every discovered installation of an
/// `acp` provider and registers a driver when the agent really speaks v1.
async fn register_ready_acp_drivers(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    discovered_paths: &BTreeSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|installation| {
        installation.provider_id == manifest.id.as_str()
            && discovered_paths.contains(&installation.executable)
            && installation.status != InstallationStatus::UnsupportedVersion
    }) {
        let mut launch = acp_driver::AcpLaunch {
            executable: std::path::PathBuf::from(&installation.executable),
            args: manifest.discovery.launch_args.clone(),
        };
        // Trusted adapter option, not a manifest-supplied flag. Probing and
        // sessions must not start background installers alongside manual updates.
        if manifest.id.as_str() == "cursor" {
            launch.args.insert(0, "--disable-auto-update".into());
        }
        register_probed_acp_driver(&core, manifest, installation, launch).await;
    }
}

async fn register_probed_acp_driver(
    core: &Arc<Core>,
    manifest: &ProviderManifest,
    installation: &agent_protocol::InstallationSummary,
    launch: acp_driver::AcpLaunch,
) {
    let started = std::time::Instant::now();
    // Reuse the already authenticated process for providers measured to
    // support workspace-independent connections. CodeBuddy binds cwd at spawn.
    let readiness = if matches!(
        manifest.id.as_str(),
        "cursor" | "grok" | "kimi" | "opencode" | "kiro" | "deepseek-harness"
    ) {
        acp_driver::AcpDriver::connect_ready(
            manifest.id.as_str(),
            installation.installation_id.as_str(),
            launch,
            installation.version.clone(),
        )
        .await
    } else {
        match acp_driver::AcpDriver::probe_ready(&launch).await {
            Ok(report) => acp_driver::AcpDriver::new(
                manifest.id.as_str(),
                installation.installation_id.as_str(),
                launch,
                report
                    .native_version
                    .or_else(|| installation.version.clone()),
                report.resume_capability,
            )
            .map_err(|_| {
                driver_api::DriverError::Protocol("invalid ACP installation identity".into())
            }),
            Err(error) => Err(error),
        }
    };
    let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match readiness {
        Ok(driver) => {
            let driver = Arc::new(driver);
            core.register_driver(driver.clone()).await;
            // Native account queries do not submit model prompts.
            core.register_quota_provider(driver).await;
            if installation.status != InstallationStatus::Ready {
                record_readiness(
                    core,
                    installation,
                    InstallationStatus::Ready,
                    None,
                    elapsed_millis,
                )
                .await;
            }
        }
        Err(error @ driver_api::DriverError::NotConfigured(_)) => {
            record_readiness(
                core,
                installation,
                InstallationStatus::NotConfigured,
                Some(driver_api::failure::kind(&error).into()),
                elapsed_millis,
            )
            .await;
        }
        Err(_) => {
            record_readiness(
                core,
                installation,
                InstallationStatus::Broken,
                Some("ACP initialize handshake failed".into()),
                elapsed_millis,
            )
            .await;
        }
    }
}

async fn register_ready_pi_drivers(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    paths: &BTreeSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|i| {
        i.provider_id == manifest.id.as_str()
            && paths.contains(&i.executable)
            && i.status != InstallationStatus::UnsupportedVersion
    }) {
        let started = std::time::Instant::now();
        let executable = std::path::PathBuf::from(&installation.executable);
        let result = pi_driver::PiDriver::probe_ready(&executable).await;
        let (status, diagnostic) = match result {
            Ok(()) => {
                if let Ok(driver) = pi_driver::PiDriver::new(
                    installation.installation_id.as_str(),
                    executable,
                    installation.version.clone(),
                ) {
                    let driver = Arc::new(driver);
                    core.register_driver(driver.clone()).await;
                    core.register_quota_provider(driver).await;
                }
                (InstallationStatus::Ready, None)
            }
            Err(error @ driver_api::DriverError::NotConfigured(_)) => (
                InstallationStatus::NotConfigured,
                Some(driver_api::failure::kind(&error).into()),
            ),
            Err(_) => (
                InstallationStatus::Broken,
                Some("Pi RPC readiness probe failed".into()),
            ),
        };
        record_readiness(
            &core,
            installation,
            status,
            diagnostic,
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        )
        .await;
    }
}

/// Registers the native stream-json driver for Claude Code installations
/// after a real control-plane probe; no npm adapter is required or launched.
async fn register_ready_claude_drivers(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    discovered_paths: &BTreeSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|installation| {
        installation.provider_id == manifest.id.as_str()
            && discovered_paths.contains(&installation.executable)
            && installation.status != InstallationStatus::UnsupportedVersion
    }) {
        let started = std::time::Instant::now();
        let executable = std::path::PathBuf::from(&installation.executable);
        let readiness = ClaudeDriver::probe_ready(&executable).await;
        let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match readiness {
            Ok(()) => {
                let Ok(driver) = ClaudeDriver::new(
                    manifest.id.as_str(),
                    installation.installation_id.as_str(),
                    executable,
                    installation.version.clone(),
                    // Cross-process resume was verified at runtime on 2.1.218
                    // (real_resume_acceptance); evidence in
                    // docs/compatibility/claude-code.md.
                    driver_api::SessionResumeCapability::Supported,
                ) else {
                    continue;
                };
                let driver = Arc::new(driver);
                core.register_driver(driver.clone()).await;
                core.register_quota_provider(driver).await;
                if installation.status != InstallationStatus::Ready {
                    record_readiness(
                        &core,
                        installation,
                        InstallationStatus::Ready,
                        None,
                        elapsed_millis,
                    )
                    .await;
                }
            }
            Err(error @ driver_api::DriverError::NotConfigured(_)) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::NotConfigured,
                    Some(driver_api::failure::kind(&error).into()),
                    elapsed_millis,
                )
                .await;
            }
            Err(_) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::Broken,
                    Some("Claude Code stream-json handshake failed".into()),
                    elapsed_millis,
                )
                .await;
            }
        }
    }
}

async fn register_ready_codex_drivers(core: Arc<Core>, discovered_paths: &BTreeSet<String>) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|installation| {
        installation.provider_id == "codex"
            && discovered_paths.contains(&installation.executable)
            && installation.status != InstallationStatus::UnsupportedVersion
    }) {
        let Some(version) = installation.version.clone() else {
            continue;
        };
        let started = std::time::Instant::now();
        let readiness =
            CodexDriver::probe_ready(std::path::Path::new(&installation.executable)).await;
        let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match readiness {
            Ok(()) => {
                let Ok(driver) = CodexDriver::new(
                    std::path::Path::new(&installation.executable),
                    installation.installation_id.to_string(),
                    version,
                ) else {
                    continue;
                };
                let driver = Arc::new(driver);
                core.register_driver(driver.clone()).await;
                core.register_quota_provider(driver).await;
                if installation.status != InstallationStatus::Ready {
                    record_readiness(
                        &core,
                        installation,
                        InstallationStatus::Ready,
                        None,
                        elapsed_millis,
                    )
                    .await;
                }
            }
            Err(error @ driver_api::DriverError::NotConfigured(_)) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::NotConfigured,
                    Some(driver_api::failure::kind(&error).into()),
                    elapsed_millis,
                )
                .await;
            }
            Err(_) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::Broken,
                    Some("Codex App Server readiness probe failed".into()),
                    elapsed_millis,
                )
                .await;
            }
        }
    }
}

/// Registers the native stream-json driver for Antigravity installations
/// after a real event-surface probe; no extra dependencies are required.
async fn register_ready_antigravity_drivers(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    discovered_paths: &BTreeSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|installation| {
        installation.provider_id == manifest.id.as_str()
            && discovered_paths.contains(&installation.executable)
            && installation.status != InstallationStatus::UnsupportedVersion
    }) {
        let started = std::time::Instant::now();
        let executable = std::path::PathBuf::from(&installation.executable);
        let readiness = antigravity_driver::AntigravityDriver::probe_ready(&executable).await;
        let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match readiness {
            Ok(()) => {
                let Ok(driver) = antigravity_driver::AntigravityDriver::new(
                    manifest.id.as_str(),
                    installation.installation_id.as_str(),
                    executable,
                    installation.version.clone(),
                    // Cross-process resume via `--conversation` was verified at
                    // runtime on 1.1.26 (real_resume_acceptance).
                    driver_api::SessionResumeCapability::Supported,
                ) else {
                    continue;
                };
                let driver = Arc::new(driver);
                core.register_driver(driver.clone()).await;
                core.register_quota_provider(driver).await;
                // Eligibility/auth failures surface on the first real turn;
                // the installation stays Ready-but-unverified rather than
                // being marked Broken by a regional account check.
                if installation.status != InstallationStatus::Ready {
                    record_readiness(
                        &core,
                        installation,
                        InstallationStatus::Ready,
                        None,
                        elapsed_millis,
                    )
                    .await;
                }
            }
            Err(error @ driver_api::DriverError::NotConfigured(_)) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::NotConfigured,
                    Some(driver_api::failure::kind(&error).into()),
                    elapsed_millis,
                )
                .await;
            }
            Err(_) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::Broken,
                    Some("agy stream-json event surface probe failed".into()),
                    elapsed_millis,
                )
                .await;
            }
        }
    }
}

/// Registers the native account monitor independently of model readiness.
/// Only a successful zero-model-call probe registers the execution driver.
async fn register_ready_zcode_drivers(
    core: Arc<Core>,
    manifest: &ProviderManifest,
    discovered_paths: &BTreeSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations.iter().filter(|installation| {
        installation.provider_id == manifest.id.as_str()
            && discovered_paths.contains(&installation.executable)
            && installation.status != InstallationStatus::UnsupportedVersion
    }) {
        let started = std::time::Instant::now();
        // The manifest probes the app binary; the driver additionally needs
        // the bundled CLI script next to it.
        let cli_script = installation
            .executable
            .replace("Contents/MacOS/ZCode", "Contents/Resources/glm/zcode.cjs");
        let launch = zcode_driver::ZcodeLaunch {
            app_binary: std::path::PathBuf::from(&installation.executable),
            cli_script: std::path::PathBuf::from(&cli_script),
        };
        // The personal account's monitor GET does not need a model catalog or
        // an app-server session. Keep it available when model readiness fails.
        if let Ok(monitor) = ZcodeDriver::new(
            manifest.id.as_str(),
            installation.installation_id.as_str(),
            launch.clone(),
            installation.version.clone(),
            zcode_driver::verified_resume_capability(installation.version.as_deref()),
        ) {
            core.register_quota_provider(Arc::new(monitor)).await;
        }
        let readiness = ZcodeDriver::connect_ready(
            manifest.id.as_str(),
            installation.installation_id.as_str(),
            launch,
            installation.version.clone(),
            zcode_driver::verified_resume_capability(installation.version.as_deref()),
        )
        .await;
        let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match readiness {
            Ok(driver) => {
                let driver = Arc::new(driver);
                core.register_driver(driver.clone()).await;
                // Reuse the ready driver for both account and task-scoped usage.
                core.register_quota_provider(driver).await;
                if installation.status != InstallationStatus::Ready {
                    record_readiness(
                        &core,
                        installation,
                        InstallationStatus::Ready,
                        None,
                        elapsed_millis,
                    )
                    .await;
                }
            }
            Err(error @ driver_api::DriverError::NotConfigured(_)) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::NotConfigured,
                    Some(driver_api::failure::kind(&error).into()),
                    elapsed_millis,
                )
                .await;
            }
            Err(_) => {
                record_readiness(
                    &core,
                    installation,
                    InstallationStatus::Broken,
                    Some("zcode app-server probe failed".into()),
                    elapsed_millis,
                )
                .await;
            }
        }
    }
}

async fn record_readiness(
    core: &Arc<Core>,
    installation: &agent_protocol::InstallationSummary,
    status: InstallationStatus,
    diagnostic: Option<String>,
    elapsed_millis: u64,
) {
    let mut envelope = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        RecordInstallationProbe {
            provider_id: installation.provider_id.clone(),
            executable: installation.executable.clone(),
            fingerprint: installation.fingerprint.clone(),
            status,
            version: installation.version.clone(),
            version_text: installation.version_text.clone(),
            diagnostic,
            elapsed_millis,
        },
    ));
    envelope.actor = ActorRef::SystemRecovery;
    let _ = core.execute(envelope).await;
}

fn is_missing_from_successful_scan(
    installation_provider: &str,
    installation_executable: &str,
    scanned_provider: &str,
    discovered_paths: &BTreeSet<String>,
) -> bool {
    installation_provider == scanned_provider
        && !discovered_paths.contains(installation_executable)
        // PATH and manifest candidates are discovery hints, not proof that a
        // previously verified absolute executable was uninstalled. Desktop
        // app bundles are especially easy to miss when the launcher inherits
        // a different PATH. Only emit `installation_removed` after the stored
        // executable itself has actually disappeared.
        && matches!(
            std::fs::metadata(installation_executable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn zcode_account_quota_survives_model_readiness_failure() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("Contents/MacOS/ZCode");
        let resources = directory.path().join("Contents/Resources");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(resources.join("glm")).unwrap();
        std::fs::create_dir_all(resources.join("config/provider")).unwrap();
        std::fs::write(resources.join("glm/zcode.cjs"), "").unwrap();
        std::fs::write(resources.join("config/provider/zcode-builtin.json"), "{}").unwrap();
        // Isolated native/account helpers. No user credentials or provider calls.
        std::fs::write(&executable, r#"#!/bin/sh
if [ "$1" = "-e" ]; then
  input=$(cat)
  case "$input" in
    *quota_identity*) echo '{"identity":"fixture-account"}' ;;
    *'"action":"quota"'*) echo '{"identity":"fixture-account","snapshot":{"windows":[{"id":"5h","scope":null,"windowMinutes":300,"usedPercent":13,"resetsAtUnix":null}],"context":null,"collectedAtUnixMs":1,"staleAfterUnixMs":60001,"planType":"BigModel","source":"native_credentials_pull","confidence":"provider_reported","parserVersion":"fixture"}}' ;;
    *) echo '{"overlay":{"revision":"fixture","providers":{},"states":{}},"authorized":{},"checkedAt":0,"version":"fixture"}' ;;
  esac
  exit 0
fi
while IFS= read -r request; do
  id=$(printf '%s' "$request" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$request" in
    *provider/updateAccountConfig*) echo "{\"id\":$id,\"result\":{\"receivedRevision\":\"fixture\"}}" ;;
    *) echo "{\"id\":$id,\"error\":{\"code\":-32603,\"message\":\"Model config is missing\"}}" ;;
  esac
done
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        #[cfg(target_os = "macos")]
        {
            let helper = directory
                .path()
                .join("Contents/Frameworks/ZCode Helper.app/Contents");
            std::fs::create_dir_all(helper.join("MacOS")).unwrap();
            std::fs::copy(&executable, helper.join("MacOS/ZCode Helper")).unwrap();
            let mut metadata = plist::Dictionary::new();
            metadata.insert("LSUIElement".into(), plist::Value::Boolean(true));
            plist::Value::Dictionary(metadata)
                .to_file_xml(helper.join("Info.plist"))
                .unwrap();
        }
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../../drivers/manifests/zcode.toml"))
                .unwrap();
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            RecordInstallationProbe {
                provider_id: "zcode".into(),
                executable: executable.to_string_lossy().into_owned(),
                fingerprint: "fixture".into(),
                status: InstallationStatus::Installed,
                version: Some("0.16.9".into()),
                version_text: Some("0.16.9".into()),
                diagnostic: None,
                elapsed_millis: 0,
            },
        ));
        probe.actor = ActorRef::SystemRecovery;
        core.execute(probe).await.unwrap();
        register_ready_zcode_drivers(
            core.clone(),
            &manifest,
            &BTreeSet::from([executable.to_string_lossy().into_owned()]),
        )
        .await;
        let installation = core.bootstrap().await.unwrap().installations.remove(0);
        assert_eq!(installation.status, InstallationStatus::NotConfigured);
        assert!(core.registered_executors().await.is_empty());
        // Exercise the desktop RPC entry point, not just the helper directly.
        let client = daemon::CoreClient::local(core);
        let views = client
            .quota_snapshots_for(None, Some(installation.installation_id.as_str()), true)
            .await
            .unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].diagnostic.is_none(), "{views:?}");
        assert_eq!(
            views[0].quota.as_ref().unwrap().windows[0].used_percent,
            Some(13.0)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn antigravity_rescan_preserves_registered_driver_and_ready_status() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("agy");
        let script = r#"#!/bin/sh
case "$1" in
  --version) echo 1.2.13 ;;
  models) echo gemini-fixture ;;
  *)
    echo probe >> "${0}.probes"
    echo '{"event":"init"}'
    cat > /dev/null
    ;;
esac
"#;
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/antigravity.toml"
        ))
        .unwrap();
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        core.select_installation(
            "antigravity",
            Some(executable.to_string_lossy().into_owned()),
        )
        .await;
        // The first scan must probe the binary and may initialize native proxy
        // services. The deadline below applies to rescanning a registered driver.
        scan_discovered_agents(core.clone(), &manifest, vec![executable.clone()])
            .await
            .unwrap();
        for _ in 0..2 {
            core.select_installation(
                "antigravity",
                Some(executable.to_string_lossy().into_owned()),
            )
            .await;
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                scan_discovered_agents(core.clone(), &manifest, vec![executable.clone()]),
            )
            .await
            .expect("rescan must not wait for a live session's credential lock")
            .unwrap();
            assert_eq!(
                core.bootstrap().await.unwrap().installations[0].status,
                InstallationStatus::Ready
            );
        }
        let probes = executable.with_extension("probes");
        assert_eq!(std::fs::read_to_string(&probes).unwrap(), "probe\n");

        // A changed binary must still be validated, even with a registered driver.
        std::fs::write(&executable, format!("{script}\n# upgraded binary\n")).unwrap();
        scan_discovered_agents(core.clone(), &manifest, vec![executable.clone()])
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&probes).unwrap(), "probe\nprobe\n");

        // Persisted Ready alone is insufficient after a daemon restart.
        let cold_core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let installation = core.bootstrap().await.unwrap().installations.remove(0);
        record_readiness(
            &cold_core,
            &installation,
            InstallationStatus::Ready,
            None,
            0,
        )
        .await;
        scan_discovered_agents(cold_core.clone(), &manifest, vec![executable])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&probes).unwrap(),
            "probe\nprobe\nprobe\n"
        );
        assert_eq!(cold_core.registered_executors().await.len(), 1);
        core.select_installation("antigravity", Some("/different/agy".into()))
            .await;
        assert!(core.registered_executors().await.is_empty());
    }

    #[tokio::test]
    async fn local_project_is_persisted_and_new_task_uses_selected_directory() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("项目 with spaces");
        std::fs::create_dir(&workspace).unwrap();
        let database = directory.path().join("state.db");
        let task_id = TaskId::new();
        let project_id;
        {
            let core = Arc::new(Core::new(Arc::new(SqliteStore::open(&database).unwrap())));
            project_id = register_local_project(
                &daemon::CoreClient::local(core.clone()),
                String::new(),
                workspace.to_str().unwrap().into(),
            )
            .await
            .unwrap();
            core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
                CreateTask {
                    task_id: task_id.clone(),
                    project_id: project_id.clone(),
                    title: "目录验证".into(),
                    preferred_executor: ExecutorRef {
                        provider_id: ProviderId::parse("fake").unwrap(),
                        installation_id: None,
                    },
                },
            )))
            .await
            .unwrap();
        }
        // Reopen the real SQLite store, rather than checking only in-memory UI state.
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open(&database).unwrap())));
        let snapshot = core.bootstrap().await.unwrap();
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .unwrap();
        assert_eq!(task.project_id, project_id);
        let project = snapshot
            .projects
            .iter()
            .find(|project| project.project_id == task.project_id)
            .unwrap();
        assert_eq!(
            project.path,
            workspace.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(project.display_name, "项目 with spaces");
        assert!(!workspace.join(".git").exists());
    }

    #[tokio::test]
    async fn local_project_rejects_missing_paths_files_and_relative_paths_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "fixture").unwrap();
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        for path in [
            file.to_str().unwrap().to_owned(),
            directory.path().join("missing").to_str().unwrap().into(),
            "relative/path".into(),
        ] {
            assert!(
                register_local_project(
                    &daemon::CoreClient::local(core.clone()),
                    String::new(),
                    path
                )
                .await
                .is_err()
            );
        }
        assert!(core.bootstrap().await.unwrap().projects.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_project_reuses_existing_directory_through_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&workspace).unwrap();
        std::os::unix::fs::symlink(&workspace, &alias).unwrap();
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let first = register_local_project(
            &daemon::CoreClient::local(core.clone()),
            "original".into(),
            workspace.to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let second = register_local_project(
            &daemon::CoreClient::local(core.clone()),
            "original".into(),
            alias.to_str().unwrap().into(),
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(core.bootstrap().await.unwrap().projects.len(), 1);
    }

    #[tokio::test]
    async fn multi_directory_projects_normalize_persist_and_keep_distinct_roots() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("frontend");
        let second = directory.path().join("backend");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store));
        let project = register_local_multi_project(
            &daemon::CoreClient::local(core.clone()),
            "Product".into(),
            first.to_str().unwrap().into(),
            vec![
                second.to_str().unwrap().into(),
                second.to_str().unwrap().into(),
                first.to_str().unwrap().into(),
            ],
        )
        .await
        .unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        let saved = snapshot
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .unwrap();
        assert_eq!(
            saved.additional_directories,
            vec![second.canonicalize().unwrap().to_str().unwrap()]
        );
        let single = register_local_multi_project(
            &daemon::CoreClient::local(core.clone()),
            "Product".into(),
            first.to_str().unwrap().into(),
            vec![],
        )
        .await
        .unwrap();
        assert_ne!(single, project);
        assert!(
            register_local_multi_project(
                &daemon::CoreClient::local(core.clone()),
                "Bad".into(),
                first.to_str().unwrap().into(),
                vec!["relative".into()]
            )
            .await
            .is_err()
        );
        assert_eq!(core.bootstrap().await.unwrap().projects.len(), 2);
    }

    #[tokio::test]
    async fn editing_local_projects_normalizes_paths_and_rejects_stale_or_invalid_updates() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let client = daemon::CoreClient::local(core.clone());
        let path = first.path().to_str().unwrap().to_owned();
        let other = second.path().to_str().unwrap().to_owned();
        let id = register_local_multi_project(&client, "Before".into(), path.clone(), vec![])
            .await
            .unwrap();
        update_local_project(
            &client,
            id.clone(),
            1,
            " After ".into(),
            other.clone(),
            vec![path.clone(), path.clone(), other.clone()],
        )
        .await
        .unwrap();
        let project = core.bootstrap().await.unwrap().projects.remove(0);
        assert_eq!(project.project_id, id);
        assert_eq!(project.display_name, "After");
        assert_eq!(
            project.path,
            second.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            project.additional_directories,
            vec![first.path().canonicalize().unwrap().to_str().unwrap()]
        );
        assert_eq!(project.stream_version, 2);
        assert!(
            update_local_project(&client, id.clone(), 1, "Stale".into(), path.clone(), vec![])
                .await
                .unwrap_err()
                .contains("项目已被修改")
        );
        assert!(
            update_local_project(
                &client,
                id,
                2,
                "Invalid".into(),
                path,
                vec!["relative".into()]
            )
            .await
            .is_err()
        );
        store.rebuild_projections().unwrap();
        assert_eq!(core.bootstrap().await.unwrap().projects, vec![project]);
    }

    #[test]
    fn startup_failures_are_classified_without_private_data_and_lease_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let lease = acquire_host_lease(dir.path()).unwrap();
        assert!(matches!(
            acquire_host_lease(dir.path()),
            Err(StartupFailureCode::HostLeaseUnavailable)
        ));
        drop(lease);
        assert!(acquire_host_lease(dir.path()).is_ok());
        std::fs::write(dir.path().join("app-v5.db"), b"private corrupt fixture").unwrap();
        assert!(matches!(
            prepare_desktop_storage(dir.path()),
            Err(StartupFailureCode::DatabaseCheckFailed)
        ));
        let value = startup_diagnostic_value(&StartupFailure(Some(
            StartupFailureCode::DatabaseCheckFailed,
        )))
        .to_string();
        assert!(value.contains("database_check_failed"));
        assert!(!value.contains("private"));
        assert!(!value.contains(dir.path().to_str().unwrap()));
        assert!(!dir.path().join("scratch").exists());
    }

    #[test]
    fn failed_storage_preparation_preserves_database_and_does_not_create_scratch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app-v5.db");
        std::fs::write(&path, b"damaged fixture").unwrap();
        assert!(prepare_desktop_storage(directory.path()).is_err());
        assert!(!directory.path().join("scratch").exists());
        assert_eq!(std::fs::read(path).unwrap(), b"damaged fixture");
    }

    #[test]
    fn newer_database_format_has_its_own_startup_code_and_is_left_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app-v5.db");
        drop(prepare_desktop_storage(directory.path()).unwrap());
        assert!(!SqliteStore::written_by_newer_version(&path));
        rusqlite::Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", agent_store::STORE_FORMAT_VERSION + 1)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(SqliteStore::written_by_newer_version(&path));
        assert!(matches!(
            prepare_desktop_storage(directory.path()),
            Err(StartupFailureCode::DatabaseFromNewerVersion)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn first_launch_prepares_store_and_scratch() {
        let directory = tempfile::tempdir().unwrap();
        let (_store, scratch) = prepare_desktop_storage(directory.path()).unwrap();
        assert!(scratch.is_dir());
        assert!(directory.path().join("app-v5.db").is_file());
    }

    #[test]
    fn artifact_commands_are_restricted_to_the_main_window() {
        assert!(require_main_artifact_window("main").is_ok());
        assert!(require_main_artifact_window("companion").is_err());
        assert!(require_main_artifact_window("remote").is_err());
    }

    #[test]
    fn successful_scan_only_reconciles_paths_that_actually_disappeared() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("codex");
        std::fs::write(&existing, b"fixture").unwrap();
        let existing = existing.to_string_lossy().into_owned();
        let missing = directory
            .path()
            .join("removed-codex")
            .to_string_lossy()
            .into_owned();
        let discovered = BTreeSet::from([existing.clone()]);

        assert!(!is_missing_from_successful_scan(
            "codex",
            &existing,
            "codex",
            &discovered,
        ));
        // A transient PATH/manifest miss must not delete a still-present
        // desktop-bundled runtime from the installation projection.
        assert!(!is_missing_from_successful_scan(
            "codex",
            &existing,
            "codex",
            &BTreeSet::new(),
        ));
        assert!(is_missing_from_successful_scan(
            "codex",
            &missing,
            "codex",
            &discovered,
        ));
        assert!(!is_missing_from_successful_scan(
            "claude",
            &missing,
            "codex",
            &discovered,
        ));
    }

    #[test]
    fn the_claude_manifest_targets_the_native_stream_json_channel() {
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        assert_eq!(manifest.protocol.kind, "claude_stream_json");
        assert!(manifest.discovery.requires_executables.is_empty());
    }

    #[test]
    fn every_embedded_agent_manifest_is_valid() {
        for source in [
            include_str!("../../../../drivers/manifests/codex.toml"),
            include_str!("../../../../drivers/manifests/claude-code.toml"),
            include_str!("../../../../drivers/manifests/kimi.toml"),
            include_str!("../../../../drivers/manifests/grok.toml"),
            include_str!("../../../../drivers/manifests/zcode.toml"),
            include_str!("../../../../drivers/manifests/cursor.toml"),
            include_str!("../../../../drivers/manifests/codebuddy.toml"),
            include_str!("../../../../drivers/manifests/antigravity.toml"),
            include_str!("../../../../drivers/manifests/pi.toml"),
            include_str!("../../../../drivers/manifests/opencode.toml"),
            include_str!("../../../../drivers/manifests/deepseek-harness.toml"),
            include_str!("../../../../drivers/manifests/kiro.toml"),
        ] {
            ProviderManifest::parse_toml(source).unwrap();
        }
    }

    #[test]
    fn acp_providers_declare_a_launch_mode() {
        for source in [
            include_str!("../../../../drivers/manifests/kimi.toml"),
            include_str!("../../../../drivers/manifests/grok.toml"),
            include_str!("../../../../drivers/manifests/cursor.toml"),
            include_str!("../../../../drivers/manifests/codebuddy.toml"),
        ] {
            let manifest = ProviderManifest::parse_toml(source).unwrap();
            assert!(
                !manifest.discovery.launch_args.is_empty()
                    || !manifest.discovery.requires_executables.is_empty(),
                "{} must declare launch args or an adapter requirement",
                manifest.id.as_str(),
            );
        }
    }

    #[tokio::test]
    async fn empty_installation_creates_a_scratch_conversation_with_offline_fallback() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store));
        let scratch = tempfile::tempdir().unwrap();

        ensure_initial_conversation(Arc::clone(&core), scratch.path().to_path_buf()).await;

        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].display_name, "快速对话");
        assert_eq!(snapshot.projects[0].path, scratch.path().to_string_lossy());
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(
            snapshot.tasks[0].preferred_executor.provider_id.as_str(),
            "fake"
        );
        assert!(
            snapshot.tasks[0]
                .preferred_executor
                .installation_id
                .is_none()
        );
    }

    #[tokio::test]
    async fn scratch_conversation_prefers_the_exact_ready_agent_installation() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store));
        let scratch = tempfile::tempdir().unwrap();
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            RecordInstallationProbe {
                provider_id: "codex".into(),
                executable: "/opt/codex/bin/codex".into(),
                fingerprint: "ready-codex".into(),
                status: InstallationStatus::Ready,
                version: Some("1.0.0".into()),
                version_text: Some("codex 1.0.0".into()),
                diagnostic: None,
                elapsed_millis: 3,
            },
        ));
        probe.actor = ActorRef::SystemRecovery;
        core.execute(probe).await.unwrap();
        let installation_id = core.bootstrap().await.unwrap().installations[0]
            .installation_id
            .to_string();

        ensure_initial_conversation(Arc::clone(&core), scratch.path().to_path_buf()).await;

        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(
            snapshot.tasks[0].preferred_executor.provider_id.as_str(),
            "codex"
        );
        assert_eq!(
            snapshot.tasks[0]
                .preferred_executor
                .installation_id
                .as_deref(),
            Some(installation_id.as_str()),
        );
    }
}

#[tauri::command]
async fn core_subagent_history(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    item_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    require_main_artifact_window(window.label())?;
    state
        .core
        .subagent_history(&task_id, &item_id)
        .await
        .map_err(|_| "子智能体执行记录加载失败".into())
}
