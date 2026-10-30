#![cfg(windows)]

#[tokio::test]
#[ignore = "uses the installed Windows Cursor CLI; no model calls"]
async fn local_cursor_uses_its_own_launcher_and_reports_readiness() {
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    let executable = std::fs::canonicalize(
        std::path::PathBuf::from(local).join("cursor-agent/cursor-agent.cmd"),
    )
    .unwrap();
    let launch = acp_driver::AcpLaunch {
        executable,
        args: vec!["--disable-auto-update".into(), "acp".into()],
    };
    match acp_driver::AcpDriver::connect_ready(
        "cursor",
        &agent_domain::InstallationId::new().to_string(),
        launch,
        None,
    )
    .await
    {
        Ok(driver) => {
            eprintln!("Cursor: authenticated ACP readiness passed");
            drop(driver);
        }
        Err(driver_api::DriverError::NotConfigured(reason)) => {
            eprintln!("Cursor: installed, native login required: {reason}");
        }
        Err(error) => panic!("Cursor readiness failed: {error:?}"),
    }
}
