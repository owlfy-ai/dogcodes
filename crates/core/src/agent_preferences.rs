//! Host-owned Agent settings, using the canonical event/ack transaction.
use crate::{Core, CoreError};
use agent_protocol::{
    ActorRef, AgentPreferences, AgentPreferencesSnapshot, CommandEnvelope, CoreCommand,
};
use std::collections::BTreeSet;

pub const STREAM_ID: &str = "preferences.agents";
pub const EVENT_TYPE: &str = "preferences.agents.updated";

impl Core {
    pub async fn agent_preferences(&self) -> Result<AgentPreferencesSnapshot, CoreError> {
        Ok(self.repository.agent_preferences().await?)
    }
}

pub(crate) fn validate(
    preferences: &AgentPreferences,
    envelope: &CommandEnvelope<CoreCommand>,
) -> Result<(), CoreError> {
    if envelope.actor != ActorRef::LocalUser {
        return Err(CoreError::InvalidArgument(
            "Agent preferences require an authenticated local user".into(),
        ));
    }
    if envelope.expected_stream_version.is_none() {
        return Err(CoreError::InvalidArgument("backendText.m9ecc624f".into()));
    }
    for values in [&preferences.order, &preferences.disabled] {
        if values.len() > 256
            || values.iter().any(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            })
            || values.iter().collect::<BTreeSet<_>>().len() != values.len()
        {
            return Err(CoreError::InvalidArgument("backendText.mfe516439".into()));
        }
    }
    Ok(())
}
