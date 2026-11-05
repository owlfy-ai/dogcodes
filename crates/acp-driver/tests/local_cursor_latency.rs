#![cfg(unix)]
use acp_driver::{AcpDriver, AcpLaunch};
use driver_api::{AgentDriver, SessionRequest};
use std::time::Instant;

#[tokio::test]
#[ignore = "uses authenticated local Cursor metadata sessions; no prompts or model calls"]
async fn local_cursor_connection_and_shutdown_latency() {
    let executable = std::env::var("DOGCODES_ACP_BINARY").expect("DOGCODES_ACP_BINARY");
    let started = Instant::now();
    let driver = AcpDriver::connect_ready(
        "cursor",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: executable.into(),
            args: vec!["--disable-auto-update".into(), "acp".into()],
        },
        None,
    )
    .await
    .unwrap();
    println!(
        "cursor_latency readiness_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let startup = driver.take_startup_catalogs();
    assert!(startup.models.is_some());
    let directory = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let session = driver
        .create_selected_session(
            SessionRequest {
                working_directory: directory.path().to_string_lossy().into_owned(),
                additional_directories: vec![],
                app_mcps: vec![],
                permission_mode: None,
            },
            None,
            None,
        )
        .await
        .unwrap();
    println!(
        "cursor_latency session_new_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    driver.shutdown(session).await.unwrap();
    println!(
        "cursor_latency confirmed_shutdown_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
