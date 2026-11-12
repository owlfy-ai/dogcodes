use crate::{
    review::service::{ReviewContext, roots},
    workspace_service::{Preview, preview_bytes},
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
        .ok_or("backendText.mb3a29250")?;
    for file_path in std::iter::once(path).chain(change.previous_path.as_deref()) {
        validate_relative(file_path)?;
        if !root.scopes.iter().any(|scope| {
            scope.is_empty() || file_path == scope || file_path.starts_with(&format!("{scope}/"))
        }) {
            return Err("backendText.m24e5ef97".into());
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
        .map_err(|_| "backendText.m534a8de1")?;
    if !target.starts_with(base) {
        return Err("backendText.m7e1feb0a".into());
    }
    Ok(target)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewVersion {
    Working,
    Head,
}

pub async fn preview(
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
                return Err("backendText.m9abd3f91".into());
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
                return Err("backendText.m41c79dda".into());
            }
            let git = GitReview::default();
            let head_path = change.previous_path.as_deref().unwrap_or(&path);
            let bytes = git
                .head_file(&root, head_path)
                .await
                .map_err(|_| "backendText.m95ebf63e")?;
            preview_bytes(bytes, head_path)
        }
    }
}

pub async fn reveal_path(
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

pub async fn reveal_file(target: PathBuf) -> Result<(), String> {
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
        command.arg(target.parent().ok_or("backendText.m7a22c7f5")?);
        command
    };
    command.kill_on_drop(true);
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), command.status())
        .await
        .map_err(|_| "backendText.m151d7b6a")?
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("backendText.m9350695d".into())
    }
}

pub async fn ignore(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<(), String> {
    let _access = state.core.try_workspace_mutation().await?;
    let (root, _) = target(state, &task_id, &snapshot_id, &root_id, &path).await?;
    if !root.git {
        return Err("backendText.m41c79dda".into());
    }
    let snapshot = state.core.bootstrap().await?;
    if snapshot
        .tasks
        .iter()
        .any(|task| task.task_id.to_string() == task_id && task.active_primary_run.is_some())
    {
        return Err("backendText.maceaefab".into());
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
