use super::*;
#[path = "../../driver-api/tests/fixtures/retained_readiness_support.rs"]
mod support;
use support::{Fixture, wait_for};

async fn ready(fixture: &Fixture) -> CodexDriver {
    CodexDriver::connect_ready(&fixture.executable, "fixture".into(), "fixture".into())
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
    assert_eq!(fixture.starts().len(), 1);
    assert_eq!(
        driver.model_catalog().await.unwrap().models[0].id,
        "fixture"
    );
    assert_eq!(
        fixture.starts().len(),
        1,
        "catalog restarted the ready process"
    );
    assert_eq!(fixture.requests("initialize"), 1);
    assert_eq!(fixture.requests("thread/start"), 0);
    driver.model_catalog().await.unwrap();
    assert_eq!(fixture.starts().len(), 1);
    assert_eq!(fixture.requests("initialize"), 1);
    assert!(!native.stopping());
    drop(driver);
    tokio::time::timeout(Duration::from_secs(8), native.confirmed())
        .await
        .unwrap();
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
    wait_for(|| fixture.requests("model/list") == 1).await;
    query.abort();
    assert!(query.await.unwrap_err().is_cancelled());
    let _admission = tokio::time::timeout(Duration::from_secs(8), driver.catalog_probe.lock())
        .await
        .unwrap();
    native.confirmed().await;
    assert_eq!(fixture.starts().len(), 1);
}

#[tokio::test]
async fn retained_readiness_rejects_an_unauthenticated_account() {
    let fixture = Fixture::new("unauthenticated");
    assert!(matches!(
        CodexDriver::connect_ready(&fixture.executable, "fixture".into(), "fixture".into()).await,
        Err(DriverError::NotConfigured(_))
    ));
    assert_eq!(fixture.requests("model/list"), 0);
}

#[tokio::test]
async fn retained_readiness_replaces_an_exited_connection() {
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
    native.request_close();
    tokio::time::timeout(Duration::from_secs(8), native.confirmed())
        .await
        .unwrap();
    assert_eq!(
        driver.model_catalog().await.unwrap().models[0].id,
        "fixture"
    );
    assert_eq!(fixture.starts().len(), 2);
}
