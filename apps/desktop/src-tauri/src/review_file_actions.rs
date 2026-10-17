use crate::{
    DesktopState,
    review::service::{ReviewCall, ReviewContext, roots},
    review_window::require_task_window,
    workspace_browser::{Preview, preview_bytes},
};
use agent_workspace::review::{
    FileChange,
    git::{GitReview, ReviewRoot},
    validate_relative,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

async fn target(
    state: &ReviewContext<'_>,
    task: &str,
    snapshot: &str,
    root_id: &str,
    path: &str,
) -> Result<(ReviewRoot, FileChange), String> {
    validate_relative(path)?;
    let change = state.review.file_change(task, snapshot, root_id, path)?;
    let root = roots(state.core, task)
        .await?
        .into_iter()
        .find(|root| root.id == root_id)
        .ok_or("项目目录已改变，请刷新")?;
    for file_path in std::iter::once(path).chain(change.previous_path.as_deref()) {
        validate_relative(file_path)?;
        if !root.scopes.iter().any(|scope| {
            scope.is_empty() || file_path == scope || file_path.starts_with(&format!("{scope}/"))
        }) {
            return Err("文件超出当前项目范围".into());
        }
    }
    Ok((root, change))
}

fn resolve_file(root: &str, path: &str) -> Result<PathBuf, String> {
    validate_relative(path)?;
    let base = Path::new(root).canonicalize().map_err(|e| e.to_string())?;
    let target = base
        .join(path)
        .canonicalize()
        .map_err(|_| "文件不存在或无法访问")?;
    if !target.starts_with(base) {
        return Err("文件位于项目目录之外".into());
    }
    Ok(target)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewVersion {
    Working,
    Head,
}

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

pub(crate) async fn preview(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
    version: PreviewVersion,
) -> Result<Preview, String> {
    let (root, change) = target(state, &task_id, &snapshot_id, &root_id, &path).await?;
    match version {
        PreviewVersion::Working => tokio::task::spawn_blocking(move || {
            let target = resolve_file(&root.path, &path)?;
            if !target.is_file() {
                return Err("不是普通文件，无法预览".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(target)
                .map_err(|e| e.to_string())?
                .take(20 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            preview_bytes(bytes, &path)
        })
        .await
        .map_err(|e| e.to_string())?,
        PreviewVersion::Head => {
            if !root.git {
                return Err("当前目录未受 Git 管理".into());
            }
            let git = GitReview::default();
            let head_path = change.previous_path.as_deref().unwrap_or(&path);
            let bytes = git
                .head_file(&root, head_path)
                .await
                .map_err(|_| "HEAD 中没有此文件，或文件无法读取")?;
            preview_bytes(bytes, head_path)
        }
    }
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

pub(crate) async fn reveal_path(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<PathBuf, String> {
    let (root, _) = target(state, &task_id, &snapshot_id, &root_id, &path).await?;
    tokio::task::spawn_blocking(move || resolve_file(&root.path, &path))
        .await
        .map_err(|e| e.to_string())?
}

pub(crate) async fn reveal_file(target: PathBuf) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = tokio::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(target);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("explorer.exe");
        command.arg(format!("/select,{}", target.display()));
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(target.parent().ok_or("没有父目录")?);
        command
    };
    command.kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), command.status())
        .await
        .map_err(|_| "打开文件管理器超时")?
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("无法在文件管理器中显示文件".into())
    }
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

pub(crate) async fn ignore(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<(), String> {
    let _access = state
        .core
        .try_workspace_mutation()
        .map_err(|e| e.to_string())?;
    let (root, _) = target(state, &task_id, &snapshot_id, &root_id, &path).await?;
    if !root.git {
        return Err("当前目录未受 Git 管理".into());
    }
    let snapshot = state.core.bootstrap().await.map_err(|e| e.to_string())?;
    if snapshot
        .tasks
        .iter()
        .any(|task| task.task_id.to_string() == task_id && task.active_primary_run.is_some())
    {
        return Err("Agent 正在运行，请等待结束后操作".into());
    }
    GitReview::default().ignore_file(&root, &path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolve_stays_inside_the_authorized_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "ok").unwrap();
        let root = dir.path().to_str().unwrap();
        assert!(resolve_file(root, "file").is_ok());
        assert!(resolve_file(root, "../outside").is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("file"), "private").unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
            assert!(resolve_file(root, "link/file").is_err());
        }
    }
}
