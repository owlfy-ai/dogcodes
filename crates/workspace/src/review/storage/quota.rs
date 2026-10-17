use super::super::{ReviewResult, ReviewStore, private_directory};
use std::{
    fs,
    io::Write,
    time::{Duration, Instant},
};

/// Includes protected baselines and recovery content; never auto-evict those.
pub const CONTENT_CAPACITY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(in crate::review) struct Reservation<'a> {
    store: &'a ReviewStore,
    _guard: super::StorageLock,
    accounting: (u64, u64, usize),
}
impl Reservation<'_> {
    pub(in crate::review) fn commit(self) -> ReviewResult<()> {
        *self
            .store
            .quota_cache
            .lock()
            .map_err(|_| "Diff capacity cache unavailable")? = Some(self.accounting);
        Ok(())
    }
}

impl ReviewStore {
    pub fn content_capacity(&self) -> ReviewResult<u64> {
        let path = self.directory.join("capacity-bytes");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CONTENT_CAPACITY_BYTES);
            }
            Ok(metadata) if metadata.is_file() && metadata.len() <= 32 => {}
            _ => return Err("invalid Diff capacity setting".into()),
        }
        let value = fs::read_to_string(path)
            .map_err(|_| "cannot read Diff capacity setting")?
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid Diff capacity setting")?;
        validate_capacity(value)?;
        Ok(value)
    }

    pub fn set_content_capacity(&self, bytes: u64) -> ReviewResult<()> {
        if !self.maintenance {
            return Err("capacity changes require exclusive maintenance".into());
        }
        validate_capacity(bytes)?;
        // Atomic replacement, never truncate a configuration read by others.
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| "cannot prepare capacity setting")?;
        writeln!(temporary, "{bytes}").map_err(|_| "cannot write capacity setting")?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| "cannot sync capacity setting")?;
        temporary
            .persist(self.directory.join("capacity-bytes"))
            .map_err(|_| "cannot publish capacity setting")?;
        super::super::sync_directory(&self.directory)
    }

    pub(in crate::review) fn reserve_blob_capacity(
        &self,
        incoming: u64,
        capacity: u64,
    ) -> ReviewResult<Reservation<'_>> {
        self.reserve_blobs_capacity(incoming, 1, capacity)
    }

    pub(in crate::review) fn reserve_blobs_capacity(
        &self,
        incoming: u64,
        incoming_count: usize,
        capacity: u64,
    ) -> ReviewResult<Reservation<'_>> {
        let directory = self.directory.join("quota");
        private_directory(&directory)?;
        // Every publisher serializes admission and publication. The shared
        // store lease simultaneously excludes maintenance/deletion.
        // Parallel directory readers may both publish current file versions.
        // A normal short-lived reservation is contention, not a failed query.
        let guard = super::lock_directory_with_wait(&directory, true, Duration::from_secs(10))?;
        let epoch_path = directory.join("epoch");
        let epoch = match fs::symlink_metadata(&epoch_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Ok(metadata) if metadata.is_file() && metadata.len() <= 32 => {
                fs::read_to_string(&epoch_path)
                    .map_err(|_| "cannot read capacity epoch")?
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "invalid capacity epoch")?
            }
            _ => return Err("invalid capacity epoch".into()),
        };
        let cached = self
            .quota_cache
            .lock()
            .map_err(|_| "Diff capacity cache unavailable")?
            .take();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut used = 0_u64;
        let mut entries = 0_usize;
        if let Some((_, bytes, count)) =
            cached.filter(|(version, _, _)| *version == epoch && !self.maintenance)
        {
            used = bytes;
            entries = count;
        } else {
            for (count, entry) in fs::read_dir(self.directory.join("blobs"))
                .map_err(|_| "cannot inspect Diff capacity")?
                .enumerate()
            {
                if count >= 100_000 || Instant::now() >= deadline {
                    return Err("Diff 容量检查超出预算，请先回收旧缓存".into());
                }
                let entry = entry.map_err(|_| "cannot inspect Diff capacity")?;
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|_| "cannot inspect Diff capacity")?;
                if !metadata.is_file() {
                    return Err("unexpected entry in Diff content storage".into());
                }
                used = used
                    .checked_add(metadata.len())
                    .ok_or("Diff capacity overflow")?;
                entries += 1;
            }
        }
        if entries
            .checked_add(incoming_count)
            .is_none_or(|count| count > 100_000)
        {
            return Err("Diff 内容对象数量达到上限，请先回收旧缓存".into());
        }
        if used
            .checked_add(incoming)
            .is_none_or(|total| total > capacity)
        {
            return Err("Diff 内容存储已达到容量上限。请在本地 Diff 存储中预览并回收未引用旧缓存；不会自动删除会话基线或恢复备份。".into());
        }
        // Publish a new epoch before the blob. A crash or failed write leaves
        // all other handles with a mismatched epoch and forces a disk recount.
        let next = epoch.checked_add(1).ok_or("capacity epoch exhausted")?;
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)
            .map_err(|_| "cannot prepare capacity epoch")?;
        writeln!(temporary, "{next}").map_err(|_| "cannot write capacity epoch")?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| "cannot sync capacity epoch")?;
        temporary
            .persist(epoch_path)
            .map_err(|_| "cannot publish capacity epoch")?;
        super::super::sync_directory(&directory)?;
        Ok(Reservation {
            store: self,
            _guard: guard,
            accounting: (next, used + incoming, entries + incoming_count),
        })
    }
}

fn validate_capacity(bytes: u64) -> ReviewResult<()> {
    if !(256 * 1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&bytes) {
        return Err("Diff 容量上限必须在 256 MiB 到 64 GiB 之间".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_blob_publisher_waits_then_recounts_committed_content() {
        for capacity in [3, 4] {
            let directory = tempfile::tempdir().unwrap();
            let first = ReviewStore::open(directory.path().into()).unwrap();
            let second = ReviewStore::open(directory.path().into()).unwrap();
            let reservation = first.reserve_blob_capacity(2, 3).unwrap();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (result_tx, result_rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = second.put_blob_with_capacity(b"bb", capacity);
                result_tx.send(result).unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let early = result_rx.recv_timeout(Duration::from_millis(100));
            let hash = super::super::super::digest(b"aa");
            super::super::super::write_once(&directory.path().join("blobs").join(&hash), b"aa")
                .unwrap();
            reservation.commit().unwrap();
            // Always release the reservation and join before assertions, even if
            // a regression makes the competing publisher finish prematurely.
            worker.join().unwrap();
            assert!(
                matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
                "publisher did not wait for the active reservation: {early:?}"
            );
            let result = result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if capacity == 3 {
                let error = result.unwrap_err();
                assert!(error.contains("容量上限"), "{error}");
            } else {
                assert_eq!(first.read_blob(&result.unwrap()).unwrap(), b"bb");
            }
            assert_eq!(first.read_blob(&hash).unwrap(), b"aa");
            assert_eq!(
                fs::read_dir(directory.path().join("blobs"))
                    .unwrap()
                    .count(),
                if capacity == 3 { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn batch_reservation_recounts_partial_publication_and_checks_object_count() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(directory.path().into()).unwrap();
        let other = ReviewStore::open(directory.path().into()).unwrap();
        let reservation = store.reserve_blobs_capacity(4, 2, 4).unwrap();
        let hash = super::super::super::digest(b"aa");
        super::super::super::write_once(&directory.path().join("blobs").join(&hash), b"aa")
            .unwrap();
        assert!(super::super::lock_directory(&directory.path().join("quota"), true).is_err());
        drop(reservation); // Failed batch: only the first object reached disk.
        assert!(store.quota_cache.lock().unwrap().is_none());
        other.put_blob_with_capacity(b"bb", 4).unwrap();
        assert_eq!(other.quota_cache.lock().unwrap().unwrap().1, 4);
        assert_eq!(other.quota_cache.lock().unwrap().unwrap().2, 2);
        assert!(store.reserve_blobs_capacity(0, 100_000, 4).is_err());
        assert!(store.reserve_blobs_capacity(0, usize::MAX, 4).is_err());
        assert_eq!(store.read_blob(&hash).unwrap(), b"aa");
    }
    #[test]
    fn cached_accounting_recounts_after_other_writer_or_abandoned_reservation() {
        let directory = tempfile::tempdir().unwrap();
        let first = ReviewStore::open(directory.path().into()).unwrap();
        let second = ReviewStore::open(directory.path().into()).unwrap();
        first.put_blob_with_capacity(b"aa", 4).unwrap();
        assert_eq!(first.quota_cache.lock().unwrap().as_ref().unwrap().1, 2);
        second.put_blob_with_capacity(b"b", 4).unwrap();
        assert!(
            first
                .put_blob_with_capacity(b"cc", 4)
                .unwrap_err()
                .contains("容量上限")
        );
        let reservation = second.reserve_blob_capacity(1, 4).unwrap();
        drop(reservation); // Simulate failure after epoch publication, before blob publication.
        assert!(second.quota_cache.lock().unwrap().is_none());
        first.put_blob_with_capacity(b"c", 4).unwrap();
        assert_eq!(first.quota_cache.lock().unwrap().as_ref().unwrap().1, 4);
        assert_eq!(first.quota_cache.lock().unwrap().as_ref().unwrap().2, 3);
        assert!(second.put_blob_with_capacity(b"d", 4).is_err());
        assert_eq!(
            fs::read_dir(directory.path().join("blobs"))
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn capacity_setting_is_exclusive_validated_and_persistent() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(directory.path().into()).unwrap();
        assert_eq!(store.content_capacity().unwrap(), CONTENT_CAPACITY_BYTES);
        assert!(store.set_content_capacity(CONTENT_CAPACITY_BYTES).is_err());
        let hash = store.put_blob(b"protected bytes").unwrap();
        drop(store);
        let maintenance = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
        assert!(maintenance.set_content_capacity(0).is_err());
        maintenance.set_content_capacity(256 * 1024 * 1024).unwrap();
        assert!(maintenance.set_content_capacity(u64::MAX).is_err());
        drop(maintenance);
        let reopened = ReviewStore::open(directory.path().into()).unwrap();
        assert_eq!(reopened.content_capacity().unwrap(), 256 * 1024 * 1024);
        assert_eq!(reopened.read_blob(&hash).unwrap(), b"protected bytes");
    }
    #[test]
    fn capacity_rejects_new_content_but_preserves_existing_references() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(directory.path().into()).unwrap();
        let hash = store.put_blob_with_capacity(b"1234", 4).unwrap();
        assert!(
            store
                .put_blob_with_capacity(b"x", 4)
                .unwrap_err()
                .contains("容量上限")
        );
        assert_eq!(store.put_blob_with_capacity(b"1234", 1).unwrap(), hash);
        assert_eq!(store.read_blob(&hash).unwrap(), b"1234");
        assert_eq!(
            fs::read_dir(directory.path().join("blobs"))
                .unwrap()
                .count(),
            1
        );
        let other = ReviewStore::open(directory.path().into()).unwrap();
        let guard = store.reserve_blob_capacity(0, 4).unwrap();
        assert!(super::super::lock_directory(&directory.path().join("quota"), true).is_err());
        drop(guard);
        other.put_blob_with_capacity(b"x", 5).unwrap();
    }
}
