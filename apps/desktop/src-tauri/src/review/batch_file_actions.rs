use super::{
    CompareMode, DesktopState, FileChange, GitReview, ReviewStore, RootChanges,
    require_task_window,
    service::{ReviewCall, ReviewContext, roots},
};
use agent_workspace::review::{FileKind, recovery::batch::FileBatchResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchFileAction {
    Discard,
    Ignore,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFileRequest {
    pub(super) task_id: String,
    snapshot_id: String,
    root_id: String,
    paths: Vec<String>,
    action: BatchFileAction,
    #[serde(default)]
    confirmed: bool,
}

fn selected_changes(
    result: &RootChanges,
    paths: &[String],
    action: &BatchFileAction,
) -> Result<Vec<FileChange>, String> {
    if paths.is_empty() || paths.len() > 2000 {
        return Err("请选择 1 至 2000 个文件".into());
    }
    if result.error.is_some() {
        return Err("目录比较不可用，请刷新".into());
    }
    if !matches!(
        result.mode,
        CompareMode::Uncommitted | CompareMode::Unstaged | CompareMode::Session
    ) {
        return Err("当前比较范围不支持此操作".into());
    }
    let mut changes = Vec::new();
    for path in paths.iter().collect::<BTreeSet<_>>() {
        let change = result
            .files
            .iter()
            .find(|change| &change.path == path)
            .ok_or("文件不在此比较中")?;
        for path in std::iter::once(&change.path).chain(change.previous_path.iter()) {
            agent_workspace::review::validate_relative(path)?;
            if !result.root.scopes.iter().any(|scope| {
                scope.is_empty() || path == scope || path.starts_with(&format!("{scope}/"))
            }) {
                return Err("文件不在当前项目范围内".into());
            }
        }
        match action {
            BatchFileAction::Ignore
                if !result.root.git
                    || !result.untracked.contains(path)
                    || change.previous_path.is_some() =>
            {
                return Err("只能将未跟踪文件加入 .gitignore，请先移除已跟踪文件的选择".into());
            }
            BatchFileAction::Discard
                if change
                    .before
                    .iter()
                    .chain(change.after.iter())
                    .any(|version| version.kind == FileKind::Gitlink) =>
            {
                return Err("子模块不支持丢弃工作文件".into());
            }
            _ => {}
        }
        changes.push(change.clone());
    }
    Ok(changes)
}

async fn discard_selected(
    task: String,
    result: &RootChanges,
    mut changes: Vec<FileChange>,
    directory: PathBuf,
) -> Result<FileBatchResult, String> {
    let service = GitReview::default();
    let store = ReviewStore::open(directory.clone())?;
    let _index = if result.root.git {
        let comparison = result
            .git_comparison
            .as_ref()
            .ok_or("写入预览已失效，请刷新")?;
        service.check_working_preview(comparison, &store).await?;
        for change in &mut changes {
            change.before = service
                .working_replacement(comparison, change, &store)
                .await?;
        }
        let lock = service.lock_working(Path::new(&result.root.path)).await?;
        service.check_working_preview(comparison, &store).await?;
        Some(lock)
    } else {
        None
    };
    let root = result.root.path.clone();
    tokio::task::spawn_blocking(move || {
        ReviewStore::open(directory)?.discard_changes(&task, Path::new(&root), &changes)
    })
    .await
    .map_err(|_| "文件操作中断，请刷新检查；已准备的备份保留在恢复记录中")?
}

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

pub(super) async fn execute(
    state: &ReviewContext<'_>,
    request: BatchFileRequest,
) -> Result<FileBatchResult, String> {
    if matches!(request.action, BatchFileAction::Discard) && !request.confirmed {
        return Err("批量丢弃修改需要明确确认".into());
    }
    let _access = state
        .core
        .try_workspace_mutation()
        .map_err(|e| e.to_string())?;
    let result = state
        .review
        .snapshots
        .lock()
        .map_err(|_| "Review cache unavailable")?
        .get(&request.snapshot_id)
        .filter(|snapshot| snapshot.task_id == request.task_id)
        .and_then(|snapshot| {
            snapshot
                .roots
                .iter()
                .find(|root| root.root.id == request.root_id)
        })
        .cloned()
        .ok_or("比较已过期，请刷新")?;
    let current = roots(state.core, &request.task_id)
        .await?
        .into_iter()
        .find(|root| root.id == request.root_id)
        .ok_or("项目目录已改变")?;
    if current.path != result.root.path
        || current.scopes != result.root.scopes
        || current.git != result.root.git
    {
        return Err("项目目录范围已改变，请刷新".into());
    }
    if state
        .core
        .bootstrap()
        .await
        .map_err(|e| e.to_string())?
        .tasks
        .iter()
        .any(|task| {
            task.task_id.to_string() == request.task_id && task.active_primary_run.is_some()
        })
    {
        return Err("Agent 正在运行，请等待结束后操作".into());
    }
    let changes = selected_changes(&result, &request.paths, &request.action)?;
    let outcome = match request.action {
        BatchFileAction::Discard => {
            discard_selected(
                request.task_id,
                &result,
                changes,
                state.review.directory.clone(),
            )
            .await
        }
        BatchFileAction::Ignore => {
            GitReview::default()
                .ignore_files(
                    &current,
                    &changes
                        .iter()
                        .map(|change| change.path.clone())
                        .collect::<Vec<_>>(),
                )
                .await
        }
    };
    // The disk may have changed even if a later member failed; require a fresh preview.
    for snapshot in state
        .review
        .snapshots
        .lock()
        .map_err(|_| "Review cache unavailable")?
        .values_mut()
    {
        for root in &mut snapshot.roots {
            if root.root.id == request.root_id {
                root.git_comparison = None;
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_workspace::review::{ChangeKind, FileVersion, git::ReviewRoot};
    use std::collections::BTreeMap;

    fn fixture() -> RootChanges {
        RootChanges {
            root: ReviewRoot {
                id: "root".into(),
                path: "/repo".into(),
                git: true,
                scopes: vec!["src".into()],
            },
            mode: CompareMode::Uncommitted,
            base: String::new(),
            target: String::new(),
            files: ["src/a", "src/b"]
                .map(|path| FileChange {
                    path: path.into(),
                    previous_path: None,
                    kind: ChangeKind::Added,
                    before: None,
                    after: None,
                })
                .into(),
            untracked: vec!["src/a".into()],
            excluded_roots: vec![],
            error: None,
            statistics: BTreeMap::new(),
            text_plans: BTreeMap::new(),
            git_comparison: None,
        }
    }

    #[test]
    fn batch_selection_rejects_ineligible_and_out_of_scope_members_before_writing() {
        let mut result = fixture();
        let both = ["src/a".into(), "src/b".into()];
        assert!(selected_changes(&result, &both, &BatchFileAction::Ignore).is_err());
        assert_eq!(
            selected_changes(&result, &both, &BatchFileAction::Discard)
                .unwrap()
                .len(),
            2
        );
        assert!(
            selected_changes(&result, &["src/missing".into()], &BatchFileAction::Discard).is_err()
        );
        result.files[1].previous_path = Some("outside/b".into());
        assert!(selected_changes(&result, &both, &BatchFileAction::Discard).is_err());
        result.files[1].previous_path = None;
        result.mode = CompareMode::Staged;
        assert!(selected_changes(&result, &both, &BatchFileAction::Discard).is_err());
        result.mode = CompareMode::Session;
        result.root.git = false;
        assert_eq!(
            selected_changes(&result, &both, &BatchFileAction::Discard)
                .unwrap()
                .len(),
            2
        );
        assert!(selected_changes(&result, &["src/a".into()], &BatchFileAction::Ignore).is_err());
    }

    #[test]
    fn batch_selection_deduplicates_and_refuses_submodule_discard() {
        let mut result = fixture();
        assert_eq!(
            selected_changes(
                &result,
                &["src/a".into(), "src/a".into()],
                &BatchFileAction::Ignore
            )
            .unwrap()
            .len(),
            1
        );
        result.files[1].after = Some(FileVersion {
            hash: String::new(),
            bytes: 0,
            kind: FileKind::Gitlink,
            executable: false,
            binary: false,
        });
        assert!(
            selected_changes(
                &result,
                &["src/a".into(), "src/b".into()],
                &BatchFileAction::Discard
            )
            .is_err()
        );
    }
}
