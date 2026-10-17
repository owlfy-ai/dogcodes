use super::{require_git_success, run_commit_command};
use std::path::Path;

async fn git_text(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_commit_command(path, args).await?;
    if !output.status.success() {
        require_git_success(output)?;
        unreachable!();
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| "Git 输出编码无效".into())
}

pub(super) async fn pull_or_sync(path: &Path, sync: bool) -> Result<(), String> {
    let branch = git_text(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .map_err(|error| format!("REVIEW_DETACHED_HEAD\n{error}"))?;
    let upstream = git_text(path, &["rev-parse", "--symbolic-full-name", "@{upstream}"])
        .await
        .map_err(|error| format!("REVIEW_NO_UPSTREAM\n{error}"))?;
    let remote = git_text(
        path,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .await?;
    let merge = git_text(
        path,
        &["config", "--get", &format!("branch.{branch}.merge")],
    )
    .await?;
    if remote.is_empty() || !merge.starts_with("refs/heads/") {
        return Err("REVIEW_NO_UPSTREAM: 上游配置不可用".into());
    }
    // Keep Git's configured merge/rebase policy. A failed pull (including a
    // conflict) returns immediately, so sync never pushes a partial integration.
    require_git_success(
        run_commit_command(path, &["pull", "--no-edit", "--", &remote, &merge]).await?,
    )?;
    if !sync {
        return Ok(());
    }
    let current = git_text(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await?;
    if current != branch {
        return Err("同步期间当前分支已改变，请检查后重试".into());
    }
    let outgoing = git_text(
        path,
        &[
            "rev-list",
            "--count",
            &format!("{upstream}..refs/heads/{branch}"),
        ],
    )
    .await?;
    if outgoing == "0" {
        return Ok(());
    }
    // Pin both sides to the upstream chosen before pull; do not rely on a
    // different push.default or pushRemote to silently publish elsewhere.
    run_commit_command(
        path,
        &[
            "push",
            "--",
            &remote,
            &format!("refs/heads/{branch}:{merge}"),
        ],
    )
    .await
    .and_then(require_git_success)
    .map_err(|error| format!("REVIEW_SYNC_PUSH_FAILED\n{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn git(path: &Path, args: &[&str]) -> String {
        git_text(path, args).await.unwrap()
    }
    async fn setup() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let local = temp.path().join("local");
        let other = temp.path().join("other");
        git(
            temp.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        )
        .await;
        git(
            temp.path(),
            &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
        )
        .await;
        configure(&local).await;
        std::fs::write(local.join("file"), "initial\n").unwrap();
        commit(&local, "initial").await;
        git(&local, &["push", "-u", "origin", "main"]).await;
        git(
            temp.path(),
            &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
        )
        .await;
        configure(&other).await;
        (temp, remote, local, other)
    }
    async fn configure(path: &Path) {
        for args in [
            ["config", "user.name", "Test"],
            ["config", "user.email", "test@example.test"],
            ["config", "commit.gpgsign", "false"],
            ["config", "pull.rebase", "false"],
        ] {
            git(path, &args).await;
        }
    }
    async fn commit(path: &Path, message: &str) {
        git(path, &["add", "."]).await;
        git(path, &["commit", "-m", message]).await;
    }
    #[tokio::test]
    async fn pull_integrates_without_publishing_local_commits_then_sync_publishes() {
        let (_temp, remote, local, other) = setup().await;
        std::fs::write(local.join("local-file"), "local\n").unwrap();
        commit(&local, "local change").await;
        std::fs::write(other.join("remote-file"), "remote\n").unwrap();
        commit(&other, "remote change").await;
        git(&other, &["push"]).await;
        let remote_before = git(&remote, &["rev-parse", "main"]).await;
        pull_or_sync(&local, false).await.unwrap();
        assert!(local.join("remote-file").exists());
        assert_eq!(git(&remote, &["rev-parse", "main"]).await, remote_before);
        pull_or_sync(&local, true).await.unwrap();
        assert_eq!(
            git(&remote, &["rev-parse", "main"]).await,
            git(&local, &["rev-parse", "HEAD"]).await
        );
        assert!(
            git(&remote, &["show", "main:local-file"])
                .await
                .contains("local")
        );
    }
    #[tokio::test]
    async fn conflict_stops_sync_before_push_and_preserves_both_histories() {
        let (_temp, remote, local, other) = setup().await;
        std::fs::write(local.join("file"), "local edit\n").unwrap();
        commit(&local, "local change").await;
        std::fs::write(other.join("file"), "remote edit\n").unwrap();
        commit(&other, "remote change").await;
        git(&other, &["push"]).await;
        let remote_before = git(&remote, &["rev-parse", "main"]).await;
        let local_before = git(&local, &["rev-parse", "HEAD"]).await;
        assert!(
            pull_or_sync(&local, true)
                .await
                .unwrap_err()
                .contains("CONFLICT")
        );
        assert_eq!(git(&remote, &["rev-parse", "main"]).await, remote_before);
        assert_eq!(git(&local, &["rev-parse", "HEAD"]).await, local_before);
        assert!(!git(&local, &["ls-files", "--unmerged"]).await.is_empty());
    }
    #[tokio::test]
    async fn missing_upstream_and_detached_head_are_reported_before_remote_operations() {
        let (_temp, _remote, local, _other) = setup().await;
        git(&local, &["checkout", "-b", "unpublished"]).await;
        assert!(
            pull_or_sync(&local, false)
                .await
                .unwrap_err()
                .contains("REVIEW_NO_UPSTREAM")
        );
        git(&local, &["checkout", "--detach"]).await;
        assert!(
            pull_or_sync(&local, true)
                .await
                .unwrap_err()
                .contains("REVIEW_DETACHED_HEAD")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_push_reports_completed_pull_and_keeps_local_commits() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, remote, local, other) = setup().await;
        std::fs::write(local.join("local-file"), "local\n").unwrap();
        commit(&local, "local change").await;
        std::fs::write(other.join("remote-file"), "remote\n").unwrap();
        commit(&other, "remote change").await;
        git(&other, &["push"]).await;
        let before = git(&remote, &["rev-parse", "main"]).await;
        let hook = remote.join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            pull_or_sync(&local, true)
                .await
                .unwrap_err()
                .starts_with("REVIEW_SYNC_PUSH_FAILED")
        );
        assert!(local.join("remote-file").exists());
        assert!(local.join("local-file").exists());
        assert_eq!(git(&remote, &["rev-parse", "main"]).await, before);
        assert_ne!(git(&local, &["rev-parse", "HEAD"]).await, before);
    }
}
