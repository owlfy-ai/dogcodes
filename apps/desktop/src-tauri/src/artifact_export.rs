use std::{io::Write, path::Path};

/// Publish only a complete, flushed artifact and never replace an existing path.
pub fn save_new_artifact(path: &Path, content: &[u8]) -> Result<(), String> {
    if !path.is_absolute() || content.len() > agent_core::artifacts::MAX_ARTIFACT_BYTES {
        return Err("invalid artifact export destination or size".into());
    }
    let parent = path.parent().ok_or("invalid artifact export destination")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "cannot create a private export file".to_string())?;
    temporary
        .write_all(content)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| "cannot write the complete artifact".to_string())?;
    temporary.persist_noclobber(path).map_err(|_| {
        "cannot save artifact; choose a new filename in a writable directory".to_string()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_exact_bytes_without_overwriting_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.patch");
        let bytes = b"diff --git\n+non-utf8: \xff\n";
        save_new_artifact(&path, bytes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(save_new_artifact(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(save_new_artifact(Path::new("relative"), bytes).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_an_existing_destination_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("keep.txt");
        let link = directory.path().join("export.patch");
        std::fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(save_new_artifact(&link, b"replacement").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
