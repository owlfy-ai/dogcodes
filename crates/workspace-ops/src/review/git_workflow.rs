//! Commit-and-push links are durable request data, independent of any window.
use super::{
    git_operations::{GitCompletion, GitOperations, GitOutcome, GitReceipt, GitRequest},
    service::{ReviewContext, require_task},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitWorkflow {
    pub next_operation_id: String,
    pub parent_operation_id: Option<String>,
}

fn may_advance(receipt: &GitReceipt) -> bool {
    matches!(
        (receipt.request.action.as_str(), &receipt.outcome),
        (
            "commit",
            GitOutcome::Succeeded {
                completed: true,
                ..
            }
        ) | ("push", GitOutcome::Failed { .. })
    )
}
fn is_child(parent: &GitReceipt, child: &GitRequest) -> bool {
    let Some(flow) = &parent.request.workflow else {
        return false;
    };
    child.operation_id == flow.next_operation_id
        && child.task_id == parent.request.task_id
        && child.root_id == parent.request.root_id
        && child.action == "push"
        && child.message.is_empty()
        && child.stage_all == Some(false)
        && child.snapshot_id.is_none()
        && child.workflow.as_ref().is_some_and(|flow| {
            flow.parent_operation_id.as_deref() == Some(parent.request.operation_id.as_str())
        })
}
// Root closure applies to all descendants. Bound traversal and reject damaged
// chains; a partial read cannot authorize another push.
fn workflow_root(journal: &GitOperations, receipt: &GitReceipt) -> Result<GitReceipt, String> {
    let mut current = receipt.clone();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..256 {
        if !seen.insert(current.request.operation_id.clone()) {
            break;
        }
        let Some(id) = current
            .request
            .workflow
            .as_ref()
            .and_then(|flow| flow.parent_operation_id.as_deref())
        else {
            return Ok(current);
        };
        let parent = journal
            .get(&current.request.task_id, id)?
            .ok_or("backendText.mfcd2b6bd")?;
        if !is_child(&parent, &current.request) {
            return Err("backendText.m1ccc5750".into());
        }
        current = parent;
    }
    Err("backendText.m370b4fe7".into())
}
pub(super) fn with_closure(
    journal: &GitOperations,
    mut receipt: GitReceipt,
) -> Result<GitReceipt, String> {
    receipt.closed |= workflow_root(journal, &receipt)?.closed;
    Ok(receipt)
}
pub(super) async fn close(
    state: &ReviewContext<'_>,
    task: String,
    operation: String,
) -> Result<GitReceipt, String> {
    let _gate = state.review.gate.lock().await;
    require_task(state.core, &task).await?;
    let mut receipt = state
        .review
        .git_operations
        .get(&task, &operation)?
        .ok_or("backendText.mfab7b848")?;
    let root = workflow_root(&state.review.git_operations, &receipt)?;
    state
        .review
        .git_operations
        .close(&task, &root.request.operation_id)?;
    state.review.changes.changed();
    receipt.closed = true;
    Ok(receipt)
}
pub(super) fn validate(state: &ReviewContext<'_>, request: &GitRequest) -> Result<(), String> {
    let Some(flow) = &request.workflow else {
        return Ok(());
    };
    state
        .review
        .git_operations
        .file(&request.task_id, &flow.next_operation_id)?;
    if flow.next_operation_id == request.operation_id
        || flow.parent_operation_id.as_ref() == Some(&flow.next_operation_id)
    {
        return Err("backendText.m7a506128".into());
    }
    match flow.parent_operation_id.as_deref() {
        None if request.action == "commit" => Ok(()),
        Some(id) => {
            let parent = state
                .review
                .git_operations
                .get(&request.task_id, id)?
                .ok_or("backendText.m1c2e6c64")?;
            if workflow_root(&state.review.git_operations, &parent)?.closed {
                return Err("backendText.m7b17395d".into());
            }
            if !may_advance(&parent) || !is_child(&parent, request) {
                return Err("backendText.m1eb0a42c".into());
            }
            Ok(())
        }
        _ => Err("backendText.m2324582b".into()),
    }
}
/// Caller holds the review gate across validation, execution and receipt publish.
pub(super) async fn execute_locked(
    state: &ReviewContext<'_>,
    request: GitRequest,
) -> Result<bool, String> {
    require_task(state.core, &request.task_id).await?;
    validate(state, &request)?;
    let action = request.clone();
    let result = state
        .review
        .git_operations
        .execute(request, || run_action(state, action))
        .await;
    state.review.changes.changed();
    result.map(|value| value.completed)
}
/// Runs the action while answering whatever Git asks along the way. If Git stops
/// waiting, dropping the question takes it off the screen.
async fn run_action(
    state: &ReviewContext<'_>,
    action: GitRequest,
) -> Result<GitCompletion, String> {
    use super::git_credentials::{ASKER, UNATTENDED_CREDENTIAL};
    let task = action.task_id.clone();
    let prompts = &state.review.prompts;
    let _ = prompts.take_unattended(&task);
    // Git's own error ("could not read Username") does not say what to do.
    let explain = |result: Result<GitCompletion, String>| {
        result.map_err(|error| {
            if prompts.take_unattended(&task) {
                UNATTENDED_CREDENTIAL.to_owned()
            } else {
                error
            }
        })
    };
    let (asker, mut questions) = tokio::sync::mpsc::unbounded_channel();
    let work = ASKER.scope(asker, perform_action(state, action));
    tokio::pin!(work);
    loop {
        tokio::select! {
            result = &mut work => return explain(result),
            Some((prompt, reply)) = questions.recv() => tokio::select! {
                result = &mut work => return explain(result),
                answer = ask_person(state, &task, &prompt) => {
                    let _ = reply.send(answer);
                }
            },
        }
    }
}
async fn ask_person(state: &ReviewContext<'_>, task: &str, prompt: &str) -> Option<String> {
    if state.core.on_hub() {
        return state.review.prompts.ask(task, prompt).await;
    }
    let id = agent_domain::TaskId::parse(task).ok()?;
    match state.core.ask_git_credential(&id, prompt).await {
        Ok(answer) => answer,
        Err(error) => {
            // The desktop had nobody to type it; say so when Git then fails.
            if error == super::git_credentials::UNATTENDED_CREDENTIAL {
                state.review.prompts.mark_unattended(task);
            }
            None
        }
    }
}
async fn perform_action(
    state: &ReviewContext<'_>,
    action: GitRequest,
) -> Result<GitCompletion, String> {
    let workflow = action.workflow.is_some();
    let root = if workflow {
        Some(
            super::service::roots(state.core, &action.task_id)
                .await?
                .into_iter()
                .find(|root| root.git && root.id == action.root_id)
                .ok_or("backendText.m0f12ec28")?,
        )
    } else {
        None
    };
    if workflow && action.action == "push" {
        let parent_id = action
            .workflow
            .as_ref()
            .and_then(|flow| flow.parent_operation_id.as_deref())
            .ok_or("backendText.m32e8ab7e")?;
        let parent = state
            .review
            .git_operations
            .get(&action.task_id, parent_id)?
            .ok_or("backendText.m4a34c6a1")?;
        let origin = workflow_root(&state.review.git_operations, &parent)?;
        let target = match origin.outcome {
            GitOutcome::Succeeded {
                target: Some(target),
                ..
            } => target,
            GitOutcome::Succeeded { target_error, .. } => {
                return Err(format!(
                    "backendText.m4c12cb65\u{1e}{}",
                    serde_json::to_string(&[target_error
                        .unwrap_or_else(|| "backendText.m3d9b6a65".into())
                        .clone()])
                    .unwrap_or_else(|_| "[]".into())
                ));
            }
            _ => return Err("backendText.ma4273db4".into()),
        };
        super::git_push_target::push(std::path::Path::new(&root.as_ref().unwrap().path), &target)
            .await?;
        return Ok(true.into());
    }
    let before = if let Some(root) = &root {
        Some(super::git_push_target::observe(std::path::Path::new(&root.path)).await)
    } else {
        None
    };
    let marker = workflow.then(|| super::git_push_target::marker(&action.operation_id));
    let completed = super::service::git_action(
        state,
        action.task_id,
        action.root_id,
        action.action,
        action.message,
        action.stage_all,
        action.snapshot_id,
        marker,
    )
    .await?;
    let mut result = GitCompletion::from(completed);
    if completed && let Some(before) = before {
        let bound = match before {
            Ok(before) => {
                super::git_push_target::bind(
                    std::path::Path::new(&root.unwrap().path),
                    before,
                    &action.operation_id,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match bound {
            Ok(target) => result.target = Some(target),
            Err(error) => result.target_error = Some(error),
        }
    }
    Ok(result)
}
pub(super) async fn advance(
    state: &ReviewContext<'_>,
    task: String,
    operation: String,
) -> Result<GitReceipt, String> {
    let _gate = state.review.gate.lock().await;
    require_task(state.core, &task).await?;
    let parent = state
        .review
        .git_operations
        .get(&task, &operation)?
        .ok_or("backendText.md9f985a2")?;
    let flow = parent
        .request
        .workflow
        .as_ref()
        .ok_or("backendText.mc2aa903c")?;
    if !may_advance(&parent) {
        return Err("backendText.ma0e018ba".into());
    }
    if workflow_root(&state.review.git_operations, &parent)?.closed {
        return Err("backendText.m7b17395d".into());
    }
    // Return the original step even when it failed; a fresh retry requires
    // explicitly advancing that failed child's own reserved next identity.
    if let Some(child) = state
        .review
        .git_operations
        .get(&task, &flow.next_operation_id)?
    {
        if !is_child(&parent, &child.request) {
            return Err("backendText.mba501389".into());
        }
        return Ok(child);
    }
    let request = GitRequest {
        operation_id: flow.next_operation_id.clone(),
        task_id: task.clone(),
        root_id: parent.request.root_id.clone(),
        action: "push".into(),
        message: String::new(),
        stage_all: Some(false),
        snapshot_id: None,
        workflow: Some(GitWorkflow {
            parent_operation_id: Some(operation),
            next_operation_id: uuid::Uuid::new_v4().to_string(),
        }),
    };
    let result = execute_locked(state, request).await;
    if let Some(receipt) = state
        .review
        .git_operations
        .get(&task, &flow.next_operation_id)?
    {
        return Ok(receipt);
    }
    result?;
    Err("backendText.m73b1835f".into())
}
