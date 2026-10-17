//! Per-task, restartable cleanup. All paths are app-owned, never workspace paths.
use super::{ReviewResult, ReviewStore, entries, read_manifest, valid_hash};
use crate::review::{digest, private_directory, sync_directory, write_once};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{Duration, Instant},
};

const INVENTORIES: &[&str] = &[
    "baselines",
    "recovery",
    "recovery-groups",
    "turn-undo",
    "index-recovery",
    "snapshot-references",
];

#[derive(Serialize, Deserialize)]
struct DeletionPlan {
    task: String,
    manifests: BTreeSet<String>,
    hashes: BTreeSet<String>,
}

fn collect_hashes(value: &Value, hashes: &mut BTreeSet<String>) {
    match value {
        Value::String(s) if valid_hash(s) => {
            hashes.insert(s.clone());
        }
        Value::Array(items) => {
            for item in items {
                collect_hashes(item, hashes);
            }
        }
        Value::Object(items) => {
            for item in items.values() {
                collect_hashes(item, hashes);
            }
        }
        _ => {}
    }
}

fn remove_if_present(path: &Path) -> ReviewResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("会话快照清理失败，请重试".into()),
    }
}

impl ReviewStore {
    pub(in crate::review) fn require_live_task(&self, task: &str) -> ReviewResult<()> {
        let marker = self
            .directory
            .join("deleted-tasks")
            .join(digest(task.as_bytes()));
        if marker.try_exists().map_err(|_| "无法读取会话删除状态")? {
            return Err("会话已彻底删除".into());
        }
        Ok(())
    }

    pub fn delete_task_data(&self, task: &str) -> ReviewResult<()> {
        if !self.maintenance || task.is_empty() {
            return Err("删除会话快照需要独占存储锁".into());
        }
        let plans = self.directory.join("task-deletions");
        let tombstones = self.directory.join("deleted-tasks");
        private_directory(&plans)?;
        private_directory(&tombstones)?;
        let id = digest(task.as_bytes());
        let path = plans.join(&id);
        let marker = tombstones.join(&id);
        let plan: DeletionPlan = if path.try_exists().map_err(|_| "无法读取快照清理记录")?
        {
            serde_json::from_slice(&read_manifest(&path, 32 * 1024 * 1024)?)
                .map_err(|_| "快照清理记录损坏")?
        } else {
            if marker.try_exists().map_err(|_| "无法读取会话删除状态")? {
                return Ok(());
            }
            let mut plan = DeletionPlan {
                task: task.into(),
                manifests: BTreeSet::new(),
                hashes: BTreeSet::new(),
            };
            // Build and persist the complete candidate list before removing any
            // references, so a crash can never strand unreferenced content.
            for inventory in INVENTORIES {
                let paths = entries(&self.directory.join(inventory))?;
                let mut owned_stems = BTreeSet::new();
                for file in &paths {
                    let name = file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or("invalid manifest name")?;
                    let stem = name.strip_suffix(".json").unwrap_or(name);
                    if !valid_hash(stem) {
                        continue;
                    }
                    let value: Value =
                        serde_json::from_slice(&read_manifest(file, 32 * 1024 * 1024)?)
                            .map_err(|_| "快照清单损坏，无法安全清理")?;
                    let owned = if *inventory == "baselines" {
                        value["root"].as_str().is_some_and(|root| {
                            digest(format!("{task}\0{root}").as_bytes()) == name
                        })
                    } else {
                        value["taskId"].as_str().or(value["task"].as_str()) == Some(task)
                    };
                    if !owned {
                        continue;
                    }
                    collect_hashes(&value, &mut plan.hashes);
                    owned_stems.insert(stem.to_owned());
                }
                for sibling in &paths {
                    let name = sibling
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or("invalid manifest name")?;
                    if owned_stems.contains(name.split('.').next().unwrap_or_default()) {
                        plan.manifests.insert(format!("{inventory}/{name}"));
                    }
                }
            }

            write_once(
                &path,
                &serde_json::to_vec(&plan).map_err(|_| "无法保存清理计划")?,
            )?;
            plan
        };
        if plan.task != task || plan.hashes.iter().any(|h| !valid_hash(h)) {
            return Err("invalid task cleanup plan".into());
        }
        for relative in &plan.manifests {
            let Some((inventory, name)) = relative.split_once('/') else {
                return Err("invalid cleanup path".into());
            };
            if !INVENTORIES.contains(&inventory)
                || name.contains('/')
                || !valid_hash(name.split('.').next().unwrap_or_default())
            {
                return Err("invalid cleanup path".into());
            }
        }
        write_once(&marker, b"deleted")?;
        for relative in &plan.manifests {
            remove_if_present(&self.directory.join(relative))?;
        }
        for inventory in INVENTORIES {
            let directory = self.directory.join(inventory);
            if directory.exists() {
                sync_directory(&directory)?;
            }
        }
        let protected = self.protected_hashes([], Instant::now() + Duration::from_secs(30))?;
        for hash in plan.hashes.difference(&protected) {
            remove_if_present(&self.directory.join("blobs").join(hash))?;
        }
        let blobs = self.directory.join("blobs");
        if blobs.exists() {
            sync_directory(&blobs)?;
        }
        remove_if_present(&path)?;
        sync_directory(&plans)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::SnapshotBudget;

    #[test]
    fn deletion_removes_owned_baselines_and_snapshots_but_preserves_shared_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source.txt"), "shared").unwrap();
        let directory = temp.path().join("review");
        let store = ReviewStore::open(directory.clone()).unwrap();
        let first = store
            .ensure_baseline("first", &root, SnapshotBudget::default())
            .unwrap();
        store
            .ensure_baseline("second", &root, SnapshotBudget::default())
            .unwrap();
        let shared = first.files.values().next().unwrap().hash.clone();
        let recovery = store
            .prepare_recovery(
                "first",
                &root,
                "source.txt",
                first.files.values().next().cloned(),
                None,
            )
            .unwrap();
        store
            .prepare_recovery(
                "second",
                &root,
                "source.txt",
                first.files.values().next().cloned(),
                None,
            )
            .unwrap();
        let unique = store.put_blob(b"first snapshot").unwrap();
        let owner = store.create_snapshot_owner("window").unwrap();
        store
            .persist_task_snapshot_references(&owner, "first", "view", [unique.clone()])
            .unwrap();
        drop(store);
        let store = ReviewStore::try_open_maintenance(directory.clone()).unwrap();
        store.delete_task_data("first").unwrap();
        store.delete_task_data("first").unwrap();
        assert!(store.load_baseline("first", &root).is_err());
        assert!(
            store
                .ensure_baseline("first", &root, SnapshotBudget::default())
                .is_err()
        );
        assert!(store.read_blob(&unique).is_err());
        assert!(store.recovery_record("first", &recovery.id).is_err());
        assert!(store.recovery_entries("first").unwrap().is_empty());
        assert_eq!(store.recovery_entries("second").unwrap().len(), 1);
        assert_eq!(store.read_blob(&shared).unwrap(), b"shared");
        assert!(store.load_baseline("second", &root).is_ok());
        assert_eq!(fs::read(root.join("source.txt")).unwrap(), b"shared");
        assert_eq!(store.storage_usage([]).unwrap().missing_references, 0);
        store.delete_task_data("second").unwrap();
        assert!(store.read_blob(&shared).is_err());
    }

    #[test]
    fn interrupted_deletion_reopens_and_reclaims_candidates_after_references_are_gone() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        let hash = store.put_blob(b"pending content").unwrap();
        let plans = temp.path().join("task-deletions");
        private_directory(&plans).unwrap();
        write_once(
            &plans.join(digest(b"task")),
            &serde_json::to_vec(&DeletionPlan {
                task: "task".into(),
                manifests: BTreeSet::new(),
                hashes: BTreeSet::from([hash.clone()]),
            })
            .unwrap(),
        )
        .unwrap();
        drop(store);
        let store = ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        store.delete_task_data("task").unwrap();
        assert!(store.read_blob(&hash).is_err());
        assert_eq!(fs::read_dir(plans).unwrap().count(), 0);
    }
}
