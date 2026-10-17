//! Compact, durable tombstones. Retiring identities precedes unlinking any
//! released payload/marker. The store's exclusive maintenance lock serializes
//! writers; readers fail closed if the index cannot be read.
use super::{ReviewResult, valid_hash};
use rusqlite::{Connection, OpenFlags};
use std::{fs, path::Path, time::Duration};

const NAME: &str = "retired-snapshots.sqlite";
// Main database bound; SQLite's transient rollback journal is additional.
const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

fn validate_file(path: &Path) -> ReviewResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err("invalid retired snapshot index".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("retired snapshot index unavailable".into()),
    }
}

pub(super) fn contains(directory: &Path, id: &str) -> ReviewResult<bool> {
    if !valid_hash(id) {
        return Err("invalid retired snapshot identity".into());
    }
    let path = directory.join(NAME);
    if !validate_file(&path)? {
        return Ok(false);
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "retired snapshot index unavailable")?;
    db.busy_timeout(Duration::from_secs(2))
        .map_err(|_| "retired index timeout unavailable")?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM retired WHERE id=?1)",
        [hex::decode(id).unwrap()],
        |row| row.get(0),
    )
    .map_err(|_| "retired snapshot index unreadable".into())
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
        .map(|id| {
            if !valid_hash(&id) {
                return Err("invalid retired snapshot identity".to_string());
            }
            hex::decode(id).map_err(|_| "invalid retired snapshot identity".into())
        })
        .collect::<ReviewResult<Vec<_>>>()?;
    if ids.is_empty() {
        return Ok(());
    }
    let path = directory.join(NAME);
    let created = !validate_file(&path)?;
    if created {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)
            .map_err(|_| "cannot create retired snapshot index")?;
    }
    let mut db = Connection::open(&path).map_err(|_| "cannot open retired snapshot index")?;
    db.busy_timeout(Duration::from_secs(2))
        .map_err(|_| "retired index timeout unavailable")?;
    db.execute_batch("PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE;")
        .map_err(|_| "cannot initialize retired snapshot index")?;
    let page_size: u64 = db
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(|_| "retired snapshot page size unavailable")?;
    let pages = capacity
        .checked_div(page_size)
        .filter(|pages| *pages >= 2)
        .ok_or("retired snapshot capacity is too small")?;
    let applied: u64 = db
        .pragma_update_and_check(None, "max_page_count", pages, |row| row.get(0))
        .map_err(|_| "cannot limit retired snapshot index capacity")?;
    if applied > pages {
        return Err("失效快照索引已超过容量上限；保留所有旧记录，未清理元数据".into());
    }
    if created {
        db.execute_batch("CREATE TABLE retired(id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=32)) WITHOUT ROWID;")
            .map_err(|_| "cannot create retired snapshot schema")?;
    } else {
        db.prepare("SELECT id FROM retired LIMIT 1")
            .map_err(|_| "retired snapshot schema missing; refusing to recreate")?;
    }
    let transaction = db
        .transaction()
        .map_err(|_| "cannot begin retired snapshot transaction")?;
    for id in ids {
        transaction
            .execute("INSERT OR IGNORE INTO retired(id) VALUES (?1)", [id])
            .map_err(|_| "无法写入失效快照索引（容量已满或存储错误）；旧标记不会删除")?;
    }
    transaction
        .commit()
        .map_err(|_| "cannot commit retired snapshot identities")?;
    super::super::sync_directory(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_index_rolls_back_the_batch_and_preserves_existing_identities() {
        let directory = tempfile::tempdir().unwrap();
        let existing = "f".repeat(64);
        insert_with_capacity(directory.path(), [existing.clone()], 8192).unwrap();
        let ids: Vec<_> = (0..1000)
            .map(|i| super::super::super::digest(i.to_string().as_bytes()))
            .collect();
        assert!(insert_with_capacity(directory.path(), ids.clone(), 8192).is_err());
        assert!(contains(directory.path(), &existing).unwrap());
        for id in ids {
            assert!(!contains(directory.path(), &id).unwrap());
        }
        assert!(fs::metadata(directory.path().join(NAME)).unwrap().len() <= 8192);
        insert_with_capacity(directory.path(), [existing], 8192).unwrap();
    }

    #[test]
    fn compact_index_survives_reopen_and_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let ids: Vec<_> = (0..1000)
            .map(|i| super::super::super::digest(i.to_string().as_bytes()))
            .collect();
        insert(directory.path(), ids.clone()).unwrap();
        insert(directory.path(), ids.clone()).unwrap();
        for id in &ids {
            assert!(contains(directory.path(), id).unwrap());
        }
        assert!(!contains(directory.path(), &"0".repeat(64)).unwrap());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        fs::write(directory.path().join(NAME), b"corrupt").unwrap();
        assert!(contains(directory.path(), &ids[0]).is_err());
        assert!(insert(directory.path(), ids.clone()).is_err());
        fs::write(directory.path().join(NAME), b"").unwrap();
        assert!(insert(directory.path(), ids).is_err());
    }
}
