use super::*;
use agent_runtime::{ExecutionTarget, OwnedChild, ProcessRequest, SharedTarget};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    sync::{Arc, Mutex},
};

fn key(account: &str, org: &str, scope: &str) -> String {
    format!("acct:{account}|{CLIENT}:{org}:{HOST}:{scope}")
}
const SCOPE: &str = "user:inference user:profile user:file_upload user:sessions:claude_code";
fn entries(account: &str, token: &str, expires: i64) -> Value {
    json!({key(account,"org",SCOPE):{"token":token,"expiresAt":expires,"subscriptionType":"pro"}})
}

#[test]
fn only_current_accounts_unambiguous_live_code_grant_is_eligible() {
    assert!(verified_host_auth(Some("2.1.325")));
    assert!(!verified_host_auth(Some("2.1.218")));
    assert!(!verified_host_auth(None));
    let config = json!({"lastKnownAccountUuid":"current"});
    let file = PathBuf::from("unused");
    let valid = entries("current", "desktop-fixture", 100_000);
    assert_eq!(
        select(&config, &valid, 0, file.clone())
            .unwrap()
            .token
            .as_str(),
        "desktop-fixture"
    );
    for rejected in [
        entries("other", "wrong-account", 100_000),
        entries("current", "expired", 0),
        entries("current", "too-close-to-expiry", 29_999),
        entries("current", "", 100_000),
        json!({key("current","org","user:profile user:inference"):{"token":"chat-only","expiresAt":100_000}}),
    ] {
        assert!(select(&config, &rejected, 0, file.clone()).is_none());
    }
    let mut with_everyday = valid.clone();
    with_everyday[key(
        "current",
        "org",
        "user:inference user:profile user:file_upload",
    )] = json!({"token":"everyday", "expiresAt":100_000});
    assert_eq!(
        select(&config, &with_everyday, 0, file.clone())
            .unwrap()
            .token
            .as_str(),
        "everyday"
    );
    let mut ambiguous = valid;
    ambiguous[key("current", "another-org", SCOPE)] =
        json!({"token":"another-org","expiresAt":100_000});
    assert!(select(&config, &ambiguous, 0, file).is_none());
}

struct Capture {
    home: PathBuf,
    local: bool,
    request: Arc<Mutex<Option<ProcessRequest>>>,
}
impl ExecutionTarget for Capture {
    fn is_local(&self) -> bool {
        self.local
    }
    fn inherited(&self, key: &str) -> Option<OsString> {
        match key {
            "HOME" | "USERPROFILE" => Some(self.home.clone().into_os_string()),
            "ANTHROPIC_API_KEY" => Some("cli-api-fixture".into()),
            "ANTHROPIC_BASE_URL" => Some("https://custom.invalid".into()),
            "CLAUDE_CODE_OAUTH_TOKEN" => Some("cli-token-fixture".into()),
            _ => None,
        }
    }
    fn temp_directory(&self) -> PathBuf {
        self.home.clone()
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        *self.request.lock().unwrap() = Some(request);
        Err(io::ErrorKind::Interrupted.into())
    }
}

// One fixture test owns the private key cache throughout: no system Keychain,
// user credentials, network, or model prompt is used.
#[cfg(unix)]
#[tokio::test]
async fn desktop_precedence_cli_fallback_remote_isolation_and_scoped_renewal() {
    let home = tempfile::tempdir().unwrap();
    let captured = Arc::new(Mutex::new(None));
    let target: SharedTarget = Arc::new(Capture {
        home: home.path().into(),
        local: true,
        request: captured.clone(),
    });
    let executable = TargetExecutable::new(target.clone(), "private-fixture");
    let paths = {
        let root = if cfg!(target_os = "macos") {
            home.path().join("Library/Application Support/Claude")
        } else if cfg!(windows) {
            home.path().join("AppData/Roaming/Claude")
        } else {
            home.path().join(".config/Claude")
        };
        root.join("config.json")
    };
    let expires = chrono::Utc::now().timestamp_millis() + 600_000;
    storage::fixture(
        &paths,
        "account",
        &entries("account", "desktop-fixture", expires),
    )
    .await;
    let mut command = executable.command();
    for key in super::super::environment::AUTH_VARIABLES {
        command.inherit(key);
    }
    let mut lease = apply(&executable, Some("2.1.325"), &mut command)
        .await
        .unwrap();
    let _ = command.spawn();
    let env = captured.lock().unwrap().take().unwrap().env;
    assert_eq!(
        env[&OsString::from("CLAUDE_CODE_OAUTH_TOKEN")],
        "desktop-fixture"
    );
    assert_eq!(env[&OsString::from("ANTHROPIC_API_KEY")], "");
    assert_eq!(env[&OsString::from("ANTHROPIC_BASE_URL")], HOST);
    assert_eq!(env[&OsString::from("CLAUDE_CODE_ORGANIZATION_UUID")], "org");
    assert_eq!(env[&OsString::from("CLAUDE_CODE_OAUTH_CLIENT_ID")], CLIENT);
    assert!(!env.contains_key(&OsString::from("CLAUDE_CODE_OAUTH_REFRESH_TOKEN")));

    assert!(
        lease.refreshed().await.is_none(),
        "refused token must not be handed back unchanged"
    );
    storage::fixture(
        &paths,
        "account",
        &entries("account", "renewed-fixture", expires),
    )
    .await;
    assert_eq!(lease.refreshed().await.unwrap().as_str(), "renewed-fixture");
    // The protocol fixture below checks the actual host callback reply path.
    let script = home.path().join("sink.py");
    tokio::fs::write(&script,"import sys,json\nr=json.loads(sys.stdin.readline())\nassert r['response']['request_id']=='refresh-fixture'\nassert r['response']['response']['accessToken']=='renewed-fixture'\n").await.unwrap();
    let mut sink = TargetCommand::new(agent_runtime::local_target(), "python3");
    sink.arg(&script).inherit("PATH");
    let mut child = sink.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    assert!(respond(Some(&mut lease),&json!({"type":"control_request","request_id":"refresh-fixture","request":{"subtype":"oauth_token_refresh","token_state":"refused"}}),&mut input).await.unwrap());
    drop(input);
    assert!(child.wait().await.unwrap().success());
    assert_eq!(lease.token.as_str(), "renewed-fixture");
    storage::fixture(
        &paths,
        "other-account",
        &entries("other-account", "other-fixture", expires),
    )
    .await;
    assert!(lease.refreshed().await.is_none());
    storage::fixture(
        &paths,
        "account",
        &entries("account", "renewed-fixture", expires),
    )
    .await;
    reject(&lease);
    assert!(
        apply(&executable, Some("2.1.325"), &mut executable.command())
            .await
            .is_none(),
        "rejected desktop bearer falls back on the next explicit process start"
    );
    storage::fixture(
        &paths,
        "account",
        &entries("account", "new-login-fixture", expires),
    )
    .await;
    assert!(
        apply(&executable, Some("2.1.325"), &mut executable.command())
            .await
            .is_some(),
        "a new desktop grant is eligible again"
    );

    let remote = TargetExecutable::new(
        Arc::new(Capture {
            home: home.path().into(),
            local: false,
            request: captured.clone(),
        }),
        "remote-fixture",
    );
    assert!(
        apply(&remote, Some("2.1.325"), &mut remote.command())
            .await
            .is_none()
    );
    storage::fixture(&paths, "account", &entries("account", "expired", 0)).await;
    let mut command = executable.command();
    for key in super::super::environment::AUTH_VARIABLES {
        command.inherit(key);
    }
    assert!(
        apply(&executable, Some("2.1.325"), &mut command)
            .await
            .is_none()
    );
    let _ = command.spawn();
    let env = captured.lock().unwrap().take().unwrap().env;
    let expected: BTreeMap<_, _> = [
        ("ANTHROPIC_API_KEY", "cli-api-fixture"),
        ("ANTHROPIC_BASE_URL", "https://custom.invalid"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "cli-token-fixture"),
    ]
    .into_iter()
    .map(|(k, v)| (OsString::from(k), OsString::from(v)))
    .collect();
    assert_eq!(env, expected);
}
