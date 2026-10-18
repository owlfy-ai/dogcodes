//! Frozen desktop submission intent; future transports reuse the same local service.
use crate::{ContextPreviewRequest, TaskSummary};
use agent_domain::{CommandId, ExecutorRef, ProjectId, PromptAttachment, TaskId, TurnId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionInput {
    pub prompt: String,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub permission_mode: Option<String>,
    pub attachments: Vec<PromptAttachment>,
}
/// Original client input for a send, before the service assigns task/stage IDs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionStartRequest {
    pub operation_id: CommandId,
    pub scope: String,
    pub task_id: Option<TaskId>,
    pub project_id: ProjectId,
    pub target: ExecutorRef,
    pub input: SubmissionInput,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionCreation {
    pub operation_id: CommandId,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionContext {
    pub operation_id: CommandId,
    pub request: ContextPreviewRequest,
    pub hash: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<crate::ActorRef>,
    pub schema_version: u8,
    pub operation_id: CommandId,
    pub scope: String,
    pub requested_at: String,
    pub task: TaskSummary,
    pub input: SubmissionInput,
    pub target: ExecutorRef,
    pub creation: Option<SubmissionCreation>,
    pub context: Option<SubmissionContext>,
    pub submission: Option<TaskSummary>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SubmissionResolution {
    /// The task tombstone forbids any future execution. This does not assert
    /// whether the old send ran before permanent deletion purged its events.
    TaskDeleted {
        #[serde(rename = "taskId")]
        task_id: TaskId,
        #[serde(rename = "deletionSequence")]
        deletion_sequence: u64,
    },
    Submitted {
        #[serde(rename = "taskId")]
        task_id: TaskId,
        sequence: u64,
        #[serde(rename = "turnId")]
        turn_id: TurnId,
    },
    Invalidated {
        #[serde(rename = "taskId")]
        task_id: TaskId,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubmissionIntentEntry {
    Pending {
        intent: Box<SubmissionIntent>,
    },
    Closed {
        fingerprint: String,
        resolution: SubmissionResolution,
        /// Fingerprint of the original start request (`start_request_fingerprint`);
        /// absent on records closed before it was recorded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionIntentPage {
    pub operation_ids: Vec<CommandId>,
    pub next_cursor: Option<CommandId>,
    /// Directory entries without a usable identity must not look like an empty journal.
    #[serde(default)]
    pub unrecognized_entries: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubmissionIntentCall {
    Resolve {
        intent: Box<SubmissionIntent>,
    },
    Put {
        intent: Box<SubmissionIntent>,
    },
    Get {
        #[serde(rename = "operationId")]
        operation_id: CommandId,
        expected: Option<Box<SubmissionIntent>>,
    },
    /// Checked lookup is a separate variant so older services cannot silently
    /// ignore the expected request and return an unrelated receipt.
    GetStart {
        #[serde(rename = "operationId")]
        operation_id: CommandId,
        request: Box<SubmissionStartRequest>,
    },
    List {
        after: Option<CommandId>,
        limit: usize,
    },
    Close {
        intent: Box<SubmissionIntent>,
    },
    /// The service freezes a new send and drives it to acceptance. Retrying the
    /// same operation with the same request continues the original instead.
    Start {
        #[serde(rename = "operationId")]
        operation_id: CommandId,
        scope: String,
        /// Absent for a new conversation, which the service creates first.
        #[serde(rename = "taskId")]
        task_id: Option<TaskId>,
        #[serde(rename = "projectId")]
        project_id: ProjectId,
        target: ExecutorRef,
        input: Box<SubmissionInput>,
    },
    /// Explicitly continue a pending original send with its frozen identities.
    Continue {
        #[serde(rename = "operationId")]
        operation_id: CommandId,
    },
    /// Fence every not-yet-accepted step of a pending original and close it.
    Retire {
        #[serde(rename = "operationId")]
        operation_id: CommandId,
    },
}
