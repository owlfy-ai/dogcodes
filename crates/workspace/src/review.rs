//! Review data is independent of per-turn handoff artifacts. Snapshots never
//! attribute filesystem writes to a particular process or agent.

mod bounded_read;
pub mod git;
pub mod recovery;
#[cfg(test)]
#[path = "review/scan_crash_tests.rs"]
mod scan_crash_tests;
pub mod storage;
pub mod text;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub type ReviewResult<T> = Result<T, String>;
static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CompareMode {
    Uncommitted,
    Unstaged,
    Staged,
    Committed,
    Branch,
    Session,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Symlink,
    Gitlink,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub hash: String,
    pub bytes: u64,
    pub kind: FileKind,
    pub executable: bool,
    pub binary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySnapshot {
    pub root: String,
    pub files: BTreeMap<String, FileVersion>,
    pub directories: BTreeMap<String, DirectoryVersion>,
    pub complete: bool,
    pub diagnostics: Vec<String>,
    pub excluded_entries: usize,
    /// Repository boundaries excluded from this non-Git directory snapshot.
    pub excluded_roots: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryVersion {
    pub readonly: bool,
    pub unix_mode: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub previous_path: Option<String>,
    pub kind: ChangeKind,
    pub before: Option<FileVersion>,
    pub after: Option<FileVersion>,
}

#[derive(Clone, Copy)]
pub struct SnapshotBudget {
    pub max_entries: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub timeout: Duration,
}

impl Default for SnapshotBudget {
    fn default() -> Self {
        Self {
            max_entries: 20_000,
            max_file_bytes: 4 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            timeout: Duration::from_secs(15),
        }
    }
}

/// Content and manifests live in app-private storage, not inside the project.
/// Blob filenames are validated digests, never project-relative user paths.
pub struct ReviewStore {
    directory: PathBuf,
    // The lock file is permanent; deleting it would split the lock domain.
    _storage_lock: storage::StorageLock,
    maintenance: bool,
    quota_cache: std::sync::Mutex<Option<(u64, u64, usize)>>,
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// Directory sync only exists on Unix; the Result serves that path.
#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
fn sync_directory(path: &Path) -> ReviewResult<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "review directory sync failed")?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn private_directory(path: &Path) -> ReviewResult<()> {
    fs::create_dir_all(path).map_err(|_| "review storage unavailable")?;
    if fs::symlink_metadata(path)
        .map_err(|_| "review storage unavailable")?
        .file_type()
        .is_symlink()
    {
        return Err("review storage cannot be a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "review storage permissions unavailable")?;
    }
    sync_directory(path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn write_once(path: &Path, bytes: &[u8]) -> ReviewResult<bool> {
    write_once_with_directory_sync(path, bytes, true)
}

// Only scanner batches defer the directory barrier; their manifest is not
// published until flush_scan_blobs has synced the shared blob directory.
fn write_once_with_directory_sync(
    path: &Path,
    bytes: &[u8],
    sync_parent: bool,
) -> ReviewResult<bool> {
    let temporary = path.with_extension(format!(
        "pending-{}-{}",
        std::process::id(),
        WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&temporary) {
        Ok(mut file) => {
            let result = (|| {
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| "review write failed")?;
                match fs::hard_link(&temporary, path) {
                    Ok(()) => {
                        if sync_parent {
                            sync_directory(path.parent().ok_or("review parent unavailable")?)?;
                        }
                        Ok(true)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                    Err(_) => Err("review atomic write failed".into()),
                }
            })();
            drop(file);
            let _ = fs::remove_file(&temporary);
            result
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => Err("review write failed".into()),
    }
}

impl ReviewStore {
    pub fn open(directory: PathBuf) -> ReviewResult<Self> {
        Self::open_locked(directory, false)
    }

    /// Maintenance must additionally protect cached/persistent references.
    /// This lock alone is not authorization to delete unreferenced blobs.
    pub fn try_open_maintenance(directory: PathBuf) -> ReviewResult<Self> {
        Self::open_locked(directory, true)
    }

    fn open_locked(directory: PathBuf, exclusive: bool) -> ReviewResult<Self> {
        if !directory.is_absolute() {
            return Err("review storage must be absolute".into());
        }
        private_directory(&directory)?;
        let storage_lock = storage::lock_directory(&directory, exclusive)?;
        for subdirectory in ["blobs", "baselines"] {
            private_directory(&directory.join(subdirectory))?;
        }
        Ok(Self {
            directory,
            _storage_lock: storage_lock,
            maintenance: exclusive,
            quota_cache: std::sync::Mutex::new(None),
        })
    }

    pub fn put_blob(&self, bytes: &[u8]) -> ReviewResult<String> {
        self.put_blob_with_capacity(bytes, self.content_capacity()?)
    }

    fn put_blob_with_capacity(&self, bytes: &[u8], capacity: u64) -> ReviewResult<String> {
        let hash = digest(bytes);
        let path = self.directory.join("blobs").join(&hash);
        if path
            .try_exists()
            .map_err(|_| "review content unavailable")?
        {
            if self.read_blob(&hash)? != bytes {
                return Err("review content verification failed".into());
            }
            return Ok(hash);
        }
        let reservation = self.reserve_blob_capacity(bytes.len() as u64, capacity)?;
        write_once(&path, bytes)?;
        // A partial write or unexpected storage modification is never accepted.
        let stored = self.read_blob(&hash)?;
        if stored != bytes {
            return Err("review content verification failed".into());
        }
        reservation.commit()?;
        Ok(hash)
    }

    pub fn read_blob(&self, hash: &str) -> ReviewResult<Vec<u8>> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err("invalid review content reference".into());
        }
        let path = self.directory.join("blobs").join(hash);
        let bytes = bounded_read::read(&path, 128 * 1024 * 1024)?;
        if digest(&bytes) != hash {
            return Err("review content verification failed".into());
        }
        Ok(bytes)
    }

    /// A bounded scanner batch shares one durable quota epoch. Each object is
    /// still independently synced and verified before the manifest can refer to it.
    /// On partial failure the reservation cache stays invalid, forcing recount.
    fn flush_scan_blobs(&self, pending: &mut BTreeMap<String, Vec<u8>>) -> ReviewResult<()> {
        self.flush_blobs_with_capacity(pending, self.content_capacity()?)
    }

    fn flush_blobs_with_capacity(
        &self,
        pending: &mut BTreeMap<String, Vec<u8>>,
        capacity: u64,
    ) -> ReviewResult<()> {
        let mut missing = BTreeMap::new();
        for (hash, bytes) in std::mem::take(pending) {
            let path = self.directory.join("blobs").join(&hash);
            if path
                .try_exists()
                .map_err(|_| "review content unavailable")?
            {
                if self.read_blob(&hash)? != bytes {
                    return Err("review content verification failed".into());
                }
            } else {
                missing.insert(hash, bytes);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let total = missing
            .values()
            .try_fold(0_u64, |sum, bytes| sum.checked_add(bytes.len() as u64))
            .ok_or("scan batch size overflow")?;
        let reservation = self.reserve_blobs_capacity(total, missing.len(), capacity)?;
        #[cfg(test)]
        scan_crash_tests::checkpoint(self, "reserved");
        let objects: Vec<_> = missing.into_iter().collect();
        // Bound both memory and concurrency. Workers publish distinct hashes;
        // the quota guard stays held until every worker exits, including errors.
        std::thread::scope(|scope| -> ReviewResult<()> {
            let mut workers = Vec::new();
            for chunk in objects.chunks(objects.len().div_ceil(4)) {
                workers.push(scope.spawn(move || -> ReviewResult<()> {
                    for (hash, bytes) in chunk {
                        write_once_with_directory_sync(
                            &self.directory.join("blobs").join(hash),
                            bytes,
                            false,
                        )?;
                        if self.read_blob(hash)? != *bytes {
                            return Err("review content verification failed".into());
                        }
                        #[cfg(test)]
                        scan_crash_tests::checkpoint(self, "partial");
                    }
                    Ok(())
                }));
            }
            let mut result = Ok(());
            for worker in workers {
                let completed = worker
                    .join()
                    .unwrap_or_else(|_| Err("scan content worker failed".into()));
                if result.is_ok() {
                    result = completed;
                }
            }
            result
        })?;
        #[cfg(test)]
        scan_crash_tests::checkpoint(self, "published");
        sync_directory(&self.directory.join("blobs"))?;
        reservation.commit()
    }

    fn baseline_path(&self, task: &str, root: &str) -> PathBuf {
        self.directory
            .join("baselines")
            .join(digest(format!("{task}\0{root}").as_bytes()))
    }

    /// Must run before the first agent execution, not on first opening Review.
    /// Repeated calls retain the original baseline, including across restarts.
    pub fn ensure_baseline(
        &self,
        task: &str,
        root: &Path,
        budget: SnapshotBudget,
    ) -> ReviewResult<DirectorySnapshot> {
        self.require_live_task(task)?;
        let root = canonical_root(root)?;
        let root_string = root.to_str().ok_or("workspace path is not UTF-8")?;
        let path = self.baseline_path(task, root_string);
        if path.exists() {
            return self.load_baseline(task, &root);
        }
        let snapshot = self.scan(&root, budget)?;
        // Persist incompleteness too: silently retrying later would move the
        // baseline past the first agent edits and manufacture a false origin.
        let bytes = serde_json::to_vec(&snapshot).map_err(|_| "invalid review manifest")?;
        write_once(&path, &bytes)?;
        self.load_baseline(task, &root)
    }

    pub fn load_baseline(&self, task: &str, root: &Path) -> ReviewResult<DirectorySnapshot> {
        let root = canonical_root(root)?;
        let root_string = root.to_str().ok_or("workspace path is not UTF-8")?;
        let path = self.baseline_path(task, root_string);
        let snapshot: DirectorySnapshot =
            serde_json::from_slice(&bounded_read::read(&path, 32 * 1024 * 1024)?)
                .map_err(|_| "invalid session baseline")?;
        if snapshot.root != root_string {
            return Err("session baseline root mismatch".into());
        }
        for relative in snapshot.files.keys() {
            validate_relative(relative)?;
        }
        Ok(snapshot)
    }

    pub fn scan(&self, root: &Path, budget: SnapshotBudget) -> ReviewResult<DirectorySnapshot> {
        self.scan_with_policy(root, budget, false)
    }

    /// Recovery backups include ignored/build/private files because overwriting
    /// a directory must not silently drop them. Contents stay in local CAS.
    /// Git metadata, nested repositories and this store remain separate bounds.
    pub fn scan_for_recovery(
        &self,
        root: &Path,
        budget: SnapshotBudget,
    ) -> ReviewResult<DirectorySnapshot> {
        self.scan_with_policy(root, budget, true)
    }

    fn scan_with_policy(
        &self,
        root: &Path,
        budget: SnapshotBudget,
        recovery: bool,
    ) -> ReviewResult<DirectorySnapshot> {
        let root = canonical_root(root)?;
        let mut snapshot = DirectorySnapshot {
            root: root.to_str().ok_or("workspace path is not UTF-8")?.into(),
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
            complete: true,
            diagnostics: vec![],
            excluded_entries: 0,
            excluded_roots: vec![],
        };
        let mut pending = vec![root.clone()];
        let start = Instant::now();
        let mut visited = 0;
        let mut total = 0;
        let mut scan_blobs = BTreeMap::new();
        let mut batch_bytes = 0_usize;
        while let Some(directory) = pending.pop() {
            if start.elapsed() > budget.timeout {
                snapshot.incomplete("scan time budget exceeded");
                break;
            }
            // Recheck queued directories to avoid following swapped symlinks.
            let Ok(metadata) = fs::symlink_metadata(&directory) else {
                snapshot.incomplete("directory unavailable");
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                snapshot.incomplete("directory changed during scan");
                continue;
            }
            if directory != root {
                match fs::symlink_metadata(directory.join(".git")) {
                    Ok(marker) if marker.is_file() || marker.is_dir() => {
                        let relative = directory
                            .strip_prefix(&root)
                            .map_err(|_| "nested repository outside root")?
                            .to_str()
                            .ok_or("non UTF-8 nested repository")?
                            .replace(std::path::MAIN_SEPARATOR, "/");
                        snapshot.excluded_roots.push(relative);
                        snapshot.excluded_entries += 1;
                        continue;
                    }
                    Ok(_) => {
                        snapshot.incomplete("unsupported nested repository marker");
                        continue;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        snapshot.incomplete("nested repository marker unavailable");
                        continue;
                    }
                }
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                snapshot.incomplete("directory cannot be read");
                continue;
            };
            if directory != root {
                let relative = directory
                    .strip_prefix(&root)
                    .map_err(|_| "directory outside root")?
                    .to_str()
                    .ok_or("non UTF-8 directory")?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                #[cfg(unix)]
                let unix_mode = {
                    use std::os::unix::fs::PermissionsExt;
                    Some(metadata.permissions().mode() & 0o7777)
                };
                #[cfg(not(unix))]
                let unix_mode = None;
                snapshot.directories.insert(
                    relative,
                    DirectoryVersion {
                        readonly: metadata.permissions().readonly(),
                        unix_mode,
                    },
                );
            }
            for entry in entries {
                visited += 1;
                if visited > budget.max_entries || start.elapsed() > budget.timeout {
                    snapshot.incomplete("scan entry or time budget exceeded");
                    return Ok(snapshot);
                }
                let Ok(entry) = entry else {
                    snapshot.incomplete("directory entry unavailable");
                    continue;
                };
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    snapshot.incomplete("non UTF-8 filename excluded");
                    continue;
                };
                if name == ".git" || (!recovery && excluded(name)) {
                    snapshot.excluded_entries += 1;
                    continue;
                }
                let path = entry.path();
                if path.starts_with(&self.directory) {
                    snapshot.excluded_entries += 1;
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|_| "path outside workspace")?
                    .to_str()
                    .ok_or("workspace path is not UTF-8")?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                let Ok(before) = fs::symlink_metadata(&path) else {
                    snapshot.incomplete("file changed during scan");
                    continue;
                };
                if before.is_dir() {
                    pending.push(path);
                    continue;
                }
                if !before.is_file() && !before.file_type().is_symlink() {
                    snapshot.incomplete("special file excluded");
                    continue;
                }
                if before.len() > budget.max_file_bytes as u64
                    || before.len() > budget.max_total_bytes.saturating_sub(total) as u64
                {
                    snapshot.incomplete("file or total content budget exceeded");
                    continue;
                }
                let content = if before.file_type().is_symlink() {
                    fs::read_link(&path)
                        .ok()
                        .and_then(|target| target.to_str().map(|s| s.as_bytes().to_vec()))
                } else {
                    let resolved = path.canonicalize().ok();
                    if resolved.as_ref().is_none_or(|p| !p.starts_with(&root)) {
                        snapshot.incomplete("file escaped workspace during scan");
                        continue;
                    }
                    File::open(&path).ok().and_then(|file| {
                        let opened = file.metadata().ok()?;
                        if !same_file(&before, &opened) {
                            return None;
                        }
                        let mut bytes = Vec::new();
                        file.take(budget.max_file_bytes as u64 + 1)
                            .read_to_end(&mut bytes)
                            .ok()
                            .map(|_| bytes)
                    })
                };
                let Some(bytes) = content else {
                    snapshot.incomplete("file content unavailable");
                    continue;
                };
                let Ok(after) = fs::symlink_metadata(&path) else {
                    snapshot.incomplete("file changed during scan");
                    continue;
                };
                if !same_file(&before, &after)
                    || bytes.len() > budget.max_file_bytes
                    || total + bytes.len() > budget.max_total_bytes
                {
                    snapshot.incomplete("file changed or exceeded budget during scan");
                    continue;
                }
                total += bytes.len();
                // Flush before adding to keep extra scanner memory <=16 MiB
                // (plus the one current file already bounded by SnapshotBudget).
                if batch_bytes + bytes.len() > 16 * 1024 * 1024 || scan_blobs.len() >= 64 {
                    self.flush_scan_blobs(&mut scan_blobs)?;
                    batch_bytes = 0;
                }
                let hash = digest(&bytes);
                if !scan_blobs.contains_key(&hash) {
                    batch_bytes += bytes.len();
                    scan_blobs.insert(hash.clone(), bytes.clone());
                }
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    before.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                snapshot.files.insert(
                    relative,
                    FileVersion {
                        hash,
                        bytes: bytes.len() as u64,
                        kind: if before.file_type().is_symlink() {
                            FileKind::Symlink
                        } else {
                            FileKind::File
                        },
                        executable,
                        binary: bytes.contains(&0) || std::str::from_utf8(&bytes).is_err(),
                    },
                );
            }
        }
        self.flush_scan_blobs(&mut scan_blobs)?;
        if start.elapsed() > budget.timeout {
            snapshot.incomplete("scan time budget exceeded during content publication");
        }
        Ok(snapshot)
    }
}

impl DirectorySnapshot {
    fn incomplete(&mut self, reason: &str) {
        self.complete = false;
        if !self.diagnostics.iter().any(|item| item == reason) {
            self.diagnostics.push(reason.into());
        }
    }
}

fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return false;
        }
    }
    before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && before.file_type() == after.file_type()
}

pub fn canonical_root(root: &Path) -> ReviewResult<PathBuf> {
    if !root.is_absolute() {
        return Err("workspace root must be absolute".into());
    }
    let canonical = root
        .canonicalize()
        .map_err(|_| "workspace root unavailable")?;
    if !canonical.is_dir() {
        return Err("workspace root must be a directory".into());
    }
    Ok(canonical)
}

pub fn validate_relative(path: &str) -> ReviewResult<()> {
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\\')
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("invalid workspace-relative path".into());
    }
    #[cfg(windows)]
    for component in path.split('/').filter(|part| !part.is_empty()) {
        validate_windows_component(component)?;
    }
    Ok(())
}

// Validate Windows-specific aliases before joining paths, including when the
// root uses an extended-length prefix. These checks never rewrite a filename.
#[cfg(any(windows, test))]
fn validate_windows_component(component: &str) -> ReviewResult<()> {
    if component.ends_with([' ', '.'])
        || component
            .chars()
            .any(|c| c <= '\u{1f}' || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err("unsupported Windows filename or alternate data stream".into());
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered_device
    {
        return Err("Windows device paths are not workspace files".into());
    }
    Ok(())
}

fn excluded(name: &str) -> bool {
    if name.starts_with(".agent-review-") {
        return true;
    }
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".DS_Store"
            | "dist"
            | "build"
            | ".next"
            | ".cache"
            | ".ssh"
            | "credentials.json"
            | "auth.json"
            | "id_rsa"
            | "id_ed25519"
    ) || name == ".env"
        || name.starts_with(".env.")
        || Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("key"))
}

pub fn compare_snapshots(
    before: &DirectorySnapshot,
    after: &DirectorySnapshot,
) -> ReviewResult<Vec<FileChange>> {
    if before.root != after.root {
        return Err("cannot compare different workspace roots".into());
    }
    if !before.complete || !after.complete {
        return Err(
            "review scan incomplete; missing files cannot be interpreted as deletions".into(),
        );
    }
    let paths: BTreeSet<_> = before.files.keys().chain(after.files.keys()).collect();
    let excluded: BTreeSet<_> = before
        .excluded_roots
        .iter()
        .chain(&after.excluded_roots)
        .collect();
    for path in &excluded {
        validate_relative(path)?;
    }
    Ok(paths
        .into_iter()
        .filter(|path| {
            !excluded
                .iter()
                .any(|root| *path == *root || path.starts_with(&format!("{root}/")))
        })
        .filter_map(|path| {
            let old = before.files.get(path);
            let new = after.files.get(path);
            if old == new {
                return None;
            }
            Some(FileChange {
                path: path.clone(),
                previous_path: None,
                kind: match (old, new) {
                    (None, _) => ChangeKind::Added,
                    (_, None) => ChangeKind::Deleted,
                    _ => ChangeKind::Modified,
                },
                before: old.cloned(),
                after: new.cloned(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn full_batch_capacity_refusal_preserves_baseline_and_allows_retry() {
        use super::*;
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("notes");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"old").unwrap();
        let data = fixture.path().join("store");
        let store = ReviewStore::open(data.clone()).unwrap();
        let baseline = store
            .ensure_baseline("task", &root, SnapshotBudget::default())
            .unwrap();
        let mut incomplete = serde_json::to_value(&baseline).unwrap();
        incomplete.as_object_mut().unwrap().remove("excludedRoots");
        assert!(serde_json::from_value::<DirectorySnapshot>(incomplete).is_err());
        let before = store.storage_usage([]).unwrap();
        assert_eq!(before.total_bytes, 3);
        let batch = BTreeMap::from([
            (digest(b"one"), b"one".to_vec()),
            (digest(b"two"), b"two".to_vec()),
        ]);
        assert!(
            store
                .flush_blobs_with_capacity(&mut batch.clone(), 8)
                .is_err()
        );
        assert_eq!(store.storage_usage([]).unwrap().blob_count, 1);
        for hash in batch.keys() {
            assert!(!data.join("blobs").join(hash).exists());
        }
        assert_eq!(
            store.read_blob(&baseline.files["file"].hash).unwrap(),
            b"old"
        );
        // A fresh handle cannot inherit a falsely committed quota cache.
        drop(store);
        let reopened = ReviewStore::open(data.clone()).unwrap();
        reopened
            .flush_blobs_with_capacity(&mut batch.clone(), 9)
            .unwrap();
        let usage = reopened.storage_usage([]).unwrap();
        assert_eq!(
            (
                usage.blob_count,
                usage.total_bytes,
                usage.missing_references
            ),
            (3, 9, 0)
        );
        assert_eq!(
            reopened.load_baseline("task", &root).unwrap().files,
            baseline.files
        );
        for (hash, bytes) in &batch {
            assert_eq!(&reopened.read_blob(hash).unwrap(), bytes);
        }
        // Existing content is reusable even when no admission space remains.
        reopened
            .flush_blobs_with_capacity(&mut batch.clone(), 1)
            .unwrap();
        assert!(
            reopened
                .flush_blobs_with_capacity(&mut BTreeMap::from([(digest(b"x"), b"x".to_vec())]), 9)
                .is_err()
        );
        assert_eq!(fs::read(root.join("file")).unwrap(), b"old");
    }

    #[test]
    fn scan_batch_joins_failed_workers_without_committing_quota_cache() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(directory.path().into()).unwrap();
        let good = digest(b"good");
        // Inject an invalid object into the private batch to exercise a worker
        // verification failure after publication (not a real filesystem crash).
        let bad = digest(b"expected");
        let mut batch = BTreeMap::from([
            (good.clone(), b"good".to_vec()),
            (bad.clone(), b"wrong".to_vec()),
        ]);
        assert!(store.flush_scan_blobs(&mut batch).is_err());
        assert!(store.quota_cache.lock().unwrap().is_none());
        assert_eq!(store.read_blob(&good).unwrap(), b"good");
        assert!(store.read_blob(&bad).is_err());
        assert_eq!(
            fs::read_dir(directory.path().join("blobs"))
                .unwrap()
                .count(),
            2
        );
        assert_eq!(
            fs::read_dir(directory.path().join("baselines"))
                .unwrap()
                .count(),
            0
        );
        // All workers and the reservation guard are gone on return. Another
        // handle can publish and must count both objects, even the invalid one.
        let other = ReviewStore::open(directory.path().into()).unwrap();
        other.put_blob_with_capacity(b"x", 10).unwrap();
        assert_eq!(other.quota_cache.lock().unwrap().unwrap().1, 10);
        assert_eq!(other.quota_cache.lock().unwrap().unwrap().2, 3);
    }
    #[test]
    fn recovery_snapshot_preserves_empty_directories_and_modes() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("empty/nested")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.path().join("empty/nested"),
                std::fs::Permissions::from_mode(0o750),
            )
            .unwrap();
        }
        let store = super::ReviewStore::open(data.path().into()).unwrap();
        let snapshot = store
            .scan_for_recovery(root.path(), super::SnapshotBudget::default())
            .unwrap();
        assert!(snapshot.complete);
        assert!(snapshot.files.is_empty());
        assert_eq!(snapshot.directories.len(), 2);
        #[cfg(unix)]
        assert_eq!(snapshot.directories["empty/nested"].unix_mode, Some(0o750));
        let decoded: super::DirectorySnapshot =
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded.directories, snapshot.directories);
    }

    use super::*;

    #[test]
    fn windows_filename_guard_rejects_aliases_without_rewriting_names() {
        for name in [
            "file:stream",
            "file::$DATA",
            "NUL",
            "nul.txt",
            "Con .log",
            "COM1",
            "Lpt9.ext",
            "COM¹",
            "LPT².log",
            "CONIN$",
            "CONOUT$.txt",
            "file.",
            "file ",
            "a?b",
            "a|b",
            "a\u{1f}b",
        ] {
            assert!(
                validate_windows_component(name).is_err(),
                "accepted {name:?}"
            );
        }
        for name in [
            "normal.txt",
            "console",
            "auxiliary",
            "COM10",
            "company",
            "文件名.rs",
            ".gitignore",
            "space inside.txt",
        ] {
            assert!(
                validate_windows_component(name).is_ok(),
                "rejected {name:?}"
            );
        }
        #[cfg(not(windows))]
        assert!(validate_relative("legal:unix-name").is_ok());
        #[cfg(windows)]
        assert!(validate_relative("directory/file:stream").is_err());
    }

    #[test]
    fn nested_repository_boundaries_apply_to_both_sides_and_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::create_dir_all(root.path().join("child")).unwrap();
        fs::write(root.path().join("child/file"), "before").unwrap();
        fs::write(root.path().join("sibling"), "before").unwrap();
        let before = store.scan(root.path(), SnapshotBudget::default()).unwrap();
        fs::create_dir(root.path().join("child/.git")).unwrap();
        fs::write(root.path().join("child/file"), "after").unwrap();
        fs::write(root.path().join("sibling"), "after").unwrap();
        let after = store
            .ensure_baseline("task", root.path(), SnapshotBudget::default())
            .unwrap();
        assert_eq!(after.excluded_roots, ["child"]);
        assert!(!after.files.contains_key("child/file"));
        let changes = compare_snapshots(&before, &after).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "sibling");
        // Removing a repository marker must not turn excluded child files into additions.
        fs::remove_dir(root.path().join("child/.git")).unwrap();
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        let baseline = reopened.load_baseline("task", root.path()).unwrap();
        let current = reopened
            .scan(root.path(), SnapshotBudget::default())
            .unwrap();
        assert!(compare_snapshots(&baseline, &current).unwrap().is_empty());
    }

    #[tokio::test]
    async fn startup_validates_roots_without_creating_review_data() {
        use agent_core::WorkspaceContextProvider;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let storage = data.path().join("not-created");
        let provider =
            crate::GitWorkspaceContextProvider::default().with_review_directory(storage.clone());
        provider
            .validate_workspace_roots(&[workspace.path().to_string_lossy().into_owned()])
            .await
            .unwrap();
        assert!(!storage.exists());
        assert!(
            provider
                .validate_workspace_roots(&[workspace
                    .path()
                    .join("missing")
                    .to_string_lossy()
                    .into_owned()])
                .await
                .is_err()
        );
        assert!(!storage.exists());
    }

    #[test]
    fn session_baseline_survives_reopen_and_tracks_only_net_changes() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("a.txt"), "before\n").unwrap();
        let initial = store
            .ensure_baseline("task-a", root.path(), SnapshotBudget::default())
            .unwrap();
        fs::write(root.path().join("a.txt"), "middle\n").unwrap();
        fs::write(root.path().join("a.txt"), "after\n").unwrap();
        fs::write(root.path().join("binary"), [0, 1, 2]).unwrap();
        let reopened = ReviewStore::open(data.path().into()).unwrap();
        let baseline = reopened
            .ensure_baseline("task-a", root.path(), SnapshotBudget::default())
            .unwrap();
        assert_eq!(baseline.files, initial.files);
        let current = reopened
            .scan(root.path(), SnapshotBudget::default())
            .unwrap();
        let changes = compare_snapshots(&baseline, &current).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        assert!(current.files["binary"].binary);
        assert_eq!(
            reopened.read_blob(&initial.files["a.txt"].hash).unwrap(),
            b"before\n"
        );
        fs::write(root.path().join("a.txt"), "before\n").unwrap();
        fs::remove_file(root.path().join("binary")).unwrap();
        assert!(
            compare_snapshots(
                &baseline,
                &reopened
                    .scan(root.path(), SnapshotBudget::default())
                    .unwrap()
            )
            .unwrap()
            .is_empty()
        );
        fs::remove_file(root.path().join("a.txt")).unwrap();
        assert_eq!(
            compare_snapshots(
                &baseline,
                &reopened
                    .scan(root.path(), SnapshotBudget::default())
                    .unwrap()
            )
            .unwrap()[0]
                .kind,
            ChangeKind::Deleted
        );
    }

    #[test]
    fn incomplete_scan_never_manufactures_deletions_or_moves_baseline() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join("a"), "long content").unwrap();
        let budget = SnapshotBudget {
            max_file_bytes: 2,
            ..SnapshotBudget::default()
        };
        let baseline = store.ensure_baseline("task", root.path(), budget).unwrap();
        assert!(!baseline.complete);
        let current = store.scan(root.path(), SnapshotBudget::default()).unwrap();
        assert!(compare_snapshots(&baseline, &current).is_err());
        assert!(
            !store
                .ensure_baseline("task", root.path(), SnapshotBudget::default())
                .unwrap()
                .complete
        );
    }

    #[test]
    fn references_are_validated_and_credentials_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        fs::write(root.path().join(".env"), "secret").unwrap();
        let snapshot = store.scan(root.path(), SnapshotBudget::default()).unwrap();
        assert!(snapshot.files.is_empty());
        assert_eq!(snapshot.excluded_entries, 1);
        for path in ["../a", "/a", "a/../b", "", "C:\\secret"] {
            assert!(validate_relative(path).is_err());
        }
        assert!(store.read_blob("../../secret").is_err());
        let hash = store.put_blob(b"content").unwrap();
        fs::write(data.path().join("blobs").join(&hash), "corrupt").unwrap();
        assert!(store.read_blob(&hash).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_recorded_without_reading_their_targets() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "must not be read").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let snapshot = store.scan(root.path(), SnapshotBudget::default()).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files["link"].kind, FileKind::Symlink);
        assert_eq!(
            store.read_blob(&snapshot.files["link"].hash).unwrap(),
            outside.path().to_str().unwrap().as_bytes()
        );
    }
}
