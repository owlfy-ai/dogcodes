//! Store reads use one bounded handle, not a metadata check followed by `fs::read`.
use super::{ReviewResult, same_file};
use std::{fs, io::Read, path::Path};

pub(super) fn read(path: &Path, limit: u64) -> ReviewResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "review file unavailable")?;
    read_checked(path, limit, &metadata)
}

fn read_checked(path: &Path, limit: u64, before: &fs::Metadata) -> ReviewResult<Vec<u8>> {
    if !before.is_file() || before.len() > limit {
        return Err("invalid or oversized review file".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path replaced with a FIFO must not block; a symlink must not be followed.
        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
        options.custom_flags(i32::try_from(flags.bits()).map_err(|_| "invalid review open flags")?);
    }
    let file = options.open(path).map_err(|_| "review file unavailable")?;
    let opened = file
        .metadata()
        .map_err(|_| "review file metadata unavailable")?;
    if !opened.is_file() || !same_file(before, &opened) {
        return Err("review file changed before read".into());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(limit.checked_add(1).ok_or("review read limit overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "review file read failed")?;
    let after = file
        .metadata()
        .map_err(|_| "review file metadata unavailable")?;
    let current = fs::symlink_metadata(path).map_err(|_| "review file disappeared during read")?;
    if bytes.len() as u64 > limit
        || bytes.len() as u64 != before.len()
        || !same_file(before, &after)
        || !same_file(before, &current)
    {
        return Err("review file changed during read".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_store_read_rejects_growth_and_non_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        fs::write(&path, b"a").unwrap();
        let before = fs::symlink_metadata(&path).unwrap();
        fs::write(&path, b"larger").unwrap();
        assert!(read_checked(&path, 8, &before).is_err());
        assert!(read(&path, 1).is_err());
        assert!(read(dir.path(), 100).is_err());
        assert_eq!(read(&path, 6).unwrap(), b"larger");
    }
    #[cfg(unix)]
    #[test]
    fn bounded_store_read_rejects_replacement_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        fs::write(&path, b"original").unwrap();
        let before = fs::symlink_metadata(&path).unwrap();
        fs::rename(&path, dir.path().join("saved")).unwrap();
        fs::write(&path, b"replaced").unwrap();
        assert!(read_checked(&path, 8, &before).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("saved"), &path).unwrap();
        assert!(read_checked(&path, 8, &before).is_err());
        assert!(read(&path, 8).is_err());
    }
}
