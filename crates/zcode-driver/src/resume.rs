//! Validate native identity and ownership before admitting a restored session.
use driver_api::DriverError;
use serde_json::Value;
use std::path::Path;

pub(super) fn validate(snapshot: &Value, id: &str, workspace: &str) -> Result<(), DriverError> {
    if super::session_id_from_snapshot(snapshot) != Some(id) {
        return Err(DriverError::Protocol(
            "ZCode resumed a different native session".into(),
        ));
    }
    let native = snapshot
        .pointer("/session/workspace/workspacePath")
        .and_then(Value::as_str)
        .ok_or_else(|| DriverError::Protocol("ZCode resumed session has no workspace".into()))?;
    let same = Path::new(native)
        .canonicalize()
        .ok()
        .zip(Path::new(workspace).canonicalize().ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        return Err(DriverError::Protocol(
            "ZCode resumed session belongs to a different workspace".into(),
        ));
    }
    if snapshot.pointer("/session/status").and_then(Value::as_str) == Some("running")
        || snapshot
            .pointer("/projection/currentTurnId")
            .is_some_and(|v| !v.is_null())
    {
        return Err(DriverError::CapabilityUnavailable(
            "ZCode session is still executing native work".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn restore_requires_same_identity_workspace_and_idle_native_state() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let mut state = json!({"session":{"sessionId":"s","workspace":{"workspacePath":path},"status":"idle"},"projection":{"currentTurnId":null}});
        validate(&state, "s", path).unwrap();
        assert!(validate(&state, "other", path).is_err());
        assert!(validate(&state, "s", other.path().to_str().unwrap()).is_err());
        state["projection"]["currentTurnId"] = json!("active");
        assert!(matches!(
            validate(&state, "s", path),
            Err(DriverError::CapabilityUnavailable(_))
        ));
        state["projection"]["currentTurnId"] = Value::Null;
        state["session"]["status"] = json!("running");
        assert!(validate(&state, "s", path).is_err());
    }
}
