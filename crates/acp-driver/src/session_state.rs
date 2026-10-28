//! Per-connection, per-session metadata. Restore responses must not overwrite
//! config notifications received while the request was in flight.
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(super) struct SessionStates {
    values: HashMap<String, Value>,
    pending: HashMap<String, (u64, Value)>,
    revision: u64,
    bindings: HashMap<String, Value>,
    retired: HashSet<String>,
}
pub(super) fn merge(base: &Value, update: &Value) -> Value {
    let mut result = base.as_object().cloned().unwrap_or_default();
    if let Some(update) = update.as_object() {
        for (k, v) in update {
            let value = match result.get(k) {
                Some(old) if old.is_object() && v.is_object() => merge(old, v),
                _ => v.clone(),
            };
            result.insert(k.clone(), value);
        }
    }
    Value::Object(result)
}
fn binding(mut params: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.remove("sessionId");
    }
    params
}
impl SessionStates {
    pub fn forget(&mut self, id: &str) {
        self.values.remove(id);
        self.bindings.remove(id);
        self.pending.remove(id);
        self.retired.insert(id.into());
    }
    /// Only sessions fully initialized on this connection may skip session/load.
    pub fn resident(&self, id: &str, params: &Value) -> Option<Value> {
        if self.pending.contains_key(id) || self.bindings.get(id) != Some(&binding(params.clone()))
        {
            return None;
        }
        self.values.get(id).cloned()
    }
    pub fn finish_setup(
        &mut self,
        id: &str,
        revision: u64,
        response: Value,
        params: Value,
    ) -> Option<Value> {
        let result = self.finish(id, revision, response)?;
        self.bindings.insert(id.into(), binding(params));
        Some(result)
    }
    pub fn invalidate(&mut self, id: &str) {
        self.bindings.remove(id);
        // A late setup response cannot make a faulted native session resident again.
        self.pending.remove(id);
    }

    pub fn begin(&mut self, id: &str) -> u64 {
        self.retired.remove(id);
        self.bindings.remove(id);
        self.revision += 1;
        self.pending.insert(id.into(), (self.revision, json!({})));
        self.revision
    }
    pub fn finish(&mut self, id: &str, revision: u64, response: Value) -> Option<Value> {
        let (current, updates) = self.pending.get(id)?;
        if *current != revision {
            return None;
        }
        let response = merge(
            &merge(self.values.get(id).unwrap_or(&Value::Null), &response),
            updates,
        );
        self.pending.remove(id);
        self.values.insert(id.into(), response.clone());
        Some(response)
    }
    pub fn fail(&mut self, id: &str, revision: u64) {
        if self.pending.get(id).is_some_and(|(v, _)| *v == revision) {
            self.pending.remove(id);
            // Keep the last committed configuration when restore/setup fails.
        }
    }
    pub fn new_session(&mut self, id: &str, response: Value) -> Value {
        self.retired.remove(id);
        let response = merge(&response, self.values.get(id).unwrap_or(&Value::Null));
        self.values.insert(id.into(), response.clone());
        response
    }
    pub fn observe(&mut self, event: &Value) {
        let Some(id) = event["sessionId"].as_str() else {
            return;
        };
        if self.retired.contains(id) {
            return;
        }
        let update = &event["update"];
        let patch = match update["sessionUpdate"].as_str() {
            Some("config_option_update") => json!({"configOptions":update["configOptions"]}),
            Some("current_mode_update") => {
                let mut modes = self
                    .values
                    .get(id)
                    .and_then(|v| v.get("modes"))
                    .cloned()
                    .unwrap_or(json!({}));
                modes["currentModeId"] = update["currentModeId"].clone();
                json!({"modes":modes})
            }
            _ => return,
        };
        if let Some((_, updates)) = self.pending.get_mut(id) {
            *updates = merge(updates, &patch);
            return;
        }
        let next = merge(self.values.get(id).unwrap_or(&Value::Null), &patch);
        self.values.insert(id.into(), next);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retirement_forgets_cached_bindings_and_rejects_late_metadata_until_explicit_setup() {
        let mut states = SessionStates::default();
        let params = json!({"cwd":"/work","mcpServers":[]});
        states.new_session("a", json!({"models":{"currentModelId":"before"}}));
        let original = states.begin("a");
        states
            .finish_setup("a", original, json!({}), params.clone())
            .unwrap();
        states.forget("a");
        states.observe(&json!({"sessionId":"a","update":{"sessionUpdate":"config_option_update","configOptions":["late"]}}));
        assert!(!states.values.contains_key("a"));
        assert!(states.resident("a", &params).is_none());
        assert!(
            states
                .finish_setup(
                    "a",
                    original,
                    json!({"models":{"currentModelId":"stale"}}),
                    params.clone()
                )
                .is_none()
        );
        let restored = states.begin("a");
        states
            .finish_setup(
                "a",
                restored,
                json!({"models":{"currentModelId":"restored"}}),
                params.clone(),
            )
            .unwrap();
        assert_eq!(
            states.resident("a", &params).unwrap()["models"]["currentModelId"],
            "restored"
        );
        assert!(
            states
                .resident("a", &params)
                .unwrap()
                .get("configOptions")
                .is_none()
        );
    }
    #[test]
    fn failed_session_cannot_be_revived_by_a_late_setup_reply() {
        let mut state = SessionStates::default();
        let params = json!({"cwd":"/work","mcpServers":[]});
        let old = state.begin("s");
        state.invalidate("s");
        assert!(
            state
                .finish_setup("s", old, json!({}), params.clone())
                .is_none()
        );
        assert!(state.resident("s", &params).is_none());
        let current = state.begin("s");
        state
            .finish_setup(
                "s",
                current,
                json!({"models":{"currentModelId":"confirmed"}}),
                params.clone(),
            )
            .unwrap();
        assert_eq!(
            state.resident("s", &params).unwrap()["models"]["currentModelId"],
            "confirmed"
        );
    }
    #[test]
    fn residency_requires_committed_matching_binding_on_this_connection() {
        let mut s = SessionStates::default();
        let params = json!({"cwd":"/one","additionalDirectories":["/two"],"mcpServers":[{"token":"a"}],"_meta":{"yoloMode":false}});
        let mut load = params.clone();
        load["sessionId"] = json!("a");
        s.new_session("a", json!({"models":{"currentModelId":"one"}}));
        assert!(s.resident("a", &load).is_none());
        let revision = s.begin("a");
        s.finish_setup("a", revision, json!({}), params.clone())
            .unwrap();
        assert!(s.resident("a", &load).is_some());
        assert!(s.resident("other", &load).is_none());
        assert!(SessionStates::default().resident("a", &load).is_none());
        for (key, value) in [
            ("cwd", json!("/other")),
            ("additionalDirectories", json!([])),
            ("mcpServers", json!([])),
            ("_meta", json!({"yoloMode":true})),
        ] {
            let mut changed = load.clone();
            changed[key] = value;
            assert!(s.resident("a", &changed).is_none());
        }
        s.observe(&json!({"sessionId":"a","update":{"sessionUpdate":"config_option_update","configOptions":["new"]}}));
        assert_eq!(
            s.resident("a", &load).unwrap()["configOptions"],
            json!(["new"])
        );
        let revision = s.begin("a");
        assert!(s.resident("a", &load).is_none());
        s.fail("a", revision);
        assert!(s.resident("a", &load).is_none());
        let obsolete = s.begin("a");
        let latest = s.begin("a");
        assert!(
            s.finish_setup("a", obsolete, json!({}), params.clone())
                .is_none()
        );
        s.finish_setup("a", latest, json!({}), params).unwrap();
        s.invalidate("a");
        assert!(s.resident("a", &load).is_none());
    }
    #[test]
    fn restore_keeps_newer_notifications_and_rejects_obsolete_response() {
        let mut s = SessionStates::default();
        s.new_session(
            "a",
            json!({"configOptions":["old"],"models":{"currentModelId":"x"}}),
        );
        let old = s.begin("a");
        let current = s.begin("a");
        s.observe(&json!({"sessionId":"a","update":{"sessionUpdate":"config_option_update","configOptions":["new"]}}));
        assert!(
            s.finish("a", old, json!({"configOptions":["stale"]}))
                .is_none()
        );
        let v = s
            .finish("a", current, json!({"configOptions":["response"]}))
            .unwrap();
        assert_eq!(v["configOptions"], json!(["new"]));
        assert_eq!(v["models"]["currentModelId"], "x");
        let b = s.begin("b");
        assert!(
            s.finish("b", b, json!({}))
                .unwrap()
                .get("configOptions")
                .is_none()
        );
    }
    #[test]
    fn early_mode_notification_keeps_available_modes() {
        let mut s = SessionStates::default();
        s.observe(&json!({"sessionId":"a","update":{"sessionUpdate":"current_mode_update","currentModeId":"plan"}}));
        let v=s.new_session("a",json!({"modes":{"currentModeId":"agent","availableModes":[{"id":"agent"},{"id":"plan"}]}}));
        assert_eq!(v["modes"]["currentModeId"], "plan");
        assert_eq!(v["modes"]["availableModes"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn explicit_empty_config_clears_but_omission_preserves() {
        let mut s = SessionStates::default();
        s.new_session("a", json!({"configOptions":[1]}));
        let n = s.begin("a");
        assert_eq!(
            s.finish("a", n, json!({})).unwrap()["configOptions"],
            json!([1])
        );
        let n = s.begin("a");
        assert_eq!(
            s.finish("a", n, json!({"configOptions":[]})).unwrap()["configOptions"],
            json!([])
        );
        let n = s.begin("a");
        s.fail("a", n);
        assert_eq!(s.values["a"]["configOptions"], json!([]));
        let n = s.begin("a");
        s.observe(&json!({"sessionId":"a","update":{"sessionUpdate":"config_option_update","configOptions":["uncommitted"]}}));
        s.fail("a", n);
        assert_eq!(s.values["a"]["configOptions"], json!([]));
    }
}
