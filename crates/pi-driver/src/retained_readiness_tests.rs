use super::*;
#[path = "../../driver-api/tests/fixtures/retained_readiness_support.rs"]
mod support;
use std::time::Duration;
use support::{Fixture, wait_for};

async fn ready(fixture: &Fixture) -> PiDriver {
    PiDriver::connect_ready("fixture", fixture.executable.clone(), None)
        .await
        .unwrap()
}
async fn exited(mut cleanup: watch::Receiver<Option<Result<(), DriverError>>>) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Some(result) = cleanup.borrow_and_update().clone() {
                result.unwrap();
                return;
            }
            cleanup.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn retained_readiness_hands_the_original_process_to_catalog_and_closes_on_owner_drop() {
    let fixture = Fixture::new("normal");
    let driver = ready(&fixture).await;
    assert_eq!(fixture.starts().len(), 1);
    let result = driver.model_catalog().await.unwrap();
    assert_eq!(result.models[0].id, "fixture/base");
    assert_eq!(result.models[0].thinking_options.len(), 2);
    assert_eq!(
        fixture.starts().len(),
        1,
        "catalog restarted the ready process"
    );
    assert_eq!(fixture.requests("prompt"), 0);
    driver.model_catalog().await.unwrap();
    assert_eq!(fixture.starts().len(), 1);
    let cleanup = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .cleanup
        .clone();
    drop(driver);
    exited(cleanup).await;
}

#[tokio::test]
async fn retained_readiness_does_not_reuse_another_workspace() {
    let fixture = Fixture::new("normal");
    let driver = ready(&fixture).await;
    let cleanup = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .cleanup
        .clone();
    driver
        .model_catalog_in_workspace(None, fixture.root.path().to_str())
        .await
        .unwrap();
    exited(cleanup).await;
    assert_eq!(fixture.starts().len(), 2);
}

#[tokio::test]
async fn retained_readiness_cancelled_catalog_keeps_admission_until_exit() {
    let fixture = Fixture::new("blocked-catalog");
    let driver = Arc::new(ready(&fixture).await);
    let cleanup = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .cleanup
        .clone();
    let caller = driver.clone();
    let query = tokio::spawn(async move { caller.model_catalog().await });
    wait_for(|| fixture.requests("get_available_models") == 2).await;
    query.abort();
    assert!(query.await.unwrap_err().is_cancelled());
    let _admission = tokio::time::timeout(Duration::from_secs(8), driver.catalog_probe.lock())
        .await
        .unwrap();
    exited(cleanup).await;
    assert_eq!(fixture.starts().len(), 1);
}

#[tokio::test]
async fn retained_readiness_rejects_an_empty_model_configuration() {
    let fixture = Fixture::new("empty");
    assert!(matches!(
        PiDriver::connect_ready("fixture", fixture.executable.clone(), None).await,
        Err(DriverError::NotConfigured(_))
    ));
}

#[tokio::test]
async fn retained_readiness_selected_model_retires_its_mutated_scope() {
    let fixture = Fixture::new("normal");
    let driver = ready(&fixture).await;
    let cleanup = driver
        .ready_connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .cleanup
        .clone();
    driver
        .model_catalog_for(Some("fixture/base"))
        .await
        .unwrap();
    exited(cleanup).await;
    assert!(driver.ready_connection.lock().await.is_none());
    driver.model_catalog().await.unwrap();
    assert_eq!(fixture.starts().len(), 2);
}
