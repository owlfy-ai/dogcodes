//! A durable paired-path intent. It is recoverable, not filesystem-atomic:
//! failures preserve member state, and success requires both members complete.
use super::{
    RecoveryEntry, ReviewResult, ReviewStore, digest, private_directory, transaction, valid_id,
    write_once,
};
use crate::review::{FileChange, FileKind, validate_relative};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryGroup {
    pub id: String,
    pub task_id: String,
    pub members: [String; 2],
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryGroupEntry {
    pub group: RecoveryGroup,
    pub entries: [RecoveryEntry; 2],
    pub applied: bool,
    pub undone: bool,
    pub undo_started: bool,
}

impl ReviewStore {
    pub fn prepare_rename_recovery(
        &self,
        task: &str,
        root: &Path,
        change: &FileChange,
    ) -> ReviewResult<RecoveryGroup> {
        let previous = change
            .previous_path
            .as_deref()
            .ok_or("rename source missing")?;
        validate_relative(previous)?;
        validate_relative(&change.path)?;
        if previous == change.path
            || previous.starts_with(&format!("{}/", change.path))
            || change.path.starts_with(&format!("{previous}/"))
        {
            return Err("rename paths must be distinct and non-overlapping".into());
        }
        let before = change.before.as_ref().ok_or("rename baseline missing")?;
        let after = change
            .after
            .as_ref()
            .ok_or("rename working version missing")?;
        if before.kind == FileKind::Gitlink || after.kind == FileKind::Gitlink {
            return Err("submodule rename recovery unsupported".into());
        }
        let root = root.to_str().ok_or("non UTF-8 root")?;
        if transaction::current_version(self, root, previous)?.is_some()
            || transaction::current_version(self, root, &change.path)?.as_ref() != Some(after)
        {
            return Err("rename paths changed since preview".into());
        }
        // Restore the source first, then remove destination. On interruption,
        // retaining an extra copy is preferable to temporarily losing both.
        let source = self.prepare_recovery_intent(
            task,
            Path::new(root),
            previous,
            None,
            Some(before.clone()),
            true,
        )?;
        let destination = self.prepare_recovery_intent(
            task,
            Path::new(root),
            &change.path,
            Some(after.clone()),
            None,
            true,
        )?;
        let members = [source.id, destination.id];
        let id = digest(format!("{task}\0{}\0{}", members[0], members[1]).as_bytes());
        let group = RecoveryGroup {
            id,
            task_id: task.into(),
            members,
        };
        let directory = self.directory.join("recovery-groups");
        private_directory(&directory)?;
        write_once(
            &directory.join(format!("{}.json", group.id)),
            &serde_json::to_vec(&group).map_err(|_| "cannot serialize recovery group")?,
        )?;
        Ok(group)
    }

    pub fn recovery_group(
        &self,
        task: &str,
        id: &str,
    ) -> ReviewResult<(RecoveryGroup, [RecoveryEntry; 2])> {
        valid_id(id)?;
        let path = self
            .directory
            .join("recovery-groups")
            .join(format!("{id}.json"));
        let metadata = fs::symlink_metadata(&path).map_err(|_| "recovery group unavailable")?;
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err("invalid recovery group file".into());
        }
        let group: RecoveryGroup =
            serde_json::from_slice(&fs::read(path).map_err(|_| "cannot read recovery group")?)
                .map_err(|_| "invalid recovery group")?;
        let identity =
            digest(format!("{task}\0{}\0{}", group.members[0], group.members[1]).as_bytes());
        if group.id != id
            || identity != id
            || group.task_id != task
            || group.members[0] == group.members[1]
        {
            return Err("recovery group identity mismatch".into());
        }
        let first = self.recovery_record(task, &group.members[0])?;
        let second = self.recovery_record(task, &group.members[1])?;
        if first.record.root != second.record.root
            || first.record.path == second.record.path
            || first.record.original.is_some()
            || second.record.replacement.is_some()
            || first.record.replacement.is_none()
            || second.record.original.is_none()
            || !first.record.group_member
            || !second.record.group_member
            || first
                .record
                .path
                .starts_with(&format!("{}/", second.record.path))
            || second
                .record
                .path
                .starts_with(&format!("{}/", first.record.path))
        {
            return Err("invalid paired recovery intent".into());
        }
        Ok((group, [first, second]))
    }

    /// List published paired intents. Unpublished members cannot have been
    /// applied by the group API and are deliberately not actionable here.
    pub fn recovery_group_entries(&self, task: &str) -> ReviewResult<Vec<RecoveryGroupEntry>> {
        let directory = self.directory.join("recovery-groups");
        let files = match fs::read_dir(directory) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err("recovery group directory unavailable".into()),
        };
        let mut output = Vec::new();
        let mut members = std::collections::BTreeSet::new();
        for (index, file) in files.enumerate() {
            if index >= 10000 {
                return Err("recovery group listing budget exceeded".into());
            }
            let file = file.map_err(|_| "recovery group entry unavailable")?;
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            valid_id(id)?;
            let metadata =
                fs::symlink_metadata(file.path()).map_err(|_| "recovery group unavailable")?;
            if !metadata.is_file() || metadata.len() > 4096 {
                return Err("invalid recovery group file".into());
            }
            let manifest: RecoveryGroup = serde_json::from_slice(
                &fs::read(file.path()).map_err(|_| "cannot read recovery group")?,
            )
            .map_err(|_| "invalid recovery group")?;
            if manifest.task_id != task {
                continue;
            }
            let (group, entries) = self.recovery_group(task, id)?;
            for member in &group.members {
                if !members.insert(member.clone()) {
                    return Err("recovery member belongs to multiple groups".into());
                }
            }
            output.push(RecoveryGroupEntry {
                applied: entries.iter().all(|entry| entry.applied),
                undone: entries.iter().all(|entry| entry.undone),
                undo_started: self.recovery_group_undo_started(id)?
                    || entries.iter().any(|entry| entry.undo_started),
                group,
                entries,
            });
        }
        output.sort_by(|a, b| {
            b.entries[0]
                .record
                .created_at_ms
                .cmp(&a.entries[0].record.created_at_ms)
                .then_with(|| b.group.id.cmp(&a.group.id))
        });
        Ok(output)
    }

    /// Caller must hold the same workspace/index guards as single-file writes.
    pub fn apply_recovery_group(&self, task: &str, id: &str) -> ReviewResult<()> {
        let (group, entries) = self.recovery_group(task, id)?;
        if self.recovery_group_undo_started(id)? {
            return Err("paired recovery is already undoing".into());
        }
        // Check both paths before changing either one. Each member performs
        // its own content/permission validation again immediately before write.
        for entry in &entries {
            if entry.undone || entry.undo_started {
                return Err("group member is already undoing or undone".into());
            }
            let current =
                transaction::current_version(self, &entry.record.root, &entry.record.path)?;
            if (entry.applied || current != entry.record.original)
                && current != entry.record.replacement
            {
                return Err("paired recovery found newer work; no further member applied".into());
            }
        }
        for member in &group.members {
            self.reconcile_recovery(task, member)?;
        }
        let (_, entries) = self.recovery_group(task, id)?;
        if entries.iter().any(|entry| !entry.applied || entry.undone) {
            return Err("paired recovery remains incomplete".into());
        }
        Ok(())
    }

    pub fn recovery_group_undo_started(&self, id: &str) -> ReviewResult<bool> {
        valid_id(id)?;
        super::marker(
            &self
                .directory
                .join("recovery-groups")
                .join(format!("{id}.undo-started")),
            b"started",
        )
    }

    /// Undo in reverse member order so the destination is restored before the
    /// source is removed. A persistent direction marker prevents accidental redo.
    pub fn undo_recovery_group(&self, task: &str, id: &str) -> ReviewResult<()> {
        let (group, entries) = self.recovery_group(task, id)?;
        if entries.iter().any(|entry| !entry.applied) {
            return Err("finish interrupted paired recovery before undo".into());
        }
        for entry in &entries {
            let current =
                transaction::current_version(self, &entry.record.root, &entry.record.path)?;
            let matches = if entry.undone {
                current == entry.record.original
            } else if entry.undo_started {
                current == entry.record.original || current == entry.record.replacement
            } else {
                current == entry.record.replacement
            };
            if !matches {
                return Err("paired undo found newer work; refusing overwrite".into());
            }
        }
        write_once(
            &self
                .directory
                .join("recovery-groups")
                .join(format!("{id}.undo-started")),
            b"started",
        )?;
        for member in group.members.iter().rev() {
            let entry = self.recovery_record(task, member)?;
            if entry.undone {
                continue;
            }
            if entry.undo_started {
                self.reconcile_recovery(task, member)?;
            } else {
                self.undo_recovery(task, member)?;
            }
        }
        if self
            .recovery_group(task, id)?
            .1
            .iter()
            .any(|entry| !entry.undone)
        {
            return Err("paired undo remains incomplete".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{ChangeKind, FileVersion};

    #[test]
    fn unpublished_members_are_not_independent_recovery_actions() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let version = FileVersion {
            hash: store.put_blob(b"saved").unwrap(),
            bytes: 5,
            kind: FileKind::File,
            executable: false,
            binary: false,
        };
        let member = store
            .prepare_recovery_intent(
                "task",
                root.path(),
                "source",
                None,
                Some(version.clone()),
                true,
            )
            .unwrap();
        // Preparation stopped before the other member and manifest existed.
        let store = ReviewStore::open(data.path().into()).unwrap();
        assert!(store.recovery_entries("task").unwrap().is_empty());
        assert!(store.recovery_group_entries("task").unwrap().is_empty());
        assert!(
            store
                .recovery_record("task", &member.id)
                .unwrap()
                .record
                .group_member
        );
        assert!(!root.path().join("source").exists());
        let single = store
            .prepare_recovery("task", root.path(), "single", None, Some(version))
            .unwrap();
        let entries = store.recovery_entries("task").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].record.id, single.id);
    }

    #[test]
    fn paired_rename_resumes_after_first_member_and_refuses_newer_destination() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("new"), "changed").unwrap();
        let before = FileVersion {
            hash: store.put_blob(b"original").unwrap(),
            bytes: 8,
            kind: FileKind::File,
            executable: false,
            binary: false,
        };
        let after =
            transaction::current_version(&store, root.path().to_str().unwrap(), "new").unwrap();
        let change = FileChange {
            path: "new".into(),
            previous_path: Some("old".into()),
            kind: ChangeKind::Renamed,
            before: Some(before),
            after,
        };
        let group = store
            .prepare_rename_recovery("task", root.path(), &change)
            .unwrap();
        assert!(store.recovery_entries("task").unwrap().is_empty());
        let listed = store.recovery_group_entries("task").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].group.id, group.id);
        assert!(!listed[0].applied && !listed[0].undone && !listed[0].undo_started);
        assert!(
            store
                .recovery_group_entries("another-task")
                .unwrap()
                .is_empty()
        );
        // Simulate process termination after the source path was restored.
        store.apply_recovery("task", &group.members[0]).unwrap();
        assert!(!store.recovery_group_entries("task").unwrap()[0].applied);
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("new"), "newer work").unwrap();
        assert!(reopened.apply_recovery_group("task", &group.id).is_err());
        assert_eq!(fs::read(root.path().join("old")).unwrap(), b"original");
        assert_eq!(fs::read(root.path().join("new")).unwrap(), b"newer work");
        fs::write(root.path().join("new"), "changed").unwrap();
        reopened.apply_recovery_group("task", &group.id).unwrap();
        assert!(!root.path().join("new").exists());
        assert!(
            reopened
                .recovery_group("task", &group.id)
                .unwrap()
                .1
                .iter()
                .all(|entry| entry.applied)
        );
        assert!(reopened.recovery_group("other-task", &group.id).is_err());
        // Persist undo direction and emulate interruption after restoring new.
        write_once(
            &data
                .path()
                .join("recovery-groups")
                .join(format!("{}.undo-started", group.id)),
            b"started",
        )
        .unwrap();
        reopened.undo_recovery("task", &group.members[1]).unwrap();
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        assert!(reopened.apply_recovery_group("task", &group.id).is_err());
        fs::write(root.path().join("old"), "new source work").unwrap();
        assert!(reopened.undo_recovery_group("task", &group.id).is_err());
        assert_eq!(fs::read(root.path().join("new")).unwrap(), b"changed");
        assert_eq!(
            fs::read(root.path().join("old")).unwrap(),
            b"new source work"
        );
        fs::write(root.path().join("old"), "original").unwrap();
        reopened.undo_recovery_group("task", &group.id).unwrap();
        let listed = reopened.recovery_group_entries("task").unwrap();
        assert!(listed[0].applied && listed[0].undone && listed[0].undo_started);
        assert!(!root.path().join("old").exists());
        assert_eq!(fs::read(root.path().join("new")).unwrap(), b"changed");
        assert!(
            reopened
                .recovery_group("task", &group.id)
                .unwrap()
                .1
                .iter()
                .all(|entry| entry.undone)
        );
    }
}
