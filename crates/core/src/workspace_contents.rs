//! Split retained bodies out of turn manifests. The artifact store deduplicates by digest.
use crate::{Core, CoreError, NewArtifact};
use agent_domain::{ArtifactId, TaskId};
use agent_protocol::{
    ArtifactMetadata,
    workspace_content::{
        MAX_WORKSPACE_FILE_BYTES, WORKSPACE_CONTENT_TYPE, hydrate_workspace_contents,
    },
};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) fn detach(value: &mut Value) -> Result<Vec<NewArtifact>, CoreError> {
    fn visit(
        value: &mut Value,
        artifacts: &mut BTreeMap<String, NewArtifact>,
    ) -> Result<(), CoreError> {
        match value {
            Value::Object(map) => {
                if map.contains_key("hex") && map.contains_key("sha256") {
                    let bytes = hex::decode(
                        map["hex"]
                            .as_str()
                            .ok_or_else(|| CoreError::Internal("invalid snapshot body".into()))?,
                    )
                    .map_err(|e| CoreError::Internal(e.to_string()))?;
                    // Git keeps its existing inline format and budgets for oversized versions.
                    if bytes.len() <= MAX_WORKSPACE_FILE_BYTES {
                        let digest = hex::encode(Sha256::digest(&bytes));
                        if map["sha256"] != digest {
                            return Err(CoreError::Internal(
                                "snapshot body checksum mismatch".into(),
                            ));
                        }
                        let artifact =
                            artifacts
                                .entry(digest.clone())
                                .or_insert_with(|| NewArtifact {
                                    metadata: ArtifactMetadata {
                                        artifact_id: ArtifactId::new(),
                                        payload_sha256: digest,
                                        media_type: WORKSPACE_CONTENT_TYPE.into(),
                                        byte_length: bytes.len() as u64,
                                        created_at: Utc::now(),
                                        redaction_ruleset_version: 0,
                                        redaction_count: 0,
                                    },
                                    content: bytes,
                                });
                        map.remove("hex");
                        map.insert("contentArtifact".into(), json!(artifact.metadata));
                    }
                } else {
                    for child in map.values_mut() {
                        visit(child, artifacts)?;
                    }
                }
            }
            Value::Array(values) => {
                for child in values {
                    visit(child, artifacts)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    let mut artifacts = BTreeMap::new();
    visit(value, &mut artifacts)?;
    Ok(artifacts.into_values().collect())
}

impl Core {
    pub(crate) async fn hydrate_workspace_contents(
        &self,
        task: &TaskId,
        value: &mut Value,
    ) -> Result<(), CoreError> {
        hydrate_workspace_contents(value, |meta| async move {
            self.verified_local_artifact(
                &agent_protocol::ActorRef::LocalUser,
                task,
                &meta.artifact_id,
                &meta.payload_sha256,
            )
            .await
            .map(|a| a.content)
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(CoreError::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bodies_are_deduplicated_and_hydrate_without_changing_snapshots() {
        let bytes = b"old\n";
        let entry = json!({"hex":hex::encode(bytes),"sha256":hex::encode(Sha256::digest(bytes))});
        let original = json!({"entries":{"a":entry,"b":entry}});
        let mut value = original.clone();
        let artifacts = detach(&mut value).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert!(value["entries"]["a"].get("hex").is_none());
        hydrate_workspace_contents(&mut value, |_| async { Ok(bytes.to_vec()) })
            .await
            .unwrap();
        assert_eq!(value, original);
        value["entries"]["a"]["hex"] = json!("00");
        assert!(detach(&mut value).is_err());
    }
}
