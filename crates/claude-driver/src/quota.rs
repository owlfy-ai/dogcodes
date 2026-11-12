//! Read-only subscription lookup using Claude's existing OAuth credentials.
//! Never refresh/write credentials or send a model prompt.
use agent_runtime::SharedTarget;
use driver_api::{QuotaSnapshot, QuotaWindow};
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const WRITE_OUT: &str = "\n%header{retry-after}\n%{http_code}";
const RATE_LIMITED: &str = "claude_quota_rate_limited";
/// Claude Code renews its own access token whenever it runs; this driver never does.
const TOKEN_EXPIRED: &str = "claude_quota_token_expired";
const MIN_BACKOFF_MS: i64 = 60_000;
const MAX_BACKOFF_MS: i64 = 3_600_000;

#[derive(Default)]
pub(super) struct Cache {
    // The cache guard can be cancelled before physical helper retirement.
    // This permit stays with each original helper until its tree is gone.
    admission: Arc<Mutex<()>>,
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
        target: &SharedTarget,
    ) -> Option<QuotaSnapshot> {
        let now = chrono::Utc::now().timestamp_millis();
        if self.due(now, force) {
            self.next_attempt = now + 60_000;
            match tokio::time::timeout(Duration::from_secs(20), async {
                // Whichever machine the agent runs on holds the account to report.
                if target.is_local() {
                    pull(now, version, self.admission.clone()).await
                } else {
                    pull_on_target(now, version, target).await
                }
            })
            .await
            {
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

async fn credentials(admission: Arc<Mutex<()>>) -> Option<Value> {
    let custom = std::env::var_os("CLAUDE_CONFIG_DIR");
    if custom.is_none() && cfg!(target_os = "macos") {
        let mut command = Command::new("/usr/bin/security");
        command
            .args([
                "find-generic-password",
                "-w",
                "-s",
                "Claude Code-credentials",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            command_output(command, None, 1024 * 1024, admission),
        )
        .await
        .ok()
        .and_then(Result::ok);
        if let Some(output) = output.filter(|output| output.status.success())
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

/// What an account has on the machine it is signed in on.
struct Account {
    settings: Option<Value>,
    stored: Option<Value>,
    base_url: Option<String>,
    token: Option<String>,
}

async fn local_account(admission: Arc<Mutex<()>>) -> Account {
    let directory = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".claude"))
        });
    Account {
        settings: match directory {
            Some(directory) => tokio::fs::read(directory.join("settings.json"))
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
            None => None,
        },
        stored: credentials(admission).await,
        base_url: std::env::var("ANTHROPIC_BASE_URL").ok(),
        token: std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok(),
    }
}

/// The shim asks with the login on its own machine, which never leaves it.
async fn pull_on_target(
    now: i64,
    version: Option<&str>,
    target: &SharedTarget,
) -> Result<QuotaSnapshot, PullError> {
    let request = agent_management::operation::Request::ClaudeUsage {
        version: version.map(str::to_owned),
    };
    let usage: agent_management::account::ClaudeUsage =
        agent_management::operation::call(target, &request)
            .await
            .map_err(|_| PullError::from("claude_quota_network_error"))?;
    if let Some(reason) = usage.failure.as_deref() {
        let reason = match reason {
            "claude_quota_custom_provider" => "claude_quota_custom_provider",
            "claude_quota_rate_limited" => RATE_LIMITED,
            "claude_quota_token_expired" => TOKEN_EXPIRED,
            "claude_quota_login_required" => "claude_quota_login_required",
            "claude_quota_invalid_response" => "claude_quota_invalid_response",
            _ => "claude_quota_network_error",
        };
        return Err(PullError {
            reason,
            retry_after_ms: usage
                .retry_after
                .as_deref()
                .and_then(|value| retry_after_ms(value, now)),
        });
    }
    let body = usage.body.ok_or("claude_quota_invalid_response")?;
    parse(&body, now, usage.subscription).ok_or_else(|| "claude_quota_no_windows".into())
}

async fn pull(
    now: i64,
    version: Option<&str>,
    admission: Arc<Mutex<()>>,
) -> Result<QuotaSnapshot, PullError> {
    let account = local_account(admission.clone()).await;
    let settings = account.settings;
    let base = account.base_url.or_else(|| {
        settings.as_ref()?["env"]["ANTHROPIC_BASE_URL"]
            .as_str()
            .map(str::to_owned)
    });
    if base.is_some_and(|base| base.trim_end_matches('/') != "https://api.anthropic.com") {
        return Err("claude_quota_custom_provider".into());
    }
    let stored = account.stored;
    let oauth = stored.as_ref().and_then(|v| v.get("claudeAiOauth"));
    let configured = account.token.or_else(|| {
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
    // Identify as Claude Code so the service answers with its real status codes.
    let agent = version
        .filter(|v| {
            !v.is_empty()
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        })
        .map_or_else(|| "claude-code".to_owned(), |v| format!("claude-code/{v}"));
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
            WRITE_OUT,
            "--config",
            "-",
            USAGE_URL,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    agent_runtime::apply_proxy_environment(&mut command);
    let config = format!(
        "header = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nuser-agent = \"{agent}\"\n"
    );
    let output = command_output(
        command,
        Some(config.into_bytes()),
        1024 * 1024 + 4096,
        admission,
    )
    .await?;
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

/// Keep stdin delivery, bounded output and original tree retirement under one
/// owner. Cancelling a UI wait must not release admission during cleanup, nor
/// leave a partially written credential request to another query.
async fn command_output(
    command: Command,
    input: Option<Vec<u8>>,
    limit: usize,
    admission: Arc<Mutex<()>>,
) -> Result<std::process::Output, PullError> {
    let permit = admission.lock_owned().await;
    let (mut reply, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        if reply.is_closed() {
            return;
        }
        let Ok(mut child) = agent_runtime::OwnedChild::spawn(command) else {
            let _ = reply.send(Err("claude_quota_transport_unavailable".into()));
            return;
        };
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let io = async {
            let write = async {
                if let Some(input) = input {
                    let mut stdin = stdin.take().ok_or("claude_quota_network_error")?;
                    stdin
                        .write_all(&input)
                        .await
                        .map_err(|_| "claude_quota_network_error")?;
                    stdin
                        .shutdown()
                        .await
                        .map_err(|_| "claude_quota_network_error")?;
                }
                drop(stdin.take());
                Ok::<_, PullError>(())
            };
            let read = async {
                let mut bytes = Vec::new();
                stdout
                    .ok_or("claude_quota_network_error")?
                    .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| "claude_quota_network_error")?;
                if bytes.len() > limit {
                    return Err("claude_quota_network_error".into());
                }
                Ok::<_, PullError>(bytes)
            };
            let (status, (), stdout) = tokio::try_join!(
                async {
                    child
                        .wait()
                        .await
                        .map_err(|_| PullError::from("claude_quota_network_error"))
                },
                write,
                read,
            )?;
            Ok(std::process::Output {
                status,
                stdout,
                stderr: Vec::new(),
            })
        };
        let outcome = tokio::select! {
            biased;
            () = reply.closed() => None,
            result = io => Some(result),
        };
        let mut delay = 1;
        while child.terminate().await.is_err() {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
        if let Some(outcome) = outcome {
            let _ = reply.send(outcome);
        }
    });
    result
        .await
        .map_err(|_| PullError::from("claude_quota_network_error"))?
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
        balances: Vec::new(),
        session_usage: None,
        windows,
        context: None,
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        plan_type: plan,
        source: "native_pull".into(),
        confidence: "provider_reported".into(),
        parser_version: "claude-oauth-usage-v1".into(),
        ..Default::default()
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
            pull(chrono::Utc::now().timestamp_millis(), None, Arc::default()),
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
    /// Answers as a server's shim would, whatever was asked.
    #[cfg(unix)]
    struct Elsewhere(PathBuf);

    #[cfg(unix)]
    impl agent_runtime::ExecutionTarget for Elsewhere {
        fn is_local(&self) -> bool {
            false
        }
        fn inherited(&self, _: &str) -> Option<std::ffi::OsString> {
            None
        }
        fn temp_directory(&self) -> PathBuf {
            std::env::temp_dir()
        }
        fn shim_program(&self) -> Option<String> {
            Some(self.0.display().to_string())
        }
        fn spawn(
            &self,
            request: agent_runtime::ProcessRequest,
        ) -> std::io::Result<agent_runtime::OwnedChild> {
            agent_runtime::local_target().spawn(request)
        }
    }

    #[cfg(unix)]
    fn shim_answering(directory: &std::path::Path, reply: &str) -> SharedTarget {
        use std::os::unix::fs::PermissionsExt;
        let shim = directory.join("shim");
        std::fs::write(
            &shim,
            format!("#!/bin/sh\ncat > \"$0.request\"\ncat <<'REPLY'\n{reply}\nREPLY\n"),
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        Arc::new(Elsewhere(shim))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_agent_elsewhere_reports_what_its_own_machine_answered() {
        let directory = tempfile::tempdir().unwrap();
        let target = shim_answering(
            directory.path(),
            r#"{"Ok":{"failure":null,"body":{"five_hour":{"utilization":40}},"subscription":"max","retry_after":null}}"#,
        );
        let snapshot = pull_on_target(10, Some("2.0.0"), &target).await.unwrap();
        assert_eq!(snapshot.windows[0].used_percent, Some(40.0));
        assert_eq!(snapshot.plan_type.as_deref(), Some("max"));
        let request = std::fs::read_to_string(directory.path().join("shim.request")).unwrap();
        assert!(request.contains("claude_usage") && request.contains("2.0.0"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_login_missing_over_there_is_reported_as_such() {
        let directory = tempfile::tempdir().unwrap();
        let target = shim_answering(
            directory.path(),
            r#"{"Ok":{"failure":"claude_quota_login_required","body":null,"subscription":null,"retry_after":null}}"#,
        );
        let failure = pull_on_target(10, None, &target).await.unwrap_err();
        assert_eq!(failure.reason, "claude_quota_login_required");
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

#[cfg(all(test, unix))]
mod scope_tests;
