//! Read-only Cursor `DashboardService` adapter using the CLI's native OAuth store.
//! Credentials stay in memory; no terminal, model prompt or credential refresh.
use driver_api::{DriverError, QuotaSnapshot, QuotaWindow};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

const ENDPOINT: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/";
const BODY_LIMIT: usize = 1_048_576;

fn login_required() -> DriverError {
    DriverError::NotConfigured("Cursor requires login; run `agent login`".into())
}

fn parse(usage: &Value, plan: &Value) -> Result<QuotaSnapshot, DriverError> {
    let percent = |key: &str| {
        usage["planUsage"][key]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.min(100.0))
    };
    // Spend/limit excludes bonuses and model buckets. Only use native percentages.
    let auto = percent("autoPercentUsed")
        .ok_or_else(|| DriverError::Protocol("Cursor usage percentages unavailable".into()))?;
    let end = &usage["billingCycleEnd"];
    let reset = end
        .as_i64()
        .or_else(|| end.as_str()?.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|date| date.timestamp());
    let window = |id: &str, scope: &str, used| QuotaWindow {
        id: id.into(),
        scope: Some(scope.into()),
        window_minutes: 0,
        used_percent: Some(used),
        resets_at_unix: reset,
    };
    let mut windows = vec![window("cursor-models", "Cursor Models", auto)];
    if let Some(api) = percent("apiPercentUsed") {
        windows.push(window("cursor-other-models", "Other Models", api));
    }
    let now = chrono::Utc::now().timestamp_millis();
    Ok(QuotaSnapshot {
        windows,
        plan_type: plan["planInfo"]["planName"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        source: "native_credentials_pull".into(),
        confidence: "provider_reported".into(),
        parser_version: "cursor-dashboard-usage-v1".into(),
        ..QuotaSnapshot::default()
    })
}

#[derive(Debug, PartialEq, Eq)]
enum CredentialSource {
    Keychain,
    File(PathBuf),
}

fn credential_source(
    platform: &str,
    api_key: Option<&str>,
    store: Option<&str>,
    home: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<CredentialSource, DriverError> {
    if api_key.is_some_and(|value| !value.trim().is_empty()) || store == Some("memory") {
        return Err(DriverError::CapabilityUnavailable(
            "Cursor subscription quota requires native OAuth credentials".into(),
        ));
    }
    if platform == "macos" && store != Some("file") {
        return Ok(CredentialSource::Keychain);
    }
    let home = home.ok_or_else(login_required)?;
    let path = match platform {
        "macos" => home.join(".cursor/auth.json"),
        "windows" => config
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Cursor/auth.json"),
        _ => config
            .unwrap_or_else(|| home.join(".config"))
            .join("cursor/auth.json"),
    };
    Ok(CredentialSource::File(path))
}

async fn access_token() -> Result<String, DriverError> {
    let source = credential_source(
        std::env::consts::OS,
        std::env::var("CURSOR_API_KEY").ok().as_deref(),
        std::env::var("AGENT_CLI_CREDENTIAL_STORE").ok().as_deref(),
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from),
        std::env::var_os(if cfg!(target_os = "windows") {
            "APPDATA"
        } else {
            "XDG_CONFIG_HOME"
        })
        .map(PathBuf::from),
    )?;
    let token = match source {
        CredentialSource::Keychain => {
            let mut command = tokio::process::Command::new("/usr/bin/security");
            command
                .args([
                    "find-generic-password",
                    "-s",
                    "cursor-access-token",
                    "-a",
                    "cursor-user",
                    "-w",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            let child = agent_runtime::OwnedChild::spawn(command).map_err(|_| login_required())?;
            let output =
                tokio::time::timeout(Duration::from_secs(5), child.wait_with_output(64 * 1024))
                    .await
                    .map_err(|_| login_required())?
                    .map_err(|_| login_required())?;
            if !output.status.success() {
                return Err(login_required());
            }
            String::from_utf8(output.stdout).map_err(|_| login_required())?
        }
        CredentialSource::File(path) => {
            let bytes = tokio::fs::read(path).await.map_err(|_| login_required())?;
            let auth: Value = serde_json::from_slice(&bytes).map_err(|_| login_required())?;
            auth["accessToken"]
                .as_str()
                .ok_or_else(login_required)?
                .to_owned()
        }
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(login_required());
    }
    Ok(token.to_owned())
}

async fn request(client: &reqwest::Client, url: &str, token: &str) -> Result<Value, DriverError> {
    let mut response = client
        .post(url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}")
        .send()
        .await
        .map_err(|_| {
            DriverError::Disconnected("Cursor usage request failed or timed out".into())
        })?;
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            return Err(login_required());
        }
        status if !status.is_success() => {
            return Err(DriverError::Protocol(format!(
                "Cursor usage endpoint returned HTTP {}",
                status.as_u16()
            )));
        }
        _ => {}
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DriverError::Protocol("Cursor usage response interrupted".into()))?
    {
        if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(DriverError::Protocol(
                "Cursor usage response exceeded size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| DriverError::Protocol("Cursor usage endpoint returned invalid JSON".into()))
}

pub(super) async fn read() -> Result<QuotaSnapshot, DriverError> {
    let token = access_token().await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| DriverError::Protocol("Cursor HTTP client unavailable".into()))?;
    let usage_url = format!("{ENDPOINT}GetCurrentPeriodUsage");
    let plan_url = format!("{ENDPOINT}GetPlanInfo");
    let (usage, plan) = tokio::join!(
        request(&client, &usage_url, &token),
        request(&client, &plan_url, &token),
    );
    // A plan lookup failure must not discard valid account usage.
    parse(&usage?, &plan.unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_native_pools_and_exact_billing_reset() {
        for end in [json!(1_793_491_200_000_i64), json!("1793491200000")] {
            let quota = parse(
                &json!({"billingCycleEnd":end,"planUsage":{
                    "autoPercentUsed":20.5,"apiPercentUsed":110,"spend":5,"limit":10
                }}),
                &json!({"planInfo":{"planName":"Pro"}}),
            )
            .unwrap();
            assert_eq!(quota.windows.len(), 2);
            assert_eq!(quota.windows[0].id, "cursor-models");
            assert_eq!(quota.windows[0].scope.as_deref(), Some("Cursor Models"));
            assert_eq!(quota.windows[0].used_percent, Some(20.5));
            assert_eq!(quota.windows[1].id, "cursor-other-models");
            assert_eq!(quota.windows[1].scope.as_deref(), Some("Other Models"));
            assert_eq!(quota.windows[1].used_percent, Some(100.0));
            assert!(
                quota
                    .windows
                    .iter()
                    .all(|w| w.resets_at_unix == Some(1_793_491_200))
            );
            assert_eq!(quota.plan_type.as_deref(), Some("Pro"));
            assert_eq!(
                quota.stale_after_unix_ms - quota.collected_at_unix_ms,
                60_000
            );
        }
    }

    #[test]
    fn missing_fields_stay_unknown_without_inventing_balances() {
        for usage in [
            json!({}),
            json!({"planUsage":{"spend":5,"limit":10}}),
            json!({"planUsage":{"autoPercentUsed":-1}}),
            json!({"planUsage":{"autoPercentUsed":"20"}}),
        ] {
            assert!(parse(&usage, &Value::Null).is_err());
        }
        for end in [
            Value::Null,
            json!("bad"),
            json!(0),
            json!(-1),
            json!(i64::MAX),
        ] {
            let quota = parse(
                &json!({"billingCycleEnd":end,"planUsage":{"autoPercentUsed":0}}),
                &Value::Null,
            )
            .unwrap();
            assert_eq!(quota.windows.len(), 1);
            assert_eq!(quota.windows[0].used_percent, Some(0.0));
            assert_eq!(quota.windows[0].resets_at_unix, None);
            assert_eq!(quota.plan_type, None);
        }
    }

    #[test]
    fn follows_native_credential_store_and_excludes_unrelated_oauth() {
        let home = Some(PathBuf::from("/home/user"));
        assert_eq!(
            credential_source("macos", None, None, home.clone(), None).unwrap(),
            CredentialSource::Keychain
        );
        for (platform, store, config, expected) in [
            ("macos", Some("file"), None, "/home/user/.cursor/auth.json"),
            ("linux", None, None, "/home/user/.config/cursor/auth.json"),
            (
                "linux",
                None,
                Some(PathBuf::from("/config")),
                "/config/cursor/auth.json",
            ),
            (
                "windows",
                None,
                Some(PathBuf::from("/roaming")),
                "/roaming/Cursor/auth.json",
            ),
        ] {
            assert_eq!(
                credential_source(platform, None, store, home.clone(), config).unwrap(),
                CredentialSource::File(PathBuf::from(expected))
            );
        }
        assert!(credential_source("macos", Some("api-key"), None, home.clone(), None).is_err());
        assert!(credential_source("macos", None, Some("memory"), home, None).is_err());
    }

    #[tokio::test]
    async fn dashboard_requests_are_read_only_and_sanitize_errors() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, body) in [
            ("200 OK", r#"{"planUsage":{"autoPercentUsed":12}}"#),
            ("401 Unauthorized", "private-server-detail"),
            ("403 Forbidden", "private-server-detail"),
            ("302 Found", "private-server-detail"),
            ("200 OK", "private-invalid-json"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!(
                "http://{}/GetCurrentPeriodUsage",
                listener.local_addr().unwrap()
            );
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 1024];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.ends_with(b"\r\n\r\n{}") {
                        break;
                    }
                }
                let request = String::from_utf8(bytes).unwrap().to_lowercase();
                assert!(request.starts_with("post /getcurrentperiodusage http/1.1\r\n"));
                assert!(request.contains("authorization: bearer native-token\r\n"));
                assert!(request.contains("connect-protocol-version: 1\r\n"));
                assert!(request.contains("content-type: application/json\r\n"));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let result = request(&client, &url, "native-token").await;
            if body.starts_with('{') {
                assert_eq!(result.unwrap()["planUsage"]["autoPercentUsed"], 12);
            } else {
                let error = result.unwrap_err();
                assert!(!error.to_string().contains("private-"));
                assert!(!error.to_string().contains("native-token"));
                if status.starts_with("401") || status.starts_with("403") {
                    assert!(driver_api::failure::invalidates_account_quota(&error));
                    assert_eq!(
                        driver_api::failure::quota_diagnostic(&error),
                        "quota_login_required"
                    );
                    assert!(matches!(error, DriverError::NotConfigured(_)));
                }
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires authenticated Cursor; reads native OAuth and official quota only"]
    async fn real_native_quota() {
        let snapshot = tokio::time::timeout(Duration::from_secs(20), read())
            .await
            .unwrap()
            .unwrap();
        assert!(!snapshot.windows.is_empty());
        eprintln!(
            "Cursor quota: {:?}; plan: {:?}",
            snapshot.windows, snapshot.plan_type
        );
    }
}
