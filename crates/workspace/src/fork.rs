//! Durable turn-end trees. Private refs keep objects alive without moving the user's branch.
use super::{GIT_OUTPUT_LIMIT, GitWorkspaceContextProvider, TemporaryGitIndex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepositorySnapshot {
    repository_root: PathBuf,
    common_directory: PathBuf,
    commit: String,
    base_commit: Option<String>,
    index_tree: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RootSnapshot {
    repository: RepositorySnapshot,
    relative_path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForkSnapshot {
    schema_version: u32,
    roots: Vec<RootSnapshot>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckoutProgress {
    snapshot: RepositorySnapshot,
    stage: String,
}

fn save_marker(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let directory = path.parent().ok_or("Fork 标记目录无效")?;
    std::fs::create_dir_all(directory).map_err(|_| "无法创建 Fork 恢复目录")?;
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法保存 Fork 恢复记录")?;
    serde_json::to_writer(&mut file, value).map_err(|_| "无法编码 Fork 恢复记录")?;
    file.as_file_mut()
        .sync_all()
        .map_err(|_| "无法持久保存 Fork 恢复记录")?;
    file.persist(path).map_err(|_| "无法发布 Fork 恢复记录")?;
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Fork 恢复记录保存结果待核实")?;
    Ok(())
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

impl GitWorkspaceContextProvider {
    async fn fork_git(&self, workspace: &Path, args: &[&str]) -> Result<String, String> {
        self.git(workspace, args)
            .await
            .map_err(|_| "读取或保存 Git 快照失败（请检查仓库、外部过滤器及目录权限）".into())
    }

    async fn common_directory(&self, root: &Path) -> Result<PathBuf, String> {
        let common = self
            .fork_git(
                root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
        Path::new(common.trim_end_matches('\n'))
            .canonicalize()
            .map_err(|_| "Git 仓库目录不可用".into())
    }

    async fn snapshot_head(&self, root: &Path) -> Result<Option<String>, String> {
        if let Ok(head) = self
            .git(root, &["rev-parse", "--verify", "HEAD^{commit}"])
            .await
        {
            return Ok(Some(head.trim().to_owned()));
        }
        // Only an absent branch is unborn. A broken HEAD must not discard ancestry.
        let branch = self.fork_git(root, &["symbolic-ref", "HEAD"]).await?;
        let result = self
            .git_process(
                root,
                &["show-ref", "--verify", "--quiet", branch.trim()],
                BTreeMap::new(),
                GIT_OUTPUT_LIMIT,
            )
            .await
            .map_err(|_| "无法读取本轮提交基点")?;
        if result.exit_code == Some(1) {
            Ok(None)
        } else {
            Err("本轮 Git HEAD 不可用".into())
        }
    }

    pub(super) async fn save_fork_snapshot(&self, paths: &[String]) -> Result<Value, String> {
        if paths.is_empty() {
            return Err("没有可保存的项目目录".into());
        }
        let mut repositories = BTreeMap::<PathBuf, RepositorySnapshot>::new();
        let mut roots = Vec::new();
        for path in paths {
            let source = Path::new(path)
                .canonicalize()
                .map_err(|_| "项目目录不可用")?;
            let root = self
                .fork_git(&source, &["rev-parse", "--show-toplevel"])
                .await
                .map_err(|_| "新 worktree 需要项目及附加目录均属于 Git 仓库")?;
            let root = Path::new(root.trim_end_matches('\n'))
                .canonicalize()
                .map_err(|_| "Git 根目录不可用")?;
            let relative_path = source
                .strip_prefix(&root)
                .map_err(|_| "项目目录不在 Git 工作区内")?
                .to_owned();
            let repository = if let Some(saved) = repositories.get(&root) {
                saved.clone()
            } else {
                let saved = self.save_repository_snapshot(&root).await?;
                repositories.insert(root, saved.clone());
                saved
            };
            roots.push(RootSnapshot {
                repository,
                relative_path,
            });
        }
        serde_json::to_value(ForkSnapshot {
            schema_version: 1,
            roots,
        })
        .map_err(|_| "快照编码失败".into())
    }

    async fn save_repository_snapshot(&self, root: &Path) -> Result<RepositorySnapshot, String> {
        let common_directory = self.common_directory(root).await?;
        let temporary = TemporaryGitIndex::new().map_err(|_| "无法创建快照临时索引")?;
        // The index is private; objects go into the repository and are pinned by a private ref.
        let environment = BTreeMap::from([
            (
                "GIT_INDEX_FILE".into(),
                temporary.index_path.as_os_str().to_owned(),
            ),
            ("GIT_LITERAL_PATHSPECS".into(), OsString::from("1")),
            ("GIT_AUTHOR_NAME".into(), OsString::from("dog.codes")),
            (
                "GIT_AUTHOR_EMAIL".into(),
                OsString::from("snapshot@dogcodes.local"),
            ),
            ("GIT_COMMITTER_NAME".into(), OsString::from("dog.codes")),
            (
                "GIT_COMMITTER_EMAIL".into(),
                OsString::from("snapshot@dogcodes.local"),
            ),
            // Content-addressed deduplication, independent of the user's identity and clock.
            (
                "GIT_AUTHOR_DATE".into(),
                OsString::from("2000-01-01T00:00:00Z"),
            ),
            (
                "GIT_COMMITTER_DATE".into(),
                OsString::from("2000-01-01T00:00:00Z"),
            ),
        ]);
        let base_commit = self.snapshot_head(root).await?;
        let command = |args: Vec<String>| {
            let environment = environment.clone();
            async move {
                let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                self.git_with_options(root, &args, environment, GIT_OUTPUT_LIMIT)
                    .await
                    .map_err(|_| "无法保存完整 Git 文件快照".to_owned())
            }
        };
        command(vec!["read-tree".into(), "--empty".into()]).await?;
        // Include newly staged files even when they match ignore rules. Reading the
        // real index is safe; only the private index receives these entries.
        let staged = self
            .git_bytes_with_options(
                root,
                &["ls-files", "--stage", "-z"],
                BTreeMap::new(),
                4 * 1024 * 1024,
            )
            .await
            .map_err(|_| "无法读取本轮暂存文件")?;
        let indexed = self
            .git_process_input(
                root,
                &["update-index", "-z", "--index-info"],
                environment.clone(),
                GIT_OUTPUT_LIMIT,
                Some(&staged),
            )
            .await
            .map_err(|_| "无法保存本轮暂存文件")?;
        if !indexed.success {
            return Err("无法保存本轮暂存文件".into());
        }
        let index_tree = command(vec!["write-tree".into()]).await?.trim().to_owned();
        self.stage_snapshot_files(root, &environment).await?;
        let tree = command(vec!["write-tree".into()]).await?.trim().to_owned();
        // Gitlinks do not contain nested worktree files; fail explicitly instead of claiming fidelity.
        let listing = self.fork_git(root, &["ls-tree", "-r", "-z", &tree]).await?;
        if listing
            .split('\0')
            .any(|entry| entry.starts_with("160000 "))
        {
            return Err("本轮含有 Git 子模块，尚不能完整恢复子模块文件快照".into());
        }
        // A second staging pass detects edits during capture without relying on bounded diff bytes.
        self.stage_snapshot_files(root, &environment).await?;
        let check_tree = command(vec!["write-tree".into()]).await?;
        let check_base = self.snapshot_head(root).await?;
        let check_staged = self
            .git_bytes_with_options(
                root,
                &["ls-files", "--stage", "-z"],
                BTreeMap::new(),
                4 * 1024 * 1024,
            )
            .await
            .map_err(|_| "无法校验本轮暂存文件")?;
        if tree != check_tree.trim() || base_commit != check_base || staged != check_staged {
            return Err("保存轮末快照时文件仍在变化，未保存不一致的快照".into());
        }
        let mut args = vec![
            "commit-tree".into(),
            tree,
            "-m".into(),
            "dog.codes turn snapshot".into(),
        ];
        if let Some(base) = &base_commit {
            args.extend(["-p".into(), base.clone()]);
        }
        let commit = command(args).await?.trim().to_owned();
        if !oid(&commit) {
            return Err("Git 快照提交无效".into());
        }
        self.fork_git(
            root,
            &[
                "update-ref",
                &format!("refs/dogcodes/turn-snapshots/{commit}"),
                &commit,
            ],
        )
        .await?;
        self.fork_git(
            root,
            &[
                "update-ref",
                &format!("refs/dogcodes/turn-indexes/{index_tree}"),
                &index_tree,
            ],
        )
        .await?;
        Ok(RepositorySnapshot {
            repository_root: root.to_owned(),
            common_directory,
            commit,
            base_commit,
            index_tree,
        })
    }

    async fn stage_snapshot_files(
        &self,
        root: &Path,
        environment: &BTreeMap<String, OsString>,
    ) -> Result<(), String> {
        // Update tracked files first; this also checks for external content filters.
        self.git_with_options(root, &["add", "-u"], environment.clone(), GIT_OUTPUT_LIMIT)
            .await
            .map_err(|_| "无法保存本轮已跟踪文件")?;
        // Enumeration uses the user's global excludes, while content commands stay
        // isolated from global executable filters. Do not accidentally snapshot ignored files.
        let untracked = self
            .git_bytes_with_options(
                root,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                BTreeMap::new(),
                4 * 1024 * 1024,
            )
            .await
            .map_err(|_| "无法读取本轮新增文件")?;
        if !untracked.is_empty() {
            let output = self
                .git_process_input(
                    root,
                    &["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
                    environment.clone(),
                    GIT_OUTPUT_LIMIT,
                    Some(&untracked),
                )
                .await
                .map_err(|_| "无法保存本轮新增文件")?;
            if !output.success {
                return Err("无法保存本轮新增文件".into());
            }
        }
        Ok(())
    }

    pub(super) async fn restore_fork_snapshot(
        &self,
        snapshot: Value,
        name: &str,
    ) -> Result<Vec<String>, String> {
        agent_domain::TaskId::parse(name).map_err(|_| "无效的 fork 标识")?;
        let snapshot: ForkSnapshot =
            serde_json::from_value(snapshot).map_err(|_| "轮末文件快照格式无效")?;
        if snapshot.schema_version != 1 || snapshot.roots.is_empty() {
            return Err("轮末文件快照版本无效".into());
        }
        // Validate every root before creating any checkout. Never share a live additional directory.
        let mut repositories = BTreeMap::<PathBuf, RepositorySnapshot>::new();
        for root in &snapshot.roots {
            let repo = &root.repository;
            if root
                .relative_path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || !repo.repository_root.is_absolute()
                || !repo.common_directory.is_absolute()
                || !oid(&repo.commit)
                || !oid(&repo.index_tree)
                || repo.base_commit.as_ref().is_some_and(|base| !oid(base))
            {
                return Err("轮末文件快照路径或对象无效".into());
            }
            // Do not follow a stored symlink back into the original workspace.
            let mut prefix = PathBuf::new();
            for component in root.relative_path.components() {
                prefix.push(component);
                let spec = format!(
                    "{}:{}",
                    repo.commit,
                    prefix.to_string_lossy().replace('\\', "/")
                );
                if let Ok(kind) = self
                    .git(&repo.repository_root, &["cat-file", "-t", &spec])
                    .await
                    && kind.trim() != "tree"
                {
                    return Err("快照中的项目目录已变成文件或符号链接".into());
                }
            }
            if let Some(previous) = repositories.get(&repo.repository_root) {
                if previous != repo {
                    return Err("轮末文件快照中的仓库状态不一致".into());
                }
                continue;
            }
            if self.common_directory(&repo.repository_root).await? != repo.common_directory {
                return Err("源仓库位置或身份已变化，无法恢复本轮快照".into());
            }
            let pinned = self
                .fork_git(
                    &repo.repository_root,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("refs/dogcodes/turn-snapshots/{}^{{commit}}", repo.commit),
                    ],
                )
                .await
                .map_err(|_| "本轮的 Git 快照对象已不可用")?;
            if pinned.trim() != repo.commit {
                return Err("本轮 Git 快照校验失败".into());
            }
            let index = self
                .fork_git(
                    &repo.repository_root,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("refs/dogcodes/turn-indexes/{}^{{tree}}", repo.index_tree),
                    ],
                )
                .await
                .map_err(|_| "本轮的 Git 暂存快照已不可用")?;
            if index.trim() != repo.index_tree {
                return Err("本轮 Git 暂存快照校验失败".into());
            }
            let parents = self
                .fork_git(
                    &repo.repository_root,
                    &["rev-list", "--parents", "-n", "1", &repo.commit],
                )
                .await?;
            let expected = match &repo.base_commit {
                Some(base) => format!("{} {base}", repo.commit),
                None => repo.commit.clone(),
            };
            if parents.trim() != expected {
                return Err("本轮 Git 快照基点校验失败".into());
            }
            repositories.insert(repo.repository_root.clone(), repo.clone());
        }
        let mut destinations = BTreeMap::new();
        // Keep the primary checkout's branch name stable even when extra roots sort earlier.
        for root in &snapshot.roots {
            let repo = &root.repository;
            if destinations.contains_key(&repo.repository_root) {
                continue;
            }
            let suffix = if destinations.is_empty() {
                name.to_owned()
            } else {
                format!("{name}-{}", destinations.len() + 1)
            };
            let destination = self
                .restore_repository_snapshot(repo, &suffix)
                .await
                .map_err(|error| {
                    format!(
                        "{error}；未完成的 Fork：{suffix}，此前已恢复 {} 个仓库，现有目录均已保留",
                        destinations.len()
                    )
                })?;
            destinations.insert(repo.repository_root.clone(), destination);
        }
        snapshot
            .roots
            .iter()
            .map(|root| {
                let path = destinations[&root.repository.repository_root].join(&root.relative_path);
                // Empty project directories are not represented by Git trees.
                std::fs::create_dir_all(&path).map_err(|_| "无法恢复项目目录".to_owned())?;
                Ok(path.to_string_lossy().into_owned())
            })
            .collect()
    }

    async fn restore_repository_snapshot(
        &self,
        snapshot: &RepositorySnapshot,
        name: &str,
    ) -> Result<PathBuf, String> {
        let source = &snapshot.repository_root;
        let destination = snapshot
            .common_directory
            .join("dogcodes-worktrees")
            .join(name);
        let branch = format!("codex/fork-{name}");
        let branch_ref = format!("refs/heads/{branch}");
        let marker_directory = snapshot.common_directory.join("dogcodes-fork-checkouts");
        let marker = marker_directory.join(format!("{name}.json"));
        let progress_file = marker_directory.join(format!("{name}.progress.json"));
        let progress = match std::fs::read(&progress_file) {
            Ok(bytes) => Some(
                serde_json::from_slice::<CheckoutProgress>(&bytes)
                    .map_err(|_| "Fork 恢复阶段记录损坏，未修改目录")?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取 Fork 恢复阶段，未修改目录".into()),
        };
        if progress.as_ref().is_some_and(|record| {
            record.snapshot != *snapshot
                || !matches!(record.stage.as_str(), "creating" | "head_restored")
        }) {
            return Err("Fork 恢复计划不匹配，未修改目录".into());
        }
        if destination.exists() && marker.exists() {
            let recorded: RepositorySnapshot = std::fs::read(&marker)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .ok_or("Fork 完成记录损坏，未修改目录")?;
            if recorded != *snapshot
                || self.common_directory(&destination).await? != snapshot.common_directory
                || self
                    .fork_git(&destination, &["symbolic-ref", "HEAD"])
                    .await?
                    .trim()
                    != branch_ref
            {
                return Err("fork 目录已被其他工作区使用".into());
            }
            // A completed checkout may contain new user work. Never reset it.
            return Ok(destination);
        }
        if destination.exists() && progress.is_none() {
            return Err("fork 目录已存在但没有可核实的恢复计划，请保留目录并人工核实".into());
        }
        if !destination.exists() {
            if marker.exists() || progress.as_ref().is_some_and(|p| p.stage != "creating") {
                return Err("Fork 目录已被移动或删除，未重新创建".into());
            }
            // A branch without its checkout is ambiguous; never delete or adopt it.
            if self
                .git(source, &["show-ref", "--verify", &branch_ref])
                .await
                .is_ok()
            {
                return Err("Fork 分支已存在但目录不可用，请核实原 Worktree，未重新创建".into());
            }
            save_marker(
                &progress_file,
                &CheckoutProgress {
                    snapshot: snapshot.clone(),
                    stage: "creating".into(),
                },
            )?;
            let path = destination.to_str().ok_or("worktree 路径编码无效")?;
            self.fork_git(
                source,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    "--",
                    path,
                    &snapshot.commit,
                ],
            )
            .await?;
        }
        if self.common_directory(&destination).await? != snapshot.common_directory
            || self
                .fork_git(&destination, &["symbolic-ref", "HEAD"])
                .await?
                .trim()
                != branch_ref
        {
            return Err("Fork 半成品的仓库或分支已改变，未修改目录".into());
        }
        let head = self.snapshot_head(&destination).await?;
        let index = self
            .fork_git(&destination, &["write-tree"])
            .await?
            .trim()
            .to_owned();
        let original_tree = self
            .fork_git(
                source,
                &["rev-parse", &format!("{}^{{tree}}", snapshot.commit)],
            )
            .await?
            .trim()
            .to_owned();
        let base_tree = match &snapshot.base_commit {
            Some(base) => Some(
                self.fork_git(source, &["rev-parse", &format!("{base}^{{tree}}")])
                    .await?
                    .trim()
                    .to_owned(),
            ),
            None => None,
        };
        if head.as_deref() == Some(snapshot.commit.as_str()) {
            if index != original_tree {
                return Err("Fork 半成品的暂存区已被修改，未重置或覆盖".into());
            }
            if let Some(base) = &snapshot.base_commit {
                self.fork_git(&destination, &["reset", "--mixed", base])
                    .await?;
            } else {
                self.fork_git(
                    &destination,
                    &["update-ref", "-d", &branch_ref, &snapshot.commit],
                )
                .await?;
            }
        } else if head != snapshot.base_commit
            || (index != snapshot.index_tree
                && base_tree.as_ref() != Some(&index)
                && !(snapshot.base_commit.is_none() && index == original_tree))
        {
            return Err("Fork 半成品的提交或暂存区已改变，未重置或覆盖".into());
        }
        save_marker(
            &progress_file,
            &CheckoutProgress {
                snapshot: snapshot.clone(),
                stage: "head_restored".into(),
            },
        )?;
        // These commands change only HEAD/index. Working files, including user
        // edits made during an interrupted attempt, are never checked out again.
        self.fork_git(&destination, &["read-tree", &snapshot.index_tree])
            .await?;
        save_marker(&marker, snapshot)?;
        Ok(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::WorkspaceContextProvider;
    use agent_domain::TaskId;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args.iter().map(|arg| crate::git_path_argument(arg)))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    fn init(root: &Path) {
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("tracked.txt"), "baseline\n").unwrap();
        std::fs::write(root.join("deleted.txt"), "deleted\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "base",
            ],
        );
    }
    fn paths(root: &Path) -> Vec<String> {
        vec![root.to_str().unwrap().to_owned()]
    }

    #[tokio::test]
    async fn full_snapshot_restores_large_binary_deletion_rename_and_pending_edits_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        init(root);
        let head = git(root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("tracked.txt"), "staged\n").unwrap();
        git(root, &["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "unstaged\n").unwrap();
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        let binary: Vec<u8> = (0..1_100_000_u32)
            .map(|n| u8::try_from(n % 251).unwrap())
            .collect();
        std::fs::write(root.join("新增.bin"), &binary).unwrap();
        git(root, &["mv", ".gitignore", "renamed-ignore"]);
        // Keep ignore rules active while testing omitted generated files.
        std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(root.join("ignored.log"), "generated").unwrap();
        // A force-added ignored file is still part of the user's result.
        std::fs::write(root.join(".git/info/exclude"), "staged-ignore.txt\n").unwrap();
        std::fs::write(root.join("staged-ignore.txt"), "staged ignored outcome").unwrap();
        git(root, &["add", "--force", "staged-ignore.txt"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            symlink("tracked.txt", root.join("link")).unwrap();
            std::fs::set_permissions(
                root.join("tracked.txt"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let expected_status = git(root, &["status", "--porcelain"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let snapshot = GitWorkspaceContextProvider::default()
            .capture_fork_snapshot(&paths(root))
            .await
            .unwrap();
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
        std::fs::write(root.join("tracked.txt"), "later\n").unwrap();
        std::fs::remove_file(root.join("新增.bin")).unwrap();
        let status = git(root, &["status", "--porcelain"]);
        git(root, &["gc", "--prune=now"]);
        let name = TaskId::new().to_string();
        let provider = GitWorkspaceContextProvider::default();
        let restored = provider
            .fork_worktrees(snapshot.clone(), &name)
            .await
            .unwrap();
        let target = Path::new(&restored[0]);
        assert_eq!(std::fs::read(target.join("新增.bin")).unwrap(), binary);
        assert_eq!(
            std::fs::read_to_string(target.join("tracked.txt")).unwrap(),
            "unstaged\n"
        );
        assert!(!target.join("deleted.txt").exists());
        assert!(!target.join("ignored.log").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("staged-ignore.txt")).unwrap(),
            "staged ignored outcome"
        );
        assert!(target.join("renamed-ignore").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::read_link(target.join("link")).unwrap(),
                Path::new("tracked.txt")
            );
            assert_ne!(
                std::fs::metadata(target.join("tracked.txt"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
        assert_eq!(git(target, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(target, &["status", "--porcelain"]), expected_status);
        assert_eq!(git(root, &["status", "--porcelain"]), status);
        // A retry cannot overwrite subsequent edits in the new worktree.
        std::fs::write(target.join("tracked.txt"), "fork edit\n").unwrap();
        assert_eq!(
            provider.fork_worktrees(snapshot, &name).await.unwrap(),
            restored
        );
        assert_eq!(
            std::fs::read_to_string(target.join("tracked.txt")).unwrap(),
            "fork edit\n"
        );
    }

    #[tokio::test]
    async fn additional_roots_are_restored_and_same_repository_directories_share_one_checkout() {
        let directory = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        init(directory.path());
        init(other.path());
        let subdir = directory.path().join("new-subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("new.txt"), "subdir outcome").unwrap();
        std::fs::write(other.path().join("tracked.txt"), "other outcome").unwrap();
        let selected = vec![
            subdir.to_str().unwrap().to_owned(),
            directory.path().to_str().unwrap().to_owned(),
            other.path().to_str().unwrap().to_owned(),
        ];
        let provider = GitWorkspaceContextProvider::default();
        let snapshot = provider.capture_fork_snapshot(&selected).await.unwrap();
        std::fs::write(subdir.join("new.txt"), "later").unwrap();
        let restored = provider
            .fork_worktrees(snapshot, TaskId::new().as_str())
            .await
            .unwrap();
        assert_eq!(
            Path::new(&restored[0]).parent(),
            Some(Path::new(&restored[1]))
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&restored[0]).join("new.txt")).unwrap(),
            "subdir outcome"
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&restored[2]).join("tracked.txt")).unwrap(),
            "other outcome"
        );
        assert_ne!(Path::new(&restored[2]), other.path());
    }

    #[tokio::test]
    async fn unborn_repository_restores_pending_files_without_creating_a_source_commit() {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(directory.path().join("new.txt"), "first result").unwrap();
        let provider = GitWorkspaceContextProvider::default();
        let snapshot = provider
            .capture_fork_snapshot(&paths(directory.path()))
            .await
            .unwrap();
        let restored = provider
            .fork_worktrees(snapshot, TaskId::new().as_str())
            .await
            .unwrap();
        assert!(git(Path::new(&restored[0]), &["status", "--porcelain"]).contains("?? new.txt"));
        assert_eq!(
            git(directory.path(), &["symbolic-ref", "HEAD"]),
            "refs/heads/main"
        );
        assert!(!directory.path().join(".git/refs/heads/main").exists());
    }

    #[tokio::test]
    async fn missing_snapshot_objects_and_non_git_additional_directories_are_not_silently_replaced()
    {
        let directory = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        init(directory.path());
        let provider = GitWorkspaceContextProvider::default();
        let mut selected = paths(directory.path());
        selected.extend(paths(extra.path()));
        assert!(
            provider
                .capture_fork_snapshot(&selected)
                .await
                .unwrap_err()
                .contains("附加目录")
        );
        let snapshot = provider
            .capture_fork_snapshot(&paths(directory.path()))
            .await
            .unwrap();
        let commit = snapshot["roots"][0]["repository"]["commit"]
            .as_str()
            .unwrap();
        git(
            directory.path(),
            &[
                "update-ref",
                "-d",
                &format!("refs/dogcodes/turn-snapshots/{commit}"),
            ],
        );
        let result = provider
            .fork_worktrees(snapshot, TaskId::new().as_str())
            .await;
        assert!(result.unwrap_err().contains("快照对象已不可用"));
        assert!(!directory.path().join(".git/dogcodes-worktrees").exists());
    }
    #[tokio::test]
    async fn interrupted_checkout_resumes_without_overwriting_working_files() {
        for user_staged in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            init(directory.path());
            std::fs::write(directory.path().join("tracked.txt"), "turn result\n").unwrap();
            let provider = GitWorkspaceContextProvider::default();
            let value = provider
                .capture_fork_snapshot(&paths(directory.path()))
                .await
                .unwrap();
            let snapshot: ForkSnapshot = serde_json::from_value(value.clone()).unwrap();
            let repo = &snapshot.roots[0].repository;
            let name = TaskId::new().to_string();
            let destination = repo.common_directory.join("dogcodes-worktrees").join(&name);
            let marker = repo
                .common_directory
                .join("dogcodes-fork-checkouts")
                .join(format!("{name}.progress.json"));
            save_marker(
                &marker,
                &CheckoutProgress {
                    snapshot: repo.clone(),
                    stage: "creating".into(),
                },
            )
            .unwrap();
            git(
                directory.path(),
                &[
                    "worktree",
                    "add",
                    "-b",
                    &format!("codex/fork-{name}"),
                    "--",
                    destination.to_str().unwrap(),
                    &repo.commit,
                ],
            );
            std::fs::write(destination.join("tracked.txt"), "new user work\n").unwrap();
            if user_staged {
                git(&destination, &["add", "tracked.txt"]);
            }
            let index = git(&destination, &["write-tree"]);
            let result = provider.fork_worktrees(value, &name).await;
            if user_staged {
                assert!(result.unwrap_err().contains("暂存区已被修改"));
                assert_eq!(git(&destination, &["write-tree"]), index);
            } else {
                let restored = result.unwrap();
                assert_eq!(
                    Path::new(&restored[0]).canonicalize().unwrap(),
                    destination.canonicalize().unwrap()
                );
                assert_eq!(git(&destination, &["write-tree"]), repo.index_tree);
                assert_eq!(
                    git(&destination, &["rev-parse", "HEAD"]),
                    repo.base_commit.clone().unwrap()
                );
            }
            assert_eq!(
                std::fs::read_to_string(destination.join("tracked.txt")).unwrap(),
                "new user work\n"
            );
        }
    }

    #[tokio::test]
    async fn multiple_repositories_resume_only_missing_checkouts_and_keep_completed_user_edits() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        init(&first);
        init(&second);
        let provider = GitWorkspaceContextProvider::default();
        let value = provider
            .capture_fork_snapshot(&[
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ])
            .await
            .unwrap();
        let snapshot: ForkSnapshot = serde_json::from_value(value.clone()).unwrap();
        let name = TaskId::new().to_string();
        let collision = format!("codex/fork-{name}-2");
        git(&second, &["branch", &collision]);
        assert!(
            provider
                .fork_worktrees(value.clone(), &name)
                .await
                .unwrap_err()
                .contains("已恢复 1 个仓库")
        );
        let destination = snapshot.roots[0]
            .repository
            .common_directory
            .join("dogcodes-worktrees")
            .join(&name);
        std::fs::write(destination.join("tracked.txt"), "keep my changes\n").unwrap();
        git(&destination, &["add", "tracked.txt"]);
        let index = git(&destination, &["write-tree"]);
        git(&second, &["branch", "-D", &collision]);
        assert_eq!(
            provider.fork_worktrees(value, &name).await.unwrap().len(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("tracked.txt")).unwrap(),
            "keep my changes\n"
        );
        assert_eq!(git(&destination, &["write-tree"]), index);
    }
}
