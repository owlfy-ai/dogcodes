//! Immutable workspace bodies travel separately from bounded snapshot manifests.
use crate::ArtifactMetadata;
use serde_json::Value;
use std::{collections::BTreeMap, fmt::Write, future::Future};

pub const WORKSPACE_CONTENT_TYPE: &str = "application/vnd.dogcodes.workspace-content";
pub const MAX_WORKSPACE_FILE_BYTES: usize = 512 * 1024;
pub const MAX_WORKSPACE_CONTENT_BYTES: usize = 16 * 1024 * 1024;

pub fn content_references(value: &Value) -> Result<Vec<(String, ArtifactMetadata)>, String> {
    fn visit(
        value: &Value,
        path: &str,
        refs: &mut Vec<(String, ArtifactMetadata)>,
    ) -> Result<(), String> {
        match value {
            Value::Object(map) => {
                if let Some(reference) = map.get("contentArtifact") {
                    let metadata: ArtifactMetadata =
                        serde_json::from_value(reference.clone()).map_err(|e| e.to_string())?;
                    if metadata.media_type != WORKSPACE_CONTENT_TYPE
                        || metadata.byte_length > MAX_WORKSPACE_FILE_BYTES as u64
                        || value["sha256"].as_str() != Some(metadata.payload_sha256.as_str())
                    {
                        return Err("workspace content reference does not match its file".into());
                    }
                    refs.push((path.into(), metadata));
                } else {
                    for (key, child) in map {
                        visit(
                            child,
                            &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                            refs,
                        )?;
                    }
                }
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    visit(child, &format!("{path}/{index}"), refs)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    let mut refs = Vec::new();
    visit(value, "", &mut refs)?;
    Ok(refs)
}

/// The loader must verify the artifact's task ownership, length and digest.
pub async fn hydrate_workspace_contents<F, Fut>(
    value: &mut Value,
    mut load: F,
) -> Result<(), String>
where
    F: FnMut(ArtifactMetadata) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let mut bodies = BTreeMap::new();
    let mut remaining = 2 * MAX_WORKSPACE_CONTENT_BYTES;
    for (path, metadata) in content_references(value)? {
        let digest = metadata.payload_sha256.clone();
        if !bodies.contains_key(&digest) {
            remaining = remaining
                .checked_sub(usize::try_from(metadata.byte_length).map_err(|e| e.to_string())?)
                .ok_or("workspace content exceeds hydration budget")?;
            let length = metadata.byte_length;
            let bytes = load(metadata).await?;
            if bytes.len() as u64 != length {
                return Err("workspace content length mismatch".into());
            }
            let mut encoded = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                write!(encoded, "{byte:02x}").map_err(|e| e.to_string())?;
            }
            bodies.insert(digest.clone(), encoded);
        }
        let file = value
            .pointer_mut(&path)
            .and_then(Value::as_object_mut)
            .ok_or("workspace content path disappeared")?;
        file.remove("contentArtifact");
        file.insert("hex".into(), Value::String(bodies[&digest].clone()));
    }
    Ok(())
}
