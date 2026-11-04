use super::{RepositoryError, SqliteStore};
use agent_core::execution_targets::{EVENT_TYPE, STREAM_ID};
use agent_protocol::ExecutionTargetsSnapshot;

impl SqliteStore {
    pub(super) fn read_execution_targets(
        &self,
    ) -> Result<ExecutionTargetsSnapshot, RepositoryError> {
        let (revision, last_sequence, targets) = self.read_preferences(STREAM_ID, EVENT_TYPE)?;
        Ok(ExecutionTargetsSnapshot {
            revision,
            last_sequence,
            targets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Core;
    use agent_domain::TargetId;
    use agent_protocol::{
        CommandEnvelope, CoreCommand, ExecutionTarget, ExecutionTargets, SshAuth, SshConnection,
    };
    use std::sync::Arc;

    fn command(revision: u64, name: &str, id: &TargetId) -> CommandEnvelope<CoreCommand> {
        let mut request =
            CommandEnvelope::local(CoreCommand::SetExecutionTargets(ExecutionTargets {
                targets: vec![ExecutionTarget {
                    target_id: id.clone(),
                    display_name: name.into(),
                    connection: SshConnection {
                        destination: "build-box".into(),
                        port: Some(2222),
                        user: Some("dev".into()),
                        auth: SshAuth::System,
                        identity_file: None,
                        install_directory: None,
                    },
                    machine_identity: None,
                }],
            }));
        request.expected_stream_version = Some(revision);
        request
    }

    #[tokio::test]
    async fn targets_survive_reopen_and_rebuild_and_stale_writes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("targets.db");
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let core = Arc::new(Core::new(store.clone()));
        assert_eq!(core.execution_targets().await.unwrap().revision, 0);
        let id = TargetId::new();
        core.execute(command(0, "First", &id)).await.unwrap();
        core.execute(command(1, "Second", &id)).await.unwrap();
        assert!(core.execute(command(0, "Stale", &id)).await.is_err());
        store.rebuild_projections().unwrap();
        drop(core);
        drop(store);
        let reopened = Core::new(Arc::new(SqliteStore::open(path).unwrap()));
        let current = reopened.execution_targets().await.unwrap();
        assert_eq!(current.revision, 2);
        assert_eq!(current.targets.targets[0].display_name, "Second");
        assert_eq!(current.targets.targets[0].connection.port, Some(2222));
    }
}
