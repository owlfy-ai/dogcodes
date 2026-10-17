//! Standalone index publication intent. Preparing and completing are separate
//! durable steps; neither writes the repository's real index.
use super::{
    ReviewResult, ReviewStore, digest, private_directory, valid_id, validate_relative, write_once,
};
use crate::review::git::operations::RepositoryVersion;
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path, sync::atomic::Ordering};

// A comparison contains at most 2,000 files; each rename can touch two paths.
const MAX_INDEX_RECOVERY_PATHS: usize = 4_000;
const MAX_INDEX_RECOVERY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRecovery {
    pub task_id: String,
    pub root: String,
    pub paths: Vec<String>,
    pub stage: bool,
    pub original: Option<String>,
    pub candidate: String,
    pub operation: String,
    pub repository: RepositoryVersion,
}

pub struct IndexRecoveryContent<'a> {
    pub original: Option<&'a [u8]>,
    pub candidate: &'a [u8],
    pub repository: RepositoryVersion,
}

/// An observation, not permission to write. Mutation callers must re-read the
/// index under its repository lock immediately before publishing anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexRecoveryState {
    Prepared,
    Published,
    Applied,
    Diverged,
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;

impl ReviewStore {
    pub fn index_recovery_state(
        &self,
        task: &str,
        id: &str,
        current: Option<&[u8]>,
    ) -> ReviewResult<IndexRecoveryState> {
        let record = self.index_recovery(task, id)?;
        let marker = self
            .directory
            .join("index-recovery")
            .join(format!("{id}.applied"));
        let applied = match fs::symlink_metadata(&marker) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() != 7 {
                    return Err("invalid index completion marker".into());
                }
                let file = fs::File::open(&marker).map_err(|_| "index completion unavailable")?;
                let opened = file
                    .metadata()
                    .map_err(|_| "index completion unavailable")?;
                if !crate::review::same_file(&metadata, &opened) {
                    return Err("index completion changed while opening".into());
                }
                let mut bytes = Vec::new();
                file.take(8)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "index completion unavailable")?;
                if bytes != b"applied" {
                    return Err("invalid index completion marker".into());
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err("index completion unavailable".into()),
        };
        let current = current.map(digest);
        // Completed historical operations must never become resumable merely
        // because a later operation happened to recreate the original bytes.
        if applied {
            return Ok(IndexRecoveryState::Applied);
        }
        if current.as_deref() == Some(record.candidate.as_str()) {
            Ok(IndexRecoveryState::Published)
        } else if current == record.original {
            Ok(IndexRecoveryState::Prepared)
        } else {
            Ok(IndexRecoveryState::Diverged)
        }
    }

    pub fn prepare_index_recovery(
        &self,
        task: &str,
        root: &Path,
        paths: Vec<String>,
        stage: bool,
        content: IndexRecoveryContent<'_>,
    ) -> ReviewResult<String> {
        if task.is_empty() || task.contains('\0') {
            return Err("invalid index recovery task".into());
        }
        if paths.is_empty() || paths.len() > MAX_INDEX_RECOVERY_PATHS {
            return Err("invalid index operation paths".into());
        }
        for path in &paths {
            validate_relative(path)?;
        }
        let root = root
            .canonicalize()
            .map_err(|_| "index repository unavailable")?;
        if !root.is_dir() {
            return Err("index repository must be a directory".into());
        }
        let original = content
            .original
            .map(|bytes| self.put_blob(bytes))
            .transpose()?;
        if original != content.repository.index {
            return Err("index recovery base does not match repository".into());
        }
        let candidate = self.put_blob(content.candidate)?;
        let operation = format!(
            "{}:{}:{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "clock unavailable")?
                .as_nanos(),
            crate::review::WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let record = IndexRecovery {
            task_id: task.into(),
            root: root.to_string_lossy().into_owned(),
            paths,
            stage,
            original,
            candidate,
            operation,
            repository: content.repository,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| "cannot encode index recovery")?;
        if bytes.len() > MAX_INDEX_RECOVERY_BYTES {
            return Err("index recovery exceeds read budget".into());
        }
        let id = digest(&bytes);
        self.persist_snapshot_references(
            &format!("index-recovery/{id}"),
            record
                .original
                .iter()
                .chain(std::iter::once(&record.candidate))
                .cloned(),
        )?;
        let directory = self.directory.join("index-recovery");
        private_directory(&directory)?;
        write_once(&directory.join(&id), &bytes)?;
        Ok(id)
    }

    pub fn index_recovery(&self, task: &str, id: &str) -> ReviewResult<IndexRecovery> {
        if task.is_empty() || task.contains('\0') {
            return Err("invalid index recovery task".into());
        }
        let record = self.read_index_manifest(id)?;
        if record.task_id != task {
            return Err("index recovery scope mismatch".into());
        }
        for hash in record
            .original
            .iter()
            .chain(std::iter::once(&record.candidate))
        {
            self.read_blob(hash)?;
        }
        Ok(record)
    }

    /// List only this task's records. Validate manifests before trusting task
    /// ownership, but do not load other tasks' index contents.
    pub fn index_recovery_entries(&self, task: &str) -> ReviewResult<Vec<(String, IndexRecovery)>> {
        if task.is_empty() || task.contains('\0') {
            return Err("invalid index recovery task".into());
        }
        let entries = match fs::read_dir(self.directory.join("index-recovery")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("index recovery listing unavailable".into()),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut result = Vec::new();
        for (count, entry) in entries.enumerate() {
            if count >= 10000 || std::time::Instant::now() >= deadline {
                return Err("index recovery listing budget exceeded".into());
            }
            let entry = entry.map_err(|_| "index recovery entry unavailable")?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if valid_id(&id).is_err() {
                continue;
            }
            let record = self.read_index_manifest(&id)?;
            if record.task_id == task {
                result.push((id, record));
            }
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }

    fn read_index_manifest(&self, id: &str) -> ReviewResult<IndexRecovery> {
        valid_id(id)?;
        let path = self.directory.join("index-recovery").join(id);
        let metadata = fs::symlink_metadata(&path).map_err(|_| "index recovery unavailable")?;
        if !metadata.is_file() || metadata.len() > MAX_INDEX_RECOVERY_BYTES as u64 {
            return Err("invalid index recovery".into());
        }
        let file = fs::File::open(&path).map_err(|_| "cannot open index recovery")?;
        let opened = file
            .metadata()
            .map_err(|_| "cannot inspect index recovery")?;
        if !opened.is_file() || !crate::review::same_file(&metadata, &opened) {
            return Err("index recovery changed while opening".into());
        }
        let mut bytes = Vec::new();
        file.take((MAX_INDEX_RECOVERY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read index recovery")?;
        if bytes.len() > MAX_INDEX_RECOVERY_BYTES {
            return Err("index recovery exceeds read budget".into());
        }
        if digest(&bytes) != id {
            return Err("index recovery identity mismatch".into());
        }
        let record: IndexRecovery =
            serde_json::from_slice(&bytes).map_err(|_| "invalid index recovery")?;
        if record.task_id.is_empty()
            || record.task_id.contains('\0')
            || !Path::new(&record.root).is_absolute()
        {
            return Err("index recovery scope mismatch".into());
        }
        if record.paths.is_empty()
            || record.paths.len() > MAX_INDEX_RECOVERY_PATHS
            || record.operation.is_empty()
        {
            return Err("invalid index operation".into());
        }
        if record.original != record.repository.index {
            return Err("index recovery base mismatch".into());
        }
        for path in &record.paths {
            validate_relative(path)?;
        }
        Ok(record)
    }

    pub fn complete_index_recovery(&self, task: &str, id: &str) -> ReviewResult<()> {
        self.index_recovery(task, id)?;
        write_once(
            &self
                .directory
                .join("index-recovery")
                .join(format!("{id}.applied")),
            b"applied",
        )?;
        Ok(())
    }
}
