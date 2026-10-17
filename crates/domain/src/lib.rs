//! Core domain types and state machines.

pub mod questions;
pub use questions::{QuestionAnswer, QuestionForm};

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ulid::Ulid;

macro_rules! prefixed_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, Ulid::new()))
            }

            pub fn parse(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                let Some(raw) = value.strip_prefix(concat!($prefix, "_")) else {
                    return Err(IdError::WrongPrefix {
                        expected: $prefix,
                        actual: value,
                    });
                };
                Ulid::from_string(raw).map_err(|_| IdError::InvalidUlid(value.clone()))?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdError {
    #[error("expected id prefix {expected}, got {actual}")]
    WrongPrefix {
        expected: &'static str,
        actual: String,
    },
    #[error("invalid ULID in id {0}")]
    InvalidUlid(String),
}

prefixed_id!(ProjectId, "prj");
prefixed_id!(TaskId, "tsk");
prefixed_id!(TurnId, "trn");
prefixed_id!(RunId, "run");
prefixed_id!(CommandId, "cmd");
prefixed_id!(EventId, "evt");
prefixed_id!(CorrelationId, "cor");
prefixed_id!(InteractionId, "pin");
prefixed_id!(DeviceId, "dev");
prefixed_id!(InstallationId, "ins");
prefixed_id!(SessionMapId, "ses");
prefixed_id!(ArtifactId, "art");

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty()
            || !value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(DomainError::InvalidProviderId(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ProviderId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorRef {
    pub provider_id: ProviderId,
    pub installation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Starting,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Interrupting,
    Interrupted,
    Completed,
    Failed,
    Disconnected,
    Reconnecting,
}

impl RunState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Interrupted | Self::Completed | Self::Failed)
    }

    pub fn transition(self, next: Self) -> Result<Self, DomainError> {
        use RunState::{
            Completed, Disconnected, Failed, Interrupted, Interrupting, Queued, Reconnecting,
            Running, Starting, WaitingForApproval, WaitingForInput,
        };
        let valid = matches!(
            (self, next),
            (Queued, Starting)
                | (Starting | Reconnecting, Running | Failed | Disconnected)
                | (
                    Running,
                    WaitingForInput
                        | WaitingForApproval
                        | Interrupting
                        | Completed
                        | Failed
                        | Disconnected
                )
                | (
                    WaitingForInput | WaitingForApproval,
                    Running | Interrupting | Failed | Disconnected
                )
                | (Interrupting, Interrupted | Failed)
                | (Disconnected, Reconnecting | Failed | Interrupted)
        );
        valid
            .then_some(next)
            .ok_or(DomainError::InvalidRunTransition {
                from: self,
                to: next,
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRun {
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub executor: ExecutorRef,
    pub state: RunState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecutionState {
    pub preferred_executor: ExecutorRef,
    pub active_primary_run: Option<ActiveRun>,
    pub secondary_runs: Vec<ActiveRun>,
    pub version: u64,
}

impl TaskExecutionState {
    #[must_use]
    pub fn executor_for_next_turn(&self, explicit: Option<&ExecutorRef>) -> ExecutorRef {
        explicit
            .cloned()
            .unwrap_or_else(|| self.preferred_executor.clone())
    }

    pub fn set_preferred_executor(
        &mut self,
        executor: ExecutorRef,
        expected_version: u64,
    ) -> Result<(), DomainError> {
        if self.version != expected_version {
            return Err(DomainError::VersionConflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        self.preferred_executor = executor;
        self.version += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionSemanticRole {
    ApproveOnce,
    Reject,
    Cancel,
    ApprovePersistent,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionOption {
    pub option_id: String,
    pub display_label: String,
    pub semantic_role: InteractionSemanticRole,
}

#[must_use]
pub fn quick_approval_options(
    options: &[InteractionOption],
    safe_summary_available: bool,
) -> Option<(&InteractionOption, &InteractionOption)> {
    if !safe_summary_available {
        return None;
    }
    let positive = options
        .iter()
        .filter(|o| o.semantic_role == InteractionSemanticRole::ApproveOnce)
        .collect::<Vec<_>>();
    let negative = options
        .iter()
        .filter(|o| {
            matches!(
                o.semantic_role,
                InteractionSemanticRole::Reject | InteractionSemanticRole::Cancel
            )
        })
        .collect::<Vec<_>>();
    match (positive.as_slice(), negative.as_slice()) {
        ([positive], [negative]) => Some((*positive, *negative)),
        _ => None,
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("provider id must contain only lowercase ASCII letters, digits, or hyphens: {0}")]
    InvalidProviderId(String),
    #[error("invalid run transition from {from:?} to {to:?}")]
    InvalidRunTransition { from: RunState, to: RunState },
    #[error("stream version conflict: expected {expected}, actual {actual}")]
    VersionConflict { expected: u64, actual: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executor(name: &str) -> ExecutorRef {
        ExecutorRef {
            provider_id: ProviderId::parse(name).unwrap(),
            installation_id: None,
        }
    }

    #[test]
    fn changing_preferred_executor_does_not_mutate_active_run() {
        let active_executor = executor("codex");
        let mut state = TaskExecutionState {
            preferred_executor: active_executor.clone(),
            active_primary_run: Some(ActiveRun {
                run_id: RunId::new(),
                turn_id: TurnId::new(),
                executor: active_executor.clone(),
                state: RunState::Running,
            }),
            secondary_runs: vec![],
            version: 4,
        };

        state.set_preferred_executor(executor("cursor"), 4).unwrap();

        assert_eq!(state.active_primary_run.unwrap().executor, active_executor);
        assert_eq!(state.preferred_executor, executor("cursor"));
        assert_eq!(state.version, 5);
    }

    #[test]
    fn quick_approval_requires_one_unambiguous_pair() {
        let options = vec![
            InteractionOption {
                option_id: "yes".into(),
                display_label: "Allow once".into(),
                semantic_role: InteractionSemanticRole::ApproveOnce,
            },
            InteractionOption {
                option_id: "always".into(),
                display_label: "Always allow".into(),
                semantic_role: InteractionSemanticRole::ApprovePersistent,
            },
            InteractionOption {
                option_id: "no".into(),
                display_label: "Reject".into(),
                semantic_role: InteractionSemanticRole::Reject,
            },
        ];

        let (positive, negative) = quick_approval_options(&options, true).unwrap();
        assert_eq!(positive.option_id, "yes");
        assert_eq!(negative.option_id, "no");
        assert!(quick_approval_options(&options, false).is_none());
    }

    #[test]
    fn artifact_id_is_a_prefixed_ulid() {
        let artifact_id = ArtifactId::new();
        assert_eq!(
            ArtifactId::parse(artifact_id.to_string()).unwrap(),
            artifact_id
        );
        assert!(ArtifactId::parse("art_not-a-ulid").is_err());
    }
}

/// A user-selected file retained by the application. Image paths never substitute
/// for inline image content on transports that support it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptAttachment {
    pub name: String,
    pub path: String,
    pub mime_type: String,
    /// Absent only for references written before managed uploads were introduced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<AttachmentIntegrity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentIntegrity {
    pub sha256: String,
    pub byte_length: u64,
}
