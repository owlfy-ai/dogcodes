//! Actual private stdio/processes; no installed Pi or account is contacted.
use super::*;
use std::{path::Path, time::Duration};

fn fixture(mode: &str) -> (tempfile::TempDir, PiDriver) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture-mode"), mode).unwrap();
    let driver = PiDriver::new(
        &agent_domain::InstallationId::new().to_string(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog.py"),
        None,
    )
    .unwrap();
    (root, driver)
}
async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn pids(root: &Path) -> Vec<u32> {
    wait_for(|| root.join("pids.json").exists()).await;
    serde_json::from_slice(&std::fs::read(root.join("pids.json")).unwrap()).unwrap()
}
fn present(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim() == pid.to_string()
}
fn count(root: &Path, method: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["type"] == method)
        .count()
}
async fn catalog(
    driver: &PiDriver,
    root: &Path,
    model: Option<&str>,
) -> Result<ModelCatalog, DriverError> {
    driver
        .model_catalog_in_workspace(model, root.to_str())
        .await
}
#[tokio::test]
async fn concurrent_catalogs_wait_for_the_cancelled_original_tree_without_stopping_a_peer() {
    let (root, driver) = fixture("blocked");
    let driver = Arc::new(driver);
    let (peer_root, _) = fixture("normal");
    let peer = driver
        .create_session(session_request(peer_root.path().to_str().unwrap()))
        .await
        .unwrap();
    let peer_ids = pids(peer_root.path()).await;
    let _running = driver
        .prompt(&peer, super::tests::prompt(peer_root.path(), "slow"))
        .await
        .unwrap();
    let peer_connection = driver.connection(&peer.native_session_id).await.unwrap();
    assert!(peer_connection.state.lock().unwrap().turn.is_some());
    let first_driver = driver.clone();
    let first_root = root.path().to_owned();
    let first = tokio::spawn(async move { catalog(&first_driver, &first_root, None).await });
    let original = pids(root.path()).await;
    wait_for(|| count(root.path(), "get_available_models") == 1).await;
    let (next_root, _) = fixture("normal");
    std::fs::write(
        next_root.path().join("previous-pids.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    let mut second = Box::pin(catalog(&driver, next_root.path(), Some("fixture/other")));
    assert!(
        tokio::time::timeout(Duration::from_secs(4), &mut second)
            .await
            .is_err()
    );
    assert!(
        !next_root.path().join("pids.json").exists(),
        "overlapping metadata helper was started"
    );
    first.abort();
    let _ = first.await;
    let result = tokio::time::timeout(Duration::from_secs(12), second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.default_model.as_deref(), Some("fixture/base"));
    let selected = result
        .models
        .iter()
        .find(|row| row.id == "fixture/other")
        .unwrap();
    assert_eq!(selected.default_thinking.as_deref(), Some("high"));
    assert_eq!(
        selected
            .thinking_options
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["off", "high"]
    );
    assert!(
        result
            .models
            .iter()
            .find(|row| row.id == "fixture/base")
            .unwrap()
            .thinking_options
            .is_empty()
    );
    let started: Value =
        serde_json::from_slice(&std::fs::read(next_root.path().join("start.json")).unwrap())
            .unwrap();
    assert_eq!(started["previousAlive"], json!([]));
    assert!(
        started["args"]
            .as_array()
            .unwrap()
            .contains(&json!("--no-session"))
    );
    assert!(original.iter().all(|id| !present(*id)));
    assert!(pids(next_root.path()).await.iter().all(|id| !present(*id)));
    assert!(peer_ids.iter().all(|id| present(*id)));
    assert!(peer_connection.state.lock().unwrap().turn.is_some());
    assert_eq!(count(peer_root.path(), "prompt"), 1);
    assert_eq!(count(root.path(), "get_available_models"), 1);
    assert_eq!(count(root.path(), "prompt"), 0);
    driver.shutdown(peer).await.unwrap();
}
#[tokio::test]
async fn failed_metadata_returns_only_after_cleanup_and_allows_the_next_query() {
    let (root, driver) = fixture("error");
    assert!(catalog(&driver, root.path(), None).await.is_err());
    assert!(pids(root.path()).await.iter().all(|id| !present(*id)));
    let (next_root, _) = fixture("normal");
    assert!(
        !catalog(&driver, next_root.path(), None)
            .await
            .unwrap()
            .models
            .is_empty()
    );
    assert!(pids(next_root.path()).await.iter().all(|id| !present(*id)));
    assert_eq!(count(next_root.path(), "prompt"), 0);
}

#[tokio::test]
async fn a_cancelled_queued_catalog_never_starts_later_or_holds_admission() {
    let (root, driver) = fixture("normal");
    let admission = driver.catalog_probe.clone().lock_owned().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            catalog(&driver, root.path(), None)
        )
        .await
        .is_err()
    );
    assert!(!root.path().join("pids.json").exists());
    drop(admission);
    catalog(&driver, root.path(), None).await.unwrap();
    assert_eq!(count(root.path(), "get_available_models"), 1);
    assert!(pids(root.path()).await.iter().all(|id| !present(*id)));
}
