//! Whole-turn restoration uses the same guarded, durable file transactions as Review.
use super::{ReviewStore, digest, private_directory, transaction, write_once};
use crate::review::{FileKind, FileVersion, validate_relative};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct TurnUndo {
    task: String,
    artifact: String,
    records: Vec<String>,
}

impl ReviewStore {
    /// Compare only paths already captured for undo. No session-wide scan or
    /// baseline is created; live reads use the same guarded path handling as undo.
    pub fn recorded_file_changes(
        &self,
        root: &Path,
        before: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Vec<crate::review::FileChange>, String> {
        use crate::review::{ChangeKind, FileChange};
        let root = root.canonicalize().map_err(|_| "工作目录不可用")?;
        let root = root.to_str().ok_or("工作目录编码无效")?;
        let mut changes = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for (path, value) in before {
            if std::time::Instant::now() >= deadline {
                return Err("读取已记录修改超时".into());
            }
            validate_relative(path)?;
            let before = snapshot_version(self, value)?;
            let after = transaction::current_version(self, root, path)?;
            if before == after {
                continue;
            }
            let kind = match (&before, &after) {
                (None, _) => ChangeKind::Added,
                (_, None) => ChangeKind::Deleted,
                _ => ChangeKind::Modified,
            };
            changes.push(FileChange {
                path: path.clone(),
                previous_path: None,
                kind,
                before,
                after,
            });
        }
        Ok(changes)
    }

    fn turn_undo_path(&self, task: &str, artifact: &str) -> PathBuf {
        self.directory
            .join("turn-undo")
            .join(digest(format!("{task}\0{artifact}").as_bytes()))
    }

    pub fn turn_is_undone(&self, task: &str, artifact: &str) -> Result<bool, String> {
        let path = self.turn_undo_path(task, artifact);
        if !path.exists() {
            return Ok(false);
        }
        let undo: TurnUndo =
            serde_json::from_slice(&std::fs::read(path).map_err(|_| "无法读取撤销记录")?)
                .map_err(|_| "撤销记录损坏")?;
        if undo.task != task || undo.artifact != artifact {
            return Err("撤销记录不匹配".into());
        }
        Ok(!undo.records.is_empty()
            && undo.records.iter().all(|id| {
                self.recovery_record(task, id)
                    .is_ok_and(|entry| entry.applied && !entry.undone)
            }))
    }

    pub fn undo_turn(
        &self,
        task: &str,
        artifact: &str,
        data: &Value,
        allowed: &[String],
    ) -> Result<(), String> {
        let manifest = self.turn_undo_path(task, artifact);
        if manifest.exists() {
            return if self.turn_is_undone(task, artifact)? {
                Ok(())
            } else {
                Err("上次撤销未完成，请到审查中的恢复记录处理后再操作".into())
            };
        }
        if data["attribution"] != "workspace_interval" || data["recoveredAfterInterruption"] == true
        {
            return Err("此记录不是完整的一轮修改，无法整轮撤销".into());
        }
        let files = data["files"].as_array().ok_or("缺少文件快照")?;
        if files.is_empty() {
            return Err("本轮没有可撤销的文件修改".into());
        }
        let roots = data["workspacePaths"].as_array();
        let mut seen = BTreeSet::new();
        let mut changes = Vec::new();
        // Validate every snapshot and current version before writing any workspace file.
        for file in files {
            let (root, relative) = if data["schemaVersion"] == 2 {
                (
                    file["workspacePath"].as_str(),
                    file["workspaceRelativePath"]
                        .as_str()
                        .or_else(|| file["relativePath"].as_str()),
                )
            } else {
                (
                    roots.and_then(|r| if r.len() == 1 { r[0].as_str() } else { None }),
                    file["workspaceRelativePath"]
                        .as_str()
                        .or_else(|| file["path"].as_str()),
                )
            };
            let root = root.ok_or("旧记录缺少原始工作目录，无法安全撤销")?;
            let relative = relative.ok_or("记录缺少文件路径")?;
            validate_relative(relative)?;
            if !allowed.iter().any(|p| p == root) {
                return Err("本轮工作目录已不在当前项目中，无法撤销".into());
            }
            let canonical = Path::new(root)
                .canonicalize()
                .map_err(|_| "原始工作目录不可用")?;
            if !seen.insert(canonical.join(relative)) {
                return Err("记录包含重复文件路径".into());
            }
            let before = snapshot_version(self, file.get("before").ok_or("缺少修改前快照")?)?;
            let after = snapshot_version(self, file.get("after").ok_or("缺少修改后快照")?)?;
            if before == after {
                continue;
            }
            let canonical_root = canonical.to_str().ok_or("工作目录编码无效")?;
            if transaction::current_version(self, canonical_root, relative)? != after {
                return Err(format!("{relative} 在本轮结束后又有修改，未撤销任何文件"));
            }
            changes.push((
                canonical_root.to_owned(),
                relative.to_owned(),
                after,
                before,
            ));
        }
        // File/directory topology replacement needs a separate operation ordering.
        for a in &seen {
            if seen.iter().any(|b| a != b && a.starts_with(b)) {
                return Err("本轮包含文件与目录互换，暂不能整轮撤销".into());
            }
        }
        if changes.is_empty() {
            return Err("本轮没有可撤销的文件修改".into());
        }
        let mut records = Vec::new();
        for (root, path, after, before) in changes {
            records.push(
                self.prepare_recovery(task, Path::new(&root), &path, after, before)?
                    .id,
            );
        }
        // Preserve the complete operation intent before the first file is touched.
        private_directory(manifest.parent().ok_or("撤销存储不可用")?)?;
        write_once(
            &manifest,
            &serde_json::to_vec(&TurnUndo {
                task: task.into(),
                artifact: artifact.into(),
                records: records.clone(),
            })
            .map_err(|_| "无法保存撤销记录")?,
        )?;
        for (index, id) in records.iter().enumerate() {
            if let Err(error) = self.apply_recovery(task, id) {
                // A racing external editor can still invalidate a file after preflight.
                // Roll back completed members without overwriting subsequent edits.
                let mut recovered = self.reconcile_recovery(task, id).is_ok();
                for previous in records[..=index].iter().rev() {
                    if self
                        .recovery_record(task, previous)
                        .is_ok_and(|e| e.applied && !e.undone)
                    {
                        recovered &= self.undo_recovery(task, previous).is_ok();
                    }
                }
                return Err(format!(
                    "撤销未完成：{error}。{}",
                    if recovered {
                        "已恢复已处理的文件；备份保存在恢复记录中"
                    } else {
                        "备份已保留，请在审查的恢复记录中处理"
                    }
                ));
            }
        }
        Ok(())
    }
}

fn snapshot_version(store: &ReviewStore, value: &Value) -> Result<Option<FileVersion>, String> {
    if value.is_null() {
        return Ok(None);
    }
    if value["contentOmitted"] == true {
        return Err("本轮快照内容不完整，无法撤销全部修改".into());
    }
    let bytes = hex::decode(value["hex"].as_str().ok_or("缺少完整文件内容")?)
        .map_err(|_| "文件快照编码错误")?;
    let hash = store.put_blob(&bytes)?;
    if value["sha256"].as_str() != Some(&hash) {
        return Err("文件快照校验失败".into());
    }
    let (kind, executable) = if let Some(mode) = value["mode"].as_str() {
        match mode {
            "100644" => (FileKind::File, false),
            "100755" => (FileKind::File, cfg!(unix)),
            "120000" => (FileKind::Symlink, false),
            _ => return Err("不支持撤销此文件类型".into()),
        }
    } else if let (Some(kind), Some(mode)) = (value["kind"].as_str(), value["mode"].as_u64()) {
        match kind {
            "file" => (FileKind::File, cfg!(unix) && mode & 0o111 != 0),
            "symlink" => (FileKind::Symlink, false),
            _ => return Err("不支持撤销此文件类型".into()),
        }
    } else {
        return Err("旧记录缺少文件类型快照，无法安全撤销".into());
    };
    Ok(Some(FileVersion {
        hash,
        bytes: bytes.len() as u64,
        kind,
        executable,
        binary: bytes.contains(&0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn version(bytes: &[u8]) -> Value {
        json!({"hex":hex::encode(bytes),"sha256":digest(bytes),"mode":"100644"})
    }
    fn artifact(root: &Path, files: Value) -> Value {
        json!({"schemaVersion":1,"workspacePaths":[root],"attribution":"workspace_interval","files":files})
    }
    #[test]
    fn recorded_changes_read_only_known_paths_and_follow_current_content_without_baseline() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        std::fs::write(root.path().join("edited"), b"latest external edit").unwrap();
        std::fs::write(root.path().join("added"), b"\0\x01").unwrap();
        // An unrecorded large file must not be scanned or included in the list.
        std::fs::File::create(root.path().join("unrelated"))
            .unwrap()
            .set_len(200 * 1024 * 1024)
            .unwrap();
        let before = std::collections::BTreeMap::from([
            ("edited".into(), version(b"original")),
            ("added".into(), Value::Null),
            ("deleted".into(), version(b"removed")),
        ]);
        let changes = store.recorded_file_changes(root.path(), &before).unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(
            store
                .read_blob(&changes[2].after.as_ref().unwrap().hash)
                .unwrap(),
            b"latest external edit"
        );
        assert!(changes[0].after.as_ref().unwrap().binary);
        assert!(changes[1].after.is_none());
        assert_eq!(
            std::fs::read_dir(data.path().join("baselines"))
                .unwrap()
                .count(),
            0
        );
        std::fs::write(root.path().join("edited"), b"original").unwrap();
        assert_eq!(
            store
                .recorded_file_changes(root.path(), &before)
                .unwrap()
                .len(),
            2
        );
        assert!(
            store
                .recorded_file_changes(
                    root.path(),
                    &std::collections::BTreeMap::from([("../outside".into(), Value::Null)])
                )
                .is_err()
        );
    }
    #[test]
    fn whole_turn_restores_modification_addition_deletion_binary_and_persists() {
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(storage.path().into()).unwrap();
        std::fs::write(root.path().join("edited"), b"after").unwrap();
        std::fs::write(root.path().join("added"), b"\0\x01").unwrap();
        std::fs::write(root.path().join("unrelated"), b"keep").unwrap();
        let data = artifact(
            root.path(),
            json!([
                {"path":"edited","before":version(b"uncommitted before"),"after":version(b"after")},
                {"path":"added","before":null,"after":version(b"\0\x01")},
                {"path":"deleted","before":version(b"restore me"),"after":null}
            ]),
        );
        let allowed = vec![root.path().to_str().unwrap().into()];
        store
            .undo_turn("task", "artifact", &data, &allowed)
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("edited")).unwrap(),
            b"uncommitted before"
        );
        assert!(!root.path().join("added").exists());
        assert_eq!(
            std::fs::read(root.path().join("deleted")).unwrap(),
            b"restore me"
        );
        assert_eq!(
            std::fs::read(root.path().join("unrelated")).unwrap(),
            b"keep"
        );
        drop(store);
        let store = ReviewStore::open(storage.path().into()).unwrap();
        assert!(store.turn_is_undone("task", "artifact").unwrap());
        std::fs::write(root.path().join("edited"), b"later").unwrap();
        store
            .undo_turn("task", "artifact", &data, &allowed)
            .unwrap();
        assert_eq!(std::fs::read(root.path().join("edited")).unwrap(), b"later");
    }
    #[test]
    fn preflight_rejects_later_edits_incomplete_snapshots_and_outside_paths_before_any_write() {
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(storage.path().into()).unwrap();
        std::fs::write(root.path().join("first"), b"after").unwrap();
        std::fs::write(root.path().join("second"), b"later edit").unwrap();
        let mut data = artifact(
            root.path(),
            json!([
                {"path":"first","before":version(b"before"),"after":version(b"after")},
                {"path":"second","before":version(b"before"),"after":version(b"after")}
            ]),
        );
        let allowed = vec![root.path().to_str().unwrap().into()];
        assert!(
            store
                .undo_turn("task", "drift", &data, &allowed)
                .unwrap_err()
                .contains("后又有修改")
        );
        data["files"][1]["before"] = json!({"contentOmitted":true});
        assert!(store.undo_turn("task", "omitted", &data, &allowed).is_err());
        data["files"][1]["path"] = json!("../outside");
        assert!(
            store
                .undo_turn("task", "traversal", &data, &allowed)
                .is_err()
        );
        assert_eq!(std::fs::read(root.path().join("first")).unwrap(), b"after");
        assert_eq!(
            std::fs::read(root.path().join("second")).unwrap(),
            b"later edit"
        );
    }
    #[tokio::test]
    async fn actual_git_capture_undo_preserves_prior_dirty_work_and_index() {
        use agent_core::WorkspaceContextProvider;
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(root.path().join("file.txt"), b"staged\n").unwrap();
        git(&["add", "file.txt"]);
        std::fs::write(root.path().join("file.txt"), b"preexisting dirty work\n").unwrap();
        let index = std::fs::read(root.path().join(".git/index")).unwrap();
        let capture = crate::GitWorkspaceContextProvider::default()
            .begin_turn(root.path().to_str().unwrap())
            .await
            .unwrap();
        std::fs::write(root.path().join("file.txt"), b"agent change\n").unwrap();
        std::fs::write(root.path().join("binary"), b"\0\xff").unwrap();
        let mut data = capture.finish().await.unwrap();
        data["workspacePaths"] = json!([root.path()]);
        let store = ReviewStore::open(storage.path().into()).unwrap();
        store
            .undo_turn(
                "task",
                "git",
                &data,
                &[root.path().to_str().unwrap().into()],
            )
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("file.txt")).unwrap(),
            b"preexisting dirty work\n"
        );
        assert!(!root.path().join("binary").exists());
        assert_eq!(
            std::fs::read(root.path().join(".git/index")).unwrap(),
            index
        );
    }
    #[test]
    fn multiple_roots_with_same_filename_restore_together() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        for root in [first.path(), second.path()] {
            std::fs::write(root.join("same"), b"after").unwrap();
        }
        let data = json!({"schemaVersion":2,"attribution":"workspace_interval","files":[
            {"workspacePath":first.path(),"relativePath":"same","before":version(b"one"),"after":version(b"after")},
            {"workspacePath":second.path(),"relativePath":"same","before":version(b"two"),"after":version(b"after")}
        ]});
        ReviewStore::open(storage.path().into())
            .unwrap()
            .undo_turn(
                "task",
                "multi",
                &data,
                &[
                    first.path().to_str().unwrap().into(),
                    second.path().to_str().unwrap().into(),
                ],
            )
            .unwrap();
        assert_eq!(std::fs::read(first.path().join("same")).unwrap(), b"one");
        assert_eq!(std::fs::read(second.path().join("same")).unwrap(), b"two");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn git_subdirectory_handles_executable_and_symlink_without_touching_siblings() {
        use agent_core::WorkspaceContextProvider;
        use std::os::unix::fs::{PermissionsExt, symlink};
        let repo = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        let root = repo.path().join("selected");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(repo.path().join("outside"), b"untouched").unwrap();
        std::fs::write(root.join("script"), b"before").unwrap();
        std::fs::set_permissions(root.join("script"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        symlink("script", root.join("link")).unwrap();
        let capture = crate::GitWorkspaceContextProvider::default()
            .begin_turn(root.to_str().unwrap())
            .await
            .unwrap();
        std::fs::write(root.join("script"), b"after").unwrap();
        std::fs::set_permissions(root.join("script"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::remove_file(root.join("link")).unwrap();
        let mut data = capture.finish().await.unwrap();
        data["workspacePaths"] = json!([root]);
        ReviewStore::open(storage.path().into())
            .unwrap()
            .undo_turn("task", "subdir", &data, &[root.to_str().unwrap().into()])
            .unwrap();
        assert_eq!(std::fs::read(root.join("script")).unwrap(), b"before");
        assert_ne!(
            std::fs::metadata(root.join("script"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            std::fs::read_link(root.join("link")).unwrap(),
            Path::new("script")
        );
        assert_eq!(
            std::fs::read(repo.path().join("outside")).unwrap(),
            b"untouched"
        );
    }
}
