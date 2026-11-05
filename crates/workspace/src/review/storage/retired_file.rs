//! Tombstones as fixed-size records in one append-only file, with the same contract
//! as the `SQLite` index: durable before anything is unlinked, bounded, fail closed.
use super::{ReviewResult, valid_hash};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const NAME: &str = "retired-snapshots.bin";
const RECORD: usize = 32;
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

fn read(path: &Path) -> ReviewResult<Vec<u8>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err("retired snapshot index unavailable".into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_INDEX_BYTES {
        return Err("invalid retired snapshot index".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| "retired snapshot index unavailable")?;
    if bytes.len() % RECORD != 0 {
        return Err("retired snapshot index unreadable".into());
    }
    Ok(bytes)
}

fn decode(id: &str) -> ReviewResult<Vec<u8>> {
    if !valid_hash(id) {
        return Err("invalid retired snapshot identity".into());
    }
    hex::decode(id).map_err(|_| "invalid retired snapshot identity".into())
}

pub(super) fn contains(directory: &Path, id: &str) -> ReviewResult<bool> {
    let id = decode(id)?;
    let bytes = read(&directory.join(NAME))?;
    Ok(bytes.chunks_exact(RECORD).any(|record| record == id))
}

pub(super) fn insert(directory: &Path, ids: impl IntoIterator<Item = String>) -> ReviewResult<()> {
    insert_with_capacity(directory, ids, MAX_INDEX_BYTES)
}

fn insert_with_capacity(
    directory: &Path,
    ids: impl IntoIterator<Item = String>,
    capacity: u64,
) -> ReviewResult<()> {
    let ids = ids
        .into_iter()
        .map(|id| decode(&id))
        .collect::<ReviewResult<Vec<_>>>()?;
    let path = directory.join(NAME);
    let existing = read(&path)?;
    let mut added = Vec::new();
    for id in ids {
        let known = existing.chunks_exact(RECORD).any(|record| record == id)
            || added.chunks_exact(RECORD).any(|record| record == id);
        if !known {
            added.extend_from_slice(&id);
        }
    }
    if added.is_empty() {
        return Ok(());
    }
    if existing.len() as u64 + added.len() as u64 > capacity {
        return Err("失效快照索引已超过容量上限；保留所有旧记录，未写入新记录".into());
    }
    let mut options = fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| "cannot open retired snapshot index")?;
    file.write_all(&added)
        .and_then(|()| file.sync_all())
        .map_err(|_| "cannot commit retired snapshot identities")?;
    super::super::sync_directory(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_index_keeps_what_it_has_and_refuses_the_batch() {
        let directory = tempfile::tempdir().unwrap();
        let existing = "f".repeat(64);
        insert_with_capacity(directory.path(), [existing.clone()], 64).unwrap();
        let ids = vec!["a".repeat(64), "b".repeat(64)];
        assert!(insert_with_capacity(directory.path(), ids.clone(), 64).is_err());
        assert!(contains(directory.path(), &existing).unwrap());
        assert!(!contains(directory.path(), &ids[0]).unwrap());
        insert_with_capacity(directory.path(), [existing], 64).unwrap();
    }

    #[test]
    fn identities_survive_and_corruption_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let id = "c".repeat(64);
        insert(directory.path(), [id.clone(), id.clone()]).unwrap();
        assert_eq!(
            fs::metadata(directory.path().join(NAME)).unwrap().len(),
            RECORD as u64
        );
        assert!(contains(directory.path(), &id).unwrap());
        assert!(!contains(directory.path(), &"0".repeat(64)).unwrap());
        fs::write(directory.path().join(NAME), b"corrupt").unwrap();
        assert!(contains(directory.path(), &id).is_err());
        assert!(insert(directory.path(), [id]).is_err());
    }
}
