//! Private real processes; no installed Antigravity runtime or account.
use super::*;
use std::{path::Path, time::Duration};

pub(super) fn fixture() -> (tempfile::TempDir, PathBuf) {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let root = tempfile::tempdir_in(&assets).unwrap();
    let executable = root.path().join("runtime");
    std::fs::hard_link(assets.join("metadata.py"), &executable).unwrap();
    (root, executable)
}

pub(super) async fn cancelled<R: Send + 'static>(
    root: &Path,
    work: tokio::task::JoinHandle<R>,
    lock: SpawnLock,
) {
    let marker = root.join("started.json");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pids: Vec<u32> = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    assert!(lock.try_lock().is_err());
    work.abort();
    assert!(matches!(work.await, Err(error) if error.is_cancelled()));
    let acquired = tokio::time::timeout(Duration::from_secs(5), lock.clone().lock_owned()).await;
    let prompt = acquired.is_ok();
    // The unfixed worker is still supervised. Let it exit so the negative
    // assertion never strands the private tree or contaminates later tests.
    let _guard = if let Ok(guard) = acquired {
        guard
    } else {
        std::fs::write(root.join("release"), "release").unwrap();
        tokio::time::timeout(Duration::from_secs(10), lock.lock_owned())
            .await
            .unwrap()
    };
    for pid in pids {
        assert!(
            !cleanup_tests::alive(pid),
            "auth slot released before original PID {pid} exited"
        );
    }
    assert!(
        prompt,
        "cancelled helper kept the shared auth slot until its query deadline"
    );
}

#[tokio::test]
async fn cancelled_catalog_retires_its_tree_and_releases_auth_without_waiting_for_timeout() {
    let (root, executable) = fixture();
    let driver = Arc::new(tests::test_driver(executable));
    let worker = driver.clone();
    let work = tokio::spawn(async move { worker.available_models().await });
    cancelled(root.path(), work, driver.spawn.clone()).await;
    std::fs::write(root.path().join("release"), "release").unwrap();
    assert_eq!(driver.available_models().await.unwrap(), ["fixture-model"]);
}

#[tokio::test]
async fn cancelled_readiness_with_open_stdout_releases_auth_without_waiting_for_probe_timeout() {
    let (root, executable) = fixture();
    let lock = SpawnLock::default();
    let probe_lock = lock.clone();
    let work =
        tokio::spawn(
            async move { AntigravityDriver::probe_ready_with(&executable, &probe_lock).await },
        );
    cancelled(root.path(), work, lock).await;
}

#[tokio::test]
async fn cancelled_hook_verification_retires_its_tree_without_launching_a_session() {
    let (root, executable) = fixture();
    let driver = Arc::new(tests::test_driver(executable));
    let worker = driver.clone();
    let workspace = cleanup_tests::request(root.path());
    let work = tokio::spawn(async move { worker.create_session(workspace).await });
    cancelled(root.path(), work, driver.spawn.clone()).await;
    assert!(driver.processes.lock().await.is_empty());
    std::fs::write(root.path().join("release"), "release").unwrap();
    assert_eq!(driver.available_models().await.unwrap(), ["fixture-model"]);
}
