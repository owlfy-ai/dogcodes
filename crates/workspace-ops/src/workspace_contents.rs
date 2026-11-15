//! Resolve only verified task-owned immutable bodies, including on remote hosts.
use crate::ReviewFacts;
use agent_domain::TaskId;
use serde_json::Value;

pub(crate) async fn hydrate(
    core: &dyn ReviewFacts,
    task: &TaskId,
    data: &mut Value,
) -> Result<(), String> {
    agent_protocol::workspace_content::hydrate_workspace_contents(data, |metadata| async move {
        core.verified_artifact(task, &metadata.artifact_id, &metadata.payload_sha256)
            .await
            .map(|a| a.content)
    })
    .await
}
