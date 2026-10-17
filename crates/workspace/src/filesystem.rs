//! Bounded filesystem snapshots. No Git initialization, symlink traversal or watchers.
use agent_core::WorkspaceTurnCapture;
use agent_protocol::{WorkspaceCollectionDiagnostic, WorkspaceDiagnosticKind};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 128_000;
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

fn snapshot(root: &Path) -> Result<BTreeMap<String, Value>, WorkspaceCollectionDiagnostic> {
    let started = Instant::now();
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut remaining = MAX_BYTES;
    let mut hash_remaining = MAX_HASH_BYTES;
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(|_| failure())? {
            visited += 1;
            if visited > MAX_ENTRIES || started.elapsed() > Duration::from_secs(5) {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::PathBudgetExceeded,
                });
            }
            let entry = entry.map_err(|_| failure())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(failure)?;
            if EXCLUDED.contains(&name) {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| failure())?
                .to_str()
                .ok_or_else(failure)?
                .to_owned();
            let before = fs::symlink_metadata(&path).map_err(|_| failure())?;
            if before.is_dir() {
                pending.push(path);
                continue;
            }
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
                let mut bytes = (before.len() <= remaining as u64).then(Vec::new);
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
    }
    Ok(entries)
}

pub(super) fn begin(
    root: &Path,
) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
    Ok(Box::new(Capture {
        root: root.to_owned(),
        before: snapshot(root)?,
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
}

#[async_trait]
impl WorkspaceTurnCapture for Capture {
    async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
        Ok(Some(checkpoint("filesystem", &self.before)))
    }
    async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic> {
        let after = snapshot(&self.root)?;
        Ok(diff(&self.before, &after, "filesystem"))
    }
}

fn diff(before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>, backend: &str) -> Value {
    let mut paths: Vec<_> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    let mut patch = String::new();
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
        let mut binary = true;
        if let (Some(old_text), Some(new_text)) = (decode(old), decode(new)) {
            binary = false;
            if old_text != new_text {
                // A valid full-file unified hunk; deliberately no heuristic summary.
                let _ = write!(
                    patch,
                    "--- {}\n+++ {}\n@@ -{},{} +{},{} @@\n",
                    if old.is_some() {
                        format!("a/{path}")
                    } else {
                        "/dev/null".into()
                    },
                    if new.is_some() {
                        format!("b/{path}")
                    } else {
                        "/dev/null".into()
                    },
                    usize::from(!old_text.is_empty()),
                    old_text.lines().count(),
                    usize::from(!new_text.is_empty()),
                    new_text.lines().count()
                );
                for (prefix, text) in [("-", &old_text), ("+", &new_text)] {
                    for line in text.split_inclusive('\n') {
                        patch.push_str(prefix);
                        patch.push_str(line);
                        if !line.ends_with('\n') {
                            patch.push_str("\n\\ No newline at end of file\n");
                        }
                    }
                }
            }
        }
        files.push(json!({"path":path,"status":status,"before":old,"after":new,"binary":binary}));
    }
    json!({"schemaVersion":1,"backend":backend,"attribution":"workspace_interval","scope":"bounded snapshot comparison; no symlink traversal","excludedNames":if backend == "filesystem" { EXCLUDED } else { &[] },"files":files,"patch":patch,"patchHex":null})
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
