//! Resolve immutable turn artifacts on the host; the frontend never supplies file bytes/paths.
use crate::review::service::ReviewContext;
use agent_domain::{ArtifactId, TaskId};
use agent_workspace::review::ReviewStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoRequest {
    pub task_id: TaskId,
    artifact_id: ArtifactId,
    expected_payload_sha256: String,
    #[serde(default)]
    confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
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
    .map_err(|_| "backendText.undoStatusRead")?
}

async fn load(
    state: &ReviewContext<'_>,
    request: &UndoRequest,
) -> Result<(serde_json::Value, Vec<String>), String> {
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
        return Err("backendText.undoTurnOnly".into());
    }
    let data =
        serde_json::from_slice(&artifact.content).map_err(|_| "backendText.undoSnapshotRead")?;
    let snapshot = state.core.bootstrap().await?;
    let task = snapshot
        .tasks
        .iter()
        .find(|t| t.task_id == request.task_id)
        .ok_or("backendText.conversationMissing")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id == task.project_id)
        .ok_or("backendText.projectMissing")?;
    if project.target_id.is_some() && state.core.on_hub() {
        return Err("backendText.projectOnOtherMachine".into());
    }
    let roots = task.directories(project);
    Ok((data, roots))
}

mod patch;
mod proof;

struct CheckedFiles {
    data: Value,
    roots: Vec<String>,
    states: BTreeMap<String, String>,
}

fn select_files(data: &mut Value, paths: &[String]) -> Result<(), String> {
    let selected: BTreeSet<_> = paths.iter().map(String::as_str).collect();
    if paths.is_empty() || selected.len() != paths.len() {
        return Err("backendText.snapshotPathMissing".into());
    }
    let files = data["files"]
        .as_array_mut()
        .ok_or("backendText.snapshotMissing")?;
    files.retain(|f| f["path"].as_str().is_some_and(|p| selected.contains(p)));
    if files
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect::<BTreeSet<_>>()
        != selected
        || files.len() != selected.len()
    {
        return Err("backendText.snapshotPathMissing".into());
    }
    Ok(())
}

impl CheckedFiles {
    fn authorized(self) -> Result<(Value, Vec<String>), String> {
        for (path, reason) in self.states {
            if reason != "ready" {
                return Err(if reason == "changed" || reason == "restored" {
                    agent_domain::user_text("backendText.undoFileChanged", &[&path])
                } else {
                    "backendText.undoIncompleteTurn".into()
                });
            }
        }
        Ok((self.data, self.roots))
    }
}

/// The client selects names only. Resolve versions and edit evidence from the verified turn.
async fn prepare(state: &ReviewContext<'_>, request: &UndoRequest) -> Result<CheckedFiles, String> {
    let (mut data, roots) = load(state, request).await?;
    let paths = request.paths.as_ref().ok_or("backendText.undoNoChanges")?;
    select_files(&mut data, paths)?;
    let shared =
        data["sharedWorkspace"] == true || data["attribution"] == "shared_workspace_interval";
    let attribution = data["attribution"].as_str();
    let recognized = matches!(
        attribution,
        Some("workspace_interval" | "shared_workspace_interval" | "workspace_recovery_interval")
    );
    let needs_proof = shared
        || data["recoveredAfterInterruption"] == true
        || attribution == Some("workspace_recovery_interval");
    let edits = if needs_proof && recognized {
        state
            .core
            .turn_edits(&request.task_id, &request.artifact_id)
            .await?
    } else {
        Vec::new()
    };
    let primary = data["workspacePaths"][0].as_str().unwrap_or("").to_owned();
    let mut states = BTreeMap::new();
    for f in data["files"].as_array_mut().unwrap() {
        let path = f["path"]
            .as_str()
            .ok_or("backendText.snapshotPathMissing")?
            .to_owned();
        let reason = if !recognized {
            Some("recovery")
        } else if crate::workspace_contents::hydrate(state.core, &request.task_id, f)
            .await
            .is_err()
            || ["before", "after"]
                .iter()
                .any(|k| f.get(*k).is_none() || f[*k]["contentOmitted"] == true)
        {
            Some("incomplete")
        } else {
            None
        };
        if let Some(reason) = reason {
            states.insert(path, reason.to_string());
        }
    }
    tokio::task::spawn_blocking(move || {
        for f in data["files"].as_array().unwrap() {
            let path = f["path"].as_str().unwrap().to_owned();
            states.entry(path).or_insert_with(|| {
                let live = ReviewStore::turn_file_state(&data, f, &roots).unwrap_or("unavailable");
                if live == "ready" && needs_proof && !proof::explains(&primary, f, &edits) {
                    "unverified".into()
                } else {
                    live.to_owned()
                }
            });
        }
        Ok(CheckedFiles {
            data,
            roots,
            states,
        })
    })
    .await
    .map_err(|_| "backendText.undoStatusRead")?
}

pub async fn eligibility(
    state: &ReviewContext<'_>,
    request: UndoRequest,
) -> Result<BTreeMap<String, String>, String> {
    Ok(prepare(state, &request).await?.states)
}

pub async fn undo(
    state: &ReviewContext<'_>,
    request: UndoRequest,
    operation: String,
) -> Result<(), String> {
    if !request.confirmed {
        return Err("backendText.undoConfirmationRequired".into());
    }
    let (data, roots, id) = if request.paths.is_some() {
        let (data, roots) = prepare(state, &request).await?.authorized()?;
        (data, roots, format!("{}:{operation}", request.artifact_id))
    } else {
        let (mut data, roots) = load(state, &request).await?;
        crate::workspace_contents::hydrate(state.core, &request.task_id, &mut data).await?;
        (data, roots, request.artifact_id.to_string())
    };
    let access = state.core.try_turn_undo(&request.task_id, &roots).await?;
    let directory = state.review.directory.clone();
    tokio::task::spawn_blocking(move || {
        let _access = access;
        let store = ReviewStore::open(directory)?;
        if request.paths.is_some() {
            store.undo_verified_files(request.task_id.as_str(), &id, &data, &roots)
        } else {
            store.undo_turn(request.task_id.as_str(), &id, &data, &roots)
        }
    })
    .await
    .map_err(|_| "backendText.undoTaskFailed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    #[test]
    fn guarded_file_and_whole_undo_preserve_unselected_and_later_writes() {
        let work = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let root = work.path().canonicalize().unwrap();
        let roots = vec![root.to_str().unwrap().to_owned()];
        let store = ReviewStore::open(storage.path().to_owned()).unwrap();
        let version = |s: &str| json!({"mode":"100644","hex":hex::encode(s),"sha256":hex::encode(Sha256::digest(s.as_bytes()))});
        let files = ["a", "b", "other"]
            .map(|path| json!({"path":path,"before":version("old"),"after":version("new")}));
        let original = json!({"workspacePaths":roots,"attribution":"shared_workspace_interval","sharedWorkspace":true,"files":files});
        for path in ["a", "b", "other"] {
            std::fs::write(root.join(path), "new").unwrap();
        }
        let check = |paths: &[&str]| {
            let mut data = original.clone();
            select_files(
                &mut data,
                &paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            )
            .unwrap();
            let states = data["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    let path = f["path"].as_str().unwrap();
                    let edit =
                        json!({"input":{"path": path,"old_string":"old","new_string":"new"}});
                    assert!(proof::explains(&roots[0], f, &[edit]));
                    (
                        path.into(),
                        ReviewStore::turn_file_state(&data, f, &roots)
                            .unwrap()
                            .into(),
                    )
                })
                .collect();
            CheckedFiles {
                data,
                states,
                roots: roots.clone(),
            }
        };
        let (group, _) = check(&["a", "b"]).authorized().unwrap();
        std::fs::write(root.join("b"), "later").unwrap();
        assert!(
            store
                .undo_verified_files("task", "racing", &group, &roots)
                .is_err()
        );
        assert_eq!(std::fs::read(root.join("a")).unwrap(), b"new");
        let mixed = check(&["a", "b"]);
        assert_eq!(mixed.states["b"], "changed");
        assert!(mixed.authorized().is_err());
        let (single, _) = check(&["a"]).authorized().unwrap();
        store
            .undo_verified_files("task", "single", &single, &roots)
            .unwrap();
        assert_eq!(check(&["a"]).states["a"], "restored");
        assert_eq!(std::fs::read(root.join("b")).unwrap(), b"later");
        for path in ["a", "b"] {
            std::fs::write(root.join(path), "new").unwrap();
        }
        let (group, _) = check(&["a", "b"]).authorized().unwrap();
        store
            .undo_verified_files("task", "group", &group, &roots)
            .unwrap();
        for path in ["a", "b"] {
            assert_eq!(std::fs::read(root.join(path)).unwrap(), b"old");
        }
        assert_eq!(std::fs::read(root.join("other")).unwrap(), b"new");
    }
}
