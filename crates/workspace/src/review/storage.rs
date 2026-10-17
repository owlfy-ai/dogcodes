//! Storage accounting is conservative: every durable baseline and recovery
//! intent protects its blobs, including unfinished or orphaned group members.
use super::{DirectorySnapshot, ReviewResult, ReviewStore, recovery::RecoveryRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{Duration, Instant},
};

mod cleanup;
mod maintenance;
mod metadata;
mod task_deletion;
pub use maintenance::{StorageCleanupPlan, StorageCleanupResult};
mod owners;
mod quota;
mod retired;
pub use cleanup::{CleanupPlan, CleanupResult};
pub use metadata::MetadataCleanupPlan;
pub use quota::CONTENT_CAPACITY_BYTES;

/// The logical lease ends at guard drop, not when the last duplicated OS file
/// description closes (for example after a concurrent fork reaches exec).
pub(super) struct StorageLock(fs::File);

impl Drop for StorageLock {
    fn drop(&mut self) {
        // Close remains the fallback on error. Never delete the permanent lock
        // path, which would let different processes enter separate lock domains.
        let _ = self.0.unlock();
    }
}

pub(super) fn lock_directory(directory: &Path, exclusive: bool) -> ReviewResult<StorageLock> {
    lock_directory_with_wait(directory, exclusive, Duration::ZERO)
}

/// Only short publication reservations opt into bounded contention waiting.
/// Maintenance and snapshot-owner probes retain immediate acquisition semantics.
pub(super) fn lock_directory_with_wait(
    directory: &Path,
    exclusive: bool,
    wait: Duration,
) -> ReviewResult<StorageLock> {
    let path = directory.join("storage.lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| "review storage lock unavailable")?;
            if !metadata.is_file() {
                return Err("invalid review storage lock".into());
            }
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|_| "review storage lock unavailable")?
        }
        Err(_) => return Err("review storage lock unavailable".into()),
    };
    let metadata = fs::symlink_metadata(&path).map_err(|_| "review storage lock unavailable")?;
    if !metadata.is_file()
        || !file
            .metadata()
            .map_err(|_| "review storage lock unavailable")?
            .is_file()
    {
        return Err("invalid review storage lock".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file
            .metadata()
            .map_err(|_| "review storage lock unavailable")?;
        if opened.ino() != metadata.ino() || opened.dev() != metadata.dev() {
            return Err("review storage lock changed while opening".into());
        }
    }
    let deadline = Instant::now() + wait;
    loop {
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => return Ok(StorageLock(file)),
            Err(fs::TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("Review 存储正在使用或维护，请稍后重试".into());
                }
                std::thread::sleep(remaining.min(Duration::from_millis(5)));
            }
            Err(fs::TryLockError::Error(_)) => {
                return Err("review storage lock unavailable".into());
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub capacity_bytes: u64,
    pub blob_count: usize,
    pub total_bytes: u64,
    pub protected_bytes: u64,
    pub unreferenced_bytes: u64,
    pub missing_references: usize,
    pub recovery_metadata_bytes: u64,
    pub recovery_metadata_files: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotReferences {
    snapshot_id: String,
    hashes: BTreeSet<String>,
    owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

pub struct SnapshotOwner {
    id: String,
    directory: std::path::PathBuf,
    _lock: StorageLock,
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn snapshot_released(path: &Path) -> ReviewResult<bool> {
    let marker = path.with_extension("released");
    match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let directory = path
                .parent()
                .and_then(Path::parent)
                .ok_or("invalid reference path")?;
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid reference identity")?;
            retired::contains(directory, id)
        }
        Err(_) => Err("snapshot release marker unavailable".into()),
        Ok(_) => {
            if read_manifest(&marker, 8)? != b"released" {
                return Err("invalid snapshot release marker".into());
            }
            Ok(true)
        }
    }
}

/// Stream inventories under the caller's deadline without first allocating a
/// path for every entry. Missing inventories are empty; other I/O errors remain
/// fatal, so a partial reference set can never authorize content deletion.
fn stream_entries(
    directory: &Path,
) -> ReviewResult<impl Iterator<Item = ReviewResult<std::path::PathBuf>>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("review storage directory unavailable".into()),
    };
    Ok(entries.into_iter().flatten().map(|entry| {
        entry
            .map(|entry| entry.path())
            .map_err(|_| "review storage entry unavailable".into())
    }))
}

fn entries(directory: &Path) -> ReviewResult<Vec<std::path::PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err("review storage directory unavailable".into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        if paths.len() >= 100_000 {
            return Err("review storage entry budget exceeded".into());
        }
        paths.push(
            entry
                .map_err(|_| "review storage entry unavailable")?
                .path(),
        );
    }
    Ok(paths)
}

fn read_manifest(path: &Path, limit: u64) -> ReviewResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "review manifest unavailable")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("invalid review storage manifest".into());
    }
    fs::read(path).map_err(|_| "review manifest unavailable".into())
}

impl ReviewStore {
    pub fn create_snapshot_owner(&self, unique_id: &str) -> ReviewResult<SnapshotOwner> {
        if unique_id.is_empty() {
            return Err("snapshot owner ID is empty".into());
        }
        let id = super::digest(unique_id.as_bytes());
        if retired::contains(&self.directory, &owners::retired_id(&id))? {
            return Err("snapshot owner identity is retired".into());
        }
        let owners = self.directory.join("snapshot-owners");
        super::private_directory(&owners)?;
        let path = owners.join(&id);
        // Owner identities are single-use, including after a crash.
        fs::create_dir(&path).map_err(|_| "snapshot owner already exists or unavailable")?;
        super::private_directory(&path)?;
        Ok(SnapshotOwner {
            id,
            directory: self.directory.clone(),
            _lock: lock_directory(&path, true)?,
        })
    }

    pub fn persist_owned_snapshot_references(
        &self,
        owner: &SnapshotOwner,
        snapshot_id: &str,
        hashes: impl IntoIterator<Item = String>,
    ) -> ReviewResult<()> {
        if owner.directory != self.directory {
            return Err("snapshot owner belongs to another store".into());
        }
        self.persist_references(snapshot_id, hashes, Some(owner.id.clone()), None)
    }

    pub fn persist_task_snapshot_references(
        &self,
        owner: &SnapshotOwner,
        task_id: &str,
        snapshot_id: &str,
        hashes: impl IntoIterator<Item = String>,
    ) -> ReviewResult<()> {
        if owner.directory != self.directory {
            return Err("snapshot owner belongs to another store".into());
        }
        self.require_live_task(task_id)?;
        self.persist_references(
            snapshot_id,
            hashes,
            Some(owner.id.clone()),
            Some(task_id.into()),
        )
    }

    /// A maintenance lock excludes writers while owner locks distinguish live
    /// caches from crashed processes. Unknown owners are preserved, not guessed.
    pub fn release_abandoned_snapshots(&self) -> ReviewResult<usize> {
        if !self.maintenance {
            return Err("snapshot maintenance requires exclusive storage access".into());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut released = 0;
        for path in stream_entries(&self.directory.join("snapshot-references"))? {
            if Instant::now() >= deadline {
                return Err("snapshot maintenance timed out".into());
            }
            let path = path?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid snapshot name")?;
            if !valid_hash(name) {
                continue;
            }
            let manifest: SnapshotReferences =
                serde_json::from_slice(&read_manifest(&path, 16 * 1024 * 1024)?)
                    .map_err(|_| "invalid durable snapshot references")?;
            if super::digest(manifest.snapshot_id.as_bytes()) != name {
                return Err("snapshot identity mismatch".into());
            }
            if snapshot_released(&path)? {
                continue;
            }
            let Some(owner) = manifest.owner else {
                continue;
            };
            if !valid_hash(&owner) {
                return Err("invalid snapshot owner".into());
            }
            let directory = self.directory.join("snapshot-owners").join(owner);
            if !fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir()) {
                continue;
            }
            // Lock contention or unreadable lock is conservatively protected.
            let Ok(_owner_lock) = lock_directory(&directory, true) else {
                continue;
            };
            self.release_snapshot_references(&manifest.snapshot_id)?;
            released += 1;
        }
        Ok(released)
    }

    /// Publish immutable references before making an in-memory snapshot
    /// available. A crashed publisher conservatively leaves protection behind.
    pub fn persist_snapshot_references(
        &self,
        snapshot_id: &str,
        hashes: impl IntoIterator<Item = String>,
    ) -> ReviewResult<()> {
        self.persist_references(snapshot_id, hashes, None, None)
    }

    fn persist_references(
        &self,
        snapshot_id: &str,
        hashes: impl IntoIterator<Item = String>,
        owner: Option<String>,
        task_id: Option<String>,
    ) -> ReviewResult<()> {
        if snapshot_id.is_empty() || snapshot_id.len() > 256 || snapshot_id.contains('\0') {
            return Err("invalid snapshot reference identity".into());
        }
        let references = SnapshotReferences {
            snapshot_id: snapshot_id.into(),
            hashes: hashes.into_iter().collect(),
            owner,
            task_id,
        };
        if references.hashes.len() > 100_000
            || references.hashes.iter().any(|hash| !valid_hash(hash))
        {
            return Err("invalid snapshot references".into());
        }
        for hash in &references.hashes {
            self.read_blob(hash)?;
        }
        let directory = self.directory.join("snapshot-references");
        super::private_directory(&directory)?;
        let path = directory.join(super::digest(snapshot_id.as_bytes()));
        if snapshot_released(&path)? {
            return Err("snapshot references already released".into());
        }
        let bytes = serde_json::to_vec(&references)
            .map_err(|_| "snapshot reference serialization failed")?;
        super::write_once(&path, &bytes)?;
        if read_manifest(&path, 16 * 1024 * 1024)? != bytes {
            return Err("snapshot references are immutable".into());
        }
        Ok(())
    }

    /// Release only after removing the snapshot from the cache. Existing
    /// readers must hold a `ReviewStore` shared lock until they finish.
    pub fn release_snapshot_references(&self, snapshot_id: &str) -> ReviewResult<()> {
        let path = self
            .directory
            .join("snapshot-references")
            .join(super::digest(snapshot_id.as_bytes()));
        let manifest: SnapshotReferences =
            serde_json::from_slice(&read_manifest(&path, 16 * 1024 * 1024)?)
                .map_err(|_| "invalid durable snapshot references")?;
        if manifest.snapshot_id != snapshot_id {
            return Err("snapshot identity mismatch".into());
        }
        super::write_once(&path.with_extension("released"), b"released")?;
        if !snapshot_released(&path)? {
            return Err("snapshot reference release failed".into());
        }
        Ok(())
    }

    /// Caller supplies hashes of in-memory snapshots. This is an inventory,
    /// not permission to delete: writers may publish new references afterward.
    pub fn storage_usage(
        &self,
        live_hashes: impl IntoIterator<Item = String>,
    ) -> ReviewResult<StorageUsage> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let protected = self.protected_hashes(live_hashes, deadline)?;
        let mut usage = self.measure_blobs(protected, deadline)?;
        for name in [
            "recovery",
            "recovery-groups",
            "turn-undo",
            "index-recovery",
            "submodule-recovery",
            "nested-recovery",
        ] {
            for path in stream_entries(&self.directory.join(name))? {
                if Instant::now() >= deadline {
                    return Err("recovery metadata inventory budget exceeded".into());
                }
                let metadata =
                    fs::symlink_metadata(path?).map_err(|_| "recovery metadata unavailable")?;
                if !metadata.is_file() {
                    return Err("invalid recovery metadata entry".into());
                }
                usage.recovery_metadata_files += 1;
                usage.recovery_metadata_bytes = usage
                    .recovery_metadata_bytes
                    .checked_add(metadata.len())
                    .ok_or("recovery metadata size overflow")?;
            }
        }
        Ok(usage)
    }

    fn protected_hashes(
        &self,
        live_hashes: impl IntoIterator<Item = String>,
        deadline: Instant,
    ) -> ReviewResult<BTreeSet<String>> {
        let mut protected: BTreeSet<String> = live_hashes.into_iter().collect();
        for directory in ["baselines", "recovery", "snapshot-references"] {
            for path in stream_entries(&self.directory.join(directory))? {
                if Instant::now() >= deadline {
                    return Err("review storage scan timed out".into());
                }
                let path = path?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("invalid review manifest name")?;
                // Recovery status markers and unpublished write temporaries do
                // not themselves contain additional content references.
                if !valid_hash(name) {
                    continue;
                }
                if directory == "baselines" {
                    let manifest: DirectorySnapshot =
                        serde_json::from_slice(&read_manifest(&path, 32 * 1024 * 1024)?)
                            .map_err(|_| "invalid baseline during storage inventory")?;
                    protected.extend(manifest.files.into_values().map(|version| version.hash));
                } else if directory == "snapshot-references" {
                    let manifest: SnapshotReferences =
                        serde_json::from_slice(&read_manifest(&path, 16 * 1024 * 1024)?)
                            .map_err(|_| "invalid durable snapshot references")?;
                    if super::digest(manifest.snapshot_id.as_bytes()) != name {
                        return Err("durable snapshot identity mismatch".into());
                    }
                    if !snapshot_released(&path)? {
                        protected.extend(manifest.hashes);
                    }
                } else {
                    let manifest: RecoveryRecord =
                        serde_json::from_slice(&read_manifest(&path, 65536)?)
                            .map_err(|_| "invalid recovery during storage inventory")?;
                    if manifest.id != name {
                        return Err("recovery identity mismatch during storage inventory".into());
                    }
                    protected.extend(manifest.original_xattrs);
                    protected.extend(manifest.original_acl);
                    protected.extend(
                        manifest
                            .original
                            .into_iter()
                            .chain(manifest.replacement)
                            .map(|version| version.hash),
                    );
                }
            }
        }
        if protected.iter().any(|hash| !valid_hash(hash)) {
            return Err("invalid protected content reference".into());
        }
        Ok(protected)
    }

    fn measure_blobs(
        &self,
        protected: BTreeSet<String>,
        deadline: Instant,
    ) -> ReviewResult<StorageUsage> {
        let mut missing = protected.clone();
        let mut usage = StorageUsage {
            capacity_bytes: self.content_capacity()?,
            blob_count: 0,
            total_bytes: 0,
            protected_bytes: 0,
            unreferenced_bytes: 0,
            missing_references: 0,
            recovery_metadata_bytes: 0,
            recovery_metadata_files: 0,
        };
        for path in stream_entries(&self.directory.join("blobs"))? {
            if Instant::now() >= deadline {
                return Err("review storage scan timed out".into());
            }
            let path = path?;
            let hash = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid blob name")?;
            if !valid_hash(hash) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|_| "review blob unavailable")?;
            if !metadata.is_file() {
                return Err("invalid blob during storage inventory".into());
            }
            usage.blob_count += 1;
            usage.total_bytes = usage
                .total_bytes
                .checked_add(metadata.len())
                .ok_or("review storage size overflow")?;
            if protected.contains(hash) {
                usage.protected_bytes += metadata.len();
            } else {
                usage.unreferenced_bytes += metadata.len();
            }
            missing.remove(hash);
        }
        usage.missing_references = missing.len();
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "1000 valid manifests with 1000 references each; run explicitly"]
    fn dense_valid_snapshot_inventory_preserves_all_referenced_content() {
        let temp = tempfile::tempdir().unwrap();
        let store = super::ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        let mut pending = std::collections::BTreeMap::new();
        for index in 0..1000 {
            let bytes = format!("content {index}").into_bytes();
            pending.insert(crate::review::digest(&bytes), bytes);
        }
        let hashes: std::collections::BTreeSet<_> = pending.keys().cloned().collect();
        store.flush_scan_blobs(&mut pending).unwrap();
        store
            .persist_snapshot_references("snapshot-0", hashes.clone())
            .unwrap();
        // Seed valid serialized records directly to isolate inventory cost from
        // a million repeated publication-time blob reads. This is not a timing
        // claim about the public snapshot publication API.
        for index in 1..1000 {
            let id = format!("snapshot-{index}");
            let record = super::SnapshotReferences {
                snapshot_id: id.clone(),
                hashes: hashes.clone(),
                owner: None,
                task_id: None,
            };
            crate::review::write_once(
                &temp
                    .path()
                    .join("snapshot-references")
                    .join(crate::review::digest(id.as_bytes())),
                &serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
        }
        super::retired::insert(temp.path(), [crate::review::digest(b"previously-retired")])
            .unwrap();
        let expired = store.put_blob(b"unreferenced expired").unwrap();
        std::fs::File::open(temp.path().join("blobs").join(&expired))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(24 * 8))
            .unwrap();
        drop(store);
        let store = super::ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        let started = std::time::Instant::now();
        let usage = store.storage_usage([]).unwrap();
        let inventory_ms = started.elapsed().as_millis();
        assert_eq!(usage.blob_count, 1001);
        assert_eq!(usage.missing_references, 0);
        let started = std::time::Instant::now();
        let plan = store.preview_storage_cleanup().unwrap();
        let preview_ms = started.elapsed().as_millis();
        assert_eq!(plan.content.count, 1);
        assert_eq!(plan.metadata.count, 0);
        let started = std::time::Instant::now();
        assert_eq!(
            store.execute_storage_cleanup(&plan).unwrap().removed_count,
            1
        );
        println!(
            "validManifests=1000 referencesPerManifest=1000 inventoryMs={inventory_ms} previewMs={preview_ms} executeMs={}",
            started.elapsed().as_millis()
        );
        for index in 0..1000 {
            let bytes = format!("content {index}").into_bytes();
            assert_eq!(
                store.read_blob(&crate::review::digest(&bytes)).unwrap(),
                bytes
            );
        }
        assert!(!temp.path().join("blobs").join(expired).exists());
    }

    #[test]
    #[ignore = "large blob and metadata inventory; run explicitly with --nocapture"]
    fn storage_accounting_streams_large_inventories_without_partial_totals() {
        let temp = tempfile::tempdir().unwrap();
        let store = super::ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        let hash = store.put_blob(b"protected content").unwrap();
        store
            .persist_snapshot_references("live", [hash.clone()])
            .unwrap();
        let blobs = temp.path().join("blobs");
        // Inventory measures physical size, not digest integrity. These fresh
        // zero-byte placeholders must not be described as valid CAS content.
        for index in 0..100_001 {
            std::fs::File::create(blobs.join(format!("{index:064x}"))).unwrap();
        }
        let started = std::time::Instant::now();
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.blob_count, 100_002);
        assert_eq!(usage.total_bytes, 17);
        assert_eq!(usage.protected_bytes, 17);
        assert_eq!(usage.missing_references, 0);
        // Reuse the large fixture to check recovery-journal accounting too;
        // restore the live blob before inventory, never edit user data.
        let metadata = temp.path().join("nested-recovery");
        std::fs::rename(&blobs, &metadata).unwrap();
        std::fs::create_dir(&blobs).unwrap();
        std::fs::rename(metadata.join(&hash), blobs.join(&hash)).unwrap();
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.blob_count, 1);
        assert_eq!(usage.recovery_metadata_files, 100_001);
        assert_eq!(usage.recovery_metadata_bytes, 0);
        assert_eq!(usage.missing_references, 0);
        std::fs::create_dir(metadata.join("invalid-entry")).unwrap();
        assert!(store.storage_usage([]).is_err());
        assert_eq!(store.read_blob(&hash).unwrap(), b"protected content");
        println!(
            "largeInventoryAccountingMs={}",
            started.elapsed().as_millis()
        );
    }

    #[test]
    #[ignore = "large directory inventory; run explicitly with --nocapture"]
    fn reference_inventory_streams_past_old_entry_limit() {
        let temp = tempfile::tempdir().unwrap();
        let store = super::ReviewStore::try_open_maintenance(temp.path().into()).unwrap();
        let hash = store.put_blob(b"protected content").unwrap();
        store
            .persist_snapshot_references("live", [hash.clone()])
            .unwrap();
        let directory = temp.path().join("snapshot-references");
        // Unpublished temporary names do not hold references, but used to make
        // the eager path collector fail before it could read the live record.
        for index in 0..100_001 {
            std::fs::File::create(directory.join(format!("temporary-{index}"))).unwrap();
        }
        assert_eq!(
            super::stream_entries(&directory)
                .unwrap()
                .collect::<super::ReviewResult<Vec<_>>>()
                .unwrap()
                .len(),
            100_002
        );
        assert!(super::entries(&directory).is_err());
        let started = std::time::Instant::now();
        // Match the desktop maintenance preflight before combined preview.
        assert_eq!(store.release_abandoned_snapshots().unwrap(), 0);
        let plan = store.preview_storage_cleanup().unwrap();
        assert_eq!(plan.content.count, 0);
        assert_eq!(
            store.execute_storage_cleanup(&plan).unwrap().removed_count,
            0
        );
        assert_eq!(store.read_blob(&hash).unwrap(), b"protected content");
        println!(
            "streamedReferenceEntries=100002 elapsedMs={}",
            started.elapsed().as_millis()
        );
        assert!(
            store
                .protected_hashes([], std::time::Instant::now())
                .is_err()
        );
    }

    #[test]
    fn nested_parent_journals_are_included_in_metadata_usage() {
        let temp = tempfile::tempdir().unwrap();
        let store = super::ReviewStore::open(temp.path().into()).unwrap();
        let before = store.storage_usage([]).unwrap();
        let directory = temp.path().join("nested-recovery");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("parent"), b"parent manifest").unwrap();
        std::fs::write(directory.join("parent.started"), b"started").unwrap();
        std::fs::write(directory.join("parent.applied"), b"applied").unwrap();
        let after = store.storage_usage([]).unwrap();
        assert_eq!(
            after.recovery_metadata_files,
            before.recovery_metadata_files + 3
        );
        assert_eq!(
            after.recovery_metadata_bytes,
            before.recovery_metadata_bytes + 29
        );
        assert_eq!(after.total_bytes, before.total_bytes);
        // Unknown entries must not be followed or silently undercounted.
        std::fs::create_dir(directory.join("unexpected-directory")).unwrap();
        assert!(store.storage_usage([]).is_err());
    }

    #[test]
    fn dropping_guard_releases_lock_even_with_duplicated_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let guard = super::lock_directory(directory.path(), true).unwrap();
        let duplicate = guard.0.try_clone().unwrap();
        assert!(super::lock_directory(directory.path(), true).is_err());
        drop(guard);
        // Model a descriptor copied into a concurrently spawned child before
        // exec closes it. It must not extend the parent's logical lease.
        let next = super::lock_directory(directory.path(), true).unwrap();
        drop(duplicate);
        assert!(super::lock_directory(directory.path(), true).is_err());
        drop(next);
        assert!(super::lock_directory(directory.path(), true).is_ok());
    }
    #[test]
    fn contention_wait_is_bounded_and_does_not_bypass_active_owner() {
        let directory = tempfile::tempdir().unwrap();
        let owner = super::lock_directory(directory.path(), true).unwrap();
        let started = Instant::now();
        assert!(
            super::lock_directory_with_wait(directory.path(), true, Duration::from_millis(30))
                .is_err()
        );
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert!(super::lock_directory(directory.path(), true).is_err());
        drop(owner);
        assert!(
            super::lock_directory_with_wait(directory.path(), true, Duration::from_millis(30))
                .is_ok()
        );
    }
    use super::*;
    use crate::review::{FileKind, FileVersion, SnapshotBudget};

    #[test]
    fn maintenance_releases_only_snapshots_whose_owner_has_exited() {
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let owner = store.create_snapshot_owner("unique-process").unwrap();
        let hash = store.put_blob(b"protected").unwrap();
        store
            .persist_owned_snapshot_references(&owner, "owned", [hash.clone()])
            .unwrap();
        assert!(store.release_abandoned_snapshots().is_err());
        assert!(store.create_snapshot_owner("unique-process").is_err());
        drop(store);
        let maintenance = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 0);
        assert_eq!(maintenance.storage_usage([]).unwrap().protected_bytes, 9);
        drop(owner);
        assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 1);
        assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 0);
        assert_eq!(maintenance.storage_usage([]).unwrap().protected_bytes, 0);
        assert_eq!(maintenance.read_blob(&hash).unwrap(), b"protected");
        assert!(maintenance.create_snapshot_owner("unique-process").is_err());
    }

    #[test]
    fn durable_snapshot_references_survive_restart_and_cannot_be_retargeted() {
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let first = store.put_blob(b"first").unwrap();
        let other = store.put_blob(b"other").unwrap();
        store
            .persist_snapshot_references("snapshot", [first.clone(), first.clone()])
            .unwrap();
        store
            .persist_snapshot_references("snapshot", [first.clone()])
            .unwrap();
        assert!(
            store
                .persist_snapshot_references("snapshot", [other])
                .is_err()
        );
        assert!(
            store
                .persist_snapshot_references("missing", ["0".repeat(64)])
                .is_err()
        );
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.protected_bytes, 5);
        assert_eq!(usage.unreferenced_bytes, 5);
        assert_eq!(usage.missing_references, 0);
        store.release_snapshot_references("snapshot").unwrap();
        store.release_snapshot_references("snapshot").unwrap();
        assert!(
            store
                .persist_snapshot_references("snapshot", [first.clone()])
                .is_err()
        );
        drop(store);
        let store = ReviewStore::open(data.path().into()).unwrap();
        let usage = store.storage_usage([]).unwrap();
        assert_eq!(usage.protected_bytes, 0);
        assert_eq!(usage.unreferenced_bytes, 10);
        assert_eq!(store.read_blob(&first).unwrap(), b"first");
    }

    #[test]
    fn storage_lock_child() {
        let Some(directory) = std::env::var_os("DOGCODES_REVIEW_LOCK_TEST") else {
            return;
        };
        assert!(ReviewStore::open(directory.into()).is_err());
    }

    #[test]
    fn maintenance_excludes_readers_and_releases_on_drop_across_processes() {
        let data = tempfile::tempdir().unwrap();
        let first = ReviewStore::open(data.path().into()).unwrap();
        let second = ReviewStore::open(data.path().into()).unwrap();
        assert!(ReviewStore::try_open_maintenance(data.path().into()).is_err());
        drop(first);
        drop(second);
        let maintenance = ReviewStore::try_open_maintenance(data.path().into()).unwrap();
        assert!(ReviewStore::open(data.path().into()).is_err());
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "review::storage::tests::storage_lock_child"])
            .env("DOGCODES_REVIEW_LOCK_TEST", data.path())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        drop(maintenance);
        assert!(ReviewStore::open(data.path().into()).is_ok());
        assert!(data.path().join("storage.lock").is_file());
    }

    #[test]
    fn inventories_baseline_recovery_and_live_references_without_deleting() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("file"), "base").unwrap();
        store
            .ensure_baseline("task", root.path(), SnapshotBudget::default())
            .unwrap();
        let backup = store.put_blob(b"backup").unwrap();
        store
            .prepare_recovery(
                "task",
                root.path(),
                "other",
                None,
                Some(FileVersion {
                    hash: backup,
                    bytes: 6,
                    kind: FileKind::File,
                    executable: false,
                    binary: false,
                }),
            )
            .unwrap();
        let live = store.put_blob(b"live").unwrap();
        let unused = store.put_blob(b"unused").unwrap();
        let usage = store.storage_usage([live, "0".repeat(64)]).unwrap();
        assert_eq!(usage.blob_count, 4);
        assert_eq!(usage.total_bytes, 20);
        assert_eq!(usage.protected_bytes, 14);
        assert_eq!(usage.unreferenced_bytes, 6);
        assert_eq!(usage.missing_references, 1);
        assert_eq!(store.read_blob(&unused).unwrap(), b"unused");
        fs::write(data.path().join("baselines").join("a".repeat(64)), "broken").unwrap();
        assert!(store.storage_usage([]).is_err());
    }
}
