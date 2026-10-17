use super::{ReviewResult, ReviewStore, stream_entries, valid_hash};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

const RETENTION: Duration = Duration::from_hours(168);
const CLEANUP_BATCH: usize = 1024;
const CLEANUP_BATCH_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    pub count: usize,
    pub bytes: u64,
    #[serde(skip)]
    directory: PathBuf,
    #[serde(skip)]
    candidates: BTreeMap<String, (u64, SystemTime)>,
    #[serde(skip)]
    cutoff: SystemTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub removed_count: usize,
    pub removed_bytes: u64,
}

impl ReviewStore {
    pub fn preview_cleanup(&self) -> ReviewResult<CleanupPlan> {
        self.preview_cleanup_at(SystemTime::now())
    }

    fn preview_cleanup_at(&self, now: SystemTime) -> ReviewResult<CleanupPlan> {
        if !self.maintenance {
            return Err("cleanup requires exclusive storage access".into());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let protected = self.protected_hashes([], deadline)?;
        let cutoff = now
            .checked_sub(RETENTION)
            .ok_or("cleanup clock unavailable")?;
        let mut plan = CleanupPlan {
            count: 0,
            bytes: 0,
            directory: self.directory.clone(),
            candidates: BTreeMap::new(),
            cutoff,
        };
        for path in stream_entries(&self.directory.join("blobs"))? {
            if plan.candidates.len() >= CLEANUP_BATCH || plan.bytes >= CLEANUP_BATCH_BYTES {
                break;
            }
            if Instant::now() >= deadline {
                // Reference inventory has already completed. A partial content
                // candidate batch is safe; a partial reference set never is.
                if !plan.candidates.is_empty() {
                    break;
                }
                return Err("cleanup preview timed out".into());
            }
            let path = path?;
            let hash = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid blob name")?;
            if !valid_hash(hash) || protected.contains(hash) {
                continue;
            }
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| "cleanup content unavailable")?;
            if !metadata.is_file() {
                return Err("invalid cleanup content".into());
            }
            let modified = metadata
                .modified()
                .map_err(|_| "cleanup content time unavailable")?;
            if modified >= cutoff {
                continue;
            }
            if !plan.candidates.is_empty()
                && metadata.len() > CLEANUP_BATCH_BYTES.saturating_sub(plan.bytes)
            {
                break;
            }
            plan.bytes = plan
                .bytes
                .checked_add(metadata.len())
                .ok_or("cleanup size overflow")?;
            plan.candidates
                .insert(hash.into(), (metadata.len(), modified));
        }
        plan.count = plan.candidates.len();
        Ok(plan)
    }

    /// Only a server-held plan may be executed; recompute references after the
    /// user confirms, and validate every candidate before removing any content.
    pub fn execute_cleanup(&self, plan: &CleanupPlan) -> ReviewResult<CleanupResult> {
        if !self.maintenance || self.directory != plan.directory {
            return Err("invalid cleanup store or lock".into());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let protected = self.protected_hashes([], deadline)?;
        for (hash, (bytes, modified)) in &plan.candidates {
            if Instant::now() >= deadline {
                return Err("cleanup validation timed out; nothing removed".into());
            }
            if protected.contains(hash) {
                return Err("cleanup preview is stale: content is now protected".into());
            }
            let path = self.directory.join("blobs").join(hash);
            let metadata = fs::symlink_metadata(&path).map_err(|_| "cleanup preview is stale")?;
            if !metadata.is_file()
                || metadata.len() != *bytes
                || metadata.modified().ok() != Some(*modified)
                || *modified >= plan.cutoff
            {
                return Err("cleanup preview is stale: content changed".into());
            }
            self.read_blob(hash)?;
        }
        let mut result = CleanupResult {
            removed_count: 0,
            removed_bytes: 0,
        };
        for (hash, (bytes, _)) in &plan.candidates {
            if Instant::now() >= deadline {
                return Err(format!(
                    "cleanup timed out after removing {} objects ({} bytes)",
                    result.removed_count, result.removed_bytes
                ));
            }
            fs::remove_file(self.directory.join("blobs").join(hash)).map_err(|_| {
                format!(
                    "cleanup interrupted after removing {} objects ({} bytes)",
                    result.removed_count, result.removed_bytes
                )
            })?;
            result.removed_count += 1;
            result.removed_bytes += bytes;
        }
        super::super::sync_directory(&self.directory.join("blobs")).map_err(|error| {
            format!(
                "{error}; removed {} objects ({} bytes), directory durability unconfirmed",
                result.removed_count, result.removed_bytes
            )
        })?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_cleanup_byte_budget_bounds_preview_and_still_checks_hashes() {
        let fixture = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(fixture.path().into()).unwrap();
        fs::create_dir_all(fixture.path().join("blobs")).unwrap();
        let old = SystemTime::now() - Duration::from_hours(24 * 8);
        // Sparse fixture sizes exercise planning without allocating large test
        // buffers. Their deliberately invalid digests must prevent execution.
        for (name, mib) in [("a", 33), ("b", 33), ("c", 65)] {
            let file =
                fs::File::create(fixture.path().join("blobs").join(name.repeat(64))).unwrap();
            file.set_len(mib * 1024 * 1024).unwrap();
            file.set_modified(old).unwrap();
        }
        for remaining in (1..=3).rev() {
            let plan = store.preview_cleanup().unwrap();
            assert_eq!(plan.count, 1);
            assert!(plan.bytes == 33 * 1024 * 1024 || plan.bytes == 65 * 1024 * 1024);
            assert!(store.execute_cleanup(&plan).is_err());
            assert_eq!(
                fs::read_dir(fixture.path().join("blobs")).unwrap().count(),
                remaining
            );
            // Advance this isolated invalid-content fixture manually; a failed
            // production cleanup must not remove corrupt candidates for us.
            let hash = plan.candidates.keys().next().unwrap();
            fs::remove_file(fixture.path().join("blobs").join(hash)).unwrap();
        }
    }

    #[test]
    fn content_cleanup_batches_survive_reopen_and_preserve_live_content() {
        let fixture = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(fixture.path().into()).unwrap();
        let live = store.put_blob(b"live reader").unwrap();
        store
            .persist_snapshot_references("reader", [live.clone()])
            .unwrap();
        let old = SystemTime::now() - Duration::from_hours(24 * 8);
        let mut hashes = Vec::new();
        for index in 0..CLEANUP_BATCH + 3 {
            let hash = store
                .put_blob(format!("old content {index}").as_bytes())
                .unwrap();
            fs::File::open(fixture.path().join("blobs").join(&hash))
                .unwrap()
                .set_modified(old)
                .unwrap();
            hashes.push(hash);
        }
        fs::File::open(fixture.path().join("blobs").join(&live))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let plan = store.preview_storage_cleanup().unwrap();
        assert_eq!(plan.content.count, CLEANUP_BATCH);
        assert_eq!(
            store.execute_storage_cleanup(&plan).unwrap().removed_count,
            CLEANUP_BATCH
        );
        drop(store);
        let store = ReviewStore::try_open_maintenance(fixture.path().into()).unwrap();
        let plan = store.preview_storage_cleanup().unwrap();
        assert_eq!(plan.content.count, 3);
        assert_eq!(
            store.execute_storage_cleanup(&plan).unwrap().removed_count,
            3
        );
        assert_eq!(store.preview_storage_cleanup().unwrap().content.count, 0);
        assert_eq!(store.read_blob(&live).unwrap(), b"live reader");
        assert!(
            hashes
                .iter()
                .all(|hash| !fixture.path().join("blobs").join(hash).exists())
        );
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.blob_count, 1);
        assert_eq!(usage.missing_references, 0);
    }

    #[test]
    fn many_refreshes_cleanup_preserves_baseline_and_frozen_reader_after_reopen() {
        use crate::review::{SnapshotBudget, compare_snapshots};
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("notes");
        fs::create_dir(&root).unwrap();
        let data = fixture.path().join("store");
        let store = ReviewStore::open(data.clone()).unwrap();
        for file in 0..8 {
            fs::write(root.join(format!("{file}.txt")), format!("base {file}\n")).unwrap();
        }
        let baseline = store
            .ensure_baseline("task", &root, SnapshotBudget::default())
            .unwrap();
        assert!(baseline.complete);
        let mut frozen = None;
        let mut latest = None;
        for round in 0..64 {
            for file in 0..8 {
                fs::write(
                    root.join(format!("{file}.txt")),
                    format!("round {round} file {file}\n"),
                )
                .unwrap();
            }
            let snapshot = store.scan(&root, SnapshotBudget::default()).unwrap();
            assert!(snapshot.complete);
            store
                .persist_snapshot_references(
                    &format!("view-{round}"),
                    snapshot.files.values().map(|file| file.hash.clone()),
                )
                .unwrap();
            if round > 1 {
                store
                    .release_snapshot_references(&format!("view-{}", round - 1))
                    .unwrap();
            }
            if round == 0 {
                frozen = Some(snapshot.clone());
            }
            latest = Some(snapshot);
        }
        assert_eq!(store.storage_usage([]).unwrap().blob_count, 520);
        drop(store);
        let store = ReviewStore::try_open_maintenance(data.clone()).unwrap();
        // Advance only fixture timestamps, not the system clock or retention.
        let now = SystemTime::now();
        for entry in fs::read_dir(data.join("blobs")).unwrap() {
            fs::File::options()
                .write(true)
                .open(entry.unwrap().path())
                .unwrap()
                .set_modified(now - RETENTION - Duration::from_mins(1))
                .unwrap();
        }
        let plan = store.preview_cleanup_at(now).unwrap();
        assert_eq!(plan.count, 496);
        assert_eq!(store.execute_cleanup(&plan).unwrap().removed_count, 496);
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.blob_count, 24);
        assert_eq!(usage.missing_references, 0);
        assert_eq!(store.preview_cleanup_at(now).unwrap().count, 0);
        drop(store);
        let reopened = ReviewStore::open(data).unwrap();
        let baseline_again = reopened.load_baseline("task", &root).unwrap();
        assert_eq!(baseline_again.files, baseline.files);
        for snapshot in [
            &baseline_again,
            frozen.as_ref().unwrap(),
            latest.as_ref().unwrap(),
        ] {
            for file in snapshot.files.values() {
                assert!(!reopened.read_blob(&file.hash).unwrap().is_empty());
            }
        }
        assert_eq!(
            compare_snapshots(&baseline_again, latest.as_ref().unwrap())
                .unwrap()
                .len(),
            8
        );
        assert!(!root.join(".git").exists());
    }

    #[test]
    fn corrupted_candidate_aborts_before_removing_any_other_candidate() {
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        let now = SystemTime::now();
        for bytes in [b"aaa", b"bbb"] {
            let hash = store.put_blob(bytes).unwrap();
            let file = fs::File::options()
                .write(true)
                .open(data.path().join("blobs").join(hash))
                .unwrap();
            file.set_modified(now - RETENTION - Duration::from_mins(1))
                .unwrap();
        }
        let plan = store.preview_cleanup_at(now).unwrap();
        assert_eq!(plan.count, 2);
        let (last, (_, modified)) = plan.candidates.last_key_value().unwrap();
        let path = data.path().join("blobs").join(last);
        fs::write(&path, b"xxx").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(*modified)
            .unwrap();
        assert!(
            store
                .execute_cleanup(&plan)
                .unwrap_err()
                .contains("verification")
        );
        for hash in plan.candidates.keys() {
            assert!(data.path().join("blobs").join(hash).exists());
        }
    }

    #[test]
    fn shared_handles_and_different_stores_cannot_execute_a_plan() {
        let data = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        let plan = store.preview_cleanup().unwrap();
        drop(store);
        let reader = ReviewStore::open(data.path().into()).unwrap();
        assert!(reader.preview_cleanup().is_err());
        assert!(reader.execute_cleanup(&plan).is_err());
        let other_store = ReviewStore::try_open_maintenance(other.path().into()).unwrap();
        assert!(other_store.execute_cleanup(&plan).is_err());
    }

    #[test]
    fn cleanup_retains_recent_and_protected_content_and_rechecks_new_references() {
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        let now = SystemTime::now();
        let old = store.put_blob(b"old").unwrap();
        let recent = store.put_blob(b"recent").unwrap();
        let file = fs::File::options()
            .write(true)
            .open(data.path().join("blobs").join(&old))
            .unwrap();
        file.set_modified(now - RETENTION - Duration::from_mins(1))
            .unwrap();
        drop(file);
        let plan = store.preview_cleanup_at(now).unwrap();
        assert_eq!((plan.count, plan.bytes), (1, 3));
        store
            .persist_snapshot_references("protect", [old.clone()])
            .unwrap();
        assert!(store.execute_cleanup(&plan).is_err());
        assert_eq!(store.read_blob(&old).unwrap(), b"old");
        store.release_snapshot_references("protect").unwrap();
        let result = store.execute_cleanup(&plan).unwrap();
        assert_eq!((result.removed_count, result.removed_bytes), (1, 3));
        assert_eq!(store.read_blob(&recent).unwrap(), b"recent");
        assert!(store.read_blob(&old).is_err());
    }
}
