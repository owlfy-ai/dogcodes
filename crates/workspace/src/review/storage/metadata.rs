//! Released payloads/marker files can be discarded after their identities are
//! committed to the compact retirement index, preventing identity reuse.
use super::{
    ReviewResult, ReviewStore, SnapshotReferences, read_manifest, snapshot_released, valid_hash,
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

const SNAPSHOT_CLEANUP_BATCH: usize = 256;

#[derive(Clone)]
pub struct MetadataCleanupPlan {
    pub count: usize,
    pub bytes: u64,
    directory: PathBuf,
    candidates: BTreeMap<String, (Option<String>, SystemTime)>,
    owners: BTreeMap<String, SystemTime>,
}

impl ReviewStore {
    pub fn preview_metadata_cleanup(&self) -> ReviewResult<MetadataCleanupPlan> {
        self.preview_metadata_cleanup_at(SystemTime::now())
    }

    fn preview_metadata_cleanup_at(&self, now: SystemTime) -> ReviewResult<MetadataCleanupPlan> {
        if !self.maintenance {
            return Err("metadata cleanup requires exclusive storage access".into());
        }
        let cutoff = now
            .checked_sub(Duration::from_hours(24 * 30))
            .ok_or("metadata clock unavailable")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut plan = MetadataCleanupPlan {
            count: 0,
            bytes: 0,
            directory: self.directory.clone(),
            candidates: BTreeMap::new(),
            owners: super::owners::preview(self, cutoff)?,
        };
        let references = match fs::read_dir(self.directory.join("snapshot-references")) {
            Ok(references) => references,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Stores that have never published a snapshot can still contain
                // retired owners eligible for maintenance.
                plan.count = plan.owners.len();
                return Ok(plan);
            }
            Err(_) => return Err("snapshot reference directory unavailable".into()),
        };
        for entry in references {
            if plan.candidates.len() >= SNAPSHOT_CLEANUP_BATCH {
                break;
            }
            if Instant::now() >= deadline {
                if !plan.candidates.is_empty() {
                    break;
                }
                return Err("metadata preview budget exceeded without eligible candidates".into());
            }
            let marker = entry
                .map_err(|_| "snapshot reference entry unavailable")?
                .path();
            if marker.extension().and_then(|value| value.to_str()) != Some("released") {
                continue;
            }
            let name = marker
                .file_stem()
                .and_then(|n| n.to_str())
                .ok_or("invalid metadata name")?;
            let path = marker.with_extension("");
            if !valid_hash(name) || !snapshot_released(&path)? {
                continue;
            }
            let marker_time = fs::symlink_metadata(path.with_extension("released"))
                .and_then(|m| m.modified())
                .map_err(|_| "release timestamp unavailable")?;
            if marker_time >= cutoff {
                continue;
            }
            if let Err(error) = fs::symlink_metadata(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err("snapshot payload metadata unavailable".into());
                }
                plan.bytes += 8;
                plan.candidates.insert(name.into(), (None, marker_time));
                continue;
            }
            let bytes = read_manifest(&path, 16 * 1024 * 1024)?;
            let record: SnapshotReferences =
                serde_json::from_slice(&bytes).map_err(|_| "invalid snapshot reference payload")?;
            if super::super::digest(record.snapshot_id.as_bytes()) != name {
                return Err("snapshot identity mismatch".into());
            }
            plan.bytes += bytes.len() as u64 + 8;
            plan.candidates.insert(
                name.into(),
                (Some(super::super::digest(&bytes)), marker_time),
            );
        }
        plan.count = plan.candidates.len() + plan.owners.len();
        Ok(plan)
    }

    pub(super) fn validate_metadata_cleanup(&self, plan: &MetadataCleanupPlan) -> ReviewResult<()> {
        if !self.maintenance || self.directory != plan.directory {
            return Err("invalid metadata cleanup store".into());
        }
        super::owners::validate(self, &plan.owners)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        // Revalidate every payload before deleting any. Only exact server-held
        // candidates are accepted; callers cannot deserialize arbitrary paths.
        for (name, (hash, marker_time)) in &plan.candidates {
            if Instant::now() >= deadline {
                return Err("metadata validation timed out; nothing removed".into());
            }
            let path = self.directory.join("snapshot-references").join(name);
            if !snapshot_released(&path)?
                || fs::symlink_metadata(path.with_extension("released"))
                    .and_then(|m| m.modified())
                    .ok()
                    != Some(*marker_time)
            {
                return Err("metadata cleanup preview is stale".into());
            }
            match hash {
                Some(hash)
                    if super::super::digest(&read_manifest(&path, 16 * 1024 * 1024)?) == *hash => {}
                None if fs::symlink_metadata(&path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                _ => return Err("metadata cleanup payload state changed".into()),
            }
        }
        Ok(())
    }

    pub fn execute_metadata_cleanup(&self, plan: &MetadataCleanupPlan) -> ReviewResult<usize> {
        self.validate_metadata_cleanup(plan)?;
        super::retired::insert(&self.directory, plan.candidates.keys().cloned())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut removed = 0;
        for (name, (hash, _)) in &plan.candidates {
            if Instant::now() >= deadline {
                return Err(format!(
                    "metadata cleanup timed out after removing {removed} payloads"
                ));
            }
            if hash.is_some() {
                fs::remove_file(self.directory.join("snapshot-references").join(name)).map_err(
                    |_| format!("metadata cleanup failed after removing {removed} records"),
                )?;
            }
            removed += 1;
            fs::remove_file(
                self.directory
                    .join("snapshot-references")
                    .join(name)
                    .with_extension("released"),
            )
            .map_err(|_| {
                format!("metadata markers cleanup failed after removing {removed} payloads")
            })?;
        }
        if removed > 0 {
            super::super::sync_directory(&self.directory.join("snapshot-references"))?;
        }
        let owners = super::owners::remove(self, &plan.owners)
            .map_err(|error| format!("{error}; removed {removed} snapshot records"))?;
        Ok(removed + owners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn released_snapshots_are_retired_in_bounded_resumable_batches() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let blob = store.put_blob(b"retained reader").unwrap();
        store
            .persist_snapshot_references("live", [blob.clone()])
            .unwrap();
        let old = SystemTime::now() - Duration::from_hours(24 * 31);
        for index in 0..260 {
            let id = format!("released-{index}");
            store
                .persist_snapshot_references(&id, [blob.clone()])
                .unwrap();
            store.release_snapshot_references(&id).unwrap();
            let path = directory
                .path()
                .join("snapshot-references")
                .join(super::super::super::digest(id.as_bytes()))
                .with_extension("released");
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        // Exercise the same combined content/metadata plan used by the desktop,
        // rather than proving only the standalone metadata helper can advance.
        let first = store.preview_storage_cleanup().unwrap();
        assert_eq!(first.metadata.count, SNAPSHOT_CLEANUP_BATCH);
        assert_eq!(
            store
                .execute_storage_cleanup(&first)
                .unwrap()
                .removed_metadata_count,
            SNAPSHOT_CLEANUP_BATCH
        );
        drop(store);
        let reopened = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let second = reopened.preview_storage_cleanup().unwrap();
        assert_eq!(second.metadata.count, 4);
        assert_eq!(
            reopened
                .execute_storage_cleanup(&second)
                .unwrap()
                .removed_metadata_count,
            4
        );
        assert_eq!(reopened.preview_metadata_cleanup().unwrap().count, 0);
        assert_eq!(reopened.storage_usage([]).unwrap().missing_references, 0);
        assert_eq!(reopened.read_blob(&blob).unwrap(), b"retained reader");
        for index in 0..260 {
            assert!(
                reopened
                    .persist_snapshot_references(&format!("released-{index}"), [blob.clone()])
                    .is_err()
            );
        }
        // Live identity is neither retired nor removed during batched cleanup.
        reopened
            .persist_snapshot_references("live", [blob])
            .unwrap();
    }

    #[test]
    fn orphan_markers_are_compacted_but_reappearing_payloads_reject_the_plan() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let blob = store.put_blob(b"keep").unwrap();
        store
            .persist_snapshot_references("orphan", [blob.clone()])
            .unwrap();
        store.release_snapshot_references("orphan").unwrap();
        let path = directory
            .path()
            .join("snapshot-references")
            .join(super::super::super::digest(b"orphan"));
        fs::File::options()
            .write(true)
            .open(path.with_extension("released"))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(24 * 31))
            .unwrap();
        fs::remove_file(&path).unwrap();
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(plan.count, 1);
        assert_eq!(plan.bytes, 8);
        fs::write(&path, b"new state").unwrap();
        assert!(store.execute_metadata_cleanup(&plan).is_err());
        assert!(path.with_extension("released").exists());
        fs::remove_file(&path).unwrap();
        assert_eq!(store.execute_metadata_cleanup(&plan).unwrap(), 1);
        assert!(!path.with_extension("released").exists());
        drop(store);
        let reopened = ReviewStore::open(directory.path().into()).unwrap();
        assert!(
            reopened
                .persist_snapshot_references("orphan", [blob])
                .is_err()
        );
    }

    #[test]
    fn stale_metadata_plan_rejects_before_deleting_other_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let blob = store.put_blob(b"keep").unwrap();
        let paths: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|id| {
                store
                    .persist_snapshot_references(id, [blob.clone()])
                    .unwrap();
                store.release_snapshot_references(id).unwrap();
                let path = directory
                    .path()
                    .join("snapshot-references")
                    .join(super::super::super::digest(id.as_bytes()));
                fs::File::options()
                    .write(true)
                    .open(path.with_extension("released"))
                    .unwrap()
                    .set_modified(SystemTime::now() - Duration::from_hours(24 * 31))
                    .unwrap();
                path
            })
            .collect();
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(plan.count, 2);
        let unused = store.put_blob(b"unused old content").unwrap();
        let unused_path = directory.path().join("blobs").join(unused);
        fs::File::options()
            .write(true)
            .open(&unused_path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(24 * 31))
            .unwrap();
        let combined = store.preview_storage_cleanup().unwrap();
        assert_eq!(combined.content.count, 1);
        fs::write(&paths[1], b"changed after preview").unwrap();
        assert!(store.execute_metadata_cleanup(&plan).is_err());
        assert!(store.execute_storage_cleanup(&combined).is_err());
        assert!(unused_path.exists());
        assert!(paths.iter().all(|path| path.exists()));
        assert_eq!(store.read_blob(&blob).unwrap(), b"keep");
    }

    #[test]
    fn only_old_released_payloads_are_removed_and_ids_stay_retired() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let blob = store.put_blob(b"original").unwrap();
        for id in ["live", "recent", "old"] {
            store
                .persist_snapshot_references(id, [blob.clone()])
                .unwrap();
        }
        store.release_snapshot_references("recent").unwrap();
        store.release_snapshot_references("old").unwrap();
        let old = directory
            .path()
            .join("snapshot-references")
            .join(super::super::super::digest(b"old"));
        fs::File::options()
            .write(true)
            .open(old.with_extension("released"))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(24 * 31))
            .unwrap();
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(plan.count, 1);
        let combined = store.preview_storage_cleanup().unwrap();
        let result = store.execute_storage_cleanup(&combined).unwrap();
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.removed_metadata_count, 1);
        assert_eq!(result.removed_bytes, plan.bytes);
        assert!(!old.exists());
        assert!(!old.with_extension("released").exists());
        assert!(snapshot_released(&old).unwrap());
        assert!(
            store
                .persist_snapshot_references("old", [blob.clone()])
                .is_err()
        );
        assert_eq!(store.read_blob(&blob).unwrap(), b"original");
        assert_eq!(store.preview_metadata_cleanup().unwrap().count, 0);
        assert_eq!(store.storage_usage([]).unwrap().protected_bytes, 8);
    }
}
