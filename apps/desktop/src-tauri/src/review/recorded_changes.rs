//! Non-Git comparisons reuse undo records; no session baseline or migration.
use super::ReviewRoot;
use agent_domain::TaskId;
use agent_protocol::ActorRef;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(super) type RecordedChanges = BTreeMap<String, BTreeMap<String, Value>>;

pub(super) async fn load(
    core: &agent_core::Core,
    task_id: &str,
    roots: &[ReviewRoot],
) -> Result<RecordedChanges, String> {
    if roots.iter().all(|root| root.git) {
        return Ok(BTreeMap::new());
    }
    let task = TaskId::parse(task_id).map_err(|_| "invalid task")?;
    let mut cursor = None;
    let mut result = BTreeMap::new();
    let mut remaining = 32 * 1024 * 1024u64;
    loop {
        let page = core
            .task_artifacts(&ActorRef::LocalUser, &task, cursor.as_ref(), 100)
            .await
            .map_err(|e| e.to_string())?;
        // Artifact pages are newest first. Older versions overwrite newer ones
        // so each file starts at its first recorded modification in this task.
        for entry in page.items {
            let metadata = entry.metadata;
            if !matches!(
                metadata.media_type.as_str(),
                "application/vnd.dogcodes.turn-changes.v1+json"
                    | "application/vnd.dogcodes.turn-changes.v2+json"
            ) {
                continue;
            }
            remaining = remaining
                .checked_sub(metadata.byte_length)
                .ok_or("修改记录过多，请从具体轮次查看差异")?;
            let artifact = core
                .verified_local_artifact(
                    &ActorRef::LocalUser,
                    &task,
                    &metadata.artifact_id,
                    &metadata.payload_sha256,
                )
                .await
                .map_err(|e| e.to_string())?;
            let data = serde_json::from_slice(&artifact.content).map_err(|_| "修改记录无法读取")?;
            collect(&mut result, &data, roots)?;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(result)
}

fn collect(result: &mut RecordedChanges, data: &Value, roots: &[ReviewRoot]) -> Result<(), String> {
    let files = data["files"].as_array().ok_or("修改记录缺少文件列表")?;
    for file in files {
        let (source, relative) = if data["schemaVersion"] == 2 {
            (
                file["workspacePath"].as_str(),
                file["relativePath"].as_str(),
            )
        } else {
            (
                data["workspacePaths"]
                    .as_array()
                    .filter(|paths| paths.len() == 1)
                    .and_then(|paths| paths[0].as_str()),
                file["path"].as_str(),
            )
        };
        let source = source.ok_or("修改记录缺少工作目录")?;
        let Some(root) = roots.iter().find(|root| {
            !root.git
                && (root.path == source
                    || Path::new(source).canonicalize().ok().as_deref()
                        == Some(Path::new(&root.path)))
        }) else {
            continue;
        };
        let path = relative.ok_or("修改记录缺少相对路径")?;
        agent_workspace::review::validate_relative(path)?;
        if !root.scopes.iter().any(|scope| {
            scope.is_empty() || path == scope || path.starts_with(&format!("{scope}/"))
        }) {
            continue;
        }
        let before = file.get("before").ok_or("修改记录缺少原始内容")?;
        result
            .entry(root.id.clone())
            .or_default()
            .insert(path.into(), before.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fmt::Write;
    #[tokio::test]
    async fn query_loads_verified_turn_records_without_a_session_baseline() {
        use agent_core::{AppendRequest, Core, CoreRepository, NewArtifact};
        use agent_domain::{ArtifactId, CommandId, ExecutorRef, ProjectId};
        use agent_protocol::{
            ArtifactMetadata, CommandEnvelope, CoreCommand, CreateTask, NewEvent, RegisterProject,
            StreamRef, StreamType,
        };
        use agent_store::SqliteStore;
        use agent_workspace::review::ReviewStore;
        use std::sync::Arc;
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let blobs = ReviewStore::open(storage.path().into()).unwrap();
        let repository = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(repository.clone()));
        let project = ProjectId::new();
        let task = TaskId::new();
        let path = workspace.path().to_str().unwrap().to_owned();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project.clone(),
                display_name: "test".into(),
                path: path.clone(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project,
                title: "test".into(),
                preferred_executor: ExecutorRef {
                    provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        let client = core;
        let roots = vec![ReviewRoot {
            id: "root".into(),
            path: path.clone(),
            git: false,
            scopes: vec![String::new()],
        }];
        assert!(
            load(&client, task.as_str(), &roots)
                .await
                .unwrap()
                .is_empty()
        );
        for before in ["original", "later"] {
            let hex = before
                .as_bytes()
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    write!(hex, "{byte:02x}").unwrap();
                    hex
                });
            let data = json!({"schemaVersion":1,"workspacePaths":[path],"files":[{"path":"file","before":{
                "hex":hex,
                "sha256":blobs.put_blob(before.as_bytes()).unwrap(),"mode":"100644"
            }}]});
            let content = serde_json::to_vec(&data).unwrap();
            let metadata = ArtifactMetadata {
                artifact_id: ArtifactId::new(),
                payload_sha256: blobs.put_blob(&content).unwrap(),
                media_type: "application/vnd.dogcodes.turn-changes.v1+json".into(),
                byte_length: content.len() as u64,
                created_at: chrono::Utc::now(),
                redaction_ruleset_version: 0,
                redaction_count: 0,
            };
            repository.append(AppendRequest { request_fingerprint: None, command_id: CommandId::new(), idempotency_key: CommandId::new().to_string(),
                expected_stream_version: None, artifacts: vec![NewArtifact { metadata: metadata.clone(), content }],
                events: vec![NewEvent::hub(StreamRef { stream_type: StreamType::Task, stream_id: task.to_string() },
                    "workspace.turn_changes_captured", json!({"taskId":task,"turnId":agent_domain::TurnId::new(),"runId":agent_domain::RunId::new(),"artifact":metadata}))],
            }).await.unwrap();
        }
        std::fs::write(workspace.path().join("file"), "current").unwrap();
        let recorded = load(&client, task.as_str(), &roots).await.unwrap();
        let changes = blobs
            .recorded_file_changes(workspace.path(), &recorded["root"])
            .unwrap();
        assert_eq!(
            blobs
                .read_blob(&changes[0].before.as_ref().unwrap().hash)
                .unwrap(),
            b"original"
        );
        assert_eq!(
            blobs
                .read_blob(&changes[0].after.as_ref().unwrap().hash)
                .unwrap(),
            b"current"
        );
        assert_eq!(
            std::fs::read_dir(storage.path().join("baselines"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn first_recorded_version_wins_and_identical_names_stay_in_their_root() {
        let roots: Vec<_> = ["/one", "/two"]
            .iter()
            .map(|path| ReviewRoot {
                id: path.to_string(),
                path: path.to_string(),
                git: false,
                scopes: vec![String::new()],
            })
            .collect();
        let mut result = RecordedChanges::new();
        collect(
            &mut result,
            &json!({"schemaVersion":2,"files":[
                {"workspacePath":"/one","relativePath":"same","before":"later"},
                {"workspacePath":"/two","relativePath":"same","before":null},
                {"workspacePath":"/outside","relativePath":"same","before":"ignore"}
            ]}),
            &roots,
        )
        .unwrap();
        collect(
            &mut result,
            &json!({"schemaVersion":1,"workspacePaths":["/one"],"files":[
                {"path":"same","before":"original"}
            ]}),
            &roots,
        )
        .unwrap();
        assert_eq!(result["/one"]["same"], "original");
        assert!(result["/two"]["same"].is_null());
        assert_eq!(result.len(), 2);
        assert!(
            collect(
                &mut result,
                &json!({"schemaVersion":1,"workspacePaths":["/one"],"files":[
                    {"path":"../escape","before":null}
                ]}),
                &roots
            )
            .is_err()
        );
    }
}
