//! One management request to `dogcodes-shim manage`, run under the login environment of
//! the server's account. A line of JSON goes in and a line of JSON comes back.
use crate::{account, detect, install, provider_account, updates};
use agent_protocol::InstallationSummary;
use agent_runtime::{Pipe, SharedTarget, TargetCommand};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const REPLY_LIMIT: usize = 4 * 1024 * 1024;
const UPGRADE_LIMIT: Duration = Duration::from_mins(15);
const ANSWER_LIMIT: Duration = Duration::from_mins(2);

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Detect,
    CheckUpdate {
        installation: Box<InstallationSummary>,
    },
    Upgrade {
        installation: Box<InstallationSummary>,
    },
    Install {
        provider: String,
    },
    ClaudeUsage {
        version: Option<String>,
    },
    CursorUsage,
    KimiUsage {
        executable: String,
    },
    ProviderAccount {
        agent: provider_account::Agent,
        /// The account last read, by its fingerprint. Not read again while it is current.
        known: Option<String>,
    },
}

/// `kind` says how the asker should treat it, as errors don't survive the trip as types.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Failure {
    pub kind: String,
    pub message: String,
}

impl Failure {
    #[must_use]
    pub fn new(kind: &str, message: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.to_owned(),
        }
    }
    fn other(message: impl Into<String>) -> Self {
        Self {
            kind: "other".into(),
            message: message.into(),
        }
    }
}

/// Where an upgrade ended up. The asker rescans, then checks `goal` took effect.
#[derive(Debug, Deserialize, Serialize)]
pub struct Upgraded {
    pub update: updates::AgentUpdate,
    /// Nothing was installed when this is empty: it was already up to date.
    pub goal: Option<String>,
}

async fn upgrade(mut i: InstallationSummary) -> Result<Upgraded, Failure> {
    let npm = crate::npm_runtime_on_path;
    let plan = updates::plan(&i);
    if matches!(plan, updates::Plan::Host(_) | updates::Plan::Unsupported) {
        return Err(Failure::other("请在宿主应用或原安装渠道中升级"));
    }
    let launcher = if let updates::Plan::StandaloneCli { prefix, .. } = &plan {
        updates::managed_cli::launcher(prefix, &i.provider_id)
    } else {
        updates::stable_launcher(&i)
    };
    // Read the actual binary: a native auto-updater may have run since it was detected.
    let actual = updates::command(
        updates::new_agents::version_spec(&i, &plan, npm)
            .await
            .map_err(Failure::other)?,
    )
    .await
    .map_err(Failure::other)?;
    i.version = Some(if i.provider_id == "cursor" {
        actual.trim().to_owned()
    } else {
        updates::version(&actual)
            .ok_or_else(|| Failure::other("升级前无法验证当前版本"))?
            .to_string()
    });
    let update = updates::inspect(&i, &plan).await.map_err(Failure::other)?;
    if !update.can_upgrade {
        return Ok(Upgraded { update, goal: None });
    }
    let goal = update
        .latest_version
        .clone()
        .ok_or_else(|| Failure::other("缺少目标版本"))?;
    let install = updates::new_agents::install_spec(&i, &plan, &goal, npm)
        .await
        .map_err(Failure::other)?;
    let native = matches!(plan, updates::Plan::Native(_));
    if i.provider_id == "kimi" && native {
        let help = updates::command(updates::spec(
            std::path::Path::new(&i.executable),
            vec!["__update_download".into(), "--help".into()],
            false,
        ))
        .await
        .map_err(Failure::other)?;
        if !help.contains("--manual") {
            return Err(Failure::other(
                "此 Kimi 版本不支持非交互更新，请在终端执行 kimi upgrade",
            ));
        }
    }
    updates::command(install).await.map_err(Failure::other)?;
    updates::managed_cli::activate(&i, &plan, &goal, npm)
        .await
        .map_err(Failure::other)?;
    if native && matches!(i.provider_id.as_str(), "kiro" | "kimi") {
        updates::wait_for_updated_version(&launcher, &goal, Duration::from_mins(1))
            .await
            .map_err(Failure::other)?;
    }
    Ok(Upgraded {
        update,
        goal: Some(goal),
    })
}

fn as_value<T: Serialize>(value: T) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|error| Failure::other(error.to_string()))
}

async fn answer(request: Request) -> Result<Value, Failure> {
    match request {
        Request::Detect => as_value(detect::detect().await),
        Request::CheckUpdate { installation } => {
            let plan = updates::plan(&installation);
            let checked = updates::inspect(&installation, &plan).await;
            as_value(checked.map_err(Failure::other)?)
        }
        Request::Upgrade { installation } => as_value(upgrade(*installation).await?),
        Request::Install { provider } => as_value(install::install(&provider).await?),
        Request::ClaudeUsage { version } => as_value(account::claude(version.as_deref()).await),
        Request::CursorUsage => account::cursor().await,
        Request::KimiUsage { executable } => account::kimi(&executable).await,
        Request::ProviderAccount { agent, known } => {
            as_value(provider_account::read_through_curl(&agent, known.as_deref()).await?)
        }
    }
}

/// The shim's side: reads one request from stdin and writes its answer to stdout.
pub async fn serve() {
    let mut line = String::new();
    let read = BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await;
    let reply = match read {
        Ok(_) => match serde_json::from_str::<Request>(&line) {
            Ok(request) => answer(request).await,
            Err(error) => Err(Failure::other(error.to_string())),
        },
        Err(error) => Err(Failure::other(error.to_string())),
    };
    let mut text = serde_json::to_string(&reply).unwrap_or_else(|_| "null".into());
    text.push('\n');
    let mut stdout = tokio::io::stdout();
    let _ = stdout.write_all(text.as_bytes()).await;
    let _ = stdout.flush().await;
}

/// The asking side, from the desktop over the connection.
///
/// # Errors
/// When the machine can't be asked, or says it couldn't do it.
pub async fn call<T: DeserializeOwned>(
    target: &SharedTarget,
    request: &Request,
) -> Result<T, Failure> {
    let lost = |message: &str| Failure::new("disconnected", message);
    let shim = target
        .shim_program()
        .ok_or_else(|| Failure::new("capability", "only a connected server has a shim"))?;
    let limit = if matches!(request, Request::Upgrade { .. } | Request::Install { .. }) {
        UPGRADE_LIMIT
    } else {
        ANSWER_LIMIT
    };
    let mut command = TargetCommand::new(target.clone(), shim);
    command
        .arg("manage")
        .stdio(Pipe::Piped, Pipe::Piped, Pipe::Null);
    // The whole login environment: it's what finds and runs things the way a shell would.
    for (name, value) in target.login_environment() {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|_| lost("could not reach the server"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| lost("could not reach the server"))?;
    let mut text =
        serde_json::to_string(request).map_err(|error| Failure::other(error.to_string()))?;
    text.push('\n');
    stdin
        .write_all(text.as_bytes())
        .await
        .map_err(|_| lost("the connection closed"))?;
    stdin
        .shutdown()
        .await
        .map_err(|_| lost("the connection closed"))?;
    drop(stdin);
    // Abandoning this ends the request on the server too.
    let output = tokio::time::timeout(limit, child.wait_with_output(REPLY_LIMIT))
        .await
        .map_err(|_| lost("the server took too long to answer"))?
        .map_err(|_| lost("the connection closed"))?;
    let reply: Result<T, Failure> = serde_json::from_slice(&output.stdout)
        .map_err(|_| lost("the server gave no usable answer"))?;
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_survive_the_trip() {
        let text = serde_json::to_string(&Request::KimiUsage {
            executable: "/x/kimi".into(),
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_str::<Request>(&text).unwrap(),
            Request::KimiUsage { executable } if executable == "/x/kimi"
        ));
    }
}
