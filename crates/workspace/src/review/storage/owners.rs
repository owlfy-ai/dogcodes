//! Owner retirement uses the same exclusive maintenance boundary as snapshots.
use super::{
    ReviewResult, ReviewStore, SnapshotReferences, entries, lock_directory, read_manifest,
    stream_entries, valid_hash,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

pub(super) fn retired_id(id: &str) -> String {
    super::super::digest(format!("snapshot-owner/{id}").as_bytes())
}

const OWNER_CLEANUP_BATCH: usize = 256;

fn referenced(store: &ReviewStore) -> ReviewResult<BTreeSet<String>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut owners = BTreeSet::new();
    // Keep owners until even their released reference payloads are cleaned.
    for path in stream_entries(&store.directory.join("snapshot-references"))? {
        if Instant::now() > deadline {
            return Err("owner reference scan timed out".into());
        }
        let path = path?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid reference name")?;
        if !valid_hash(name) {
            continue;
        }
        let manifest: SnapshotReferences =
            serde_json::from_slice(&read_manifest(&path, 16 * 1024 * 1024)?)
                .map_err(|_| "invalid owner reference manifest")?;
        if super::super::digest(manifest.snapshot_id.as_bytes()) != name {
            return Err("owner reference identity mismatch".into());
        }
        if let Some(owner) = manifest.owner {
            owners.insert(owner);
        }
    }
    Ok(owners)
}

fn idle_since(store: &ReviewStore, id: &str, path: &Path) -> ReviewResult<Option<SystemTime>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "owner directory unavailable")?;
    if !metadata.is_dir() {
        return Ok(None);
    }
    let children = entries(path)?;
    // Interrupted cleanup may have removed only the lock file. A durable
    // retirement record is required; an unexplained empty directory is kept.
    if children.is_empty() && super::retired::contains(&store.directory, &retired_id(id))? {
        return Ok(Some(
            metadata
                .modified()
                .map_err(|_| "owner timestamp unavailable")?,
        ));
    }
    if children.len() != 1
        || children[0]
            .file_name()
            .is_none_or(|name| name != "storage.lock")
    {
        return Ok(None);
    }
    if read_manifest(&children[0], 0).is_err() {
        return Ok(None);
    }
    let Ok(_lock) = lock_directory(path, true) else {
        return Ok(None);
    };
    Ok(Some(
        metadata
            .modified()
            .map_err(|_| "owner timestamp unavailable")?,
    ))
}

pub(super) fn preview(
    store: &ReviewStore,
    cutoff: SystemTime,
) -> ReviewResult<BTreeMap<String, SystemTime>> {
    let protected = referenced(store)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut result = BTreeMap::new();
    for path in stream_entries(&store.directory.join("snapshot-owners"))? {
        if result.len() >= OWNER_CLEANUP_BATCH {
            break;
        }
        if Instant::now() > deadline {
            if !result.is_empty() {
                break;
            }
            return Err("owner preview timed out".into());
        }
        let path = path?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid owner name")?;
        if !valid_hash(name) || protected.contains(name) {
            continue;
        }
        if let Some(time) = idle_since(store, name, &path)?
            && time < cutoff
        {
            result.insert(name.into(), time);
        }
    }
    Ok(result)
}

pub(super) fn validate(
    store: &ReviewStore,
    candidates: &BTreeMap<String, SystemTime>,
) -> ReviewResult<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let protected = referenced(store)?;
    for (id, time) in candidates {
        if protected.contains(id)
            || idle_since(store, id, &store.directory.join("snapshot-owners").join(id))?
                != Some(*time)
        {
            return Err("owner cleanup preview is stale".into());
        }
    }
    Ok(())
}

pub(super) fn remove(
    store: &ReviewStore,
    candidates: &BTreeMap<String, SystemTime>,
) -> ReviewResult<usize> {
    validate(store, candidates)?;
    super::retired::insert(&store.directory, candidates.keys().map(|id| retired_id(id)))?;
    for (removed, id) in candidates.keys().enumerate() {
        let path = store.directory.join("snapshot-owners").join(id);
        fs::remove_file(path.join("storage.lock"))
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .and_then(|()| fs::remove_dir(&path))
            .map_err(|_| format!("owner cleanup failed after {removed} directories"))?;
    }
    if !candidates.is_empty() {
        super::super::sync_directory(&store.directory.join("snapshot-owners"))?;
    }
    Ok(candidates.len())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn owner_cleanup_batches_resume_after_reopen_and_keep_live_owner() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let live = store.create_snapshot_owner("live-reader").unwrap();
        let old = SystemTime::now() - Duration::from_hours(24 * 31);
        fs::File::open(directory.path().join("snapshot-owners").join(&live.id))
            .unwrap()
            .set_modified(old)
            .unwrap();
        for index in 0..OWNER_CLEANUP_BATCH + 3 {
            let owner = store
                .create_snapshot_owner(&format!("idle-{index}"))
                .unwrap();
            let path = directory.path().join("snapshot-owners").join(&owner.id);
            drop(owner);
            fs::File::open(path).unwrap().set_modified(old).unwrap();
        }
        let plan = store.preview_storage_cleanup().unwrap();
        assert_eq!(plan.metadata.count, OWNER_CLEANUP_BATCH);
        assert_eq!(
            store
                .execute_storage_cleanup(&plan)
                .unwrap()
                .removed_metadata_count,
            OWNER_CLEANUP_BATCH
        );
        drop(store);
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let plan = store.preview_storage_cleanup().unwrap();
        assert_eq!(plan.metadata.count, 3);
        assert_eq!(
            store
                .execute_storage_cleanup(&plan)
                .unwrap()
                .removed_metadata_count,
            3
        );
        assert_eq!(store.preview_storage_cleanup().unwrap().metadata.count, 0);
        assert!(
            directory
                .path()
                .join("snapshot-owners")
                .join(&live.id)
                .exists()
        );
        for index in 0..OWNER_CLEANUP_BATCH + 3 {
            assert!(
                store
                    .create_snapshot_owner(&format!("idle-{index}"))
                    .is_err()
            );
        }
        assert_eq!(
            fs::read_dir(directory.path().join("snapshot-owners"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn interrupted_cleanup_reclaims_only_a_retired_empty_directory() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let owner = store.create_snapshot_owner("interrupted-owner").unwrap();
        let id = owner.id.clone();
        drop(owner);
        let path = directory.path().join("snapshot-owners").join(&id);
        fs::remove_file(path.join("storage.lock")).unwrap();
        fs::File::open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(24 * 31))
            .unwrap();
        assert_eq!(store.preview_metadata_cleanup().unwrap().count, 0);
        super::super::retired::insert(directory.path(), [retired_id(&id)]).unwrap();
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(plan.count, 1);
        assert_eq!(store.execute_metadata_cleanup(&plan).unwrap(), 1);
        assert!(!path.exists());
        assert!(store.create_snapshot_owner("interrupted-owner").is_err());
    }

    #[test]
    fn cleanup_keeps_live_and_referenced_owners_and_retires_idle_identity() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        let live = store.create_snapshot_owner("live-owner").unwrap();
        let referenced = store.create_snapshot_owner("referenced-owner").unwrap();
        let idle = store.create_snapshot_owner("idle-owner").unwrap();
        let blob = store.put_blob(b"content").unwrap();
        store
            .persist_owned_snapshot_references(&referenced, "protected", [blob])
            .unwrap();
        let old = SystemTime::now() - Duration::from_hours(24 * 31);
        for id in [&live.id, &referenced.id, &idle.id] {
            fs::File::open(directory.path().join("snapshot-owners").join(id))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let idle_path = directory.path().join("snapshot-owners").join(&idle.id);
        drop(idle);
        drop(referenced);
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(plan.count, 1);
        fs::write(idle_path.join("unexpected"), b"keep").unwrap();
        assert!(store.execute_metadata_cleanup(&plan).is_err());
        assert!(idle_path.join("unexpected").exists());
        fs::remove_file(idle_path.join("unexpected")).unwrap();
        fs::File::open(&idle_path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let plan = store.preview_metadata_cleanup().unwrap();
        assert_eq!(store.execute_metadata_cleanup(&plan).unwrap(), 1);
        assert!(!idle_path.exists());
        assert!(store.create_snapshot_owner("idle-owner").is_err());
        assert!(
            directory
                .path()
                .join("snapshot-owners")
                .join(&live.id)
                .exists()
        );
        assert_eq!(
            fs::read_dir(directory.path().join("snapshot-owners"))
                .unwrap()
                .count(),
            2
        );
    }
}
