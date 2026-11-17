//! Bounded filesystem snapshots. No Git initialization, symlink traversal or watchers.
use agent_protocol::workspace_context::WorkspaceTurnCapture;
use agent_protocol::{WorkspaceCollectionDiagnostic, WorkspaceDiagnosticKind};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_BYTES: usize = agent_protocol::workspace_content::MAX_WORKSPACE_CONTENT_BYTES;
const MAX_FILE_BYTES: usize = agent_protocol::workspace_content::MAX_WORKSPACE_FILE_BYTES;
// Content retention and change detection have separate budgets. A generated PDF
// must remain in the file list even when its bytes cannot fit in a diff artifact.
const MAX_HASH_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000;
const EXCLUDED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "__pycache__",
    ".DS_Store",
];

fn failure() -> WorkspaceCollectionDiagnostic {
    WorkspaceCollectionDiagnostic {
        kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
    }
}

#[cfg(test)]
fn snapshot(root: &Path) -> Result<BTreeMap<String, Value>, WorkspaceCollectionDiagnostic> {
    snapshot_with_rules(root, None).map(|(entries, _)| entries)
}

type IgnoreRules = BTreeMap<String, String>;
fn snapshot_with_rules(
    root: &Path,
    frozen: Option<&IgnoreRules>,
) -> Result<(BTreeMap<String, Value>, IgnoreRules), WorkspaceCollectionDiagnostic> {
    let started = Instant::now();
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut rules = frozen.cloned().unwrap_or_default();
    let mut remaining = MAX_BYTES;
    let mut hash_remaining = MAX_HASH_BYTES;
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        // Stable traversal makes retention predictable even when a quota is reached.
        let mut children = fs::read_dir(&directory)
            .map_err(|_| failure())?
            .take(MAX_ENTRIES + 1)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| failure())?;
        if children.len() > MAX_ENTRIES {
            return Err(WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::PathBudgetExceeded,
            });
        }
        children.sort_by_key(std::fs::DirEntry::file_name);
        let mut ancestors: Vec<_> = directory
            .ancestors()
            .take_while(|p| p.starts_with(root))
            .collect();
        ancestors.reverse();
        let mut matchers = Vec::new();
        let ignore_paths = ancestors.iter().map(|p| p.join(".gitignore"));
        for path in ignore_paths {
            let key = path
                .strip_prefix(root)
                .map_err(|_| failure())?
                .to_str()
                .ok_or_else(failure)?
                .to_owned();
            if frozen.is_none() && !rules.contains_key(&key) {
                match fs::File::open(&path) {
                    Ok(file) => {
                        let mut text = String::new();
                        file.take(128_001)
                            .read_to_string(&mut text)
                            .map_err(|_| failure())?;
                        if rules.values().map(String::len).sum::<usize>() + text.len() > 128_000 {
                            return Err(failure());
                        }
                        rules.insert(key.clone(), text);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => return Err(failure()),
                }
            }
            if let Some(text) = rules.get(&key) {
                let mut builder =
                    ignore::gitignore::GitignoreBuilder::new(path.parent().ok_or_else(failure)?);
                for line in text.trim_start_matches('\u{feff}').lines() {
                    let _ = builder.add_line(Some(path.clone()), line);
                }
                matchers.push(builder.build().map_err(|_| failure())?);
            }
        }
        for entry in children {
            visited += 1;
            if visited > MAX_ENTRIES || started.elapsed() > Duration::from_secs(5) {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::PathBudgetExceeded,
                });
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(failure)?;
            if EXCLUDED.contains(&name) {
                continue;
            }
            let path = entry.path();
            let before = fs::symlink_metadata(&path).map_err(|_| failure())?;
            let matched = matchers
                .iter()
                .rev()
                .map(|matcher| matcher.matched_path_or_any_parents(&path, before.is_dir()))
                .find(|matched| !matched.is_none());
            if matched.is_some_and(|matched| matched.is_ignore()) {
                continue;
            }
            if before.is_dir() {
                if !path
                    .join(agent_protocol::workspace_context::CONVERSATION_FOLDERS_MARKER)
                    .exists()
                {
                    pending.push(path);
                }
                continue;
            }
            files.push(path);
        }
    }
    // Reserve body space for source/configuration before lockfiles and generated data.
    files.sort_by_key(|path| {
        let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
        let priority = !matches!(
            extension,
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "cts"
                | "mjs"
                | "py"
                | "go"
                | "java"
                | "c"
                | "h"
                | "cpp"
                | "css"
                | "html"
                | "toml"
                | "yaml"
                | "yml"
                | "md"
        );
        (priority, path.clone())
    });
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| failure())?
            .to_str()
            .ok_or_else(failure)?
            .to_owned();
        let before = fs::symlink_metadata(&path).map_err(|_| failure())?;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            before.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = u32::from(before.permissions().readonly());
        let (kind, digest, bytes) = if before.file_type().is_symlink() {
            let bytes = fs::read_link(&path)
                .map_err(|_| failure())?
                .to_str()
                .ok_or_else(failure)?
                .as_bytes()
                .to_vec();
            ("symlink", hex::encode(Sha256::digest(&bytes)), Some(bytes))
        } else if before.is_file() {
            if before.len() > hash_remaining as u64 {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::OutputTruncated,
                });
            }
            let mut file = fs::File::open(&path).map_err(|_| failure())?;
            let mut hash = Sha256::new();
            let mut bytes = (before.len() <= remaining as u64
                && before.len() <= MAX_FILE_BYTES as u64)
                .then(Vec::new);
            let mut buffer = vec![0u8; 64 * 1024];
            let mut read = 0u64;
            loop {
                if started.elapsed() > Duration::from_secs(5) {
                    return Err(WorkspaceCollectionDiagnostic {
                        kind: WorkspaceDiagnosticKind::TimedOut,
                    });
                }
                let count = file.read(&mut buffer).map_err(|_| failure())?;
                if count == 0 {
                    break;
                }
                hash_remaining =
                    hash_remaining
                        .checked_sub(count)
                        .ok_or(WorkspaceCollectionDiagnostic {
                            kind: WorkspaceDiagnosticKind::OutputTruncated,
                        })?;
                read += count as u64;
                hash.update(&buffer[..count]);
                if let Some(retained) = &mut bytes {
                    if retained.len() + count <= remaining {
                        retained.extend_from_slice(&buffer[..count]);
                    } else {
                        bytes = None;
                    }
                }
            }
            if read != before.len() {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::WorkspaceChanged,
                });
            }
            ("file", hex::encode(hash.finalize()), bytes)
        } else {
            return Err(failure());
        };
        let mut value = json!({"kind":kind,"mode":mode,"sha256":digest});
        if let Some(bytes) = bytes.filter(|bytes| bytes.len() <= remaining) {
            remaining -= bytes.len();
            value["hex"] = json!(hex::encode(bytes));
        } else {
            value["contentOmitted"] = json!(true);
            value["omissionReason"] = json!(if before.len() > MAX_FILE_BYTES as u64 {
                "file_limit"
            } else {
                "snapshot_limit"
            });
        }
        let after = fs::symlink_metadata(&path).map_err(|_| failure())?;
        if before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || before.file_type() != after.file_type()
        {
            return Err(WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::WorkspaceChanged,
            });
        }
        entries.insert(relative, value);
    }
    Ok((entries, rules))
}

pub(super) fn begin(
    root: &Path,
) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
    let (before, ignore_rules) = snapshot_with_rules(root, None)?;
    Ok(Box::new(Capture {
        root: root.to_owned(),
        before,
        ignore_rules,
    }))
}

pub(super) fn checkpoint(backend: &str, entries: &BTreeMap<String, Value>) -> Value {
    json!({"schemaVersion":1,"backend":backend,"entries":entries})
}

pub(super) fn compare(
    checkpoint: &Value,
    after: &Value,
) -> Result<Value, WorkspaceCollectionDiagnostic> {
    if checkpoint["schemaVersion"] != 1 || checkpoint["backend"] != after["backend"] {
        return Err(failure());
    }
    let before: BTreeMap<String, Value> =
        serde_json::from_value(checkpoint["entries"].clone()).map_err(|_| failure())?;
    let after: BTreeMap<String, Value> =
        serde_json::from_value(after["entries"].clone()).map_err(|_| failure())?;
    Ok(diff(
        &before,
        &after,
        checkpoint["backend"].as_str().ok_or_else(failure)?,
    ))
}

struct Capture {
    root: PathBuf,
    before: BTreeMap<String, Value>,
    ignore_rules: IgnoreRules,
}

#[async_trait]
impl WorkspaceTurnCapture for Capture {
    async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
        let mut value = checkpoint("filesystem", &self.before);
        value["ignoreRules"] = json!(self.ignore_rules);
        Ok(Some(value))
    }
    async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic> {
        let (after, _) = snapshot_with_rules(&self.root, Some(&self.ignore_rules))?;
        Ok(diff(&self.before, &after, "filesystem"))
    }
}

pub(super) fn recover(root: &Path, before: &Value) -> Result<Value, WorkspaceCollectionDiagnostic> {
    let rules: IgnoreRules =
        serde_json::from_value(before["ignoreRules"].clone()).map_err(|_| failure())?;
    let (entries, _) = snapshot_with_rules(root, Some(&rules))?;
    compare(before, &checkpoint("filesystem", &entries))
}

fn diff(before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>, backend: &str) -> Value {
    let mut paths: Vec<_> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    let mut patch = String::new();
    let mut patch_truncated = false;
    for path in paths {
        let old = before.get(path);
        let new = after.get(path);
        // Retained bytes may vary with the shared content budget. Compare file
        // identity, not whether this particular scan happened to retain text.
        if old.zip(new).is_some_and(|(old, new)| {
            ["kind", "mode", "sha256"]
                .iter()
                .all(|key| old[key] == new[key])
        }) {
            continue;
        }
        let status = if old.is_none() {
            "A"
        } else if new.is_none() {
            "D"
        } else {
            "M"
        };
        let decode = |value: Option<&Value>| -> Option<String> {
            let bytes = value.map_or_else(
                || Some(Vec::new()),
                |v| hex::decode(v["hex"].as_str()?).ok(),
            )?;
            if bytes.contains(&0) {
                return None;
            }
            String::from_utf8(bytes).ok()
        };
        let old_text = decode(old);
        let new_text = decode(new);
        let missing = [old, new]
            .into_iter()
            .flatten()
            .find(|v| v["contentOmitted"] == true);
        let binary = missing.is_none() && (old_text.is_none() || new_text.is_none());
        let mut file =
            json!({"path":path,"status":status,"before":old,"after":new,"binary":binary});
        if let Some(missing) = missing {
            file["diffUnavailableReason"] = missing["omissionReason"].clone();
        } else if binary {
            file["diffUnavailableReason"] = json!("binary");
        } else if let (Some(old_text), Some(new_text)) = (old_text, new_text) {
            let changes = similar::TextDiff::configure()
                .timeout(Duration::from_millis(100))
                .diff_lines(&old_text, &new_text);
            let mut additions = 0;
            let mut deletions = 0;
            for change in changes.iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => additions += 1,
                    similar::ChangeTag::Delete => deletions += 1,
                    similar::ChangeTag::Equal => (),
                }
            }
            file["additions"] = json!(additions);
            file["deletions"] = json!(deletions);
            // Keep the raw patch useful for small changes without duplicating every body.
            if patch.len() < 128_000 {
                let unified = changes
                    .unified_diff()
                    .context_radius(3)
                    .header(&format!("a/{path}"), &format!("b/{path}"))
                    .to_string();
                if patch.len() + unified.len() <= 128_000 {
                    patch.push_str(&unified);
                } else {
                    patch_truncated = true;
                }
            } else {
                patch_truncated = true;
            }
        }
        files.push(file);
    }
    json!({"schemaVersion":1,"backend":backend,"attribution":"workspace_interval","scope":"bounded snapshot comparison; no symlink traversal","excludedNames":if backend == "filesystem" { EXCLUDED } else { &[] },"files":files,"patch":patch,"patchTruncated":patch_truncated,"patchHex":null})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn large_generated_files_keep_the_summary_and_small_text_diff() {
        let root = tempfile::tempdir().unwrap();
        let capture = begin(root.path()).unwrap();
        let checkpoint = capture.checkpoint().await.unwrap().unwrap();
        let pdf = vec![0x80; 548_217];
        fs::write(root.path().join("irregular-verbs.pdf"), &pdf).unwrap();
        fs::write(
            root.path().join("irregular-verbs.html"),
            "<html>verbs</html>\n",
        )
        .unwrap();
        let changes = capture.finish().await.unwrap();
        let files = changes["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        let pdf_file = files
            .iter()
            .find(|file| file["path"] == "irregular-verbs.pdf")
            .unwrap();
        assert_eq!(pdf_file["status"], "A");
        assert_eq!(pdf_file["after"]["contentOmitted"], true);
        assert_eq!(
            pdf_file["after"]["sha256"],
            hex::encode(Sha256::digest(&pdf))
        );
        assert!(
            changes["patch"]
                .as_str()
                .unwrap()
                .contains("+<html>verbs</html>")
        );
        assert!(serde_json::to_vec(&changes).unwrap().len() < 1_048_576);
        let next = begin(root.path()).unwrap();
        let after = next.checkpoint().await.unwrap().unwrap();
        assert_eq!(compare(&checkpoint, &after).unwrap(), changes);
        assert!(
            next.finish().await.unwrap()["files"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::write(
            root.path().join("irregular-verbs.pdf"),
            vec![0x81; pdf.len()],
        )
        .unwrap();
        let modified = next.finish().await.unwrap();
        assert_eq!(modified["files"].as_array().unwrap().len(), 1);
        assert_eq!(modified["files"][0]["status"], "M");
        fs::remove_file(root.path().join("irregular-verbs.pdf")).unwrap();
        assert_eq!(next.finish().await.unwrap()["files"][0]["status"], "D");
    }

    #[tokio::test]
    async fn source_bodies_survive_a_project_larger_than_the_old_shared_budget() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("package-lock.json"), " ".repeat(120_000)).unwrap();
        fs::create_dir(root.path().join("electron")).unwrap();
        let source = "const value = 1;\n".repeat(4_000);
        for name in ["automation.ts", "store.ts", "main.ts"] {
            fs::write(root.path().join("electron").join(name), &source).unwrap();
        }
        let capture = begin(root.path()).unwrap();
        for name in ["automation.ts", "store.ts", "main.ts"] {
            fs::write(
                root.path().join("electron").join(name),
                format!("{source}export const changed = true;\n"),
            )
            .unwrap();
        }
        let changes = capture.finish().await.unwrap();
        for file in changes["files"].as_array().unwrap() {
            assert!(file["before"]["hex"].is_string());
            assert!(file["after"]["hex"].is_string());
            assert_eq!(file["binary"], false);
            assert_eq!(file["additions"], 1);
            assert_eq!(file["deletions"], 0);
        }
        assert_eq!(changes["files"].as_array().unwrap().len(), 3);
        assert!(!root.path().join(".git").exists());
    }

    #[test]
    fn ignores_build_outputs_and_supports_nested_rules() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".gitignore"),
            "dist/\ndist-electron/\n*.log\n*.tmp\n!keep.tmp\nprivate/\n",
        )
        .unwrap();
        for directory in ["dist", "dist-electron", "private", "src"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        for name in [
            "dist/app.js",
            "dist-electron/main.js",
            "private/data",
            "app.log",
            "keep.tmp",
            "src/a.ts",
            "src/skip.ts",
        ] {
            fs::write(root.path().join(name), "text").unwrap();
        }
        fs::write(root.path().join("skip.ts"), "root file").unwrap();
        fs::write(root.path().join("src/.gitignore"), "/skip.ts\n").unwrap();
        let entries = snapshot(root.path()).unwrap();
        assert!(entries.contains_key("keep.tmp"));
        assert!(entries.contains_key("src/a.ts"));
        assert!(entries.contains_key("skip.ts"));
        for path in [
            "dist/app.js",
            "dist-electron/main.js",
            "private/data",
            "app.log",
            "src/skip.ts",
        ] {
            assert!(!entries.contains_key(path), "{path}");
        }
    }

    #[tokio::test]
    async fn file_size_omission_is_not_misclassified_as_binary() {
        let root = tempfile::tempdir().unwrap();
        let capture = begin(root.path()).unwrap();
        fs::write(root.path().join("large.ts"), "x".repeat(MAX_FILE_BYTES + 1)).unwrap();
        fs::write(root.path().join("binary.bin"), [0, 255]).unwrap();
        let changes = capture.finish().await.unwrap();
        let files = changes["files"].as_array().unwrap();
        let large = files.iter().find(|v| v["path"] == "large.ts").unwrap();
        assert_eq!(large["diffUnavailableReason"], "file_limit");
        assert_eq!(large["binary"], false);
        let binary = files.iter().find(|v| v["path"] == "binary.bin").unwrap();
        assert_eq!(binary["diffUnavailableReason"], "binary");
        assert_eq!(binary["binary"], true);
    }

    #[tokio::test]
    async fn ignore_changes_do_not_fabricate_deletions_and_recovery_keeps_the_same_scope() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitignore"), "").unwrap();
        fs::write(root.path().join("a.ts"), "same\n").unwrap();
        let capture = begin(root.path()).unwrap();
        let checkpoint = capture.checkpoint().await.unwrap().unwrap();
        fs::write(root.path().join(".gitignore"), "*.ts\n").unwrap();
        let changes = capture.finish().await.unwrap();
        assert_eq!(changes["files"].as_array().unwrap().len(), 1);
        assert_eq!(changes["files"][0]["path"], ".gitignore");
        assert_eq!(recover(root.path(), &checkpoint).unwrap(), changes);
    }

    #[test]
    fn snapshot_quota_preserves_source_priority_and_records_the_actual_omission() {
        let root = tempfile::tempdir().unwrap();
        let body = "x".repeat(MAX_FILE_BYTES);
        for index in 0..33 {
            fs::write(root.path().join(format!("{index:02}.data")), &body).unwrap();
        }
        fs::write(root.path().join("z.ts"), "export const value = 1;\n").unwrap();
        let entries = snapshot(root.path()).unwrap();
        assert!(entries["z.ts"]["hex"].is_string());
        assert_eq!(entries["32.data"]["omissionReason"], "snapshot_limit");
        assert!(entries["32.data"]["sha256"].is_string());
    }

    #[test]
    fn retention_budget_changes_do_not_report_unchanged_files_as_modified() {
        let retained = json!({"kind":"file","mode":420,"sha256":"same","hex":"6162"});
        let omitted = json!({"kind":"file","mode":420,"sha256":"same","contentOmitted":true});
        let before = BTreeMap::from([("file.txt".into(), retained)]);
        let after = BTreeMap::from([("file.txt".into(), omitted)]);
        assert!(
            diff(&before, &after, "filesystem")["files"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            diff(&after, &before, "filesystem")["files"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn exclusions_deletions_symlinks_and_incomplete_scans_are_explicit() {
        let root = std::env::temp_dir().join(format!("dogcodes-fs-edges-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("delete.txt"), "old").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/ignored"), vec![0; MAX_BYTES + 1]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/not-followed", root.join("link")).unwrap();
        let capture = begin(&root).unwrap();
        let checkpoint = capture.checkpoint().await.unwrap().unwrap();
        fs::remove_file(root.join("delete.txt")).unwrap();
        let changes = capture.finish().await.unwrap();
        assert_eq!(changes["files"][0]["status"], "D");
        assert_eq!(changes["files"].as_array().unwrap().len(), 1);
        let after = begin(&root).unwrap().checkpoint().await.unwrap().unwrap();
        assert_eq!(compare(&checkpoint, &after).unwrap(), changes);
        assert!(
            compare(
                &checkpoint,
                &json!({"schemaVersion":1,"backend":"git","entries":{}})
            )
            .is_err()
        );
        let large = fs::File::create(root.join("too-large")).unwrap();
        large.set_len(MAX_HASH_BYTES as u64 + 1).unwrap();
        assert!(capture.finish().await.is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_of_conversation_folders_is_left_out() {
        let root = tempfile::tempdir().unwrap();
        let drafts = root.path().join("drafts");
        fs::create_dir_all(drafts.join("tsk_one")).unwrap();
        fs::write(drafts.join("tsk_one/screenshot.png"), b"\x89PNG").unwrap();
        fs::write(root.path().join("notes.txt"), b"mine").unwrap();
        // Snapshot keys use the platform's separator.
        let screenshot = Path::new("drafts")
            .join("tsk_one")
            .join("screenshot.png")
            .to_str()
            .unwrap()
            .to_owned();
        assert!(snapshot(root.path()).unwrap().contains_key(&screenshot));
        fs::write(
            drafts.join(agent_protocol::workspace_context::CONVERSATION_FOLDERS_MARKER),
            "",
        )
        .unwrap();
        let entries = snapshot(root.path()).unwrap();
        assert_eq!(entries.keys().collect::<Vec<_>>(), ["notes.txt"]);
        // The conversation's own folder still sees its files.
        assert!(
            snapshot(&drafts.join("tsk_one"))
                .unwrap()
                .contains_key("screenshot.png")
        );
    }

    #[tokio::test]
    async fn captures_interval_without_initializing_git() {
        let root = std::env::temp_dir().join(format!("dogcodes-fs-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "user edit\n").unwrap();
        let capture = begin(&root).unwrap();
        fs::write(root.join("a.txt"), "agent edit\n").unwrap();
        fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        let changes = capture.finish().await.unwrap();
        assert!(
            changes["patch"]
                .as_str()
                .unwrap()
                .contains("-user edit\n+agent edit")
        );
        assert_eq!(changes["files"].as_array().unwrap().len(), 2);
        assert!(!root.join(".git").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
