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

    fn probe(target: Option<TargetId>, executable: &str) -> CommandEnvelope<CoreCommand> {
        let mut request = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                target_id: target,
                provider_id: "kimi".into(),
                executable: executable.into(),
                fingerprint: "fingerprint".into(),
                status: agent_protocol::InstallationStatus::Ready,
                version: Some("2.5.0".into()),
                version_text: None,
                diagnostic: None,
                elapsed_millis: 1,
            },
        ));
        request.actor = agent_protocol::ActorRef::SystemRecovery;
        request
    }

    #[tokio::test]
    async fn the_same_path_on_two_machines_is_two_installations() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path().join("t.db")).unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let target = TargetId::new();
        let path = "/usr/local/bin/kimi";
        core.execute(probe(None, path)).await.unwrap();
        core.execute(probe(Some(target.clone()), path))
            .await
            .unwrap();
        // Probing again updates each one instead of adding a third.
        core.execute(probe(Some(target.clone()), path))
            .await
            .unwrap();
        store.rebuild_projections().unwrap();
        let installations = core.bootstrap().await.unwrap().installations;
        assert_eq!(installations.len(), 2);
        assert!(installations.iter().any(|i| i.target_id.is_none()));
        assert!(
            installations
                .iter()
                .any(|i| i.target_id == Some(target.clone()))
        );
        // Choosing a local installation must not hide the remote one.
        core.select_installation("kimi", None).await;
        let visible = core.bootstrap().await.unwrap().installations;
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].target_id, Some(target));
    }

    #[tokio::test]
    async fn a_remote_project_keeps_its_machine_and_its_path_is_not_checked_here() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path().join("t.db")).unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let target = TargetId::new();
        let project = agent_domain::ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            agent_protocol::RegisterProject {
                target_id: Some(target.clone()),
                additional_directories: vec!["/only/on/that/machine".into()],
                project_id: project.clone(),
                display_name: "Remote".into(),
                path: "/srv/app".into(),
            },
        )))
        .await
        .unwrap();
        store.rebuild_projections().unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.projects[0].target_id, Some(target));
        assert_eq!(snapshot.projects[0].path, "/srv/app");
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
