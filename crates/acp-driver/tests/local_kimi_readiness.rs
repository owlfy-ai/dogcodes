#![cfg(windows)]

#[tokio::test]
#[ignore = "uses the installed Windows Kimi CLI; no model calls"]
async fn local_kimi_reports_readiness_or_an_explicit_login_requirement() {
    let home = std::env::var_os("USERPROFILE").expect("USERPROFILE");
    let executable =
        std::fs::canonicalize(std::path::PathBuf::from(home).join(".kimi-code/bin/kimi.exe"))
            .unwrap();
    let launch = acp_driver::AcpLaunch {
        executable,
        args: vec!["acp".into()],
    };
    match acp_driver::AcpDriver::connect_ready(
        "kimi",
        &agent_domain::InstallationId::new().to_string(),
        launch,
        None,
    )
    .await
    {
        Ok(driver) => {
            eprintln!("Kimi: authenticated ACP readiness passed");
            drop(driver);
        }
        Err(driver_api::DriverError::NotConfigured(reason)) => {
            eprintln!("Kimi: installed, native login required: {reason}");
        }
        Err(error) => panic!("Kimi readiness failed: {error:?}"),
    }
}
