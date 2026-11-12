//! Immutable logical identities for Core commands, independent of delivery attempts.
use crate::CoreError;
use agent_protocol::{CommandAck, CommandEnvelope, CoreCommand};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map.into_iter().collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}

pub fn fingerprint(envelope: &CommandEnvelope<CoreCommand>) -> Result<String, CoreError> {
    // A retry may have a different delivery commandId/issuedAt. The logical key
    // remains the lookup key; actor and optimistic version are part of intent.
    let value = serde_json::json!({
        "schemaVersion": envelope.schema_version,
        "actor": envelope.actor,
        "expectedStreamVersion": envelope.expected_stream_version,
        "payload": envelope.payload,
    });
    let bytes = serde_json::to_vec(&canonical(value))
        .map_err(|error| CoreError::Internal(error.to_string()))?;
    Ok(format!("v1:{}", hex::encode(Sha256::digest(bytes))))
}

pub fn verify(ack: &CommandAck, fingerprint: &str) -> Result<(), CoreError> {
    match ack.request_fingerprint.as_deref() {
        Some(original) if original == fingerprint => Ok(()),
        Some(_) => Err(CoreError::Conflict("backendText.requestIdConflict".into())),
        None => Err(CoreError::Conflict("backendText.m1a98db4b".into())),
    }
}
