//! Ask the native Claude process about its own account; never read or renew credentials here.
use super::{Classified, ClaudeDriver, PROBE_TIMEOUT, native::NativeProcess};
use agent_runtime::TargetExecutable;
use driver_api::{QuotaSnapshot, QuotaWindow};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, watch},
};

const RATE_LIMITED: &str = "claude_quota_rate_limited";
#[derive(Default)]
pub(super) struct Cache {
    admission: Arc<Mutex<()>>,
    pub(super) next_attempt: i64,
    snapshot: Option<QuotaSnapshot>,
    pub(super) failure: Option<&'static str>,
}
impl Cache {
    pub(super) fn withholds_reading(&self) -> bool {
        self.failure.is_some()
    }
    pub(super) async fn read(
        &mut self,
        force: bool,
        driver: &ClaudeDriver,
    ) -> Option<QuotaSnapshot> {
        let now = chrono::Utc::now().timestamp_millis();
        if driver
            .quota
            .auth_recovered
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            self.failure = None;
            self.snapshot = None;
            self.next_attempt = 0;
        }
        if now >= self.next_attempt
            || (force
                && !matches!(
                    self.failure,
                    Some(RATE_LIMITED | "claude_quota_refresh_busy")
                ))
        {
            self.next_attempt = now + 60_000;
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                pull(
                    driver.executable.clone(),
                    driver.native_version.clone(),
                    driver.stopping.subscribe(),
                    self.admission.clone(),
                ),
            )
            .await
            .unwrap_or(Err("claude_quota_timeout"))
            .and_then(|value| parse_native(&value, now));
            match result {
                Ok(snapshot) => {
                    self.snapshot = Some(snapshot);
                    self.failure = None;
                }
                Err(reason) => {
                    self.snapshot = None;
                    self.failure = Some(reason);
                }
            }
        }
        self.snapshot.clone()
    }
}

async fn request(
    stdin: &mut agent_runtime::ProcessInput,
    stdout: &mut BufReader<tokio::io::DuplexStream>,
    id: &str,
    request: Value,
    mut desktop_auth: Option<&mut super::desktop_auth::Lease>,
) -> Result<Value, &'static str> {
    stdin
        .write_all(
            format!(
                "{}\n",
                json!({"type":"control_request","request_id":id,"request":request})
            )
            .as_bytes(),
        )
        .await
        .map_err(|_| "claude_quota_unavailable")?;
    stdin
        .flush()
        .await
        .map_err(|_| "claude_quota_unavailable")?;
    let mut bytes = 0;
    loop {
        let mut line = String::new();
        let size = (&mut *stdout)
            .take(super::FRAME_LIMIT as u64 + 1)
            .read_line(&mut line)
            .await
            .map_err(|_| "claude_quota_unavailable")?;
        bytes += size;
        if size == 0 || bytes > super::FRAME_LIMIT {
            return Err("claude_quota_invalid_response");
        }
        if let Ok(raw) = serde_json::from_str(&line)
            && super::desktop_auth::respond(desktop_auth.as_deref_mut(), &raw, stdin)
                .await
                .map_err(|_| "claude_quota_unavailable")?
        {
            continue;
        }
        if let Some(Classified::ControlResponse {
            request_id,
            success,
            data,
        }) = super::classify(&line)
        {
            if request_id == id {
                return if success {
                    Ok(data)
                } else {
                    let reason = serde_json::from_str::<Value>(&line)
                        .ok()
                        .map_or("claude_quota_unavailable", |raw| {
                            super::turn_failure::control_failure(&raw["response"])
                        });
                    super::desktop_auth::reject_failure(desktop_auth.as_deref(), reason);
                    Err(reason)
                };
            }
        } else if let Ok(raw) = serde_json::from_str::<Value>(&line)
            && let Some(reason) = super::turn_failure::quota_failure(&raw)
        {
            super::desktop_auth::reject_failure(desktop_auth.as_deref(), reason);
            return Err(reason);
        }
    }
}

/// An observer's cancellation cannot release admission before the original CLI retires.
async fn pull(
    executable: TargetExecutable,
    native_version: Option<String>,
    mut owner: watch::Receiver<bool>,
    admission: Arc<Mutex<()>>,
) -> Result<Value, &'static str> {
    let permit = admission.lock_owned().await;
    let (mut reply, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        if reply.is_closed() || owner.has_changed().is_err() || *owner.borrow() {
            return;
        }
        let mut command = executable.command();
        command.args(super::session_args(
            &uuid::Uuid::new_v4().to_string(),
            false,
        ));
        command.args([
            "--no-session-persistence",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--setting-sources",
            "user",
            "--settings",
            "{\"disableAllHooks\":true}",
        ]);
        command.current_dir(executable.target.temp_directory());
        for variable in super::ENV_ALLOWLIST
            .iter()
            .chain(super::environment::AUTH_VARIABLES)
        {
            command.inherit(variable);
        }
        let mut desktop_auth =
            super::desktop_auth::apply(&executable, native_version.as_deref(), &mut command).await;
        let child = command.spawn();
        let Ok(child) = child else {
            let _ = reply.send(Err("claude_quota_unavailable"));
            return;
        };
        let Ok((native, mut stdin, stdout)) =
            NativeProcess::start_with_grace(child, Duration::from_secs(5))
        else {
            let _ = reply.send(Err("claude_quota_unavailable"));
            return;
        };
        native.observe_owner(owner.clone());
        let mut stdout = BufReader::new(stdout);
        let outcome = tokio::select! {
            biased;
            () = reply.closed() => None,
            _ = owner.changed() => None,
            result = tokio::time::timeout(PROBE_TIMEOUT, async {
                request(&mut stdin, &mut stdout, "dogcodes-quota-init", json!({"subtype":"initialize","hooks":{}}), desktop_auth.as_mut()).await?;
                request(&mut stdin, &mut stdout, "dogcodes-quota", json!({"subtype":"get_usage","skip_behaviors":true}), desktop_auth.as_mut()).await
            }) => Some(result.unwrap_or(Err("claude_quota_timeout"))),
        };
        drop(stdin);
        native.request_close();
        native.confirmed().await;
        if let Some(outcome) = outcome {
            let _ = reply.send(outcome);
        }
    });
    result.await.map_err(|_| "claude_quota_unavailable")?
}

fn parse_native(value: &Value, now: i64) -> Result<QuotaSnapshot, &'static str> {
    if value["rate_limits_available"] == false {
        return Err("claude_quota_custom_provider");
    }
    let value_limits = &value["rate_limits"];
    let mut windows = Vec::new();
    for (id, minutes, scope) in [
        ("five_hour", 300, None),
        ("seven_day", 10080, None),
        ("seven_day_opus", 10080, Some("Opus")),
        ("seven_day_sonnet", 10080, Some("Sonnet")),
        ("seven_day_oauth_apps", 10080, Some("OAuth apps")),
    ] {
        let node = &value_limits[id];
        if let Some(used) = node["utilization"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        {
            windows.push(QuotaWindow {
                id: id.into(),
                scope: scope.map(str::to_owned),
                window_minutes: minutes,
                used_percent: Some(used),
                resets_at_unix: node["resets_at"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.timestamp()),
            });
        }
    }
    if windows.is_empty() {
        return Err("claude_quota_no_windows");
    }
    Ok(QuotaSnapshot {
        windows,
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        plan_type: value["subscription_type"].as_str().map(str::to_owned),
        source: "native_control".into(),
        confidence: "provider_reported".into(),
        parser_version: "claude-get-usage-v1".into(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_usage_does_not_invent_limits_from_session_cost_or_empty_readings() {
        assert_eq!(
            parse_native(&json!({"rate_limits_available":false}), 0).unwrap_err(),
            "claude_quota_custom_provider"
        );
        assert_eq!(
            parse_native(
                &json!({"rate_limits_available":true,"rate_limits":null,"total_cost_usd":2}),
                0
            )
            .unwrap_err(),
            "claude_quota_no_windows"
        );
        let value = parse_native(&json!({"subscription_type":"pro","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":40},"seven_day":{"utilization":125},"seven_day_opus":{"utilization":20}}}),10).unwrap();
        assert_eq!(value.windows.len(), 2);
        assert_eq!(value.windows[0].used_percent, Some(40.0));
        assert_eq!(value.plan_type.as_deref(), Some("pro"));
        assert_eq!(value.windows[1].scope.as_deref(), Some("Opus"));
    }
}

#[cfg(all(test, unix))]
mod scope_tests;
