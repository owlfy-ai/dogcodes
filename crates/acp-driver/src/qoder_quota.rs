//! Qoder CN account credits, read through the CLI's own SDK control channel.
//!
//! ACP carries no account usage. The CLI's SDK mode answers
//! `daemon.get_usage_info` after `daemon.authenticate` with `{type: "qodercli"}`,
//! which makes the CLI load its own local login: no credential ever reaches
//! this process. No session is opened and no model turn is sent.
use super::{
    AcpLaunch, DriverError, ENV_ALLOWLIST, QuotaSnapshot, QuotaWindow, driver_error_from_channel,
    lifecycle, now_unix_ms,
};
use agent_runtime::Pipe;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// The CLI's daemon control protocol version this reader speaks.
const DAEMON_PROTOCOL_VERSION: u64 = 2;
/// Credits change with every turn; a reading is kept for one minute.
const QUOTA_TTL_MS: i64 = 60_000;
const SDK_ARGS: [&str; 5] = [
    "--sdk",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
];

/// The `usage` object of a successful `daemon.get_usage_info` reply.
pub(super) async fn read(launch: &AcpLaunch) -> Result<serde_json::Value, DriverError> {
    let mut command = if super::electron_node_script(launch) {
        let node = agent_runtime::electron_node_executable(&launch.executable.path)
            .map_err(|error| DriverError::NotConfigured(error.to_string()))?;
        let mut command = agent_runtime::TargetCommand::new(launch.executable.target.clone(), node);
        command
            .env("ELECTRON_RUN_AS_NODE", "1")
            .arg(&launch.args[0]);
        command
    } else {
        launch.executable.command()
    };
    command
        .args(SDK_ARGS)
        .current_dir(agent_runtime::external_path(
            launch.executable.target.temp_directory(),
        ))
        .stdio(Pipe::Piped, Pipe::Piped, Pipe::Null);
    for variable in ENV_ALLOWLIST {
        command.inherit(variable);
    }
    let mut child = command
        .spawn()
        .map_err(|_| DriverError::Disconnected("Qoder usage query could not start".into()))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let process = lifecycle::Process::start_with_grace(child, None, std::time::Duration::ZERO);
    let result = tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let mut stdin = stdin.ok_or_else(driver_error_from_channel)?;
        let stdout = stdout.ok_or_else(driver_error_from_channel)?;
        // Bound the whole conversation, including an unterminated line.
        let mut reader = BufReader::new(stdout.take(4 * 1_048_576));
        let mut control = async |id: &str, request: serde_json::Value| {
            let line = serde_json::json!({
                "type": "control_request",
                "request_id": id,
                "request": request,
            });
            stdin
                .write_all(format!("{line}\n").as_bytes())
                .await
                .map_err(|_| driver_error_from_channel())?;
            stdin
                .flush()
                .await
                .map_err(|_| driver_error_from_channel())?;
            response(&mut reader, id).await
        };
        control(
            "dogcodes-initialize",
            serde_json::json!({
                "subtype": "daemon.initialize",
                "daemon_protocol_version": DAEMON_PROTOCOL_VERSION,
                "sdk_version": concat!("dogcodes-", env!("CARGO_PKG_VERSION")),
                "capabilities": ["multi_session_v2"],
            }),
        )
        .await?;
        let auth = control(
            "dogcodes-authenticate",
            serde_json::json!({"subtype": "daemon.authenticate", "auth": {"type": "qodercli"}}),
        )
        .await?;
        if auth["authenticated"] != true {
            return Err(DriverError::NotConfigured(
                "Qoder CLI is not signed in".into(),
            ));
        }
        let usage = control(
            "dogcodes-usage",
            serde_json::json!({"subtype": "daemon.get_usage_info"}),
        )
        .await?;
        Ok(usage["usage"].clone())
    })
    .await
    .unwrap_or_else(|_| {
        Err(DriverError::Disconnected(
            "Qoder usage query timed out".into(),
        ))
    });
    process.control.request_close();
    // Keep this helper's admission until its OS scope is proven gone.
    while process.control.closed().await.is_err() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    result
}

/// Reads stdout until the reply to `id`; other stream messages are skipped.
async fn response(
    reader: &mut (impl AsyncBufReadExt + Unpin),
    id: &str,
) -> Result<serde_json::Value, DriverError> {
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .await
            .map_err(|_| driver_error_from_channel())?
            == 0
        {
            return Err(DriverError::Disconnected(
                "Qoder usage query ended without a reply".into(),
            ));
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let reply = &message["response"];
        if message["type"] != "control_response" || reply["request_id"] != id {
            continue;
        }
        return if reply["subtype"] == "success" {
            Ok(reply["response"].clone())
        } else {
            // Native messages may name the account; only the code leaves here.
            let code = reply["code"].as_str().unwrap_or("unknown");
            Err(match code {
                "DAEMON_AUTH_REQUIRED" | "DAEMON_NOT_AUTHENTICATED" => {
                    DriverError::NotConfigured("Qoder CLI is not signed in".into())
                }
                _ if reply["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("credentials")) =>
                {
                    DriverError::NotConfigured("Qoder CLI is not signed in".into())
                }
                _ => DriverError::Protocol(format!("Qoder usage query failed ({code})")),
            })
        };
    }
}

pub(super) fn parse(usage: &serde_json::Value) -> Result<QuotaSnapshot, DriverError> {
    let invalid = || DriverError::Protocol("Qoder account usage unavailable or invalid".into());
    let plan = &usage["userQuota"];
    let used = plan["used"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(invalid)?;
    let total = plan["total"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(invalid)?;
    // Native `percentage` is rounded to whole numbers; derive from exact usage.
    let used_percent = (used / total * 100.0).clamp(0.0, 100.0);
    // Add-on credits keep requests available after the plan's credits run out,
    // so the plan pool is an included allowance, not an account-wide limit.
    let purchased_quota = usage["addOnQuota"]["remaining"]
        .as_f64()
        .is_some_and(|remaining| remaining > 0.0);
    let plan_type = usage["userType"]
        .as_str()
        .filter(|kind| !kind.is_empty() && *kind != "invalid")
        .map(plan_name);
    let now = now_unix_ms();
    Ok(QuotaSnapshot {
        windows: vec![QuotaWindow {
            scope: Some("included".into()),
            id: "qoder-included".into(),
            window_minutes: 0,
            used_percent: Some(used_percent),
            // `expiresAt` is when the plan ends, not when credits reset.
            resets_at_unix: None,
        }],
        plan_type,
        purchased_quota,
        collected_at_unix_ms: now,
        stale_after_unix_ms: now.saturating_add(QUOTA_TTL_MS),
        source: "native_pull".into(),
        confidence: "provider_reported".into(),
        parser_version: "qoder-usage-info-v1".into(),
        ..Default::default()
    })
}

/// `personal_professional_trial` reads as "Personal Professional Trial".
fn plan_name(kind: &str) -> String {
    kind.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> serde_json::Value {
        json!({"userId":"u","userType":"personal_professional_trial","totalUsagePercentage":3,"isHighestTier":false,"expiresAt":1_796_870_846_033_i64,"isQuotaExceeded":false,"usageType":"credits","userQuota":{"total":300,"used":10,"remaining":290,"percentage":3,"unit":"credits"},"isPlanQuotaProrated":false,"addOnQuota":{"total":100,"used":0,"remaining":100,"percentage":0,"unit":"credits"}})
    }

    #[test]
    fn plan_credits_become_an_included_window_with_exact_usage() {
        let snapshot = parse(&fixture()).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        let window = &snapshot.windows[0];
        assert_eq!(window.scope.as_deref(), Some("included"));
        assert!((window.used_percent.unwrap() - 10.0 / 3.0).abs() < 1e-9);
        assert_eq!(window.resets_at_unix, None);
        assert_eq!(
            snapshot.plan_type.as_deref(),
            Some("Personal Professional Trial")
        );
        assert!(snapshot.purchased_quota);
        assert_eq!(snapshot.confidence, "provider_reported");
    }

    #[test]
    fn spent_add_ons_and_unknown_plans_are_not_reported() {
        let mut usage = fixture();
        usage["addOnQuota"]["remaining"] = json!(0);
        usage["userType"] = json!("invalid");
        usage["userQuota"]["used"] = json!(400);
        let snapshot = parse(&usage).unwrap();
        assert!(!snapshot.purchased_quota);
        assert_eq!(snapshot.plan_type, None);
        assert_eq!(snapshot.windows[0].used_percent, Some(100.0));
    }

    #[test]
    fn rejects_missing_or_malformed_plan_credits() {
        for (key, value) in [
            ("total", json!(0)),
            ("used", json!(-1)),
            ("used", json!(null)),
            ("total", json!("300")),
        ] {
            let mut usage = fixture();
            usage["userQuota"][key] = value;
            assert!(parse(&usage).is_err());
        }
        assert!(parse(&json!(null)).is_err());
    }

    #[tokio::test]
    #[ignore = "requires the signed-in Qoder CN desktop app; account read only, no session or model turn"]
    async fn real_qoder_cn_account_read_only() {
        use crate::{AcpDriver, QuotaProvider, SessionResumeCapability};
        let executable = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap())
            .join("Programs/Qoder CN/Qoder CN.exe");
        let script = agent_runtime::electron_resources_directory(&executable)
            .unwrap()
            .join("app.asar.unpacked/node_modules/@qoder-ai/qoder-cn-agent-sdk/dist/_worker/qoder-worker-runtime.obf.mjs");
        let driver = AcpDriver::new(
            "qoder-cn",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: executable.into(),
                args: vec![script.to_string_lossy().into_owned(), "--acp".into()],
            },
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let started = std::time::Instant::now();
        let snapshot = driver.read_quota(None).await.unwrap();
        println!(
            "qoder-cn quota_ms={} used_percent={:?} plan={:?} add_on={}",
            started.elapsed().as_millis(),
            snapshot.windows[0].used_percent,
            snapshot.plan_type,
            snapshot.purchased_quota
        );
        assert_eq!(snapshot.source, "native_pull");
        // A fresh reading is served from the cache without another process.
        let cached = driver.read_quota(None).await.unwrap();
        assert_eq!(snapshot.collected_at_unix_ms, cached.collected_at_unix_ms);
    }

    #[tokio::test]
    async fn replies_are_matched_by_id_and_failures_keep_native_text_out() {
        let lines = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
            "not json\n",
            "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"other\",\"response\":{}}}\n",
            "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"a\",\"response\":{\"ok\":1}}}\n",
            "{\"type\":\"control_response\",\"response\":{\"subtype\":\"error\",\"request_id\":\"b\",\"error\":\"daemon has no credentials for someone@example.com\",\"code\":\"DAEMON_COMMAND_FAILED\"}}\n",
            "{\"type\":\"control_response\",\"response\":{\"subtype\":\"error\",\"request_id\":\"c\",\"error\":\"private detail\",\"code\":\"X\"}}\n",
        );
        let mut reader = BufReader::new(lines.as_bytes());
        assert_eq!(response(&mut reader, "a").await.unwrap(), json!({"ok":1}));
        let signed_out = response(&mut reader, "b").await.unwrap_err();
        assert!(matches!(signed_out, DriverError::NotConfigured(_)));
        assert!(!signed_out.to_string().contains("example.com"));
        let failed = response(&mut reader, "c").await.unwrap_err().to_string();
        assert!(!failed.contains("private detail"));
        assert!(response(&mut reader, "d").await.is_err());
    }
}
