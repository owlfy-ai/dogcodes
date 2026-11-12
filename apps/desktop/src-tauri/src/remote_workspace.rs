//! Sends workspace operations for a server's projects to that server. The same code
//! runs over there; only what it asks about tasks comes back here to be answered.
use crate::execution_targets::TargetHost;
use agent_core::Core;
use agent_domain::{ArtifactId, ExecutorRef, ProjectId, RunId, TargetId, TaskId};
use agent_protocol::{ArtifactCursor, ArtifactPage, BootstrapSnapshot};
use agent_runtime::SharedTarget;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use workspace_ops::{
    MutationGuard, ReviewFacts, VerifiedArtifact,
    operation::{self, Operation, RunningOperation},
    review::{
        ReviewHost,
        service::{self, ReviewCall},
    },
    workspace_service::DirectoryRequest,
};

/// What a server is told. It is not trusted with anything beyond its own projects:
/// it may only ask about the task it is working for, and may only start agents that
/// are installed on it, whatever an operation running over there asks for.
pub(crate) struct TargetFacts {
    core: Arc<Core>,
    target: TargetId,
    task: TaskId,
    /// Whoever can answer what Git on the server asks. Not every operation runs Git.
    prompts: Option<workspace_ops::review::git_credentials::GitPrompts>,
}

impl TargetFacts {
    fn require_task(&self, task: &TaskId) -> Result<(), String> {
        if task == &self.task {
            Ok(())
        } else {
            Err("这项操作不能访问其他会话".into())
        }
    }

    async fn require_executor(&self, executor: &ExecutorRef) -> Result<(), String> {
        let snapshot = ReviewFacts::bootstrap(&self.core).await?;
        let here = snapshot.installations.iter().any(|item| {
            item.target_id.as_ref() == Some(&self.target)
                && executor.installation_id.as_deref() == Some(item.installation_id.as_str())
        });
        if here {
            Ok(())
        } else {
            Err("所选 Agent 不在这个项目所在的机器上".into())
        }
    }
}

#[async_trait]
impl ReviewFacts for TargetFacts {
    fn on_hub(&self) -> bool {
        false
    }
    async fn bootstrap(&self) -> Result<BootstrapSnapshot, String> {
        let mut snapshot = ReviewFacts::bootstrap(&self.core).await?;
        snapshot
            .projects
            .retain(|project| project.target_id.as_ref() == Some(&self.target));
        let projects = snapshot.projects.clone();
        let on_this_server =
            |project: &ProjectId| projects.iter().any(|p| p.project_id == *project);
        snapshot
            .tasks
            .retain(|task| on_this_server(&task.project_id));
        snapshot
            .deleted_tasks
            .retain(|task| on_this_server(&task.project_id));
        let tasks = &snapshot.tasks;
        snapshot
            .session_maps
            .retain(|map| tasks.iter().any(|task| task.task_id == map.task_id));
        snapshot
            .companion
            .workspaces
            .retain(|workspace| on_this_server(&workspace.project_id));
        // Neither says which task it belongs to, so neither can be narrowed.
        snapshot.companion.recent_completion = None;
        snapshot.active_runs.clear();
        snapshot.installations.clear();
        Ok(snapshot)
    }
    async fn try_workspace_mutation(&self) -> Result<MutationGuard, String> {
        ReviewFacts::try_workspace_mutation(&self.core).await
    }
    async fn task_artifacts(
        &self,
        task: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, String> {
        self.require_task(task)?;
        ReviewFacts::task_artifacts(&self.core, task, before, limit).await
    }
    async fn verified_artifact(
        &self,
        task: &TaskId,
        artifact: &ArtifactId,
        sha256: &str,
    ) -> Result<VerifiedArtifact, String> {
        self.require_task(task)?;
        ReviewFacts::verified_artifact(&self.core, task, artifact, sha256).await
    }
    async fn summarize_commit(
        &self,
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    ) -> Result<String, String> {
        self.require_executor(&executor).await?;
        ReviewFacts::summarize_commit(&self.core, executor, changes, language).await
    }
    async fn review_run_exists(&self, task: &TaskId, run: &RunId) -> Result<bool, String> {
        self.require_task(task)?;
        ReviewFacts::review_run_exists(&self.core, task, run).await
    }
    async fn start_project_review(
        &self,
        request: Value,
        input: Option<Value>,
    ) -> Result<Value, String> {
        let task = request["taskId"]
            .as_str()
            .and_then(|id| TaskId::parse(id).ok());
        self.require_task(task.as_ref().ok_or("无效的会话标识")?)?;
        let executor: ExecutorRef =
            serde_json::from_value(request["executor"].clone()).map_err(|e| e.to_string())?;
        self.require_executor(&executor).await?;
        ReviewFacts::start_project_review(&self.core, request, input).await
    }
    async fn ask_git_credential(
        &self,
        task: &TaskId,
        prompt: &str,
    ) -> Result<Option<String>, String> {
        self.require_task(task)?;
        let prompts = self.prompts.as_ref().ok_or("这项操作不能询问凭据")?;
        Ok(prompts.ask(task.as_str(), prompt).await)
    }
}

/// The machine holding this task's project, if it is not this one.
async fn task_target(
    core: &Arc<Core>,
    targets: &TargetHost,
    task: &str,
) -> Result<Option<(SharedTarget, TargetFacts)>, String> {
    let task = TaskId::parse(task).map_err(|_| "无效的会话标识")?;
    let snapshot = ReviewFacts::bootstrap(core).await?;
    let target = snapshot
        .tasks
        .iter()
        .find(|entry| entry.task_id == task)
        .and_then(|entry| {
            snapshot
                .projects
                .iter()
                .find(|project| project.project_id == entry.project_id)
        })
        .and_then(|project| project.target_id.clone());
    let Some(target) = target else {
        return Ok(None);
    };
    Ok(Some((
        targets.target(&target)?,
        TargetFacts {
            core: core.clone(),
            target,
            task,
            prompts: None,
        },
    )))
}

pub(crate) async fn review(
    core: &Arc<Core>,
    targets: &TargetHost,
    host: &ReviewHost,
    request: ReviewCall,
) -> Result<Value, String> {
    if let Some(task) = request.task_id()
        && let Some((target, mut facts)) = task_target(core, targets, &task).await?
    {
        facts.prompts = Some(host.prompts.clone());
        // These hand back a path for this machine to open.
        if matches!(
            request,
            ReviewCall::RevealPath { .. } | ReviewCall::WorkspacePath { .. }
        ) {
            return Err("backendText.fileOnServer".into());
        }
        let result = operation::call(&target, &facts, &Operation::Review(request)).await;
        // Open views refresh from the same signal as a local change.
        host.changes.changed();
        return result;
    }
    service::dispatch(core, host, request).await
}

pub(crate) async fn workspace(
    core: &Arc<Core>,
    targets: &TargetHost,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let task = match method {
        "workspace_roots" => args.as_str().map(str::to_owned),
        "workspace_find_files" => args[0].as_str().map(str::to_owned),
        _ => args["taskId"].as_str().map(str::to_owned),
    };
    if let Some(task) = task
        && let Some((target, facts)) = task_target(core, targets, &task).await?
    {
        let operation = Operation::Workspace {
            method: method.to_owned(),
            args,
        };
        return operation::call(&target, &facts, &operation).await;
    }
    crate::workspace_service::dispatch(core, method, args).await
}

/// A directory watch running on the machine holding the directory.
pub(crate) struct RemoteWatch {
    running: RunningOperation,
    facts: TargetFacts,
}

impl RemoteWatch {
    pub(crate) async fn next(
        &mut self,
    ) -> Result<crate::workspace_service::watch::WatchMessage, String> {
        let event = self.running.next_event(&self.facts).await?;
        serde_json::from_value(event).map_err(|e| e.to_string())
    }
}

pub(crate) async fn watch(
    core: &Arc<Core>,
    targets: &TargetHost,
    request: &DirectoryRequest,
) -> Result<Option<RemoteWatch>, String> {
    let Some((target, facts)) = task_target(core, targets, &request.task_id).await? else {
        return Ok(None);
    };
    let operation = Operation::WatchDirectory(request.clone());
    let mut running = RunningOperation::start(&target, &operation).await?;
    // The first event says the watch is in place, or the operation ends with why not.
    running.next_event(&facts).await?;
    Ok(Some(RemoteWatch { running, facts }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::ProviderId;
    use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject};
    use agent_store::SqliteStore;
    use serde_json::json;

    async fn task(core: &Arc<Core>, target: Option<TargetId>, path: &str) -> TaskId {
        let project = ProjectId::new();
        let task = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: target,
                project_id: project.clone(),
                display_name: path.into(),
                path: path.into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project,
                title: "Conversation".into(),
                preferred_executor: ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        task
    }

    #[tokio::test]
    async fn a_server_learns_and_reaches_nothing_beyond_its_own_work() {
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let target = TargetId::new();
        let own = task(&core, Some(target.clone()), "/srv/own").await;
        let same_server = task(&core, Some(target.clone()), "/srv/other").await;
        let elsewhere = task(&core, Some(TargetId::new()), "/srv/elsewhere").await;
        let here = task(&core, None, "/Users/me/secret").await;
        let facts = TargetFacts {
            core: core.clone(),
            target,
            task: own.clone(),
            prompts: None,
        };
        let snapshot = facts.bootstrap().await.unwrap();
        let tasks: Vec<_> = snapshot.tasks.iter().map(|t| t.task_id.clone()).collect();
        assert!(tasks.contains(&own) && tasks.contains(&same_server));
        assert!(!tasks.contains(&elsewhere) && !tasks.contains(&here));
        assert!(snapshot.projects.iter().all(|p| p.path.starts_with("/srv")));
        assert!(snapshot.projects.iter().all(|p| p.path != "/srv/elsewhere"));
        assert!(snapshot.installations.is_empty());
        assert!(snapshot.active_runs.is_empty());
        assert!(snapshot.companion.recent_completion.is_none());
        let projects: Vec<_> = snapshot.projects.iter().map(|p| &p.project_id).collect();
        let unfiltered = ReviewFacts::bootstrap(&core).await.unwrap();
        assert!(
            unfiltered
                .companion
                .workspaces
                .iter()
                .any(|workspace| workspace.task_id == here),
            "the companion lists this machine's work before filtering"
        );
        assert!(
            snapshot
                .companion
                .workspaces
                .iter()
                .all(|workspace| projects.contains(&&workspace.project_id))
        );
        assert!(
            snapshot
                .session_maps
                .iter()
                .all(|map| tasks.contains(&map.task_id))
        );
        for other in [&same_server, &elsewhere, &here] {
            assert!(facts.task_artifacts(other, None, 10).await.is_err());
            assert!(
                facts
                    .verified_artifact(other, &ArtifactId::new(), "00")
                    .await
                    .is_err()
            );
            assert!(facts.review_run_exists(other, &RunId::new()).await.is_err());
            let request = json!({"taskId": other, "executor": {"providerId": "fake"}});
            assert!(facts.start_project_review(request, None).await.is_err());
        }
        // An agent on this machine cannot be fed text by a server.
        let local = ExecutorRef {
            provider_id: ProviderId::parse("fake").unwrap(),
            installation_id: None,
        };
        assert!(
            facts
                .summarize_commit(local, "text from the server".into(), None)
                .await
                .is_err()
        );
    }
}
