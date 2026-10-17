//! Lightweight worktree identity and status; no diff artifacts or index writes.
use super::{GitReview, ReviewResult};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct WorktreeIdentity {
    pub root: PathBuf,
    pub common: PathBuf,
    pub primary: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WorktreeStatus {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub changed_files: usize,
}

impl GitReview {
    pub async fn worktree_identity(&self, path: &Path) -> ReviewResult<Option<WorktreeIdentity>> {
        if crate::is_non_git(path) {
            return Ok(None);
        }
        let root = self.text(path, &["rev-parse", "--show-toplevel"]).await?;
        let common = self
            .text(
                path,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
        let git_dir = self
            .text(path, &["rev-parse", "--absolute-git-dir"])
            .await?;
        let canonical = |value: &str| {
            Path::new(value.trim_end_matches('\n'))
                .canonicalize()
                .map_err(|_| "工作区目录不可用".to_owned())
        };
        let root = canonical(&root)?;
        let common = canonical(&common)?;
        Ok(Some(WorktreeIdentity {
            root,
            primary: canonical(&git_dir)? == common,
            common,
        }))
    }

    pub async fn worktree_status(&self, path: &Path) -> ReviewResult<WorktreeStatus> {
        let bytes = self
            .run(
                path,
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "-z",
                    "--untracked-files=all",
                    "--",
                    ".",
                ],
                4 * 1024 * 1024,
            )
            .await?;
        parse_status(&bytes)
    }
}

fn parse_status(bytes: &[u8]) -> ReviewResult<WorktreeStatus> {
    let mut result = WorktreeStatus::default();
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(branch) = record.strip_prefix(b"# branch.head ") {
            if branch != b"(detached)" {
                result.branch =
                    Some(String::from_utf8(branch.to_vec()).map_err(|_| "分支名编码无效")?);
            }
        } else if let Some(head) = record.strip_prefix(b"# branch.oid ") {
            if head != b"(initial)" {
                result.head =
                    Some(String::from_utf8(head.to_vec()).map_err(|_| "提交标识编码无效")?);
            }
        } else if matches!(record.first(), Some(b'1' | b'2' | b'u' | b'?')) {
            result.changed_files += 1;
            if record[0] == b'2' {
                records.next().ok_or("重命名状态不完整")?;
            }
        } else if record[0] != b'#' {
            return Err("Git 状态格式无效".into());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(path: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn identifies_linked_worktrees_and_reads_independent_status_without_changing_indexes() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let fork = temp.path().join("fork");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        let review = GitReview::default();
        let initial = review.worktree_status(&main).await.unwrap();
        assert_eq!(initial.branch.as_deref(), Some("main"));
        assert!(initial.head.is_none());
        std::fs::write(main.join("file"), "base").unwrap();
        git(&main, &["add", "."]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "base",
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "codex/fork",
                fork.to_str().unwrap(),
            ],
        );
        let source = review.worktree_identity(&main).await.unwrap().unwrap();
        let linked = review.worktree_identity(&fork).await.unwrap().unwrap();
        assert_eq!(source.common, linked.common);
        assert_ne!(source.root, linked.root);
        assert!(source.primary);
        assert!(!linked.primary);
        std::fs::write(main.join("file"), "source change").unwrap();
        git(&fork, &["mv", "file", "renamed"]);
        std::fs::write(fork.join("line\nname"), "new").unwrap();
        let main_index = main.join(".git/index");
        let fork_index = source.common.join("worktrees/fork/index");
        let before = (
            std::fs::read(&main_index).unwrap(),
            std::fs::read(&fork_index).unwrap(),
        );
        assert_eq!(
            review.worktree_status(&main).await.unwrap().changed_files,
            1
        );
        let status = review.worktree_status(&fork).await.unwrap();
        assert_eq!(status.branch.as_deref(), Some("codex/fork"));
        assert_eq!(status.changed_files, 2);
        assert_eq!(
            before,
            (
                std::fs::read(main_index).unwrap(),
                std::fs::read(fork_index).unwrap()
            )
        );
        git(&fork, &["checkout", "--detach"]);
        let detached = review.worktree_status(&fork).await.unwrap();
        assert!(detached.branch.is_none());
        assert!(detached.head.is_some());
        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert!(review.worktree_identity(&plain).await.unwrap().is_none());
    }

    #[test]
    fn counts_files_once_including_rename_conflicts_and_newlines() {
        let value = parse_status(b"# branch.oid abc\0# branch.head feature/demo\0# branch.ab +1 -2\x001 MM file\x002 R. new\0old\0? line\nname\0u UU conflict\0").unwrap();
        assert_eq!(value.branch.as_deref(), Some("feature/demo"));
        assert_eq!(value.changed_files, 4);
        assert!(parse_status(b"2 R. new\0").is_err());
        let value = parse_status(b"# branch.oid (initial)\0# branch.head main\0").unwrap();
        assert!(value.head.is_none());
        assert_eq!(
            parse_status(b"# branch.head (detached)\0").unwrap().branch,
            None
        );
    }
}
