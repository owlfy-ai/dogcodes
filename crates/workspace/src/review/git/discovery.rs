use super::super::{ReviewResult, canonical_root, excluded};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{Duration, Instant},
};

/// Discover only inside explicitly selected directories. Do not follow links,
/// inspect .git contents, or descend into dependency/build output directories.
pub(super) fn expand(directories: &[String]) -> ReviewResult<Vec<String>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut selected = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut entries = 0usize;
    for directory in directories {
        let root = canonical_root(Path::new(directory))?;
        selected.insert(
            root.to_str()
                .ok_or("non UTF-8 project directory")?
                .to_owned(),
        );
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            if Instant::now() >= deadline {
                return Err("repository discovery time budget exceeded".into());
            }
            if !visited.insert(directory.clone()) {
                continue;
            }
            let actual = directory
                .canonicalize()
                .map_err(|_| "repository discovery directory unavailable")?;
            if !actual.starts_with(&root) || actual != directory {
                return Err(
                    "directory changed or escaped project during repository discovery".into(),
                );
            }
            match fs::symlink_metadata(directory.join(".git")) {
                Ok(metadata) if metadata.is_dir() || metadata.is_file() => {
                    selected.insert(
                        directory
                            .to_str()
                            .ok_or("non UTF-8 repository directory")?
                            .to_owned(),
                    );
                }
                Ok(_) => return Err("unsupported .git marker in project".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("repository marker unavailable".into()),
            }
            if selected.len() > 128 {
                return Err("repository discovery root budget exceeded".into());
            }
            for entry in fs::read_dir(&directory).map_err(|_| "repository directory unreadable")? {
                entries += 1;
                if entries > 50_000 {
                    return Err("repository discovery entry budget exceeded".into());
                }
                let entry = entry.map_err(|_| "repository directory entry unreadable")?;
                let name = entry.file_name();
                let name = name.to_str().ok_or("non UTF-8 project entry")?;
                if excluded(name) {
                    continue;
                }
                if entry
                    .file_type()
                    .map_err(|_| "repository entry metadata unavailable")?
                    .is_dir()
                {
                    pending.push(entry.path());
                }
            }
        }
    }
    Ok(selected.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::expand;
    use std::fs;

    #[test]
    fn finds_nested_markers_and_deduplicates_overlapping_selection() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("a/.git")).unwrap();
        fs::create_dir_all(root.path().join("a/b")).unwrap();
        fs::write(root.path().join("a/b/.git"), "gitdir: elsewhere").unwrap();
        fs::create_dir_all(root.path().join("node_modules/ignored/.git")).unwrap();
        let paths = expand(&[
            root.path().to_str().unwrap().into(),
            root.path().join("a").to_str().unwrap().into(),
        ])
        .unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().any(|path| path.ends_with("a/b")));
        assert!(paths.iter().all(|path| !path.contains("node_modules")));
    }

    #[cfg(unix)]
    #[test]
    fn never_discovers_repositories_through_directory_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(outside.path().join(".git")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("external")).unwrap();
        assert_eq!(
            expand(&[root.path().to_str().unwrap().into()])
                .unwrap()
                .len(),
            1
        );
    }
}
