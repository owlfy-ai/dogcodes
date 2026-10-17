//! Code-owned, schema/path-specific policy for diagnostic copies, never live protocol frames.

use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{DeterministicRedactor, REDACTION_RULESET_VERSION, SecretRedactor};

pub const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 4096;
const MAX_ARRAY_ITEMS: usize = 128;
const MAX_STRING_BYTES: usize = 4096;
const MAX_KEY_BYTES: usize = 256;

/// Policies are declared in reviewed code, not loaded from the frame or supplied by the UI.
pub enum FieldPolicy {
    Object(&'static [(&'static str, FieldPolicy)]),
    Array(&'static FieldPolicy),
    Boolean,
    Integer { min: i64, max: i64 },
    Enum(&'static [&'static str]),
    RedactedText,
    Mask,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizationAudit {
    pub dropped_fields: u32,
    pub sensitive_fields: u32,
    pub masked_values: u32,
    pub type_mismatches: u32,
    pub text_findings: u32,
}

/// No Deserialize or public constructor: raw Values cannot masquerade as sanitized output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedDocument {
    redaction_ruleset_version: u16,
    audit: SanitizationAudit,
    payload: Value,
}

impl SanitizedDocument {
    #[must_use]
    pub fn payload(&self) -> &Value {
        &self.payload
    }

    #[must_use]
    pub fn audit(&self) -> &SanitizationAudit {
        &self.audit
    }

    /// Re-apply the same structural policy and text redactor before a persistence boundary.
    pub fn verify_again(&self, policy: &FieldPolicy) -> Result<(), StructuredRedactionError> {
        let bytes =
            serde_json::to_vec(&self.payload).map_err(|_| StructuredRedactionError::InvalidJson)?;
        let second = sanitize_json(&bytes, policy)?;
        if second.payload != self.payload || second.audit.text_findings != 0 {
            return Err(StructuredRedactionError::UnstableSanitization);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StructuredRedactionError {
    #[error("diagnostic frame is not valid JSON")]
    InvalidJson,
    #[error("diagnostic frame exceeds a structural budget")]
    BudgetExceeded,
    #[error("diagnostic frame root does not match the selected policy")]
    RootMismatch,
    #[error("diagnostic frame did not pass secondary sanitization")]
    UnstableSanitization,
}

pub fn sanitize_json(
    input: &[u8],
    policy: &FieldPolicy,
) -> Result<SanitizedDocument, StructuredRedactionError> {
    if input.len() > MAX_FRAME_BYTES {
        return Err(StructuredRedactionError::BudgetExceeded);
    }
    // serde_json's default recursion limit remains enabled. Its parse errors never escape.
    let value: Value =
        serde_json::from_slice(input).map_err(|_| StructuredRedactionError::InvalidJson)?;
    let mut nodes = 0;
    check_budget(&value, 0, &mut nodes)?;
    let mut audit = SanitizationAudit::default();
    let payload =
        project(&value, policy, &mut audit).ok_or(StructuredRedactionError::RootMismatch)?;
    // Replacements can grow short strings. Output must obey the same structural limits.
    check_budget(&payload, 0, &mut 0)?;
    if serde_json::to_vec(&payload)
        .map_err(|_| StructuredRedactionError::InvalidJson)?
        .len()
        > MAX_FRAME_BYTES
    {
        return Err(StructuredRedactionError::BudgetExceeded);
    }
    Ok(SanitizedDocument {
        redaction_ruleset_version: REDACTION_RULESET_VERSION,
        audit,
        payload,
    })
}

fn check_budget(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), StructuredRedactionError> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err(StructuredRedactionError::BudgetExceeded);
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key.len() > MAX_KEY_BYTES {
                    return Err(StructuredRedactionError::BudgetExceeded);
                }
                check_budget(value, depth + 1, nodes)?;
            }
        }
        Value::Array(array) => {
            if array.len() > MAX_ARRAY_ITEMS {
                return Err(StructuredRedactionError::BudgetExceeded);
            }
            for value in array {
                check_budget(value, depth + 1, nodes)?;
            }
        }
        Value::String(text) if text.len() > MAX_STRING_BYTES => {
            return Err(StructuredRedactionError::BudgetExceeded);
        }
        _ => (),
    }
    Ok(())
}

fn project(value: &Value, policy: &FieldPolicy, audit: &mut SanitizationAudit) -> Option<Value> {
    let output = match policy {
        FieldPolicy::Object(fields) => {
            if let Some(object) = value.as_object() {
                let mut output = Map::new();
                for (name, value) in object {
                    // Decoded JSON field names are checked before any allowlist entry, for all types.
                    if sensitive_field(name) {
                        audit.sensitive_fields += 1;
                    } else if let Some((_, field_policy)) =
                        fields.iter().find(|(key, _)| *key == name)
                    {
                        if let Some(safe) = project(value, field_policy, audit) {
                            output.insert(name.clone(), safe);
                        }
                    } else {
                        audit.dropped_fields += 1;
                    }
                }
                Some(Value::Object(output))
            } else {
                None
            }
        }
        FieldPolicy::Array(element) => value.as_array().map(|items| {
            Value::Array(
                items
                    .iter()
                    .map(|item| project(item, element, audit).unwrap_or(Value::Null))
                    .collect(),
            )
        }),
        FieldPolicy::Boolean => value.as_bool().map(Value::Bool),
        FieldPolicy::Integer { min, max } => value
            .as_i64()
            .filter(|number| (*min..=*max).contains(number))
            .map(Value::from),
        FieldPolicy::Enum(allowed) => value
            .as_str()
            .filter(|text| allowed.contains(text))
            .map(|text| Value::String(text.into())),
        FieldPolicy::RedactedText => value.as_str().map(|text| {
            let result = DeterministicRedactor.redact(text);
            audit.text_findings += result.finding_count();
            Value::String(result.text)
        }),
        FieldPolicy::Mask => {
            audit.masked_values += 1;
            Some(if value.is_string() {
                Value::String("[REDACTED_FIELD]".into())
            } else {
                Value::Null
            })
        }
    };
    if output.is_none() {
        audit.type_mismatches += 1;
    }
    output
}

fn sensitive_field(name: &str) -> bool {
    let canonical: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    matches!(
        canonical.as_str(),
        "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "token"
            | "password"
            | "passwd"
            | "secret"
            | "clientsecret"
            | "privatekey"
            | "authorization"
            | "proxyauthorization"
            | "cookie"
            | "setcookie"
            | "credentials"
            | "environment"
            | "env"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static ITEM: FieldPolicy = FieldPolicy::Object(&[
        ("ok", FieldPolicy::Boolean),
        ("note", FieldPolicy::RedactedText),
    ]);
    static POLICY: FieldPolicy = FieldPolicy::Object(&[
        ("items", FieldPolicy::Array(&ITEM)),
        ("status", FieldPolicy::Enum(&["completed", "failed"])),
        ("count", FieldPolicy::Integer { min: 0, max: 100 }),
        ("id", FieldPolicy::Mask),
        ("token", FieldPolicy::RedactedText), // Even an accidentally allowed secret field is denied.
    ]);

    #[test]
    fn projects_exact_paths_and_never_persists_unknown_keys_or_values() {
        let input = json!({"status":"completed","count":2,"id":"native-secret-id", "unknown-secret-name":"tiny",
            "items":[{"ok":true,"count":44,"note":"Bearer synthetic"}]});
        let original = input.clone();
        let safe = sanitize_json(&serde_json::to_vec(&input).unwrap(), &POLICY).unwrap();
        assert_eq!(
            safe.payload(),
            &json!({"status":"completed","count":2,"id":"[REDACTED_FIELD]", "items":[{"ok":true,"note":"Bearer [REDACTED]"}]})
        );
        assert_eq!(safe.audit().dropped_fields, 2);
        assert_eq!(input, original);
        assert!(
            !serde_json::to_string(&safe)
                .unwrap()
                .contains("secret-name")
        );
        safe.verify_again(&POLICY).unwrap();
    }

    #[test]
    fn decoded_sensitive_names_are_removed_before_policy_regardless_of_value_type() {
        let safe = sanitize_json(
            br#"{"\u0074oken":"tiny","PASSWORD":123,"cookie":false,"env":{"x":1},"secret":[1,2]}"#,
            &POLICY,
        )
        .unwrap();
        assert_eq!(safe.payload(), &json!({}));
        assert_eq!(safe.audit().sensitive_fields, 5);
    }

    #[test]
    fn rejects_type_confusion_and_out_of_range_values_without_echoing_them() {
        let safe = sanitize_json(
            br#"{"status":"user@example.com","count":101,"items":[false,{"ok":"true"}],"id":42}"#,
            &POLICY,
        )
        .unwrap();
        assert_eq!(safe.payload(), &json!({"items":[null,{}],"id":null}));
        assert_eq!(safe.audit().type_mismatches, 4);
        assert_eq!(
            sanitize_json(b"[]", &POLICY).unwrap_err(),
            StructuredRedactionError::RootMismatch
        );
    }

    #[test]
    fn checks_budgets_even_inside_dropped_fields() {
        let mut deep = json!(null);
        for _ in 0..18 {
            deep = json!({"unknown":deep});
        }
        for input in [
            deep,
            json!({"unknown":vec![0;129]}),
            json!({"unknown":"a".repeat(4097)}),
            json!({"unknown":(0..128).map(|_| vec![0;33]).collect::<Vec<_>>()}),
        ] {
            assert_eq!(
                sanitize_json(&serde_json::to_vec(&input).unwrap(), &POLICY).unwrap_err(),
                StructuredRedactionError::BudgetExceeded
            );
        }
        assert_eq!(
            sanitize_json(&vec![b' '; MAX_FRAME_BYTES + 1], &POLICY).unwrap_err(),
            StructuredRedactionError::BudgetExceeded
        );
    }

    #[test]
    fn parse_errors_do_not_include_payload_fragments() {
        let error = sanitize_json(b"{\"password\":\"do-not-print", &POLICY).unwrap_err();
        assert_eq!(error, StructuredRedactionError::InvalidJson);
        assert!(!error.to_string().contains("do-not-print"));
    }

    #[test]
    fn redaction_expansion_cannot_bypass_output_budgets() {
        let value = json!({"items":[{"note":"Bearer x ".repeat(440)}]});
        assert_eq!(
            sanitize_json(&serde_json::to_vec(&value).unwrap(), &POLICY).unwrap_err(),
            StructuredRedactionError::BudgetExceeded
        );
    }

    #[test]
    fn text_fields_use_entropy_rules_after_structural_projection() {
        let safe = sanitize_json(br#"{"items":[{"note":"candidate Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE","unknown":"low entropy private"}]}"#, &POLICY).unwrap();
        assert_eq!(safe.audit().text_findings, 1);
        assert!(!serde_json::to_string(&safe).unwrap().contains("Ab3dEf"));
        safe.verify_again(&POLICY).unwrap();
    }
}
