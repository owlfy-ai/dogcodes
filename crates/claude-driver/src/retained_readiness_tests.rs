use super::*;
#[path = "../../driver-api/tests/fixtures/retained_readiness_support.rs"]
mod support;
use std::time::Duration;
use support::{Fixture, wait_for};

async fn ready(fixture: &Fixture) -> ClaudeDriver {
    ClaudeDriver::connect_ready(
        "claude-code",
        &InstallationId::new().to_string(),
        fixture.executable.clone(),
        None,
        SessionResumeCapability::Supported,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn retained_readiness_hands_the_original_process_to_catalog_and_closes_on_owner_drop() {
    let fixture = Fixture::new("normal");
    let driver = ready(&fixture).await;
    let native = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone();
    assert!(!native.stopping());
    let startup = driver.take_startup_catalogs();
    assert!(
        startup
            .models
            .unwrap()
            .models
            .iter()
            .any(|m| m.id == "fixture")
    );
    assert!(
        startup
            .permissions
            .unwrap()
            .modes
            .iter()
            .any(|m| m.id == "auto")
    );
    assert_eq!(fixture.requests("initialize"), 1);
    assert!(
        driver
            .model_catalog()
            .await
            .unwrap()
            .models
            .iter()
            .any(|m| m.id == "fixture")
    );
    assert_eq!(
        fixture.starts().len(),
        1,
        "catalog restarted the ready process"
    );
    assert_eq!(fixture.requests("initialize"), 2);
    assert_eq!(fixture.requests("prompt"), 0);
    driver.permission_catalog(None).await.unwrap();
    assert_eq!(fixture.starts().len(), 1);
    assert_eq!(fixture.requests("initialize"), 3);
    assert!(!native.stopping());
    drop(driver);
    tokio::time::timeout(Duration::from_secs(8), native.confirmed())
        .await
        .unwrap();
}

#[tokio::test]
async fn retained_readiness_does_not_reuse_another_workspace() {
    let fixture = Fixture::new("normal");
    let driver = ready(&fixture).await;
    let native = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone();
    driver
        .model_catalog_in_workspace(None, fixture.root.path().to_str())
        .await
        .unwrap();
    native.confirmed().await;
    assert_eq!(fixture.starts().len(), 2);
}

#[tokio::test]
async fn retained_readiness_cancelled_catalog_keeps_admission_until_exit() {
    let fixture = Fixture::new("blocked-catalog");
    let driver = Arc::new(ready(&fixture).await);
    let native = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone();
    let caller = driver.clone();
    let query = tokio::spawn(async move { caller.model_catalog().await });
    wait_for(|| fixture.requests("initialize") == 2).await;
    query.abort();
    assert!(query.await.unwrap_err().is_cancelled());
    let _admission = tokio::time::timeout(Duration::from_secs(8), driver.catalog_probe.lock())
        .await
        .unwrap();
    native.confirmed().await;
    assert_eq!(fixture.starts().len(), 1);
}
