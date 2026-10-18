//! Git sources for Review. Never use a session baseline for Git repositories.
mod discovery;
mod file_actions;
mod index_lock;
pub mod operations;
mod working_renames;
pub mod worktrees;
use super::{
    CompareMode, DirectorySnapshot, FileChange, FileKind, FileVersion, ReviewResult, ReviewStore,
    canonical_root, compare_snapshots, digest, validate_relative,
};
use crate::GitWorkspaceContextProvider;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

const MAX_FILES: usize = 2_000;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const QUERY_BUDGET: Duration = Duration::from_secs(30);

fn check_deadline(deadline: Instant) -> ReviewResult<()> {
    if Instant::now() >= deadline {
        Err("Git Review query time budget exceeded; no partial comparison returned".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRoot {
    pub id: String,
    pub path: String,
    pub git: bool,
    /// Repository-relative selected directories; empty string means all.
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummary {
    pub name: String,
    pub commit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitComparison {
    pub repository_version: Option<operations::RepositoryVersion>,
    pub root: ReviewRoot,
    pub mode: CompareMode,
    pub base: String,
    pub target: String,
    pub changes: Vec<FileChange>,
    pub untracked: Vec<String>,
    /// Digest of the frozen version pairs, independent of future disk changes.
    pub version: String,
}

#[derive(Default)]
pub struct GitReview {
    provider: GitWorkspaceContextProvider,
    /// Git object IDs are immutable; share blob reads across HEAD/index and
    /// revalidation passes. Never cache mutable working-tree metadata here.
    blobs: std::sync::Mutex<BTreeMap<(String, String), FileVersion>>,
}

impl GitReview {
    async fn run(&self, root: &Path, args: &[&str], limit: usize) -> ReviewResult<Vec<u8>> {
        self.provider
            .git_bytes_with_options(root, args, BTreeMap::new(), limit)
            .await
            .map_err(|error| format!("Git Review: {error:?}"))
    }

    async fn text(&self, root: &Path, args: &[&str]) -> ReviewResult<String> {
        String::from_utf8(self.run(root, args, MAX_BYTES).await?)
            .map_err(|_| "Git returned non UTF-8 metadata".into())
    }

    /// Called only with roots resolved from a persisted project by the host.
    pub async fn discover(&self, directories: &[String]) -> ReviewResult<Vec<ReviewRoot>> {
        let directories = directories.to_vec();
        let directories = tokio::task::spawn_blocking(move || discovery::expand(&directories))
            .await
            .map_err(|_| "repository discovery interrupted")??;
        let mut groups: BTreeMap<String, ReviewRoot> = BTreeMap::new();
        for directory in &directories {
            let selected = canonical_root(Path::new(directory))?;
            let (root, git) = if crate::is_non_git(&selected) {
                (selected.clone(), false)
            } else {
                let top = self
                    .text(&selected, &["rev-parse", "--show-toplevel"])
                    .await?;
                (canonical_root(Path::new(top.trim_end_matches('\n')))?, true)
            };
            let root_string = root.to_str().ok_or("non UTF-8 root")?.to_owned();
            let scope = selected
                .strip_prefix(&root)
                .map_err(|_| "directory outside repository")?
                .to_str()
                .ok_or("non UTF-8 scope")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            let group = groups
                .entry(root_string.clone())
                .or_insert_with(|| ReviewRoot {
                    id: digest(root_string.as_bytes()),
                    path: root_string,
                    git,
                    scopes: vec![],
                });
            if !group
                .scopes
                .iter()
                .any(|s| s.is_empty() || scope == *s || scope.starts_with(&format!("{s}/")))
            {
                group
                    .scopes
                    .retain(|s| !scope.is_empty() && !s.starts_with(&format!("{scope}/")));
                group.scopes.push(scope);
            }
        }
        Ok(groups.into_values().collect())
    }

    async fn resolve(&self, root: &Path, reference: &str) -> ReviewResult<String> {
        if reference.is_empty()
            || reference.starts_with('-')
            || reference.contains('\0')
            || reference.len() > 1024
        {
            return Err("invalid Git reference".into());
        }
        let object = self
            .text(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{reference}^{{commit}}"),
                ],
            )
            .await?;
        let object = object.trim();
        if !matches!(object.len(), 40 | 64) || !object.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid commit object".into());
        }
        Ok(object.into())
    }

    async fn head(&self, root: &Path) -> ReviewResult<Option<String>> {
        let output = self
            .provider
            .git_process(
                root,
                &["rev-parse", "--verify", "--quiet", "HEAD"],
                BTreeMap::new(),
                256,
            )
            .await
            .map_err(|e| format!("Git HEAD: {e:?}"))?;
        if output.success {
            return Ok(Some(
                String::from_utf8(output.stdout)
                    .map_err(|_| "invalid HEAD")?
                    .trim()
                    .into(),
            ));
        }
        // Only an unborn symbolic branch counts as empty; detached/broken HEAD
        // errors must not turn into an apparently empty comparison.
        self.text(root, &["symbolic-ref", "-q", "HEAD"]).await?;
        if output.exit_code == Some(1) {
            Ok(None)
        } else {
            Err("Git HEAD unavailable".into())
        }
    }

    pub async fn commits(
        &self,
        root: &ReviewRoot,
        skip: usize,
    ) -> ReviewResult<Vec<CommitSummary>> {
        if !root.git {
            return Err("not a Git repository".into());
        }
        let path = Path::new(&root.path);
        if self.head(path).await?.is_none() {
            return Ok(vec![]);
        }
        let bytes = self
            .run(
                path,
                &[
                    "log",
                    "-z",
                    "--max-count=50",
                    &format!("--skip={skip}"),
                    "--format=%H%x00%P%x00%an%x00%aI%x00%s",
                    "HEAD",
                    "--",
                ],
                MAX_BYTES,
            )
            .await?;
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        let fields = if fields.last() == Some(&&b""[..]) {
            &fields[..fields.len() - 1]
        } else {
            &fields[..]
        };
        if fields.len() % 5 != 0 {
            return Err("invalid Git log response".into());
        }
        fields
            .chunks_exact(5)
            .map(|row| {
                let values: Result<Vec<_>, _> =
                    row.iter().map(|v| std::str::from_utf8(v)).collect();
                let values = values.map_err(|_| "non UTF-8 commit metadata")?;
                Ok(CommitSummary {
                    id: values[0].into(),
                    parents: values[1].split_whitespace().map(str::to_owned).collect(),
                    author: values[2].into(),
                    date: values[3].into(),
                    subject: values[4].into(),
                })
            })
            .collect()
    }

    pub async fn branches(&self, root: &ReviewRoot) -> ReviewResult<Vec<BranchSummary>> {
        let output = self
            .text(
                Path::new(&root.path),
                &[
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)",
                    "refs/heads/",
                    "refs/remotes/",
                ],
            )
            .await?;
        output
            .lines()
            .map(|line| {
                let (name, commit) = line.split_once('\0').ok_or("invalid Git branch metadata")?;
                Ok(BranchSummary {
                    name: name.into(),
                    commit: commit.into(),
                })
            })
            .collect()
    }

    fn selected(root: &ReviewRoot, path: &str) -> bool {
        root.scopes.iter().any(|scope| {
            scope.is_empty() || path == scope || path.starts_with(&format!("{scope}/"))
        })
    }

    fn empty(root: &ReviewRoot) -> DirectorySnapshot {
        DirectorySnapshot {
            root: root.path.clone(),
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
            complete: true,
            diagnostics: vec![],
            excluded_entries: 0,
            excluded_roots: vec![],
        }
    }

    #[cfg(test)]
    async fn objects(
        &self,
        root: &ReviewRoot,
        revision: Option<&str>,
        index: bool,
        store: &ReviewStore,
        deadline: Instant,
    ) -> ReviewResult<DirectorySnapshot> {
        self.objects_filtered(root, revision, index, store, deadline, None)
            .await
    }

    async fn changed_paths(
        &self,
        root: &ReviewRoot,
        base: Option<&str>,
        target: &str,
    ) -> ReviewResult<std::collections::BTreeSet<String>> {
        // No rename coalescing here: both paths must remain in frozen pairs.
        // Disable external diff/textconv; object identity determines candidates.
        let args = if let Some(base) = base {
            vec![
                "diff",
                "--name-only",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "-z",
                base,
                target,
                "--",
            ]
        } else {
            vec![
                "diff-tree",
                "--root",
                "--no-commit-id",
                "-r",
                "--name-only",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "-z",
                target,
                "--",
            ]
        };
        let output = self.run(Path::new(&root.path), &args, MAX_BYTES).await?;
        Self::parse_changed_paths(root, &output)
    }

    fn parse_changed_paths(
        root: &ReviewRoot,
        output: &[u8],
    ) -> ReviewResult<std::collections::BTreeSet<String>> {
        let mut paths = std::collections::BTreeSet::new();
        for path in output.split(|b| *b == 0).filter(|path| !path.is_empty()) {
            let path = std::str::from_utf8(path).map_err(|_| "non UTF-8 Git path")?;
            validate_relative(path)?;
            if Self::selected(root, path) {
                paths.insert(path.to_owned());
            }
            if paths.len() > MAX_FILES {
                return Err("Git Review changed file budget exceeded".into());
            }
        }
        Ok(paths)
    }

    async fn working_candidates(
        &self,
        root: &ReviewRoot,
        head: Option<&str>,
        mode: &CompareMode,
    ) -> ReviewResult<std::collections::BTreeSet<String>> {
        let mut args = vec![
            "diff",
            "--ignore-submodules=none",
            "--numstat",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
        ];
        if *mode == CompareMode::Uncommitted
            && let Some(head) = head
        {
            args.push(head);
        }
        args.push("--");
        let path = Path::new(&root.path);
        // name-only can report stat-dirty but content-clean entries. Numstat
        // forces Git's content conversion without refreshing the real index.
        let output = self.run(path, &args, MAX_BYTES).await?;
        let mut names = Vec::new();
        for record in output.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
            let name = record
                .splitn(3, |byte| *byte == b'\t')
                .nth(2)
                .filter(|name| !name.is_empty())
                .ok_or("invalid Git numstat record")?;
            names.extend_from_slice(name);
            names.push(0);
        }
        let mut paths = Self::parse_changed_paths(root, &names)?;
        if *mode == CompareMode::Uncommitted && head.is_none() {
            paths.extend(Self::parse_changed_paths(
                root,
                &self
                    .run(
                        path,
                        &[
                            "diff",
                            "--cached",
                            "--name-only",
                            "--no-renames",
                            "--no-ext-diff",
                            "--no-textconv",
                            "-z",
                            "--",
                        ],
                        MAX_BYTES,
                    )
                    .await?,
            )?);
        }
        paths.extend(
            Self::parse_changed_paths(
                root,
                &self
                    .run(
                        path,
                        &["ls-files", "--others", "--exclude-standard", "-z"],
                        MAX_BYTES,
                    )
                    .await?,
            )?
            .into_iter()
            .filter(|path| {
                !path
                    .split('/')
                    .any(|part| part.starts_with(".agent-review-"))
            }),
        );
        if paths.len() > MAX_FILES {
            return Err("Git Review changed file budget exceeded".into());
        }
        Ok(paths)
    }

    async fn objects_filtered(
        &self,
        root: &ReviewRoot,
        revision: Option<&str>,
        index: bool,
        store: &ReviewStore,
        deadline: Instant,
        paths: Option<&std::collections::BTreeSet<String>>,
    ) -> ReviewResult<DirectorySnapshot> {
        let mut snapshot = Self::empty(root);
        if paths.is_some_and(std::collections::BTreeSet::is_empty) {
            return Ok(snapshot);
        }
        let path = Path::new(&root.path);
        let bytes = if index {
            self.run(path, &["ls-files", "--stage", "-z"], MAX_BYTES)
                .await?
        } else if let Some(revision) = revision {
            self.run(
                path,
                &["ls-tree", "-r", "-z", "--full-tree", revision],
                MAX_BYTES,
            )
            .await?
        } else {
            return Ok(snapshot);
        };
        let mut entries = Vec::new();
        for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            check_deadline(deadline)?;
            let record = std::str::from_utf8(record).map_err(|_| "non UTF-8 Git path")?;
            let (header, relative) = record.split_once('\t').ok_or("invalid Git entry")?;
            if !Self::selected(root, relative) {
                continue;
            }
            if paths.is_some_and(|paths| !paths.contains(relative)) {
                continue;
            }
            validate_relative(relative)?;
            if entries.len() + snapshot.files.len() >= MAX_FILES {
                return Err("Git Review file budget exceeded".into());
            }
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 {
                return Err("invalid Git entry metadata".into());
            }
            if index && fields[2] != "0" {
                return Err("repository has unresolved merge conflicts".into());
            }
            let oid = fields[if index { 1 } else { 2 }];
            let mode = fields[0];
            if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("invalid Git blob ID".into());
            }
            if mode == "160000" {
                let mut pointer = version(
                    store,
                    format!("Subproject commit {oid}\n").as_bytes(),
                    false,
                    false,
                )?;
                pointer.kind = FileKind::Gitlink;
                snapshot.files.insert(relative.into(), pointer);
                continue;
            }
            entries.push((relative, mode, oid));
        }
        let missing: Vec<String> = {
            let cache = self
                .blobs
                .lock()
                .map_err(|_| "Git blob cache unavailable")?;
            entries
                .iter()
                .filter_map(|(_, _, oid)| {
                    (!cache.contains_key(&(root.path.clone(), (*oid).to_owned())))
                        .then_some((*oid).to_owned())
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        // Small batches bound peak retained output, while avoiding one process
        // (and configuration probe) for every immutable object.
        let mut fetched_bytes = 0;
        for batch in missing.chunks(32) {
            check_deadline(deadline)?;
            let input = format!("{}\n", batch.join("\n"));
            let output = self
                .provider
                .git_process_input(
                    path,
                    &["cat-file", "--batch"],
                    BTreeMap::new(),
                    batch.len() * (MAX_BYTES + 128),
                    Some(input.as_bytes()),
                )
                .await
                .map_err(|error| format!("Git Review batch: {error:?}"))?;
            if !output.success {
                return Err("Git blob batch failed".into());
            }
            let content = parse_blob_batch(&output.stdout, batch)?;
            fetched_bytes += content.iter().map(|bytes| bytes.len()).sum::<usize>();
            if fetched_bytes > 128 * 1024 * 1024 {
                return Err("Git Review content budget exceeded".into());
            }
            let mut pending = BTreeMap::new();
            let mut fetched = Vec::new();
            for (oid, bytes) in batch.iter().zip(content) {
                check_deadline(deadline)?;
                let file = FileVersion {
                    hash: digest(bytes),
                    bytes: bytes.len() as u64,
                    kind: FileKind::File,
                    executable: false,
                    binary: bytes.contains(&0) || std::str::from_utf8(bytes).is_err(),
                };
                pending
                    .entry(file.hash.clone())
                    .or_insert_with(|| bytes.to_vec());
                fetched.push(((root.path.clone(), oid.clone()), file));
            }
            // Publish no cache entry until every blob and the directory barrier
            // have completed. Failure keeps the quota reservation invalid.
            store.flush_scan_blobs(&mut pending)?;
            check_deadline(deadline)?;
            self.blobs
                .lock()
                .map_err(|_| "Git blob cache unavailable")?
                .extend(fetched);
        }
        let mut total = 0;
        for (relative, mode, oid) in entries {
            check_deadline(deadline)?;
            let key = (root.path.clone(), oid.to_owned());
            let cached = self
                .blobs
                .lock()
                .map_err(|_| "Git blob cache unavailable")?
                .get(&key)
                .cloned();
            let mut file = cached.ok_or("Git blob batch omitted an object")?;
            // CAS corruption is still an error, not a successful cache hit.
            store.read_blob(&file.hash)?;
            file.kind = if mode == "120000" {
                FileKind::Symlink
            } else {
                FileKind::File
            };
            file.executable = mode == "100755";
            total += usize::try_from(file.bytes).map_err(|_| "Git file size overflow")?;
            if total > 128 * 1024 * 1024 {
                return Err("Git Review content budget exceeded".into());
            }
            snapshot.files.insert(relative.into(), file);
        }
        Ok(snapshot)
    }

    async fn rename_pairs(
        &self,
        root: &ReviewRoot,
        mode: &CompareMode,
        base: &str,
        target: &str,
    ) -> ReviewResult<Vec<(String, String)>> {
        let mut args = if *mode == CompareMode::Staged {
            vec!["diff", "--cached"]
        } else if base.is_empty() {
            vec!["diff-tree", "--root", "--no-commit-id", "-r"]
        } else {
            vec!["diff"]
        };
        args.extend([
            "--name-status",
            "-z",
            "--find-renames=50%",
            "-l2000",
            "--no-ext-diff",
            "--no-textconv",
        ]);
        if !base.is_empty() {
            args.push(base);
        }
        if *mode != CompareMode::Staged {
            args.push(target);
        }
        args.push("--");
        let output = self.run(Path::new(&root.path), &args, MAX_BYTES).await?;
        let mut fields = output.split(|b| *b == 0).filter(|field| !field.is_empty());
        let mut pairs = Vec::new();
        while let Some(status) = fields.next() {
            let path = std::str::from_utf8(fields.next().ok_or("missing Git status path")?)
                .map_err(|_| "non UTF-8 Git path")?;
            validate_relative(path)?;
            if status.starts_with(b"R") || status.starts_with(b"C") {
                let destination =
                    std::str::from_utf8(fields.next().ok_or("missing Git rename destination")?)
                        .map_err(|_| "non UTF-8 Git path")?;
                validate_relative(destination)?;
                if status.starts_with(b"R")
                    && Self::selected(root, path)
                    && Self::selected(root, destination)
                {
                    pairs.push((path.to_owned(), destination.to_owned()));
                }
            }
        }
        Ok(pairs)
    }

    async fn working_gitlink(
        &self,
        directory: &Path,
        original: &FileVersion,
        store: &ReviewStore,
    ) -> ReviewResult<FileVersion> {
        match fs::symlink_metadata(directory.join(".git")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(original.clone());
            }
            Ok(marker) if marker.is_file() || marker.is_dir() => {}
            _ => return Err("submodule repository marker unavailable".into()),
        }
        let top = self
            .text(directory, &["rev-parse", "--show-toplevel"])
            .await?;
        if canonical_root(Path::new(top.trim_end_matches('\n')))? != canonical_root(directory)? {
            return Err("submodule repository boundary changed".into());
        }
        let head = self
            .head(directory)
            .await?
            .ok_or("submodule has no checked out commit")?;
        let status = self
            .run(
                directory,
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=normal",
                    "--ignore-submodules=none",
                ],
                MAX_BYTES,
            )
            .await?;
        if self.head(directory).await?.as_ref() != Some(&head) {
            return Err("submodule HEAD changed during Review".into());
        }
        let text = format!(
            "Subproject commit {head}{}\n",
            if status.is_empty() { "" } else { "-dirty" }
        );
        let mut pointer = version(store, text.as_bytes(), false, false)?;
        pointer.kind = FileKind::Gitlink;
        Ok(pointer)
    }

    async fn working(
        &self,
        root: &ReviewRoot,
        index: &DirectorySnapshot,
        store: &ReviewStore,
        deadline: Instant,
        candidates: &std::collections::BTreeSet<String>,
    ) -> ReviewResult<(DirectorySnapshot, Vec<String>)> {
        let mut snapshot = Self::empty(root);
        let path = Path::new(&root.path);
        let others = self
            .run(
                path,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                MAX_BYTES,
            )
            .await?;
        let mut untracked = Vec::new();
        for record in others.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let relative = std::str::from_utf8(record).map_err(|_| "non UTF-8 Git path")?;
            if relative.split('/').any(|p| p.starts_with(".agent-review-")) {
                continue;
            }
            if Self::selected(root, relative) {
                validate_relative(relative)?;
                untracked.push(relative.to_owned());
            }
        }
        let mut files: Vec<_> = index
            .files
            .keys()
            .cloned()
            .chain(untracked.iter().cloned())
            .chain(candidates.iter().cloned())
            .collect();
        files.sort();
        files.dedup();
        if files.len() > MAX_FILES {
            return Err("Git Review file budget exceeded".into());
        }
        let mut total = 0;
        let mut pending = BTreeMap::new();
        let mut pending_bytes = 0;
        for relative in files {
            check_deadline(deadline)?;
            let file = path.join(&relative);
            let metadata = match fs::symlink_metadata(&file) {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("working file unavailable".into()),
            };
            // Every ancestor must remain inside this repository. Never follow
            // a parent symlink to a different project during Review reads.
            if !file
                .parent()
                .ok_or("invalid file parent")?
                .canonicalize()
                .map_err(|_| "file parent unavailable")?
                .starts_with(path)
            {
                return Err("working file escaped repository".into());
            }
            if metadata.len() > MAX_BYTES as u64 {
                return Err("Git Review file content budget exceeded".into());
            }
            if metadata.is_dir() {
                if let Some(original) = index
                    .files
                    .get(&relative)
                    .filter(|v| v.kind == FileKind::Gitlink)
                {
                    let pointer = self.working_gitlink(&file, original, store).await?;
                    snapshot.files.insert(relative, pointer);
                    continue;
                }
                // Untracked nested repositories have their own discovered group.
                if fs::symlink_metadata(file.join(".git")).is_ok() {
                    continue;
                }
            }
            let bytes = if metadata.file_type().is_symlink() {
                fs::read_link(&file)
                    .map_err(|_| "symlink unavailable")?
                    .to_str()
                    .ok_or("non UTF-8 symlink")?
                    .as_bytes()
                    .to_vec()
            } else if metadata.is_file() {
                use std::io::Read;
                let handle = fs::File::open(&file).map_err(|_| "file unavailable")?;
                if !super::same_file(
                    &metadata,
                    &handle.metadata().map_err(|_| "file metadata unavailable")?,
                ) {
                    return Err("file changed during Review".into());
                }
                let mut bytes = Vec::new();
                handle
                    .take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "file read failed")?;
                bytes
            } else {
                return Err("unsupported working file type".into());
            };
            if !super::same_file(
                &metadata,
                &fs::symlink_metadata(&file).map_err(|_| "file changed during Review")?,
            ) {
                return Err("file changed during Review".into());
            }
            total += bytes.len();
            if bytes.len() > MAX_BYTES || total > 128 * 1024 * 1024 {
                return Err("Git Review content budget exceeded".into());
            }
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = index.files.get(&relative).is_some_and(|v| v.executable);
            snapshot.files.insert(
                relative,
                FileVersion {
                    hash: digest(&bytes),
                    bytes: bytes.len() as u64,
                    kind: if metadata.file_type().is_symlink() {
                        FileKind::Symlink
                    } else {
                        FileKind::File
                    },
                    executable,
                    binary: bytes.contains(&0) || std::str::from_utf8(&bytes).is_err(),
                },
            );
            if pending.len() >= 64 || pending_bytes + bytes.len() > 16 * 1024 * 1024 {
                store.flush_scan_blobs(&mut pending)?;
                check_deadline(deadline)?;
                pending_bytes = 0;
            }
            let hash = digest(&bytes);
            if let std::collections::btree_map::Entry::Vacant(entry) = pending.entry(hash) {
                pending_bytes += bytes.len();
                entry.insert(bytes);
            }
        }
        store.flush_scan_blobs(&mut pending)?;
        check_deadline(deadline)?;
        Ok((snapshot, untracked))
    }

    pub async fn compare(
        &self,
        root: &ReviewRoot,
        mode: CompareMode,
        reference: Option<&str>,
        parent: usize,
        store: &ReviewStore,
    ) -> ReviewResult<GitComparison> {
        self.compare_with_budget(root, mode, reference, parent, store, QUERY_BUDGET)
            .await
    }

    async fn compare_with_budget(
        &self,
        root: &ReviewRoot,
        mode: CompareMode,
        reference: Option<&str>,
        parent: usize,
        store: &ReviewStore,
        budget: Duration,
    ) -> ReviewResult<GitComparison> {
        let deadline = Instant::now() + budget;
        check_deadline(deadline)?;
        tokio::time::timeout(
            budget,
            self.compare_inner(root, mode, reference, parent, store, deadline),
        )
        .await
        .map_err(|_| {
            "Git Review query time budget exceeded; no partial comparison returned".to_owned()
        })?
    }

    async fn compare_inner(
        &self,
        root: &ReviewRoot,
        mode: CompareMode,
        reference: Option<&str>,
        parent: usize,
        store: &ReviewStore,
        deadline: Instant,
    ) -> ReviewResult<GitComparison> {
        if !root.git {
            return Err("Git comparison requires a repository".into());
        }
        let path = Path::new(&root.path);
        let head = self.head(path).await?;
        let mutable = matches!(
            mode,
            CompareMode::Uncommitted | CompareMode::Unstaged | CompareMode::Staged
        );
        let repository_version = if mutable {
            Some(self.repository_version(path).await?)
        } else {
            None
        };
        if repository_version.as_ref().is_some_and(|v| v.head != head) {
            return Err("HEAD changed before comparison".into());
        }
        let (before, after, base, target, untracked) = match mode {
            CompareMode::Uncommitted | CompareMode::Unstaged | CompareMode::Staged => {
                let changed_paths = if mode == CompareMode::Staged {
                    let mut args = vec![
                        "diff",
                        "--cached",
                        "--name-only",
                        "--no-renames",
                        "--no-ext-diff",
                        "--no-textconv",
                        "-z",
                    ];
                    if let Some(head) = head.as_deref() {
                        args.push(head);
                    }
                    args.push("--");
                    Some(Self::parse_changed_paths(
                        root,
                        &self.run(path, &args, MAX_BYTES).await?,
                    )?)
                } else {
                    Some(
                        self.working_candidates(root, head.as_deref(), &mode)
                            .await?,
                    )
                };
                let index = self
                    .objects_filtered(root, None, true, store, deadline, changed_paths.as_ref())
                    .await?;
                // Unstaged compares only index and disk; HEAD blob contents are irrelevant.
                let head_snapshot = if mode == CompareMode::Unstaged {
                    Self::empty(root)
                } else {
                    self.objects_filtered(
                        root,
                        head.as_deref(),
                        false,
                        store,
                        deadline,
                        changed_paths.as_ref(),
                    )
                    .await?
                };
                if mode == CompareMode::Staged {
                    (
                        head_snapshot,
                        index,
                        head.clone().unwrap_or_default(),
                        "index".into(),
                        vec![],
                    )
                } else {
                    let candidates = changed_paths.as_ref().ok_or("missing working candidates")?;
                    let (working, untracked) = self
                        .working(root, &index, store, deadline, candidates)
                        .await?;
                    if self
                        .working_candidates(root, head.as_deref(), &mode)
                        .await?
                        != *candidates
                    {
                        return Err(
                            "working change set changed during Review; refresh required".into()
                        );
                    }
                    // A staged deletion may still have a working-tree file,
                    // which Git reports as untracked and working() includes.
                    let before = if mode == CompareMode::Unstaged {
                        index.clone()
                    } else {
                        head_snapshot
                    };
                    // Untracked entries retain immutable content for preview;
                    // consumers partition them using `untracked`, not as tracked
                    // unstaged changes or staged content.
                    // The final raw index/HEAD version check below detects mutation
                    // without rereading every immutable index blob a second time.
                    (
                        before,
                        working,
                        if mode == CompareMode::Unstaged {
                            "index".into()
                        } else {
                            head.clone().unwrap_or_default()
                        },
                        "working".into(),
                        untracked,
                    )
                }
            }
            CompareMode::Committed => {
                let commit = self
                    .resolve(path, reference.ok_or("select a commit")?)
                    .await?;
                let parents = self
                    .text(path, &["rev-list", "--parents", "-n", "1", &commit])
                    .await?;
                let parents: Vec<_> = parents.split_whitespace().skip(1).collect();
                let base = if parents.is_empty() {
                    None
                } else {
                    Some(
                        *parents
                            .get(parent)
                            .ok_or("invalid merge parent selection")?,
                    )
                };
                let paths = self.changed_paths(root, base, &commit).await?;
                (
                    self.objects_filtered(root, base, false, store, deadline, Some(&paths))
                        .await?,
                    self.objects_filtered(
                        root,
                        Some(&commit),
                        false,
                        store,
                        deadline,
                        Some(&paths),
                    )
                    .await?,
                    base.unwrap_or_default().into(),
                    commit,
                    vec![],
                )
            }
            CompareMode::Branch => {
                let target = head.ok_or("current branch has no commits")?;
                let against = self
                    .resolve(path, reference.ok_or("select a target branch")?)
                    .await?;
                let base = self
                    .text(path, &["merge-base", &target, &against])
                    .await?
                    .trim()
                    .to_owned();
                let paths = self.changed_paths(root, Some(&base), &target).await?;
                (
                    self.objects_filtered(root, Some(&base), false, store, deadline, Some(&paths))
                        .await?,
                    self.objects_filtered(
                        root,
                        Some(&target),
                        false,
                        store,
                        deadline,
                        Some(&paths),
                    )
                    .await?,
                    base,
                    target,
                    vec![],
                )
            }
            CompareMode::Session => {
                return Err("Git directories do not use session baselines".into());
            }
        };
        let mut changes = compare_snapshots(&before, &after)?;
        if matches!(
            mode,
            CompareMode::Staged | CompareMode::Committed | CompareMode::Branch
        ) {
            let pairs = self.rename_pairs(root, &mode, &base, &target).await?;
            for (previous, destination) in pairs {
                let deleted = changes
                    .iter()
                    .position(|c| c.path == previous && c.kind == super::ChangeKind::Deleted);
                let added = changes
                    .iter()
                    .position(|c| c.path == destination && c.kind == super::ChangeKind::Added);
                if let (Some(deleted), Some(added)) = (deleted, added) {
                    let original = changes[deleted].before.clone();
                    changes[added].before = original;
                    changes[added].previous_path = Some(previous);
                    changes[added].kind = super::ChangeKind::Renamed;
                    changes.remove(deleted);
                }
            }
        }
        if let Some(expected) = &repository_version
            && self.repository_version(path).await? != *expected
        {
            return Err("repository changed during comparison".into());
        }
        let mut changes = pair_exact_renames(changes);
        if matches!(mode, CompareMode::Uncommitted | CompareMode::Unstaged) {
            self.pair_working_renames(&mut changes, store).await?;
        }
        check_deadline(deadline)?;
        let version = digest(
            &serde_json::to_vec(&(&base, &target, &changes, &untracked))
                .map_err(|_| "comparison serialization failed")?,
        );
        Ok(GitComparison {
            repository_version,
            root: root.clone(),
            mode,
            base,
            target,
            changes,
            untracked,
            version,
        })
    }
}

fn pair_exact_renames(mut changes: Vec<FileChange>) -> Vec<FileChange> {
    use super::ChangeKind;
    let mut removed = std::collections::BTreeSet::new();
    for added in 0..changes.len() {
        if changes[added].kind != ChangeKind::Added {
            continue;
        }
        let Some(after) = changes[added].after.as_ref() else {
            continue;
        };
        let matches =
            |version: &FileVersion| version.hash == after.hash && version.kind == after.kind;
        // Do not guess which of several identical copies was renamed.
        if changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Added && c.after.as_ref().is_some_and(matches))
            .count()
            != 1
        {
            continue;
        }
        let candidates: Vec<_> = changes
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.kind == ChangeKind::Deleted && c.before.as_ref().is_some_and(matches)
            })
            .map(|(i, _)| i)
            .collect();
        if let [deleted] = candidates.as_slice() {
            changes[added].previous_path = Some(changes[*deleted].path.clone());
            let original = changes[*deleted].before.clone();
            changes[added].before = original;
            changes[added].kind = ChangeKind::Renamed;
            removed.insert(*deleted);
        }
    }
    changes
        .into_iter()
        .enumerate()
        .filter_map(|(i, change)| (!removed.contains(&i)).then_some(change))
        .collect()
}

fn parse_blob_batch<'a>(mut bytes: &'a [u8], requested: &[String]) -> ReviewResult<Vec<&'a [u8]>> {
    let mut contents = Vec::with_capacity(requested.len());
    for oid in requested {
        let end = bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or("missing Git batch header")?;
        let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "invalid Git batch header")?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3 || fields[0] != oid || fields[1] != "blob" {
            return Err("unexpected or missing Git batch object".into());
        }
        let size: usize = fields[2].parse().map_err(|_| "invalid Git batch size")?;
        if size > MAX_BYTES {
            return Err("Git Review file content budget exceeded".into());
        }
        bytes = &bytes[end + 1..];
        if bytes.get(size) != Some(&b'\n') {
            return Err("truncated Git batch object".into());
        }
        contents.push(&bytes[..size]);
        bytes = &bytes[size + 1..];
    }
    if !bytes.is_empty() {
        return Err("unexpected trailing Git batch data".into());
    }
    Ok(contents)
}

fn version(
    store: &ReviewStore,
    bytes: &[u8],
    symlink: bool,
    executable: bool,
) -> ReviewResult<FileVersion> {
    Ok(FileVersion {
        hash: store.put_blob(bytes)?,
        bytes: bytes.len() as u64,
        kind: if symlink {
            FileKind::Symlink
        } else {
            FileKind::File
        },
        executable,
        binary: bytes.contains(&0) || std::str::from_utf8(bytes).is_err(),
    })
}

#[cfg(test)]
#[path = "git/performance_tests.rs"]
mod performance_tests;

#[cfg(test)]
mod tests {
    mod batch_discard_tests;
    mod batch_index_tests;
    use super::*;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) -> String {
        let result = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().into()
    }

    #[test]
    fn batch_parser_is_binary_safe_and_rejects_incomplete_or_unexpected_objects() {
        let requested = vec!["a".repeat(40), "b".repeat(40)];
        let mut bytes = format!("{} blob 3\n", requested[0]).into_bytes();
        bytes.extend_from_slice(&[0, 255, b'\n', b'\n']);
        bytes.extend_from_slice(format!("{} blob 0\n\n", requested[1]).as_bytes());
        assert_eq!(
            parse_blob_batch(&bytes, &requested).unwrap(),
            vec![&[0, 255, b'\n'][..], &[]]
        );
        assert!(parse_blob_batch(&bytes[..bytes.len() - 1], &requested).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(parse_blob_batch(&trailing, &requested).is_err());
        let mut wrong = requested.clone();
        wrong.reverse();
        assert!(parse_blob_batch(&bytes, &wrong).is_err());
        assert!(
            parse_blob_batch(
                format!("{} missing\n", requested[0]).as_bytes(),
                &requested[..1]
            )
            .is_err()
        );
        assert!(
            parse_blob_batch(
                format!("{} blob {}\n", requested[0], MAX_BYTES + 1).as_bytes(),
                &requested[..1]
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn git_similarity_pairs_edited_renames_in_staged_and_history() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let mut before = String::new();
        for i in 0..30 {
            use std::fmt::Write;
            writeln!(before, "original line number {i}").unwrap();
        }
        let after = before.replace("number 15", "number changed");
        fs::write(repo.path().join("old.txt"), &before).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["branch", "base"]);
        fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        fs::write(repo.path().join("new.txt"), &after).unwrap();
        git(repo.path(), &["add", "."]);
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        for (mode, reference) in [
            (CompareMode::Staged, None),
            (CompareMode::Committed, Some("HEAD")),
            (CompareMode::Branch, Some("base")),
        ] {
            if mode == CompareMode::Committed {
                git(repo.path(), &["commit", "-m", "rename and edit"]);
            }
            let comparison = service
                .compare(&root, mode, reference, 0, &store)
                .await
                .unwrap();
            assert_eq!(comparison.changes.len(), 1);
            let change = &comparison.changes[0];
            assert_eq!(change.previous_path.as_deref(), Some("old.txt"));
            assert_eq!(change.path, "new.txt");
            assert_eq!(
                store
                    .read_blob(&change.before.as_ref().unwrap().hash)
                    .unwrap(),
                before.as_bytes()
            );
            assert_eq!(
                store
                    .read_blob(&change.after.as_ref().unwrap().hash)
                    .unwrap(),
                after.as_bytes()
            );
        }
    }

    #[tokio::test]
    async fn exact_rename_keeps_both_paths_and_ambiguous_copies_stay_separate() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join("old"), "same content").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "base"]);
        fs::rename(repo.path().join("old"), repo.path().join("new")).unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
            let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
            assert_eq!(comparison.changes.len(), 1);
            assert_eq!(
                comparison.changes[0].kind,
                super::super::ChangeKind::Renamed
            );
            assert_eq!(comparison.changes[0].previous_path.as_deref(), Some("old"));
            assert_eq!(comparison.changes[0].path, "new");
        }
        let working = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index(
                "task",
                &working,
                "new",
                operations::IndexAction::Stage,
                &store,
            )
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["ls-files"]), "new");
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(staged.changes[0].previous_path.as_deref(), Some("old"));
        service
            .change_index(
                "task",
                &staged,
                "new",
                operations::IndexAction::Unstage,
                &store,
            )
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["ls-files"]), "old");
        assert!(!repo.path().join("old").exists());
        assert_eq!(fs::read(repo.path().join("new")).unwrap(), b"same content");
        fs::write(repo.path().join("another"), "same content").unwrap();
        let comparison = service
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(comparison.changes.len(), 3);
        assert!(
            comparison
                .changes
                .iter()
                .all(|change| change.previous_path.is_none())
        );
    }

    #[tokio::test]
    async fn working_gitlink_reports_checked_out_commit_and_dirty_state() {
        let outer = repository();
        let child = outer.path().join("child");
        fs::create_dir(&child).unwrap();
        git(&child, &["init", "-b", "main"]);
        git(&child, &["config", "user.name", "Review Test"]);
        git(&child, &["config", "user.email", "review@example.invalid"]);
        fs::write(child.join("file"), "initial").unwrap();
        git(&child, &["add", "."]);
        git(&child, &["commit", "-m", "child base"]);
        let old = git(&child, &["rev-parse", "HEAD"]);
        git(
            outer.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &old,
                "child",
            ],
        );
        git(outer.path(), &["commit", "-m", "parent base"]);
        fs::write(child.join("file"), "next").unwrap();
        git(&child, &["add", "."]);
        git(&child, &["commit", "-m", "child next"]);
        let new = git(&child, &["rev-parse", "HEAD"]);
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let root = service
            .discover(&[outer.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .into_iter()
            .find(|root| root.path == outer.path().canonicalize().unwrap().to_str().unwrap())
            .unwrap();
        for dirty in [false, true] {
            if dirty {
                fs::write(child.join("file"), "uncommitted work").unwrap();
            }
            for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
                let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
                assert_eq!(comparison.changes.len(), 1);
                let pointer = comparison.changes[0].after.as_ref().unwrap();
                assert_eq!(pointer.kind, FileKind::Gitlink);
                assert_eq!(
                    store.read_blob(&pointer.hash).unwrap(),
                    format!(
                        "Subproject commit {new}{}\n",
                        if dirty { "-dirty" } else { "" }
                    )
                    .as_bytes()
                );
            }
        }
    }

    #[tokio::test]
    async fn merge_commit_comparison_uses_the_selected_real_parent() {
        let repo = repository();
        fs::write(repo.path().join("base"), "base\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "root"]);
        let branch = git(repo.path(), &["branch", "--show-current"]);
        git(repo.path(), &["checkout", "-b", "side"]);
        fs::write(repo.path().join("side-file"), "side\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "side change"]);
        let side = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", &branch]);
        fs::write(repo.path().join("main-file"), "main\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "main change"]);
        let main = git(repo.path(), &["rev-parse", "HEAD"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "side", "-m", "merge both"],
        );
        let head = git(repo.path(), &["rev-parse", "HEAD"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let commits = service.commits(&root, 0).await.unwrap();
        assert_eq!(commits[0].parents, [main.clone(), side.clone()]);
        for (parent, expected_base, expected_path) in
            [(0, &main, "side-file"), (1, &side, "main-file")]
        {
            let comparison = service
                .compare(&root, CompareMode::Committed, Some(&head), parent, &store)
                .await
                .unwrap();
            assert_eq!(&comparison.base, expected_base);
            assert_eq!(comparison.target, head);
            assert_eq!(comparison.changes.len(), 1);
            assert_eq!(comparison.changes[0].path, expected_path);
        }
        assert!(
            service
                .compare(&root, CompareMode::Committed, Some(&head), 2, &store)
                .await
                .is_err()
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    }

    #[tokio::test]
    async fn submodule_missing_uninitialized_and_replaced_file_boundaries() {
        let repo = repository();
        let oid = "a".repeat(40);
        git(
            repo.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &oid,
                "child",
            ],
        );
        fs::write(repo.path().join("ordinary.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "ordinary.txt"]);
        git(repo.path(), &["commit", "-m", "base"]);
        fs::write(repo.path().join("ordinary.txt"), "other work\n").unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        // Missing directory is a deletion, not a read error for the whole repo.
        let missing = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        assert!(
            missing
                .changes
                .iter()
                .any(|c| c.path == "child" && c.kind == crate::review::ChangeKind::Deleted)
        );
        assert!(missing.changes.iter().any(|c| c.path == "ordinary.txt"));
        service
            .change_index(
                "task",
                &missing,
                "child",
                operations::IndexAction::Stage,
                &store,
            )
            .await
            .unwrap();
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index(
                "task",
                &staged,
                "child",
                operations::IndexAction::Unstage,
                &store,
            )
            .await
            .unwrap();
        assert!(!repo.path().join("child").exists());
        assert!(git(repo.path(), &["ls-files", "--stage", "child"]).contains("160000"));
        // Empty uninitialized submodule retains its recorded pointer.
        fs::create_dir(repo.path().join("child")).unwrap();
        let empty = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        assert!(!empty.changes.iter().any(|c| c.path == "child"));
        fs::remove_dir(repo.path().join("child")).unwrap();
        // A regular file replacing a gitlink is a visible type change; index
        // operations must never remove or overwrite those working bytes.
        fs::write(repo.path().join("child"), "replacement file\n").unwrap();
        let replacement = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        let change = replacement
            .changes
            .iter()
            .find(|c| c.path == "child")
            .unwrap();
        assert_eq!(change.before.as_ref().unwrap().kind, FileKind::Gitlink);
        assert_eq!(change.after.as_ref().unwrap().kind, FileKind::File);
        service
            .change_index(
                "task",
                &replacement,
                "child",
                operations::IndexAction::Stage,
                &store,
            )
            .await
            .unwrap();
        assert!(git(repo.path(), &["ls-files", "--stage", "child"]).contains("100644"));
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index(
                "task",
                &staged,
                "child",
                operations::IndexAction::Unstage,
                &store,
            )
            .await
            .unwrap();
        assert!(git(repo.path(), &["ls-files", "--stage", "child"]).contains("160000"));
        assert_eq!(
            fs::read_to_string(repo.path().join("child")).unwrap(),
            "replacement file\n"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("ordinary.txt")).unwrap(),
            "other work\n"
        );
    }

    #[tokio::test]
    async fn submodule_index_updates_preserve_dirty_child_worktree() {
        let outer = repository();
        let child = outer.path().join("child");
        fs::create_dir(&child).unwrap();
        git(&child, &["init", "-b", "main"]);
        git(&child, &["config", "user.name", "Review Test"]);
        git(&child, &["config", "user.email", "review@example.invalid"]);
        fs::write(child.join("file"), "base").unwrap();
        fs::write(child.join(".gitattributes"), "eol.txt text eol=crlf\n").unwrap();
        fs::write(child.join("eol.txt"), "line\n").unwrap();
        git(&child, &["add", "."]);
        git(&child, &["commit", "-m", "base"]);
        let old = git(&child, &["rev-parse", "HEAD"]);
        git(
            outer.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &old,
                "child",
            ],
        );
        git(outer.path(), &["commit", "-m", "base pointer"]);
        fs::write(child.join("file"), "committed").unwrap();
        fs::write(child.join(".gitattributes"), "eol.txt text eol=lf\n").unwrap();
        git(&child, &["commit", "-am", "next"]);
        let new = git(&child, &["rev-parse", "HEAD"]);
        fs::write(child.join("file"), "keep my uncommitted work").unwrap();
        fs::write(child.join(".env"), "LOCAL_FIXTURE_ONLY=keep\n").unwrap();
        fs::create_dir(child.join("build")).unwrap();
        fs::write(child.join("build/output"), b"ignored fixture bytes").unwrap();
        let child_index = fs::read(child.join(".git/index")).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let root = service
            .discover(&[outer.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.path == outer.path().canonicalize().unwrap().to_str().unwrap())
            .unwrap();
        let preview = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index(
                "task",
                &preview,
                "child",
                operations::IndexAction::Stage,
                &store,
            )
            .await
            .unwrap();
        assert!(
            git(outer.path(), &["ls-files", "--stage", "child"]).contains(&format!("160000 {new}"))
        );
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index(
                "task",
                &staged,
                "child",
                operations::IndexAction::Unstage,
                &store,
            )
            .await
            .unwrap();
        assert!(
            git(outer.path(), &["ls-files", "--stage", "child"]).contains(&format!("160000 {old}"))
        );
        assert_eq!(
            fs::read_to_string(child.join("file")).unwrap(),
            "keep my uncommitted work"
        );
        assert_eq!(git(&child, &["rev-parse", "HEAD"]), new);
        assert_eq!(fs::read(child.join(".git/index")).unwrap(), child_index);
    }

    #[tokio::test]
    async fn frozen_gitlink_pointers_are_readable_but_not_regular_file_mutations() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let old = "a".repeat(40);
        let new = "b".repeat(40);
        git(
            repo.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &old,
                "child",
            ],
        );
        git(repo.path(), &["commit", "-m", "old pointer"]);
        git(
            repo.path(),
            &["update-index", "--cacheinfo", "160000", &new, "child"],
        );
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(staged.changes.len(), 1);
        let change = &staged.changes[0];
        assert_eq!(change.after.as_ref().unwrap().kind, FileKind::Gitlink);
        assert_eq!(
            store
                .read_blob(&change.before.as_ref().unwrap().hash)
                .unwrap(),
            format!("Subproject commit {old}\n").as_bytes()
        );
        assert!(
            store
                .prepare_recovery(
                    "task",
                    repo.path(),
                    "child",
                    change.after.clone(),
                    change.before.clone()
                )
                .is_err()
        );
        git(repo.path(), &["commit", "-m", "new pointer"]);
        let committed = service
            .compare(&root, CompareMode::Committed, Some("HEAD"), 0, &store)
            .await
            .unwrap();
        assert_eq!(committed.changes[0].after, change.after);
    }

    #[tokio::test]
    async fn discovers_real_nested_repositories_without_duplicate_groups() {
        let outer = repository();
        let nested = outer.path().join("packages/inner");
        fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-b", "main"]);
        let roots = GitReview::default()
            .discover(&[
                outer.path().to_str().unwrap().into(),
                nested.to_str().unwrap().into(),
            ])
            .await
            .unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|root| root.git && root.scopes == [""]));
        assert_ne!(roots[0].id, roots[1].id);
    }

    #[tokio::test]
    async fn all_modes_read_only_changed_blobs_in_large_trees() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        for i in 0..=MAX_FILES {
            fs::write(repo.path().join(format!("unchanged-{i}")), "unchanged").unwrap();
        }
        fs::write(
            repo.path().join("large-unchanged"),
            vec![b'x'; MAX_BYTES + 1],
        )
        .unwrap();
        fs::write(repo.path().join("changed\nfile"), "before\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "large baseline"]);
        git(repo.path(), &["branch", "baseline"]);
        fs::write(repo.path().join("changed\nfile"), "after\n").unwrap();
        git(repo.path(), &["add", "."]);
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(staged.changes.len(), 1);
        assert_eq!(staged.changes[0].path, "changed\nfile");
        assert_eq!(service.blobs.lock().unwrap().len(), 2);
        service
            .change_index(
                "task",
                &staged,
                "changed\nfile",
                operations::IndexAction::Unstage,
                &store,
            )
            .await
            .unwrap();
        assert!(
            service
                .compare(&root, CompareMode::Staged, None, 0, &store)
                .await
                .unwrap()
                .changes
                .is_empty()
        );
        assert_eq!(
            fs::read(repo.path().join("changed\nfile")).unwrap(),
            b"after\n"
        );
        for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
            let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
            assert_eq!(comparison.changes.len(), 1);
            assert_eq!(comparison.changes[0].path, "changed\nfile");
        }
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "one change"]);
        for (mode, reference) in [
            (CompareMode::Committed, "HEAD"),
            (CompareMode::Branch, "baseline"),
        ] {
            let comparison = service
                .compare(&root, mode, Some(reference), 0, &store)
                .await
                .unwrap();
            assert_eq!(comparison.changes.len(), 1);
            assert_eq!(comparison.changes[0].path, "changed\nfile");
            assert_eq!(
                store
                    .read_blob(&comparison.changes[0].before.as_ref().unwrap().hash)
                    .unwrap(),
                b"before\n"
            );
        }
        assert_eq!(service.blobs.lock().unwrap().len(), 2);
        let empty = service
            .compare(&root, CompareMode::Branch, Some("HEAD"), 0, &store)
            .await
            .unwrap();
        assert!(empty.changes.is_empty());
    }

    #[tokio::test]
    async fn reads_multiple_batches_with_binary_and_empty_blobs() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        for i in 0..19u8 {
            fs::write(repo.path().join(format!("file-{i}")), [0, i, 255, b'\n']).unwrap();
        }
        fs::write(repo.path().join("empty"), []).unwrap();
        git(repo.path(), &["add", "."]);
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let comparison = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(comparison.changes.len(), 20);
        assert_eq!(service.blobs.lock().unwrap().len(), 20);
        for change in comparison.changes {
            assert_eq!(
                store.read_blob(&change.after.unwrap().hash).unwrap(),
                fs::read(repo.path().join(change.path)).unwrap()
            );
        }
    }

    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-b", "main"]);
        git(root.path(), &["config", "user.name", "Review Test"]);
        git(
            root.path(),
            &["config", "user.email", "review@example.invalid"],
        );
        root
    }

    #[tokio::test]
    async fn review_excludes_ignored_files_and_respects_exceptions() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join("tracked.log"), "before\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join(".gitignore"), "*.log\nbuild/\n!keep.log\n").unwrap();
        fs::write(repo.path().join("tracked.log"), "after\n").unwrap();
        fs::write(repo.path().join("ignored.log"), "ignored\n").unwrap();
        fs::write(repo.path().join("keep.log"), "visible\n").unwrap();
        fs::create_dir(repo.path().join("build")).unwrap();
        fs::write(repo.path().join("build/output.js"), "ignored\n").unwrap();
        fs::write(repo.path().join("local.txt"), "ignored locally\n").unwrap();
        fs::write(repo.path().join(".git/info/exclude"), "local.txt\n").unwrap();
        let root = service
            .discover(&[repo.path().to_string_lossy().into_owned()])
            .await
            .unwrap()
            .remove(0);
        for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
            let result = service.compare(&root, mode, None, 0, &store).await.unwrap();
            let paths: std::collections::BTreeSet<_> = result
                .changes
                .iter()
                .map(|file| file.path.as_str())
                .collect();
            assert_eq!(
                paths,
                std::collections::BTreeSet::from([".gitignore", "keep.log", "tracked.log"])
            );
            assert!(!result.untracked.iter().any(|path| path == "ignored.log"
                || path.starts_with("build/")
                || path == "local.txt"));
        }
    }

    #[tokio::test]
    async fn compares_index_working_commits_and_branch_without_mutating_repository() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "initial"]);
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        fs::write(repo.path().join("file.txt"), "staged\n").unwrap();
        git(repo.path(), &["add", "file.txt"]);
        fs::write(repo.path().join("file.txt"), "working\n").unwrap();
        fs::write(repo.path().join("new file.txt"), "untracked\n").unwrap();
        let original_index = fs::read(repo.path().join(".git/index")).unwrap();
        let all = service
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        let unstaged = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(all.changes.len(), 2);
        assert_eq!(unstaged.untracked, ["new file.txt"]);
        assert_eq!(staged.changes.len(), 1);
        let content = |change: &FileChange, before: bool| {
            store
                .read_blob(
                    &if before {
                        change.before.as_ref()
                    } else {
                        change.after.as_ref()
                    }
                    .unwrap()
                    .hash,
                )
                .unwrap()
        };
        assert_eq!(content(&all.changes[0], true), b"initial\n");
        assert_eq!(content(&unstaged.changes[0], true), b"staged\n");
        assert_eq!(content(&staged.changes[0], false), b"staged\n");
        assert_eq!(
            fs::read(repo.path().join(".git/index")).unwrap(),
            original_index
        );
        git(repo.path(), &["checkout", "-b", "feature"]);
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "feature"]);
        let history = service.commits(&root, 0).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].subject, "feature");
        assert_eq!(service.branches(&root).await.unwrap().len(), 2);
        let committed = service
            .compare(
                &root,
                CompareMode::Committed,
                Some(&history[0].id),
                0,
                &store,
            )
            .await
            .unwrap();
        let branch = service
            .compare(
                &root,
                CompareMode::Branch,
                Some("refs/heads/main"),
                0,
                &store,
            )
            .await
            .unwrap();
        assert_eq!(committed.changes.len(), 2);
        assert_eq!(committed.version, branch.version);
        let first = service
            .compare(
                &root,
                CompareMode::Committed,
                Some(&history[1].id),
                0,
                &store,
            )
            .await
            .unwrap();
        assert!(first.base.is_empty());
        assert_eq!(first.changes[0].kind, super::super::ChangeKind::Added);
    }

    #[tokio::test]
    async fn index_operations_are_scoped_and_reject_stale_previews_and_locks() {
        use super::operations::IndexAction;
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join("a"), "old\n").unwrap();
        fs::write(repo.path().join("b"), "other\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join("a"), "new\n").unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let preview = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index("task", &preview, "a", IndexAction::Stage, &store)
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["show", ":a"]), "new");
        assert_eq!(git(repo.path(), &["show", ":b"]), "other");
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index("task", &staged, "a", IndexAction::Unstage, &store)
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["show", ":a"]), "old");
        assert_eq!(fs::read_to_string(repo.path().join("a")).unwrap(), "new\n");
        let preview = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        fs::write(repo.path().join("a"), "later\n").unwrap();
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        assert!(
            service
                .change_index("task", &preview, "a", IndexAction::Stage, &store)
                .await
                .is_err()
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert!(!repo.path().join(".git/index.lock").exists());
        fs::write(repo.path().join(".git/index.lock"), "external").unwrap();
        assert!(
            service
                .change_index("task", &preview, "a", IndexAction::Stage, &store)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(repo.path().join(".git/index.lock")).unwrap(),
            "external"
        );
    }

    #[tokio::test]
    async fn empty_repository_and_subdirectory_scopes() {
        let repo = repository();
        fs::create_dir(repo.path().join("one")).unwrap();
        fs::create_dir(repo.path().join("two")).unwrap();
        fs::write(repo.path().join("one/a"), "a").unwrap();
        fs::write(repo.path().join("two/b"), "b").unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let root = service
            .discover(&[repo.path().join("one").to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        assert_eq!(root.scopes, ["one"]);
        let all = service
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(all.changes.len(), 1);
        assert_eq!(all.changes[0].path, "one/a");
        assert!(service.commits(&root, 0).await.unwrap().is_empty());
        let groups = service
            .discover(&[
                repo.path().join("one").to_str().unwrap().into(),
                repo.path().to_str().unwrap().into(),
            ])
            .await
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].scopes, [""]);
    }

    #[tokio::test]
    async fn working_edited_rename_uses_frozen_content_without_staging() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        let mut original = String::new();
        for i in 0..20 {
            use std::fmt::Write;
            writeln!(original, "original line {i}").unwrap();
        }
        let checkout_original = original.replace('\n', "\r\n");
        fs::write(
            repo.path().join(".gitattributes"),
            "old.txt text eol=crlf\nnew.txt text eol=lf\n",
        )
        .unwrap();
        fs::write(repo.path().join("old.txt"), &checkout_original).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "original"]);
        fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
        let edited = format!("{original}new line\n");
        fs::write(repo.path().join("new.txt"), &edited).unwrap();
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
            let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
            assert_eq!(comparison.changes.len(), 1);
            let mut change = comparison.changes[0].clone();
            assert_eq!(change.previous_path.as_deref(), Some("old.txt"));
            assert_eq!(change.path, "new.txt");
            change.before = service
                .working_replacement(&comparison, &change, &store)
                .await
                .unwrap();
            let group = store
                .prepare_rename_recovery("rename-test", repo.path(), &change)
                .unwrap();
            store
                .apply_recovery_group("rename-test", &group.id)
                .unwrap();
            assert_eq!(
                fs::read_to_string(repo.path().join("old.txt")).unwrap(),
                checkout_original
            );
            assert!(!repo.path().join("new.txt").exists());
            store.undo_recovery_group("rename-test", &group.id).unwrap();
            assert_eq!(
                fs::read_to_string(repo.path().join("new.txt")).unwrap(),
                edited
            );
            assert!(!repo.path().join("old.txt").exists());
        }
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    }

    #[tokio::test]
    async fn checkout_conversion_preserves_crlf_and_recovery_bytes() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join(".gitattributes"), "*.txt text eol=crlf\n").unwrap();
        fs::write(repo.path().join("a.txt"), "base\r\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "base"]);
        fs::write(repo.path().join("a.txt"), "edited\r\n").unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
            let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
            let change = comparison
                .changes
                .iter()
                .find(|c| c.path == "a.txt")
                .unwrap();
            assert_eq!(
                store
                    .read_blob(&change.before.as_ref().unwrap().hash)
                    .unwrap(),
                b"base\n"
            );
            let replacement = service
                .working_replacement(&comparison, change, &store)
                .await
                .unwrap();
            assert_eq!(
                store
                    .read_blob(&replacement.as_ref().unwrap().hash)
                    .unwrap(),
                b"base\r\n"
            );
            let record = store
                .prepare_recovery(
                    "checkout-test",
                    repo.path(),
                    "a.txt",
                    change.after.clone(),
                    replacement,
                )
                .unwrap();
            store.apply_recovery("checkout-test", &record.id).unwrap();
            assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"base\r\n");
            let index = fs::read(repo.path().join(".git/index")).unwrap();
            let restored = service
                .compare(&root, CompareMode::Unstaged, None, 0, &store)
                .await
                .unwrap();
            assert!(restored.changes.is_empty());
            assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
            store.undo_recovery("checkout-test", &record.id).unwrap();
            assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"edited\r\n");
        }
        // A configured driver that no attribute names cannot convert any content.
        git(repo.path(), &["config", "filter.test.smudge", "do-not-run"]);
        assert!(
            service
                .compare(&root, CompareMode::Uncommitted, None, 0, &store)
                .await
                .is_ok()
        );
        // Once an attribute names it, comparison refuses instead of guessing its output.
        fs::write(
            repo.path().join(".gitattributes"),
            "*.txt text eol=crlf filter=test\n",
        )
        .unwrap();
        assert!(
            service
                .compare(&root, CompareMode::Uncommitted, None, 0, &store)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn checkout_conversion_matches_git_for_encoding_ident_autocrlf_and_deletion() {
        let utf16: Vec<u8> = "hello\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        for (attributes, autocrlf, initial) in [
            ("", "true", b"hello\r\n".to_vec()),
            ("a.txt ident\n", "false", b"$Id$\n".to_vec()),
            (
                "a.txt text working-tree-encoding=UTF-16LE eol=lf\n",
                "false",
                utf16,
            ),
        ] {
            let repo = repository();
            let data = tempfile::tempdir().unwrap();
            let store = ReviewStore::open(data.path().into()).unwrap();
            let service = GitReview::default();
            git(repo.path(), &["config", "core.autocrlf", autocrlf]);
            fs::write(repo.path().join(".gitattributes"), attributes).unwrap();
            fs::write(repo.path().join("a.txt"), &initial).unwrap();
            git(repo.path(), &["add", "."]);
            git(repo.path(), &["commit", "-m", "conversion fixture"]);
            // Git's actual checkout is the oracle, not a hand-coded converter.
            fs::remove_file(repo.path().join("a.txt")).unwrap();
            git(repo.path(), &["checkout", "--", "a.txt"]);
            let expected = fs::read(repo.path().join("a.txt")).unwrap();
            let root = service
                .discover(&[repo.path().to_str().unwrap().into()])
                .await
                .unwrap()
                .remove(0);
            for deleted in [false, true] {
                if deleted {
                    fs::remove_file(repo.path().join("a.txt")).unwrap();
                } else {
                    fs::write(
                        repo.path().join("a.txt"),
                        [initial.clone(), initial.clone()].concat(),
                    )
                    .unwrap();
                }
                for mode in [CompareMode::Uncommitted, CompareMode::Unstaged] {
                    let comparison = service.compare(&root, mode, None, 0, &store).await.unwrap();
                    let change = comparison
                        .changes
                        .iter()
                        .find(|c| c.path == "a.txt")
                        .unwrap();
                    let original = fs::read(repo.path().join("a.txt")).ok();
                    let replacement = service
                        .working_replacement(&comparison, change, &store)
                        .await
                        .unwrap();
                    assert_eq!(
                        store
                            .read_blob(&replacement.as_ref().unwrap().hash)
                            .unwrap(),
                        expected,
                        "{attributes}, {autocrlf}"
                    );
                    let record = store
                        .prepare_recovery(
                            "conversion-matrix",
                            repo.path(),
                            "a.txt",
                            change.after.clone(),
                            replacement,
                        )
                        .unwrap();
                    store
                        .apply_recovery("conversion-matrix", &record.id)
                        .unwrap();
                    assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), expected);
                    store
                        .undo_recovery("conversion-matrix", &record.id)
                        .unwrap();
                    assert_eq!(fs::read(repo.path().join("a.txt")).ok(), original);
                }
            }
        }
    }

    #[tokio::test]
    async fn stage_unborn_repository_applies_text_attributes_and_unstage_removes_entry() {
        use super::operations::IndexAction;
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join(".gitattributes"), "*.txt text eol=lf\n").unwrap();
        fs::write(repo.path().join("new.txt"), "one\r\ntwo\r\n").unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let preview = service
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index("task", &preview, "new.txt", IndexAction::Stage, &store)
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["show", ":new.txt"]), "one\ntwo");
        let staged = service
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        service
            .change_index("task", &staged, "new.txt", IndexAction::Unstage, &store)
            .await
            .unwrap();
        assert_eq!(git(repo.path(), &["ls-files"]), "");
        assert_eq!(
            fs::read(repo.path().join("new.txt")).unwrap(),
            b"one\r\ntwo\r\n"
        );
    }

    #[tokio::test]
    async fn immutable_blob_cache_deduplicates_content_but_not_file_modes() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        for path in ["a", "b"] {
            fs::write(repo.path().join(path), "identical\n").unwrap();
        }
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["update-index", "--chmod=+x", "b"]);
        git(
            repo.path(),
            &["commit", "-m", "same content, distinct modes"],
        );
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let deadline = Instant::now() + QUERY_BUDGET;
        let index = service
            .objects(&root, None, true, &store, deadline)
            .await
            .unwrap();
        let head = service.head(repo.path()).await.unwrap();
        let committed = service
            .objects(&root, head.as_deref(), false, &store, deadline)
            .await
            .unwrap();
        assert_eq!(service.blobs.lock().unwrap().len(), 1);
        assert!(!index.files["a"].executable);
        assert!(index.files["b"].executable);
        assert_eq!(index.files, committed.files);
        let hash = &index.files["a"].hash;
        fs::write(data.path().join("blobs").join(hash), "corrupted").unwrap();
        assert!(
            service
                .objects(&root, None, true, &store, deadline)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unstaged_skips_head_only_blobs_and_query_budget_returns_no_partial_result() {
        let repo = repository();
        let data = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        let service = GitReview::default();
        fs::write(repo.path().join("file"), "HEAD only\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "base"]);
        let old_oid = git(repo.path(), &["rev-parse", "HEAD:file"]);
        fs::write(repo.path().join("file"), "staged\n").unwrap();
        git(repo.path(), &["add", "."]);
        fs::write(repo.path().join("file"), "working\n").unwrap();
        let root = service
            .discover(&[repo.path().to_str().unwrap().into()])
            .await
            .unwrap()
            .remove(0);
        let error = service
            .compare_with_budget(
                &root,
                CompareMode::Unstaged,
                None,
                0,
                &store,
                Duration::ZERO,
            )
            .await
            .unwrap_err();
        assert!(error.contains("time budget exceeded"));
        assert!(service.blobs.lock().unwrap().is_empty());
        let comparison = service
            .compare(&root, CompareMode::Unstaged, None, 0, &store)
            .await
            .unwrap();
        assert_eq!(comparison.changes.len(), 1);
        assert_eq!(
            store
                .read_blob(&comparison.changes[0].before.as_ref().unwrap().hash)
                .unwrap(),
            b"staged\n"
        );
        assert_eq!(
            store
                .read_blob(&comparison.changes[0].after.as_ref().unwrap().hash)
                .unwrap(),
            b"working\n"
        );
        assert!(
            !service
                .blobs
                .lock()
                .unwrap()
                .contains_key(&(root.path, old_oid))
        );
    }
}
