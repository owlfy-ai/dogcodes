//! An agent on another machine reports the account signed in over there.
use super::*;
use agent_runtime::{ExecutionTarget, OwnedChild, ProcessRequest, SharedTarget, local_target};
use std::{collections::BTreeMap, ffi::OsString, io, os::unix::fs::PermissionsExt, path::Path};

/// Runs here, but counts as another machine with only the values it was given.
struct Elsewhere(BTreeMap<String, String>);

impl ExecutionTarget for Elsewhere {
    fn is_local(&self) -> bool {
        false
    }
    fn inherited(&self, name: &str) -> Option<OsString> {
        self.0.get(name).map(OsString::from)
    }
    fn temp_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        local_target().spawn(request)
    }
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn elsewhere(bin: &Path) -> SharedTarget {
    Arc::new(Elsewhere(BTreeMap::from([(
        "PATH".to_owned(),
        format!("{}:/usr/bin:/bin", bin.display()),
    )])))
}

fn signed_in_with_a_file(bin: &Path, home: &Path) -> SharedTarget {
    Arc::new(Elsewhere(BTreeMap::from([
        (
            "PATH".to_owned(),
            format!("{}:/usr/bin:/bin", bin.display()),
        ),
        ("HOME".to_owned(), home.display().to_string()),
        ("AGENT_CLI_CREDENTIAL_STORE".to_owned(), "file".to_owned()),
    ])))
}

#[tokio::test]
async fn kimi_usage_is_fetched_on_the_machine_its_endpoint_listens_on() {
    let bin = tempfile::tempdir().unwrap();
    script(
        &bin.path().join("kimi"),
        "echo \"Serving at http://127.0.0.1:43210/#token=target-token\"\nexec sleep 30",
    );
    script(
        &bin.path().join("curl"),
        "echo \"$@\" > \"$0.args\"\ncat > \"$0.stdin\"\nprintf '{\"from\":\"target\"}'",
    );
    let executable = TargetExecutable::new(elsewhere(bin.path()), bin.path().join("kimi"));
    let permit = Arc::new(tokio::sync::Mutex::new(())).lock_owned().await;

    let payload = read_kimi_web_usage(&executable, permit).await.unwrap();

    assert_eq!(payload["from"], "target");
    let args = std::fs::read_to_string(bin.path().join("curl.args")).unwrap();
    assert!(args.contains("http://127.0.0.1:43210/api/v1/oauth/usage"));
    assert!(!args.contains("target-token"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("Bearer target-token"));
}

#[tokio::test]
async fn cursor_quota_uses_the_login_stored_on_the_target() {
    let bin = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // A Mac keeps it directly under home, anything else under the config directory.
    for folder in [".cursor", ".config/cursor"] {
        let folder = home.path().join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("auth.json"),
            r#"{"accessToken":"target-token"}"#,
        )
        .unwrap();
    }
    script(
        &bin.path().join("curl"),
        r#"cat >> "$0.stdin"
case "$*" in
*GetCurrentPeriodUsage*) printf '{"planUsage":{"autoPercentUsed":25,"apiPercentUsed":50}}\n200' ;;
*) printf '{"planInfo":{"planName":"Pro"}}\n200' ;;
esac"#,
    );
    let target = signed_in_with_a_file(bin.path(), home.path());

    let quota = cursor_quota::read(&target).await.unwrap();

    assert_eq!(quota.windows[0].used_percent, Some(25.0));
    assert_eq!(quota.plan_type.as_deref(), Some("Pro"));
    let config = std::fs::read_to_string(bin.path().join("curl.stdin")).unwrap();
    assert!(config.contains("Bearer target-token"));
}

#[tokio::test]
async fn cursor_without_a_login_on_the_target_says_so() {
    let bin = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let target = signed_in_with_a_file(bin.path(), home.path());
    assert!(matches!(
        cursor_quota::read(&target).await,
        Err(DriverError::NotConfigured(_))
    ));
}
