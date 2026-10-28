use crate::{
    cleanup_tests::{driver, events, native, present, prompt, session, wait_for},
    *,
};
use std::{path::Path, time::Duration};

pub(crate) fn fixture(mode: &str) -> (tempfile::TempDir, ZcodeLaunch) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("metadata-mode"), mode).unwrap();
    let launch = ZcodeLaunch {
        app_binary: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog-runtime"),
        cli_script: root.path().join("Resources/glm/zcode.cjs"),
    };
    (root, launch)
}
pub(crate) fn records(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("catalog.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn workers(root: &Path) -> Vec<u32> {
    records(root)
        .iter()
        .filter(|row| row["method"] == "native.spawn")
        .filter_map(|row| row["pid"].as_u64().map(|pid| u32::try_from(pid).unwrap()))
        .collect()
}
pub(crate) fn ids(root: &Path) -> Vec<u32> {
    records(root)
        .iter()
        .filter(|row| row["method"] == "native.spawn" || row["method"] == "account.spawn")
        .flat_map(|row| {
            [
                u32::try_from(row["pid"].as_u64().unwrap()).unwrap(),
                u32::try_from(row["child"].as_u64().unwrap()).unwrap(),
            ]
        })
        .collect()
}

#[tokio::test]
async fn model_query_does_not_wait_for_or_reconfigure_the_active_turn_connection() {
    let (root, launch) = fixture("direct");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let original = native(&driver).await;
    let mut stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        DriverEvent::TurnStarted { .. }
    ));
    let query = tokio::time::timeout(Duration::from_secs(2), driver.model_catalog()).await;
    let original_alive = !original.stopping();
    driver.interrupt(&handle).await.unwrap();
    assert!(
        events(stream)
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
    drop(driver);
    original.closed().await.unwrap();
    assert!(
        query.is_ok(),
        "model menu waited for the active task's connection"
    );
    assert_eq!(query.unwrap().unwrap().models.len(), 1);
    assert!(original_alive);
}

#[tokio::test]
async fn deferred_catalog_session_closes_and_process_scope_exits_before_result() {
    let (root, launch) = fixture("modern");
    let driver = driver(launch);
    let catalog = driver.model_catalog().await.unwrap();
    assert_eq!(catalog.models.len(), 1);
    assert!(driver.connection.lock().await.is_none());
    assert!(driver.sessions.lock().unwrap().is_empty());
    assert_eq!(workers(root.path()).len(), 1);
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    let rows = records(root.path());
    for method in ["workspace/readState", "session/create", "session/close"] {
        assert_eq!(rows.iter().filter(|row| row["method"] == method).count(), 1);
    }
    assert!(!rows.iter().any(|row| row["method"] == "session/send"));
    assert!(
        rows.iter()
            .any(|row| row["method"] == "session/close" && row["params"]["sessionId"] == "probe")
    );
}

#[tokio::test]
async fn cancelled_catalog_retains_admission_until_original_scope_exits_then_queued_probe_runs() {
    let (root, launch) = fixture("create-stuck");
    let driver = driver(launch);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.model_catalog().await });
    wait_for(|| {
        records(root.path())
            .iter()
            .any(|row| row["method"] == "session/create")
    })
    .await;
    let original = ids(root.path());
    let next_driver = driver.clone();
    let next = tokio::spawn(async move { next_driver.model_catalog().await });
    std::fs::write(root.path().join("metadata-mode"), "direct").unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let result = tokio::time::timeout(Duration::from_secs(8), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.models.len(), 1);
    let spawns: Vec<_> = records(root.path())
        .into_iter()
        .filter(|row| row["method"] == "native.spawn")
        .collect();
    assert_eq!(spawns.len(), 2);
    assert_eq!(
        spawns[1]["params"]["priorAlive"],
        json!([]),
        "replacement started before original cleanup"
    );
    assert!(original.iter().all(|pid| !present(*pid)));
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    assert!(driver.connection.lock().await.is_none());
}

#[tokio::test]
async fn a_cancelled_queued_catalog_never_starts_a_second_helper() {
    let (root, launch) = fixture("create-stuck");
    let driver = driver(launch);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.model_catalog().await });
    wait_for(|| {
        records(root.path())
            .iter()
            .any(|row| row["method"] == "session/create")
    })
    .await;
    let queued_driver = driver.clone();
    let queued = tokio::spawn(async move { queued_driver.model_catalog().await });
    tokio::task::yield_now().await;
    queued.abort();
    assert!(queued.await.unwrap_err().is_cancelled());
    std::fs::write(root.path().join("allow-create"), "").unwrap();
    assert_eq!(first.await.unwrap().unwrap().models.len(), 1);
    assert_eq!(workers(root.path()).len(), 1);
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    assert!(driver.catalog_probe.try_lock().is_ok());
}

#[tokio::test]
async fn failed_catalog_retires_its_private_process_and_does_not_poison_the_next_query() {
    let (root, launch) = fixture("missing-id");
    let driver = driver(launch);
    assert!(driver.model_catalog().await.is_err());
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    std::fs::write(root.path().join("metadata-mode"), "direct").unwrap();
    assert_eq!(driver.model_catalog().await.unwrap().models.len(), 1);
    assert_eq!(workers(root.path()).len(), 2);
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn model_probe_confirms_its_own_account_overlay_and_keeps_the_business_connection_unchanged()
{
    let (root, launch) = fixture("modern");
    std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
    std::fs::write(&launch.cli_script, "").unwrap();
    let builtin = root
        .path()
        .join("Resources/config/provider/zcode-builtin.json");
    std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
    std::fs::write(builtin, "{}").unwrap();
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let original = native(&driver).await;
    let observed = driver
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .next_request_id;
    assert!(observed > 1);
    let result = driver.model_catalog().await.unwrap();
    assert_eq!(result.models.len(), 1);
    assert_eq!(
        driver
            .connection
            .lock()
            .await
            .as_ref()
            .unwrap()
            .next_request_id,
        observed
    );
    assert!(!original.stopping());
    let roots = workers(root.path());
    assert_eq!(roots.len(), 2);
    let rows = records(root.path());
    for pid in roots {
        let methods: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["pid"] == pid
                    && !matches!(
                        row["method"].as_str(),
                        Some("native.spawn" | "account.spawn")
                    )
            })
            .filter_map(|row| row["method"].as_str())
            .collect();
        assert_eq!(methods[0], "provider/updateAccountConfig");
        assert_eq!(
            methods
                .iter()
                .filter(|method| **method == "provider/updateAccountConfig")
                .count(),
            1
        );
    }
    driver.shutdown(handle).await.unwrap();
    drop(driver);
    original.closed().await.unwrap();
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn cancelling_an_account_probe_stops_both_original_helpers_before_the_next_query() {
    let (root, launch) = fixture("account-stuck");
    std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
    std::fs::write(&launch.cli_script, "").unwrap();
    let builtin = root
        .path()
        .join("Resources/config/provider/zcode-builtin.json");
    std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
    std::fs::write(builtin, "{}").unwrap();
    let driver = driver(launch);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.model_catalog().await });
    wait_for(|| {
        records(root.path())
            .iter()
            .any(|row| row["method"] == "account.spawn")
    })
    .await;
    let original = ids(root.path());
    assert_eq!(original.len(), 4);
    let next_driver = driver.clone();
    let next = tokio::spawn(async move { next_driver.model_catalog().await });
    std::fs::write(root.path().join("metadata-mode"), "direct").unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let result = tokio::time::timeout(Duration::from_secs(8), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.models.len(), 1);
    let spawns: Vec<_> = records(root.path())
        .into_iter()
        .filter(|row| row["method"] == "native.spawn")
        .collect();
    assert_eq!(spawns.len(), 2);
    assert_eq!(spawns[1]["params"]["priorAlive"], json!([]));
    assert!(original.iter().all(|pid| !present(*pid)));
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
    assert!(
        !records(root.path())
            .iter()
            .any(|row| row["pid"] == spawns[0]["pid"] && row["method"] == "session/create")
    );
}
