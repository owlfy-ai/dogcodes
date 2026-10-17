//! Read-only subscription lookup using Claude's existing OAuth credentials.
//! Never refresh/write credentials or send a model prompt.
use driver_api::{QuotaSnapshot, QuotaWindow};
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

const RATE_LIMITED: &str = "claude_quota_rate_limited";
/// Claude Code renews its own access token whenever it runs; this driver never does.
const TOKEN_EXPIRED: &str = "claude_quota_token_expired";
const MIN_BACKOFF_MS: i64 = 60_000;
const MAX_BACKOFF_MS: i64 = 3_600_000;

#[derive(Default)]
pub(super) struct Cache {
    pub(super) next_attempt: i64,
    /// Set after a 429; not even a forced refresh may query before it.
    backoff_until: i64,
    rate_limits: u32,
    snapshot: Option<QuotaSnapshot>,
    pub(super) failure: Option<&'static str>,
}

/// A failed pull, with the server's requested wait when it sent one.
#[derive(Debug)]
struct PullError {
    reason: &'static str,
    retry_after_ms: Option<i64>,
}

impl From<&'static str> for PullError {
    fn from(reason: &'static str) -> Self {
        Self {
            reason,
            retry_after_ms: None,
        }
    }
}

impl Cache {
    /// The last pull failed for a reason that leaves the previous reading outdated
    /// rather than wrong, so it must not be presented as current.
    pub(super) fn withholds_reading(&self) -> bool {
        matches!(self.failure, Some(RATE_LIMITED | TOKEN_EXPIRED))
    }

    fn due(&self, now: i64, force: bool) -> bool {
        (force && now >= self.backoff_until) || now >= self.next_attempt
    }

    pub(super) async fn read(
        &mut self,
        force: bool,
        version: Option<&str>,
    ) -> Option<QuotaSnapshot> {
        let now = chrono::Utc::now().timestamp_millis();
        if self.due(now, force) {
            self.next_attempt = now + 60_000;
            match tokio::time::timeout(Duration::from_secs(20), pull(now, version)).await {
                Ok(Ok(snapshot)) => {
                    self.snapshot = Some(snapshot);
                    self.failure = None;
                    self.rate_limits = 0;
                }
                Ok(Err(error)) => {
                    if error.reason == "claude_quota_login_required" {
                        self.snapshot = None;
                    }
                    if error.reason == RATE_LIMITED {
                        self.back_off(now, error.retry_after_ms);
                    }
                    self.failure = Some(error.reason);
                }
                Err(_) => self.failure = Some("claude_quota_timeout"),
            }
        }
        if force && self.failure.is_some() {
            return None;
        }
        self.snapshot.clone()
    }

    /// Honour Retry-After; without one, wait 1, 2, 4… minutes. Polling on
    /// schedule while limited would only keep the limit in place.
    fn back_off(&mut self, now: i64, retry_after_ms: Option<i64>) {
        self.rate_limits = self.rate_limits.saturating_add(1);
        let doubling = MIN_BACKOFF_MS.saturating_mul(1 << (self.rate_limits - 1).min(6));
        let wait = retry_after_ms
            .unwrap_or(doubling)
            .clamp(MIN_BACKOFF_MS, MAX_BACKOFF_MS);
        self.backoff_until = now + wait;
        self.next_attempt = self.next_attempt.max(self.backoff_until);
    }
}

fn token_expired(oauth: Option<&Value>, now: i64) -> bool {
    oauth
        .and_then(|v| v.get("expiresAt")?.as_i64())
        .is_some_and(|expires| expires <= now)
}

/// Retry-After is either delay-seconds or an HTTP date.
fn retry_after_ms(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds.saturating_mul(1_000));
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|at| at.timestamp_millis() - now)
}

async fn credentials() -> Option<Value> {
    let custom = std::env::var_os("CLAUDE_CONFIG_DIR");
    if custom.is_none() && cfg!(target_os = "macos") {
        let output = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-w",
                "-s",
                "Claude Code-credentials",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;
        if let Some(output) = output.ok().filter(|output| output.status.success())
            && let Ok(value) = serde_json::from_slice(&output.stdout)
        {
            return Some(value);
        }
    }
    let directory = custom.map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".claude"))
    })?;
    let bytes = tokio::fs::read(directory.join(".credentials.json"))
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn pull(now: i64, version: Option<&str>) -> Result<QuotaSnapshot, PullError> {
    let directory = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".claude"))
        });
    let settings: Option<Value> = match directory {
        Some(directory) => tokio::fs::read(directory.join("settings.json"))
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
        None => None,
    };
    let base = std::env::var("ANTHROPIC_BASE_URL").ok().or_else(|| {
        settings.as_ref()?["env"]["ANTHROPIC_BASE_URL"]
            .as_str()
            .map(str::to_owned)
    });
    if base.is_some_and(|base| base.trim_end_matches('/') != "https://api.anthropic.com") {
        return Err("claude_quota_custom_provider".into());
    }
    let stored = credentials().await;
    let oauth = stored.as_ref().and_then(|v| v.get("claudeAiOauth"));
    let configured = std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().or_else(|| {
        settings.as_ref()?["env"]["CLAUDE_CODE_OAUTH_TOKEN"]
            .as_str()
            .map(str::to_owned)
    });
    // An expired stored token is answered with a misleading 429 rather than 401;
    // don't ask at all. Claude Code refreshes it on its next run.
    if configured.is_none() && token_expired(oauth, now) {
        return Err(TOKEN_EXPIRED.into());
    }
    let token = configured
        .or_else(|| oauth?.get("accessToken")?.as_str().map(str::to_owned))
        .ok_or("claude_quota_login_required")?;
    // The credential goes through stdin, never process arguments or logs.
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
    {
        return Err("claude_quota_login_required".into());
    }
    let mut command = Command::new("curl");
    command
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--max-time",
            "12",
            "--max-filesize",
            "1048576",
            "--write-out",
            "\n%header{retry-after}\n%{http_code}",
            "--config",
            "-",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    agent_runtime::apply_proxy_environment(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| "claude_quota_transport_unavailable")?;
    // Identify as Claude Code so the service answers with its real status codes.
    let agent = version
        .filter(|v| {
            !v.is_empty()
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        })
        .map_or_else(|| "claude-code".to_owned(), |v| format!("claude-code/{v}"));
    let config = format!(
        "header = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nuser-agent = \"{agent}\"\n"
    );
    child
        .stdin
        .take()
        .ok_or("claude_quota_network_error")?
        .write_all(config.as_bytes())
        .await
        .map_err(|_| "claude_quota_network_error")?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| "claude_quota_network_error")?;
    // The body is followed by a Retry-After line and the status line.
    let split = output
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("claude_quota_network_error")?;
    let header = output.stdout[..split]
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("claude_quota_network_error")?;
    match &output.stdout[split + 1..] {
        b"200" if output.status.success() => {}
        b"401" | b"403" => return Err("claude_quota_login_required".into()),
        b"429" => {
            return Err(PullError {
                reason: RATE_LIMITED,
                retry_after_ms: std::str::from_utf8(&output.stdout[header + 1..split])
                    .ok()
                    .and_then(|value| retry_after_ms(value, now)),
            });
        }
        _ => return Err("claude_quota_network_error".into()),
    }
    let value = serde_json::from_slice(&output.stdout[..header])
        .map_err(|_| "claude_quota_invalid_response")?;
    parse(
        &value,
        now,
        oauth
            .and_then(|v| v["subscriptionType"].as_str())
            .map(str::to_owned),
    )
    .ok_or_else(|| "claude_quota_no_windows".into())
}

fn parse(value: &Value, now: i64, plan: Option<String>) -> Option<QuotaSnapshot> {
    let mut windows = Vec::new();
    for (id, minutes) in [("five_hour", 300), ("seven_day", 10080)] {
        let node = &value[id];
        let Some(used) = node["utilization"]
            .as_f64()
            .filter(|v| (0.0..=100.0).contains(v))
        else {
            continue;
        };
        windows.push(QuotaWindow {
            id: id.into(),
            scope: None,
            window_minutes: minutes,
            // HTTP usage reports percentages, unlike stream-json fractions.
            used_percent: Some(used),
            resets_at_unix: node["resets_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp()),
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some(QuotaSnapshot {
        session_usage: None,
        windows,
        context: None,
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        plan_type: plan,
        source: "native_pull".into(),
        confidence: "provider_reported".into(),
        parser_version: "claude-oauth-usage-v1".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "reads local Claude credentials and queries the official usage endpoint"]
    async fn real_usage_read_only() {
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            pull(chrono::Utc::now().timestamp_millis(), None),
        )
        .await;
        let snapshot = result
            .expect("usage request timed out")
            .expect("usage unavailable: check native OAuth login, scopes and network");
        assert!(!snapshot.windows.is_empty());
        println!(
            "Claude official usage verified: {} windows",
            snapshot.windows.len()
        );
    }
    #[test]
    fn rate_limits_back_off_even_for_forced_refreshes() {
        let mut cache = Cache::default();
        cache.back_off(0, None);
        assert_eq!(cache.backoff_until, 60_000);
        assert!(!cache.due(59_999, true));
        assert!(cache.due(60_000, true));
        cache.back_off(60_000, None);
        assert_eq!(cache.backoff_until, 180_000);
        cache.back_off(0, Some(5_000));
        assert_eq!(cache.backoff_until, MIN_BACKOFF_MS);
        cache.back_off(0, Some(7_200_000));
        assert_eq!(cache.backoff_until, MAX_BACKOFF_MS);
        assert!(cache.next_attempt >= cache.backoff_until);
    }

    #[test]
    fn expired_stored_tokens_are_detected_without_a_request() {
        let oauth = serde_json::json!({"accessToken": "t", "expiresAt": 1_000});
        assert!(token_expired(Some(&oauth), 1_000));
        assert!(!token_expired(Some(&oauth), 999));
        assert!(!token_expired(
            Some(&serde_json::json!({"accessToken": "t"})),
            1_000
        ));
        let cache = Cache {
            failure: Some(TOKEN_EXPIRED),
            ..Cache::default()
        };
        assert!(cache.withholds_reading());
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_ms(" 120 ", 0), Some(120_000));
        let now = chrono::DateTime::parse_from_rfc3339("2015-10-21T07:27:30Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            retry_after_ms("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(30_000)
        );
        assert_eq!(retry_after_ms("", now), None);
        assert_eq!(retry_after_ms("%header{retry-after}", now), None);
    }

    #[test]
    fn percentages_are_not_stream_fractions() {
        let snapshot = parse(&serde_json::json!({"five_hour":{"utilization":0.5},"seven_day":{"utilization":100,"resets_at":"2026-09-13T00:00:00Z"}}), 10, None).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, Some(0.5));
        assert_eq!(snapshot.windows[1].used_percent, Some(100.0));
        assert!(snapshot.windows[1].resets_at_unix.is_some());
        assert!(
            parse(
                &serde_json::json!({"five_hour":{"utilization":-1}}),
                10,
                None
            )
            .is_none()
        );
    }
}
