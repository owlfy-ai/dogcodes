//! Explicit offline diagnostic export. No app-server process or live-frame mutation.

use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

use redaction::structured::{
    FieldPolicy, MAX_FRAME_BYTES, SanitizedDocument, StructuredRedactionError, sanitize_json,
};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_FRAMES: usize = 256;
const KNOWN_METHODS: &[&str] = &[
    "turn/started",
    "turn/completed",
    "item/agentMessage/delta",
    "turn/diff/updated",
    "thread/tokenUsage/updated",
    "thread/started",
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
    "configWarning",
    "warning",
    "error",
];

static TURN_POLICY: FieldPolicy = FieldPolicy::Object(&[
    ("jsonrpc", FieldPolicy::Enum(&["2.0"])),
    ("id", FieldPolicy::Mask),
    (
        "method",
        FieldPolicy::Enum(&["turn/started", "turn/completed"]),
    ),
    (
        "params",
        FieldPolicy::Object(&[
            ("threadId", FieldPolicy::Mask),
            (
                "turn",
                FieldPolicy::Object(&[
                    ("id", FieldPolicy::Mask),
                    (
                        "status",
                        FieldPolicy::Enum(&["inProgress", "completed", "interrupted", "failed"]),
                    ),
                    ("items", FieldPolicy::Mask),
                    ("error", FieldPolicy::Mask),
                ]),
            ),
        ]),
    ),
]);

// Responses lack the request method. Do not infer a result schema from its field names.
static ENVELOPE_POLICY: FieldPolicy = FieldPolicy::Object(&[
    ("jsonrpc", FieldPolicy::Enum(&["2.0"])),
    ("id", FieldPolicy::Mask),
    ("method", FieldPolicy::Enum(KNOWN_METHODS)),
    ("params", FieldPolicy::Mask),
    ("result", FieldPolicy::Mask),
    (
        "error",
        FieldPolicy::Object(&[
            (
                "code",
                FieldPolicy::Integer {
                    min: -32768,
                    max: 32767,
                },
            ),
            ("message", FieldPolicy::Mask),
            ("data", FieldPolicy::Mask),
        ]),
    ),
]);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFrame {
    schema_version: u16,
    policy_id: &'static str,
    parser_version: &'static str,
    driver_id: &'static str,
    driver_version: &'static str,
    source_kind: &'static str,
    native_version: Option<&'static str>,
    ordinal: usize,
    recognized_method: bool,
    diagnostic: SanitizedDocument,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureExportReport {
    kind: &'static str,
    frame_count: usize,
    output_bytes: usize,
    policy_id: &'static str,
}

#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("fixture paths must be absolute regular-input/new-output paths")]
    InvalidPath,
    #[error("fixture input exceeds its byte or frame budget")]
    InputBudget,
    #[error("fixture output exceeds its byte budget")]
    OutputBudget,
    #[error("fixture contains no frames")]
    Empty,
    #[error("cannot read fixture input")]
    Read,
    #[error("cannot publish fixture; choose a new file in a writable directory")]
    Publish,
    #[error("fixture is not canonical sanitized output for this policy")]
    UnsafeFixture,
    #[error(transparent)]
    Sanitization(#[from] StructuredRedactionError),
}

pub fn sanitize_codex_frame(
    input: &[u8],
    ordinal: usize,
) -> Result<DiagnosticFrame, StructuredRedactionError> {
    if input.len() > MAX_FRAME_BYTES {
        return Err(StructuredRedactionError::BudgetExceeded);
    }
    let raw: Value =
        serde_json::from_slice(input).map_err(|_| StructuredRedactionError::InvalidJson)?;
    let method = raw.get("method").and_then(Value::as_str);
    let policy = if matches!(method, Some("turn/started" | "turn/completed")) {
        &TURN_POLICY
    } else {
        &ENVELOPE_POLICY
    };
    let diagnostic = sanitize_json(input, policy)?;
    diagnostic.verify_again(policy)?;
    Ok(DiagnosticFrame {
        schema_version: 1,
        policy_id: "codex-m0-diagnostic-v1",
        parser_version: env!("CARGO_PKG_VERSION"),
        driver_id: "codex-app-server-m0",
        driver_version: env!("CARGO_PKG_VERSION"),
        source_kind: "offline_import",
        native_version: None,
        ordinal,
        recognized_method: method.is_some_and(|value| KNOWN_METHODS.contains(&value)),
        diagnostic,
    })
}

/// Validate the entire bounded import before creating any output or temporary file.
pub fn export_fixture(input: &Path, output: &Path) -> Result<FixtureExportReport, FixtureError> {
    if !input.is_absolute()
        || !output.is_absolute()
        || !std::fs::symlink_metadata(input)
            .map_err(|_| FixtureError::InvalidPath)?
            .file_type()
            .is_file()
        || std::fs::symlink_metadata(output).is_ok()
    {
        return Err(FixtureError::InvalidPath);
    }
    let mut raw = Vec::new();
    File::open(input)
        .map_err(|_| FixtureError::Read)?
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| FixtureError::Read)?;
    if raw.len() > MAX_INPUT_BYTES {
        return Err(FixtureError::InputBudget);
    }
    let mut safe_bytes = Vec::new();
    let mut frame_count = 0;
    for line in raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        if frame_count == MAX_FRAMES {
            return Err(FixtureError::InputBudget);
        }
        let frame = sanitize_codex_frame(line, frame_count + 1)?;
        // Canonical object ordering lets the checker reject duplicate-key/hidden-byte tricks.
        let canonical = serde_json::to_value(&frame).map_err(|_| FixtureError::OutputBudget)?;
        serde_json::to_writer(&mut safe_bytes, &canonical)
            .map_err(|_| FixtureError::OutputBudget)?;
        safe_bytes.push(b'\n');
        if safe_bytes.len() > MAX_OUTPUT_BYTES {
            return Err(FixtureError::OutputBudget);
        }
        frame_count += 1;
    }
    if frame_count == 0 {
        return Err(FixtureError::Empty);
    }
    let parent = output.parent().ok_or(FixtureError::InvalidPath)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| FixtureError::Publish)?;
    temporary
        .write_all(&safe_bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| FixtureError::Publish)?;
    temporary
        .persist_noclobber(output)
        .map_err(|_| FixtureError::Publish)?;
    Ok(FixtureExportReport {
        kind: "sanitized-fixture",
        frame_count,
        output_bytes: safe_bytes.len(),
        policy_id: "codex-m0-diagnostic-v1",
    })
}

/// Read-only secondary scan suitable for CI. Only this version's canonical export format passes.
pub fn check_fixture(input: &Path) -> Result<FixtureExportReport, FixtureError> {
    if !input.is_absolute()
        || !std::fs::symlink_metadata(input)
            .map_err(|_| FixtureError::InvalidPath)?
            .file_type()
            .is_file()
    {
        return Err(FixtureError::InvalidPath);
    }
    let mut bytes = Vec::new();
    File::open(input)
        .map_err(|_| FixtureError::Read)?
        .take((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| FixtureError::Read)?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(FixtureError::OutputBudget);
    }
    let mut frame_count = 0;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if frame_count == MAX_FRAMES || line.len() > MAX_FRAME_BYTES + 1024 {
            return Err(FixtureError::InputBudget);
        }
        let record: Value =
            serde_json::from_slice(line).map_err(|_| FixtureError::UnsafeFixture)?;
        if serde_json::to_vec(&record).map_err(|_| FixtureError::UnsafeFixture)? != line {
            return Err(FixtureError::UnsafeFixture);
        }
        let payload = record
            .get("diagnostic")
            .and_then(|value| value.get("payload"))
            .ok_or(FixtureError::UnsafeFixture)?;
        let audit = record
            .get("diagnostic")
            .and_then(|value| value.get("audit"))
            .and_then(Value::as_object)
            .ok_or(FixtureError::UnsafeFixture)?;
        let audit_names = [
            "droppedFields",
            "sensitiveFields",
            "maskedValues",
            "typeMismatches",
            "textFindings",
        ];
        if audit.len() != audit_names.len()
            || !audit_names.iter().all(|name| {
                audit
                    .get(*name)
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count <= MAX_FRAME_BYTES as u64)
            })
        {
            return Err(FixtureError::UnsafeFixture);
        }
        let raw_payload = serde_json::to_vec(payload).map_err(|_| FixtureError::UnsafeFixture)?;
        let fresh = sanitize_codex_frame(&raw_payload, frame_count + 1)?;
        let mut expected = serde_json::to_value(fresh).map_err(|_| FixtureError::UnsafeFixture)?;
        // Source audit counts cannot be reconstructed after dropping private data. Validate shape/range,
        // but never treat them as proof of the original frame or as Canonical business facts.
        expected["diagnostic"]["audit"] = Value::Object(audit.clone());
        if expected != record {
            return Err(FixtureError::UnsafeFixture);
        }
        frame_count += 1;
    }
    if frame_count == 0 {
        return Err(FixtureError::Empty);
    }
    Ok(FixtureExportReport {
        kind: "checked-fixture",
        frame_count,
        output_bytes: bytes.len(),
        policy_id: "codex-m0-diagnostic-v1",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::classify_frame;
    use serde_json::json;

    #[test]
    fn preserves_reviewed_status_but_drops_ids_payloads_and_unknown_method_text() {
        let raw = json!({"method":"turn/completed","params":{"threadId":"SECRET_SENTINEL", "turn":{"id":"SECRET_SENTINEL","status":"completed","items":[{"text":"SECRET_SENTINEL"}],"future":"SECRET_SENTINEL"}}});
        let live = classify_frame(raw.clone()).unwrap();
        let safe = sanitize_codex_frame(&serde_json::to_vec(live.raw()).unwrap(), 1).unwrap();
        let serialized = serde_json::to_string(&safe).unwrap();
        assert!(!serialized.contains("SECRET_SENTINEL"));
        assert!(!serialized.contains("future"));
        assert_eq!(
            safe.diagnostic.payload()["params"]["turn"]["status"],
            "completed"
        );
        assert_eq!(live.raw(), &raw);
        let unknown = sanitize_codex_frame(
            br#"{"method":"SECRET_SENTINEL","params":{"status":"completed"}}"#,
            2,
        )
        .unwrap();
        assert!(!unknown.recognized_method);
        assert!(
            !serde_json::to_string(&unknown)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        assert_eq!(unknown.native_version, None);
    }

    #[test]
    fn approval_ids_remain_usable_in_memory_but_not_in_export() {
        let live = classify_frame(json!({"id":"native-approval-secret","method":"item/commandExecution/requestApproval","params":{"command":"private command","token":123}})).unwrap();
        let safe = sanitize_codex_frame(&serde_json::to_vec(live.raw()).unwrap(), 1).unwrap();
        assert_eq!(live.raw()["id"], "native-approval-secret");
        let output = serde_json::to_string(&safe).unwrap();
        assert!(!output.contains("native-approval-secret"));
        assert!(!output.contains("private command"));
        assert_eq!(safe.diagnostic.payload()["id"], "[REDACTED_FIELD]");
    }

    #[test]
    fn response_results_are_not_trusted_without_request_correlation() {
        let safe = sanitize_codex_frame(br#"{"id":1,"result":{"status":"SECRET_SENTINEL"},"error":{"code":-32600,"message":"SECRET_SENTINEL","data":{"token":"SECRET_SENTINEL"}}}"#, 1).unwrap();
        assert_eq!(safe.diagnostic.payload()["result"], Value::Null);
        assert_eq!(safe.diagnostic.payload()["error"]["code"], -32600);
        assert!(
            !serde_json::to_string(&safe)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
    }
}
