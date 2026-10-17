//! Local-user Artifact retrieval. No filesystem paths or unbounded payloads cross this port.

use agent_domain::{ArtifactId, TaskId};
use agent_protocol::{ActorRef, ArtifactChunk, ArtifactCursor, ArtifactPage, ArtifactReadRequest};
use sha2::{Digest, Sha256};

use crate::{Core, CoreError, StoredArtifact};

pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 32 * 1024;

impl Core {
    pub async fn task_artifacts(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, CoreError> {
        require_local_user(actor)?;
        if !(1..=100).contains(&limit) {
            return Err(CoreError::InvalidArgument(
                "artifact list limit must be 1..100".into(),
            ));
        }
        self.require_task(task_id).await?;
        let mut items = self
            .repository
            .task_artifacts(task_id, before, limit + 1)
            .await?;
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = if has_more {
            items.last().map(|item| ArtifactCursor {
                sequence: item.source_sequence,
                artifact_id: item.metadata.artifact_id.clone(),
            })
        } else {
            None
        };
        Ok(ArtifactPage { items, next_cursor })
    }

    pub async fn read_artifact(
        &self,
        actor: &ActorRef,
        request: &ArtifactReadRequest,
    ) -> Result<ArtifactChunk, CoreError> {
        if !(1..=MAX_CHUNK_BYTES).contains(&request.limit) {
            return Err(CoreError::InvalidArgument(
                "artifact chunk limit must be 1..32768 bytes".into(),
            ));
        }
        let artifact = self
            .verified_local_artifact(
                actor,
                &request.task_id,
                &request.artifact_id,
                &request.expected_payload_sha256,
            )
            .await?;
        let offset = usize::try_from(request.offset)
            .map_err(|_| CoreError::InvalidArgument("artifact offset is out of range".into()))?;
        if offset > artifact.content.len() {
            return Err(CoreError::InvalidArgument(
                "artifact offset is out of range".into(),
            ));
        }
        let end = offset
            .saturating_add(request.limit)
            .min(artifact.content.len());
        Ok(ArtifactChunk {
            metadata: artifact.metadata,
            offset: request.offset,
            bytes: artifact.content[offset..end].to_vec(),
            next_offset: (end < artifact.content.len()).then_some(end as u64),
        })
    }

    /// Shared by chunk reads and the desktop's native save-dialog adapter.
    /// This is not exposed as an unbounded UI command.
    pub async fn verified_local_artifact(
        &self,
        actor: &ActorRef,
        task_id: &TaskId,
        artifact_id: &ArtifactId,
        expected_sha256: &str,
    ) -> Result<StoredArtifact, CoreError> {
        require_local_user(actor)?;
        self.require_task(task_id).await?;
        let summary = self
            .repository
            .task_artifact(task_id, artifact_id)
            .await?
            .ok_or_else(|| CoreError::NotFound("artifact is not available in this task".into()))?;
        if summary.metadata.payload_sha256 != expected_sha256 {
            return Err(CoreError::Conflict(
                "artifact differs from the reviewed hash".into(),
            ));
        }
        if summary.metadata.byte_length > MAX_ARTIFACT_BYTES as u64 {
            return Err(CoreError::ResourceExhausted(
                "artifact exceeds the local read budget".into(),
            ));
        }
        if !matches!(
            summary.metadata.media_type.as_str(),
            "application/vnd.dogcodes.context-package.v1+json"
                | "application/vnd.dogcodes.test-report.v1+json"
                | "application/vnd.dogcodes.git-diff.v1.patch"
                | "application/vnd.dogcodes.turn-changes.v1+json"
                | "application/vnd.dogcodes.turn-changes.v2+json"
                | "application/vnd.dogcodes.operation-changes.v1+json"
                | "application/vnd.dogcodes.operation-changes.v2+json"
        ) {
            return Err(CoreError::InvalidArgument(
                "artifact media type is not supported for local retrieval".into(),
            ));
        }
        let artifact = self
            .repository
            .artifact(artifact_id)
            .await?
            .ok_or_else(|| CoreError::NotFound("artifact content is unavailable".into()))?;
        // Event metadata timestamps use full precision; the CAS stores epoch milliseconds.
        let metadata_matches = artifact.metadata.artifact_id == summary.metadata.artifact_id
            && artifact.metadata.payload_sha256 == summary.metadata.payload_sha256
            && artifact.metadata.media_type == summary.metadata.media_type
            && artifact.metadata.byte_length == summary.metadata.byte_length
            && artifact.metadata.created_at.timestamp_millis()
                == summary.metadata.created_at.timestamp_millis()
            && artifact.metadata.redaction_ruleset_version
                == summary.metadata.redaction_ruleset_version
            && artifact.metadata.redaction_count == summary.metadata.redaction_count;
        if !metadata_matches
            || artifact.content.len() > MAX_ARTIFACT_BYTES
            || artifact.content.len() as u64 != artifact.metadata.byte_length
            || hex::encode(Sha256::digest(&artifact.content)) != artifact.metadata.payload_sha256
        {
            return Err(CoreError::Internal(
                "artifact failed integrity verification".into(),
            ));
        }
        Ok(artifact)
    }
}

fn require_local_user(actor: &ActorRef) -> Result<(), CoreError> {
    if actor != &ActorRef::LocalUser {
        return Err(CoreError::InvalidArgument(
            "artifact retrieval requires the local user".into(),
        ));
    }
    Ok(())
}
