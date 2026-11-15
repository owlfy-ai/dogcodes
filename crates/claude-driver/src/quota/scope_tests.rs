//! Private native CLI fixtures; no real credentials, network requests or model turns.
use crate::{
    native_tests::{fixture, ids, present, requests, wait_for},
    *,
};
use std::time::Duration;

#[tokio::test]
async fn native_usage_queries_are_control_only_and_cache_valid_readings() {
    let (root, driver) = fixture("quota-normal");
    let value = driver.read_quota_with_refresh(None, true).await.unwrap();
    assert_eq!(value.windows[0].used_percent, Some(40.0));
    assert_eq!(value.plan_type.as_deref(), Some("pro"));
    assert_eq!(requests(root.path(), "initialize"), 1);
    assert_eq!(requests(root.path(), "get_usage"), 1);
    assert_eq!(requests(root.path(), "prompt"), 0);
    assert_eq!(value, driver.read_quota(None).await.unwrap());
    assert_eq!(requests(root.path(), "get_usage"), 1);
    assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
    let args = std::fs::read_to_string(root.path().join("quota-args.json")).unwrap();
    assert!(args.contains("--no-session-persistence") && args.contains("disableAllHooks"));
}

#[tokio::test]
async fn failed_or_missing_native_readings_are_not_reported_as_fresh_limits() {
    for (mode, expected) in [
        ("quota-expired", "claude_quota_login_expired"),
        ("quota-empty", "claude_quota_no_windows"),
        ("quota-rejected", "claude_quota_unavailable"),
    ] {
        let (root, driver) = fixture(mode);
        assert!(matches!(driver.read_quota_with_refresh(None,true).await,
            Err(DriverError::CapabilityUnavailable(reason)) if reason == expected));
        assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
        assert_eq!(requests(root.path(), "prompt"), 0);
    }
}

#[tokio::test]
async fn cancelled_usage_queries_retire_the_original_tree_before_the_next_query() {
    let (root, driver) = fixture("quota-blocked");
    let driver = Arc::new(driver);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.read_quota_with_refresh(None, true).await });
    wait_for(|| requests(root.path(), "get_usage") == 1).await;
    let original = ids(root.path()).await;
    std::fs::write(
        root.path().join("previous-pids.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    std::fs::write(root.path().join("fixture-mode"), "quota-normal").unwrap();
    let value = tokio::time::timeout(
        Duration::from_secs(8),
        driver.read_quota_with_refresh(None, true),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(value.windows.len(), 2);
    let start: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("quota-start.json")).unwrap())
            .unwrap();
    assert_eq!(start["previousAlive"], json!([]));
    assert!(original.iter().all(|pid| !present(*pid)));
    assert!(ids(root.path()).await.iter().all(|pid| !present(*pid)));
}
