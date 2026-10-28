//! Private stdio processes; no installed Claude or account is contacted.
use crate::{
    native_tests::{fixture, ids, present, requests, wait_for, workspace},
    *,
};
use std::time::Duration;

async fn catalogue(
    driver: &ClaudeDriver,
    root: &std::path::Path,
) -> Result<driver_api::ModelCatalog, DriverError> {
    driver.model_catalog_in_workspace(None, root.to_str()).await
}

#[tokio::test]
async fn stderr_is_drained_and_catalogue_returns_only_after_original_process_tree_exits() {
    let (root, driver) = fixture("catalog-stderr");
    let result =
        tokio::time::timeout(Duration::from_secs(4), catalogue(&driver, root.path())).await;
    assert!(result.is_ok(), "catalogue blocked behind unread stderr");
    let catalog = result.unwrap().unwrap();
    assert!(catalog.models.iter().any(|m| m.id == "fixture-model"));
    let owned = ids(root.path()).await;
    assert!(
        owned.iter().all(|pid| !present(*pid)),
        "metadata returned before physical cleanup"
    );
    assert_eq!(requests(root.path(), "initialize"), 1);
    assert_eq!(requests(root.path(), "prompt"), 0);
}

#[tokio::test]
async fn cancelling_a_catalogue_retires_its_scope_before_permission_probe_without_stopping_a_peer()
{
    let (root, driver) = fixture("catalog-blocked");
    let driver = Arc::new(driver);
    let (peer_root, _) = fixture("normal");
    let peer = driver
        .create_session(workspace(peer_root.path()))
        .await
        .unwrap();
    let peer_ids = ids(peer_root.path()).await;
    let worker_driver = driver.clone();
    let first_root = root.path().to_owned();
    let first = tokio::spawn(async move { catalogue(&worker_driver, &first_root).await });
    let original = ids(root.path()).await;
    wait_for(|| requests(root.path(), "initialize") == 1).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let (next_root, _) = fixture("catalog-normal");
    std::fs::write(
        next_root.path().join("previous-pids.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    let permissions = tokio::time::timeout(
        Duration::from_secs(6),
        driver.permission_catalog(next_root.path().to_str()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(permissions.modes.iter().any(|mode| mode.id == "auto"));
    let started: Value = serde_json::from_slice(
        &std::fs::read(next_root.path().join("catalog-start.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        started["previousAlive"],
        json!([]),
        "replacement launched before original retirement"
    );
    assert!(original.iter().all(|pid| !present(*pid)));
    let next_ids = ids(next_root.path()).await;
    assert!(next_ids.iter().all(|pid| !present(*pid)));
    assert!(peer_ids.iter().all(|pid| present(*pid)));
    assert_eq!(requests(root.path(), "initialize"), 1);
    driver.shutdown(peer).await.unwrap();
}

#[tokio::test]
async fn concurrent_model_and_permission_queries_do_not_start_overlapping_helpers() {
    let (root, driver) = fixture("catalog-blocked");
    let driver = Arc::new(driver);
    let first_driver = driver.clone();
    let first_root = root.path().to_owned();
    let first = tokio::spawn(async move { catalogue(&first_driver, &first_root).await });
    let original = ids(root.path()).await;
    let (next_root, _) = fixture("catalog-normal");
    std::fs::write(
        next_root.path().join("previous-pids.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    let mut second = Box::pin(driver.permission_catalog(next_root.path().to_str()));
    assert!(
        tokio::time::timeout(Duration::from_secs(4), &mut second)
            .await
            .is_err(),
        "second probe bypassed original admission"
    );
    assert!(
        !next_root.path().join("pids.json").exists(),
        "parallel helper already started"
    );
    first.abort();
    let _ = first.await;
    tokio::time::timeout(Duration::from_secs(6), second)
        .await
        .unwrap()
        .unwrap();
    let started: Value = serde_json::from_slice(
        &std::fs::read(next_root.path().join("catalog-start.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(started["previousAlive"], json!([]));
}

#[tokio::test]
async fn failed_catalogue_releases_admission_after_confirmed_cleanup() {
    let (root, driver) = fixture("catalog-empty");
    assert!(catalogue(&driver, root.path()).await.is_err());
    assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
    let (next_root, _) = fixture("catalog-normal");
    catalogue(&driver, next_root.path()).await.unwrap();
    assert!(ids(next_root.path()).await.iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn cancelling_a_queued_query_never_starts_a_helper_later() {
    let (root, driver) = fixture("catalog-normal");
    let admission = driver.catalog_probe.clone().lock_owned().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), catalogue(&driver, root.path()))
            .await
            .is_err()
    );
    assert!(!root.path().join("pids.json").exists());
    drop(admission);
    // Successful reuse also proves a cancelled waiter did not retain the gate.
    catalogue(&driver, root.path()).await.unwrap();
    assert_eq!(requests(root.path(), "initialize"), 1);
    assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
}
