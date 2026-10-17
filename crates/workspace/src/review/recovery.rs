//! Durable recovery intent is written before changing a workspace file.
//! Completion is a separate immutable marker: a crash never erases intent.
pub mod batch;
pub mod group;
pub mod index;
mod transaction;
pub mod turn;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod xattrs;
use super::{
    FileVersion, ReviewResult, ReviewStore, canonical_root, digest, private_directory,
    validate_relative, write_once,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOwner {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRecord {
    pub id: String,
    pub task_id: String,
    pub root: String,
    pub path: String,
    /// Original working file, including absence, captured before the operation.
    pub original: Option<FileVersion>,
    /// The state the user approved replacing it with.
    pub replacement: Option<FileVersion>,
    pub created_at_ms: u128,
    pub original_mode: Option<u32>,
    /// Content-addressed extended attributes, separate from the bounded intent.
    pub original_xattrs: Option<String>,
    pub original_acl: Option<String>,
    pub original_owner: Option<FileOwner>,
    /// Group members are never offered as independent user operations, even
    /// when preparation stops before the group manifest has been published.
    pub group_member: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryEntry {
    pub record: RecoveryRecord,
    pub applied: bool,
    pub undone: bool,
    pub undo_started: bool,
}

fn valid_id(id: &str) -> ReviewResult<()> {
    if id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("invalid recovery record ID".into())
    }
}

pub(crate) fn marker(path: &Path, expected: &[u8]) -> ReviewResult<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("recovery marker unavailable".into()),
    };
    if !metadata.is_file()
        || metadata.len() != expected.len() as u64
        || fs::read(path).map_err(|_| "recovery marker unavailable")? != expected
    {
        return Err("corrupted recovery marker".into());
    }
    Ok(true)
}

impl ReviewStore {
    pub fn recovery_entries(&self, task_id: &str) -> ReviewResult<Vec<RecoveryEntry>> {
        let directory = self.directory.join("recovery");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err("recovery directory unavailable".into()),
        };
        let mut output = Vec::new();
        let mut scanned = 0;
        for entry in entries {
            scanned += 1;
            if scanned > 10000 {
                return Err("recovery listing budget exceeded".into());
            }
            let entry = entry.map_err(|_| "recovery entry unavailable")?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if valid_id(&id).is_err() {
                continue;
            }
            // Parse task ownership before doing the more expensive blob checks.
            let metadata = entry
                .file_type()
                .map_err(|_| "recovery entry unavailable")?;
            if !metadata.is_file()
                || entry
                    .metadata()
                    .map_err(|_| "recovery entry unavailable")?
                    .len()
                    > 65536
            {
                return Err("invalid recovery entry".into());
            }
            let record: RecoveryRecord = serde_json::from_slice(
                &fs::read(entry.path()).map_err(|_| "recovery entry unavailable")?,
            )
            .map_err(|_| "invalid recovery entry")?;
            if record.task_id == task_id && !record.group_member {
                output.push(self.recovery_record(task_id, &id)?);
            }
        }
        output.sort_by(|a, b| {
            b.record
                .created_at_ms
                .cmp(&a.record.created_at_ms)
                .then_with(|| b.record.id.cmp(&a.record.id))
        });
        Ok(output)
    }
    pub fn prepare_recovery(
        &self,
        task_id: &str,
        root: &Path,
        path: &str,
        original: Option<FileVersion>,
        replacement: Option<FileVersion>,
    ) -> ReviewResult<RecoveryRecord> {
        self.prepare_recovery_intent(task_id, root, path, original, replacement, false)
    }

    fn prepare_recovery_intent(
        &self,
        task_id: &str,
        root: &Path,
        path: &str,
        original: Option<FileVersion>,
        replacement: Option<FileVersion>,
        group_member: bool,
    ) -> ReviewResult<RecoveryRecord> {
        validate_relative(path)?;
        if task_id.is_empty() || task_id.contains('\0') {
            return Err("invalid recovery task".into());
        }
        let root = canonical_root(root)?
            .to_str()
            .ok_or("non UTF-8 root")?
            .to_owned();
        if original == replacement {
            return Err("file already matches requested state".into());
        }
        // Validate both backup and replacement blobs before granting intent.
        for version in original.iter().chain(&replacement) {
            if version.kind == super::FileKind::Gitlink {
                return Err("submodule pointers cannot be restored as ordinary files".into());
            }
            if self.read_blob(&version.hash)?.len() as u64 != version.bytes {
                return Err("backup content size mismatch".into());
            }
        }
        let directory = self.directory.join("recovery");
        private_directory(&directory)?;
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock unavailable")?
            .as_millis();
        let identity = format!(
            "{task_id}\0{root}\0{path}\0{created_at_ms}\0{}\0{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let id = digest(identity.as_bytes());
        // New files have no original permissions. Their parent may still be a
        // file scheduled for removal by an enclosing topology transaction.
        // Execution still validates the actual parent and expected absence.
        let original_mode = if original.is_some() {
            transaction::file_mode(&root, path)?
        } else {
            None
        };
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let original_xattrs = if original
            .as_ref()
            .is_some_and(transaction::supports_metadata)
        {
            Some(
                self.put_blob(
                    &serde_json::to_vec(&transaction::capture_attributes(&root, path)?)
                        .map_err(|_| "cannot serialize extended attributes")?,
                )?,
            )
        } else {
            None
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let original_xattrs = None;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let original_owner = if original
            .as_ref()
            .is_some_and(transaction::supports_metadata)
        {
            Some(transaction::capture_owner(&root, path)?)
        } else {
            None
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let original_owner = None;
        #[cfg(target_os = "macos")]
        let original_acl = if original
            .as_ref()
            .is_some_and(transaction::supports_metadata)
        {
            Some(self.put_blob(&transaction::capture_acl(&root, path)?)?)
        } else {
            None
        };
        #[cfg(not(target_os = "macos"))]
        let original_acl = None;
        let record = RecoveryRecord {
            id: id.clone(),
            task_id: task_id.into(),
            root,
            path: path.into(),
            original,
            replacement,
            created_at_ms,
            original_mode,
            original_xattrs,
            original_acl,
            original_owner,
            group_member,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| "cannot serialize recovery record")?;
        if !write_once(&directory.join(&id), &bytes)? {
            return Err("recovery record collision".into());
        }
        Ok(record)
    }

    pub fn recovery_record(&self, task_id: &str, id: &str) -> ReviewResult<RecoveryEntry> {
        valid_id(id)?;
        let directory = self.directory.join("recovery");
        let path = directory.join(id);
        let metadata = fs::symlink_metadata(&path).map_err(|_| "recovery record unavailable")?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            return Err("invalid recovery record".into());
        }
        let record: RecoveryRecord =
            serde_json::from_slice(&fs::read(&path).map_err(|_| "cannot read recovery record")?)
                .map_err(|_| "invalid recovery record")?;
        if record.id != id || record.task_id != task_id {
            return Err("recovery record belongs to another task".into());
        }
        validate_relative(&record.path)?;
        for version in record.original.iter().chain(&record.replacement) {
            self.read_blob(&version.hash)?;
        }
        Ok(RecoveryEntry {
            record,
            applied: marker(&directory.join(format!("{id}.applied")), b"applied")?,
            undone: marker(&directory.join(format!("{id}.undone")), b"undone")?,
            undo_started: marker(&directory.join(format!("{id}.undo-started")), b"started")?,
        })
    }

    /// Call only after the filesystem transaction was verified to have applied.
    pub fn mark_recovery_applied(&self, task_id: &str, id: &str) -> ReviewResult<()> {
        let entry = self.recovery_record(task_id, id)?;
        if entry.undone {
            return Err("recovery already undone".into());
        }
        write_once(
            &self
                .directory
                .join("recovery")
                .join(format!("{id}.applied")),
            b"applied",
        )?;
        Ok(())
    }

    /// Undo must compare the disk state to the approved replacement, not simply
    /// overwrite whatever happens to occupy the same path today.
    pub fn check_recovery_undo(
        &self,
        task_id: &str,
        id: &str,
        current: &Option<FileVersion>,
    ) -> ReviewResult<RecoveryRecord> {
        let entry = self.recovery_record(task_id, id)?;
        if entry.undone {
            return Err("recovery already undone".into());
        }
        if !entry.applied {
            return Err("operation completion unknown; reconcile before undo".into());
        }
        if &entry.record.replacement != current {
            return Err("file changed after operation; refusing to overwrite new work".into());
        }
        Ok(entry.record)
    }

    pub fn mark_recovery_undone(&self, task_id: &str, id: &str) -> ReviewResult<()> {
        let entry = self.recovery_record(task_id, id)?;
        if !entry.applied {
            return Err("operation was not marked applied".into());
        }
        write_once(
            &self.directory.join("recovery").join(format!("{id}.undone")),
            b"undone",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::FileKind;
    fn version(store: &ReviewStore, bytes: &[u8]) -> FileVersion {
        FileVersion {
            hash: store.put_blob(bytes).unwrap(),
            bytes: bytes.len() as u64,
            kind: FileKind::File,
            executable: false,
            binary: false,
        }
    }

    #[test]
    fn prepares_new_file_below_blocker_but_writes_only_after_parent_is_safe() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("parent"), b"blocker").unwrap();
        let replacement = version(&store, b"new content");
        let record = store
            .prepare_recovery_intent(
                "task",
                root.path(),
                "parent/new",
                None,
                Some(replacement),
                true,
            )
            .unwrap();
        let mut incomplete = serde_json::to_value(&record).unwrap();
        incomplete.as_object_mut().unwrap().remove("groupMember");
        assert!(serde_json::from_value::<RecoveryRecord>(incomplete).is_err());
        assert!(record.original_mode.is_none());
        assert!(store.recovery_entries("task").unwrap().is_empty());
        assert!(store.apply_recovery("task", &record.id).is_err());
        assert_eq!(fs::read(root.path().join("parent")).unwrap(), b"blocker");
        fs::remove_file(root.path().join("parent")).unwrap();
        fs::create_dir(root.path().join("parent")).unwrap();
        store.apply_recovery("task", &record.id).unwrap();
        assert_eq!(
            fs::read(root.path().join("parent/new")).unwrap(),
            b"new content"
        );
        store.undo_recovery("task", &record.id).unwrap();
        assert!(!root.path().join("parent/new").exists());
    }

    #[test]
    fn recovery_survives_reopen_and_refuses_newer_edits() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let original = version(&store, b"user work");
        let replacement = version(&store, b"baseline");
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/a"), b"user work").unwrap();
        let record = store
            .prepare_recovery(
                "task",
                root.path(),
                "src/a",
                Some(original.clone()),
                Some(replacement.clone()),
            )
            .unwrap();
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        assert!(
            reopened
                .check_recovery_undo("task", &record.id, &Some(replacement.clone()))
                .is_err()
        );
        reopened.mark_recovery_applied("task", &record.id).unwrap();
        assert!(
            reopened
                .check_recovery_undo("task", &record.id, &Some(version(&store, b"new work")))
                .is_err()
        );
        let undo = reopened
            .check_recovery_undo("task", &record.id, &Some(replacement))
            .unwrap();
        assert_eq!(undo.original, Some(original));
        reopened.mark_recovery_undone("task", &record.id).unwrap();
        assert!(reopened.recovery_record("task", &record.id).unwrap().undone);
    }

    #[test]
    fn validates_task_path_and_backup_content_before_recording_intent() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let original = version(&store, b"created file");
        fs::write(root.path().join("new"), b"created file").unwrap();
        assert!(
            store
                .prepare_recovery(
                    "task",
                    root.path(),
                    "../escape",
                    Some(original.clone()),
                    None
                )
                .is_err()
        );
        let record = store
            .prepare_recovery("task", root.path(), "new", Some(original), None)
            .unwrap();
        assert!(store.recovery_record("other", &record.id).is_err());
        assert!(store.recovery_record("task", "../escape").is_err());
        store.mark_recovery_applied("task", &record.id).unwrap();
        assert!(store.check_recovery_undo("task", &record.id, &None).is_ok());
    }
}
