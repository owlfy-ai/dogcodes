//! Project registration and editing run in the service, so every client gets
//! the same canonical roots, duplicate reuse and ID allocation. The service
//! serializes these calls; a per-window or per-process gate cannot stop two
//! clients from registering the same directory twice.
use std::sync::Arc;

use agent_core::Core;
use agent_domain::ProjectId;
use agent_protocol::{CommandEnvelope, CoreCommand, RegisterProject};
use serde::{Deserialize, Serialize};

const MAX_ADDITIONAL_DIRECTORIES: usize = 31;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegistration {
    pub display_name: String,
    pub path: String,
    #[serde(default)]
    pub additional_directories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEdit {
    pub project_id: ProjectId,
    pub expected_stream_version: u64,
    pub display_name: String,
    pub path: String,
    #[serde(default)]
    pub additional_directories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registered {
    pub project_id: ProjectId,
    /// False when an existing project with the same name and roots was reused.
    pub created: bool,
}

fn canonical_directory(raw: &str) -> Result<String, String> {
    let requested = std::path::Path::new(raw);
    if !requested.is_absolute() {
        return Err("请输入完整的本地目录路径".to_owned());
    }
    let canonical = requested
        .canonicalize()
        .map_err(|error| format!("无法打开项目目录：{error}"))?;
    if !canonical.is_dir() {
        return Err("所选路径不是目录".to_owned());
    }
    std::fs::read_dir(&canonical).map_err(|error| format!("无法读取项目目录：{error}"))?;
    canonical
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "目录路径不是有效的 UTF-8".to_owned())
}

/// Returns the canonical primary root and the deduplicated additional roots.
pub async fn validate_directories(
    path: String,
    additional_directories: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    if additional_directories.len() > MAX_ADDITIONAL_DIRECTORIES {
        return Err("每个项目最多添加 32 个目录".into());
    }
    tokio::task::spawn_blocking(move || {
        let mut roots = Vec::new();
        for directory in additional_directories {
            let value = canonical_directory(&directory)?;
            if !roots.contains(&value) {
                roots.push(value);
            }
        }
        let path = canonical_directory(&path)?;
        roots.retain(|root| root != &path);
        Ok((path, roots))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Callers must hold the service project gate.
pub async fn register(
    core: &Arc<Core>,
    request: ProjectRegistration,
) -> Result<Registered, String> {
    let (path, additional_directories) =
        validate_directories(request.path, request.additional_directories).await?;
    let display_name = request.display_name.trim();
    let snapshot = core.bootstrap().await.map_err(|error| error.to_string())?;
    // Repeated submission is idempotent for the same name and canonical roots.
    if let Some(project) = snapshot.projects.into_iter().find(|project| {
        (display_name.is_empty() || project.display_name == display_name)
            && project.additional_directories == additional_directories
            && project.path == path
    }) {
        return Ok(Registered {
            project_id: project.project_id,
            created: false,
        });
    }
    let project_id = ProjectId::new();
    let display_name = if display_name.is_empty() {
        std::path::Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path)
            .to_owned()
    } else {
        display_name.to_owned()
    };
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            additional_directories,
            project_id: project_id.clone(),
            display_name,
            path,
        },
    )))
    .await
    .map_err(|error| error.to_string())?;
    Ok(Registered {
        project_id,
        created: true,
    })
}

/// Callers must hold the service project gate and the review scope gate.
pub async fn update(core: &Arc<Core>, request: ProjectEdit) -> Result<(), String> {
    let (path, additional_directories) =
        validate_directories(request.path, request.additional_directories).await?;
    let mut envelope = CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
        project_id: request.project_id,
        display_name: request.display_name.trim().to_owned(),
        path,
        additional_directories,
    }));
    envelope.expected_stream_version = Some(request.expected_stream_version);
    core.execute(envelope).await.map(|_| ()).map_err(|error| {
        let error = error.to_string();
        if error.contains("stream version conflict") {
            "项目已被修改，请重新打开编辑窗口后再保存".into()
        } else {
            error
        }
    })
}
