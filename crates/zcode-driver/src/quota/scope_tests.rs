//! Only private native/account fixtures; no credentials, provider or model call.
use crate::{
    cleanup_tests::{driver, present, wait_for},
    metadata::tests::{fixture, ids, records},
    *,
};
use std::{path::Path, time::Duration};

fn configured(mode: &str) -> (tempfile::TempDir, ZcodeLaunch) {
    let (root, launch) = fixture(mode);
    std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
    std::fs::write(&launch.cli_script, "").unwrap();
    let builtin = launch.bundled_provider_config().unwrap_or_else(|| {
        root.path()
            .join("Resources/config/provider/zcode-builtin.json")
    });
    std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
    std::fs::write(builtin, "{}").unwrap();
    (root, launch)
}
fn started(root: &Path, action: &str) -> bool {
    records(root)
        .iter()
        .any(|row| row["method"] == "account.spawn" && row["params"]["action"] == action)
}

#[tokio::test]
async fn cancelled_identity_and_usage_queries_keep_admission_until_original_exit() {
    for (mode, action) in [
        ("quota-identity-blocked", "quota_identity"),
        ("quota-blocked", "quota"),
    ] {
        let (root, launch) = configured(mode);
        let driver = driver(launch);
        let first_driver = driver.clone();
        let first =
            tokio::spawn(async move { first_driver.read_quota_with_refresh(None, true).await });
        wait_for(|| started(root.path(), action)).await;
        let original = ids(root.path());
        let count = records(root.path()).len();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        std::fs::write(root.path().join("metadata-mode"), "quota-normal").unwrap();
        let next = tokio::time::timeout(
            Duration::from_secs(8),
            driver.read_quota_with_refresh(None, true),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(next.windows[0].used_percent, Some(25.0));
        assert!(original.iter().all(|pid| !present(*pid)));
        assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
        assert!(
            records(root.path())[count..]
                .iter()
                .all(|row| row["params"]["priorAlive"] == json!([])),
            "replacement account helper started before original retirement"
        );
        assert!(driver.connection.lock().await.is_none());
        assert!(driver.sessions.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn exited_account_leader_does_not_wait_for_descendants_terminal_eof() {
    let (root, launch) = configured("quota-inherited");
    let driver = driver(launch);
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        driver.read_quota_with_refresh(None, true),
    )
    .await;
    // If the old implementation times out, still let the private worker clean
    // its tree before failing, rather than leaving fixture processes behind.
    wait_for(|| ids(root.path()).iter().all(|pid| !present(*pid))).await;
    assert!(
        result.is_ok(),
        "account read waited for descendant-held stdout instead of observing the original leader"
    );
    assert_eq!(result.unwrap().unwrap().windows[0].used_percent, Some(25.0));
    assert!(driver.connection.lock().await.is_none());
}

#[tokio::test]
async fn queued_quota_cancellation_never_starts_a_late_helper() {
    let (root, launch) = configured("quota-normal");
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let held = gate.clone().lock_owned().await;
    let query_launch = launch.clone();
    let query_gate = gate.clone();
    let first = tokio::spawn(async move {
        crate::account::Account::quota_read(&query_launch, true, query_gate).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(records(root.path()).is_empty());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    drop(held);
    crate::account::Account::quota_read(&launch, true, gate.clone())
        .await
        .unwrap();
    assert_eq!(records(root.path()).len(), 1);
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    assert!(gate.try_lock().is_ok());
}

#[tokio::test]
async fn cached_quota_rechecks_identity_and_discards_it_after_account_change_or_login_loss() {
    let (root, launch) = configured("quota-normal");
    let driver = driver(launch);
    let first = driver.read_quota(None).await.unwrap();
    assert_eq!(first.windows[0].used_percent, Some(25.0));
    assert_eq!(records(root.path()).len(), 2);
    let cached = driver.read_quota(None).await.unwrap();
    assert_eq!(first, cached);
    assert_eq!(
        records(root.path()).len(),
        3,
        "cache hit should check identity but not pull again"
    );
    std::fs::write(root.path().join("metadata-mode"), "quota-other").unwrap();
    let other = driver.read_quota(None).await.unwrap();
    assert_eq!(other.windows[0].used_percent, Some(75.0));
    assert_eq!(records(root.path()).len(), 5);
    std::fs::write(root.path().join("metadata-mode"), "quota-login").unwrap();
    assert!(
        matches!(driver.read_quota(None).await, Err(DriverError::CapabilityUnavailable(reason)) if reason == "quota_login_required")
    );
    assert!(driver.quota_cache.lock().await.snapshot.is_none());
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    assert!(driver.connection.lock().await.is_none());
}

#[tokio::test]
async fn malformed_or_excessive_account_output_retires_before_a_successful_retry() {
    for mode in ["quota-malformed", "quota-overflow"] {
        let (root, launch) = configured(mode);
        let driver = driver(launch);
        let result = tokio::time::timeout(
            Duration::from_secs(4),
            driver.read_quota_with_refresh(None, true),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(DriverError::CapabilityUnavailable(reason)) if reason == "quota_unavailable")
        );
        assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
        assert!(driver.quota_cache.lock().await.admission.try_lock().is_ok());
        std::fs::write(root.path().join("metadata-mode"), "quota-normal").unwrap();
        assert_eq!(
            driver
                .read_quota_with_refresh(None, true)
                .await
                .unwrap()
                .windows[0]
                .used_percent,
            Some(25.0)
        );
        assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    }
}
