//! Commit-and-push links are durable request data, independent of any window.
use super::{
    git_operations::{GitCompletion, GitOperations, GitOutcome, GitReceipt, GitRequest},
    service::{ReviewCall, ReviewContext, require_task},
};
use crate::{DesktopState, review_window::require_task_window};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GitWorkflow {
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
            .ok_or("Git 上一步记录缺失，无法核实工作流")?;
        if !is_child(&parent, &current.request) {
            return Err("Git 工作流关联损坏，请核实原记录".into());
        }
        current = parent;
    }
    Err("Git 工作流关联过长或循环，请核实原记录".into())
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
        .ok_or("原操作记录不存在，请先核实")?;
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
        return Err("Git 接续标识不能复用当前或上一步标识".into());
    }
    match flow.parent_operation_id.as_deref() {
        None if request.action == "commit" => Ok(()),
        Some(id) => {
            let parent = state
                .review
                .git_operations
                .get(&request.task_id, id)?
                .ok_or("Git 上一步记录不存在，请先核实原操作")?;
            if workflow_root(&state.review.git_operations, &parent)?.closed {
                return Err("Git 工作流已结束，未执行推送".into());
            }
            if !may_advance(&parent) || !is_child(&parent, request) {
                return Err("Git 上一步未完成或接续请求不匹配，未执行推送".into());
            }
            Ok(())
        }
        _ => Err("只有提交并推送可以创建组合操作".into()),
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
async fn run_action(
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
                .ok_or("原 Git 目录不在当前项目范围内")?,
        )
    } else {
        None
    };
    if workflow && action.action == "push" {
        let parent_id = action
            .workflow
            .as_ref()
            .and_then(|flow| flow.parent_operation_id.as_deref())
            .ok_or("原组合操作不存在")?;
        let parent = state
            .review
            .git_operations
            .get(&action.task_id, parent_id)?
            .ok_or("原提交记录不存在")?;
        let origin = workflow_root(&state.review.git_operations, &parent)?;
        let target = match origin.outcome {
            GitOutcome::Succeeded {
                target: Some(target),
                ..
            } => target,
            GitOutcome::Succeeded { target_error, .. } => {
                return Err(format!(
                    "提交已完成，但未绑定推送目标：{}",
                    target_error.unwrap_or_else(|| "请核实仓库后使用独立推送入口".into())
                ));
            }
            _ => return Err("原提交结果尚未核实，未执行推送".into()),
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
        .ok_or("Git 原操作记录不存在，请先核实")?;
    let flow = parent
        .request
        .workflow
        .as_ref()
        .ok_or("原操作没有提交并推送意图，未执行推送")?;
    if !may_advance(&parent) {
        return Err("Git 原操作尚不允许接续，请先核实结果；未自动重发".into());
    }
    if workflow_root(&state.review.git_operations, &parent)?.closed {
        return Err("Git 工作流已结束，未执行推送".into());
    }
    // Return the original step even when it failed; a fresh retry requires
    // explicitly advancing that failed child's own reserved next identity.
    if let Some(child) = state
        .review
        .git_operations
        .get(&task, &flow.next_operation_id)?
    {
        if !is_child(&parent, &child.request) {
            return Err("Git 接续标识已被其他请求占用，未执行推送".into());
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
    Err("Git 接续结果尚未核实，请查询原操作记录".into())
}
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
