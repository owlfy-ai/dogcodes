//! Bounded, read-only workspace facts for deterministic Context Packages.

mod filesystem;
mod fork;
pub mod review;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use agent_core::{WorkspaceContextProvider, WorkspaceContextSnapshot, WorkspaceDiffCapture};
use agent_protocol::{
    ChangeSummary, GitWorkspaceSnapshotV1, RelevantFile, WorkspaceCollectionDiagnostic,
    WorkspaceDiagnosticKind,
};
use agent_runtime::{ProcessBytesOutput, ProcessError, ProcessSpec};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

const GIT_OUTPUT_LIMIT: usize = 256 * 1_024;
const MAX_RELEVANT_FILES: usize = 1_000;
const MAX_HASHED_FILE_BYTES: u64 = 1024 * 1024;
const MAX_DIFF_BYTES: usize = 1024 * 1024;
static TEMP_INDEX_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct GitWorkspaceContextProvider {
    executable: PathBuf,
    review_directory: Option<PathBuf>,
}

impl Default for GitWorkspaceContextProvider {
    fn default() -> Self {
        Self {
            executable: discover_git().unwrap_or_else(|| PathBuf::from("git")),
            review_directory: None,
        }
    }
}

fn is_non_git(workspace: &Path) -> bool {
    workspace.is_absolute() && workspace.is_dir() && workspace.ancestors().all(|parent| {
        matches!(parent.join(".git").symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            && !(parent.join("HEAD").is_file() && parent.join("objects").is_dir())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitCollectionError {
    WorkspaceUnavailable,
    NotRepository,
    TimedOut,
    CommandFailed,
    OutputTruncated,
    PathBudgetExceeded,
    WorkspaceChanged,
    ExternalFilterConfigured,
    InvalidOutput,
}

impl From<GitCollectionError> for WorkspaceDiagnosticKind {
    fn from(value: GitCollectionError) -> Self {
        match value {
            GitCollectionError::WorkspaceUnavailable => Self::WorkspaceUnavailable,
            GitCollectionError::NotRepository => Self::NotRepository,
            GitCollectionError::TimedOut => Self::TimedOut,
            GitCollectionError::CommandFailed => Self::CommandFailed,
            GitCollectionError::OutputTruncated => Self::OutputTruncated,
            GitCollectionError::PathBudgetExceeded => Self::PathBudgetExceeded,
            GitCollectionError::WorkspaceChanged => Self::WorkspaceChanged,
            GitCollectionError::ExternalFilterConfigured => Self::ExternalFilterConfigured,
            GitCollectionError::InvalidOutput => Self::InvalidOutput,
        }
    }
}

impl GitWorkspaceContextProvider {
    #[must_use]
    pub fn discover() -> Option<Self> {
        discover_git().map(|executable| Self {
            executable,
            review_directory: None,
        })
    }

    #[must_use]
    pub fn new(executable: PathBuf) -> Option<Self> {
        executable.is_absolute().then_some(Self {
            executable,
            review_directory: None,
        })
    }

    #[must_use]
    pub fn with_review_directory(mut self, directory: PathBuf) -> Self {
        self.review_directory = Some(directory);
        self
    }

    async fn git(&self, workspace: &Path, args: &[&str]) -> Result<String, GitCollectionError> {
        self.git_with_options(
            workspace,
            args,
            BTreeMap::from([("GIT_OPTIONAL_LOCKS".to_string(), OsString::from("0"))]),
            GIT_OUTPUT_LIMIT,
        )
        .await
    }

    async fn git_with_options(
        &self,
        workspace: &Path,
        args: &[&str],
        env_overrides: BTreeMap<String, OsString>,
        stdout_limit: usize,
    ) -> Result<String, GitCollectionError> {
        let bytes = self
            .git_bytes_with_options(workspace, args, env_overrides, stdout_limit)
            .await?;
        String::from_utf8(bytes).map_err(|_| GitCollectionError::InvalidOutput)
    }

    async fn git_bytes_with_options(
        &self,
        workspace: &Path,
        args: &[&str],
        env_overrides: BTreeMap<String, OsString>,
        stdout_limit: usize,
    ) -> Result<Vec<u8>, GitCollectionError> {
        // A temporary index does not prevent clean/process filters from running.
        // Inspect effective configuration before each command, without returning
        // configured commands or filter names to the caller. Do not silently
        // disable content conversion and label the resulting patch faithful.
        let filters = self
            .git_process(
                workspace,
                &[
                    "config",
                    "--null",
                    "--name-only",
                    "--get-regexp",
                    r"^filter\..*\.(clean|smudge|process)$",
                ],
                env_overrides.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
        match filters.exit_code {
            Some(1) if filters.stdout.is_empty() => {}
            Some(0) if !filters.stdout.is_empty() => {
                // A configured driver (for example Git LFS installed globally)
                // only runs for paths whose attributes name it.
                if self
                    .filter_attributes_in_effect(workspace, &filters.stdout, &env_overrides)
                    .await?
                {
                    return Err(GitCollectionError::ExternalFilterConfigured);
                }
            }
            _ => return Err(GitCollectionError::CommandFailed),
        }
        let output = self
            .git_process(workspace, args, env_overrides, stdout_limit)
            .await?;
        if !output.success {
            return Err(GitCollectionError::CommandFailed);
        }
        Ok(output.stdout)
    }

    /// Whether any attribute source of this repository names a configured
    /// filter driver. Attributes come only from `.gitattributes` files, the
    /// repository's `info/attributes`, the global attributes file and the system
    /// file; a source that cannot be read counts as naming one, so the guard
    /// against unfaithful content conversion stays conservative.
    async fn filter_attributes_in_effect(
        &self,
        workspace: &Path,
        configured: &[u8],
        env: &BTreeMap<String, OsString>,
    ) -> Result<bool, GitCollectionError> {
        // Attributes belong to the repository, not to a temporary index an
        // operation happens to use.
        let mut env = env.clone();
        env.remove("GIT_INDEX_FILE");
        let env = &env;
        let drivers: std::collections::BTreeSet<&str> = configured
            .split(|byte| *byte == 0)
            .filter_map(|key| std::str::from_utf8(key).ok())
            .filter_map(|key| key.strip_prefix("filter.")?.rsplit_once('.'))
            .map(|(name, _)| name)
            .collect();
        if drivers.is_empty() {
            return Ok(true);
        }
        let text = |output: ProcessBytesOutput| {
            output.success.then(|| {
                String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches(['\n', '\r'])
                    .to_owned()
            })
        };
        let Some(top) = text(
            self.git_process(
                workspace,
                &["rev-parse", "--show-toplevel"],
                env.clone(),
                64 * 1024,
            )
            .await?,
        ) else {
            // Not a repository: no attributes apply, and the command itself
            // reports why it cannot run.
            return Ok(false);
        };
        let top = PathBuf::from(top);
        // (path, whether it must exist to be trusted)
        let mut sources = vec![(PathBuf::from("/etc/gitattributes"), false)];
        let Some(info) = text(
            self.git_process(
                workspace,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "info/attributes",
                ],
                env.clone(),
                64 * 1024,
            )
            .await?,
        ) else {
            return Ok(true);
        };
        sources.push((PathBuf::from(info), false));
        let global = self
            .git_process(
                workspace,
                &["config", "--path", "--get", "core.attributesFile"],
                env.clone(),
                64 * 1024,
            )
            .await?;
        match global.exit_code {
            Some(0) => sources.push((PathBuf::from(text(global).unwrap_or_default()), false)),
            Some(1) => {
                let base = std::env::var_os("XDG_CONFIG_HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                    });
                if let Some(base) = base {
                    sources.push((base.join("git/attributes"), false));
                }
            }
            _ => return Ok(true),
        }
        let mut listed = self
            .git_process(
                &top,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "--",
                    ":(glob)**/.gitattributes",
                ],
                env.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
        if !listed.success {
            // An unreadable index (for example mid-recovery): the committed tree
            // still lists the tracked attribute files without needing it, and the
            // worktree holds every attribute file Git could read from it.
            if !Self::worktree_attribute_files(&top, &mut sources) {
                return Ok(true);
            }
            let unborn = !self
                .git_process(
                    &top,
                    &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
                    env.clone(),
                    64 * 1024,
                )
                .await?
                .success;
            if unborn {
                listed.success = true;
                listed.stdout.clear();
            } else {
                listed = self
                    .git_process(
                        &top,
                        &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
                        env.clone(),
                        GIT_OUTPUT_LIMIT,
                    )
                    .await?;
            }
        }
        if !listed.success || listed.stdout_truncated {
            return Ok(true);
        }
        for path in listed
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let Ok(path) = std::str::from_utf8(path) else {
                return Ok(true);
            };
            // ls-tree does not support glob pathspecs. Filter its bounded output
            // here; a tracked missing attributes file still applies from the index.
            if Path::new(path)
                .file_name()
                .is_some_and(|name| name == ".gitattributes")
            {
                sources.push((top.join(path), true));
            }
        }
        for (source, required) in sources {
            match std::fs::read(&source) {
                Ok(bytes) => {
                    let names = String::from_utf8_lossy(&bytes);
                    if names
                        .split_whitespace()
                        .filter_map(|token| token.strip_prefix("filter="))
                        .any(|name| drivers.contains(name))
                    {
                        return Ok(true);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {}
                Err(_) => return Ok(true),
            }
        }
        Ok(false)
    }

    /// Adds every `.gitattributes` below `top`, skipping Git directories. Returns
    /// false when the tree is too large or unreadable to scan with confidence.
    fn worktree_attribute_files(top: &Path, sources: &mut Vec<(PathBuf, bool)>) -> bool {
        let mut pending = vec![top.to_path_buf()];
        let mut visited = 0usize;
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                return false;
            };
            for entry in entries {
                visited += 1;
                let Ok(entry) = entry else { return false };
                if visited > 100_000 {
                    return false;
                }
                let Ok(kind) = entry.file_type() else {
                    return false;
                };
                let name = entry.file_name();
                if kind.is_dir() && name != ".git" {
                    pending.push(entry.path());
                } else if kind.is_file() && name == ".gitattributes" {
                    sources.push((entry.path(), true));
                }
            }
        }
        true
    }

    async fn git_process(
        &self,
        workspace: &Path,
        args: &[&str],
        env_overrides: BTreeMap<String, OsString>,
        stdout_limit: usize,
    ) -> Result<ProcessBytesOutput, GitCollectionError> {
        self.git_process_input(workspace, args, env_overrides, stdout_limit, None)
            .await
    }

    async fn git_process_input(
        &self,
        workspace: &Path,
        args: &[&str],
        mut env_overrides: BTreeMap<String, OsString>,
        stdout_limit: usize,
        input: Option<&[u8]>,
    ) -> Result<ProcessBytesOutput, GitCollectionError> {
        let mut command_args = vec![
            "-c".to_string(),
            "color.ui=false".to_string(),
            "-c".to_string(),
            "core.quotepath=false".to_string(),
            "-c".to_string(),
            "core.fsmonitor=false".to_string(),
            "-c".to_string(),
            "diff.autoRefreshIndex=false".to_string(),
            "-c".to_string(),
            format!(
                "core.hooksPath={}",
                if cfg!(windows) { "NUL" } else { "/dev/null" }
            ),
            "-c".to_string(),
            "core.untrackedCache=false".to_string(),
            "-c".to_string(),
            "core.splitIndex=false".to_string(),
            "-c".to_string(),
            "gc.auto=0".to_string(),
            "-c".to_string(),
            "maintenance.auto=false".to_string(),
            "-C".to_string(),
            workspace
                .to_str()
                .ok_or(GitCollectionError::InvalidOutput)?
                .to_string(),
        ];
        command_args.extend(args.iter().map(|value| (*value).to_string()));
        let mut spec = ProcessSpec::probe(self.executable.clone(), command_args);
        #[cfg(windows)]
        spec.env_allowlist.insert("PATH".into());
        // Only file enumeration needs user configuration to resolve global
        // excludes. Keep content-reading commands isolated from global filters.
        if args.first() == Some(&"ls-files") && args.contains(&"--exclude-standard") {
            spec.env_allowlist.extend(
                [
                    "HOME",
                    "XDG_CONFIG_HOME",
                    "USERPROFILE",
                    "HOMEDRIVE",
                    "HOMEPATH",
                    "GIT_CONFIG_GLOBAL",
                    "GIT_CONFIG_SYSTEM",
                    "GIT_CONFIG_NOSYSTEM",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        spec.timeout = Duration::from_secs(if args.first() == Some(&"worktree") {
            120
        } else {
            3
        });
        spec.stdout_limit = stdout_limit;
        spec.stderr_limit = 16 * 1_024;
        env_overrides.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
        env_overrides.insert("GIT_NO_LAZY_FETCH".into(), "1".into());
        env_overrides.insert("GIT_ALLOW_PROTOCOL".into(), "".into());
        env_overrides.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        spec.env_overrides = env_overrides;
        let output = agent_runtime::run_bounded_bytes_with_input(&spec, input)
            .await
            .map_err(|error| match error {
                ProcessError::TimedOut { .. } => GitCollectionError::TimedOut,
                ProcessError::InvalidSpec(_) | ProcessError::Spawn(_) | ProcessError::Io(_) => {
                    GitCollectionError::CommandFailed
                }
            })?;
        if output.stdout_truncated || output.stderr_truncated {
            return Err(GitCollectionError::OutputTruncated);
        }
        Ok(output)
    }

    async fn collect_git(
        &self,
        workspace: &Path,
    ) -> Result<WorkspaceContextSnapshot, GitCollectionError> {
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(GitCollectionError::WorkspaceUnavailable);
        }
        match self
            .git(workspace, &["rev-parse", "--is-inside-work-tree"])
            .await
        {
            Ok(value) if value.trim() == "true" => {}
            Ok(_) | Err(GitCollectionError::CommandFailed) => {
                return Err(GitCollectionError::NotRepository);
            }
            Err(error) => return Err(error),
        }
        let repo_output = self
            .git(workspace, &["rev-parse", "--show-toplevel"])
            .await?;
        let repo_root = repo_output
            .strip_suffix('\n')
            .unwrap_or(&repo_output)
            .to_string();
        let git_dir_output = self
            .git(workspace, &["rev-parse", "--absolute-git-dir"])
            .await?;
        let git_dir = git_dir_output.strip_suffix('\n').unwrap_or(&git_dir_output);
        if repo_root.is_empty() || git_dir.is_empty() {
            return Err(GitCollectionError::InvalidOutput);
        }
        let status = self
            .git(
                workspace,
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "-z",
                    "--untracked-files=all",
                    "--",
                    ".",
                ],
            )
            .await?;
        let (branch, detached) = parse_branch(&status);
        let mut relevant_files = parse_status_v2(&status)?;
        if relevant_files.len() > MAX_RELEVANT_FILES {
            return Err(GitCollectionError::PathBudgetExceeded);
        }
        for file in &mut relevant_files {
            file.content_hash = hash_regular_file(Path::new(&repo_root), &file.path);
        }
        let dependency_changes = relevant_files
            .iter()
            .filter(|file| is_dependency_file(&file.path))
            .cloned()
            .collect();
        let head = parse_head(&status)?;
        let base = head.clone().unwrap_or_else(|| "unborn".into());
        let unstaged = self
            .git(
                workspace,
                &[
                    "diff",
                    "--stat",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--",
                    ".",
                ],
            )
            .await?;
        let staged = self
            .git(
                workspace,
                &[
                    "diff",
                    "--cached",
                    "--stat",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--",
                    ".",
                ],
            )
            .await?;
        let diff_stat = [staged.trim(), unstaged.trim()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let mut fingerprint = Sha256::new();
        fingerprint.update(repo_root.as_bytes());
        fingerprint.update(base.as_bytes());
        fingerprint.update(branch.as_deref().unwrap_or("(detached)"));
        fingerprint.update([u8::from(detached)]);
        let worktree_identity =
            format!("sha256:{}", hex::encode(Sha256::digest(git_dir.as_bytes())));
        fingerprint.update(worktree_identity.as_bytes());
        fingerprint.update(status.as_bytes());
        fingerprint.update(staged.as_bytes());
        fingerprint.update(unstaged.as_bytes());
        for file in &relevant_files {
            fingerprint.update(file.path.as_bytes());
            fingerprint.update(file.status.as_bytes());
            if let Some(content_hash) = &file.content_hash {
                fingerprint.update(content_hash.as_bytes());
            }
        }
        let relevant_file_count = relevant_files.len();
        let status_fingerprint = hex::encode(fingerprint.finalize());
        Ok(WorkspaceContextSnapshot {
            relevant_files,
            dependency_changes,
            change_summary: ChangeSummary {
                base,
                head: format!("working-tree:{status_fingerprint}"),
                diff_stat: if diff_stat.is_empty() && relevant_file_count == 0 {
                    "clean".into()
                } else if diff_stat.is_empty() {
                    format!("{relevant_file_count} changed paths; line stat unavailable")
                } else {
                    diff_stat
                },
                artifact_ref: None,
            },
            git_snapshot: Some(GitWorkspaceSnapshotV1 {
                schema_version: 1,
                repo_root,
                head,
                branch,
                detached,
                worktree_identity,
                status_format: "porcelain_v2".into(),
                status_fingerprint,
            }),
            diagnostic: None,
        })
    }

    async fn capture_git_diff(
        &self,
        workspace: &Path,
    ) -> Result<WorkspaceDiffCapture, GitCollectionError> {
        let before = self.collect_git(workspace).await?;
        let before_git = before
            .git_snapshot
            .clone()
            .ok_or(GitCollectionError::InvalidOutput)?;
        let temporary = TemporaryGitIndex::new()?;
        let object_output = self
            .git(workspace, &["rev-parse", "--git-path", "objects"])
            .await?;
        let object_path = PathBuf::from(object_output.strip_suffix('\n').unwrap_or(&object_output));
        let object_path = if object_path.is_absolute() {
            object_path
        } else {
            workspace.join(object_path)
        };
        if !object_path.is_dir() {
            return Err(GitCollectionError::InvalidOutput);
        }
        let mut environment = BTreeMap::from([
            (
                "GIT_INDEX_FILE".to_string(),
                temporary.index_path.as_os_str().to_owned(),
            ),
            ("GIT_OPTIONAL_LOCKS".to_string(), OsString::from("0")),
            (
                "GIT_OBJECT_DIRECTORY".to_string(),
                temporary.object_directory.as_os_str().to_owned(),
            ),
            (
                "GIT_ALTERNATE_OBJECT_DIRECTORIES".to_string(),
                object_path.as_os_str().to_owned(),
            ),
        ]);
        let base = if let Some(head) = &before_git.head {
            self.git_with_options(
                workspace,
                &["read-tree", head],
                environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
            head.clone()
        } else {
            self.git_with_options(
                workspace,
                &["read-tree", "--empty"],
                environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
            self.git_with_options(
                workspace,
                &["hash-object", "-t", "tree", "--stdin"],
                environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?
            .trim()
            .to_string()
        };
        if base.is_empty() {
            return Err(GitCollectionError::InvalidOutput);
        }
        self.git_with_options(
            workspace,
            &["add", "-A", "--", "."],
            environment.clone(),
            GIT_OUTPUT_LIMIT,
        )
        .await?;
        let diff = self
            .git_bytes_with_options(
                workspace,
                &[
                    "-c",
                    "core.quotepath=true",
                    "diff",
                    "--cached",
                    "--binary",
                    "--full-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    &base,
                    "--",
                    ".",
                ],
                std::mem::take(&mut environment),
                MAX_DIFF_BYTES,
            )
            .await?;
        let after = self.collect_git(workspace).await?;
        let after_git = after
            .git_snapshot
            .ok_or(GitCollectionError::InvalidOutput)?;
        if before_git.status_fingerprint != after_git.status_fingerprint {
            return Err(GitCollectionError::WorkspaceChanged);
        }
        Ok(WorkspaceDiffCapture {
            base: before.change_summary.base,
            status_fingerprint: before_git.status_fingerprint,
            content: diff,
        })
    }
}

#[async_trait]
impl WorkspaceContextProvider for GitWorkspaceContextProvider {
    async fn capture_fork_snapshot(&self, paths: &[String]) -> Result<serde_json::Value, String> {
        self.save_fork_snapshot(paths).await
    }

    async fn fork_worktrees(
        &self,
        snapshot: serde_json::Value,
        name: &str,
    ) -> Result<Vec<String>, String> {
        self.restore_fork_snapshot(snapshot, name).await
    }

    async fn delete_task_review_data(&self, task_id: &str) -> Result<(), String> {
        let Some(directory) = self.review_directory.clone() else {
            return Ok(());
        };
        let task = task_id.to_owned();
        tokio::task::spawn_blocking(move || {
            review::ReviewStore::try_open_maintenance(directory)?.delete_task_data(&task)
        })
        .await
        .map_err(|_| "快照清理任务失败".to_string())?
    }

    async fn validate_workspace_roots(
        &self,
        directories: &[String],
    ) -> Result<(), WorkspaceCollectionDiagnostic> {
        for root in directories {
            if !std::fs::metadata(root).is_ok_and(|metadata| metadata.is_dir()) {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
                });
            }
        }
        Ok(())
    }
    async fn recover_turn(
        &self,
        workspace_path: &str,
        checkpoint: serde_json::Value,
    ) -> Result<serde_json::Value, WorkspaceCollectionDiagnostic> {
        let capture = self.begin_turn(workspace_path).await?;
        let after = capture
            .checkpoint()
            .await?
            .ok_or(WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
            })?;
        let mut changes = filesystem::compare(&checkpoint, &after)?;
        changes["attribution"] = serde_json::json!("workspace_recovery_interval");
        changes["recoveredAfterInterruption"] = serde_json::json!(true);
        Ok(changes)
    }
    async fn begin_turn(
        &self,
        workspace_path: &str,
    ) -> Result<Box<dyn agent_core::WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
        // A missing repository is a filesystem fact, not any failed Git command.
        // Broken/inaccessible .git entries must continue through the Git backend.
        let workspace = Path::new(workspace_path);
        if is_non_git(workspace) {
            return filesystem::begin(workspace);
        }
        let result = async {
            let workspace = PathBuf::from(workspace_path);
            let facts = self.collect_git(&workspace).await?;
            let temporary = TemporaryGitIndex::new()?;
            let objects = self
                .git(&workspace, &["rev-parse", "--git-path", "objects"])
                .await?;
            let objects = PathBuf::from(objects.trim_end_matches('\n'));
            let objects = if objects.is_absolute() {
                objects
            } else {
                workspace.join(objects)
            };
            let environment = BTreeMap::from([
                (
                    "GIT_INDEX_FILE".into(),
                    temporary.index_path.as_os_str().to_owned(),
                ),
                (
                    "GIT_OBJECT_DIRECTORY".into(),
                    temporary.object_directory.as_os_str().to_owned(),
                ),
                (
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                    objects.into_os_string(),
                ),
                ("GIT_OPTIONAL_LOCKS".into(), OsString::from("0")),
            ]);
            let head = facts.git_snapshot.as_ref().and_then(|s| s.head.as_deref());
            self.git_with_options(
                &workspace,
                &["read-tree", head.unwrap_or("--empty")],
                environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
            let mut capture = TurnCapture {
                provider: Self {
                    executable: self.executable.clone(),
                    review_directory: self.review_directory.clone(),
                },
                workspace,
                environment,
                _temporary: temporary,
                before: String::new(),
            };
            capture.before = capture.tree().await?;
            Ok::<_, GitCollectionError>(capture)
        }
        .await;
        result
            .map(|capture| Box::new(capture) as Box<dyn agent_core::WorkspaceTurnCapture>)
            .map_err(|e| WorkspaceCollectionDiagnostic { kind: e.into() })
    }
    async fn collect(&self, workspace_path: &str) -> WorkspaceContextSnapshot {
        if is_non_git(Path::new(workspace_path)) {
            return WorkspaceContextSnapshot::default();
        }
        match self.collect_git(Path::new(workspace_path)).await {
            Ok(snapshot) => snapshot,
            Err(error) => WorkspaceContextSnapshot {
                diagnostic: Some(WorkspaceCollectionDiagnostic { kind: error.into() }),
                ..WorkspaceContextSnapshot::default()
            },
        }
    }

    async fn capture_diff(
        &self,
        workspace_path: &str,
    ) -> Result<WorkspaceDiffCapture, WorkspaceCollectionDiagnostic> {
        self.capture_git_diff(Path::new(workspace_path))
            .await
            .map_err(|error| WorkspaceCollectionDiagnostic { kind: error.into() })
    }
}

struct TurnCapture {
    provider: GitWorkspaceContextProvider,
    workspace: PathBuf,
    environment: BTreeMap<String, OsString>,
    _temporary: TemporaryGitIndex,
    before: String,
}

impl TurnCapture {
    async fn tree(&self) -> Result<String, GitCollectionError> {
        let before = self.provider.collect_git(&self.workspace).await?;
        self.provider
            .git_with_options(
                &self.workspace,
                &["add", "-A", "--", "."],
                self.environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
        let tree = self
            .provider
            .git_with_options(
                &self.workspace,
                &["write-tree"],
                self.environment.clone(),
                GIT_OUTPUT_LIMIT,
            )
            .await?;
        let after = self.provider.collect_git(&self.workspace).await?;
        if before != after {
            return Err(GitCollectionError::WorkspaceChanged);
        }
        Ok(tree.trim().to_string())
    }
}

#[async_trait]
impl agent_core::WorkspaceTurnCapture for TurnCapture {
    async fn checkpoint(&self) -> Result<Option<serde_json::Value>, WorkspaceCollectionDiagnostic> {
        let result = async {
            let listing = self.provider.git_with_options(&self.workspace, &["ls-tree", "-r", "-z", &self.before, "--", "."], self.environment.clone(), GIT_OUTPUT_LIMIT).await?;
            let mut entries = BTreeMap::new();
            let mut budget = 128_000usize;
            for entry in listing.split('\0').filter(|v| !v.is_empty()) {
                let (metadata, path) = entry.split_once('\t').ok_or(GitCollectionError::InvalidOutput)?;
                let fields: Vec<_> = metadata.split_whitespace().collect();
                if fields.len() != 3 || fields[1] != "blob" { return Err(GitCollectionError::InvalidOutput); }
                if entries.len() >= MAX_RELEVANT_FILES || budget == 0 { return Err(GitCollectionError::OutputTruncated); }
                let bytes = self.provider.git_bytes_with_options(&self.workspace, &["cat-file", "blob", fields[2]], self.environment.clone(), budget).await?;
                budget -= bytes.len();
                entries.insert(path.to_string(), serde_json::json!({"mode":fields[0],"sha256":hex::encode(Sha256::digest(&bytes)),"hex":hex::encode(bytes)}));
            }
            Ok(Some(filesystem::checkpoint("git", &entries)))
        }.await;
        result.map_err(|e: GitCollectionError| WorkspaceCollectionDiagnostic { kind: e.into() })
    }
    async fn finish(&self) -> Result<serde_json::Value, WorkspaceCollectionDiagnostic> {
        let result = async {
            let after = self.tree().await?;
            let diff = self.provider.git_bytes_with_options(&self.workspace,
                &["-c", "core.quotepath=true", "diff", "--binary", "--full-index", "--no-ext-diff", "--no-textconv", &self.before, &after, "--", "."],
                self.environment.clone(), MAX_DIFF_BYTES / 2).await?;
            let paths = self.provider.git_with_options(&self.workspace,
                &["diff", "--name-status", "-z", "--no-renames", &self.before, &after, "--", "."], self.environment.clone(), GIT_OUTPUT_LIMIT).await?;
            let fields: Vec<_> = paths.split('\0').filter(|v| !v.is_empty()).collect();
            let prefix = self.provider.git_with_options(&self.workspace, &["rev-parse", "--show-prefix"], self.environment.clone(), GIT_OUTPUT_LIMIT).await?;
            let prefix = prefix.trim_end_matches('\n');
            let mut modes = BTreeMap::new();
            for tree in [&self.before, &after] {
                let listing = self.provider.git_with_options(&self.workspace,
                    &["ls-tree", "-r", "-z", "--full-name", tree, "--", "."], self.environment.clone(), GIT_OUTPUT_LIMIT).await?;
                for entry in listing.split('\0').filter(|v| !v.is_empty()) {
                    let (metadata, path) = entry.split_once('\t').ok_or(GitCollectionError::InvalidOutput)?;
                    let mode = metadata.split_whitespace().next().ok_or(GitCollectionError::InvalidOutput)?;
                    modes.insert((tree.as_str(), path.to_owned()), mode.to_owned());
                }
            }
            let mut files = Vec::new();
            let mut snapshot_budget = 128_000usize;
            for pair in fields.chunks_exact(2) {
                let relative = pair[1].strip_prefix(prefix).ok_or(GitCollectionError::InvalidOutput)?;
                let mut file = serde_json::json!({"status":pair[0],"path":pair[1],"workspaceRelativePath":relative});
                for (key, tree, absent) in [("before", self.before.as_str(), pair[0] == "A"), ("after", after.as_str(), pair[0] == "D")] {
                    if absent { file[key] = serde_json::Value::Null; continue; }
                    let object = format!("{tree}:{}", pair[1]);
                    if snapshot_budget == 0 { file[key] = serde_json::json!({"contentOmitted":true}); continue; }
                    match self.provider.git_bytes_with_options(&self.workspace, &["cat-file", "blob", &object], self.environment.clone(), snapshot_budget).await {
                        Ok(bytes) => {
                            snapshot_budget -= bytes.len();
                            let mode = modes.get(&(tree, pair[1].to_owned())).ok_or(GitCollectionError::InvalidOutput)?;
                            file[key] = serde_json::json!({"mode":mode,"sha256":hex::encode(Sha256::digest(&bytes)),"hex":hex::encode(bytes)});
                        }
                        Err(_) => { file[key] = serde_json::json!({"contentOmitted":true}); }
                    }
                }
                files.push(file);
            }
            let (patch, patch_hex) = match String::from_utf8(diff) {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(hex::encode(error.into_bytes()))),
            };
            Ok::<_, GitCollectionError>(serde_json::json!({"schemaVersion":1,"beforeTree":self.before,"afterTree":after,
                "attribution":"workspace_interval", "scope":"tracked and non-ignored files under selected workspace; submodule worktrees excluded",
                "files":files,"patch":patch,"patchHex":patch_hex}))
        }.await;
        result.map_err(|e| WorkspaceCollectionDiagnostic { kind: e.into() })
    }
}

struct TemporaryGitIndex {
    directory: PathBuf,
    index_path: PathBuf,
    object_directory: PathBuf,
}

impl TemporaryGitIndex {
    fn new() -> Result<Self, GitCollectionError> {
        for _ in 0..16 {
            let sequence = TEMP_INDEX_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir()
                .join(format!("dogcodes-index-{}-{sequence}", std::process::id()));
            match create_private_directory(&directory) {
                Ok(()) => {
                    let object_directory = directory.join("objects");
                    if create_private_directory(&object_directory).is_err() {
                        let _ = std::fs::remove_dir(&directory);
                        return Err(GitCollectionError::WorkspaceUnavailable);
                    }
                    return Ok(Self {
                        index_path: directory.join("index"),
                        object_directory,
                        directory,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(GitCollectionError::WorkspaceUnavailable),
            }
        }
        Err(GitCollectionError::WorkspaceUnavailable)
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

impl Drop for TemporaryGitIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn parse_status_v2(status: &str) -> Result<Vec<RelevantFile>, GitCollectionError> {
    let mut files = BTreeMap::<String, String>::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.starts_with("# ") {
            continue;
        }
        let (code, path, consumes_original_path) = if let Some(value) = entry.strip_prefix("1 ") {
            let fields = value.splitn(8, ' ').collect::<Vec<_>>();
            if fields.len() != 8 {
                return Err(GitCollectionError::InvalidOutput);
            }
            (fields[0], fields[7], false)
        } else if let Some(value) = entry.strip_prefix("2 ") {
            let fields = value.splitn(9, ' ').collect::<Vec<_>>();
            if fields.len() != 9 {
                return Err(GitCollectionError::InvalidOutput);
            }
            (fields[0], fields[8], true)
        } else if let Some(value) = entry.strip_prefix("u ") {
            let fields = value.splitn(10, ' ').collect::<Vec<_>>();
            if fields.len() != 10 {
                return Err(GitCollectionError::InvalidOutput);
            }
            (fields[0], fields[9], false)
        } else if let Some(path) = entry.strip_prefix("? ") {
            ("??", path, false)
        } else {
            return Err(GitCollectionError::InvalidOutput);
        };
        if !safe_relative_path(path) {
            return Err(GitCollectionError::InvalidOutput);
        }
        let state = if code.contains('D') {
            "deleted"
        } else if code.contains('R') {
            "renamed"
        } else if code.contains('A') || code == "??" {
            "added"
        } else {
            "modified"
        };
        files.insert(path.to_string(), state.to_string());
        if consumes_original_path && entries.next().is_none() {
            return Err(GitCollectionError::InvalidOutput);
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, status)| RelevantFile {
            path,
            status,
            content_hash: None,
        })
        .collect())
}

fn parse_head(status: &str) -> Result<Option<String>, GitCollectionError> {
    match status
        .split('\0')
        .find_map(|entry| entry.strip_prefix("# branch.oid "))
    {
        Some("(initial)") => Ok(None),
        Some(oid)
            if matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Ok(Some(oid.to_string()))
        }
        _ => Err(GitCollectionError::InvalidOutput),
    }
}

fn parse_branch(status: &str) -> (Option<String>, bool) {
    let head = status
        .split('\0')
        .find_map(|entry| entry.strip_prefix("# branch.head "));
    match head {
        Some("(detached)") | None => (None, true),
        Some(branch) => (Some(branch.to_string()), false),
    }
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn hash_regular_file(workspace: &Path, relative: &str) -> Option<String> {
    let path = workspace.join(relative);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_HASHED_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(Sha256::digest(bytes)))
}

fn is_dependency_file(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "Cargo.toml"
                    | "Cargo.lock"
                    | "package.json"
                    | "pnpm-lock.yaml"
                    | "yarn.lock"
                    | "package-lock.json"
                    | "requirements.txt"
                    | "pyproject.toml"
                    | "uv.lock"
                    | "go.mod"
                    | "go.sum"
            )
        })
}

fn discover_git() -> Option<PathBuf> {
    let known = ["/usr/bin/git", "/opt/homebrew/bin/git"]
        .into_iter()
        .map(PathBuf::from);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(if cfg!(windows) { "git.exe" } else { "git" }));
    known
        .chain(candidates)
        .filter(|path| path.is_absolute() && path.is_file())
        .find_map(|path| path.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn global_ignore_inherited_child() {
        use super::*;
        let Some(root) = std::env::var_os("DOGCODES_IGNORE_TEST_REPO") else {
            return;
        };
        let files = GitWorkspaceContextProvider::default()
            .git_bytes_with_options(
                Path::new(&root),
                &["ls-files", "--others", "--exclude-standard", "-z"],
                BTreeMap::new(),
                GIT_OUTPUT_LIMIT,
            )
            .await
            .unwrap();
        assert_eq!(files, b"visible.txt\0");
    }

    #[tokio::test]
    async fn global_ignore_configuration_is_respected() {
        use super::*;
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let provider = GitWorkspaceContextProvider::default();
        provider
            .git_bytes_with_options(repo.path(), &["init"], BTreeMap::new(), GIT_OUTPUT_LIMIT)
            .await
            .unwrap();
        std::fs::create_dir(repo.path().join(".claude")).unwrap();
        std::fs::write(repo.path().join(".claude/settings.local.json"), "{}").unwrap();
        std::fs::write(repo.path().join("visible.txt"), "visible").unwrap();
        let config = home.path().join("config");
        std::fs::create_dir_all(config.join("git")).unwrap();
        std::fs::write(
            config.join("git/ignore"),
            "**/.claude/settings.local.json\n",
        )
        .unwrap();
        let mut env = BTreeMap::from([
            ("HOME".into(), home.path().as_os_str().to_owned()),
            ("XDG_CONFIG_HOME".into(), config.as_os_str().to_owned()),
            (
                "GIT_CONFIG_GLOBAL".into(),
                home.path().join(".gitconfig").into_os_string(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]);
        // Run in a child test process to verify inherited configuration without
        // mutating process-wide environment during parallel Rust tests.
        let inherited = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::global_ignore_inherited_child",
                "--nocapture",
            ])
            .envs(&env)
            .env("DOGCODES_IGNORE_TEST_REPO", repo.path())
            .output()
            .unwrap();
        assert!(
            inherited.status.success(),
            "{}{}",
            String::from_utf8_lossy(&inherited.stdout),
            String::from_utf8_lossy(&inherited.stderr)
        );
        let args = ["ls-files", "--others", "--exclude-standard", "-z"];
        let files = provider
            .git_bytes_with_options(repo.path(), &args, env.clone(), GIT_OUTPUT_LIMIT)
            .await
            .unwrap();
        assert_eq!(files, b"visible.txt\0");
        // A configured core.excludesFile must override the default XDG ignore file.
        std::fs::write(home.path().join("custom-ignore"), "visible.txt\n").unwrap();
        std::fs::write(
            home.path().join(".gitconfig"),
            "[core]\nexcludesFile = ~/custom-ignore\n",
        )
        .unwrap();
        let files = provider
            .git_bytes_with_options(repo.path(), &args, env.clone(), GIT_OUTPUT_LIMIT)
            .await
            .unwrap();
        assert_eq!(files, b".claude/settings.local.json\0");
        // Without an explicit XDG location, Git uses ~/.config/git/ignore.
        std::fs::remove_file(home.path().join(".gitconfig")).unwrap();
        std::fs::create_dir_all(home.path().join(".config/git")).unwrap();
        std::fs::write(
            home.path().join(".config/git/ignore"),
            "**/.claude/settings.local.json\n",
        )
        .unwrap();
        env.insert("XDG_CONFIG_HOME".into(), "".into());
        let files = provider
            .git_bytes_with_options(repo.path(), &args, env, GIT_OUTPUT_LIMIT)
            .await
            .unwrap();
        assert_eq!(files, b"visible.txt\0");
    }

    use super::*;

    fn run_git(provider: &GitWorkspaceContextProvider, workspace: &Path, args: &[&str]) {
        let status = std::process::Command::new(&provider.executable)
            .args(args)
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn temporary_workspace(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("dogcodes-{name}-{}-{unique}", std::process::id()))
    }

    fn count_files(root: &Path) -> usize {
        std::fs::read_dir(root).map_or(0, |entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| {
                    if entry.path().is_dir() {
                        count_files(&entry.path())
                    } else {
                        1
                    }
                })
                .sum()
        })
    }

    #[test]
    fn parses_sorted_bounded_status_and_dependency_changes() {
        let files = parse_status_v2(
            "1 .M N... 100644 100644 100644 abc def src/lib.rs\0? Cargo.lock\0\
             1 D. N... 100644 000000 000000 abc 000 old.txt\0",
        )
        .unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["Cargo.lock", "old.txt", "src/lib.rs"]
        );
        assert_eq!(files[0].status, "added");
        assert!(is_dependency_file(&files[0].path));
        assert!(!safe_relative_path("../secret"));
        assert_eq!(parse_head("# branch.oid (initial)\0"), Ok(None));
        assert_eq!(
            parse_head("# branch.oid invalid\0"),
            Err(GitCollectionError::InvalidOutput)
        );
        assert_eq!(
            parse_head("# branch.head main\0"),
            Err(GitCollectionError::InvalidOutput)
        );
        assert_eq!(
            parse_status_v2("1 malformed\0"),
            Err(GitCollectionError::InvalidOutput)
        );
    }

    #[tokio::test]
    async fn durable_git_checkpoint_survives_private_object_cleanup() {
        let provider = GitWorkspaceContextProvider::discover().unwrap();
        let workspace = temporary_workspace("durable-turn");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("file.txt"), "before\n").unwrap();
        let capture = provider
            .begin_turn(workspace.to_str().unwrap())
            .await
            .unwrap();
        let checkpoint = capture.checkpoint().await.unwrap().unwrap();
        drop(capture);
        std::fs::write(workspace.join("file.txt"), "recovered\n").unwrap();
        let changes = provider
            .recover_turn(workspace.to_str().unwrap(), checkpoint)
            .await
            .unwrap();
        assert_eq!(changes["attribution"], "workspace_recovery_interval");
        assert!(
            changes["patch"]
                .as_str()
                .unwrap()
                .contains("-before\n+recovered")
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn turn_diff_excludes_preexisting_edits_and_preserves_user_index() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("turn-net-diff");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("file.txt"), "original\n").unwrap();
        run_git(&provider, &workspace, &["add", "file.txt"]);
        let index_before = std::fs::read(workspace.join(".git/index")).unwrap();
        std::fs::write(workspace.join("file.txt"), "user edit\n").unwrap();
        let capture = provider
            .begin_turn(workspace.to_str().unwrap())
            .await
            .unwrap();
        std::fs::write(workspace.join("file.txt"), "agent edit\n").unwrap();
        std::fs::write(workspace.join("new.txt"), "new content\n").unwrap();
        let change = capture.finish().await.unwrap();
        let patch = change["patch"].as_str().unwrap();
        assert!(patch.contains("-user edit"));
        assert!(patch.contains("+agent edit"));
        assert!(!patch.contains("-original"));
        assert_eq!(
            change["files"][0]["before"]["hex"],
            hex::encode("user edit\n")
        );
        assert_eq!(
            change["files"][0]["after"]["hex"],
            hex::encode("agent edit\n")
        );
        assert_eq!(
            std::fs::read(workspace.join(".git/index")).unwrap(),
            index_before
        );
        drop(capture);
        let unchanged = provider
            .begin_turn(workspace.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(unchanged.finish().await.unwrap()["patch"], "");
        drop(unchanged);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn collects_a_stable_snapshot_and_detects_workspace_drift() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("Cargo.toml"), b"[package]\nname='demo'\n").unwrap();
        std::fs::write(workspace.join("src/lib.rs"), b"pub fn demo() {}\n").unwrap();

        let first = provider.collect(workspace.to_str().unwrap()).await;
        let second = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(first, second);
        assert_eq!(first.relevant_files.len(), 2);
        assert_eq!(first.dependency_changes.len(), 1);
        assert_eq!(first.dependency_changes[0].path, "Cargo.toml");
        assert!(
            first
                .relevant_files
                .iter()
                .all(|file| file.content_hash.is_some())
        );

        std::fs::write(workspace.join("src/lib.rs"), b"pub fn changed() {}\n").unwrap();
        let changed = provider.collect(workspace.to_str().unwrap()).await;
        assert_ne!(first.change_summary.head, changed.change_summary.head);
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn classifies_staged_unstaged_rename_delete_binary_symlink_and_unicode() {
        use std::os::unix::fs::symlink;

        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-matrix");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        for (path, content) in [
            ("mixed.txt", b"base\n".as_slice()),
            ("delete.txt", b"delete\n".as_slice()),
            ("rename-old.txt", b"rename\n".as_slice()),
            ("binary.bin", &[0, 1, 2, 255]),
        ] {
            std::fs::write(workspace.join(path), content).unwrap();
        }
        run_git(&provider, &workspace, &["add", "."]);
        run_git(
            &provider,
            &workspace,
            &[
                "-c",
                "user.name=dogcodes Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        );

        std::fs::write(workspace.join("mixed.txt"), b"staged\n").unwrap();
        run_git(&provider, &workspace, &["add", "mixed.txt"]);
        std::fs::write(workspace.join("mixed.txt"), b"staged and unstaged\n").unwrap();
        run_git(
            &provider,
            &workspace,
            &["mv", "rename-old.txt", "rename-new.txt"],
        );
        std::fs::remove_file(workspace.join("delete.txt")).unwrap();
        std::fs::write(workspace.join("binary.bin"), [0, 159, 146, 150]).unwrap();
        std::fs::write(workspace.join("你好.rs"), b"pub fn unicode() {}\n").unwrap();
        symlink("mixed.txt", workspace.join("mixed-link")).unwrap();

        let snapshot = provider.collect(workspace.to_str().unwrap()).await;
        let files = snapshot
            .relevant_files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    (file.status.as_str(), file.content_hash.is_some()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(files["mixed.txt"].0, "modified");
        assert!(files["mixed.txt"].1);
        assert_eq!(files["rename-new.txt"].0, "renamed");
        assert_eq!(files["delete.txt"], ("deleted", false));
        assert_eq!(files["binary.bin"].0, "modified");
        assert!(files["binary.bin"].1);
        assert_eq!(files["mixed-link"], ("added", false));
        assert_eq!(files["你好.rs"].0, "added");
        assert!(snapshot.change_summary.diff_stat.contains("mixed.txt"));
        assert_ne!(snapshot.change_summary.base, "unborn");
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn covers_clean_stage_modes_ignored_exec_detached_worktree_and_path_edges() {
        use std::os::unix::fs::PermissionsExt;

        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-more-matrix");
        let linked_worktree = temporary_workspace("git-linked-worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        run_git(&provider, &workspace, &["config", "core.filemode", "true"]);

        let unborn = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(unborn.change_summary.base, "unborn");
        assert_eq!(unborn.change_summary.diff_stat, "clean");
        assert!(unborn.relevant_files.is_empty());
        let unborn_git = unborn.git_snapshot.as_ref().unwrap();
        assert_eq!(unborn_git.status_format, "porcelain_v2");
        assert!(unborn_git.head.is_none());
        assert!(!unborn_git.detached);
        assert!(unborn_git.branch.is_some());

        std::fs::write(workspace.join(".gitignore"), b"ignored.log\n").unwrap();
        std::fs::write(workspace.join("script.sh"), b"#!/bin/sh\necho base\n").unwrap();
        run_git(&provider, &workspace, &["add", ".gitignore", "script.sh"]);
        run_git(
            &provider,
            &workspace,
            &[
                "-c",
                "user.name=dogcodes Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        );

        let clean = provider.collect(workspace.to_str().unwrap()).await;
        assert_ne!(clean.change_summary.base, "unborn");
        assert_eq!(clean.change_summary.diff_stat, "clean");
        assert!(clean.relevant_files.is_empty());
        let clean_git = clean.git_snapshot.as_ref().unwrap();
        assert_eq!(
            clean_git.head.as_deref(),
            Some(clean.change_summary.base.as_str())
        );
        assert!(!clean_git.detached);
        assert_eq!(
            Path::new(&clean_git.repo_root).canonicalize().unwrap(),
            workspace.canonicalize().unwrap()
        );

        std::fs::write(workspace.join("ignored.log"), b"ignored\n").unwrap();
        assert!(
            provider
                .collect(workspace.to_str().unwrap())
                .await
                .relevant_files
                .is_empty()
        );

        std::fs::write(workspace.join("script.sh"), b"#!/bin/sh\necho staged\n").unwrap();
        run_git(&provider, &workspace, &["add", "script.sh"]);
        let staged = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(staged.relevant_files[0].status, "modified");
        assert!(staged.change_summary.diff_stat.contains("script.sh"));

        run_git(
            &provider,
            &workspace,
            &["reset", "--hard", "--quiet", "HEAD"],
        );
        std::fs::write(workspace.join("script.sh"), b"#!/bin/sh\necho unstaged\n").unwrap();
        let unstaged = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(unstaged.relevant_files[0].status, "modified");
        assert_ne!(staged.change_summary.head, unstaged.change_summary.head);

        run_git(
            &provider,
            &workspace,
            &["reset", "--hard", "--quiet", "HEAD"],
        );
        let script = workspace.join("script.sh");
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        let executable = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(executable.relevant_files[0].status, "modified");
        assert!(executable.relevant_files[0].content_hash.is_some());
        assert_ne!(clean.change_summary.head, executable.change_summary.head);

        run_git(
            &provider,
            &workspace,
            &["reset", "--hard", "--quiet", "HEAD"],
        );
        run_git(
            &provider,
            &workspace,
            &["checkout", "--detach", "--quiet", "HEAD"],
        );
        let detached = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(detached.change_summary.base, clean.change_summary.base);
        assert_eq!(detached.change_summary.diff_stat, "clean");
        let detached_git = detached.git_snapshot.as_ref().unwrap();
        assert!(detached_git.detached);
        assert!(detached_git.branch.is_none());

        run_git(
            &provider,
            &workspace,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked_worktree.to_str().unwrap(),
                "HEAD",
            ],
        );
        let linked = provider.collect(linked_worktree.to_str().unwrap()).await;
        assert_eq!(linked.change_summary.base, clean.change_summary.base);
        assert_eq!(linked.change_summary.diff_stat, "clean");
        let linked_git = linked.git_snapshot.as_ref().unwrap();
        assert!(linked_git.detached);
        assert_ne!(linked_git.worktree_identity, detached_git.worktree_identity);

        std::fs::write(workspace.join("space name.txt"), b"space\n").unwrap();
        std::fs::write(workspace.join("line\nbreak.txt"), b"newline\n").unwrap();
        let path_edges = provider.collect(workspace.to_str().unwrap()).await;
        assert!(
            path_edges
                .relevant_files
                .iter()
                .any(|file| file.path == "space name.txt")
        );
        assert!(
            path_edges
                .relevant_files
                .iter()
                .any(|file| file.path == "line\nbreak.txt")
        );

        run_git(
            &provider,
            &workspace,
            &[
                "worktree",
                "remove",
                "--force",
                linked_worktree.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn captures_a_full_binary_diff_without_mutating_the_real_index() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-full-diff");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("staged.txt"), b"base staged\n").unwrap();
        std::fs::write(workspace.join("unstaged.txt"), b"base unstaged\n").unwrap();
        std::fs::write(workspace.join("binary.bin"), [0, 1, 2, 3]).unwrap();
        run_git(&provider, &workspace, &["add", "."]);
        run_git(
            &provider,
            &workspace,
            &[
                "-c",
                "user.name=dogcodes Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        );

        std::fs::write(workspace.join("staged.txt"), b"captured staged\n").unwrap();
        run_git(&provider, &workspace, &["add", "staged.txt"]);
        std::fs::write(workspace.join("unstaged.txt"), b"captured unstaged\n").unwrap();
        std::fs::write(workspace.join("untracked.txt"), b"captured untracked\n").unwrap();
        std::fs::write(workspace.join("binary.bin"), [0, 159, 146, 150]).unwrap();

        let index_path = workspace.join(".git/index");
        let index_before = std::fs::read(&index_path).unwrap();
        let object_count_before = count_files(&workspace.join(".git/objects"));
        let status_before = provider
            .git(
                &workspace,
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "-z",
                    "--untracked-files=all",
                ],
            )
            .await
            .unwrap();
        let captured = provider
            .capture_diff(workspace.to_str().unwrap())
            .await
            .unwrap();
        let patch = String::from_utf8(captured.content).unwrap();

        assert!(patch.contains("diff --git a/staged.txt b/staged.txt"));
        assert!(patch.contains("+captured staged"));
        assert!(patch.contains("diff --git a/unstaged.txt b/unstaged.txt"));
        assert!(patch.contains("+captured unstaged"));
        assert!(patch.contains("diff --git a/untracked.txt b/untracked.txt"));
        assert!(patch.contains("+captured untracked"));
        assert!(patch.contains("diff --git a/binary.bin b/binary.bin"));
        assert!(patch.contains("GIT binary patch"));
        assert_eq!(std::fs::read(&index_path).unwrap(), index_before);
        assert_eq!(
            count_files(&workspace.join(".git/objects")),
            object_count_before
        );
        assert_eq!(
            provider
                .git(
                    &workspace,
                    &[
                        "status",
                        "--porcelain=v2",
                        "--branch",
                        "-z",
                        "--untracked-files=all",
                    ],
                )
                .await
                .unwrap(),
            status_before
        );
        assert_eq!(
            captured.status_fingerprint,
            provider
                .collect(workspace.to_str().unwrap())
                .await
                .git_snapshot
                .unwrap()
                .status_fingerprint
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn configured_filters_that_no_attribute_names_do_not_block_collection() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-filter-unused");
        std::fs::create_dir_all(workspace.join("sub")).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        // Like `git lfs install`: drivers configured, but this repository never uses them.
        for driver in ["clean", "smudge", "process"] {
            run_git(
                &provider,
                &workspace,
                &[
                    "config",
                    &format!("filter.lfs.{driver}"),
                    "printf invoked > filter-ran",
                ],
            );
        }
        std::fs::write(workspace.join(".gitattributes"), "*.bin binary\n").unwrap();
        std::fs::write(workspace.join("content.txt"), "untracked content\n").unwrap();
        let snapshot = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(snapshot.diagnostic, None);
        assert!(snapshot.git_snapshot.is_some());
        assert!(!workspace.join("filter-ran").exists());

        // A nested attributes file, or a macro, that names the driver brings the guard back.
        let expected = Some(WorkspaceCollectionDiagnostic {
            kind: WorkspaceDiagnosticKind::ExternalFilterConfigured,
        });
        std::fs::write(workspace.join("sub/.gitattributes"), "*.psd filter=lfs\n").unwrap();
        assert_eq!(
            provider
                .collect(workspace.to_str().unwrap())
                .await
                .diagnostic,
            expected
        );
        std::fs::remove_file(workspace.join("sub/.gitattributes")).unwrap();
        std::fs::write(
            workspace.join(".gitattributes"),
            "[attr]big filter=lfs\n*.psd big\n",
        )
        .unwrap();
        assert_eq!(
            provider
                .collect(workspace.to_str().unwrap())
                .await
                .diagnostic,
            expected
        );
        assert!(!workspace.join("filter-ran").exists());

        // An unreadable index in a repository without commits: the worktree still
        // shows which attribute files exist.
        std::fs::write(workspace.join(".gitattributes"), "*.bin binary\n").unwrap();
        std::fs::write(workspace.join(".git/index"), b"not an index").unwrap();
        let configured = b"filter.lfs.clean\0";
        assert!(
            !provider
                .filter_attributes_in_effect(&workspace, configured, &BTreeMap::new())
                .await
                .unwrap()
        );
        std::fs::write(workspace.join("sub/.gitattributes"), "*.psd filter=lfs\n").unwrap();
        assert!(
            provider
                .filter_attributes_in_effect(&workspace, configured, &BTreeMap::new())
                .await
                .unwrap()
        );
        // With a committed tree and a broken recovery index, ls-tree is the
        // fallback. Its pathspec dialect does not include :(glob).
        std::fs::remove_file(workspace.join("sub/.gitattributes")).unwrap();
        std::fs::remove_file(workspace.join(".git/index")).unwrap();
        run_git(&provider, &workspace, &["add", "."]);
        run_git(
            &provider,
            &workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "attributes",
            ],
        );
        std::fs::write(workspace.join(".git/index"), b"not an index").unwrap();
        assert!(
            !provider
                .filter_attributes_in_effect(&workspace, configured, &BTreeMap::new())
                .await
                .unwrap()
        );
        // A committed attributes file that is absent in the working tree must
        // still make the guard conservative with an unreadable index.
        std::fs::remove_file(workspace.join(".gitattributes")).unwrap();
        assert!(
            provider
                .filter_attributes_in_effect(&workspace, configured, &BTreeMap::new())
                .await
                .unwrap()
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn rejects_clean_and_process_filters_including_included_configuration() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        for (driver, included) in [("clean", false), ("process", false), ("clean", true)] {
            let workspace = temporary_workspace("git-filter-guard");
            std::fs::create_dir_all(&workspace).unwrap();
            run_git(&provider, &workspace, &["init", "--quiet"]);
            std::fs::write(workspace.join(".gitattributes"), "*.txt filter=fixture\n").unwrap();
            std::fs::write(workspace.join("content.txt"), "untracked content\n").unwrap();
            if included {
                std::fs::write(
                    workspace.join(".git/fixture-config"),
                    "[filter \"fixture\"]\nclean = printf invoked > filter-ran\n",
                )
                .unwrap();
                run_git(
                    &provider,
                    &workspace,
                    &["config", "include.path", "fixture-config"],
                );
            } else {
                run_git(
                    &provider,
                    &workspace,
                    &[
                        "config",
                        &format!("filter.fixture.{driver}"),
                        "printf invoked > filter-ran",
                    ],
                );
            }
            let object_count = count_files(&workspace.join(".git/objects"));
            let expected = WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::ExternalFilterConfigured,
            };
            let snapshot = provider.collect(workspace.to_str().unwrap()).await;
            assert_eq!(snapshot.diagnostic, Some(expected.clone()));
            assert!(snapshot.git_snapshot.is_none());
            assert!(snapshot.relevant_files.is_empty());
            assert_eq!(
                provider.capture_diff(workspace.to_str().unwrap()).await,
                Err(expected)
            );
            assert!(!workspace.join("filter-ran").exists());
            assert!(!workspace.join(".git/index").exists());
            assert_eq!(count_files(&workspace.join(".git/objects")), object_count);
            std::fs::remove_dir_all(&workspace).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn does_not_execute_fsmonitor_hooks_or_textconv_when_collecting_or_capturing() {
        use std::os::unix::fs::PermissionsExt;

        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-external-guard");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join(".gitattributes"), "*.txt diff=fixture\n").unwrap();
        std::fs::write(workspace.join("content.txt"), "base\n").unwrap();
        run_git(&provider, &workspace, &["add", "."]);
        for (config, marker) in [
            ("core.fsmonitor", "fsmonitor-ran"),
            ("diff.fixture.textconv", "textconv-ran"),
            ("diff.external", "external-diff-ran"),
        ] {
            run_git(
                &provider,
                &workspace,
                &["config", config, &format!("printf invoked > {marker}")],
            );
        }
        let hook = workspace.join(".git/hooks/post-index-change");
        std::fs::write(&hook, "#!/bin/sh\nprintf invoked > hook-ran\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let index_before = std::fs::read(workspace.join(".git/index")).unwrap();
        let object_count = count_files(&workspace.join(".git/objects"));
        std::fs::write(workspace.join("content.txt"), "changed\n").unwrap();
        let snapshot = provider.collect(workspace.to_str().unwrap()).await;
        assert!(snapshot.diagnostic.is_none(), "{snapshot:?}");
        let capture = provider
            .capture_diff(workspace.to_str().unwrap())
            .await
            .unwrap();
        assert!(
            String::from_utf8(capture.content)
                .unwrap()
                .contains("+changed")
        );
        for marker in [
            "fsmonitor-ran",
            "textconv-ran",
            "external-diff-ran",
            "hook-ran",
        ] {
            assert!(!workspace.join(marker).exists(), "{marker} was executed");
        }
        assert_eq!(
            std::fs::read(workspace.join(".git/index")).unwrap(),
            index_before
        );
        assert_eq!(count_files(&workspace.join(".git/objects")), object_count);
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn preserves_non_utf8_patch_content_without_replacement_characters() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-lossless-patch");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        let content = b"non-utf8: \xff\xfe\n";
        std::fs::write(workspace.join("legacy.txt"), content).unwrap();
        let capture = provider
            .capture_diff(workspace.to_str().unwrap())
            .await
            .unwrap();
        assert!(
            capture
                .content
                .windows(content.len())
                .any(|value| value == content)
        );
        assert!(
            !capture
                .content
                .windows(3)
                .any(|value| value == [0xef, 0xbf, 0xbd])
        );
        assert!(String::from_utf8(capture.content).is_err());
        assert!(!workspace.join(".git/index").exists());
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn rejects_non_utf8_text_output_without_silently_rewriting_it() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-non-utf8-text");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("legacy.txt"), b"data-\xff\n").unwrap();
        run_git(&provider, &workspace, &["add", "legacy.txt"]);
        assert_eq!(
            provider.git(&workspace, &["show", ":legacy.txt"]).await,
            Err(GitCollectionError::InvalidOutput)
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn keeps_subdirectory_status_hashes_and_patch_in_the_same_scope() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-subdirectory");
        let subdirectory = workspace.join("project");
        std::fs::create_dir_all(&subdirectory).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(workspace.join("outside.txt"), "out of scope\n").unwrap();
        std::fs::write(subdirectory.join("inside.txt"), "before\n").unwrap();
        let snapshot = provider.collect(subdirectory.to_str().unwrap()).await;
        assert!(snapshot.diagnostic.is_none(), "{snapshot:?}");
        assert_eq!(snapshot.relevant_files.len(), 1);
        assert_eq!(snapshot.relevant_files[0].path, "project/inside.txt");
        assert!(snapshot.relevant_files[0].content_hash.is_some());
        std::fs::write(subdirectory.join("inside.txt"), "after!\n").unwrap();
        let changed = provider.collect(subdirectory.to_str().unwrap()).await;
        assert_ne!(snapshot.change_summary.head, changed.change_summary.head);
        let capture = provider
            .capture_diff(subdirectory.to_str().unwrap())
            .await
            .unwrap();
        let patch = String::from_utf8(capture.content).unwrap();
        assert!(patch.contains("project/inside.txt"));
        assert!(patch.contains("+after!"));
        assert!(!patch.contains("outside.txt"));
        assert_eq!(
            capture.status_fingerprint,
            changed.git_snapshot.unwrap().status_fingerprint
        );
        assert!(!workspace.join(".git/index").exists());
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn rejects_an_oversized_diff_without_creating_a_real_index() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-diff-overflow");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        std::fs::write(
            workspace.join("too-large.txt"),
            vec![b'a'; MAX_DIFF_BYTES + 32 * 1024],
        )
        .unwrap();

        assert_eq!(
            provider.capture_diff(workspace.to_str().unwrap()).await,
            Err(WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::OutputTruncated,
            })
        );
        assert!(!workspace.join(".git/index").exists());
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[tokio::test]
    async fn degrades_to_unknown_when_the_changed_path_budget_is_exceeded() {
        let Some(provider) = GitWorkspaceContextProvider::discover() else {
            return;
        };
        let workspace = temporary_workspace("git-overflow");
        std::fs::create_dir_all(&workspace).unwrap();
        run_git(&provider, &workspace, &["init", "--quiet"]);
        for index in 0..=MAX_RELEVANT_FILES {
            std::fs::write(workspace.join(format!("file-{index:04}.txt")), []).unwrap();
        }

        let snapshot = provider.collect(workspace.to_str().unwrap()).await;
        assert_eq!(snapshot.relevant_files, vec![]);
        assert_eq!(
            snapshot.change_summary,
            WorkspaceContextSnapshot::default().change_summary
        );
        assert_eq!(
            snapshot.diagnostic.unwrap().kind,
            WorkspaceDiagnosticKind::PathBudgetExceeded
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }
}
