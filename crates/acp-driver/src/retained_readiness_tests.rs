use super::*;
#[path = "../../driver-api/tests/fixtures/retained_readiness_support.rs"]
mod support;
use support::{Fixture, wait_for};

#[tokio::test]
async fn retained_readiness_codebuddy_retires_the_probe_before_changing_workspace() {
    let fixture = Fixture::new("normal");
    let driver = AcpDriver::connect_ready(
        "codebuddy",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: (fixture.executable.clone()).into(),
            args: vec![],
        },
        None,
    )
    .await
    .unwrap();
    let original = driver
        .shared
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone()
        .unwrap();
    wait_for(|| fixture.starts().len() == 1).await;
    assert_eq!(fixture.requests("session/new"), 1);
    let startup = driver.take_startup_catalogs();
    assert_eq!(
        startup.models.unwrap().default_model.as_deref(),
        Some("fixture")
    );
    assert_eq!(
        startup.permissions.unwrap().default_mode_id.as_deref(),
        Some("default")
    );
    assert!(driver.take_startup_catalogs().models.is_none());
    assert_eq!(fixture.requests("session/new"), 1);
    let session = driver
        .create_session(SessionRequest {
            working_directory: fixture.root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            app_mcps: vec![],
            permission_mode: None,
        })
        .await
        .unwrap();
    original.closed().await.unwrap();
    assert_eq!(
        fixture.starts().len(),
        2,
        "CodeBuddy must rebuild its cwd-bound process"
    );
    assert_eq!(fixture.requests("session/prompt"), 0);
    let retained = driver
        .shared
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone()
        .unwrap();
    driver.shutdown(session).await.unwrap();
    drop(driver);
    retained.closed().await.unwrap();
}
