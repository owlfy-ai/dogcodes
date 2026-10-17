//! Worktree navigation resolves only persisted projects and task IDs.
use super::service::ReviewCall;
use crate::{DesktopState, review_window::require_task_window};
use agent_core::Core;
use agent_domain::TaskId;
use agent_workspace::review::git::{GitReview, worktrees::WorktreeStatus};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
// A serialized view model: each flag is an independent fact shown in the UI.
#[allow(clippy::struct_excessive_bools)]
pub struct ReviewWorkspace {
    task_id: String,
    name: String,
    conversation_title: String,
    path: String,
    branch: Option<String>,
    head: Option<String>,
    changed_files: Option<usize>,
    current: bool,
    primary: bool,
    git: bool,
    running: bool,
    error: Option<String>,
}

pub async fn list(core: &Core, task_id: &str) -> Result<Vec<ReviewWorkspace>, String> {
    let id = TaskId::parse(task_id).map_err(|_| "会话标识无效")?;
    let snapshot = core.bootstrap().await.map_err(|_| "无法读取工作区列表")?;
    let source = snapshot
        .tasks
        .iter()
        .find(|t| t.task_id == id)
        .ok_or("会话已不存在")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id == source.project_id)
        .ok_or("项目已不存在")?;
    let git = GitReview::default();
    let identity = git.worktree_identity(Path::new(&project.path)).await?;
    // Current project comes first so its task and selected subdirectory win deduplication.
    let mut projects = vec![project];
    projects.extend(
        snapshot
            .projects
            .iter()
            .filter(|p| p.project_id != project.project_id),
    );
    let mut seen = BTreeMap::<String, usize>::new();
    let mut result = Vec::<ReviewWorkspace>::new();
    for candidate in projects {
        let tasks: Vec<_> = snapshot
            .tasks
            .iter()
            .filter(|t| t.project_id == candidate.project_id)
            .collect();
        let Some(task) = tasks
            .iter()
            .find(|t| t.task_id == id)
            .copied()
            .or_else(|| tasks.iter().min_by_key(|t| t.task_id.to_string()).copied())
        else {
            continue;
        };
        let current = candidate.project_id == project.project_id;
        let candidate_identity = if current {
            identity.clone()
        } else {
            if identity.is_none() {
                continue;
            }
            match git.worktree_identity(Path::new(&candidate.path)).await {
                Ok(Some(value)) => Some(value),
                _ => continue,
            }
        };
        if !current
            && candidate_identity.as_ref().map(|i| &i.common)
                != identity.as_ref().map(|i| &i.common)
        {
            continue;
        }
        let key = candidate_identity.as_ref().map_or_else(
            || candidate.path.clone(),
            |i| i.root.to_string_lossy().into_owned(),
        );
        let running = tasks.iter().any(|t| t.active_primary_run.is_some());
        if let Some(index) = seen.get(&key) {
            result[*index].running |= running;
            continue;
        }
        seen.insert(key, result.len());
        let is_git = candidate_identity.is_some();
        let (status, error) = if is_git {
            match git.worktree_status(Path::new(&candidate.path)).await {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };
        let changed_files = status.as_ref().map(|s| s.changed_files);
        let WorktreeStatus { branch, head, .. } = status.unwrap_or_default();
        result.push(ReviewWorkspace {
            task_id: task.task_id.to_string(),
            name: candidate.display_name.clone(),
            conversation_title: task.title.clone(),
            path: candidate.path.clone(),
            branch,
            head,
            changed_files,
            current,
            primary: candidate_identity.as_ref().is_some_and(|i| i.primary),
            git: is_git,
            running,
            error,
        });
    }
    Ok(result)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Core;
    use agent_domain::{ExecutorRef, ProjectId, ProviderId};
    use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject};
    use agent_store::SqliteStore;
    use std::sync::Arc;

    async fn register(core: &Arc<Core>, path: &Path, name: &str) -> TaskId {
        let project_id = ProjectId::new();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: name.into(),
                path: path.to_str().unwrap().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: format!("{name} conversation"),
                preferred_executor: ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        task_id
    }

    fn git(path: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn lists_only_related_persisted_workspaces_and_marks_the_actual_chat_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let fork = temp.path().join("fork");
        let unrelated = temp.path().join("unrelated");
        let plain = temp.path().join("plain");
        for path in [&main, &unrelated, &plain] {
            std::fs::create_dir(path).unwrap();
        }
        git(&main, &["init", "-b", "main"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "codex/fork",
                fork.to_str().unwrap(),
            ],
        );
        git(&unrelated, &["init", "-b", "main"]);
        std::fs::write(fork.join("fork-only"), "change").unwrap();
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let source = register(&core, &main, "Main").await;
        let child = register(&core, &fork, "Fork").await;
        register(&core, &unrelated, "Other repository").await;
        let non_git = register(&core, &plain, "Plain").await;
        // Two projects can point at the same worktree through different scopes.
        let subdirectory = main.join("sub");
        std::fs::create_dir(&subdirectory).unwrap();
        register(&core, &subdirectory, "Subdirectory").await;
        let client = core;
        let items = list(&client, source.as_str()).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task_id, source.as_str());
        assert!(items[0].current && items[0].primary);
        assert_eq!(items[0].changed_files, Some(0));
        assert_eq!(items[1].task_id, child.as_str());
        assert!(!items[1].current && !items[1].primary);
        assert_eq!(items[1].changed_files, Some(1));
        assert_eq!(items[1].branch.as_deref(), Some("codex/fork"));
        let items = list(&client, child.as_str()).await.unwrap();
        assert!(items[0].current && !items[0].primary);
        assert_eq!(items[0].task_id, child.as_str());
        assert!(items[1].primary && !items[1].current);
        let items = list(&client, non_git.as_str()).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].current && !items[0].git);
        assert!(items[0].changed_files.is_none());
    }
}
