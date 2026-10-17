use super::{RepositoryError, SqliteStore};
use agent_core::history_preferences::{EVENT_TYPE, STREAM_ID};
use agent_protocol::HistoryPreferencesSnapshot;

impl SqliteStore {
    pub(super) fn read_history_preferences(
        &self,
    ) -> Result<HistoryPreferencesSnapshot, RepositoryError> {
        let (revision, last_sequence, preferences) =
            self.read_preferences(STREAM_ID, EVENT_TYPE)?;
        Ok(HistoryPreferencesSnapshot {
            revision,
            last_sequence,
            preferences,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Core;
    use agent_protocol::{
        ActorRef, AgentPreferences, CommandEnvelope, CoreCommand, HistoryPreferences,
        HistoryProjectState, HistorySort, HistoryTaskState,
    };
    use std::sync::Arc;

    fn command(revision: u64, state: HistoryTaskState) -> CommandEnvelope<CoreCommand> {
        let mut request =
            CommandEnvelope::local(CoreCommand::SetHistoryPreferences(HistoryPreferences {
                tasks: [("tsk_example".into(), state)].into(),
                projects: [("prj_example".into(), HistoryProjectState::Pinned)].into(),
                sorts: [("projects".into(), HistorySort::Updated)].into(),
                orders: [("project:prj_example".into(), vec!["tsk_example".into()])].into(),
            }));
        request.expected_stream_version = Some(revision);
        request
    }

    #[tokio::test]
    async fn history_preferences_survive_reopen_rebuild_and_lost_ack_without_overwriting_newer_state()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let first = Arc::new(Core::new(store.clone()));
        let second = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
        let request = command(0, HistoryTaskState::Pinned);
        let ack = first.execute(request.clone()).await.unwrap();
        assert_eq!(
            second.history_preferences().await.unwrap(),
            first.history_preferences().await.unwrap()
        );
        second
            .execute(command(1, HistoryTaskState::Archived))
            .await
            .unwrap();
        assert_eq!(first.execute(request.clone()).await.unwrap(), ack);
        assert!(
            first
                .execute(command(0, HistoryTaskState::Pinned))
                .await
                .is_err()
        );
        store.rebuild_projections().unwrap();
        drop(first);
        drop(second);
        drop(store);
        let reopened = Arc::new(Core::new(Arc::new(SqliteStore::open(path).unwrap())));
        assert_eq!(reopened.execute(request).await.unwrap(), ack);
        let current = reopened.history_preferences().await.unwrap();
        assert_eq!(current.revision, 2);
        assert_eq!(
            current.preferences.tasks["tsk_example"],
            HistoryTaskState::Archived
        );
        assert_eq!(
            current.preferences.orders["project:prj_example"],
            ["tsk_example"]
        );
        assert_eq!(reopened.sync_events(0, 100).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn history_preferences_cas_serializes_two_clients_but_not_other_settings_streams() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let first = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
        let second = Arc::new(Core::new(Arc::new(SqliteStore::open(&path).unwrap())));
        let (a, b) = tokio::join!(
            first.execute(command(0, HistoryTaskState::Pinned)),
            second.execute(command(0, HistoryTaskState::Archived))
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let mut agent =
            CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences::default()));
        agent.expected_stream_version = Some(0);
        first.execute(agent).await.unwrap();
        let snapshot = second.history_preferences().await.unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.last_sequence, 2);
        first
            .execute(command(1, HistoryTaskState::Archived))
            .await
            .unwrap();
        let replay = second
            .sync_events(snapshot.last_sequence, 100)
            .await
            .unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event_type, EVENT_TYPE);
    }

    #[tokio::test]
    async fn history_preferences_reject_unversioned_foreign_invalid_and_visual_settings() {
        let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
        let mut request = command(0, HistoryTaskState::Pinned);
        request.expected_stream_version = None;
        assert!(core.execute(request).await.is_err());
        let mut request = command(0, HistoryTaskState::Pinned);
        request.actor = ActorRef::SystemRecovery;
        assert!(core.execute(request).await.is_err());
        for payload in [
            serde_json::json!({"tasks":{},"projects":{},"sorts":{},"orders":{},"collapsed":true}),
            serde_json::json!({"tasks":{"a":"deleted"},"projects":{},"sorts":{},"orders":{}}),
        ] {
            assert!(serde_json::from_value::<agent_protocol::HistoryPreferences>(payload).is_err());
        }
        for orders in [vec!["same".into(), "same".into()], vec!["\n".into()]] {
            let mut request = command(0, HistoryTaskState::Pinned);
            if let CoreCommand::SetHistoryPreferences(value) = &mut request.payload {
                value.orders.insert("projects".into(), orders);
            }
            assert!(core.execute(request).await.is_err());
        }
        assert_eq!(core.history_preferences().await.unwrap().revision, 0);
        assert!(core.sync_events(0, 100).await.unwrap().is_empty());
    }
}
