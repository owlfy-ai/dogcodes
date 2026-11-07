#![cfg(windows)]

#[tokio::test]
#[ignore = "read-only installed CLI readiness from a detached host; run with a console-window event listener"]
async fn probe_installed_antigravity_from_consoleless_host() {
    use std::os::windows::process::CommandExt;
    const HOST: &str = "DOGCODES_TEST_AGY_DETACHED_READINESS";
    if std::env::var_os(HOST).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "probe_installed_antigravity_from_consoleless_host",
                "--ignored",
                "--nocapture",
            ])
            .env(HOST, "1")
            .env_remove("TERM")
            .creation_flags(0x0000_0008)
            .output()
            .unwrap();
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let path =
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("agy/bin/agy.exe");
    let result = antigravity_driver::AntigravityDriver::probe_ready(
        agent_runtime::TargetExecutable::local(path),
    )
    .await;
    eprintln!("Native readiness result: {result:?}");
    assert!(
        matches!(
            result,
            Ok(()) | Err(driver_api::DriverError::NotConfigured(_))
        ),
        "{result:?}"
    );
}
