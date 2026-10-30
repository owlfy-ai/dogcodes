#![cfg(windows)]

#[tokio::test]
#[ignore = "requires the locally installed Grok Build; no model calls"]
async fn local_grok_reports_readiness_or_an_explicit_login_requirement() {
    let home = std::env::var_os("USERPROFILE").expect("USERPROFILE");
    let launch = acp_driver::AcpLaunch {
        executable: std::path::PathBuf::from(home).join(".grok/bin/grok.exe"),
        args: vec!["agent".into(), "stdio".into()],
    };
    let result = acp_driver::AcpDriver::connect_ready(
        "grok",
        &agent_domain::InstallationId::new().to_string(),
        launch,
        None,
    )
    .await;
    match result {
        Ok(driver) => {
            eprintln!("Grok: authenticated ACP session probe passed");
            drop(driver);
        }
        Err(driver_api::DriverError::NotConfigured(reason)) => {
            eprintln!("Grok: installed, native login required: {reason}");
        }
        Err(error) => panic!("Grok readiness failed: {error:?}"),
    }
}
