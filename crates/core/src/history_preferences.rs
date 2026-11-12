use crate::{Core, CoreError};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, HistoryPreferences, HistoryPreferencesSnapshot,
};
use std::collections::BTreeSet;

pub const STREAM_ID: &str = "preferences.history";
pub const EVENT_TYPE: &str = "preferences.history.updated";

impl Core {
    pub async fn history_preferences(&self) -> Result<HistoryPreferencesSnapshot, CoreError> {
        Ok(self.repository.history_preferences().await?)
    }
}

pub(crate) fn validate(
    value: &HistoryPreferences,
    envelope: &CommandEnvelope<CoreCommand>,
) -> Result<(), CoreError> {
    if !matches!(
        envelope.actor,
        ActorRef::LocalUser | ActorRef::PairedDevice(_)
    ) || envelope.expected_stream_version.is_none()
    {
        return Err(CoreError::InvalidArgument(
            "backendText.m8621c5b2".into(),
        ));
    }
    let key_valid =
        |key: &str| !key.is_empty() && key.len() <= 512 && !key.chars().any(char::is_control);
    if value.tasks.len() > 20_000
        || value.projects.len() > 20_000
        || value.orders.len() > 20_000
        || value
            .tasks
            .keys()
            .chain(value.projects.keys())
            .chain(value.orders.keys())
            .any(|key| !key_valid(key))
        || value
            .sorts
            .keys()
            .any(|key| !matches!(key.as_str(), "pinned" | "projects" | "archived"))
        || value.orders.values().map(Vec::len).sum::<usize>() > 100_000
        || value.orders.values().any(|ids| {
            ids.iter().any(|id| !key_valid(id))
                || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        })
        || serde_json::to_vec(value)
            .map_err(|e| CoreError::InvalidArgument(e.to_string()))?
            .len()
            > 4 * 1024 * 1024
    {
        return Err(CoreError::InvalidArgument(
            "backendText.m240cba6f".into(),
        ));
    }
    Ok(())
}
