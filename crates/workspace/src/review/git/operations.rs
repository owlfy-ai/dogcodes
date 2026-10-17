//! Mutations prepare a private candidate, then lock and revalidate publication.
//! Only the candidate index is published, never a broad reset or checkout.
use super::{GitComparison, GitReview, index_lock::Lock};
use crate::review::{CompareMode, FileKind, ReviewResult, ReviewStore, digest, validate_relative};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryVersion {
    pub head: Option<String>,
    pub head_file: String,
    pub index: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum IndexAction {
    Stage,
    Unstage,
}

pub struct IndexLock {
    _guard: Lock,
}

#[derive(Clone, Copy)]
pub struct FileRecoveryLockRequest<'a> {
    pub task: &'a str,
    pub record: &'a str,
    pub grouped: bool,
}

#[cfg(test)]
#[path = "index_lock_tests.rs"]
mod lock_tests;

pub(super) fn read_head(path: &Path) -> ReviewResult<Vec<u8>> {
    crate::review::bounded_read::read(path, 4096)
        .map_err(|error| format!("cannot safely read Git HEAD: {error}"))
}

pub(super) fn read_index(path: &Path) -> ReviewResult<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("index unavailable".into()),
    };
    if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
        return Err("unsupported or oversized index".into());
    }
    let file = fs::File::open(path).map_err(|_| "cannot open index")?;
    read_index_handle(path, file, &metadata).map(Some)
}

fn read_index_handle(
    path: &Path,
    mut file: fs::File,
    before: &fs::Metadata,
) -> ReviewResult<Vec<u8>> {
    const LIMIT: u64 = 32 * 1024 * 1024;
    let opened = file.metadata().map_err(|_| "cannot inspect opened index")?;
    if !opened.is_file() || opened.len() > LIMIT || !crate::review::same_file(before, &opened) {
        return Err("index changed while opening".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read index")?;
    if bytes.len() as u64 > LIMIT {
        return Err("index exceeds read budget".into());
    }
    let after = file.metadata().map_err(|_| "cannot recheck index handle")?;
    let current = fs::symlink_metadata(path).map_err(|_| "index disappeared while reading")?;
    if !crate::review::same_file(&opened, &after)
        || !crate::review::same_file(&after, &current)
        || bytes.len() as u64 != after.len()
    {
        return Err("index changed while reading".into());
    }
    Ok(bytes)
}

impl GitReview {
    /// Explicitly resume an already prepared index. Working files are never
    /// touched. The host must hold its workspace mutation gate and check scope.
    pub async fn resume_index_recovery(
        &self,
        task: &str,
        id: &str,
        store: &ReviewStore,
        confirmed: bool,
    ) -> ReviewResult<()> {
        use crate::review::recovery::index::IndexRecoveryState;
        if !confirmed {
            return Err("index recovery requires confirmation".into());
        }
        // Verifies canonical repository scope before creating any lock.
        self.inspect_index_recovery(task, id, store).await?;
        let record = store.index_recovery(task, id)?;
        let root = Path::new(&record.root);
        let index = self.git_path(root, "index").await?;
        let lock = Lock::acquire_index(&index, task, id, store)?;
        let current = read_index(&index)?;
        let phase = store.index_recovery_state(task, id, current.as_deref())?;
        if phase == IndexRecoveryState::Applied {
            return Ok(());
        }
        if phase == IndexRecoveryState::Diverged {
            return Err("index changed after interrupted operation; refusing recovery".into());
        }
        let version = self.repository_version(root).await?;
        if version.head != record.repository.head
            || version.head_file != record.repository.head_file
        {
            return Err("HEAD changed after interrupted index operation".into());
        }
        if version.index != current.as_ref().map(|bytes| digest(bytes)) {
            return Err("index changed while checking recovery".into());
        }
        if phase == IndexRecoveryState::Prepared {
            let candidate = store.read_blob(&record.candidate)?;
            if self.repository_version(root).await? != version {
                return Err("repository changed before index recovery publication".into());
            }
            lock.publish(&index, &candidate)?;
        }
        store.complete_index_recovery(task, id)
    }

    /// Read-only recovery observation. Project authorization belongs to the
    /// host; this verifies the recorded repository still resolves to itself.
    pub async fn inspect_index_recovery(
        &self,
        task: &str,
        id: &str,
        store: &ReviewStore,
    ) -> ReviewResult<crate::review::recovery::index::IndexRecoveryState> {
        let record = store.index_recovery(task, id)?;
        let root = Path::new(&record.root);
        if crate::review::canonical_root(root)? != root
            || !self
                .discover(std::slice::from_ref(&record.root))
                .await?
                .iter()
                .any(|entry| entry.git && entry.path == record.root)
        {
            return Err("original index repository unavailable".into());
        }
        let index = self.git_path(root, "index").await?;
        let bytes = read_index(&index)?;
        store.index_recovery_state(task, id, bytes.as_deref())
    }

    /// Convert a frozen repository blob to its checkout representation before
    /// the recovery transaction writes it. Git owns EOL/encoding/ident semantics.
    pub async fn working_replacement(
        &self,
        comparison: &GitComparison,
        change: &crate::review::FileChange,
        store: &ReviewStore,
    ) -> ReviewResult<Option<crate::review::FileVersion>> {
        let Some(before) = &change.before else {
            return Ok(None);
        };
        if before.kind != FileKind::File {
            return Ok(Some(before.clone()));
        }
        let path = change.previous_path.as_deref().unwrap_or(&change.path);
        validate_relative(path)?;
        let revision = match comparison.mode {
            CompareMode::Uncommitted => format!("HEAD:{path}"),
            CompareMode::Unstaged => format!(":{path}"),
            _ => return Err("checkout conversion requires a working-tree comparison".into()),
        };
        let root = Path::new(&comparison.root.path);
        let oid = self
            .text(root, &["rev-parse", "--verify", &revision])
            .await?;
        let oid = oid.trim();
        let raw = self
            .run(root, &["cat-file", "blob", oid], super::MAX_BYTES)
            .await?;
        if raw != store.read_blob(&before.hash)? {
            return Err("repository blob changed since preview".into());
        }
        let converted = self
            .run(
                root,
                &["cat-file", "--filters", &format!("--path={path}"), oid],
                super::MAX_BYTES,
            )
            .await?;
        let mut version = before.clone();
        version.hash = store.put_blob(&converted)?;
        version.bytes = converted.len() as u64;
        version.binary = converted.contains(&0);
        Ok(Some(version))
    }

    /// Hold this guard throughout a working-file transaction to exclude Git
    /// operations which cooperate with the standard index lock.
    pub async fn lock_working(&self, root: &Path) -> ReviewResult<IndexLock> {
        let path = self.git_path(root, "index").await?;
        Ok(IndexLock {
            _guard: Lock::acquire(&path)?,
        })
    }

    /// Validate existing file transactions before acquiring their repository lock.
    pub async fn lock_file_recovery(
        &self,
        root: &Path,
        request: FileRecoveryLockRequest<'_>,
        store: &ReviewStore,
    ) -> ReviewResult<IndexLock> {
        let root = Self::file_recovery_root(root, request, store)?;
        let path = self.git_path(&root, "index").await?;
        Ok(IndexLock {
            _guard: Lock::acquire(&path)?,
        })
    }

    pub(super) fn file_recovery_root(
        root: &Path,
        request: FileRecoveryLockRequest<'_>,
        store: &ReviewStore,
    ) -> ReviewResult<PathBuf> {
        let records = if request.grouped {
            store
                .recovery_group(request.task, request.record)?
                .1
                .map(|entry| entry.record)
                .to_vec()
        } else {
            let entry = store.recovery_record(request.task, request.record)?;
            if entry.record.group_member {
                return Err("group member requires whole-group lock".into());
            }
            vec![entry.record]
        };
        let root = root
            .canonicalize()
            .map_err(|_| "file recovery root unavailable")?;
        if records.is_empty() || records.iter().any(|record| Path::new(&record.root) != root) {
            return Err("file recovery lock root mismatch".into());
        }
        Ok(root)
    }

    pub async fn check_working_preview(
        &self,
        expected: &GitComparison,
        store: &ReviewStore,
    ) -> ReviewResult<()> {
        if !matches!(
            expected.mode,
            CompareMode::Uncommitted | CompareMode::Unstaged
        ) {
            return Err("working-file restore requires a current working-tree comparison".into());
        }
        let current = self
            .compare(&expected.root, expected.mode.clone(), None, 0, store)
            .await?;
        if current.version != expected.version
            || current.repository_version != expected.repository_version
        {
            return Err("repository changed since preview; refresh before discarding".into());
        }
        Ok(())
    }
    pub(super) async fn git_path(&self, root: &Path, name: &str) -> ReviewResult<PathBuf> {
        let path = self
            .text(
                root,
                &["rev-parse", "--path-format=absolute", "--git-path", name],
            )
            .await?;
        let path = PathBuf::from(path.trim_end_matches('\n'));
        if !path.is_absolute() {
            return Err("Git metadata path is not absolute".into());
        }
        Ok(path)
    }

    pub async fn repository_version(&self, root: &Path) -> ReviewResult<RepositoryVersion> {
        let index_path = self.git_path(root, "index").await?;
        let head_path = self.git_path(root, "HEAD").await?;
        let head_file = read_head(&head_path)?;
        Ok(RepositoryVersion {
            head: self.head(root).await?,
            head_file: digest(&head_file),
            index: read_index(&index_path)?.map(|bytes| digest(&bytes)),
        })
    }

    pub async fn change_index(
        &self,
        task: &str,
        expected: &GitComparison,
        path: &str,
        action: IndexAction,
        store: &ReviewStore,
    ) -> ReviewResult<()> {
        self.change_index_many(task, expected, &[path.to_owned()], action, store)
            .await
    }

    /// Publish all selected files in one index transaction. No partial index is visible.
    pub async fn change_index_many(
        &self,
        task: &str,
        expected: &GitComparison,
        paths: &[String],
        action: IndexAction,
        store: &ReviewStore,
    ) -> ReviewResult<()> {
        if paths.is_empty() {
            return Err("no files selected".into());
        }
        match (action, &expected.mode) {
            (IndexAction::Stage, CompareMode::Uncommitted | CompareMode::Unstaged)
            | (IndexAction::Unstage, CompareMode::Staged) => {}
            _ => return Err("operation does not apply to this comparison mode".into()),
        }
        let mut changes = Vec::new();
        for path in paths.iter().collect::<std::collections::BTreeSet<_>>() {
            validate_relative(path)?;
            if !Self::selected(&expected.root, path) {
                return Err("file is outside selected project folders".into());
            }
            let change = expected
                .changes
                .iter()
                .find(|f| &f.path == path)
                .ok_or("file is not in this snapshot")?;
            if let Some(previous) = &change.previous_path {
                validate_relative(previous)?;
                if previous == path || !Self::selected(&expected.root, previous) {
                    return Err("rename source is outside selected project folders".into());
                }
            }
            changes.push(change);
        }
        let expected_state = expected
            .repository_version
            .as_ref()
            .ok_or("snapshot is not mutable")?;
        let root = Path::new(&expected.root.path);
        let index_path = self.git_path(root, "index").await?;
        if &self.repository_version(root).await? != expected_state {
            return Err("repository changed since preview; refresh required".into());
        }
        let current = self
            .compare(&expected.root, expected.mode.clone(), None, 0, store)
            .await?;
        if current.version != expected.version
            || current.repository_version.as_ref() != Some(expected_state)
        {
            return Err("working files changed since preview; refresh required".into());
        }
        let temporary = tempfile::Builder::new()
            .prefix("review-index-")
            .tempdir_in(index_path.parent().ok_or("invalid index path")?)
            .map_err(|_| "cannot prepare index transaction")?;
        let candidate = temporary.path().join("index");
        let initial_index = read_index(&index_path)?;
        if initial_index.as_ref().map(|bytes| digest(bytes)) != expected_state.index {
            return Err("index changed before candidate preparation".into());
        }
        if let Some(bytes) = initial_index {
            fs::write(&candidate, bytes).map_err(|_| "cannot prepare candidate index")?;
        }
        let env = BTreeMap::from([("GIT_INDEX_FILE".into(), candidate.as_os_str().to_owned())]);
        let absent = None;
        let mut updates = Vec::new();
        for change in &changes {
            let path = change.path.as_str();
            updates.extend(match (action, change.previous_path.as_deref()) {
                (IndexAction::Stage, Some(previous)) => {
                    vec![(previous, &absent), (path, &change.after)]
                }
                (IndexAction::Unstage, Some(previous)) => {
                    vec![(path, &absent), (previous, &change.before)]
                }
                (IndexAction::Stage, None) => vec![(path, &change.after)],
                (IndexAction::Unstage, None) => vec![(path, &change.before)],
            });
        }
        // Rename chains may reuse paths: remove old entries before adding destinations.
        updates.sort_by_key(|(_, desired)| desired.is_some());
        for (path, desired) in updates {
            let args: Vec<String> = if let Some(version) =
                desired.as_ref().filter(|v| v.kind == FileKind::Gitlink)
            {
                let content = store.read_blob(&version.hash)?;
                let text =
                    std::str::from_utf8(&content).map_err(|_| "invalid submodule pointer")?;
                let oid = text
                    .strip_prefix("Subproject commit ")
                    .and_then(|s| s.strip_suffix('\n'))
                    .ok_or("invalid submodule pointer")?;
                let oid = oid.strip_suffix("-dirty").unwrap_or(oid);
                if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("invalid submodule commit identity".into());
                }
                vec![
                    "update-index".into(),
                    "--add".into(),
                    "--cacheinfo".into(),
                    "160000".into(),
                    oid.into(),
                    path.into(),
                ]
            } else if let Some(version) = desired {
                let content = store.read_blob(&version.hash)?;
                let blob_path = temporary.path().join("blob");
                fs::write(&blob_path, content).map_err(|_| "cannot prepare blob")?;
                let blob_path = blob_path.to_str().ok_or("non UTF-8 transaction path")?;
                // Stage the reviewed bytes using Git's path-based built-in text
                // conversion. The shared runner rejects external clean/process
                // filters. HEAD blobs and symlink targets must stay verbatim.
                let conversion =
                    if matches!(action, IndexAction::Stage) && version.kind == FileKind::File {
                        format!("--path={path}")
                    } else {
                        "--no-filters".into()
                    };
                let oid = self
                    .text(root, &["hash-object", "-w", &conversion, "--", blob_path])
                    .await?;
                let mode = if version.kind == FileKind::Symlink {
                    "120000"
                } else if version.executable {
                    "100755"
                } else {
                    "100644"
                };
                vec![
                    "update-index".into(),
                    "--add".into(),
                    "--cacheinfo".into(),
                    mode.into(),
                    oid.trim().into(),
                    path.into(),
                ]
            } else {
                vec![
                    "update-index".into(),
                    "--force-remove".into(),
                    "--".into(),
                    path.into(),
                ]
            };
            self.provider
                .git_bytes_with_options(
                    root,
                    &args.iter().map(String::as_str).collect::<Vec<_>>(),
                    env.clone(),
                    1024,
                )
                .await
                .map_err(|_| "candidate index update failed")?;
        }
        // Revalidate after all preparation; failures leave the original index intact.
        if &self.repository_version(root).await? != expected_state {
            return Err("repository changed during preparation".into());
        }
        let current = self
            .compare(&expected.root, expected.mode.clone(), None, 0, store)
            .await?;
        if current.version != expected.version
            || current.repository_version.as_ref() != Some(expected_state)
        {
            return Err("working files changed during preparation".into());
        }
        let bytes = read_index(&candidate)?.ok_or("candidate index missing")?;
        let original = read_index(&index_path)?;
        if original.as_ref().map(|bytes| digest(bytes)) != expected_state.index {
            return Err("index changed before recording publication".into());
        }
        let paths = changes
            .iter()
            .flat_map(|change| {
                change
                    .previous_path
                    .iter()
                    .cloned()
                    .chain(std::iter::once(change.path.clone()))
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let id = store.prepare_index_recovery(
            task,
            root,
            paths,
            matches!(action, IndexAction::Stage),
            crate::review::recovery::index::IndexRecoveryContent {
                original: original.as_deref(),
                candidate: &bytes,
                repository: expected_state.clone(),
            },
        )?;
        // The fixed recovery record exists before any Git-visible index lock.
        // Work done above only updates the private candidate. Revalidate again
        // under the real lock; a concurrent Git writer must not be overwritten.
        let lock = Lock::acquire_index(&index_path, task, &id, store)?;
        let current = self
            .compare(&expected.root, expected.mode.clone(), None, 0, store)
            .await?;
        if current.version != expected.version
            || current.repository_version.as_ref() != Some(expected_state)
            || &self.repository_version(root).await? != expected_state
        {
            return Err(format!(
                "repository changed before locked publication · 索引恢复记录 {id}"
            ));
        }
        lock.publish(&index_path, &bytes)
            .map_err(|error| format!("{error} · 索引恢复记录 {id}"))?;
        store.complete_index_recovery(task, &id)
    }
}
