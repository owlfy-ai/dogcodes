//! Deterministic Handoff and Reconciliation context package construction.

use agent_domain::{ExecutorRef, TaskId};
use agent_protocol::{
    BuiltContextPackage, ChangeSummary, ContextDecision, ContextPackageKind, ContextPackageV1,
    ContextPointer, ContextPreviewRequest, ContextProvenance, ContextStrength, ContextTestResult,
    EventEnvelope, RelevantFile, SemanticSummary, TaskSummary,
};
use chrono::{DateTime, Utc};
use redaction::SecretRedactor;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use thiserror::Error;

use crate::WorkspaceContextSnapshot;

pub const CONTEXT_PACKAGE_SCHEMA_VERSION: u16 = 1;
pub const CONTEXT_BUILDER_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPackageBuildInput {
    pub kind: ContextPackageKind,
    pub strength: ContextStrength,
    pub source_executor: ExecutorRef,
    pub target_executor: ExecutorRef,
    pub objective: String,
    pub user_constraints: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub completed_work: Vec<String>,
    pub decisions: Vec<ContextDecision>,
    pub pending_work: Vec<String>,
    pub relevant_files: Vec<RelevantFile>,
    pub dependency_changes: Vec<RelevantFile>,
    pub change_summary: ChangeSummary,
    pub tests: Vec<ContextTestResult>,
    pub recent_turn_refs: Vec<String>,
    pub context_pointers: Vec<ContextPointer>,
    pub semantic_summary: Option<SemanticSummary>,
    pub source_after_sequence: u64,
    pub source_through_sequence: u64,
    pub source_sequences: Vec<u64>,
    pub built_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ContextBuildError {
    #[error("context source range is invalid")]
    InvalidSourceRange,
    #[error("context payload serialization failed: {0}")]
    Serialization(String),
}

/// Builds the immutable audit package used for previews and delivery evidence.
///
/// Empty facts remain empty and a missing objective becomes the literal `unknown`; this builder
/// never invokes a model or invents semantic conclusions. The returned hash covers the complete
/// serialized package, including provenance and any explicitly supplied semantic summary.
pub fn build_context_package(
    mut input: ContextPackageBuildInput,
    redactor: &dyn SecretRedactor,
) -> Result<BuiltContextPackage, ContextBuildError> {
    if input.source_after_sequence > input.source_through_sequence
        || input.source_sequences.iter().any(|sequence| {
            *sequence <= input.source_after_sequence || *sequence > input.source_through_sequence
        })
    {
        return Err(ContextBuildError::InvalidSourceRange);
    }
    input.source_sequences.sort_unstable();
    input.source_sequences.dedup();

    let package = ContextPackageV1 {
        delivery_facts: vec![],
        conversation_text: String::new(),
        schema_version: CONTEXT_PACKAGE_SCHEMA_VERSION,
        kind: input.kind,
        strength: input.strength,
        source_executor: input.source_executor,
        target_executor: input.target_executor,
        objective: unknown_if_blank(input.objective),
        user_constraints: input.user_constraints,
        acceptance_criteria: input.acceptance_criteria,
        completed_work: input.completed_work,
        decisions: input.decisions,
        pending_work: input.pending_work,
        relevant_files: input.relevant_files,
        dependency_changes: input.dependency_changes,
        change_summary: input.change_summary,
        tests: input.tests,
        recent_turn_refs: input.recent_turn_refs,
        context_pointers: input.context_pointers,
        semantic_summary: input.semantic_summary,
        provenance: ContextProvenance {
            builder_version: CONTEXT_BUILDER_VERSION,
            built_at: input.built_at,
            redaction_ruleset_version: redactor.ruleset_version(),
            redaction_count: 0,
            source_after_sequence: input.source_after_sequence,
            source_through_sequence: input.source_through_sequence,
            source_sequences: input.source_sequences,
        },
    };
    redact_and_hash(package, redactor)
}

/// Re-hashes an edited preview as a new immutable version without altering the original package.
pub fn rehash_edited_package(
    package: ContextPackageV1,
    redactor: &dyn SecretRedactor,
) -> Result<BuiltContextPackage, ContextBuildError> {
    redact_and_hash(package, redactor)
}

/// Render model-facing context without local delivery receipts and audit bookkeeping.
/// The stored package stays intact; the driver hash covers these actual transmitted bytes.
pub(crate) fn prepare_model_delivery(
    delivery: &mut driver_api::ContextDelivery,
) -> Result<Vec<agent_protocol::HistoryFactDelivery>, ContextBuildError> {
    let package: ContextPackageV1 = serde_json::from_slice(&delivery.content)
        .map_err(|e| ContextBuildError::Serialization(e.to_string()))?;
    let mut value = serde_json::to_value(&package)
        .map_err(|e| ContextBuildError::Serialization(e.to_string()))?;
    let object = value.as_object_mut().expect("context package is an object");
    for key in [
        "deliveryFacts",
        "provenance",
        "recentTurnRefs",
        "schemaVersion",
        "strength",
    ] {
        object.remove(key);
    }
    for key in ["sourceExecutor", "targetExecutor"] {
        if let Some(executor) = object
            .get_mut(key)
            .and_then(serde_json::Value::as_object_mut)
        {
            executor.remove("installationId");
        }
    }
    if let Some(pointers) = object
        .get_mut("contextPointers")
        .and_then(serde_json::Value::as_array_mut)
    {
        for pointer in pointers {
            if let Some(pointer) = pointer.as_object_mut() {
                for key in ["id", "contentHash", "byteLength", "sensitivity"] {
                    pointer.remove(key);
                }
            }
        }
    }
    if let Some(summary) = object
        .get_mut("semanticSummary")
        .and_then(serde_json::Value::as_object_mut)
    {
        for key in ["sourceRunId", "inputSourceSequences", "outputArtifactRef"] {
            summary.remove(key);
        }
    }
    if package.change_summary.artifact_ref.is_none()
        && [
            &package.change_summary.base,
            &package.change_summary.head,
            &package.change_summary.diff_stat,
        ]
        .iter()
        .all(|text| text.is_empty() || text.as_str() == "unknown")
    {
        object.remove("changeSummary");
    }
    object.retain(|_, value| {
        !value.is_null()
            && !value.as_array().is_some_and(Vec::is_empty)
            && !value.as_str().is_some_and(str::is_empty)
    });
    delivery.content =
        serde_json::to_vec(&value).map_err(|e| ContextBuildError::Serialization(e.to_string()))?;
    delivery.payload_sha256 = hex::encode(Sha256::digest(&delivery.content));
    Ok(package.delivery_facts)
}

/// Only omit positively empty captures. Unknown shapes, omitted/binary patches,
/// diagnostics and interrupted captures must remain visible to the next agent.
pub(crate) fn empty_workspace_change(value: &serde_json::Value) -> bool {
    fn empty(value: &serde_json::Value, root: bool) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "schemaVersion"
                    | "backend"
                    | "attribution"
                    | "scope"
                    | "excludedNames"
                    | "files"
                    | "patch"
                    | "patchHex"
                    | "roots"
                    | "workspacePaths"
            )
        }) {
            return false;
        }
        if object.get("patchHex").is_some_and(|patch| !patch.is_null()) {
            return false;
        }
        let files_empty = match object.get("files") {
            Some(files) => files.as_array().is_some_and(Vec::is_empty),
            None => !root,
        };
        files_empty
            && object.get("patch").and_then(serde_json::Value::as_str) == Some("")
            && object.get("roots").is_none_or(|roots| {
                roots.as_array().is_some_and(|roots| {
                    roots.iter().all(|root| {
                        root.as_object().is_some_and(|object| {
                            object
                                .keys()
                                .all(|key| matches!(key.as_str(), "workspacePath" | "changes"))
                        }) && root
                            .get("changes")
                            .is_some_and(|change| empty(change, false))
                    })
                })
            })
    }
    empty(value, true)
}

fn redact_and_hash(
    mut package: ContextPackageV1,
    redactor: &dyn SecretRedactor,
) -> Result<BuiltContextPackage, ContextBuildError> {
    let finding_count = redact_package(&mut package, redactor);
    package.provenance.redaction_ruleset_version = redactor.ruleset_version();
    package.provenance.redaction_count = package
        .provenance
        .redaction_count
        .saturating_add(finding_count);
    if package.delivery_facts.len() > 512 {
        package.delivery_facts.truncate(512);
    }
    let payload = serialize_canonical(&package)?;
    let payload_sha256 = hex::encode(Sha256::digest(&payload));
    Ok(BuiltContextPackage {
        package,
        payload_sha256,
        byte_length: payload.len() as u64,
    })
}

/// Builds attributed original conversation text from a frozen event range.
/// Terminal bodies and full diffs remain separate artifacts.
pub fn build_context_preview(
    task: &TaskSummary,
    request: &ContextPreviewRequest,
    events: &[EventEnvelope],
    built_at: DateTime<Utc>,
    workspace: WorkspaceContextSnapshot,
    redactor: &dyn SecretRedactor,
) -> Result<BuiltContextPackage, ContextBuildError> {
    let source_sequences = events
        .iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    let recent_turn_refs = retain_recent(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type.as_str(),
                    "turn.queued" | "turn.autonomous.started"
                )
            })
            .filter_map(|event| string_field(event, "turnId"))
            .collect(),
        request.strength,
    );
    let mut decision_budget = 32_000;
    let decisions = events
        .iter()
        .filter(|event| event.event_type == "decision.recorded")
        .filter_map(|event| {
            let statement = string_field(event, "statement")?;
            if statement.len() > decision_budget {
                return None;
            }
            decision_budget -= statement.len();
            Some(ContextDecision {
                statement,
                source_ref: format!("event:{}", event.event_id),
            })
        })
        .collect();
    let mut test_budget = 32_000;
    let tests = events
        .iter()
        .filter(|event| event.event_type == "test.completed")
        .filter_map(|event| {
            let command = string_field(event, "command")?;
            let outcome = string_field(event, "outcome")?;
            if command.len() + outcome.len() > test_budget {
                return None;
            }
            test_budget -= command.len() + outcome.len();
            Some(ContextTestResult {
                command,
                outcome,
                artifact_ref: string_field(event, "artifactRef"),
            })
        })
        .collect();
    let pointer = history_pointer(task, request, events)?;

    let mut built = build_context_package(
        ContextPackageBuildInput {
            kind: request.kind,
            strength: request.strength,
            source_executor: request.source_executor.clone(),
            target_executor: request.target_executor.clone(),
            objective: format!(
                "Continue the same task. Display title (not a task instruction): {}",
                task.title
            ),
            user_constraints: vec![],
            acceptance_criteria: vec![],
            completed_work: vec![],
            decisions,
            pending_work: vec![],
            relevant_files: workspace.relevant_files,
            dependency_changes: workspace.dependency_changes,
            change_summary: workspace.change_summary,
            tests,
            recent_turn_refs,
            context_pointers: vec![pointer],
            semantic_summary: None,
            source_after_sequence: request.after_sequence,
            source_through_sequence: request.through_sequence,
            source_sequences,
            built_at,
        },
        redactor,
    )?;
    let (text, facts) = conversation_with_facts(events, request.strength);
    built.package.conversation_text = text;
    let mut built = redact_and_hash(built.package, redactor)?;
    // A redacted representation never acknowledges the original byte range.
    built.package.delivery_facts = facts
        .into_iter()
        .filter(|f| {
            events
                .iter()
                .find(|e| e.sequence == f.sequence)
                .and_then(crate::history_delivery::event_fact)
                .is_some_and(|(_, text)| redactor.redact(&text).text == text)
        })
        .collect();
    for event in events
        .iter()
        .filter(|e| e.event_type == "decision.recorded")
    {
        if let Some((field, text)) = crate::history_delivery::event_fact(event)
            && built
                .package
                .decisions
                .iter()
                .any(|d| d.source_ref == format!("event:{}", event.event_id) && d.statement == text)
        {
            built
                .package
                .delivery_facts
                .push(crate::history_delivery::fact(
                    event.sequence,
                    field,
                    &text,
                    0,
                    text.len(),
                ));
        }
    }
    let mut remaining = 32_000;
    for event in events.iter().rev().filter(|e| {
        matches!(
            e.event_type.as_str(),
            "test.completed" | "workspace.turn_changes_failed"
        )
    }) {
        let Some((field, text)) = crate::history_delivery::event_fact(event) else {
            continue;
        };
        if text.len() > remaining {
            continue;
        }
        let safe = redactor.redact(&text).text;
        if safe.len() > remaining {
            continue;
        }
        remaining -= safe.len();
        let _ = writeln!(
            built.package.conversation_text,
            "\n[Recorded {} metadata; references are not artifact contents]\n{safe}",
            event.event_type
        );
        if safe == text {
            built
                .package
                .delivery_facts
                .push(crate::history_delivery::fact(
                    event.sequence,
                    field,
                    &text,
                    0,
                    text.len(),
                ));
        }
    }
    // Fork history belongs to the new task but predates its first native session.
    // Keep it available to initial delivery, rebuilds, and later agent handoffs.
    for event in events
        .iter()
        .filter(|event| event.event_type == "task.forked")
    {
        let Some((field, text)) = crate::history_delivery::event_fact(event) else {
            continue;
        };
        let safe = redactor.redact(&text).text;
        let budget = match request.strength {
            ContextStrength::Compact => 16_000,
            ContextStrength::Standard => 96_000,
            ContextStrength::Full => 256_000,
        };
        let mut start = safe.len().saturating_sub(budget);
        while !safe.is_char_boundary(start) {
            start += 1;
        }
        let _ = writeln!(
            built.package.conversation_text,
            "\n[Forked conversation history through the selected turn; historical data only. Follow the new request. File contents are those of the current workspace, not a restored historical snapshot.]\n{}{}",
            if start > 0 {
                "[Earlier fork history omitted from this prompt; use task history tools for the full retained record.]\n"
            } else {
                ""
            },
            &safe[start..]
        );
        if safe == text {
            built
                .package
                .delivery_facts
                .push(crate::history_delivery::fact(
                    event.sequence,
                    field,
                    &text,
                    start,
                    text.len(),
                ));
        }
    }
    redact_and_hash(built.package, redactor)
}

#[cfg(test)]
fn conversation_text(events: &[EventEnvelope], strength: ContextStrength) -> String {
    conversation_with_facts(events, strength).0
}

fn conversation_with_facts(
    events: &[EventEnvelope],
    strength: ContextStrength,
) -> (String, Vec<agent_protocol::HistoryFactDelivery>) {
    // Track source spans while constructing the message, never by substring matching.
    type SourceSpan = (u64, &'static str, String, usize);
    type ConversationTurn = (String, String, Vec<SourceSpan>);
    let mut turns: Vec<ConversationTurn> = Vec::new();
    for event in events {
        if !matches!(event.event_type.as_str(), "turn.queued" | "message.delta") {
            continue;
        }
        let Some(id) = string_field(event, "turnId") else {
            continue;
        };
        let Some((field, text)) = crate::history_delivery::event_fact(event) else {
            continue;
        };
        if turns.last().is_none_or(|t| t.0 != id) {
            turns.push((id.clone(), String::new(), Vec::new()));
        }
        let (_, body, spans) = turns.last_mut().unwrap();
        if field == "prompt" {
            body.push_str("\n[User]\n");
        } else if spans.last().is_none_or(|span| span.1 != "text") {
            let provider = match &event.source {
                agent_protocol::EventSource::Driver { provider_id, .. } => provider_id.as_str(),
                agent_protocol::EventSource::Hub => "agent",
            };
            let _ = writeln!(body, "\n[Assistant: {provider}]");
        }
        let start = body.len();
        body.push_str(&text);
        spans.push((event.sequence, field, text, start));
    }
    let budget = match strength {
        ContextStrength::Compact => 16_000,
        ContextStrength::Standard => 96_000,
        ContextStrength::Full => 256_000,
    };
    let total = turns.len();
    let mut selected = Vec::new();
    let mut facts = Vec::new();
    let mut remaining = budget;
    for (id, body, spans) in turns.into_iter().rev() {
        if body.len() > remaining && !selected.is_empty() {
            break;
        }
        let mut end = body.len().min(remaining);
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = end < body.len();
        for (sequence, field, text, start) in spans {
            if start <= end {
                let count = text.len().min(end - start);
                if count > 0 || text.is_empty() {
                    facts.push(crate::history_delivery::fact(
                        sequence, field, &text, 0, count,
                    ));
                }
            }
        }
        selected.push(format!(
            "[Turn {id}{}]\n{}{}",
            if truncated { "; truncated" } else { "" },
            &body[..end],
            if truncated {
                "\n[Remaining text omitted]"
            } else {
                ""
            }
        ));
        remaining -= end;
        if truncated {
            break;
        }
    }
    selected.reverse();
    (
        format!(
            "Original conversation records: {} of {total} turns included. Earlier omitted records are not present in this prompt. Use task history tools for missing facts.\n\n{}",
            selected.len(),
            selected.join("\n\n")
        ),
        facts,
    )
}

fn redact_package(package: &mut ContextPackageV1, redactor: &dyn SecretRedactor) -> u32 {
    let mut total = 0_u32;
    redact_value(&mut package.conversation_text, redactor, &mut total);
    redact_value(&mut package.objective, redactor, &mut total);
    redact_values(&mut package.user_constraints, redactor, &mut total);
    redact_values(&mut package.acceptance_criteria, redactor, &mut total);
    redact_values(&mut package.completed_work, redactor, &mut total);
    for decision in &mut package.decisions {
        redact_value(&mut decision.statement, redactor, &mut total);
    }
    redact_values(&mut package.pending_work, redactor, &mut total);
    for file in &mut package.relevant_files {
        redact_value(&mut file.path, redactor, &mut total);
    }
    for dependency in &mut package.dependency_changes {
        redact_value(&mut dependency.path, redactor, &mut total);
    }
    redact_value(&mut package.change_summary.diff_stat, redactor, &mut total);
    for test in &mut package.tests {
        redact_value(&mut test.command, redactor, &mut total);
        redact_value(&mut test.outcome, redactor, &mut total);
    }
    for pointer in &mut package.context_pointers {
        redact_value(&mut pointer.title, redactor, &mut total);
        // Only the exact typed, frozen history locator is exempt from text heuristics.
        // Arbitrary user/provider locators (including credential-bearing URLs) still redact.
        if !is_frozen_history_pointer(pointer, &package.provenance) {
            redact_value(&mut pointer.locator, redactor, &mut total);
        }
        redact_values(&mut pointer.trigger_hints, redactor, &mut total);
    }
    if let Some(summary) = &mut package.semantic_summary {
        redact_value(&mut summary.text, redactor, &mut total);
    }
    total
}

fn is_frozen_history_pointer(pointer: &ContextPointer, provenance: &ContextProvenance) -> bool {
    let Some((task, query)) = pointer
        .locator
        .strip_prefix("task/")
        .and_then(|value| value.split_once("/events?"))
    else {
        return false;
    };
    pointer.kind == "canonical_history"
        && TaskId::parse(task).is_ok()
        && query
            == format!(
                "after={}&through={}",
                provenance.source_after_sequence, provenance.source_through_sequence
            )
        && pointer.id
            == format!(
                "history:{task}:{}-{}",
                provenance.source_after_sequence, provenance.source_through_sequence
            )
        && pointer.content_hash.len() == 64
        && pointer
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn redact_values(values: &mut [String], redactor: &dyn SecretRedactor, total: &mut u32) {
    for value in values {
        redact_value(value, redactor, total);
    }
}

fn redact_value(value: &mut String, redactor: &dyn SecretRedactor, total: &mut u32) {
    let redacted = redactor.redact(value);
    *total = total.saturating_add(redacted.finding_count());
    *value = redacted.text;
}

fn history_pointer(
    task: &TaskSummary,
    request: &ContextPreviewRequest,
    events: &[EventEnvelope],
) -> Result<ContextPointer, ContextBuildError> {
    let serialized = serialize_canonical(&events)?;
    Ok(ContextPointer {
        id: format!(
            "history:{}:{}-{}",
            task.task_id, request.after_sequence, request.through_sequence
        ),
        title: "Frozen task history".into(),
        kind: "canonical_history".into(),
        locator: format!(
            "task/{}/events?after={}&through={}",
            task.task_id, request.after_sequence, request.through_sequence
        ),
        content_hash: hex::encode(Sha256::digest(&serialized)),
        trigger_hints: vec!["history".into(), "audit".into()],
        byte_length: serialized.len() as u64,
        sensitivity: "project".into(),
    })
}

fn string_field(event: &EventEnvelope, field: &str) -> Option<String> {
    event
        .payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn retain_recent<T>(mut values: Vec<T>, strength: ContextStrength) -> Vec<T> {
    let retain = match strength {
        ContextStrength::Compact => 1,
        ContextStrength::Standard => 5,
        ContextStrength::Full => return values,
    };
    if values.len() > retain {
        values.drain(..values.len() - retain);
    }
    values
}

fn serialize_canonical(value: &impl Serialize) -> Result<Vec<u8>, ContextBuildError> {
    serde_json::to_vec(value).map_err(|error| ContextBuildError::Serialization(error.to_string()))
}

fn unknown_if_blank(value: String) -> String {
    if value.trim().is_empty() {
        "unknown".into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use redaction::DeterministicRedactor;

    fn input(kind: ContextPackageKind) -> ContextPackageBuildInput {
        ContextPackageBuildInput {
            kind,
            strength: ContextStrength::Standard,
            source_executor: agent_domain::ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("fake").unwrap(),
                installation_id: None,
            },
            target_executor: agent_domain::ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("fake-peer").unwrap(),
                installation_id: None,
            },
            objective: "Implement deterministic handoff".into(),
            user_constraints: vec!["Do not call a hidden model".into()],
            acceptance_criteria: vec!["Payload hash is reproducible".into()],
            completed_work: vec!["Captured canonical events".into()],
            decisions: vec![ContextDecision {
                statement: "Use canonical JSON".into(),
                source_ref: "event:42".into(),
            }],
            pending_work: vec!["Preview in UI".into()],
            relevant_files: vec![RelevantFile {
                path: "src/context.rs".into(),
                status: "modified".into(),
                content_hash: Some("sha256:content".into()),
            }],
            dependency_changes: vec![],
            change_summary: ChangeSummary {
                base: "base".into(),
                head: "head".into(),
                diff_stat: "1 file changed".into(),
                artifact_ref: Some("artifact:diff".into()),
            },
            tests: vec![ContextTestResult {
                command: "cargo test".into(),
                outcome: "passed".into(),
                artifact_ref: None,
            }],
            recent_turn_refs: vec!["turn:latest".into()],
            context_pointers: vec![ContextPointer {
                id: "context:requirements".into(),
                title: "Requirements".into(),
                kind: "document".into(),
                locator: "artifact:requirements".into(),
                content_hash: "sha256:requirements".into(),
                trigger_hints: vec!["acceptance".into()],
                byte_length: 128,
                sensitivity: "project".into(),
            }],
            semantic_summary: None,
            source_after_sequence: 41,
            source_through_sequence: 44,
            source_sequences: vec![44, 42, 44, 43],
            built_at: Utc
                .with_ymd_and_hms(2026, 9, 2, 12, 0, 0)
                .single()
                .expect("fixed timestamp should be valid"),
        }
    }

    #[test]
    fn model_context_preserves_user_content_and_keeps_receipts_local() {
        use driver_api::{ContextDelivery, ContextDeliveryKind};
        let mut package =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .unwrap()
                .package;
        package.conversation_text =
            "[User]\n保留原文、换行和来源\n[Assistant: test]\n已经完成。\n".into();
        package.delivery_facts = (1..=202)
            .map(|seq| crate::history_delivery::fact(seq, "text", "已经完成。", 0, 15))
            .collect();
        let original = serde_json::to_vec(&package).unwrap();
        let mut delivery = ContextDelivery {
            artifact_id: agent_domain::ArtifactId::new(),
            kind: ContextDeliveryKind::Handoff,
            payload_sha256: hex::encode(Sha256::digest(&original)),
            content: original.clone(),
        };
        let facts = prepare_model_delivery(&mut delivery).unwrap();
        let model: serde_json::Value = serde_json::from_slice(&delivery.content).unwrap();
        assert_eq!(facts, package.delivery_facts);
        assert_eq!(model["conversationText"], package.conversation_text);
        for key in [
            "objective",
            "userConstraints",
            "acceptanceCriteria",
            "completedWork",
            "decisions",
            "pendingWork",
            "relevantFiles",
            "tests",
            "changeSummary",
        ] {
            assert_eq!(
                model[key],
                serde_json::to_value(&package).unwrap()[key],
                "{key}"
            );
        }
        assert_eq!(
            model["contextPointers"][0]["locator"],
            package.context_pointers[0].locator
        );
        for key in [
            "deliveryFacts",
            "provenance",
            "recentTurnRefs",
            "dependencyChanges",
            "semanticSummary",
        ] {
            assert!(model.get(key).is_none(), "{key}");
        }
        assert!(model["sourceExecutor"].get("installationId").is_none());
        assert!(delivery.content.len() < original.len() / 4);
        assert_eq!(
            delivery.payload_sha256,
            hex::encode(Sha256::digest(&delivery.content))
        );
        assert_eq!(serde_json::to_vec(&package).unwrap(), original);
    }

    #[test]
    fn empty_captures_are_omitted_but_real_changes_and_unknown_states_survive() {
        use serde_json::json;
        assert!(empty_workspace_change(&json!({"files":[],"patch":""})));
        assert!(empty_workspace_change(
            &json!({"schemaVersion":1,"backend":"filesystem","files":[],"patch":"","patchHex":null,"workspacePaths":["/project"]})
        ));
        assert!(empty_workspace_change(
            &json!({"files":[],"patch":"","roots":[{"workspacePath":"/project","changes":{"patch":"","backend":"git"}}]})
        ));
        for change in [
            json!({"files":[{"path":"deleted.txt","status":"deleted"}],"patch":""}),
            json!({"files":[],"patch":"diff --git a/f b/f"}),
            json!({"files":[],"patch":"","patchHex":"ff"}),
            json!({"files":[],"patch":"","patchOmitted":"budget"}),
            json!({"files":[],"patch":"","diagnostic":"capture_failed"}),
            json!({"files":[],"patch":"","recoveredAfterInterruption":true}),
            json!({"files":[],"patch":"","roots":[{"workspacePath":"/other","changes":{"patch":"actual nested diff"}}]}),
            json!({"files":[]}),
            json!({"patch":""}),
            json!(null),
        ] {
            assert!(!empty_workspace_change(&change), "{change}");
        }
    }

    #[test]
    fn identical_facts_produce_an_identical_payload_hash() {
        let first =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .expect("package should build");
        let second =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .expect("package should build");

        assert_eq!(first, second);
        assert_eq!(first.package.provenance.source_sequences, [42, 43, 44]);
        assert_eq!(first.payload_sha256.len(), 64);
    }

    #[test]
    fn context_payload_requires_current_history_fields_even_when_empty() {
        let package =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .unwrap()
                .package;
        let current = serde_json::to_value(&package).unwrap();
        assert_eq!(current["conversationText"], "");
        assert!(serde_json::from_value::<ContextPackageV1>(current.clone()).is_ok());
        for field in ["conversationText", "deliveryFacts"] {
            let mut incomplete = current.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<ContextPackageV1>(incomplete).is_err());
        }
    }

    fn message_event(sequence: u64, kind: &str, payload: serde_json::Value) -> EventEnvelope {
        use agent_domain::{CorrelationId, EventId};
        use agent_protocol::{EventSource, StreamRef, StreamType};
        EventEnvelope {
            schema_version: 1,
            sequence,
            event_id: EventId::new(),
            stream: StreamRef {
                stream_type: StreamType::Task,
                stream_id: TaskId::new().to_string(),
            },
            stream_version: sequence,
            event_type: kind.into(),
            occurred_at: DateTime::<Utc>::UNIX_EPOCH,
            recorded_at: DateTime::<Utc>::UNIX_EPOCH,
            correlation_id: CorrelationId::new(),
            causation_id: None,
            source: EventSource::Driver {
                provider_id: "kimi".into(),
                source_sequence: None,
            },
            payload,
            vendor_extensions: None,
        }
    }

    #[test]
    fn history_joins_chunks_in_order_and_ignores_internal_events() {
        use serde_json::json;
        let events = vec![
            message_event(
                1,
                "turn.queued",
                json!({"turnId":"one", "prompt":"保留权限校验"}),
            ),
            message_event(2, "message.delta", json!({"turnId":"one", "text":"已经"})),
            message_event(
                3,
                "tool.completed",
                json!({"turnId":"one", "text":"internal body"}),
            ),
            message_event(4, "message.delta", json!({"turnId":"one", "text":"保留。"})),
        ];
        let text = conversation_text(&events, ContextStrength::Standard);
        assert!(text.contains("保留权限校验"));
        assert!(text.find("已经").unwrap() < text.find("保留。").unwrap());
        assert!(!text.contains("internal body"));
        let mut draft =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .unwrap()
                .package;
        draft.conversation_text = format!("{text}\napi_key=supersecret");
        let safe = rehash_edited_package(draft, &DeterministicRedactor).unwrap();
        assert!(!safe.package.conversation_text.contains("supersecret"));
    }

    #[test]
    fn history_budget_marks_omissions_and_never_splits_utf8() {
        use serde_json::json;
        let events = vec![
            message_event(
                1,
                "turn.queued",
                json!({"turnId":"old", "prompt":"older text"}),
            ),
            message_event(
                2,
                "turn.queued",
                json!({"turnId":"new", "prompt":"你好".repeat(10_000)}),
            ),
        ];
        let text = conversation_text(&events, ContextStrength::Compact);
        assert!(text.contains("1 of 2 turns"));
        assert!(text.contains("truncated"));
        assert!(!text.contains("older text"));
        assert!(text.len() < 17_000);
    }

    #[test]
    fn missing_facts_stay_explicit_and_do_not_trigger_semantic_work() {
        let mut draft = input(ContextPackageKind::Handoff);
        draft.objective = "  ".into();
        draft.completed_work.clear();
        draft.semantic_summary = None;

        let built =
            build_context_package(draft, &DeterministicRedactor).expect("package should build");
        assert_eq!(built.package.objective, "unknown");
        assert!(built.package.completed_work.is_empty());
        assert!(built.package.semantic_summary.is_none());
    }

    #[test]
    fn editing_creates_a_new_hash_without_mutating_the_original() {
        let original =
            build_context_package(input(ContextPackageKind::Handoff), &DeterministicRedactor)
                .expect("package should build");
        let mut edited_package = original.package.clone();
        edited_package
            .pending_work
            .push("Run desktop smoke with Authorization: Bearer abcdefghijklmnop".into());
        edited_package
            .pending_work
            .push("Check Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE".into());
        let edited = rehash_edited_package(edited_package, &DeterministicRedactor)
            .expect("edited package should hash");

        assert_ne!(original.payload_sha256, edited.payload_sha256);
        assert_eq!(original.package.pending_work, ["Preview in UI"]);
        assert_eq!(edited.package.pending_work.len(), 3);
        assert!(!edited.package.pending_work[1].contains("abcdefghijklmnop"));
        assert!(!edited.package.pending_work[2].contains("Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE"));
        assert_eq!(edited.package.provenance.redaction_count, 2);
        assert_eq!(
            rehash_edited_package(edited.package.clone(), &DeterministicRedactor).unwrap(),
            edited
        );
    }

    #[test]
    fn entropy_redaction_preserves_only_valid_frozen_history_references() {
        let mut draft = input(ContextPackageKind::Handoff);
        let task_id = TaskId::new();
        let pointer = &mut draft.context_pointers[0];
        pointer.id = format!("history:{task_id}:41-44");
        pointer.kind = "canonical_history".into();
        pointer.locator = format!("task/{task_id}/events?after=41&through=44");
        pointer.content_hash = "0123456789abcdef".repeat(4);
        let original_pointer = pointer.clone();
        draft.objective = "Review Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE".into();
        let built = build_context_package(draft, &DeterministicRedactor).unwrap();
        assert_eq!(built.package.context_pointers[0], original_pointer);
        assert_eq!(built.package.provenance.redaction_ruleset_version, 2);
        assert_eq!(built.package.provenance.redaction_count, 1);
        let again = rehash_edited_package(built.package.clone(), &DeterministicRedactor).unwrap();
        assert_eq!(again, built);
        let mut edited = built.package.clone();
        edited.context_pointers[0].locator = "https://fake:password@example.invalid/".into();
        let safe = rehash_edited_package(edited, &DeterministicRedactor).unwrap();
        assert!(
            !safe.package.context_pointers[0]
                .locator
                .contains("fake:password")
        );
        assert_ne!(safe.payload_sha256, built.payload_sha256);
    }

    #[test]
    fn reconciliation_accepts_an_empty_but_frozen_increment() {
        let mut draft = input(ContextPackageKind::Reconciliation);
        draft.source_sequences.clear();
        let built = build_context_package(draft, &DeterministicRedactor)
            .expect("empty increment should be explicit");
        assert!(built.package.provenance.source_sequences.is_empty());
        assert_eq!(built.package.provenance.source_after_sequence, 41);
        assert_eq!(built.package.provenance.source_through_sequence, 44);
    }

    #[test]
    fn selected_sequences_must_fit_the_frozen_range() {
        let mut draft = input(ContextPackageKind::Reconciliation);
        draft.source_sequences.push(45);
        assert_eq!(
            build_context_package(draft, &DeterministicRedactor),
            Err(ContextBuildError::InvalidSourceRange)
        );
    }
    #[test]
    fn delivery_spans_do_not_acknowledge_identical_omitted_text_or_truncated_suffixes() {
        use serde_json::json;
        let events = vec![
            message_event(
                1,
                "turn.queued",
                json!({"turnId":"old","prompt":"duplicate"}),
            ),
            message_event(
                2,
                "turn.queued",
                json!({"turnId":"new","prompt":format!("duplicate{}", "中".repeat(20_000))}),
            ),
        ];
        let (_, facts) = conversation_with_facts(&events, ContextStrength::Compact);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].sequence, 2);
        assert!(facts[0].end < facts[0].total_bytes);
        assert!(
            events[1].payload["prompt"]
                .as_str()
                .unwrap()
                .is_char_boundary(facts[0].end)
        );
    }
}
