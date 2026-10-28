//! Isolated environment and private fake transport; never read user credentials.
use crate::{
    native_tests::{present, wait_for},
    *,
};
use std::{path::Path, process::Command, time::Duration};

fn query(root: &Path, index: usize) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(format!("query-{index}.json"))).unwrap())
        .unwrap()
}
fn pids(record: &Value) -> Vec<i32> {
    serde_json::from_value(record["pids"].clone()).unwrap()
}

#[test]
fn isolated_quota_transport() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("bin")).unwrap();
    symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quota-command.py"),
        root.path().join("bin/curl"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config/.credentials.json"), r#"{"claudeAiOauth":{"accessToken":"private-fixture-token","expiresAt":9223372036854775807,"subscriptionType":"private"}}"#).unwrap();
    std::fs::write(root.path().join("mode"), "blocked").unwrap();
    let path = std::env::join_paths(
        std::iter::once(root.path().join("bin"))
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "quota::scope_tests::quota_transport_fixture",
            "--nocapture",
        ])
        .current_dir(root.path())
        .env("DOGCODES_CLAUDE_QUOTA_FIXTURE", root.path())
        .env("CLAUDE_CONFIG_DIR", root.path().join("config"))
        .env("PATH", path)
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL")
        .status()
        .unwrap();
    // Failures must not leave private helpers alive. Verify their command still
    // names this unique root before signalling; never act on an old PID alone.
    for entry in std::fs::read_dir(root.path()).unwrap().flatten() {
        if entry.file_name().to_string_lossy().starts_with("query-") {
            let record: Value =
                serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
            for pid in pids(&record) {
                let output = Command::new("ps")
                    .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .unwrap();
                if String::from_utf8_lossy(&output.stdout).contains(root.path().to_str().unwrap()) {
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                }
            }
        }
    }
    assert!(status.success(), "private quota subprocess failed");
}

#[tokio::test]
async fn quota_transport_fixture() {
    let Some(root) = std::env::var_os("DOGCODES_CLAUDE_QUOTA_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let driver = Arc::new(
        ClaudeDriver::new(
            "claude-code",
            &InstallationId::new().to_string(),
            root.join("missing-cli"),
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap(),
    );
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.read_quota_with_refresh(None, true).await });
    wait_for(|| root.join("input-0.json").exists()).await;
    let original = query(&root, 0);
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    std::fs::write(root.join("mode"), "normal").unwrap();
    let snapshot = tokio::time::timeout(
        Duration::from_secs(8),
        driver.read_quota_with_refresh(None, true),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(snapshot.windows.len(), 2);
    assert_eq!(
        query(&root, 1)["previousAlive"],
        json!([]),
        "replacement started before original helper tree exited"
    );
    assert!(pids(&original).into_iter().all(|pid| !present(pid)));
    assert!(pids(&query(&root, 1)).into_iter().all(|pid| !present(pid)));
    let cached = driver.read_quota(None).await.unwrap();
    assert_eq!(snapshot, cached);
    assert!(!root.join("query-2.json").exists());

    std::fs::write(root.join("mode"), "limited").unwrap();
    for _ in 0..2 {
        assert!(matches!(driver.read_quota_with_refresh(None, true).await,
            Err(DriverError::CapabilityUnavailable(reason)) if reason == super::RATE_LIMITED));
    }
    assert!(
        !root.join("query-3.json").exists(),
        "forced retry bypassed server backoff"
    );
    assert!(pids(&query(&root, 2)).into_iter().all(|pid| !present(pid)));
    {
        let mut cache = driver.quota.pull.lock().await;
        cache.backoff_until = 0;
        cache.next_attempt = 0;
    }
    std::fs::write(root.join("mode"), "login").unwrap();
    assert!(matches!(driver.read_quota_with_refresh(None, true).await,
        Err(DriverError::CapabilityUnavailable(reason)) if reason == "claude_quota_login_required"));
    assert!(driver.quota.pull.lock().await.snapshot.is_none());
    assert!(pids(&query(&root, 3)).into_iter().all(|pid| !present(pid)));
}

fn command(root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quota-command.py"),
    );
    command
        .arg("https://api.anthropic.com/api/oauth/usage")
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    command
}
fn private_input() -> Vec<u8> {
    b"header = \"Authorization: Bearer private-fixture-token\"\n".to_vec()
}

#[tokio::test]
async fn cancelled_partial_stdin_retires_before_the_next_helper_is_admitted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("mode"), "blocked-input").unwrap();
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let mut input = private_input();
    input.resize(512 * 1024, b'x');
    let first = tokio::spawn(super::command_output(
        command(root.path()),
        Some(input),
        4096,
        gate.clone(),
    ));
    wait_for(|| root.path().join("query-0.json").exists()).await;
    assert!(!root.path().join("input-0.json").exists());
    assert!(!first.is_finished());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    std::fs::write(root.path().join("mode"), "normal").unwrap();
    let next = tokio::time::timeout(
        Duration::from_secs(8),
        super::command_output(
            command(root.path()),
            Some(private_input()),
            4096,
            gate.clone(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(next.status.success());
    assert_eq!(query(root.path(), 1)["previousAlive"], json!([]));
    assert!(
        pids(&query(root.path(), 0))
            .into_iter()
            .all(|pid| !present(pid))
    );
    assert!(
        pids(&query(root.path(), 1))
            .into_iter()
            .all(|pid| !present(pid))
    );
    assert!(gate.try_lock().is_ok());
}

#[tokio::test]
async fn output_failure_and_no_input_success_confirm_retirement_before_returning() {
    for mode in ["overflow", "credential"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("mode"), mode).unwrap();
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let input = (mode == "overflow").then(private_input);
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            super::command_output(command(root.path()), input, 1024, gate.clone()),
        )
        .await
        .unwrap();
        assert_eq!(result.is_ok(), mode == "credential");
        if let Ok(output) = result {
            assert!(output.status.success());
            assert_eq!(output.stdout, b"{\"private\":true}");
        }
        assert!(
            pids(&query(root.path(), 0))
                .into_iter()
                .all(|pid| !present(pid))
        );
        assert!(gate.try_lock().is_ok());
    }
}

#[tokio::test]
async fn cancelling_a_queued_credential_query_does_not_start_it_later() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("mode"), "credential").unwrap();
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let held = gate.clone().lock_owned().await;
    let first = tokio::spawn(super::command_output(
        command(root.path()),
        None,
        1024,
        gate.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!root.path().join("query-0.json").exists());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    drop(held);
    super::command_output(command(root.path()), None, 1024, gate)
        .await
        .unwrap();
    assert!(!root.path().join("query-1.json").exists());
    assert!(
        pids(&query(root.path(), 0))
            .into_iter()
            .all(|pid| !present(pid))
    );
}
