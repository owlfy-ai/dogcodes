//! Native stores are read, never modified; shell-backed key helpers are not executed.
use super::{DriverError, Kind, Request, unavailable};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
};

const GO: &str = "https://opencode.ai/inference/go/v1/usage";
const DEEPSEEK: &str = "https://api.deepseek.com/user/balance";
type Environment = HashMap<String, String>;

fn home(env: &Environment) -> Result<PathBuf, DriverError> {
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
fn text(file: &Path) -> Result<Option<String>, DriverError> {
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
fn json(file: &Path) -> Result<Value, DriverError> {
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
fn request(label: &str, endpoint: &str, key: String, kind: Kind) -> Request {
    Request {
        label: label.into(),
        endpoint: endpoint.into(),
        key,
        kind,
    }
}

pub(super) fn discover(provider: &str, env: &Environment) -> Result<Vec<Request>, DriverError> {
    let requests = match provider {
        "opencode" => vec![opencode(env)?],
        "pi" => pi(env)?,
        "deepseek-harness" => dsh(env)?,
        _ => Vec::new(),
    };
    let mut unique: Vec<Request> = Vec::new();
    for r in requests {
        if !unique
            .iter()
            .any(|old| old.endpoint == r.endpoint && old.key == r.key)
        {
            unique.push(r);
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

fn opencode(env: &Environment) -> Result<Request, DriverError> {
    if let Some(k) = key(env.get("OPENCODE_GO_API_KEY").map(String::as_str)) {
        return Ok(request("OpenCode Go", GO, k, Kind::Go));
    }
    let root = path(env, "XDG_DATA_HOME", home(env)?.join(".local/share")).join("opencode");
    let database = root.join(env.get("OPENCODE_DB").map_or("opencode.db", String::as_str));
    // v2 migrates auth.json to the credential table. Select exactly the native active
    // Go credential, including the pre-v2.1 type=api shape; never borrow Zen/OAuth.
    let (modern, native) = opencode_database_key(&database)?;
    let native = if modern {
        native
    } else {
        let auth = json(&root.join("auth.json"))?;
        stored_key(&auth["opencode-go"]).or_else(|| stored_key(&auth["opencode_go"]))
    };
    let k = native
        .or_else(|| key(env.get("OPENCODE_API_KEY").map(String::as_str)))
        .ok_or_else(|| unavailable("opencode_go_not_configured"))?;
    Ok(request("OpenCode Go", GO, k, Kind::Go))
}

fn opencode_database_key(file: &Path) -> Result<(bool, Option<String>), DriverError> {
    if !file.exists() {
        return Ok((false, None));
    }
    let db = rusqlite::Connection::open_with_flags(
        file,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| unavailable("OpenCode credential database unreadable"))?;
    db.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|_| unavailable("OpenCode credential database unavailable"))?;
    let modern: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='credential')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| unavailable("OpenCode credential schema unavailable"))?;
    if !modern {
        return Ok((false, None));
    }
    let value: Option<String> = db.query_row(
        "SELECT value FROM credential WHERE integration_id='opencode-go' ORDER BY active DESC, time_created DESC, id DESC LIMIT 1",
        [], |r| r.get(0)).optional().map_err(|_| unavailable("OpenCode credential selection unavailable"))?;
    let value = value
        .map(|s| {
            if s.len() > 1024 * 1024 {
                return Err(unavailable("OpenCode credential too large"));
            }
            serde_json::from_str::<Value>(&s)
                .map_err(|_| unavailable("OpenCode credential invalid"))
        })
        .transpose()?;
    Ok((true, value.as_ref().and_then(stored_key)))
}

fn provider_request(label: &str, base: &str, k: String) -> Option<Request> {
    if label.is_empty() || label.len() > 128 {
        return None;
    }
    let mut url = reqwest::Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    if url.scheme() == "https" && url.host_str() == Some("api.deepseek.com") && url.port().is_none()
    {
        return Some(request("DeepSeek API", DEEPSEEK, k, Kind::DeepSeek));
    }
    // Custom keys go only to the exact configured provider origin, with redirects disabled.
    url.set_path("/v1/usage");
    url.set_query(None);
    url.set_fragment(None);
    Some(request(label, url.as_str(), k, Kind::Sub2Api))
}

fn pi(env: &Environment) -> Result<Vec<Request>, DriverError> {
    let root = path(env, "PI_CODING_AGENT_DIR", home(env)?.join(".pi/agent"));
    let auth = json(&root.join("auth.json"))?;
    let models = json(&root.join("models.json"))?;
    let mut requests = Vec::new();
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
        if let Some(r) = provider_request("DeepSeek API", base, k) {
            requests.push(r);
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
            if let Some(r) = provider_request(id, base, k) {
                requests.push(r);
            }
        }
    }
    Ok(requests)
}

fn dsh(env: &Environment) -> Result<Vec<Request>, DriverError> {
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
        return Ok(Vec::new());
    };
    let base = env
        .get("DEEPSEEK_BASE_URL")
        .map_or("https://api.deepseek.com", String::as_str);
    Ok(provider_request("DeepSeek API", base, k)
        .into_iter()
        .collect())
}
