//! Private handshake/EOF fixtures; never execute the user's Claude or Keychain.
use crate::{
    native_tests::{ids, present, requests, wait_for, workspace},
    *,
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

fn fixture(mode: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::symlink;
    let (root, _) = native_tests::fixture(mode);
    let executable = root.path().join("readiness-fixture");
    symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-process.py"),
        &executable,
    )
    .unwrap();
    (root, executable)
}

// Start the processing clock only after the private CLI has actually received
// initialize. Fork/exec setup is observed separately by wait_for's own bound.
async fn observed_probe(root: &Path, executable: PathBuf) -> Result<(), DriverError> {
    let mut task = tokio::spawn(async move { ClaudeDriver::probe_ready(&executable).await });
    wait_for(|| requests(root, "initialize") == 1).await;
    let result = tokio::time::timeout(Duration::from_secs(4), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
        let owned = ids(root).await;
        // Old Drop can leave a dead leader queued for Tokio's orphan reaper.
        // Stop every running private member before failing. Success below
        // still requires actual absence, including the leader's reap.
        wait_for(|| {
            owned.iter().all(|pid| {
                let output = std::process::Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "stat="])
                    .output()
                    .unwrap();
                let state = String::from_utf8_lossy(&output.stdout);
                state.trim().is_empty() || state.trim().starts_with('Z')
            })
        })
        .await;
    }
    let result = result
        .expect("readiness stalled after initialize was received")
        .unwrap();
    assert!(ids(root).await.iter().all(|pid| !present(*pid)));
    result
}

#[tokio::test]
async fn readiness_drains_stderr_and_allows_natural_finish_before_returning() {
    let (root, executable) = fixture("probe-stderr");
    observed_probe(root.path(), executable).await.unwrap();
    assert!(
        root.path().join("probe-finished").exists(),
        "probe killed its CLI before the existing graceful-exit window"
    );
    assert_eq!(requests(root.path(), "initialize"), 1);
    assert_eq!(requests(root.path(), "prompt"), 0);
}

#[tokio::test]
async fn cancelled_probe_preserves_graceful_finish_and_leaves_an_unrelated_session_alive() {
    let (peer_root, peer) = native_tests::fixture("normal");
    let handle = peer
        .create_session(workspace(peer_root.path()))
        .await
        .unwrap();
    let peer_ids = ids(peer_root.path()).await;
    let (root, executable) = fixture("probe-cancel");
    let task = tokio::spawn(async move { ClaudeDriver::probe_ready(&executable).await });
    wait_for(|| root.path().join("probe-eof").exists()).await;
    let owned = ids(root.path()).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    std::fs::write(root.path().join("allow-exit"), "").unwrap();
    wait_for(|| owned.iter().all(|pid| !present(*pid))).await;
    let peer_alive = peer_ids.iter().all(|pid| present(*pid));
    peer.shutdown(handle).await.unwrap();
    assert!(peer_alive);
    assert!(
        root.path().join("probe-finished").exists(),
        "observer cancellation killed the CLI before it could finish after EOF"
    );
    assert_eq!(requests(root.path(), "initialize"), 1);
    assert_eq!(requests(root.path(), "prompt"), 0);
}

#[tokio::test]
async fn unrelated_control_response_and_unterminated_oversize_frames_are_not_ready() {
    for mode in ["probe-wrong-id", "probe-oversize"] {
        let (root, executable) = fixture(mode);
        let result = observed_probe(root.path(), executable).await;
        assert!(matches!(result, Err(DriverError::Protocol(_))));
        assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
        assert_eq!(requests(root.path(), "initialize"), 1);
        assert_eq!(requests(root.path(), "prompt"), 0);
    }
}
