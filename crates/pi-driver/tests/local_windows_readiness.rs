#![cfg(windows)]

#[tokio::test]
#[ignore = "uses the installed Windows Pi and configured models; no model calls"]
async fn installed_pi_reads_its_configured_models_over_rpc() {
    let executable = std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join(".pi/agent/bin/pi.cmd");
    for executable in [
        executable.clone(),
        std::fs::canonicalize(executable).unwrap(),
    ] {
        let result = pi_driver::PiDriver::probe_ready(&executable).await;
        eprintln!("Pi readiness for {}: {result:?}", executable.display());
        result.expect("installed Pi should read the configured native models over RPC");
    }
}
