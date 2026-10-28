//! Private runtime callbacks; no credentials, browser, service or model calls.
use crate::{
    cleanup_tests::{present, wait_for},
    metadata::tests::{fixture, ids, records},
    *,
};
use std::{path::Path, time::Duration};

async fn configured(mode: &str) -> (tempfile::TempDir, Connection) {
    let (root, launch) = fixture(mode);
    std::fs::create_dir_all(launch.cli_script.parent().unwrap()).unwrap();
    std::fs::write(&launch.cli_script, "").unwrap();
    let builtin = root
        .path()
        .join("Resources/config/provider/zcode-builtin.json");
    std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
    std::fs::write(builtin, "{}").unwrap();
    let mut connection = Connection::spawn(&launch).unwrap();
    connection.refresh_account(true).await.unwrap();
    (root, connection)
}
fn runtime_started(root: &Path) -> bool {
    records(root)
        .iter()
        .any(|row| row["method"] == "account.spawn" && row["params"]["action"] == "runtime")
}

#[tokio::test]
async fn runtime_callback_retires_its_helper_when_original_native_scope_ends() {
    for ending in ["close", "owner", "owner-dropped", "passive"] {
        let (root, mut connection) = configured("runtime-blocked").await;
        let native = connection.native.clone();
        let (controller, observed) = tokio::sync::watch::channel(false);
        if matches!(ending, "owner" | "owner-dropped") {
            connection.observe_owner(observed);
        }
        let request = json!({"fixtureRoot":root.path()});
        let mut query = tokio::spawn(async move { connection.provider_auth(&request).await });
        wait_for(|| runtime_started(root.path())).await;
        match ending {
            "owner" => {
                controller.send_replace(true);
            }
            "owner-dropped" => drop(controller),
            "passive" => {
                native
                    .send(&json!({"method":"fixture/exit"}))
                    .await
                    .unwrap();
            }
            _ => native.request_close(),
        }
        native.closed().await.unwrap();
        let observation = tokio::time::timeout(Duration::from_secs(2), &mut query).await;
        let stopped_with_original = observation.is_ok();
        // Release the old fake if it did not follow the original scope, then
        // confirm cleanup before failing. Never leave the negative case alive.
        let value = if let Ok(value) = observation {
            value.unwrap()
        } else {
            std::fs::write(root.path().join("allow-runtime"), "").unwrap();
            query.await.unwrap()
        };
        wait_for(|| ids(root.path()).iter().all(|pid| !present(*pid))).await;
        assert!(
            stopped_with_original,
            "runtime helper outlived original {ending}"
        );
        assert_eq!(value["headersApplied"], false);
    }
}

#[tokio::test]
async fn runtime_success_keeps_its_original_native_connection_open() {
    let (root, connection) = configured("runtime-normal").await;
    let native = connection.native.clone();
    let value = connection
        .provider_auth(&json!({"fixtureRoot":root.path()}))
        .await;
    let still_running = !native.stopping();
    let helper_ids: Vec<_> = records(root.path())
        .iter()
        .filter(|row| row["method"] == "account.spawn")
        .flat_map(|row| {
            [
                u32::try_from(row["pid"].as_u64().unwrap()).unwrap(),
                u32::try_from(row["child"].as_u64().unwrap()).unwrap(),
            ]
        })
        .collect();
    let helpers_stopped = helper_ids.iter().all(|pid| !present(*pid));
    native.close().await.unwrap();
    assert_eq!(value["headersApplied"], true);
    assert!(still_running);
    assert!(helpers_stopped);
}

#[tokio::test]
async fn already_closed_native_scope_cannot_start_or_authorize_a_runtime_helper() {
    let (root, connection) = configured("runtime-normal").await;
    connection.native.close().await.unwrap();
    let value = connection
        .provider_auth(&json!({"fixtureRoot":root.path()}))
        .await;
    assert_eq!(value["headersApplied"], false);
    assert!(!runtime_started(root.path()));
    assert!(ids(root.path()).iter().all(|pid| !present(*pid)));
}
