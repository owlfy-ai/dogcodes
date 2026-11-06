//! Native stores are read, never modified; shell-backed key helpers are not executed.
use super::{Agent, Credential, Environment, Kind};
use crate::operation::Failure;
use agent_runtime::{Pipe, TargetCommand, local_target};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const GO: &str = "https://opencode.ai/inference/go/v1/usage";
const GO_USAGE: &str = "https://opencode.ai/zen/go/v1/usage";
const DEEPSEEK: &str = "https://api.deepseek.com/user/balance";
const DEEPSEEK_PLATFORM: &str = "https://platform.deepseek.com";
const EXPORT_LIMIT: usize = 4 * 1024 * 1024;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn unavailable(message: &str) -> Failure {
    Failure::new("capability", message)
}

fn home(env: &Environment) -> Result<PathBuf, Failure> {
    env.get("HOME")
        .or_else(|| env.get("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| unavailable("provider home unavailable"))
}
fn path(env: &Environment, key: &str, fallback: PathBuf) -> PathBuf {
    env.get(key)
        .filter(|s| !s.is_empty())
        .map_or(fallback, PathBuf::from)
}
fn text(file: &Path) -> Result<Option<String>, Failure> {
    let mut file = match std::fs::File::open(file) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable("provider credential file unreadable")),
    };
    let mut content = String::new();
    file.by_ref()
        .take(1024 * 1024 + 1)
        .read_to_string(&mut content)
        .map_err(|_| unavailable("provider credential file invalid"))?;
    if content.len() > 1024 * 1024 {
        return Err(unavailable("provider credential file too large"));
    }
    Ok(Some(content))
}
fn json(file: &Path) -> Result<Value, Failure> {
    text(file)?.map_or(Ok(Value::Null), |s| {
        serde_json::from_str(&s).map_err(|_| unavailable("provider credential JSON invalid"))
    })
}
fn key(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 16_384
        || value.starts_with(['!', '$'])
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.into())
}
fn stored_key(value: &Value) -> Option<String> {
    matches!(value["type"].as_str(), Some("key" | "api" | "api_key"))
        .then(|| key(value["key"].as_str()))
        .flatten()
}
fn credential(label: &str, endpoint: &str, key: String, kind: Kind) -> Credential {
    Credential {
        label: label.into(),
        endpoint: endpoint.into(),
        key,
        kind,
        org_id: None,
    }
}

pub(super) async fn discover(agent: &Agent, env: &Environment) -> Result<Vec<Credential>, Failure> {
    let found = match agent.provider.as_str() {
        "opencode" => vec![opencode(agent, env).await?],
        "pi" => pi(env)?,
        "deepseek-harness" => dsh(env)?,
        _ => Vec::new(),
    };
    let mut unique: Vec<Credential> = Vec::new();
    for c in found {
        if !unique
            .iter()
            .any(|old| old.endpoint == c.endpoint && old.key == c.key)
        {
            unique.push(c);
        }
        if unique.len() == 8 {
            break;
        }
    }
    if unique.is_empty() {
        return Err(unavailable("no supported provider balance credentials"));
    }
    Ok(unique)
}

async fn opencode(agent: &Agent, env: &Environment) -> Result<Credential, Failure> {
    if let Some(k) = key(env.get("OPENCODE_GO_API_KEY").map(String::as_str)) {
        return Ok(credential("OpenCode Go", GO, k, Kind::Go));
    }
    let Some(exported) = opencode_export(agent, env).await? else {
        return opencode_legacy(env);
    };
    // A dedicated Go API key still wins. A Go subscription lives on the signed-in
    // OpenCode account, which is OAuth. Stale files from before v2 are not consulted.
    if let Some(k) = selected(&exported, "opencode-go").and_then(stored_key) {
        return Ok(credential("OpenCode Go", GO, k, Kind::Go));
    }
    selected(&exported, "opencode")
        .and_then(go_account)
        .ok_or_else(|| unavailable("opencode_go_not_configured"))
}

/// The credentials `OpenCode` itself reports, through the command it offers for this. Its
/// private server is used so that reading quota never starts its background service.
/// `None` when this version predates the command.
async fn opencode_export(agent: &Agent, env: &Environment) -> Result<Option<Vec<Value>>, Failure> {
    let failed = || unavailable("OpenCode credentials could not be read");
    let mut command = TargetCommand::new(local_target(), &agent.executable);
    command
        .args(["auth", "export", "--standalone"])
        .stdio(Pipe::Null, Pipe::Piped, Pipe::Null);
    for (name, value) in env {
        command.env(name, value);
    }
    command.prefer_installed_node(Path::new(&agent.executable));
    let child = command.spawn().map_err(|_| failed())?;
    let output = tokio::time::timeout(EXPORT_TIMEOUT, child.wait_with_output(EXPORT_LIMIT))
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    if output.status.success()
        && let Ok(exported) = serde_json::from_slice(&output.stdout)
    {
        return Ok(Some(exported));
    }
    // From v2 on credentials live in its database, so a failure here must not bring an
    // old file's account back. Before that the command doesn't exist.
    let modern = agent
        .version
        .as_deref()
        .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 2);
    if modern { Err(failed()) } else { Ok(None) }
}

/// The active one, as with several accounts it needn't come first.
fn selected<'a>(exported: &'a [Value], integration: &str) -> Option<&'a Value> {
    exported
        .iter()
        .filter(|entry| entry["integrationID"] == integration)
        .max_by_key(|entry| (entry["active"] == true, entry["id"].as_str().unwrap_or("")))
        .map(|entry| &entry["value"])
}

fn opencode_legacy(env: &Environment) -> Result<Credential, Failure> {
    let root = path(env, "XDG_DATA_HOME", home(env)?.join(".local/share")).join("opencode");
    let auth = json(&root.join("auth.json"))?;
    if let Some(k) = stored_key(&auth["opencode-go"]).or_else(|| stored_key(&auth["opencode_go"])) {
        return Ok(credential("OpenCode Go", GO, k, Kind::Go));
    }
    if let Some(account) =
        go_account(&auth["opencode"]).or_else(|| go_account(&auth["opencode_go"]))
    {
        return Ok(account);
    }
    let k = key(env.get("OPENCODE_API_KEY").map(String::as_str))
        .ok_or_else(|| unavailable("opencode_go_not_configured"))?;
    Ok(credential("OpenCode Go", GO, k, Kind::Go))
}

/// Go subscriptions are billed to the signed-in `OpenCode` organization.
/// Only the official host is contacted, and the refresh token is never sent.
fn go_account(value: &Value) -> Option<Credential> {
    if value["type"] == "external" {
        return None;
    }
    let token = if value["type"] == "oauth" {
        key(value["access"].as_str())?
    } else {
        stored_key(value)?
    };
    let org_id = key(value["metadata"]["orgID"].as_str())?;
    let server = value["metadata"]["server"]
        .as_str()
        .unwrap_or("https://opencode.ai/console");
    let server = if let Some(rest) = server.strip_prefix("https://console.opencode.ai") {
        if rest.is_empty() || rest.starts_with('/') {
            format!("https://opencode.ai/console{rest}")
        } else {
            server.to_string()
        }
    } else {
        server.to_string()
    };
    let url = Url::parse(&server).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("opencode.ai")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(Credential {
        label: "OpenCode Go".into(),
        endpoint: GO_USAGE.into(),
        key: token,
        kind: Kind::Go,
        org_id: Some(org_id),
    })
}

fn provider_credential(label: &str, base: &str, k: String) -> Option<Credential> {
    if label.is_empty() || label.len() > 128 {
        return None;
    }
    let mut url = Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    if url.scheme() == "https" && url.host_str() == Some("api.deepseek.com") && url.port().is_none()
    {
        return Some(credential("DeepSeek API", DEEPSEEK, k, Kind::DeepSeek));
    }
    // Custom keys go only to the exact configured provider origin, with redirects disabled.
    url.set_path("/v1/usage");
    url.set_query(None);
    url.set_fragment(None);
    Some(credential(label, url.as_str(), k, Kind::Sub2Api))
}

fn pi(env: &Environment) -> Result<Vec<Credential>, Failure> {
    let root = path(env, "PI_CODING_AGENT_DIR", home(env)?.join(".pi/agent"));
    let auth = json(&root.join("auth.json"))?;
    let models = json(&root.join("models.json"))?;
    let mut found = Vec::new();
    let providers = models["providers"].as_object();
    let deepseek_provider = providers.and_then(|p| p.get("deepseek"));
    let deepseek = stored_key(&auth["deepseek"])
        .or_else(|| deepseek_provider.and_then(|p| key(p["apiKey"].as_str())))
        .or_else(|| key(env.get("DEEPSEEK_API_KEY").map(String::as_str)));
    if let Some(k) = deepseek {
        let base = providers
            .and_then(|p| p.get("deepseek"))
            .and_then(|p| p["baseUrl"].as_str())
            .unwrap_or("https://api.deepseek.com");
        if let Some(c) = provider_credential("DeepSeek API", base, k) {
            found.push(c);
        }
    }
    if let Some(providers) = providers {
        let mut entries: Vec<_> = providers.iter().collect();
        entries.sort_by_key(|(id, _)| *id);
        for (id, provider) in entries.into_iter().take(64) {
            if id == "deepseek" {
                continue;
            }
            let Some(base) = provider["baseUrl"].as_str() else {
                continue;
            };
            let Some(k) = stored_key(&auth[id]).or_else(|| key(provider["apiKey"].as_str())) else {
                continue;
            };
            if let Some(c) = provider_credential(id, base, k) {
                found.push(c);
            }
        }
    }
    Ok(found)
}

fn dsh(env: &Environment) -> Result<Vec<Credential>, Failure> {
    let root = path(env, "DSH_HOME", home(env)?.join(".dsh"));
    let stored = text(&root.join(".credentials.yaml"))?.map_or(Ok(Value::Null), |s| {
        serde_yaml_ng::from_str::<Value>(&s)
            .map_err(|_| unavailable("DeepSeek credential YAML invalid"))
    })?;
    // Native managed references override inherited environment. Opaque browser/OAuth
    // grants belong to their owner and must never be treated as platform API keys.
    let k = key(stored["refs"]["DEEPSEEK_API_KEY"].as_str())
        .or_else(|| key(stored["DEEPSEEK_API_KEY"].as_str()))
        .or_else(|| key(env.get("DEEPSEEK_API_KEY").map(String::as_str)))
        .or_else(|| {
            let record = &stored["records"]["llm-pi-ai/deepseek"];
            (record["kind"] == "api-key")
                .then(|| key(record["key"].as_str()))
                .flatten()
        });
    let Some(k) = k else {
        return Ok(
            platform_grant(&stored["records"]["deepseek-account-platform/default"])
                .map(|k| {
                    let endpoint = format!("{DEEPSEEK_PLATFORM}/api/v0/users/get_user_summary");
                    credential("DeepSeek", &endpoint, k, Kind::DeepSeekPlatform)
                })
                .into_iter()
                .collect(),
        );
    };
    let base = env
        .get("DEEPSEEK_BASE_URL")
        .map_or("https://api.deepseek.com", String::as_str);
    Ok(provider_credential("DeepSeek API", base, k)
        .into_iter()
        .collect())
}

/// The account-platform login grant is only ever sent back to its official issuer,
/// the way dsh reads its own wallet; it is never used as an inference API key.
fn platform_grant(record: &Value) -> Option<String> {
    let payload = &record["payload"];
    if record["kind"] != "grant"
        || payload["version"] != 1
        || payload["issuer"].as_str()?.trim_end_matches('/') != DEEPSEEK_PLATFORM
    {
        return None;
    }
    key(payload["token"].as_str()).filter(|t| t.bytes().all(|b| b.is_ascii_graphic()))
}
