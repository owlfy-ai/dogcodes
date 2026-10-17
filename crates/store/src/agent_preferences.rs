use super::{RepositoryError, SqliteStore};
use agent_core::agent_preferences::{EVENT_TYPE, STREAM_ID};
use agent_protocol::AgentPreferencesSnapshot;

impl SqliteStore {
    pub(super) fn read_agent_preferences(
        &self,
    ) -> Result<AgentPreferencesSnapshot, RepositoryError> {
        let (revision, last_sequence, preferences) =
            self.read_preferences(STREAM_ID, EVENT_TYPE)?;
        Ok(AgentPreferencesSnapshot {
            revision,
            last_sequence,
            preferences,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Core, CoreRepository};
    use agent_protocol::{ActorRef, AgentPreferences, CommandEnvelope, CoreCommand};
    use std::sync::Arc;

    fn command(revision: u64, disabled: &[&str]) -> CommandEnvelope<CoreCommand> {
        let mut envelope =
            CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
                order: vec!["codex".into(), "kimi".into()],
                disabled: disabled.iter().map(|id| (*id).into()).collect(),
            }));
        envelope.expected_stream_version = Some(revision);
        envelope
    }

    #[tokio::test]
    async fn durable_preferences_survive_clients_reopen_rebuild_and_lost_ack() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let request = command(0, &["kimi"]);
        let first_ack;
        {
            let store = Arc::new(SqliteStore::open(&path).unwrap());
            let first = Arc::new(Core::new(store.clone()));
            let second = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
            assert_eq!(first.agent_preferences().await.unwrap().revision, 0);
            let mut stream = first.subscribe(0).await.unwrap();
            first_ack = first.execute(request.clone()).await.unwrap();
            let event = stream.live.recv().await.unwrap();
            assert_eq!(event.event_type, EVENT_TYPE);
            assert_eq!(event.stream_version, 1);
            assert_eq!(
                second.agent_preferences().await.unwrap(),
                first.agent_preferences().await.unwrap()
            );
            // Same request after a lost response returns its original durable ack.
            assert_eq!(second.execute(request.clone()).await.unwrap(), first_ack);
            assert_eq!(store.sync_events(0, 100).await.unwrap().len(), 1);
            assert!(second.execute(command(0, &[])).await.is_err());
            assert_eq!(
                second
                    .agent_preferences()
                    .await
                    .unwrap()
                    .preferences
                    .disabled,
                ["kimi"]
            );
            second.execute(command(1, &[])).await.unwrap();
            assert_eq!(first.agent_preferences().await.unwrap().revision, 2);
            store.rebuild_projections().unwrap();
        }
        let reopened = Arc::new(Core::new(Arc::new(SqliteStore::open(path).unwrap())));
        assert_eq!(reopened.execute(request).await.unwrap(), first_ack);
        let value = reopened.agent_preferences().await.unwrap();
        assert_eq!(value.revision, 2);
        assert!(value.preferences.disabled.is_empty());
        assert_eq!(value.preferences.order, ["codex", "kimi"]);
    }

    #[tokio::test]
    async fn concurrent_writers_cannot_silently_overwrite_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let first = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
        let second = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
        let (a, b) = tokio::join!(
            first.execute(command(0, &["codex"])),
            second.execute(command(0, &["kimi"]))
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let value = first.agent_preferences().await.unwrap();
        assert_eq!(value.revision, 1);
        assert_eq!(value, second.agent_preferences().await.unwrap());
    }

    #[tokio::test]
    async fn snapshot_cursor_includes_unrelated_streams_without_changing_revision() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project = || {
            CommandEnvelope::local(CoreCommand::RegisterProject(
                agent_protocol::RegisterProject {
                    project_id: agent_domain::ProjectId::new(),
                    display_name: "settings cursor test".into(),
                    path: "/tmp".into(),
                    additional_directories: vec![],
                },
            ))
        };
        core.execute(project()).await.unwrap();
        let empty = core.agent_preferences().await.unwrap();
        assert_eq!(empty.revision, 0);
        assert_eq!(empty.last_sequence, 1);
        core.execute(command(0, &["kimi"])).await.unwrap();
        core.execute(project()).await.unwrap();
        let snapshot = core.agent_preferences().await.unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.last_sequence, 3);
        core.execute(command(1, &[])).await.unwrap();
        let replay = store
            .sync_events(snapshot.last_sequence, 100)
            .await
            .unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].stream_version, 2);
        assert_eq!(replay[0].event_type, EVENT_TYPE);
    }

    #[tokio::test]
    async fn invalid_or_unversioned_preferences_do_not_write_events() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let mut invalid = command(0, &[]);
        invalid.expected_stream_version = None;
        assert!(core.execute(invalid).await.is_err());
        let mut foreign = command(0, &[]);
        foreign.actor = ActorRef::SystemRecovery;
        assert!(core.execute(foreign).await.is_err());
        for ids in [
            vec!["kimi", "kimi"],
            vec![""],
            vec!["bad\nvalue"],
            vec!["x"; 257],
        ] {
            assert!(core.execute(command(0, &ids)).await.is_err());
        }
        assert_eq!(core.agent_preferences().await.unwrap().revision, 0);
        assert!(store.sync_events(0, 100).await.unwrap().is_empty());
    }
}
