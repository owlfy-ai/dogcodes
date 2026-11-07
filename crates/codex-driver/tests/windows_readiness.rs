#![cfg(windows)]

#[tokio::test]
#[ignore = "read-only installed Codex handshake from a consoleless host; no threads or model prompts"]
async fn installed_windows_codex_readiness() {
    use driver_api::{AgentDriver, QuotaProvider};
    use std::os::windows::process::CommandExt;
    const HOST: &str = "DOGCODES_CODEX_CONSOLELESS_READINESS";
    if std::env::var_os(HOST).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "installed_windows_codex_readiness",
                "--ignored",
                "--nocapture",
            ])
            .env(HOST, "1")
            .creation_flags(0x0000_0008)
            .output()
            .unwrap();
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success());
        return;
    }
    let path = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
        .join("dogcodes/cli/codex/codex.cmd");
    let mut previous = None;
    for attempt in 1..=3 {
        let started = std::time::Instant::now();
        let driver = codex_driver::CodexDriver::connect_ready(
            &path,
            agent_domain::InstallationId::new().to_string(),
            "installed".into(),
        )
        .await;
        eprintln!(
            "attempt={attempt} stage=readiness elapsed_ms={} error={:?}",
            started.elapsed().as_millis(),
            driver.as_ref().err()
        );
        let driver = driver.unwrap();
        let (models, quota) = tokio::join!(driver.available_models(), driver.read_quota(None));
        eprintln!(
            "attempt={attempt} stage=metadata elapsed_ms={} models_error={:?} quota_error={:?}",
            started.elapsed().as_millis(),
            models.err(),
            quota.err()
        );
        previous = Some(driver);
    }
    drop(previous);
}
