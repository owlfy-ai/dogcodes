//! Shared review execution. No window or GUI state owns these operations.
use super::{
    Arc, ArtifactId, BranchSummary, CommitSummary, CompareMode, Deserialize, DiscardRequest,
    GitReview, IndexAction, LeaseRequest, PathBuf, ReviewFile, ReviewHost, ReviewQuery, ReviewRoot,
    ReviewSnapshot, ReviewStore, Serialize, TaskId, TextComparison, batch_file_actions,
    commit_changes_tagged, git_operations, git_workflow, image_preview, mutation_operations,
    operation_history, project_analysis, publish_snapshot, query_roots, read_text,
    recorded_changes, remote_actions, require_git_success, run_commit_command, select_query_roots,
    validate_commit_preview, workspaces,
};
use agent_core::Core;
use serde_json::Value;

pub(crate) struct ReviewContext<'a> {
    pub core: &'a Arc<Core>,
    pub review: &'a ReviewHost,
}

pub(crate) async fn roots(core: &Arc<Core>, task: &str) -> Result<Vec<ReviewRoot>, String> {
    let paths = crate::workspace_service::project_paths(core, task).await?;
    let paths: Vec<_> = paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    GitReview::default().discover(&paths).await
}

pub(super) async fn query(
    state: &ReviewContext<'_>,
    query: ReviewQuery,
) -> Result<ReviewSnapshot, String> {
    let _guard = state.review.gate.lock().await;
    let all = roots(state.core, &query.task_id).await?;
    let selected = select_query_roots(all, &query)?;
    let store = ReviewStore::open(state.review.directory.clone())?;
    let recorded = recorded_changes::load(state.core, &query.task_id, &selected).await?;
    let output = query_roots(
        &selected,
        &query,
        &state.review.directory,
        Arc::new(recorded),
    )
    .await?;
    let snapshot = ReviewSnapshot {
        id: ArtifactId::new().to_string(),
        task_id: query.task_id,
        roots: output,
    };
    // Background detection must never evict the frozen snapshot being read.
    if query.probe {
        return Ok(snapshot);
    }
    let mut owner = state
        .review
        .owner
        .lock()
        .map_err(|_| "snapshot owner unavailable")?;
    if owner.is_none() {
        *owner = Some(store.create_snapshot_owner(&ArtifactId::new().to_string())?);
    }
    store.persist_task_snapshot_references(
        owner.as_ref().ok_or("snapshot owner unavailable")?,
        &snapshot.task_id,
        &snapshot.id,
        snapshot
            .roots
            .iter()
            .flat_map(|root| &root.files)
            .flat_map(|change| change.before.iter().chain(&change.after))
            .map(|version| version.hash.clone()),
    )?;
    drop(owner);
    let mut cache = state
        .review
        .snapshots
        .lock()
        .map_err(|_| "Review cache unavailable")?;
    let mut leases = state
        .review
        .leases
        .lock()
        .map_err(|_| "Review lease unavailable")?;
    match publish_snapshot(
        &mut cache,
        &mut leases,
        snapshot.clone(),
        std::time::Instant::now(),
    ) {
        Ok(Some(evicted)) => {
            let _ = store.release_snapshot_references(&evicted);
        }
        Ok(None) => {}
        Err(error) => {
            let _ = store.release_snapshot_references(&snapshot.id);
            return Err(error);
        }
    }
    Ok(snapshot)
}

pub(super) async fn snapshot_lease(
    state: &ReviewContext<'_>,
    request: LeaseRequest,
) -> Result<(), String> {
    roots(state.core, &request.task_id).await?;
    let cache = state
        .review
        .snapshots
        .lock()
        .map_err(|_| "Review cache unavailable")?;
    if request.release && !cache.contains_key(&request.snapshot_id) {
        return Ok(());
    }
    cache
        .get(&request.snapshot_id)
        .filter(|snapshot| snapshot.task_id == request.task_id)
        .ok_or("比较结果已过期，请刷新")?;
    state
        .review
        .leases
        .lock()
        .map_err(|_| "Review lease unavailable")?
        .update(&request, std::time::Instant::now())
}

pub(super) async fn file(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    path: String,
) -> Result<ReviewFile, String> {
    let _gate = state.review.gate.lock().await;
    let current_root = roots(state.core, &task_id)
        .await?
        .into_iter()
        .find(|root| root.id == root_id)
        .ok_or("项目目录已改变，请刷新")?;
    let store = ReviewStore::open(state.review.directory.clone())?;
    let change = state
        .review
        .file_change(&task_id, &snapshot_id, &root_id, &path)?;
    let plan = {
        let cache = state
            .review
            .snapshots
            .lock()
            .map_err(|_| "Review cache unavailable")?;
        let root = cache
            .get(&snapshot_id)
            .filter(|s| s.task_id == task_id)
            .and_then(|s| s.roots.iter().find(|r| r.root.id == root_id))
            .ok_or("比较结果已过期，请刷新")?;
        if root.root.path != current_root.path || root.root.scopes != current_root.scopes {
            return Err("项目目录范围已改变，请刷新".into());
        }
        if let Some(error) = root.statistics.get(&path).and_then(|s| s.error.as_ref()) {
            return Err(error.clone());
        }
        root.text_plans.get(&path).cloned()
    };
    tokio::task::spawn_blocking(move || {
        let binary = change.before.iter().chain(&change.after).any(|v| v.binary);
        if binary || path.to_ascii_lowercase().ends_with(".svg") {
            let before_image = image_preview(&store, change.before.as_ref(), &path)?;
            let after_image = image_preview(&store, change.after.as_ref(), &path)?;
            if !binary && before_image.is_none() && after_image.is_none() {
                return Err("图片超过 20 MB，暂不支持预览".into());
            }
            return Ok(ReviewFile {
                text: None,
                binary,
                before_image,
                after_image,
            });
        }
        let (before, after) = read_text(&store, &change)?;
        let plan = plan.ok_or("冻结的文本统计不可用，请刷新")?;
        let text = TextComparison {
            before: before.split_inclusive('\n').map(str::to_owned).collect(),
            after: after.split_inclusive('\n').map(str::to_owned).collect(),
            blocks: plan.blocks,
            additions: plan.additions,
            deletions: plan.deletions,
        };
        Ok(ReviewFile {
            text: Some(text),
            binary,
            before_image: None,
            after_image: None,
        })
    })
    .await
    .map_err(|_| "Review file task failed")?
}

pub(super) async fn history(
    state: &ReviewContext<'_>,
    task_id: String,
    root_id: String,
    skip: usize,
) -> Result<Vec<CommitSummary>, String> {
    let root = roots(state.core, &task_id)
        .await?
        .into_iter()
        .find(|r| r.id == root_id && r.git)
        .ok_or("Git workspace unavailable")?;
    GitReview::default().commits(&root, skip.min(100_000)).await
}

pub(super) async fn branches(
    state: &ReviewContext<'_>,
    task_id: String,
    root_id: String,
) -> Result<Vec<BranchSummary>, String> {
    let root = roots(state.core, &task_id)
        .await?
        .into_iter()
        .find(|r| r.id == root_id && r.git)
        .ok_or("Git workspace unavailable")?;
    GitReview::default().branches(&root).await
}

pub(super) async fn index(
    state: &ReviewContext<'_>,
    task_id: String,
    snapshot_id: String,
    root_id: String,
    paths: Vec<String>,
    action: IndexAction,
) -> Result<(), String> {
    let _workspace_access = state
        .core
        .try_workspace_mutation()
        .map_err(|e| e.to_string())?;
    let expected = {
        let cache = state
            .review
            .snapshots
            .lock()
            .map_err(|_| "Review cache unavailable")?;
        cache
            .get(&snapshot_id)
            .filter(|s| s.task_id == task_id)
            .and_then(|s| s.roots.iter().find(|r| r.root.id == root_id))
            .and_then(|r| r.git_comparison.clone())
            .ok_or("比较已过期或不是 Git 目录，请刷新")?
    };
    let current_root = roots(state.core, &task_id)
        .await?
        .into_iter()
        .find(|r| r.id == root_id)
        .ok_or("项目目录已改变")?;
    if current_root.path != expected.root.path || current_root.scopes != expected.root.scopes {
        return Err("项目目录范围已改变，请刷新".into());
    }
    let metadata = state.core.bootstrap().await.map_err(|_| "任务状态不可用")?;
    if metadata
        .tasks
        .iter()
        .any(|t| t.task_id.to_string() == task_id && t.active_primary_run.is_some())
    {
        return Err("Agent 正在运行，请等待结束后操作索引".into());
    }
    let action = match action {
        IndexAction::Stage => agent_workspace::review::git::operations::IndexAction::Stage,
        IndexAction::Unstage => agent_workspace::review::git::operations::IndexAction::Unstage,
    };
    let store = ReviewStore::open(state.review.directory.clone())?;
    GitReview::default()
        .change_index_many(&task_id, &expected, &paths, action, &store)
        .await?;
    // A successful mutation consumes the preview. All following writes must
    // use a fresh query; an old snapshot remains readable only until evicted.
    let mut cache = state
        .review
        .snapshots
        .lock()
        .map_err(|_| "Review cache unavailable")?;
    for snapshot in cache.values_mut() {
        for root in &mut snapshot.roots {
            if root.root.id == root_id {
                root.git_comparison = None;
            }
        }
    }
    Ok(())
}

pub(super) async fn discard(
    state: &ReviewContext<'_>,
    request: DiscardRequest,
) -> Result<String, String> {
    if !request.confirmed {
        return Err("丢弃修改需要明确确认".into());
    }
    let _access = state
        .core
        .try_workspace_mutation()
        .map_err(|e| e.to_string())?;
    let result = {
        let cache = state
            .review
            .snapshots
            .lock()
            .map_err(|_| "Review cache unavailable")?;
        cache
            .get(&request.snapshot_id)
            .filter(|s| s.task_id == request.task_id)
            .and_then(|s| s.roots.iter().find(|r| r.root.id == request.root_id))
            .cloned()
            .ok_or("比较已过期，请刷新")?
    };
    if !matches!(
        result.mode,
        CompareMode::Uncommitted | CompareMode::Unstaged | CompareMode::Session
    ) {
        return Err("历史或暂存区视图不能丢弃工作文件".into());
    }
    let current = roots(state.core, &request.task_id)
        .await?
        .into_iter()
        .find(|r| r.id == request.root_id)
        .ok_or("项目目录已改变")?;
    if current.path != result.root.path || current.scopes != result.root.scopes {
        return Err("项目范围已改变，请刷新".into());
    }
    let mut change = result
        .files
        .iter()
        .find(|f| f.path == request.path)
        .cloned()
        .ok_or("文件不在此比较中")?;
    for path in std::iter::once(&change.path).chain(change.previous_path.iter()) {
        if !current.scopes.iter().any(|scope| {
            scope.is_empty() || path == scope || path.starts_with(&format!("{scope}/"))
        }) {
            return Err("恢复路径不在当前项目范围内".into());
        }
    }
    let service = GitReview::default();
    let store = ReviewStore::open(state.review.directory.clone())?;
    if change
        .before
        .iter()
        .chain(change.after.iter())
        .any(|version| version.kind == agent_workspace::review::FileKind::Gitlink)
    {
        return Err("子模块仅支持查看及暂存提交指针，不支持目录恢复".into());
    }
    if current.git {
        service
            .check_working_preview(
                result
                    .git_comparison
                    .as_ref()
                    .ok_or("写入预览已失效，请刷新")?,
                &store,
            )
            .await?;
        change.before = service
            .working_replacement(
                result
                    .git_comparison
                    .as_ref()
                    .ok_or("写入预览已失效，请刷新")?,
                &change,
                &store,
            )
            .await?;
    }
    let directory = state.review.directory.clone();
    let prepare_task = request.task_id.clone();
    let prepare_root = current.path.clone();
    let grouped = change.previous_path.is_some();
    let record_id: String = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let store = ReviewStore::open(directory)?;
        if grouped {
            let group = store.prepare_rename_recovery(
                &prepare_task,
                std::path::Path::new(&prepare_root),
                &change,
            )?;
            return Ok(group.id);
        }
        let record = store.prepare_recovery(
            &prepare_task,
            std::path::Path::new(&prepare_root),
            &change.path,
            change.after,
            change.before,
        )?;
        Ok(record.id)
    })
    .await
    .map_err(|_| "文件备份准备失败")??;
    let _index = if current.git {
        let guard = service
            .lock_file_recovery(
                std::path::Path::new(&current.path),
                agent_workspace::review::git::operations::FileRecoveryLockRequest {
                    task: &request.task_id,
                    record: &record_id,
                    grouped,
                },
                &store,
            )
            .await?;
        service
            .check_working_preview(
                result
                    .git_comparison
                    .as_ref()
                    .ok_or("写入预览已失效，请刷新")?,
                &store,
            )
            .await?;
        Some(guard)
    } else {
        None
    };
    let directory = state.review.directory.clone();
    tokio::task::spawn_blocking(move || {
        let store = ReviewStore::open(directory)?;
        let outcome = if grouped {
            store.apply_recovery_group(&request.task_id, &record_id)
        } else {
            store.apply_recovery(&request.task_id, &record_id)
        };
        outcome?;
        Ok(record_id)
    })
    .await
    .map_err(|_| "文件操作未完成，请刷新后检查文件状态")?
}

// Mirrors the frontend call's separate fields one to one.
#[allow(clippy::too_many_arguments)]
pub(super) async fn git_action(
    state: &ReviewContext<'_>,
    task_id: String,
    root_id: String,
    action: String,
    message: String,
    stage_all: Option<bool>,
    snapshot_id: Option<String>,
    reflog_action: Option<String>,
) -> Result<bool, String> {
    let root = roots(state.core, &task_id)
        .await?
        .into_iter()
        .find(|r| r.id == root_id && r.git)
        .ok_or("当前目录未受 Git 管理")?;
    match action.as_str() {
        "commit" if !message.trim().is_empty() => {
            let _access = state
                .core
                .try_workspace_mutation()
                .map_err(|e| e.to_string())?;
            let expected = state
                .review
                .snapshots
                .lock()
                .map_err(|_| "Review cache unavailable")?
                .get(snapshot_id.as_deref().unwrap_or_default())
                .filter(|snapshot| snapshot.task_id == task_id)
                .and_then(|snapshot| snapshot.roots.iter().find(|item| item.root.id == root_id))
                .and_then(|item| item.git_comparison.clone())
                .ok_or("REVIEW_STALE_COMMIT: 比较已过期，请刷新后重新确认提交")?;
            validate_commit_preview(
                &root,
                &expected,
                &ReviewStore::open(state.review.directory.clone())?,
            )
            .await?;
            commit_changes_tagged(
                std::path::Path::new(&root.path),
                &message,
                stage_all.unwrap_or(false),
                reflog_action.as_deref(),
            )
            .await
        }
        "pull" | "sync" => {
            let _access = state
                .core
                .try_workspace_mutation()
                .map_err(|e| e.to_string())?;
            if state
                .core
                .bootstrap()
                .await
                .map_err(|e| e.to_string())?
                .tasks
                .iter()
                .any(|task| {
                    task.task_id.to_string() == task_id && task.active_primary_run.is_some()
                })
            {
                return Err("Agent 正在运行，请等待结束后操作".into());
            }
            let outcome =
                remote_actions::pull_or_sync(std::path::Path::new(&root.path), action == "sync")
                    .await;
            // Pull may modify files/index even when a later merge or push fails.
            for snapshot in state
                .review
                .snapshots
                .lock()
                .map_err(|_| "Review cache unavailable")?
                .values_mut()
            {
                for entry in &mut snapshot.roots {
                    if entry.root.id == root_id {
                        entry.git_comparison = None;
                    }
                }
            }
            outcome?;
            Ok(true)
        }
        "push" => {
            require_git_success(
                run_commit_command(
                    std::path::Path::new(&root.path),
                    &["-c", "push.default=upstream", "push"],
                )
                .await?,
            )?;
            Ok(true)
        }
        _ => Err("无效的 Git 操作或空提交说明".into()),
    }
}

pub(super) async fn commit_summary(
    state: &ReviewContext<'_>,
    task_id: String,
    root_id: String,
    executor: agent_domain::ExecutorRef,
    language: Option<String>,
) -> Result<String, String> {
    let changes = {
        let _gate = state.review.gate.lock().await;
        let root = roots(state.core, &task_id)
            .await?
            .into_iter()
            .find(|r| r.id == root_id && r.git)
            .ok_or("当前目录未受 Git 管理")?;
        let path = std::path::Path::new(&root.path);
        let staged =
            run_commit_command(path, &["diff", "--cached", "--quiet", "--exit-code"]).await?;
        let cached = match staged.status.code() {
            Some(0) => false,
            Some(1) => true,
            _ => {
                require_git_success(staged)?;
                false
            }
        };
        let args = if cached {
            vec![
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--stat",
                "--patch",
                "--",
                ".",
            ]
        } else {
            vec![
                "diff",
                "HEAD",
                "--no-ext-diff",
                "--no-textconv",
                "--stat",
                "--patch",
                "--",
                ".",
            ]
        };
        let mut output = run_commit_command(path, &args).await?;
        // An unborn repository has no HEAD; compare the empty index instead.
        if !output.status.success() && !cached {
            output = run_commit_command(
                path,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--stat",
                    "--patch",
                    "--",
                    ".",
                ],
            )
            .await?;
        }
        if !output.status.success() {
            require_git_success(output)?;
            return Err("无法读取差异".into());
        }
        let mut changes = String::from_utf8_lossy(&output.stdout).into_owned();
        if !cached {
            let untracked = run_commit_command(
                path,
                &[
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    ".",
                ],
            )
            .await?;
            if !untracked.status.success() {
                require_git_success(untracked)?;
                return Err("无法读取新增文件".into());
            }
            for name in untracked
                .stdout
                .split(|b| *b == 0)
                .filter(|b| !b.is_empty())
            {
                if changes.len() > 100_000 {
                    break;
                }
                let name = String::from_utf8_lossy(name);
                changes.push_str("\n新增文件：");
                changes.push_str(&serde_json::to_string(&name).unwrap_or_default());
                changes.push('\n');
                let file = path.join(name.as_ref());
                if let Ok(meta) = std::fs::symlink_metadata(&file)
                    && meta.is_file()
                    && meta.len() <= 32_000
                    && let Ok(text) = std::fs::read_to_string(&file)
                    && !text.contains('\0')
                {
                    changes.push_str(&text);
                }
            }
        }
        if changes.trim().is_empty() {
            return Err("没有可总结的更改".into());
        }
        if changes.len() > 100_000 {
            let mut end = 100_000;
            while !changes.is_char_boundary(end) {
                end -= 1;
            }
            changes.truncate(end);
            changes.push_str("\n[更改内容已截断]");
        }
        changes
    };
    state
        .core
        .summarize_commit(executor, changes, language.as_deref())
        .await
}

/// Private authenticated local RPC contract. Window checks remain in adapters;
/// task, artifact and current workspace scope are always checked by the service.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "arguments",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ReviewCall {
    Query(ReviewQuery),
    Operations(operation_history::HistoryQuery),
    Lease(LeaseRequest),
    File {
        task_id: String,
        snapshot_id: String,
        root_id: String,
        path: String,
    },
    History {
        task_id: String,
        root_id: String,
        skip: usize,
    },
    Branches {
        task_id: String,
        root_id: String,
    },
    Index {
        operation_id: String,
        task_id: String,
        snapshot_id: String,
        root_id: String,
        paths: Vec<String>,
        action: IndexAction,
    },
    Discard {
        operation_id: String,
        request: DiscardRequest,
    },
    GitAction(git_operations::GitRequest),
    GitClose {
        task_id: String,
        operation_id: String,
    },
    GitAdvance {
        task_id: String,
        operation_id: String,
    },
    GitOperation {
        task_id: String,
        operation_id: String,
    },
    CommitSummary {
        task_id: String,
        root_id: String,
        executor: agent_domain::ExecutorRef,
        language: Option<String>,
    },
    BatchFiles {
        operation_id: String,
        request: batch_file_actions::BatchFileRequest,
    },
    FileOperation {
        task_id: String,
        operation_id: String,
    },
    FileClose {
        task_id: String,
        operation_id: String,
    },
    Preview {
        task_id: String,
        snapshot_id: String,
        root_id: String,
        path: String,
        version: crate::review_file_actions::PreviewVersion,
    },
    Ignore {
        operation_id: String,
        task_id: String,
        snapshot_id: String,
        root_id: String,
        path: String,
    },
    RevealPath {
        task_id: String,
        snapshot_id: String,
        root_id: String,
        path: String,
    },
    UndoStatus {
        task_id: TaskId,
        artifact_id: ArtifactId,
    },
    Undo {
        operation_id: String,
        request: crate::turn_undo::UndoRequest,
    },
    Workspaces {
        task_id: String,
    },
    WorkspacePath {
        task_id: String,
        path: String,
    },
    StartProjectReview(agent_core::turn_review::ReviewRequest),
}

pub(crate) async fn dispatch(
    core: &Arc<Core>,
    review: &ReviewHost,
    request: ReviewCall,
) -> Result<Value, String> {
    use ReviewCall::{
        BatchFiles, Branches, CommitSummary, Discard, File, FileClose, FileOperation, GitAction,
        GitAdvance, GitClose, GitOperation, History, Ignore, Index, Lease, Operations, Preview,
        Query, RevealPath, StartProjectReview, Undo, UndoStatus, WorkspacePath, Workspaces,
    };
    let state = &ReviewContext { core, review };
    macro_rules! response {
        ($value:expr) => {
            serde_json::to_value($value).map_err(|e| e.to_string())
        };
    }
    match request {
        Query(request) => response!(query(state, request).await?),
        Operations(request) => response!(operation_history::query(state, request).await?),
        Lease(request) => response!(snapshot_lease(state, request).await?),
        File {
            task_id,
            snapshot_id,
            root_id,
            path,
        } => response!(file(state, task_id, snapshot_id, root_id, path).await?),
        History {
            task_id,
            root_id,
            skip,
        } => response!(history(state, task_id, root_id, skip).await?),
        Branches { task_id, root_id } => response!(branches(state, task_id, root_id).await?),
        Index {
            operation_id,
            task_id,
            snapshot_id,
            root_id,
            paths,
            action,
        } => {
            let arguments = serde_json::json!({"taskId":task_id,"snapshotId":snapshot_id,"rootId":root_id,"paths":paths,"action":action});
            mutate(
                state,
                operation_id,
                task_id.clone(),
                "core_review_index",
                arguments,
                index(state, task_id, snapshot_id, root_id, paths, action),
            )
            .await
        }
        Discard {
            operation_id,
            request,
        } => {
            let task = request.task_id.clone();
            let arguments = serde_json::json!({"request":request});
            mutate(
                state,
                operation_id,
                task,
                "core_review_discard",
                arguments,
                discard(state, request),
            )
            .await
        }
        FileOperation {
            task_id,
            operation_id,
        } => {
            require_task(core, &task_id).await?;
            response!(review.mutations.get(&task_id, &operation_id)?)
        }
        FileClose {
            task_id,
            operation_id,
        } => {
            let _gate = review.gate.lock().await;
            require_task(core, &task_id).await?;
            let receipt = review
                .mutations
                .get(&task_id, &operation_id)?
                .ok_or("原文件操作不存在，请先核实")?;
            if matches!(
                receipt.outcome,
                mutation_operations::MutationOutcome::Running
            ) {
                return Err("文件操作仍在执行，请等待结束后再结束跟踪".into());
            }
            response!(review.mutations.close(&task_id, &operation_id)?)
        }
        GitAction(request) => {
            let _gate = review.gate.lock().await;
            response!(git_workflow::execute_locked(state, request).await?)
        }
        GitAdvance {
            task_id,
            operation_id,
        } => {
            response!(git_workflow::advance(state, task_id, operation_id).await?)
        }
        GitClose {
            task_id,
            operation_id,
        } => response!(git_workflow::close(state, task_id, operation_id).await?),
        GitOperation {
            task_id,
            operation_id,
        } => {
            require_task(core, &task_id).await?;
            response!(
                review
                    .git_operations
                    .get(&task_id, &operation_id)?
                    .map(|receipt| git_workflow::with_closure(&review.git_operations, receipt))
                    .transpose()?
            )
        }
        CommitSummary {
            task_id,
            root_id,
            executor,
            language,
        } => response!(commit_summary(state, task_id, root_id, executor, language).await?),
        BatchFiles {
            operation_id,
            request,
        } => {
            let task = request.task_id.clone();
            let arguments = serde_json::json!({"request":request});
            mutate(
                state,
                operation_id,
                task,
                "core_review_batch_files",
                arguments,
                batch_file_actions::execute(state, request),
            )
            .await
        }
        Preview {
            task_id,
            snapshot_id,
            root_id,
            path,
            version,
        } => response!(
            crate::review_file_actions::preview(
                state,
                task_id,
                snapshot_id,
                root_id,
                path,
                version
            )
            .await?
        ),
        Ignore {
            operation_id,
            task_id,
            snapshot_id,
            root_id,
            path,
        } => {
            let arguments = serde_json::json!({"taskId":task_id,"snapshotId":snapshot_id,"rootId":root_id,"path":path});
            mutate(
                state,
                operation_id,
                task_id.clone(),
                "core_review_ignore",
                arguments,
                crate::review_file_actions::ignore(state, task_id, snapshot_id, root_id, path),
            )
            .await
        }
        RevealPath {
            task_id,
            snapshot_id,
            root_id,
            path,
        } => response!(
            crate::review_file_actions::reveal_path(state, task_id, snapshot_id, root_id, path)
                .await?
        ),
        UndoStatus {
            task_id,
            artifact_id,
        } => response!(crate::turn_undo::status(state, task_id, artifact_id).await?),
        Undo {
            operation_id,
            request,
        } => {
            let task = request.task_id.to_string();
            let arguments = serde_json::json!({"request":request});
            mutate(
                state,
                operation_id,
                task,
                "core_turn_undo",
                arguments,
                crate::turn_undo::undo(state, request),
            )
            .await
        }
        Workspaces { task_id } => response!(workspaces::list(core, &task_id).await?),
        WorkspacePath { task_id, path } => response!(workspace_path(core, &task_id, &path).await?),
        StartProjectReview(request) => {
            if request.turn_id.is_some() || request.source_snapshot.is_none() {
                return Err("项目审查来源无效".into());
            }
            let existing = core
                .turn_reviews(&request.task_id)
                .await
                .map_err(|e| e.to_string())?;
            // Core verifies the original immutable request/packet when replaying.
            let input = if request.retry_of.is_some()
                || existing
                    .iter()
                    .any(|run| run.request.run_id == request.run_id)
            {
                None
            } else {
                Some(project_analysis::project_review_packet(state, &request).await?)
            };
            response!(
                core.start_project_review(request, input)
                    .await
                    .map_err(|e| e.to_string())?
            )
        }
    }
}

async fn workspace_path(core: &Arc<Core>, task: &str, path: &str) -> Result<PathBuf, String> {
    let all = roots(core, task).await?;
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        for root in all {
            let Ok(base) = std::fs::canonicalize(&root.path) else {
                continue;
            };
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                base.join(&path)
            };
            let Ok(candidate) = candidate.canonicalize() else {
                continue;
            };
            let Ok(relative) = candidate.strip_prefix(&base) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if candidate.is_file()
                && root.scopes.iter().any(|scope| {
                    scope.is_empty()
                        || relative == *scope
                        || relative.starts_with(&format!("{scope}/"))
                })
            {
                return Ok(candidate);
            }
        }
        Err("文件不存在或不属于当前工作区".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(super) async fn require_task(core: &Core, task: &str) -> Result<(), String> {
    let task = TaskId::parse(task).map_err(|_| "无效的会话标识")?;
    if core
        .bootstrap()
        .await
        .map_err(|e| e.to_string())?
        .tasks
        .iter()
        .any(|entry| entry.task_id == task)
    {
        Ok(())
    } else {
        Err("会话已不存在".into())
    }
}

// Called only with typed, decoded arguments; never dispatch a stored arbitrary command.
async fn mutate<T: Serialize>(
    state: &ReviewContext<'_>,
    operation_id: String,
    task_id: String,
    command: &str,
    arguments: Value,
    action: impl std::future::Future<Output = Result<T, String>>,
) -> Result<Value, String> {
    let _gate = state.review.gate.lock().await;
    require_task(state.core, &task_id).await?;
    let request = mutation_operations::MutationRequest {
        operation_id,
        task_id,
        command: command.into(),
        arguments,
    };
    let result = state
        .review
        .mutations
        .execute(request, || async {
            serde_json::to_value(action.await?).map_err(|e| e.to_string())
        })
        .await;
    state.review.changes.changed();
    result
}
