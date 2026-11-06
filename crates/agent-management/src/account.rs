//! Reads an agent's account usage on the machine it is signed in on. The credential stays
//! there: only the provider's answer leaves, and the driver makes sense of it.
use crate::{http::curl, operation::Failure};
use agent_runtime::{Pipe, TargetCommand, local_target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Output, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};

const CLAUDE_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
const CURSOR_ENDPOINT: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/";
const BODY_LIMIT: usize = 1024 * 1024;
const STATUS_TRAILER: &str = "\n%{http_code}";

fn disconnected(message: &str) -> Failure {
    Failure::new("disconnected", message)
}

async fn keychain(args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("/usr/bin/security")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// What Claude's usage endpoint said, or why it wasn't asked.
#[derive(Debug, Deserialize, Serialize)]
pub struct ClaudeUsage {
    /// Empty when the request succeeded, otherwise the reason the driver reports.
    pub failure: Option<String>,
    pub body: Option<Value>,
    pub subscription: Option<String>,
    pub retry_after: Option<String>,
}

impl ClaudeUsage {
    fn failed(reason: &str) -> Self {
        Self {
            failure: Some(reason.to_owned()),
            body: None,
            subscription: None,
            retry_after: None,
        }
    }
}

fn read_json(path: PathBuf) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The status line and the header before it come last, after the body.
fn split_trailer(stdout: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let status = stdout.iter().rposition(|b| *b == b'\n')?;
    let header = stdout[..status].iter().rposition(|b| *b == b'\n')?;
    Some((
        &stdout[..header],
        &stdout[header + 1..status],
        &stdout[status + 1..],
    ))
}

pub async fn claude(version: Option<&str>) -> ClaudeUsage {
    let custom = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let directory = custom.clone().or_else(|| Some(home()?.join(".claude")));
    let settings = directory
        .as_ref()
        .and_then(|d| read_json(d.join("settings.json")));
    let setting = |name: &str| {
        std::env::var(name)
            .ok()
            .or_else(|| settings.as_ref()?["env"][name].as_str().map(str::to_owned))
    };
    if setting("ANTHROPIC_BASE_URL")
        .is_some_and(|base| base.trim_end_matches('/') != "https://api.anthropic.com")
    {
        return ClaudeUsage::failed("claude_quota_custom_provider");
    }
    // Without a custom directory, a Mac keeps the login in its keychain.
    let mut stored = None;
    if custom.is_none() && cfg!(target_os = "macos") {
        stored = keychain(&[
            "find-generic-password",
            "-w",
            "-s",
            "Claude Code-credentials",
        ])
        .await
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    }
    let stored = stored.or_else(|| read_json(directory?.join(".credentials.json")));
    let oauth = stored.as_ref().and_then(|value| value.get("claudeAiOauth"));
    let configured = setting("CLAUDE_CODE_OAUTH_TOKEN");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    // An expired stored token gets a misleading 429: don't ask. Claude renews it itself.
    if configured.is_none()
        && oauth
            .and_then(|value| value.get("expiresAt")?.as_i64())
            .is_some_and(|expires| expires <= now)
    {
        return ClaudeUsage::failed("claude_quota_token_expired");
    }
    let Some(token) = configured.or_else(|| oauth?.get("accessToken")?.as_str().map(str::to_owned))
    else {
        return ClaudeUsage::failed("claude_quota_login_required");
    };
    // Identify as Claude Code so the service answers with its real status codes.
    let agent = version
        .filter(|v| {
            !v.is_empty()
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        })
        .map_or_else(|| "claude-code".to_owned(), |v| format!("claude-code/{v}"));
    let user_agent = format!("User-Agent: {agent}");
    let output = curl(
        Some(&token),
        &["anthropic-beta: oauth-2025-04-20", &user_agent],
        &[
            "--fail",
            "--write-out",
            "\n%header{retry-after}\n%{http_code}",
            CLAUDE_USAGE,
        ],
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            return ClaudeUsage::failed("claude_quota_login_required");
        }
        Err(_) => return ClaudeUsage::failed("claude_quota_network_error"),
    };
    let Some((body, retry_after, status)) = split_trailer(&output.stdout) else {
        return ClaudeUsage::failed("claude_quota_network_error");
    };
    match status {
        b"200" if output.status.success() => {}
        b"401" | b"403" => return ClaudeUsage::failed("claude_quota_login_required"),
        b"429" => {
            return ClaudeUsage {
                retry_after: Some(String::from_utf8_lossy(retry_after).into_owned()),
                ..ClaudeUsage::failed("claude_quota_rate_limited")
            };
        }
        _ => return ClaudeUsage::failed("claude_quota_network_error"),
    }
    let Ok(body) = serde_json::from_slice(body) else {
        return ClaudeUsage::failed("claude_quota_invalid_response");
    };
    ClaudeUsage {
        failure: None,
        body: Some(body),
        subscription: oauth
            .and_then(|value| value["subscriptionType"].as_str())
            .map(str::to_owned),
        retry_after: None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Keychain,
    File(PathBuf),
}

const CURSOR_LOGIN_REQUIRED: &str = "Cursor quota requires a signed-in account";

/// Where Cursor keeps this machine's login. Shared with the driver's own reads here.
///
/// # Errors
/// When there is no login of the kind its quota needs.
pub fn cursor_credential_source(
    platform: &str,
    api_key: Option<&str>,
    store: Option<&str>,
    home: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<CredentialSource, Failure> {
    if api_key.is_some_and(|value| !value.trim().is_empty()) || store == Some("memory") {
        return Err(Failure::new(
            "capability",
            "Cursor subscription quota requires native OAuth credentials",
        ));
    }
    if platform == "macos" && store != Some("file") {
        return Ok(CredentialSource::Keychain);
    }
    let home = home.ok_or_else(|| Failure::new("login", CURSOR_LOGIN_REQUIRED))?;
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

async fn cursor_token() -> Result<String, Failure> {
    let login = || Failure::new("login", CURSOR_LOGIN_REQUIRED);
    let source = cursor_credential_source(
        std::env::consts::OS,
        std::env::var("CURSOR_API_KEY").ok().as_deref(),
        std::env::var("AGENT_CLI_CREDENTIAL_STORE").ok().as_deref(),
        home(),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )?;
    let token = match source {
        CredentialSource::Keychain => keychain(&[
            "find-generic-password",
            "-s",
            "cursor-access-token",
            "-a",
            "cursor-user",
            "-w",
        ])
        .await
        .ok_or_else(login)?,
        CredentialSource::File(path) => read_json(path)
            .and_then(|auth| auth["accessToken"].as_str().map(str::to_owned))
            .ok_or_else(login)?,
    };
    Ok(token.trim().to_owned()).and_then(|token| {
        if token.is_empty() {
            Err(login())
        } else {
            Ok(token)
        }
    })
}

async fn cursor_request(url: &str, token: &str) -> Result<Value, Failure> {
    let lost = || disconnected("Cursor usage request failed or timed out");
    let output: Output = curl(
        Some(token),
        &[
            "Content-Type: application/json",
            "Connect-Protocol-Version: 1",
        ],
        &[
            "--request",
            "POST",
            "--data",
            "{}",
            "--write-out",
            STATUS_TRAILER,
            url,
        ],
    )
    .await
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::InvalidInput => Failure::new("login", CURSOR_LOGIN_REQUIRED),
        _ => lost(),
    })?;
    let split = output
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or_else(lost)?;
    match &output.stdout[split + 1..] {
        b"200" => {}
        b"401" | b"403" => return Err(Failure::new("login", CURSOR_LOGIN_REQUIRED)),
        b"000" => return Err(lost()),
        status => {
            return Err(Failure::new(
                "protocol",
                &format!(
                    "Cursor usage endpoint returned HTTP {}",
                    String::from_utf8_lossy(status)
                ),
            ));
        }
    }
    serde_json::from_slice(&output.stdout[..split])
        .map_err(|_| Failure::new("protocol", "Cursor usage endpoint returned invalid JSON"))
}

/// # Errors
/// When the account can't be read; the kind tells the driver how to report it.
pub async fn cursor() -> Result<Value, Failure> {
    let token = cursor_token().await?;
    let usage_url = format!("{CURSOR_ENDPOINT}GetCurrentPeriodUsage");
    let plan_url = format!("{CURSOR_ENDPOINT}GetPlanInfo");
    let (usage, plan) = tokio::join!(
        cursor_request(&usage_url, &token),
        cursor_request(&plan_url, &token),
    );
    // A plan lookup failure must not discard valid account usage.
    Ok(json!({ "usage": usage?, "plan": plan.unwrap_or(Value::Null) }))
}

/// The port and token `kimi web` announces for its loopback management endpoint.
fn kimi_endpoint(line: &str) -> Option<(u16, &str)> {
    let rest = &line[line.find("http://127.0.0.1:")? + "http://127.0.0.1:".len()..];
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    let port = rest[..digits].parse().ok()?;
    let token = rest[digits..]
        .split("#token=")
        .nth(1)?
        .split_whitespace()
        .next()
        .filter(|token| !token.is_empty())?;
    Some((port, token))
}

async fn kimi_query(stdout: impl tokio::io::AsyncRead + Unpin) -> Result<Value, Failure> {
    let mut reader = BufReader::new(stdout.take(65_536));
    let endpoint = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let mut line = String::new();
            let read = reader
                .read_line(&mut line)
                .await
                .map_err(|_| disconnected("Kimi usage server could not start"))?;
            if read == 0 {
                return Err(Failure::new(
                    "protocol",
                    "Kimi usage endpoint missing from bounded banner",
                ));
            }
            if let Some((port, token)) = kimi_endpoint(&line) {
                return Ok((port, token.to_owned()));
            }
        }
    })
    .await
    .map_err(|_| disconnected("Kimi usage server startup timed out"))??;
    let url = format!("http://127.0.0.1:{}/api/v1/oauth/usage", endpoint.0);
    let output = curl(Some(&endpoint.1), &[], &["--fail", "--noproxy", "*", &url])
        .await
        .map_err(|_| disconnected("Kimi usage request failed or timed out"))?;
    if !output.status.success() {
        return Err(disconnected("Kimi usage request failed or timed out"));
    }
    if output.stdout.len() > BODY_LIMIT {
        return Err(Failure::new(
            "protocol",
            "Kimi usage response exceeded size limit",
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| Failure::new("protocol", "Kimi usage endpoint returned invalid JSON"))
}

/// Uses the CLI's loopback management endpoint, which only answers on its own machine.
/// No model turn is sent.
///
/// # Errors
/// When the helper doesn't start or its endpoint doesn't answer.
pub async fn kimi(executable: &str) -> Result<Value, Failure> {
    let mut command = TargetCommand::new(local_target(), executable);
    command
        .args(["web", "--port", "0", "--no-open"])
        .stdio(Pipe::Null, Pipe::Piped, Pipe::Null);
    for (name, value) in std::env::vars_os() {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|_| disconnected("Kimi usage server could not start"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| disconnected("Kimi usage server could not start"))?;
    let result = kimi_query(stdout).await;
    // It has no stdin-driven shutdown, and it's only here for this one question.
    let _ = child.terminate().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimis_banner_gives_its_port_and_token() {
        assert_eq!(
            kimi_endpoint("Serving at http://127.0.0.1:43210/#token=abc-1\n"),
            Some((43210, "abc-1"))
        );
        assert_eq!(kimi_endpoint("Listening on 0.0.0.0\n"), None);
        assert_eq!(kimi_endpoint("http://127.0.0.1:43210/\n"), None);
    }

    #[test]
    fn the_usage_trailer_comes_apart_from_the_body() {
        assert_eq!(
            split_trailer(b"{}\n30\n429"),
            Some((b"{}".as_slice(), b"30".as_slice(), b"429".as_slice()))
        );
        assert_eq!(split_trailer(b"nothing"), None);
    }

    #[test]
    fn cursor_login_follows_its_credential_store() {
        let home = Some(PathBuf::from("/home/user"));
        assert_eq!(
            cursor_credential_source("linux", None, None, home.clone(), None).unwrap(),
            CredentialSource::File("/home/user/.config/cursor/auth.json".into())
        );
        assert_eq!(
            cursor_credential_source("macos", None, None, home.clone(), None).unwrap(),
            CredentialSource::Keychain
        );
        assert_eq!(
            cursor_credential_source("linux", Some("key"), None, home, None)
                .unwrap_err()
                .kind,
            "capability"
        );
    }
}
