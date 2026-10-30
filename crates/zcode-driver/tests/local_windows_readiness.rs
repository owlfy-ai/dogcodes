#![cfg(windows)]

#[tokio::test]
#[ignore = "reads local ZCode account/model readiness; no model calls"]
async fn local_zcode_reports_readiness_or_an_explicit_account_requirement() {
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
    let app_binary =
        std::fs::canonicalize(std::path::PathBuf::from(local).join("Programs/ZCode/ZCode.exe"))
            .unwrap();
    let launch = zcode_driver::ZcodeLaunch {
        cli_script: agent_runtime::electron_resources_directory(&app_binary)
            .unwrap()
            .join("glm/zcode.cjs"),
        app_binary,
    };
    match zcode_driver::ZcodeDriver::probe_ready(&launch).await {
        Ok(()) => eprintln!("ZCode: native protocol and model readiness passed"),
        Err(driver_api::DriverError::NotConfigured(reason)) => {
            eprintln!("ZCode: installed, account/model configuration required: {reason}");
        }
        Err(error) => panic!("ZCode readiness failed: {error:?}"),
    }
}
