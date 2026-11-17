//! Read-only bridge to Claude Desktop's scoped OAuth cache. Desktop owns renewal:
//! we never copy refresh tokens, mint a grant, or modify either product's login.
mod storage;
#[cfg(all(test, unix))]
mod tests;

use agent_runtime::{ProcessInput, TargetCommand, TargetExecutable};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{LazyLock, Mutex},
};
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

const CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const HOST: &str = "https://api.anthropic.com";
const CACHE: &str = "oauth:tokenCacheV2";
type Rejected = (PathBuf, String, Zeroizing<String>);
static REJECTED: LazyLock<Mutex<Vec<Rejected>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub(super) fn reject(lease: &Lease) {
    let mut rejected = REJECTED.lock().expect("desktop credential rejections");
    rejected.retain(|(file, key, _)| file != &lease.file || key != &lease.key);
    if rejected.len() >= 32 {
        rejected.remove(0);
    }
    rejected.push((lease.file.clone(), lease.key.clone(), lease.token.clone()));
}

fn rejected(lease: &Lease) -> bool {
    REJECTED
        .lock()
        .expect("desktop credential rejections")
        .iter()
        .any(|(file, key, token)| file == &lease.file && key == &lease.key && token == &lease.token)
}

pub(super) fn reject_failure(lease: Option<&Lease>, reason: &str) {
    if matches!(
        reason,
        "claude_quota_login_expired" | "claude_quota_login_required" | "claude_quota_access_denied"
    ) && let Some(lease) = lease
    {
        reject(lease);
    }
}

/// A process stays with its original desktop account and organization, even if
/// the desktop user switches accounts while a task is running. Never Debug.
pub(super) struct Lease {
    file: PathBuf,
    key: String,
    account: String,
    organization: String,
    token: Zeroizing<String>,
    scope: String,
    subscription: String,
    tier: String,
}

fn select(config: &Value, entries: &Value, now: i64, file: PathBuf) -> Option<Lease> {
    let account = config["lastKnownAccountUuid"].as_str()?;
    if account.is_empty() {
        return None;
    }
    let prefix = format!("acct:{account}|{CLIENT}:");
    let mut eligible: Vec<_> = entries
        .as_object()?
        .iter()
        .filter_map(|(key, entry)| {
            let rest = key.strip_prefix(&prefix)?;
            let (org, scope) = rest.split_once(&format!(":{HOST}:"))?;
            let scopes: Vec<_> = scope.split_whitespace().collect();
            // Chat/profile-only grants must never be used as Code credentials.
            if org.is_empty()
                || !scopes.contains(&"user:inference")
                || !scopes.contains(&"user:profile")
                || !scopes.contains(&"user:file_upload")
                || entry["expiresAt"].as_i64()? <= now + 30_000
            {
                return None;
            }
            let token = entry["token"].as_str()?;
            if token.is_empty() || token.len() > 16_384 || token.chars().any(char::is_control) {
                return None;
            }
            Some((key, entry, org, scope, token))
        })
        .collect();
    // Match Desktop: prefer the local everyday Code grant over its Remote
    // Control fallback, which also carries the sessions scope.
    eligible.sort_by_key(|(key, entry, _, scope, _)| {
        (
            scope
                .split_whitespace()
                .any(|s| s == "user:sessions:claude_code"),
            std::cmp::Reverse(entry["expiresAt"].as_i64()),
            *key,
        )
    });
    let &(key, entry, org, scope, token) = eligible.first()?;
    // The cache alone cannot tell which organization is active when more than
    // one Code grant exists. Fall back to CLI rather than pick another account.
    if eligible
        .iter()
        .any(|(_, _, other_org, _, _)| *other_org != org)
    {
        return None;
    }
    Some(Lease {
        file,
        key: key.clone(),
        account: account.into(),
        organization: org.into(),
        token: Zeroizing::new(token.into()),
        scope: scope.into(),
        subscription: entry["subscriptionType"].as_str().unwrap_or("").into(),
        tier: entry["rateLimitTier"].as_str().unwrap_or("").into(),
    })
}

// The host-owned provider guard and refresh schema were checked in 2.1.325.
// Unknown/older runtimes retain CLI authentication rather than risk forwarding
// a desktop bearer to a settings.env custom endpoint.
fn verified_host_auth(version: Option<&str>) -> bool {
    let Some(version) = version.and_then(|v| semver::Version::parse(v).ok()) else {
        return false;
    };
    version >= semver::Version::new(2, 1, 325)
}

pub(super) async fn apply(
    executable: &TargetExecutable,
    native_version: Option<&str>,
    command: &mut TargetCommand,
) -> Option<Lease> {
    if !executable.target.is_local() || !verified_host_auth(native_version) {
        return None;
    }
    let mut selected: Option<Lease> = None;
    for file in storage::config_paths(&executable.target) {
        if let Some(lease) = load(file).await {
            if rejected(&lease) {
                return None;
            }
            if let Some(old) = &selected {
                if old.account != lease.account || old.key != lease.key {
                    return None;
                }
            } else {
                selected = Some(lease);
            }
        }
    }
    if let Some(lease) = &selected {
        lease.configure(command);
    }
    selected
}

async fn load(file: PathBuf) -> Option<Lease> {
    let config = storage::read_json(&file).await?;
    let encrypted = config[CACHE].as_str()?;
    let bytes = storage::decrypt(&file, encrypted).await?;
    let entries = serde_json::from_slice(&bytes).ok()?;
    select(
        &config,
        &entries,
        chrono::Utc::now().timestamp_millis(),
        file,
    )
}

impl Lease {
    fn configure(&self, command: &mut TargetCommand) {
        // A desktop credential always goes to the official issuer. Shell API
        // keys and settings.env custom providers must not redirect this bearer.
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_CUSTOM_HEADERS",
        ] {
            command.env(name, "");
        }
        for name in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_MANTLE",
            "CLAUDE_CODE_USE_GATEWAY",
            "CLAUDE_CODE_USE_ANTHROPIC_AWS",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
        ] {
            command.env(name, "0");
        }
        command
            .env("ANTHROPIC_BASE_URL", HOST)
            .env("CLAUDE_CODE_OAUTH_TOKEN", self.token.as_str())
            .env("CLAUDE_CODE_OAUTH_CLIENT_ID", CLIENT)
            .env("CLAUDE_CODE_ORGANIZATION_UUID", &self.organization)
            .env("CLAUDE_CODE_SESSIONS_OAUTH_TOKEN", "")
            .env("CLAUDE_CODE_OAUTH_SCOPES", &self.scope)
            .env("CLAUDE_CODE_SUBSCRIPTION_TYPE", &self.subscription)
            .env("CLAUDE_CODE_RATE_LIMIT_TIER", &self.tier)
            .env("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1")
            .env("CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH", "1");
    }

    /// Native host-refresh callback. Only a newer, still valid grant with the
    /// same scoped identity is returned. No user prompt is ever replayed here.
    async fn refreshed(&self) -> Option<Zeroizing<String>> {
        let next = load(self.file.clone()).await?;
        (next.account == self.account && next.key == self.key && next.token != self.token)
            .then_some(next.token)
    }
}

/// Used by business sessions and all control-only helpers, so a refresh callback
/// is answered before any generic presentation/diagnostic handling can see it.
pub(super) fn is_request(raw: &Value) -> bool {
    raw["type"] == "control_request" && raw["request"]["subtype"] == "oauth_token_refresh"
}

pub(super) async fn response(lease: Option<&mut Lease>, raw: &Value) -> Option<Zeroizing<String>> {
    if !is_request(raw) {
        return None;
    }
    let id = raw["request_id"].as_str()?;
    let token = if raw["request"]["kind"] == "sessions" {
        None // Do not grant remote/cloud-session access from a local Code lease.
    } else if let Some(lease) = lease {
        let token = lease.refreshed().await;
        if let Some(token) = &token {
            lease.token.clone_from(token);
        }
        token
    } else {
        None
    };
    let response = json!({"type":"control_response","response":{
        "subtype":"success","request_id":id,
        "response":{"accessToken":token.as_deref().map(String::as_str)}
    }});
    let mut line = Zeroizing::new(response.to_string());
    line.push('\n');
    Some(line)
}

pub(super) async fn respond(
    lease: Option<&mut Lease>,
    raw: &Value,
    stdin: &mut ProcessInput,
) -> std::io::Result<bool> {
    let Some(line) = response(lease, raw).await else {
        return Ok(false);
    };
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;
    Ok(true)
}
