//! Turn-local, bounded correlation of file-change items with approval requests.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

pub(super) struct FileChanges {
    thread_id: String,
    turn_id: String,
    items: BTreeMap<String, Value>,
    working_directory: Option<PathBuf>,
}

impl FileChanges {
    pub(super) fn new(thread_id: String, turn_id: String) -> Self {
        Self {
            thread_id,
            turn_id,
            items: BTreeMap::new(),
            working_directory: None,
        }
    }

    pub(super) fn set_working_directory(&mut self, directory: PathBuf) {
        self.working_directory = Some(directory);
    }

    fn present_path(&self, value: &mut Value) {
        let Some(path) = value.as_str().map(Path::new) else {
            return;
        };
        let Some(base) = &self.working_directory else {
            return;
        };
        // Lexical containment only: do not follow a file symlink or erase '..'.
        // Canonicalizing the trusted cwd handles macOS /var vs /private/var.
        let canonical = base.canonicalize().ok();
        let relative = path.strip_prefix(base).ok().or_else(|| {
            canonical
                .as_ref()
                .and_then(|base| path.strip_prefix(base).ok())
        });
        if let Some(relative) = relative.filter(|path| {
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        }) {
            *value = json!({"relativeTo":"current_working_directory", "path":relative.to_string_lossy()});
        }
    }

    fn matches(&self, params: &Value) -> bool {
        params.get("threadId").and_then(Value::as_str) == Some(&self.thread_id)
            && params.get("turnId").and_then(Value::as_str) == Some(&self.turn_id)
    }

    pub(super) fn observe(&mut self, method: &str, params: &Value) {
        if !self.matches(params) || !matches!(method, "item/started" | "item/completed") {
            return;
        }
        let Some(item) = params.get("item") else {
            return;
        };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        // Never retain stale detail after a same-ID update we cannot understand.
        self.items.remove(id);
        if method == "item/completed"
            || item.get("type").and_then(Value::as_str) != Some("fileChange")
        {
            return;
        }
        let Some(changes) = item.get("changes").and_then(Value::as_array) else {
            return;
        };
        if changes.is_empty() || self.items.len() >= 16 || id.len() > 1024 {
            return;
        }
        let files: Option<Vec<Value>> = changes.iter().map(|change| {
            let path = change.get("path")?.as_str().filter(|path| !path.is_empty())?;
            let diff = change.get("diff")?.as_str()?;
            let kind = change.get("kind")?;
            let kind_type = kind.get("type")?.as_str()?;
            if !matches!(kind_type, "add" | "delete" | "update") { return None; }
            let move_path = match kind.get("move_path").or_else(|| kind.get("movePath")) {
                None | Some(Value::Null) => None,
                Some(Value::String(path)) if !path.is_empty() => Some(path.as_str()),
                _ => return None,
            };
            Some(json!({"path":path, "diff":diff, "kind":{"type":kind_type,"movePath":move_path}}))
        }).collect();
        let Some(files) = files else {
            return;
        };
        let files = Value::Array(files);
        if files.to_string().len() <= super::MAX_FRAME_BYTES {
            self.items.insert(id.to_owned(), files);
        }
    }

    pub(super) fn summary(&self, params: &Value) -> String {
        // Explicit request details are authoritative, even when empty/invalid.
        if params.get("files").is_some() || !self.matches(params) {
            return super::file_change_approval_summary(params);
        }
        let files = params
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|id| self.items.get(id));
        let Some(files) = files else {
            return super::file_change_approval_summary(params);
        };
        let mut correlated = params.clone();
        correlated["files"] = files.clone();
        for file in correlated["files"]
            .as_array_mut()
            .expect("cached changes are an array")
        {
            self.present_path(&mut file["path"]);
            self.present_path(&mut file["kind"]["movePath"]);
        }
        let mut summary: Value =
            serde_json::from_str(&super::file_change_approval_summary(&correlated))
                .expect("approval summary is serialized JSON");
        summary["detailsSource"] = json!("matching_item_started");
        summary.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::FileChanges;
    use serde_json::{Value, json};

    fn item() -> Value {
        json!({"threadId":"thread", "turnId":"turn", "item":{"id":"edit", "type":"fileChange",
            "changes":[{"path":"/tmp/sample.txt","diff":"-before\n+after", "kind":{"type":"update","move_path":null},"privateExtension":"omit"}]}})
    }

    fn request() -> Value {
        json!({"threadId":"thread","turnId":"turn","itemId":"edit"})
    }

    fn summary(cache: &FileChanges, request: &Value) -> Value {
        serde_json::from_str(&cache.summary(request)).unwrap()
    }

    #[test]
    fn presents_only_contained_paths_relative_to_the_actual_working_directory() {
        let mut cache = FileChanges::new("thread".into(), "turn".into());
        cache.set_working_directory("/tmp".into());
        cache.observe("item/started", &item());
        let result = summary(&cache, &request());
        assert_eq!(
            result["files"][0]["path"],
            json!({"relativeTo":"current_working_directory","path":"sample.txt"})
        );
        for path in [
            "/tmp-other/sample.txt",
            "/tmp/../outside.txt",
            "/outside/sample.txt",
        ] {
            let mut event = item();
            event["item"]["changes"][0]["path"] = json!(path);
            cache.observe("item/started", &event);
            assert_eq!(summary(&cache, &request())["files"][0]["path"], path);
        }
    }

    #[test]
    fn correlates_only_same_thread_turn_item_and_clears_completed() {
        let mut cache = FileChanges::new("thread".into(), "turn".into());
        cache.observe("item/started", &item());
        let value = summary(&cache, &request());
        assert_eq!(value["scope"], "reported_files");
        assert_eq!(value["files"][0]["path"], "/tmp/sample.txt");
        assert_eq!(value["detailsSource"], "matching_item_started");
        assert!(!value.to_string().contains("privateExtension"));
        for field in ["threadId", "turnId", "itemId"] {
            let mut wrong = request();
            wrong[field] = json!("other");
            assert_eq!(summary(&cache, &wrong)["scope"], "unknown");
            wrong.as_object_mut().unwrap().remove(field);
            assert_eq!(summary(&cache, &wrong)["scope"], "unknown");
        }
        cache.observe("item/completed", &item());
        assert_eq!(summary(&cache, &request())["scope"], "unknown");
    }

    #[test]
    fn bounds_pending_items_and_ignores_foreign_notifications() {
        let mut cache = FileChanges::new("thread".into(), "turn".into());
        for index in 0..17 {
            let mut event = item();
            event["item"]["id"] = json!(format!("edit-{index}"));
            cache.observe("item/started", &event);
        }
        assert_eq!(cache.items.len(), 16);
        let mut req = request();
        req["itemId"] = json!("edit-16");
        assert_eq!(summary(&cache, &req)["scope"], "unknown");
        let mut event = item();
        event["item"]["id"] = json!("edit-0");
        event["turnId"] = json!("foreign-turn");
        cache.observe("item/completed", &event);
        assert!(cache.items.contains_key("edit-0"));
        event["turnId"] = json!("turn");
        cache.observe("item/completed", &event);
        assert!(!cache.items.contains_key("edit-0"));
        event["item"]["id"] = json!("edit-16");
        cache.observe("item/started", &event);
        assert_eq!(summary(&cache, &req)["scope"], "reported_files");
    }

    #[test]
    fn refuses_partial_oversized_and_explicit_empty_details() {
        let mut cache = FileChanges::new("thread".into(), "turn".into());
        cache.observe("item/started", &item());
        let mut explicit = request();
        explicit["files"] = json!([]);
        assert_eq!(summary(&cache, &explicit)["scope"], "unknown");
        let mut invalid = item();
        invalid["item"]["changes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"/another"}));
        cache.observe("item/started", &invalid);
        assert_eq!(summary(&cache, &request())["scope"], "unknown");
        let mut oversized = item();
        oversized["item"]["changes"][0]["diff"] = json!("x".repeat(super::super::MAX_FRAME_BYTES));
        cache.observe("item/started", &oversized);
        assert_eq!(summary(&cache, &request())["scope"], "unknown");
    }
}
