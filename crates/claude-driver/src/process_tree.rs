//! Close only the process group created for this native session.
use driver_api::DriverError;
use tokio::{process::Child, time::Duration};

fn failure() -> DriverError {
    DriverError::Disconnected("Claude process shutdown could not be confirmed".into())
}

pub(super) async fn close(child: &mut Child, pid: Option<u32>) -> Result<(), DriverError> {
    #[cfg(unix)]
    {
        use nix::{sys::signal::Signal, unistd::Pid};
        let group = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .ok_or_else(failure)?;
        let group = Pid::from_raw(group);
        group_signal(child, group, Some(Signal::SIGTERM))?;
        let start = tokio::time::Instant::now();
        loop {
            child.try_wait().map_err(|_| failure())?;
            if !group_signal(child, group, None)? {
                break;
            }
            if start.elapsed() >= Duration::from_secs(2) {
                return Err(failure());
            }
            if start.elapsed() >= Duration::from_millis(300) {
                group_signal(child, group, Some(Signal::SIGKILL))?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    #[cfg(windows)]
    {
        let pid = pid.ok_or_else(failure)?;
        let status = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new("taskkill.exe")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
        if !status.success() {
            return Err(failure());
        }
    }
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
    Ok(())
}

#[cfg(unix)]
fn group_signal(
    child: &mut Child,
    group: nix::unistd::Pid,
    signal: Option<nix::sys::signal::Signal>,
) -> Result<bool, DriverError> {
    use nix::{errno::Errno, sys::signal::killpg};
    // macOS may briefly report EPERM even after the exited leader is reaped.
    // Reap and retry; if it persists, the caller's deadline still fails closed.
    // EPERM itself is never evidence that the owned group stopped.
    child.try_wait().map_err(|_| failure())?;
    let result = match killpg(group, signal) {
        Err(Errno::EPERM) => {
            child.try_wait().map_err(|_| failure())?;
            killpg(group, signal)
        }
        result => result,
    };
    match result {
        Ok(()) | Err(Errno::EPERM) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(_) => Err(failure()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

    #[tokio::test]
    #[ignore = "requires the local Claude CLI; control requests only, no model quota"]
    async fn native_idle_shutdown_is_confirmed_without_model_requests() {
        use driver_api::{AgentDriver, SessionRequest, SessionResumeCapability};
        let executable =
            std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/claude");
        let driver = crate::ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..3 {
            let session = driver
                .create_selected_session(
                    SessionRequest {
                        working_directory: directory.path().to_string_lossy().into_owned(),
                        additional_directories: vec![],
                        history_mcp: None,
                        permission_mode: None,
                    },
                    Some("sonnet"),
                    None,
                )
                .await
                .unwrap();
            driver.shutdown(session).await.unwrap();
        }
    }

    #[tokio::test]
    async fn closes_owned_descendants_without_stopping_other_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("child.pid");
        let mut other = tokio::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut child = tokio::process::Command::new("/usr/bin/python3")
            .args([
                "-c",
                r"
import subprocess,signal,time,sys
child=subprocess.Popen(['sleep','60'])
def stop(sig,frame):
 child.wait()
 sys.exit(0)
signal.signal(signal.SIGTERM,stop)
open(sys.argv[1],'w').write(str(child.pid))
while True:time.sleep(1)
",
            ])
            .arg(&record)
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !record.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let descendant: i32 = std::fs::read_to_string(record).unwrap().parse().unwrap();
        let pid = child.id();
        close(&mut child, pid).await.unwrap();
        assert_eq!(kill(Pid::from_raw(descendant), None), Err(Errno::ESRCH));
        assert!(other.try_wait().unwrap().is_none());
        other.kill().await.unwrap();
    }
}
