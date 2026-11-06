//! An agent on another machine reports the account signed in over there: the shim reads
//! it, and the driver only makes sense of the answer.
use super::*;
use agent_runtime::{ExecutionTarget, OwnedChild, ProcessRequest, SharedTarget, local_target};
use std::{ffi::OsString, io, os::unix::fs::PermissionsExt, path::Path};

struct Elsewhere(PathBuf);

impl ExecutionTarget for Elsewhere {
    fn is_local(&self) -> bool {
        false
    }
    fn inherited(&self, _: &str) -> Option<OsString> {
        None
    }
    fn temp_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn shim_program(&self) -> Option<String> {
        Some(self.0.display().to_string())
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        local_target().spawn(request)
    }
}

fn shim_answering(directory: &Path, reply: &str) -> SharedTarget {
    let shim = directory.join("shim");
    std::fs::write(
        &shim,
        format!("#!/bin/sh\ncat > \"$0.request\"\ncat <<'REPLY'\n{reply}\nREPLY\n"),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    Arc::new(Elsewhere(shim))
}

#[tokio::test]
async fn cursor_quota_comes_from_the_shim_on_its_machine() {
    let directory = tempfile::tempdir().unwrap();
    let target = shim_answering(
        directory.path(),
        r#"{"Ok":{"usage":{"planUsage":{"autoPercentUsed":25,"apiPercentUsed":50}},"plan":{"planInfo":{"planName":"Pro"}}}}"#,
    );
    let quota = cursor_quota::read(&target).await.unwrap();
    assert_eq!(quota.windows[0].used_percent, Some(25.0));
    assert_eq!(quota.plan_type.as_deref(), Some("Pro"));
    let request = std::fs::read_to_string(directory.path().join("shim.request")).unwrap();
    assert!(request.contains("cursor_usage"));
}

#[tokio::test]
async fn a_missing_login_over_there_reads_as_not_configured() {
    let directory = tempfile::tempdir().unwrap();
    let target = shim_answering(
        directory.path(),
        r#"{"Err":{"kind":"login","message":"sign in"}}"#,
    );
    assert!(matches!(
        cursor_quota::read(&target).await,
        Err(DriverError::NotConfigured(_))
    ));
}
