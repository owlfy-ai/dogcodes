//! Replay successful edits on historical bytes; only exact final content is evidence.
use serde_json::Value;
use std::path::{Path, PathBuf};

fn resolved(root: &str, path: &str) -> PathBuf {
    Path::new(root).join(path).components().collect()
}
fn text(version: &Value) -> Option<String> {
    if version.is_null() {
        return Some(String::new());
    }
    String::from_utf8(hex::decode(version["hex"].as_str()?).ok()?).ok()
}
pub(super) fn explains(primary: &str, file: &Value, edits: &[Value]) -> bool {
    let before = &file["before"];
    let after = &file["after"];
    let regular = |v: &Value| {
        v.is_null()
            || matches!(v["mode"].as_str(), Some("100644" | "100755"))
            || v["kind"] == "file"
    };
    if !regular(before)
        || !regular(after)
        || (!before.is_null()
            && !after.is_null()
            && (before["mode"] != after["mode"] || before["kind"] != after["kind"]))
    {
        return false;
    }
    // Plain text creation does not explain a separate chmod.
    if before.is_null()
        && (after["mode"] == "100755"
            || after["mode"].as_u64().is_some_and(|mode| mode & 0o111 != 0))
    {
        return false;
    }
    let (Some(mut content), Some(expected)) = (text(before), text(after)) else {
        return false;
    };
    let Some(relative) = file["workspaceRelativePath"]
        .as_str()
        .or(file["relativePath"].as_str())
        .or(file["path"].as_str())
    else {
        return false;
    };
    let target = resolved(file["workspacePath"].as_str().unwrap_or(primary), relative);
    let matches = |path: &Value| {
        path.as_str()
            .is_some_and(|p| resolved(primary, p) == target)
    };
    // Creation/deletion must be explicit; identical empty bytes alone do not
    // prove that a directory entry appeared or disappeared.
    let mut exists = !before.is_null();
    let mut applied = false;
    for edit in edits {
        if let Some(change) = edit["changes"]
            .as_array()
            .and_then(|cs| cs.iter().find(|c| matches(&c["path"])))
        {
            let Some(diff) = change["diff"].as_str() else {
                return false;
            };
            let Some(next) = super::patch::unified(&content, diff) else {
                return false;
            };
            match change["kind"]["type"].as_str().or(change["kind"].as_str()) {
                Some("add") if !exists => exists = true,
                Some("delete") if exists && next.is_empty() => exists = false,
                _ if exists => (),
                _ => return false,
            }
            content = next;
            applied = true;
            continue;
        }
        let display = &edit["display"];
        if display["kind"] == "file_diff"
            && matches(&display["filePath"])
            && display["truncated"] != true
            && let Some(next) = super::patch::structured(&content, &display["structuredPatch"])
        {
            exists = true;
            content = next;
            applied = true;
            continue;
        }
        if let Some(block) = edit["content"].as_array().and_then(|c| {
            c.iter()
                .find(|b| b["type"] == "diff" && matches(&b["path"]))
        }) && let (Some(old), Some(new)) = (block["oldText"].as_str(), block["newText"].as_str())
        {
            if old != content {
                return false;
            }
            new.clone_into(&mut content);
            exists = true;
            applied = true;
            continue;
        }
        let i = &edit["input"];
        if ![
            "path",
            "file_path",
            "filePath",
            "target_file",
            "TargetFile",
            "AbsolutePath",
        ]
        .iter()
        .any(|key| matches(&i[*key]))
        {
            continue;
        }
        if let (Some(old), Some(new)) = (i["old_string"].as_str(), i["new_string"].as_str()) {
            if !exists || old.is_empty() || !content.contains(old) {
                return false;
            }
            if i["replace_all"] == true {
                let growth = new
                    .len()
                    .saturating_sub(old.len())
                    .saturating_mul(content.matches(old).count());
                if content.len().saturating_add(growth)
                    > agent_protocol::workspace_content::MAX_WORKSPACE_FILE_BYTES
                {
                    return false;
                }
                content = content.replace(old, new);
            } else if content.match_indices(old).nth(1).is_none() {
                content = content.replacen(old, new, 1);
            } else {
                return false;
            }
        } else if let Some(new) = i["content"].as_str()
            && matches!(
                edit["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "write" | "write_file" | "write_to_file"
            )
        {
            new.clone_into(&mut content);
            exists = true;
        } else {
            return false;
        }
        applied = true;
    }
    applied && exists != after.is_null() && content == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn file(after: &str) -> Value {
        json!({"path":"src/a","before":{"mode":"100644","hex":hex::encode("a\nb\n")},"after":{"mode":"100644","hex":hex::encode(after)}})
    }
    #[test]
    fn kimi_replacements_explain_only_their_net_changes() {
        let edit = json!({"input":{"path":"/repo/src/a","old_string":"a","new_string":"c"}});
        assert!(explains(
            "/repo",
            &file("c\nb\n"),
            std::slice::from_ref(&edit)
        ));
        assert!(!explains(
            "/repo",
            &file("c\nexternal\n"),
            std::slice::from_ref(&edit)
        ));
        assert!(!explains("/other", &file("c\nb\n"), &[edit]));
    }
    #[test]
    fn reported_hunks_must_match_original_lines_and_final_bytes() {
        let edit = json!({"changes":[{"path":"src/a","diff":"@@ -1,2 +1,2 @@\n a\n-b\n+c\n"}]});
        assert!(explains(
            "/repo",
            &file("a\nc\n"),
            std::slice::from_ref(&edit)
        ));
        assert!(!explains("/repo", &file("external\nc\n"), &[edit]));
        let zcode = json!({"display":{"kind":"file_diff","filePath":"/repo/src/a","structuredPatch":[{"oldStart":2,"oldLines":1,"newLines":1,"lines":["-b","+c"]}]}});
        assert!(explains("/repo", &file("a\nc\n"), &[zcode]));
    }
}
