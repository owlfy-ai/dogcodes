//! Kiro's read-only ACP account extension. Never creates a session or model turn.
use super::{
    Agent, ConnectionTo, DriverError, QuotaSnapshot, QuotaWindow, map_session_error, now_unix_ms,
};
use std::fmt::Write;

pub(super) async fn read(
    connection: &ConnectionTo<Agent>,
) -> Result<serde_json::Value, DriverError> {
    let request =
        agent_client_protocol::UntypedMessage::new("_kiro/account/getUsage", serde_json::json!({}))
            .map_err(|_| DriverError::Protocol("Invalid Kiro usage request".into()))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        connection.send_request(request).block_task(),
    )
    .await
    .map_err(|_| DriverError::Disconnected("Kiro account usage timed out".into()))?
    .map_err(|error| map_session_error(&error, "Kiro account usage request failed", None))
}

pub(super) fn parse(payload: &serde_json::Value) -> Result<QuotaSnapshot, DriverError> {
    let invalid = || DriverError::Protocol("Kiro account usage unavailable or invalid".into());
    if payload.get("success").is_some_and(|value| value != true) {
        return Err(invalid());
    }
    let data = payload.get("data").unwrap_or(payload);
    let resources = data["usageBreakdowns"].as_array().ok_or_else(invalid)?;
    // Credits are an included allowance, not an account-wide spending ceiling:
    // bonus/add-on balances and overages can still make requests available.
    let credit = resources
        .iter()
        .find(|entry| entry["resourceType"] == "CREDIT")
        .ok_or_else(invalid)?;
    if credit["hasLimit"] == false {
        return Err(DriverError::CapabilityUnavailable(
            "Kiro reports no finite credit allowance".into(),
        ));
    }
    let used = credit["used"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(invalid)?;
    let limit = credit["limit"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(invalid)?;
    // Native `percentage` is rounded to whole numbers; derive from exact usage.
    let used_percent = (used / limit * 100.0).clamp(0.0, 100.0);
    let mut plan = data["planName"].as_str().unwrap_or("Kiro").to_owned();
    let reset = data["billingCycleReset"].as_str();
    let resets_at_unix = reset.and_then(super::parse_iso8601_unix);
    if let Some(date) = reset.filter(|_| resets_at_unix.is_none())
        && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
    {
        // A date alone has no timezone/time-of-day. Keep it for display without
        // inventing a reset timestamp that could expire the reading too soon.
        let _ = write!(plan, " · 重置 {date}（原生日期）");
    }
    if data["overagesEnabled"] == true {
        plan.push_str(" · 按量付费已开启");
    }
    for (key, label) in [
        ("bonusCredits", "另有赠送额度"),
        ("addOnCredits", "另有加购额度"),
    ] {
        if data[key].as_array().is_some_and(|items| !items.is_empty()) {
            let _ = write!(plan, " · {label}");
        }
    }
    let now = now_unix_ms();
    Ok(QuotaSnapshot {
        session_usage: None,
        windows: vec![QuotaWindow {
            scope: Some("套餐内（Kiro）".into()),
            id: "kiro-included".into(),
            window_minutes: 0,
            used_percent: Some(used_percent),
            resets_at_unix,
        }],
        context: None,
        plan_type: Some(plan),
        collected_at_unix_ms: now,
        stale_after_unix_ms: now.saturating_add(30_000),
        source: "native_pull".into(),
        confidence: "provider_reported".into(),
        parser_version: "kiro-account-usage-v1".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AcpDriver, AcpLaunch, Command, ConnectionHandle, QuotaProvider, SessionResumeCapability,
    };
    use serde_json::json;
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    use tokio::sync::mpsc;

    fn fixture() -> serde_json::Value {
        json!({"success":true,"data":{"planName":"KIRO FREE","billingCycleReset":"2026-10-01","overagesEnabled":false,"usageBreakdowns":[{"resourceType":"CREDIT","displayName":"Credits","used":0.08,"limit":50,"percentage":0,"hasLimit":true}],"bonusCredits":[],"addOnCredits":[]}})
    }

    #[test]
    fn exact_usage_and_native_date_are_preserved_without_inventing_a_hard_limit() {
        let snapshot = parse(&fixture()).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, Some(0.16));
        assert_eq!(snapshot.windows[0].scope.as_deref(), Some("套餐内（Kiro）"));
        assert_eq!(snapshot.windows[0].resets_at_unix, None);
        assert_eq!(
            snapshot.plan_type.as_deref(),
            Some("KIRO FREE · 重置 2026-10-01（原生日期）")
        );
        assert_eq!(
            snapshot.stale_after_unix_ms - snapshot.collected_at_unix_ms,
            30_000
        );
    }

    #[test]
    fn overages_and_supplemental_pools_do_not_become_exhausted_account_limits() {
        let mut data = fixture()["data"].clone();
        data["usageBreakdowns"][0]["used"] = json!(75);
        data["overagesEnabled"] = json!(true);
        data["bonusCredits"] = json!([{}]);
        data["addOnCredits"] = json!([{}]);
        data["billingCycleReset"] = json!("2026-10-01T00:00:00Z");
        let snapshot = parse(&data).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, Some(100.0));
        assert!(snapshot.windows[0].scope.is_some());
        assert!(snapshot.windows[0].resets_at_unix.is_some());
        let plan = snapshot.plan_type.unwrap();
        for label in ["按量付费已开启", "另有赠送额度", "另有加购额度"] {
            assert!(plan.contains(label));
        }
    }

    #[test]
    fn rejects_failed_missing_unlimited_or_malformed_allowances() {
        for payload in [
            json!({}),
            json!({"success":false,"message":"secret-native-message","data":fixture()["data"]}),
            json!({"usageBreakdowns":[]}),
        ] {
            let error = parse(&payload).unwrap_err().to_string();
            assert!(!error.contains("secret-native-message"));
        }
        for (key, value) in [
            ("limit", json!(0)),
            ("used", json!(-1)),
            ("used", json!(null)),
            ("limit", json!("50")),
            ("hasLimit", json!(false)),
        ] {
            let mut payload = fixture();
            payload["data"]["usageBreakdowns"][0][key] = value;
            assert!(parse(&payload).is_err());
        }
    }

    #[tokio::test]
    async fn account_refresh_reuses_connection_caches_and_preserves_expiry_on_failure() {
        let driver = AcpDriver::new(
            "kiro",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: PathBuf::from("/missing-kiro-must-not-spawn"),
                args: vec![],
            },
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let (commands, mut receiver) = mpsc::unbounded_channel();
        let (cancels, _cancels) = mpsc::unbounded_channel();
        *driver.shared.connection.lock().await = Some(ConnectionHandle {
            commands,
            cancels,
            finished: Arc::new(AtomicBool::new(false)),
            cursor_force: false,
            codebuddy_cwd: None,
        });
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                let Command::ReadKiroUsage { reply } = command else {
                    panic!("account query must not create sessions or prompts")
                };
                let index = count.fetch_add(1, Ordering::SeqCst);
                let result = match index.cmp(&2) {
                    std::cmp::Ordering::Less => Ok(fixture()),
                    std::cmp::Ordering::Equal => {
                        Err(DriverError::Disconnected("fixture offline".into()))
                    }
                    std::cmp::Ordering::Greater => {
                        Err(DriverError::NotConfigured("authentication_failed".into()))
                    }
                };
                let _ = reply.send(result);
            }
        });
        let (first, second) = tokio::join!(driver.read_quota(None), driver.read_quota(None));
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let fresh = driver.read_quota_with_refresh(None, true).await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(driver.read_quota_with_refresh(None, true).await.is_err());
        assert_eq!(driver.read_quota(None).await.unwrap(), fresh);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert!(matches!(
            driver.read_quota_with_refresh(None, true).await,
            Err(DriverError::NotConfigured(_))
        ));
        assert!(matches!(
            driver.read_quota(None).await,
            Err(DriverError::NotConfigured(_))
        ));
        assert!(driver.shared.quota.account_snapshot(None).is_none());
        assert_eq!(requests.load(Ordering::SeqCst), 4);
        drop(driver);
        server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires local logged-in Kiro; account read only, no session or model turn"]
    async fn real_kiro_account_read_only() {
        let executable = PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/kiro-cli");
        let driver = AcpDriver::new(
            "kiro",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable,
                args: ["acp", "--agent-engine", "v3", "--auth-method", "cli"]
                    .map(str::to_owned)
                    .to_vec(),
            },
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let snapshot = driver.read_quota(None).await.unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert!(snapshot.windows[0].used_percent.is_some());
        assert_eq!(snapshot.source, "native_pull");
        let cached = driver.read_quota(None).await.unwrap();
        assert_eq!(snapshot.collected_at_unix_ms, cached.collected_at_unix_ms);
        let refreshed = driver.read_quota_with_refresh(None, true).await.unwrap();
        assert!(refreshed.collected_at_unix_ms >= snapshot.collected_at_unix_ms);
    }
}
