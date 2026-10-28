use super::*;
use std::{os::unix::fs::PermissionsExt, time::Duration};

struct Fixture {
    root: Arc<tempfile::TempDir>,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = Arc::new(tempfile::tempdir().unwrap());
        let executable = root.path().join("cursor-fixture");
        std::fs::write(
            &executable,
            include_str!("../../tests/fixtures/cursor_tui.py"),
        )
        .unwrap();
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.path().join("mode"), mode).unwrap();
        Self { root }
    }
    fn ids(&self) -> Vec<u32> {
        std::fs::read(self.root.path().join("pids.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only clean immutable private fixture PIDs whose command still names
        // this unique directory. Never signal a cached PID after it was reused.
        for pid in self.ids() {
            let command = std::process::Command::new("ps")
                .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&command.stdout).contains(self.root.path().to_str().unwrap())
            {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}
fn alive(pid: u32) -> bool {
    let state = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(9), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn query(
    root: Arc<tempfile::TempDir>,
    admission: Arc<tokio::sync::Mutex<()>>,
) -> Result<QuotaSnapshot, DriverError> {
    let executable = root.path().join("cursor-fixture");
    #[cfg(target_os = "macos")]
    {
        super::read(&executable, root, admission).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut command = tokio::process::Command::new(executable);
        command
            .args(["--disable-auto-update", "--mode", "ask", "--trust"])
            .current_dir(root.path())
            .env("TERM", "xterm-256color");
        super::read_owned(command, root, admission).await
    }
}

#[tokio::test]
async fn cancellation_retires_terminal_cli_and_descendant_before_admission_is_released() {
    let first = Fixture::new("blocked");
    let next = Fixture::new("normal");
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let task = tokio::spawn(query(first.root.clone(), gate.clone()));
    until(|| first.ids().len() == 2).await;
    let ids = first.ids();
    assert!(ids.iter().copied().all(alive));
    let following = tokio::spawn(query(next.root.clone(), gate.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(next.ids().is_empty());
    assert!(gate.try_lock().is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    until(|| !next.ids().is_empty()).await;
    assert!(ids.iter().copied().all(|pid| !alive(pid)));
    let snapshot = tokio::time::timeout(Duration::from_secs(9), following)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.windows.len(), 2);
    assert!(next.ids().iter().copied().all(|pid| !alive(pid)));
    assert!(gate.try_lock().is_ok());
}

#[tokio::test]
async fn normal_and_invalid_terminal_exit_retire_the_original_tree_before_returning() {
    for mode in ["normal", "invalid"] {
        let fixture = Fixture::new(mode);
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let result = tokio::time::timeout(
            Duration::from_secs(9),
            query(fixture.root.clone(), gate.clone()),
        )
        .await
        .unwrap();
        assert_eq!(result.is_ok(), mode == "normal");
        assert!(fixture.ids().iter().copied().all(|pid| !alive(pid)));
        assert!(gate.try_lock().is_ok());
        if mode == "normal" {
            let trace =
                std::fs::read_to_string(fixture.root.path().join("commands.jsonl")).unwrap();
            let commands: Vec<String> = trace
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(commands, ["/usage", "\r", "\u{1b}", "/quit", "\r"]);
        }
    }
}

#[tokio::test]
async fn cancelled_admission_waiter_never_later_starts_a_terminal() {
    let fixture = Fixture::new("normal");
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let held = gate.clone().lock_owned().await;
    let task = tokio::spawn(query(fixture.root.clone(), gate.clone()));
    tokio::time::sleep(Duration::from_millis(30)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(held);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fixture.ids().is_empty());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_quota_cache_does_not_restart_the_terminal_after_success() {
    use crate::{AcpDriver, AcpLaunch, QuotaProvider, SessionResumeCapability};
    let fixture = Fixture::new("normal");
    let driver = AcpDriver::new(
        "cursor",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: fixture.root.path().join("cursor-fixture"),
            args: vec![],
        },
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    driver
        .shared
        .quota
        .cursor_directory
        .set(fixture.root.clone())
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(9), driver.read_quota(None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.windows.len(), 2);
    let ids = fixture.ids();
    assert!(ids.iter().copied().all(|pid| !alive(pid)));
    let cached = driver.read_quota(None).await.unwrap();
    assert_eq!(first, cached);
    assert_eq!(fixture.ids(), ids);
    assert!(driver.shared.connection.lock().await.is_none());
}
