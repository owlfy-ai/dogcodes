//! Resolve immutable turn artifacts on the host; the frontend never supplies file bytes/paths.
use crate::review::service::ReviewContext;
use agent_domain::{ArtifactId, TaskId};
use agent_workspace::review::ReviewStore;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoRequest {
    pub task_id: TaskId,
    artifact_id: ArtifactId,
    expected_payload_sha256: String,
    confirmed: bool,
}

pub async fn status(
    state: &ReviewContext<'_>,
    task_id: TaskId,
    artifact_id: ArtifactId,
) -> Result<bool, String> {
    crate::workspace_service::project_paths(state.core, task_id.as_str()).await?;
    let directory = state.review.directory.clone();
    tokio::task::spawn_blocking(move || {
        ReviewStore::open(directory)?.turn_is_undone(&task_id.to_string(), &artifact_id.to_string())
    })
    .await
    .map_err(|_| "撤销状态读取失败")?
}

pub async fn undo(state: &ReviewContext<'_>, request: UndoRequest) -> Result<(), String> {
    if !request.confirmed {
        return Err("撤销本轮修改需要明确确认".into());
    }
    let _access = state
        .core
        .try_workspace_mutation()
        .await
        .map_err(|_| "Agent 正在执行任务，请结束后再撤销")?;
    let artifact = state
        .core
        .verified_artifact(
            &request.task_id,
            &request.artifact_id,
            &request.expected_payload_sha256,
        )
        .await?;
    if !matches!(
        artifact.media_type.as_str(),
        "application/vnd.dogcodes.turn-changes.v1+json"
            | "application/vnd.dogcodes.turn-changes.v2+json"
    ) {
        return Err("只能撤销整轮文件修改记录".into());
    }
    let data = serde_json::from_slice(&artifact.content).map_err(|_| "本轮文件快照无法读取")?;
    let snapshot = state.core.bootstrap().await?;
    let task = snapshot
        .tasks
        .iter()
        .find(|t| t.task_id == request.task_id)
        .ok_or("会话不存在")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id == task.project_id)
        .ok_or("项目不存在")?;
    if project.target_id.is_some() && state.core.on_hub() {
        return Err("这个项目在另一台机器上".into());
    }
    let roots: Vec<String> = std::iter::once(project.path.clone())
        .chain(project.additional_directories.clone())
        .collect();
    let directory = state.review.directory.clone();
    let task_id = request.task_id.to_string();
    let artifact_id = request.artifact_id.to_string();

    tokio::task::spawn_blocking(move || {
        ReviewStore::open(directory)?.undo_turn(&task_id, &artifact_id, &data, &roots)
    })
    .await
    .map_err(|_| "撤销任务执行失败；请检查恢复记录")?
}
