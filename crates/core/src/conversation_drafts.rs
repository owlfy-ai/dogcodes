//! Conversations without a project share one scratch folder. Each new one
//! works in its own `drafts/<conversation id>` folder inside it instead, so
//! conversations never see, change or get credited with each other's files.
//! Conversations created before this keep working in the scratch folder
//! itself: their agents' saved sessions are tied to that folder.
use crate::{Core, CoreError};
use agent_domain::{ProjectId, TaskId};
use agent_protocol::TaskSummary;
use std::path::{Path, PathBuf};

impl Core {
    /// `scratch` is the folder of the project used by conversations without a
    /// project.
    #[must_use]
    pub fn with_conversation_drafts(mut self, scratch: PathBuf) -> Self {
        self.draft_root = Some(scratch);
        self
    }

    /// The folder a new conversation in `project` gets for itself, created
    /// here, or `None` when it works in the project's folder.
    pub(crate) async fn new_draft_directory(
        &self,
        project: &ProjectId,
        task: &TaskId,
    ) -> Result<Option<String>, CoreError> {
        let Some(root) = &self.draft_root else {
            return Ok(None);
        };
        let scratch = self
            .repository
            .bootstrap()
            .await?
            .projects
            .into_iter()
            .any(|p| {
                &p.project_id == project && p.target_id.is_none() && Path::new(&p.path) == root
            });
        if !scratch {
            return Ok(None);
        }
        let directory = draft_directory(root, task);
        create(&directory).await?;
        mark_drafts(&root.join("drafts")).await?;
        Ok(Some(directory.to_string_lossy().into_owned()))
    }

    /// Recreates a conversation's own folder removed since, before a turn.
    pub(crate) async fn restore_draft_directory(task: &TaskSummary, local: bool) {
        if let Some(directory) = task.working_directory.as_deref().filter(|_| local) {
            let directory = Path::new(directory);
            if create(directory).await.is_ok()
                && let Some(drafts) = directory.parent()
            {
                let _ = mark_drafts(drafts).await;
            }
        }
    }
}

fn draft_directory(root: &Path, task: &TaskId) -> PathBuf {
    root.join("drafts").join(task.as_str())
}

/// Keeps captures of the scratch folder itself out of `drafts`.
async fn mark_drafts(drafts: &Path) -> Result<(), CoreError> {
    let marker = drafts.join(agent_protocol::workspace_context::CONVERSATION_FOLDERS_MARKER);
    if tokio::fs::try_exists(&marker).await.unwrap_or(false) {
        return Ok(());
    }
    tokio::fs::write(
        &marker,
        "Each folder here is one dogcodes conversation's own working folder.\n",
    )
    .await
    .map_err(|_| CoreError::Internal("会话的临时工作目录无法创建".into()))
}

async fn create(directory: &Path) -> Result<(), CoreError> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|_| CoreError::Internal("会话的临时工作目录无法创建".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_conversation_gets_a_folder_named_after_it() {
        let task = TaskId::new();
        assert_eq!(
            draft_directory(Path::new("/data/scratch"), &task),
            Path::new("/data/scratch/drafts").join(task.as_str())
        );
    }
}
