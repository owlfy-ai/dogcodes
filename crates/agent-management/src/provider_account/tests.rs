use super::*;
use serde_json::json;
use std::path::Path;

fn environment(root: &Path) -> Environment {
    HashMap::from([
        ("HOME".into(), root.to_string_lossy().into()),
        (
            "XDG_DATA_HOME".into(),
            root.join("data").to_string_lossy().into(),
        ),
    ])
}
fn write(root: &Path, relative: &str, value: &Value) {
    let p = root.join(relative);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, value.to_string()).unwrap();
}
fn agent(provider: &str) -> Agent {
    Agent {
        provider: provider.into(),
        executable: format!("/fixture/{provider}"),
        version: None,
    }
}
async fn discovered(provider: &str, env: &Environment) -> Vec<Credential> {
    credentials::discover(&agent(provider), env)
        .await
        .map_err(|failure| failure.message)
        .unwrap()
}

#[cfg(unix)]
mod opencode {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// An `OpenCode` that answers `auth export` with this, and fails at anything else.
    fn installed(root: &Path, version: Option<&str>, exported: Option<&Value>) -> Agent {
        let answer = exported.map_or_else(
            || "echo 'usage: opencode <subcommand>'\nexit 1".to_owned(),
            |value| {
                let file = root.join("export.json");
                std::fs::write(&file, value.to_string()).unwrap();
                format!("cat '{}'", file.display())
            },
        );
        let executable = root.join("opencode");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\n[ \"$*\" = 'auth export --standalone' ] || exit 9\n{answer}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Agent {
            provider: "opencode".into(),
            executable: executable.to_string_lossy().into(),
            version: version.map(str::to_owned),
        }
    }
    fn entry(id: &str, integration: &str, active: bool, value: &Value) -> Value {
        json!({"id":id,"integrationID":integration,"label":id,"active":active,"value":value})
    }

    #[tokio::test]
    async fn a_go_subscription_uses_the_signed_in_account_without_its_refresh_token() {
        let root = tempfile::tempdir().unwrap();
        let exported = json!([entry(
            "account",
            "opencode",
            true,
            &json!({
                "type": "oauth",
                "access": "access-private",
                "refresh": "refresh-private",
                "metadata": {"server": "https://console.opencode.ai", "orgID": "org-private"}
            })
        )]);
        let agent = installed(root.path(), Some("2.2.5"), Some(&exported));
        let found = credentials::discover(&agent, &environment(root.path()))
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        assert_eq!(found[0].key, "access-private");
        assert_eq!(found[0].org_id.as_deref(), Some("org-private"));
        assert_eq!(found[0].endpoint, "https://opencode.ai/zen/go/v1/usage");
    }

    #[tokio::test]
    async fn the_active_key_wins_whatever_its_order_and_old_files_stay_out_of_it() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        write(
            root.path(),
            "data/opencode/auth.json",
            &json!({"opencode-go":{"type":"api","key":"legacy-private"}}),
        );
        let exported = json!([
            entry(
                "new",
                "opencode-go",
                false,
                &json!({"type":"api","key":"inactive-private"})
            ),
            entry(
                "old",
                "opencode-go",
                true,
                &json!({"type":"key","key":"active-private"})
            ),
            entry(
                "zen",
                "opencode",
                true,
                &json!({"type":"key","key":"zen-private"})
            ),
        ]);
        let agent = installed(root.path(), Some("2.2.5"), Some(&exported));
        let found = credentials::discover(&agent, &env)
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        assert_eq!(found[0].key, "active-private");
        let exported = json!([exported[2]]);
        let agent = installed(root.path(), Some("2.2.5"), Some(&exported));
        let failure = credentials::discover(&agent, &env)
            .await
            .err()
            .unwrap()
            .message;
        assert_eq!(failure, "opencode_go_not_configured");
    }

    #[tokio::test]
    async fn only_a_version_before_the_command_falls_back_to_its_file() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        write(
            root.path(),
            "data/opencode/auth.json",
            &json!({"opencode-go":{"type":"api","key":"legacy-private"}}),
        );
        let old = installed(root.path(), Some("1.15.0"), None);
        let found = credentials::discover(&old, &env)
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        assert_eq!(found[0].key, "legacy-private");
        let failing = installed(root.path(), Some("2.2.5"), None);
        let failure = credentials::discover(&failing, &env).await.err().unwrap();
        assert_eq!(failure.message, "OpenCode credentials could not be read");
    }

    #[tokio::test]
    async fn an_old_file_doesnt_mistake_zen_or_oauth_for_a_go_key() {
        let root = tempfile::tempdir().unwrap();
        let env = environment(root.path());
        let old = installed(root.path(), Some("1.15.0"), None);
        write(
            root.path(),
            "data/opencode/auth.json",
            &json!({"opencode":{"type":"api","key":"zen-private"},"opencode-go":{"type":"api","key":"go-private"}}),
        );
        let found = credentials::discover(&old, &env)
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        assert_eq!(found[0].key, "go-private");
        write(
            root.path(),
            "data/opencode/auth.json",
            &json!({"opencode":{"type":"api","key":"zen-private"},"opencode-go":{"type":"oauth","access":"oauth-private"}}),
        );
        let failure = credentials::discover(&old, &env).await.err().unwrap();
        assert_eq!(failure.message, "opencode_go_not_configured");
    }

    #[tokio::test]
    async fn a_dedicated_key_needs_nothing_from_opencode() {
        let root = tempfile::tempdir().unwrap();
        let mut env = environment(root.path());
        env.insert("OPENCODE_GO_API_KEY".into(), "go-private".into());
        let found = credentials::discover(&agent("opencode"), &env)
            .await
            .map_err(|failure| failure.message)
            .unwrap();
        assert_eq!(found[0].key, "go-private");
    }
}

#[tokio::test]
async fn pi_uses_configured_provider_origins_without_running_key_commands() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    write(
        root.path(),
        ".pi/agent/auth.json",
        &json!({"deepseek":{"type":"api_key","key":"deep-private"},"relay":{"type":"api_key","key":"native-private"}}),
    );
    write(
        root.path(),
        ".pi/agent/models.json",
        &json!({"providers":{
            "relay":{"apiKey":"config-private","baseUrl":"https://relay.example/anthropic/v1?token=secret"},
            "command":{"apiKey":"!touch /tmp/should-not-run","baseUrl":"https://command.example"},
            "local-file":{"apiKey":"no","baseUrl":"file:///tmp/secret"}
        }}),
    );
    let found = discovered("pi", &env).await;
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].endpoint, "https://api.deepseek.com/user/balance");
    assert_eq!(found[1].endpoint, "https://relay.example/v1/usage");
    assert_eq!(found[1].key, "native-private");
}

#[tokio::test]
async fn dsh_saved_reference_wins_over_environment_and_browser_grants_stay_opaque() {
    let root = tempfile::tempdir().unwrap();
    let mut env = environment(root.path());
    env.insert("DEEPSEEK_API_KEY".into(), "env-private".into());
    std::fs::create_dir_all(root.path().join(".dsh")).unwrap();
    std::fs::write(root.path().join(".dsh/.credentials.yaml"), "version: 1\nrefs:\n  DEEPSEEK_API_KEY: stored-private\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {secret: browser-private}\n").unwrap();
    assert_eq!(
        discovered("deepseek-harness", &env).await[0].key,
        "stored-private"
    );
    std::fs::write(root.path().join(".dsh/.credentials.yaml"), "version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {secret: browser-private}\n").unwrap();
    env.remove("DEEPSEEK_API_KEY");
    assert!(
        credentials::discover(&agent("deepseek-harness"), &env)
            .await
            .is_err()
    );
}

fn dsh_platform_grant(root: &Path, issuer: &str) {
    std::fs::create_dir_all(root.join(".dsh")).unwrap();
    std::fs::write(root.join(".dsh/.credentials.yaml"), format!("version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {{secret: browser-private}}\n  deepseek-account-platform/default:\n    kind: grant\n    payload: {{version: 1, token: grant-private, issuer: '{issuer}'}}\n")).unwrap();
}

#[tokio::test]
async fn dsh_platform_grant_is_a_fallback_bound_to_its_official_issuer() {
    let root = tempfile::tempdir().unwrap();
    let mut env = environment(root.path());
    dsh_platform_grant(root.path(), "https://platform.deepseek.com");
    let found = discovered("deepseek-harness", &env).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::DeepSeekPlatform);
    assert_eq!(found[0].key, "grant-private");
    assert_eq!(
        found[0].endpoint,
        "https://platform.deepseek.com/api/v0/users/get_user_summary"
    );
    env.insert("DEEPSEEK_API_KEY".into(), "env-private".into());
    let found = discovered("deepseek-harness", &env).await;
    assert_eq!(found[0].kind, Kind::DeepSeek);
    assert_eq!(found[0].key, "env-private");
    env.remove("DEEPSEEK_API_KEY");
    for issuer in [
        "https://platform.deepseek.com.evil.example",
        "http://platform.deepseek.com",
        "https://relay.example",
    ] {
        dsh_platform_grant(root.path(), issuer);
        assert!(
            credentials::discover(&agent("deepseek-harness"), &env)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn an_unchanged_account_is_not_asked_again_and_a_rotated_one_is() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let models =
        |key: &str| json!({"providers":{"relay":{"apiKey":key,"baseUrl":"https://relay.example"}}});
    write(root.path(), ".pi/agent/models.json", &models("first"));
    let get = |_: Credential| async { Ok(json!({"ok": true})) };
    let Usage::Read { fingerprint, .. } = read(&agent("pi"), &env, None, get).await.unwrap() else {
        panic!("nothing was known yet");
    };
    assert!(!fingerprint.contains("first"));
    let again = read(&agent("pi"), &env, Some(&fingerprint), |_| async {
        panic!("the account did not change")
    })
    .await
    .unwrap();
    assert!(matches!(again, Usage::Unchanged));
    write(root.path(), ".pi/agent/models.json", &models("second"));
    let rotated = read(&agent("pi"), &env, Some(&fingerprint), get)
        .await
        .unwrap();
    assert!(matches!(rotated, Usage::Read { .. }));
}

#[tokio::test]
async fn an_answer_for_credentials_replaced_meanwhile_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    let models =
        |key: &str| json!({"providers":{"relay":{"apiKey":key,"baseUrl":"https://relay.example"}}});
    write(root.path(), ".pi/agent/models.json", &models("first"));
    let failure = read(&agent("pi"), &env, None, |_| async {
        write(root.path(), ".pi/agent/models.json", &models("second"));
        Ok(json!({}))
    })
    .await
    .unwrap_err();
    assert_eq!(failure.kind, "capability");
}

/// The real `curl`, against a server on this machine.
#[cfg(unix)]
mod through_curl {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn server(status: u16, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_owned();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            while !received.windows(4).any(|s| s == b"\r\n\r\n") {
                let mut buffer = [0; 2048];
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..read]);
            }
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            String::from_utf8(received).unwrap()
        });
        (format!("http://{address}/usage"), task)
    }

    fn sent_to(endpoint: String, kind: Kind, key: &str, org_id: Option<&str>) -> Credential {
        Credential {
            kind,
            label: "fixture".into(),
            endpoint,
            key: key.into(),
            org_id: org_id.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn a_credential_reaches_only_its_own_header_exactly_as_stored() {
        let (url, task) = server(200, "{\"ok\":true}").await;
        // Quotes and backslashes must survive curl's config syntax and add no lines to it.
        let tricky = r#"a"b\c" header = "x-injected: 1"#;
        let answer = over_curl(sent_to(url, Kind::Go, tricky, Some("org-1")))
            .await
            .unwrap();
        assert_eq!(answer, json!({"ok": true}));
        let request = task.await.unwrap();
        assert!(request.contains(&format!("Authorization: Bearer {tricky}\r\n")));
        assert!(request.contains("x-opencode-org-id: org-1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("\r\nx-injected"));

        let (url, task) = server(200, "{}").await;
        over_curl(sent_to(url, Kind::DeepSeekPlatform, "grant", Some("org-1")))
            .await
            .unwrap();
        let request = task.await.unwrap();
        assert!(request.contains("x-dsh-auth-token: grant\r\n"));
        assert!(!request.contains("Authorization"));
        assert!(!request.contains("x-opencode-org-id"));
    }

    #[tokio::test]
    async fn statuses_mean_the_same_as_they_do_on_the_desktop() {
        for (status, body, kind, message) in [
            (401, "private", "login", "authentication_failed"),
            (403, "private", "login", "subscription_required"),
            (429, "private", "protocol", "agent_rate_limited"),
            (
                302,
                "private",
                "disconnected",
                "provider quota request unsuccessful",
            ),
            (
                200,
                "private",
                "protocol",
                "provider quota response invalid",
            ),
        ] {
            let (url, task) = server(status, body).await;
            let failure = over_curl(sent_to(url, Kind::Go, "key", None))
                .await
                .unwrap_err();
            assert_eq!(
                (failure.kind.as_str(), failure.message.as_str()),
                (kind, message)
            );
            task.await.unwrap();
        }
    }
}
