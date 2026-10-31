use super::*;
use driver_api::{ContextUsageSnapshot, ProviderBalance, usage::SessionUsageSnapshot};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn go_payload() -> Value {
    json!({"usage":{
        "rolling":{"status":"ok","percent":12.5,"resetsAt":"2099-10-30T12:00:00Z"},
        "weekly":{"status":"rate-limited","percent":100,"resetsAt":"2099-11-02T00:00:00Z"},
        "monthly":{"status":"ok","percent":47,"resetsAt":"2099-11-08T15:22:22Z"}
    }})
}
fn environment(root: &Path) -> HashMap<String, String> {
    HashMap::from([
        ("HOME".into(), root.to_string_lossy().into()),
        (
            "XDG_DATA_HOME".into(),
            root.join("data").to_string_lossy().into(),
        ),
    ])
}
fn write(root: &Path, relative: &str, value: Value) {
    let p = root.join(relative);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, value.to_string()).unwrap();
}
fn monitor(provider: &str, root: &Path) -> AccountMonitor {
    let mut monitor = AccountMonitor::new(
        ExecutorRef {
            provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
            installation_id: Some("ins_test".into()),
        },
        None,
    )
    .unwrap();
    monitor.environment = Some(environment(root));
    monitor
}
async fn server(responses: Vec<(u16, String)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            while !received.windows(4).any(|s| s == b"\r\n\r\n") {
                let mut buf = [0; 2048];
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            requests.push(String::from_utf8(received).unwrap());
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
        requests
    });
    (format!("http://{address}"), task)
}

#[test]
fn go_projects_all_three_native_windows_without_a_global_model_gate() {
    let windows = projection::go(&go_payload()).unwrap();
    assert_eq!(
        windows
            .iter()
            .map(|w| w.used_percent.unwrap())
            .collect::<Vec<_>>(),
        [12.5, 100.0, 47.0]
    );
    assert_eq!(
        windows.iter().map(|w| w.window_minutes).collect::<Vec<_>>(),
        [300, 10080, 0]
    );
    assert_eq!(windows[2].id, "opencode-go-monthly");
    assert!(
        windows
            .iter()
            .all(|w| w.scope.as_deref() == Some("OpenCode Go") && w.resets_at_unix.is_some())
    );
    for field in ["percent", "resetsAt", "status"] {
        let mut bad = go_payload();
        bad["usage"]["rolling"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(projection::go(&bad).is_err());
    }
    for value in [json!(-1), json!("12"), json!(null)] {
        let mut bad = go_payload();
        bad["usage"]["weekly"]["percent"] = value;
        assert!(projection::go(&bad).is_err());
    }
}

#[test]
fn money_preserves_zero_and_currency_and_omits_unlimited_or_unknown_balances() {
    let balances = projection::deepseek(&json!({"balance_infos":[
        {"currency":"CNY","total_balance":"0.00","granted_balance":"99"},
        {"currency":"USD","total_balance":"1.2345"}
    ]}))
    .unwrap();
    assert!(balances[0].amount.abs() < f64::EPSILON);
    assert_eq!(balances[1].currency, "USD");
    assert!((balances[1].amount - 1.2345).abs() < f64::EPSILON);
    for bad in [
        json!({}),
        json!({"balance_infos":[{"currency":"USD","total_balance":"NaN"}]}),
    ] {
        assert!(projection::deepseek(&bad).is_err());
    }
    assert!(
        (projection::sub2api(
            &json!({"mode":"quota_limited","unit":"USD","quota":{"remaining":"4.5"}}),
            "relay"
        )
        .unwrap()[0]
            .amount
            - 4.5)
            .abs()
            < f64::EPSILON
    );
    for bad in [
        json!({"mode":"subscription","unit":"USD","remaining":-1}),
        json!({"mode":"unrestricted","unit":"USD"}),
        json!({"mode":"quota_limited","unit":"tokens","balance":1}),
    ] {
        assert!(projection::sub2api(&bad, "relay").is_err());
    }
}

#[test]
fn opencode_legacy_go_key_is_not_replaced_by_zen_or_oauth() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode":{"type":"api","key":"zen-private"},"opencode-go":{"type":"api","key":"go-private"}}),
    );
    assert_eq!(
        credentials::discover("opencode", &env).unwrap()[0].key,
        "go-private"
    );
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode":{"type":"api","key":"zen-private"},"opencode-go":{"type":"oauth","access":"oauth-private"}}),
    );
    let error = credentials::discover("opencode", &env).err().unwrap();
    assert_eq!(
        driver_api::failure::quota_diagnostic(&error),
        "opencode_go_not_configured"
    );
    assert!(!format!("{error:?}").contains("private"));
}

#[test]
fn opencode_v2_selects_native_active_key_and_does_not_resurrect_legacy_credentials() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"legacy-private"}}),
    );
    let db = rusqlite::Connection::open(root.path().join("data/opencode/opencode.db")).unwrap();
    db.execute_batch("CREATE TABLE credential(id TEXT, integration_id TEXT, value TEXT, active INTEGER, time_created INTEGER);").unwrap();
    for (id, provider, active, time, value) in [
        (
            "old",
            "opencode-go",
            1,
            1,
            json!({"type":"key","key":"active-private"}),
        ),
        (
            "new",
            "opencode-go",
            0,
            2,
            json!({"type":"api","key":"inactive-private"}),
        ),
        (
            "zen",
            "opencode",
            1,
            3,
            json!({"type":"key","key":"zen-private"}),
        ),
    ] {
        db.execute(
            "INSERT INTO credential VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![id, provider, value.to_string(), active, time],
        )
        .unwrap();
    }
    assert_eq!(
        credentials::discover("opencode", &env).unwrap()[0].key,
        "active-private"
    );
    db.execute(
        "DELETE FROM credential WHERE integration_id='opencode-go'",
        [],
    )
    .unwrap();
    assert!(credentials::discover("opencode", &env).is_err());
}

#[test]
fn pi_uses_configured_provider_origins_without_running_key_commands() {
    let root = tempfile::tempdir().unwrap();
    let env = environment(root.path());
    write(
        root.path(),
        ".pi/agent/auth.json",
        json!({"deepseek":{"type":"api_key","key":"deep-private"},"relay":{"type":"api_key","key":"native-private"}}),
    );
    write(
        root.path(),
        ".pi/agent/models.json",
        json!({"providers":{
            "relay":{"apiKey":"config-private","baseUrl":"https://relay.example/anthropic/v1?token=secret"},
            "command":{"apiKey":"!touch /tmp/should-not-run","baseUrl":"https://command.example"},
            "local-file":{"apiKey":"no","baseUrl":"file:///tmp/secret"}
        }}),
    );
    let requests = credentials::discover("pi", &env).unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].endpoint,
        "https://api.deepseek.com/user/balance"
    );
    assert_eq!(requests[1].endpoint, "https://relay.example/v1/usage");
    assert_eq!(requests[1].key, "native-private");
}

#[test]
fn dsh_saved_reference_wins_over_environment_and_browser_grants_stay_opaque() {
    let root = tempfile::tempdir().unwrap();
    let mut env = environment(root.path());
    env.insert("DEEPSEEK_API_KEY".into(), "env-private".into());
    std::fs::create_dir_all(root.path().join(".dsh")).unwrap();
    std::fs::write(root.path().join(".dsh/.credentials.yaml"), "version: 1\nrefs:\n  DEEPSEEK_API_KEY: stored-private\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {secret: browser-private}\n").unwrap();
    assert_eq!(
        credentials::discover("deepseek-harness", &env).unwrap()[0].key,
        "stored-private"
    );
    std::fs::write(root.path().join(".dsh/.credentials.yaml"), "version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {secret: browser-private}\n").unwrap();
    env.remove("DEEPSEEK_API_KEY");
    assert!(credentials::discover("deepseek-harness", &env).is_err());
}

fn dsh_platform_grant(root: &Path, issuer: &str) {
    std::fs::create_dir_all(root.join(".dsh")).unwrap();
    std::fs::write(root.join(".dsh/.credentials.yaml"), format!("version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {{secret: browser-private}}\n  deepseek-account-platform/default:\n    kind: grant\n    payload: {{version: 1, token: grant-private, issuer: '{issuer}'}}\n")).unwrap();
}

#[test]
fn dsh_platform_grant_is_a_fallback_bound_to_its_official_issuer() {
    let root = tempfile::tempdir().unwrap();
    let mut env = environment(root.path());
    dsh_platform_grant(root.path(), "https://platform.deepseek.com");
    let requests = credentials::discover("deepseek-harness", &env).unwrap();
    assert_eq!(requests.len(), 1);
    assert!(matches!(requests[0].kind, Kind::DeepSeekPlatform));
    assert_eq!(requests[0].key, "grant-private");
    assert_eq!(
        requests[0].endpoint,
        "https://platform.deepseek.com/api/v0/users/get_user_summary"
    );
    env.insert("DEEPSEEK_API_KEY".into(), "env-private".into());
    let requests = credentials::discover("deepseek-harness", &env).unwrap();
    assert!(matches!(requests[0].kind, Kind::DeepSeek));
    assert_eq!(requests[0].key, "env-private");
    env.remove("DEEPSEEK_API_KEY");
    for issuer in [
        "https://platform.deepseek.com.evil.example",
        "http://platform.deepseek.com",
        "https://relay.example",
    ] {
        dsh_platform_grant(root.path(), issuer);
        assert!(credentials::discover("deepseek-harness", &env).is_err());
    }
}

#[test]
fn deepseek_platform_keeps_recharge_and_bonus_wallets_apart() {
    let balances =
        projection::deepseek_platform(&json!({"code":0,"data":{"biz_code":0,"biz_data":{
            "normal_wallets":[{"currency":"CNY","balance":"0E-16"}],
            "bonus_wallets":[{"currency":"CNY","balance":"6.0000000000000000"}]
        }}}))
        .unwrap();
    assert_eq!(balances.len(), 2);
    assert_eq!(balances[0].label, "DeepSeek");
    assert!(balances[0].amount.abs() < f64::EPSILON);
    assert_eq!(balances[1].label, "DeepSeek bonus");
    assert!((balances[1].amount - 6.0).abs() < f64::EPSILON);
    let expired = projection::deepseek_platform(
        &json!({"code":40003,"msg":"Authorization Failed (invalid token)","data":null}),
    )
    .unwrap_err();
    assert_eq!(
        driver_api::failure::quota_diagnostic(&expired),
        "quota_login_required"
    );
    for bad in [
        json!({}),
        json!({"code":0,"data":{"biz_data":{"normal_wallets":[]}}}),
        json!({"code":0,"data":{"biz_data":{"normal_wallets":[],"bonus_wallets":[]}}}),
    ] {
        assert!(projection::deepseek_platform(&bad).is_err());
    }
}

#[tokio::test]
async fn dsh_platform_grant_is_sent_only_in_the_dsh_header() {
    let root = tempfile::tempdir().unwrap();
    let body = json!({"code":0,"data":{"biz_data":{
        "normal_wallets":[{"currency":"CNY","balance":"1.5"}],"bonus_wallets":[]
    }}});
    let (url, task) = server(vec![(200, body.to_string())]).await;
    dsh_platform_grant(root.path(), "https://platform.deepseek.com");
    let mut m = monitor("deepseek-harness", root.path());
    m.endpoints = Some((url.clone(), url));
    let quota = m.read_quota(None).await.unwrap();
    assert_eq!(quota.balances[0].label, "DeepSeek");
    assert!(!serde_json::to_string(&quota).unwrap().contains("private"));
    let request = task.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(request.contains("x-dsh-auth-token: grant-private"));
    assert!(!request.contains("authorization:"));
}

#[tokio::test]
async fn account_reads_are_get_only_cached_forced_and_invalidated_on_key_rotation() {
    let root = tempfile::tempdir().unwrap();
    let value = go_payload().to_string();
    let (url, task) = server(vec![
        (200, value.clone()),
        (200, value.clone()),
        (200, value),
    ])
    .await;
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"first-private"}}),
    );
    let mut m = monitor("opencode", root.path());
    m.endpoints = Some((format!("{url}/usage"), url));
    let (a, b) = tokio::join!(m.read_quota(None), m.read_quota(None));
    assert_eq!(a.unwrap(), b.unwrap());
    m.read_quota_with_refresh(None, true).await.unwrap();
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"second-private"}}),
    );
    let quota = m.read_quota(None).await.unwrap();
    assert!(!serde_json::to_string(&quota).unwrap().contains("private"));
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|r| r.starts_with("GET /usage HTTP/1.1"))
    );
    assert!(requests[0].contains("Bearer first-private"));
    assert!(requests[2].contains("Bearer second-private"));
}

#[tokio::test]
async fn native_auth_failures_do_not_reuse_a_successful_other_account_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![
        (200, go_payload().to_string()),
        (401, "private upstream response".into()),
    ])
    .await;
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"first-private"}}),
    );
    let mut m = monitor("opencode", root.path());
    m.endpoints = Some((url.clone(), url));
    m.read_quota(None).await.unwrap();
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"second-private"}}),
    );
    let e = m.read_quota(None).await.unwrap_err();
    assert_eq!(
        driver_api::failure::quota_diagnostic(&e),
        "quota_login_required"
    );
    assert!(!format!("{e:?}").contains("private"));
    task.await.unwrap();
}

#[tokio::test]
async fn credentials_rotated_during_an_http_request_discard_the_old_response() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "data/opencode/auth.json",
        json!({"opencode-go":{"type":"api","key":"first-private"}}),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let file = root.path().join("data/opencode/auth.json");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        let count = socket.read(&mut bytes).await.unwrap();
        assert!(count > 0);
        std::fs::write(
            file,
            json!({"opencode-go":{"type":"api","key":"second-private"}}).to_string(),
        )
        .unwrap();
        let body = go_payload().to_string();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let mut monitor = monitor("opencode", root.path());
    monitor.endpoints = Some((url.clone(), url));
    assert!(monitor.read_quota(None).await.is_err());
    assert!(monitor.cache.lock().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn provider_balance_failures_are_isolated_and_money_is_never_a_percentage() {
    let root = tempfile::tempdir().unwrap();
    let (ok, ok_task) = server(vec![(
        200,
        json!({"mode":"unrestricted","unit":"USD","balance":"3.25"}).to_string(),
    )])
    .await;
    let (bad, bad_task) = server(vec![(500, "private service error".into())]).await;
    write(
        root.path(),
        ".pi/agent/models.json",
        json!({"providers":{
            "good":{"baseUrl":ok,"apiKey":"good-private"},"bad":{"baseUrl":bad,"apiKey":"bad-private"}
        }}),
    );
    let quota = monitor("pi", root.path()).read_quota(None).await.unwrap();
    assert!(quota.windows.is_empty());
    assert_eq!(
        quota.balances,
        vec![ProviderBalance {
            label: "good".into(),
            amount: 3.25,
            currency: "USD".into()
        }]
    );
    assert!(ok_task.await.unwrap()[0].starts_with("GET /v1/usage HTTP/1.1"));
    bad_task.await.unwrap();
}

#[tokio::test]
async fn http_diagnostics_are_bounded_and_do_not_expose_response_or_credentials() {
    for (status, body, diagnostic) in [
        (403, "private body".into(), "quota_subscription_required"),
        (429, "private body".into(), "quota_rate_limited"),
        (302, "private redirect".into(), "quota_network_error"),
        (200, "private non-json".into(), "quota_unavailable"),
        (200, "x".repeat(1024 * 1024 + 1), "quota_unavailable"),
    ] {
        let (url, task) = server(vec![(status, body)]).await;
        let r = Request {
            kind: Kind::Go,
            label: "Go".into(),
            endpoint: url,
            key: "private-key".into(),
        };
        let error = http::get(&http::client().unwrap(), &r).await.unwrap_err();
        assert_eq!(driver_api::failure::quota_diagnostic(&error), diagnostic);
        assert!(!format!("{error:?}").contains("private"));
        task.await.unwrap();
    }
}

struct SessionProvider {
    executor: ExecutorRef,
    usage_reads: AtomicUsize,
}
#[async_trait]
impl QuotaProvider for SessionProvider {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, _: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        Ok(QuotaSnapshot {
            context: Some(ContextUsageSnapshot {
                used_tokens: 12,
                window_tokens: 100,
                cache_hit_rate_percent: None,
            }),
            ..Default::default()
        })
    }
    async fn read_session_usage(&self, id: &str) -> Option<SessionUsageSnapshot> {
        assert_eq!(id, "session-original");
        self.usage_reads.fetch_add(1, Ordering::Relaxed);
        Some(SessionUsageSnapshot {
            input_tokens: Some(12),
            ..SessionUsageSnapshot::new()
        })
    }
}
#[tokio::test]
async fn monitoring_preserves_native_session_usage_and_context() {
    let root = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![(200, go_payload().to_string())]).await;
    let mut m = monitor("opencode", root.path());
    m.environment
        .as_mut()
        .unwrap()
        .insert("OPENCODE_GO_API_KEY".into(), "private-key".into());
    m.endpoints = Some((url.clone(), url));
    let session = Arc::new(SessionProvider {
        executor: m.executor.clone(),
        usage_reads: AtomicUsize::new(0),
    });
    m.session = Some(session.clone());
    assert_eq!(
        m.read_quota(Some("session-original"))
            .await
            .unwrap()
            .context
            .unwrap()
            .used_tokens,
        12
    );
    assert_eq!(
        m.read_session_usage("session-original")
            .await
            .unwrap()
            .input_tokens,
        Some(12)
    );
    assert_eq!(session.usage_reads.load(Ordering::Relaxed), 1);
    task.await.unwrap();
}

#[tokio::test]
#[ignore = "read-only queries using native provider credentials and real internet"]
async fn live_readonly_provider_accounts() {
    for provider in ["opencode", "pi", "deepseek-harness"] {
        let monitor = AccountMonitor::new(
            ExecutorRef {
                provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
                installation_id: Some("live-check".into()),
            },
            None,
        )
        .unwrap();
        if monitor.requests().await.is_err() {
            println!("{provider}: no supported account configured; skipped");
            continue;
        }
        let quota = monitor
            .read_quota(None)
            .await
            .expect("configured provider account query failed");
        assert!(!quota.windows.is_empty() || !quota.balances.is_empty());
        println!("{provider}: {}", serde_json::to_string(&quota).unwrap());
    }
}
