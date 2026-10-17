//! Isolated acceptance storage only. Reopening never accepts an absolute path.
use std::{
    io,
    io::Write,
    path::{Path, PathBuf},
};

const PREFIX: &str = "dogcodes-review-acceptance-";
const MARKER: &str = "acceptance-seeded";
const CONTENT: &[u8] = b"dogcodes isolated review fixture v1\n";

pub fn allocate() -> io::Result<PathBuf> {
    match std::env::var_os("DOGCODES_REVIEW_REOPEN") {
        Some(name) => reopen(&std::env::temp_dir(), &name.to_string_lossy()),
        None => Ok(tempfile::Builder::new().prefix(PREFIX).tempdir()?.keep()),
    }
}

fn reopen(temporary_root: &Path, name: &str) -> io::Result<PathBuf> {
    if !name.starts_with(PREFIX)
        || name.len() <= PREFIX.len()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(io::Error::other(
            "reopen requires an isolated fixture basename",
        ));
    }
    let root = temporary_root.canonicalize()?;
    let directory = root.join(name);
    if !std::fs::symlink_metadata(&directory)?.is_dir()
        || directory.canonicalize()?.parent() != Some(root.as_path())
        || !is_seeded(&directory)?
    {
        return Err(io::Error::other(
            "reopen requires a seeded, real isolated directory",
        ));
    }
    Ok(directory)
}

pub fn is_seeded(directory: &Path) -> io::Result<bool> {
    let marker = directory.join(MARKER);
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file()
        || metadata.len() != CONTENT.len() as u64
        || std::fs::read(marker)? != CONTENT
    {
        return Err(io::Error::other("invalid isolated fixture marker"));
    }
    Ok(true)
}

pub fn mark_seeded(directory: &Path) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(MARKER))?;
    file.write_all(CONTENT)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopen_preserves_data_and_requires_completed_seed() {
        let root = tempfile::tempdir().unwrap();
        let name = format!("{PREFIX}test");
        let fixture = root.path().join(&name);
        std::fs::create_dir(&fixture).unwrap();
        assert!(reopen(root.path(), &name).is_err());
        std::fs::write(fixture.join("recovery"), b"pending").unwrap();
        mark_seeded(&fixture).unwrap();
        assert_eq!(
            reopen(root.path(), &name).unwrap(),
            fixture.canonicalize().unwrap()
        );
        assert_eq!(std::fs::read(fixture.join("recovery")).unwrap(), b"pending");
        assert!(mark_seeded(&fixture).is_err());
        for invalid in [
            "../escape",
            "/tmp/other",
            "ordinary",
            "dogcodes-review-acceptance-../x",
        ] {
            assert!(reopen(root.path(), invalid).is_err());
        }
        std::fs::write(fixture.join(MARKER), b"invalid").unwrap();
        assert!(reopen(root.path(), &name).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reopen_rejects_linked_directory_and_marker() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        mark_seeded(outside.path()).unwrap();
        let name = format!("{PREFIX}linked");
        std::os::unix::fs::symlink(outside.path(), root.path().join(&name)).unwrap();
        assert!(reopen(root.path(), &name).is_err());
        let name = format!("{PREFIX}marker");
        let fixture = root.path().join(&name);
        std::fs::create_dir(&fixture).unwrap();
        std::os::unix::fs::symlink(outside.path().join(MARKER), fixture.join(MARKER)).unwrap();
        assert!(reopen(root.path(), &name).is_err());
    }
}
