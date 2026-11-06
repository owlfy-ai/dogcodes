use super::*;
use driver_api::{ContextUsageSnapshot, ProviderBalance, usage::SessionUsageSnapshot};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
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
fn monitor_of(provider: &str, root: &Path, executable: &str) -> AccountMonitor {
    let mut monitor = AccountMonitor::new(
        ExecutorRef {
            provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
            installation_id: Some("ins_test".into()),
        },
        &TargetExecutable::local(executable),
        Some("2.2.5".into()),
        None,
    )
    .unwrap();
    monitor.environment = Some(environment(root));
    monitor
}
fn monitor(provider: &str, root: &Path) -> AccountMonitor {
    monitor_of(provider, root, "/fixture/agent")
}
/// A relay Pi is configured to use: the simplest account to replace in a test.
fn relay(root: &Path, url: &str, key: &str) {
    write(
        root,
        ".pi/agent/models.json",
        json!({"providers":{"relay":{"baseUrl":url,"apiKey":key}}}),
    );
}
fn relay_balance() -> String {
    json!({"mode":"unrestricted","unit":"USD","balance":"3.25"}).to_string()
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
        {"currency":"CNY","total_balance":"99.00","granted_balance":"99","topped_up_balance":"0.00"},
        {"currency":"USD","total_balance":"7.2345","granted_balance":"6","topped_up_balance":"1.2345"}
    ]}))
    .unwrap();
    assert!(balances[0].amount.abs() < f64::EPSILON);
    assert_eq!(balances[1].currency, "USD");
    assert!((balances[1].amount - 1.2345).abs() < f64::EPSILON);
    for bad in [
        json!({}),
        json!({"balance_infos":[{"currency":"USD","total_balance":"99","topped_up_balance":"NaN"}]}),
        json!({"balance_infos":[{"currency":"USD","total_balance":"99","granted_balance":"99"}]}),
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

fn dsh_platform_grant(root: &Path, issuer: &str) {
    std::fs::create_dir_all(root.join(".dsh")).unwrap();
    std::fs::write(root.join(".dsh/.credentials.yaml"), format!("version: 1\nrecords:\n  client-connection/browser-session:\n    kind: grant\n    payload: {{secret: browser-private}}\n  deepseek-account-platform/default:\n    kind: grant\n    payload: {{version: 1, token: grant-private, issuer: '{issuer}'}}\n")).unwrap();
}

#[test]
fn deepseek_platform_only_keeps_recharge_wallets_and_ignores_bonus_data() {
    let balances =
        projection::deepseek_platform(&json!({"code":0,"data":{"biz_code":0,"biz_data":{
            "normal_wallets":[{"currency":"CNY","balance":"0E-16"}],
            "bonus_wallets":[{"currency":"CNY","balance":"6.0000000000000000"}]
        }}}))
        .unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].label, "DeepSeek");
    assert!(balances[0].amount.abs() < f64::EPSILON);
    for bonus in [None, Some(json!("not a wallet list"))] {
        let mut payload = json!({"code":0,"data":{"biz_data":{
            "normal_wallets":[{"currency":"USD","balance":"2.5"}]
        }}});
        if let Some(bonus) = bonus {
            payload["data"]["biz_data"]["bonus_wallets"] = bonus;
        }
        let balances = projection::deepseek_platform(&payload).unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].currency, "USD");
        assert!((balances[0].amount - 2.5).abs() < f64::EPSILON);
    }
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
        json!({"code":0,"data":{"biz_data":{"normal_wallets":[],"bonus_wallets":[{"currency":"CNY","balance":"6"}]}}}),
    ] {
        assert!(projection::deepseek_platform(&bad).is_err());
    }
}

#[tokio::test]
async fn dsh_platform_grant_is_sent_only_in_the_dsh_header() {
    let root = tempfile::tempdir().unwrap();
    let body = json!({"code":0,"data":{"biz_data":{
        "normal_wallets":[{"currency":"CNY","balance":"1.5"}],
        "bonus_wallets":[{"currency":"CNY","balance":"100"}]
    }}});
    let (url, task) = server(vec![(200, body.to_string())]).await;
    dsh_platform_grant(root.path(), "https://platform.deepseek.com");
    let mut m = monitor("deepseek-harness", root.path());
    m.endpoints = Some((url.clone(), url));
    let quota = m.read_quota(None).await.unwrap();
    assert_eq!(quota.balances.len(), 1);
    assert_eq!(quota.balances[0].label, "DeepSeek");
    assert!((quota.balances[0].amount - 1.5).abs() < f64::EPSILON);
    assert!(!serde_json::to_string(&quota).unwrap().contains("private"));
    let request = task.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(request.contains("x-dsh-auth-token: grant-private"));
    assert!(!request.contains("authorization:"));
}

#[cfg(unix)]
#[tokio::test]
async fn opencode_go_subscription_sends_the_org_header() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let exported = root.path().join("export.json");
    std::fs::write(
        &exported,
        json!([{
            "id": "account", "integrationID": "opencode", "label": "me", "active": true,
            "value": {
                "type": "oauth",
                "access": "access-private",
                "refresh": "refresh-private",
                "metadata": {"server": "https://opencode.ai/console", "orgID": "org-private"}
            }
        }])
        .to_string(),
    )
    .unwrap();
    let opencode = root.path().join("opencode");
    std::fs::write(
        &opencode,
        format!("#!/bin/sh\ncat '{}'\n", exported.display()),
    )
    .unwrap();
    std::fs::set_permissions(&opencode, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (url, task) = server(vec![(200, go_payload().to_string())]).await;
    let mut monitor = monitor_of("opencode", root.path(), opencode.to_str().unwrap());
    monitor.endpoints = Some((url.clone(), url));
    let quota = monitor.read_quota(None).await.unwrap();
    assert_eq!(quota.windows.len(), 3);
    assert_eq!(quota.windows[0].id, "opencode-go-rolling");
    let request = task.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(request.contains("authorization: bearer access-private"));
    assert!(request.contains("x-opencode-org-id: org-private"));
    assert!(!request.contains("refresh-private"));
}

#[tokio::test]
async fn opencode_readings_are_kept_until_they_expire_or_are_refreshed() {
    let root = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![
        (200, go_payload().to_string()),
        (200, go_payload().to_string()),
    ])
    .await;
    let mut m = monitor("opencode", root.path());
    m.environment
        .as_mut()
        .unwrap()
        .insert("OPENCODE_GO_API_KEY".into(), "first-private".into());
    m.endpoints = Some((url.clone(), url));
    m.read_quota(None).await.unwrap();
    m.read_quota(None).await.unwrap();
    m.read_quota_with_refresh(None, true).await.unwrap();
    assert_eq!(task.await.unwrap().len(), 2);
}

#[tokio::test]
async fn account_reads_are_get_only_cached_forced_and_invalidated_on_key_rotation() {
    let root = tempfile::tempdir().unwrap();
    let value = relay_balance();
    let (url, task) = server(vec![
        (200, value.clone()),
        (200, value.clone()),
        (200, value),
    ])
    .await;
    relay(root.path(), &url, "first-private");
    let m = monitor("pi", root.path());
    let (a, b) = tokio::join!(m.read_quota(None), m.read_quota(None));
    assert_eq!(a.unwrap(), b.unwrap());
    m.read_quota_with_refresh(None, true).await.unwrap();
    relay(root.path(), &url, "second-private");
    let quota = m.read_quota(None).await.unwrap();
    assert!(!serde_json::to_string(&quota).unwrap().contains("private"));
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|r| r.starts_with("GET /v1/usage HTTP/1.1"))
    );
    assert!(requests[0].contains("Bearer first-private"));
    assert!(requests[2].contains("Bearer second-private"));
}

#[tokio::test]
async fn native_auth_failures_do_not_reuse_a_successful_other_account_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (url, task) = server(vec![
        (200, relay_balance()),
        (401, "private upstream response".into()),
    ])
    .await;
    relay(root.path(), &url, "first-private");
    let m = monitor("pi", root.path());
    m.read_quota(None).await.unwrap();
    relay(root.path(), &url, "second-private");
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    relay(root.path(), &url, "first-private");
    let (directory, address) = (root.path().to_owned(), url.clone());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        let count = socket.read(&mut bytes).await.unwrap();
        assert!(count > 0);
        relay(&directory, &address, "second-private");
        let body = relay_balance();
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
    let monitor = monitor("pi", root.path());
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
        let credential = Credential {
            kind: Kind::Go,
            label: "Go".into(),
            endpoint: url,
            key: "private-key".into(),
            org_id: None,
        };
        let error = driver_error(http::get(&credential).await.unwrap_err());
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
    for (provider, executable) in [
        ("opencode", "opencode"),
        ("pi", "pi"),
        ("deepseek-harness", "dsh"),
    ] {
        let monitor = AccountMonitor::new(
            ExecutorRef {
                provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
                installation_id: Some("live-check".into()),
            },
            &TargetExecutable::local(executable),
            None,
            None,
        )
        .unwrap();
        match monitor.read_quota(None).await {
            Ok(quota) => {
                assert!(!quota.windows.is_empty() || !quota.balances.is_empty());
                println!("{provider}: {}", serde_json::to_string(&quota).unwrap());
            }
            Err(error) => println!(
                "{provider}: {}",
                driver_api::failure::quota_diagnostic(&error)
            ),
        }
    }
}
