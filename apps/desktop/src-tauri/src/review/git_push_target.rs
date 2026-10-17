//! Bind a workflow to its own commit and original push destination. Never infer
//! an old workflow's target from the current HEAD or a newly configured remote.
use super::{require_git_success, run_commit_command};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RepositoryTarget {
    worktree: String,
    git_directory: String,
    branch: String,
    remote: String,
    destination_ref: String,
    // URLs can contain credentials. Persist only their ordered fingerprint.
    destinations_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PushTarget {
    repository: RepositoryTarget,
    oid: String,
}
pub(super) struct Observed {
    repository: RepositoryTarget,
    urls: Vec<String>,
}
async fn text(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_commit_command(path, args)
        .await
        .map_err(|_| "无法核实 Git 推送目标")?;
    if !output.status.success() || output.stdout.len() > 256 * 1024 {
        return Err("无法核实 Git 推送目标，请检查分支和上游配置".into());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "Git 目标编码无效".into())
}
async fn config(path: &Path, key: &str) -> Result<Option<String>, String> {
    let out = run_commit_command(path, &["config", "--get-all", key]).await?;
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    if !out.status.success() || out.stdout.len() > 64 * 1024 {
        return Err("Git 上游配置不可读取".into());
    }
    let value = String::from_utf8(out.stdout).map_err(|_| "Git 上游配置编码无效")?;
    let lines: Vec<_> = value.lines().collect();
    if lines.len() != 1 || lines[0].is_empty() {
        return Err("Git 上游配置不唯一，无法绑定接续推送".into());
    }
    Ok(Some(lines[0].to_owned()))
}
pub(super) async fn observe(path: &Path) -> Result<Observed, String> {
    let branch = text(path, &["symbolic-ref", "--quiet", "HEAD"]).await?;
    let name = branch
        .strip_prefix("refs/heads/")
        .ok_or("当前不在分支上，无法绑定接续推送")?;
    let remote = config(path, &format!("branch.{name}.remote"))
        .await?
        .ok_or("当前分支没有上游，提交后请核实推送目标")?;
    let destination_ref = config(path, &format!("branch.{name}.merge"))
        .await?
        .ok_or("当前分支没有上游引用")?;
    if !destination_ref.starts_with("refs/heads/") {
        return Err("上游不是分支引用，无法绑定接续推送".into());
    }
    let override_remote = config(path, &format!("branch.{name}.pushRemote"))
        .await?
        .or(config(path, "remote.pushDefault").await?);
    if override_remote
        .as_ref()
        .is_some_and(|value| value != &remote)
        || config(path, &format!("remote.{remote}.push"))
            .await?
            .is_some()
        || config(path, &format!("remote.{remote}.mirror"))
            .await?
            .is_some_and(|v| v != "false")
    {
        return Err("推送覆盖配置需要单独核实，请使用独立推送入口".into());
    }
    let urls: Vec<String> = text(path, &["remote", "get-url", "--push", "--all", &remote])
        .await?
        .lines()
        .map(str::to_owned)
        .collect();
    if urls.is_empty()
        || urls.len() > 16
        || urls.iter().any(|url| url.is_empty() || url.contains('\0'))
    {
        return Err("推送地址为空或过多，无法绑定接续推送".into());
    }
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&urls).map_err(|_| "推送目标不可编码")?)
    );
    let git_dir = text(path, &["rev-parse", "--absolute-git-dir"]).await?;
    Ok(Observed {
        repository: RepositoryTarget {
            worktree: path
                .canonicalize()
                .map_err(|_| "仓库路径不可核实")?
                .to_string_lossy()
                .into_owned(),
            git_directory: Path::new(&git_dir)
                .canonicalize()
                .map_err(|_| "Git 目录不可核实")?
                .to_string_lossy()
                .into_owned(),
            branch,
            remote,
            destination_ref,
            destinations_digest: fingerprint,
        },
        urls,
    })
}
pub(super) fn marker(operation: &str) -> String {
    format!("dogcodes-commit-{operation}")
}
fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) async fn bind(
    path: &Path,
    before: Observed,
    operation: &str,
) -> Result<PushTarget, String> {
    let after = observe(path).await?;
    if before.repository != after.repository {
        return Err("提交期间分支或推送目标已变化，请核实后单独推送".into());
    }
    // A post-commit hook or another client can move HEAD after our commit. A
    // unique reflog action identifies the command; ambiguity never authorizes push.
    let reflog = text(
        path,
        &[
            "reflog",
            "show",
            "-64",
            "--format=%H%x00%gs",
            &before.repository.branch,
        ],
    )
    .await?;
    let prefix = format!("{}: ", marker(operation));
    let matches: Vec<_> = reflog
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .filter(|(_, subject)| subject.starts_with(&prefix))
        .collect();
    if matches.len() != 1 || !valid_oid(matches[0].0) {
        return Err("本次提交已完成，但无法唯一核实其引用记录，请检查仓库后单独推送".into());
    }
    let oid = matches[0].0.to_owned();
    if text(path, &["rev-parse", "--verify", "HEAD"]).await? != oid {
        return Err("提交后 HEAD 已变化，请检查最新提交后单独推送".into());
    }
    Ok(PushTarget {
        repository: before.repository,
        oid,
    })
}
pub(super) async fn push(path: &Path, target: &PushTarget) -> Result<(), String> {
    let observed = observe(path).await?;
    if observed.repository != target.repository
        || text(path, &["rev-parse", "--verify", "HEAD"]).await? != target.oid
    {
        return Err("REVIEW_PUSH_TARGET_CHANGED: 分支、提交或远端已变化；未沿用旧操作推送，请核实后重新发起推送".into());
    }
    if !valid_oid(&target.oid) {
        return Err("原推送提交标识损坏，未执行推送".into());
    }
    require_git_success(
        run_commit_command(
            path,
            &["check-ref-format", &target.repository.destination_ref],
        )
        .await?,
    )?;
    let refspec = format!("{}:{}", target.oid, target.repository.destination_ref);
    for (index, url) in observed.urls.iter().enumerate() {
        // Explicit OID/ref prevents a later HEAD move from expanding this push.
        // Relative URLs are resolved against the original worktree, never cwd/name.
        let destination = if !url.contains(':') && !Path::new(url).is_absolute() {
            path.join(url).to_string_lossy().into_owned()
        } else {
            url.clone()
        };
        let result = run_commit_command(
            path,
            &[
                "push",
                "--porcelain",
                "--no-follow-tags",
                "--recurse-submodules=no",
                "--",
                &destination,
                &refspec,
            ],
        )
        .await;
        match result.and_then(require_git_success) {
            Ok(()) => {}
            Err(error) => {
                use redaction::SecretRedactor;
                let detail = redaction::DeterministicRedactor
                    .redact(&error.replace(url, "[推送地址]"))
                    .text;
                return Err(format!(
                    "推送未完成（已完成 {index}/{} 个目标）：{detail}",
                    observed.urls.len()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn git(path: &Path, args: &[&str]) -> String {
        let out = run_commit_command(path, args).await.unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    async fn repository() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let remote = dir.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).await;
        git(&repo, &["config", "user.name", "Test"]).await;
        git(&repo, &["config", "user.email", "test@example.invalid"]).await;
        git(&repo, &["config", "commit.gpgsign", "false"]).await;
        git(&repo, &["config", "core.hooksPath", "disabled-hooks"]).await;
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .await;
        git(&repo, &["config", "branch.main.remote", "origin"]).await;
        git(&repo, &["config", "branch.main.merge", "refs/heads/main"]).await;
        (dir, repo, remote)
    }
    async fn commit(path: &Path, operation: &str) {
        require_git_success(
            super::super::run_commit_command_tagged(
                path,
                &["commit", "--allow-empty", "-m", "change"],
                Some(&marker(operation)),
            )
            .await
            .unwrap(),
        )
        .unwrap();
    }
    #[tokio::test]
    async fn bound_push_rejects_head_branch_remote_and_destination_changes() {
        for change in ["head", "branch", "remote", "destination"] {
            let (_dir, repo, remote) = repository().await;
            git(&repo, &["init", "--bare", "-q", remote.to_str().unwrap()]).await;
            let observed = observe(&repo).await.unwrap();
            commit(&repo, "ours").await;
            let target = bind(&repo, observed, "ours").await.unwrap();
            match change {
                "head" => {
                    commit(&repo, "later").await;
                }
                "branch" => {
                    git(&repo, &["checkout", "-qb", "other"]).await;
                }
                "remote" => {
                    git(&repo, &["remote", "set-url", "origin", "elsewhere.git"]).await;
                }
                _ => {
                    git(&repo, &["config", "branch.main.merge", "refs/heads/other"]).await;
                }
            }
            assert!(
                push(&repo, &target).await.is_err(),
                "accepted changed {change}"
            );
            assert!(
                git(&remote, &["for-each-ref", "--format=%(refname)"])
                    .await
                    .is_empty()
            );
        }
    }
    #[tokio::test]
    async fn binding_does_not_confuse_later_or_ambiguous_reflog_commits_with_ours() {
        for second in ["later", "ours"] {
            let (_dir, repo, _) = repository().await;
            let observed = observe(&repo).await.unwrap();
            commit(&repo, "ours").await;
            commit(&repo, second).await; // Also models a hook inheriting the action marker.
            assert!(bind(&repo, observed, "ours").await.is_err());
            assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).await, "2");
        }
    }
    #[tokio::test]
    async fn binding_requires_reflog_evidence_and_does_not_persist_remote_credentials() {
        let (_dir, repo, _) = repository().await;
        git(
            &repo,
            &[
                "remote",
                "set-url",
                "origin",
                "https://user:secret-test-token@example.invalid/repo.git",
            ],
        )
        .await;
        let observed = observe(&repo).await.unwrap();
        commit(&repo, "ours").await;
        let target = bind(&repo, observed, "ours").await.unwrap();
        let serialized = serde_json::to_string(&target).unwrap();
        assert!(!serialized.contains("secret-test-token"));
        assert!(!serialized.contains("https://"));
        let before = observe(&repo).await.unwrap();
        std::fs::remove_dir_all(repo.join(".git/logs")).unwrap();
        git(&repo, &["config", "core.logAllRefUpdates", "false"]).await;
        commit(&repo, "without-log").await;
        assert!(bind(&repo, before, "without-log").await.is_err());
    }
    #[tokio::test]
    async fn multiple_destinations_preserve_partial_failure_and_retry_the_same_commit() {
        let (dir, repo, remote) = repository().await;
        let second = dir.path().join("second.git");
        git(&repo, &["init", "--bare", "-q", remote.to_str().unwrap()]).await;
        git(
            &repo,
            &[
                "remote",
                "set-url",
                "--add",
                "--push",
                "origin",
                remote.to_str().unwrap(),
            ],
        )
        .await;
        git(
            &repo,
            &[
                "remote",
                "set-url",
                "--add",
                "--push",
                "origin",
                second.to_str().unwrap(),
            ],
        )
        .await;
        let before = observe(&repo).await.unwrap();
        commit(&repo, "ours").await;
        let target = bind(&repo, before, "ours").await.unwrap();
        let error = push(&repo, &target).await.unwrap_err();
        assert!(error.contains("1/2"), "{error}");
        assert_eq!(git(&remote, &["rev-parse", "main"]).await, target.oid);
        git(&repo, &["init", "--bare", "-q", second.to_str().unwrap()]).await;
        push(&repo, &target).await.unwrap();
        assert_eq!(git(&second, &["rev-parse", "main"]).await, target.oid);
        assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).await, "1");
    }
}
