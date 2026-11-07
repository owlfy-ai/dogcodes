//! Desktop window checks and adapters for the workspace operations the service runs.
use crate::workspace_browser::bridges::WorkspaceWatchers;
use crate::{DesktopState, review_window::require_task_window};
use agent_domain::{ArtifactId, TaskId};
use agent_workspace::review::recovery::batch::FileBatchResult;
use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;
use workspace_ops::review::{
    BranchSummary, CommitSummary, DiscardRequest, IndexAction, LeaseRequest, ReviewFile,
    ReviewQuery, ReviewSnapshot,
    batch_file_actions::BatchFileRequest,
    git_operations::{self, GitReceipt},
    git_workflow, mutation_operations,
    operation_history::{HistoryPage, HistoryQuery},
    service::{self, ReviewCall},
    workspaces::ReviewWorkspace,
};
use workspace_ops::review_file_actions::{PreviewVersion, reveal_file};
use workspace_ops::turn_undo::UndoRequest;
use workspace_ops::workspace_service::{Preview, watch::WatchMessage};

// review
#[tauri::command]
pub async fn core_review_query(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    query: ReviewQuery,
) -> Result<ReviewSnapshot, String> {
    require_task_window(window.label(), &query.task_id)?;
    state.core.review(service::ReviewCall::Query(query)).await
}

#[tauri::command]
pub async fn core_review_snapshot_lease(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: LeaseRequest,
) -> Result<(), String> {
    require_task_window(window.label(), &request.task_id)?;
    state.core.review(service::ReviewCall::Lease(request)).await
}

#[tauri::command]
pub async fn core_review_file(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<ReviewFile, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::File {
            task_id,
            snapshot_id,
            root_id,
            path,
        })
        .await
}

#[tauri::command]
pub async fn core_review_history(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    root_id: String,
    skip: usize,
) -> Result<Vec<CommitSummary>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::History {
            task_id,
            root_id,
            skip,
        })
        .await
}

#[tauri::command]
pub async fn core_review_branches(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    root_id: String,
) -> Result<Vec<BranchSummary>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::Branches { task_id, root_id })
        .await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn core_review_index(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    operation_id: String,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    paths: Vec<String>,
    action: IndexAction,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::Index {
            operation_id,
            task_id,
            snapshot_id,
            root_id,
            paths,
            action,
        })
        .await
}

#[tauri::command]
pub async fn core_review_discard(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    operation_id: String,
    request: DiscardRequest,
) -> Result<String, String> {
    require_task_window(window.label(), &request.task_id)?;
    state
        .core
        .review(service::ReviewCall::Discard {
            operation_id,
            request,
        })
        .await
}

#[tauri::command]
pub async fn core_reveal_workspace_file(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    path: String,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    let path = workspace_file(&state, &task_id, &path).await?;
    crate::review_file_actions::reveal_file(path).await
}

/// Where a file really is, on whichever machine its task runs.
#[tauri::command]
pub async fn core_workspace_file_path(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    path: String,
) -> Result<PathBuf, String> {
    require_task_window(window.label(), &task_id)?;
    workspace_file(&state, &task_id, &path).await
}

#[tauri::command]
pub async fn core_reveal_workspace_directory(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    path: String,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    let path = state
        .core
        .review(service::ReviewCall::WorkspacePath {
            task_id,
            path,
            directory: true,
        })
        .await?;
    crate::review_file_actions::reveal_file(path).await
}

#[tauri::command]
pub async fn core_workspace_file_applications(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    path: String,
) -> Result<WorkspaceFileApplications, String> {
    require_task_window(window.label(), &task_id)?;
    let path = workspace_file(&state, &task_id, &path).await?;
    let icon_path = path.clone();
    let applications = tokio::task::spawn_blocking(move || open_with::applications(&icon_path))
        .await
        .unwrap_or_else(|_| Err("查询打开方式失败".into()));
    // Discovery and copying share one authorized resolution. A discovery error
    // must not discard the resolved file path or disable the fixed footer.
    Ok(WorkspaceFileApplications::from_result(path, applications))
}

/// Application IDs are re-resolved against the current system recommendations;
/// the frontend cannot supply arbitrary executable paths or command arguments.
#[tauri::command]
pub async fn core_open_workspace_file(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    path: String,
    application_id: Option<String>,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    let candidate = workspace_file(&state, &task_id, &path).await?;
    if let Some(id) = application_id {
        return tokio::task::spawn_blocking(move || open_with::open_with(&candidate, &id))
            .await
            .map_err(|_| "打开文件失败")?;
    }
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = tokio::process::Command::new("xdg-open");
    command.arg(candidate).kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), command.status())
        .await
        .map_err(|_| "打开文件超时")?
        .map_err(|_| "无法打开文件")?;
    if status.success() {
        Ok(())
    } else {
        Err("无法打开文件，请检查此文件类型的默认应用设置".into())
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn core_review_git_action(
    window: tauri::WebviewWindow,
    operation_id: String,
    task_id: String,
    root_id: String,
    action: String,
    message: String,
    stage_all: Option<bool>,
    snapshot_id: Option<String>,
    workflow: Option<git_workflow::GitWorkflow>,
) -> Result<bool, String> {
    require_task_window(window.label(), &task_id)?;
    let state = window
        .try_state::<DesktopState>()
        .ok_or("Desktop state unavailable")?;
    state
        .core
        .review(service::ReviewCall::GitAction(git_operations::GitRequest {
            operation_id,
            task_id,
            root_id,
            action,
            message,
            stage_all,
            snapshot_id,
            workflow,
        }))
        .await
}

#[tauri::command]
pub async fn core_review_commit_summary(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    root_id: String,
    executor: agent_domain::ExecutorRef,
    language: Option<String>,
) -> Result<String, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::CommitSummary {
            task_id,
            root_id,
            executor,
            language,
        })
        .await
}

#[tauri::command]
pub async fn core_review_git_operation(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<Option<git_operations::GitReceipt>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::GitOperation {
            task_id,
            operation_id,
        })
        .await
}

#[tauri::command]
pub async fn core_review_git_credential(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<Option<workspace_ops::review::git_credentials::CredentialPrompt>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::GitCredential { task_id })
        .await
}

#[tauri::command]
pub async fn core_review_git_credential_answer(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    id: u64,
    answer: Option<String>,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::GitCredentialAnswer {
            task_id,
            id,
            answer,
        })
        .await
}

#[tauri::command]
pub async fn core_review_file_operation(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<Option<mutation_operations::MutationReceipt>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::FileOperation {
            task_id,
            operation_id,
        })
        .await
}

#[tauri::command]
pub async fn core_review_file_close(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<mutation_operations::MutationReceipt, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(service::ReviewCall::FileClose {
            task_id,
            operation_id,
        })
        .await
}

// turn_undo
#[tauri::command]
pub async fn core_turn_undo_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    artifact_id: ArtifactId,
) -> Result<bool, String> {
    require_task_window(window.label(), &task_id.to_string())?;
    state
        .core
        .review(ReviewCall::UndoStatus {
            task_id,
            artifact_id,
        })
        .await
}

#[tauri::command]
pub async fn core_turn_undo(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    operation_id: String,
    request: UndoRequest,
) -> Result<(), String> {
    require_task_window(window.label(), &request.task_id.to_string())?;
    state
        .core
        .review(ReviewCall::Undo {
            operation_id,
            request,
        })
        .await
}

// review_file_actions
#[tauri::command]
pub async fn core_review_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
    version: PreviewVersion,
) -> Result<Preview, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(ReviewCall::Preview {
            task_id,
            snapshot_id,
            root_id,
            path,
            version,
        })
        .await
}

#[tauri::command]
pub async fn core_review_reveal(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    let target = state
        .core
        .review(ReviewCall::RevealPath {
            task_id,
            snapshot_id,
            root_id,
            path,
        })
        .await?;
    reveal_file(target).await
}

#[tauri::command]
pub async fn core_review_ignore(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    operation_id: String,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(ReviewCall::Ignore {
            operation_id,
            task_id,
            snapshot_id,
            root_id,
            path,
        })
        .await
}

// review/batch_file_actions
#[tauri::command]
pub async fn core_review_batch_files(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    operation_id: String,
    request: BatchFileRequest,
) -> Result<FileBatchResult, String> {
    require_task_window(window.label(), &request.task_id)?;
    state
        .core
        .review(ReviewCall::BatchFiles {
            operation_id,
            request,
        })
        .await
}

// review/git_workflow
#[tauri::command]
pub async fn core_review_git_advance(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<GitReceipt, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(ReviewCall::GitAdvance {
            task_id,
            operation_id,
        })
        .await
}

#[tauri::command]
pub async fn core_review_git_close(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<GitReceipt, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .review(ReviewCall::GitClose {
            task_id,
            operation_id,
        })
        .await
}

// review/operation_history
#[tauri::command]
pub async fn core_review_operations(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: HistoryQuery,
) -> Result<HistoryPage, String> {
    require_task_window(window.label(), &request.task_id)?;
    state.core.review(ReviewCall::Operations(request)).await
}

// review/workspaces
#[tauri::command]
pub async fn core_review_workspaces(
    window: tauri::Window,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<Vec<ReviewWorkspace>, String> {
    require_task_window(window.label(), &task_id)?;
    let mut items: Vec<ReviewWorkspace> = state
        .core
        .review(ReviewCall::Workspaces { task_id })
        .await?;
    // Detached windows retain their original task permission boundary.
    if window.label() != "main" {
        items.retain(|item| item.current);
    }
    Ok(items)
}

// review/watch
#[tauri::command]
pub async fn core_review_watch(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    task_id: String,
    id: String,
    changes: tauri::ipc::Channel<WatchMessage>,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    uuid::Uuid::parse_str(&id).map_err(|_| "无效的状态订阅标识")?;
    let core = state.core.clone();
    let (ready, accepted) = tokio::sync::oneshot::channel();
    watchers.start(
        (window.label().into(), format!("review:{id}")),
        async move {
            let mut stream = match core.review_watch(task_id).await {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if ready.send(Ok(())).is_err() {
                return;
            }
            loop {
                let message = stream
                    .next()
                    .await
                    .unwrap_or_else(|message| WatchMessage::Error {
                        code: "disconnected".into(),
                        message,
                    });
                let ended = matches!(message, WatchMessage::Error { .. });
                if changes.send(message).is_err() || ended {
                    return;
                }
            }
        },
    )?;
    accepted.await.map_err(|_| "状态订阅已取消")?
}

#[tauri::command]
pub fn core_review_unwatch(
    window: tauri::WebviewWindow,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    id: String,
) -> Result<(), String> {
    watchers.stop((window.label().into(), format!("review:{id}")))
}

/// Resolve and confine every file-open request on the host, including app discovery.
async fn workspace_file(
    state: &DesktopState,
    task_id: &str,
    path: &str,
) -> Result<PathBuf, String> {
    state
        .core
        .review(service::ReviewCall::WorkspacePath {
            task_id: task_id.into(),
            path: path.into(),
            directory: false,
        })
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileApplications {
    absolute_path: String,
    applications: Vec<open_with::Application>,
    error: Option<String>,
}

impl WorkspaceFileApplications {
    fn from_result(path: PathBuf, result: Result<Vec<open_with::Application>, String>) -> Self {
        let (applications, error) = match result {
            Ok(applications) => (applications, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        Self {
            absolute_path: path.to_string_lossy().into_owned(),
            applications,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_options_keep_the_resolved_path_when_application_discovery_fails() {
        let path = PathBuf::from("/workspace/history/report.html");
        for result in [Ok(Vec::new()), Err("应用查询暂时失败".into())] {
            let failed = result.is_err();
            let options = WorkspaceFileApplications::from_result(path.clone(), result);
            let json = serde_json::to_value(options).unwrap();
            assert_eq!(json["absolutePath"], "/workspace/history/report.html");
            assert!(json["applications"].is_array());
            assert_eq!(json["error"].is_string(), failed);
        }
    }
}
