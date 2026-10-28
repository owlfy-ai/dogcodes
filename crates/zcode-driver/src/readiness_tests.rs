//! Private current-protocol fixtures; never creates a native user's session.
use crate::{
    cleanup_tests::{present, wait_for},
    metadata::tests::{fixture, ids, records},
    *,
};

async fn connect(launch: ZcodeLaunch) -> Result<ZcodeDriver, DriverError> {
    ZcodeDriver::connect_ready(
        "zcode",
        &InstallationId::new().to_string(),
        launch,
        None,
        SessionResumeCapability::Supported,
    )
    .await
}

#[tokio::test]
async fn retained_readiness_creates_only_a_deferred_probe_with_explicit_close_confirmation() {
    let (root, launch) = fixture("readiness-normal");
    let driver = connect(launch).await.unwrap();
    let native = driver
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone();
    let before = ids(root.path());
    assert!(before.iter().all(|pid| present(*pid)));
    drop(driver);
    native.confirmed().await;
    assert!(
        !root.path().join("persistent-probe").exists(),
        "connection detection created a persistent session"
    );
    let rows = records(root.path());
    let created = rows
        .iter()
        .find(|row| row["method"] == "session/create")
        .unwrap();
    assert_eq!(created["params"]["persistence"], "deferred");
    let closed = rows
        .iter()
        .find(|row| row["method"] == "session/close")
        .unwrap();
    assert_eq!(closed["params"]["expectedPersistence"], "deferred");
    assert_eq!(closed["params"]["sessionId"], "probe");
    assert!(!rows.iter().any(|row| row["method"] == "session/send"));
}

#[tokio::test]
async fn missing_close_ack_and_malformed_followup_are_rejected_after_native_exit() {
    for mode in ["readiness-close-missing", "readiness-bad-list"] {
        let (root, launch) = fixture(mode);
        let result = connect(launch).await;
        let prematurely_alive = ids(root.path()).iter().any(|pid| present(*pid));
        // Old code can accept the result; close it before asserting the failure.
        let accepted = result.is_ok();
        drop(result);
        wait_for(|| ids(root.path()).iter().all(|pid| !present(*pid))).await;
        assert!(
            !accepted,
            "readiness accepted an unconfirmed close or invalid follow-up"
        );
        assert!(
            !prematurely_alive,
            "readiness failure returned before original worker exit"
        );
    }
}

#[tokio::test]
async fn cancelled_create_cannot_persist_a_readiness_session_or_send_a_task() {
    let (root, launch) = fixture("readiness-create-stuck");
    let task = tokio::spawn(async move { connect(launch).await });
    wait_for(|| {
        records(root.path())
            .iter()
            .any(|row| row["method"] == "session/create")
    })
    .await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    wait_for(|| ids(root.path()).iter().all(|pid| !present(*pid))).await;
    assert!(!root.path().join("persistent-probe").exists());
    assert!(
        !records(root.path())
            .iter()
            .any(|row| row["method"] == "session/send")
    );
}
