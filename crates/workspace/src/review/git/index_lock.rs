//! Standard Git index.lock exclusion for staging and file discard/undo.
//! Existing locks are never removed or quarantined by Review.
use crate::review::{ReviewResult, ReviewStore};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct Lock {
    path: PathBuf,
    file: Option<fs::File>,
    published: bool,
}
impl Lock {
    fn owns_path(&self) -> bool {
        let Some(file) = &self.file else { return false };
        let (Ok(opened), Ok(current)) = (file.metadata(), fs::symlink_metadata(&self.path)) else {
            return false;
        };
        current.is_file() && crate::review::same_file(&opened, &current)
    }

    pub(super) fn acquire(destination: &Path) -> ReviewResult<Self> {
        let mut name = destination.as_os_str().to_owned();
        name.push(".lock");
        let path = PathBuf::from(name);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(
                |_| "Git index is locked or unavailable; finish the other Git operation first",
            )?;
        Ok(Self {
            path,
            file: Some(file),
            published: false,
        })
    }

    pub(super) fn acquire_index(
        destination: &Path,
        task: &str,
        id: &str,
        store: &ReviewStore,
    ) -> ReviewResult<Self> {
        store.index_recovery(task, id)?;
        Self::acquire(destination)
    }
    pub(super) fn publish(mut self, destination: &Path, bytes: &[u8]) -> ReviewResult<()> {
        if !self.owns_path() {
            return Err("Git index lock was replaced".into());
        }
        let file = self.file.as_mut().ok_or("metadata lock unavailable")?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "cannot persist metadata candidate")?;
        if !self.owns_path() {
            return Err("Git index lock changed before publication".into());
        }
        fs::rename(&self.path, destination).map_err(|_| "cannot publish metadata candidate")?;
        self.published = true;
        #[cfg(unix)]
        fs::File::open(destination.parent().ok_or("metadata parent missing")?)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "cannot sync metadata directory")?;
        Ok(())
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        if !self.published && self.owns_path() {
            // Close the handle before unlink on Windows. Do not remove a replaced lock.
            self.file.take();
            let _ = fs::remove_file(&self.path);
        }
    }
}
