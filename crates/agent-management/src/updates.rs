//! Installation-scoped updates, judged on the machine an agent is installed on: this
//! machine's own agents from the desktop, a server's from the shim running there. Only
//! trusted adapters construct commands; callers supply an installation.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::wildcard_imports
)]
pub mod codex_host;
pub mod electron_host;
pub mod kiro;
pub mod managed_cli;
pub mod new_agents;
pub mod windows;
pub mod zcode_host;
use agent_domain::user_text;
use agent_protocol::InstallationSummary;
use agent_runtime::{ProcessError, ProcessSpec, run_bounded};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub token: String,
    pub task_count: usize,
    pub process_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdate {
    pub installation_id: String,
    pub status: String,
    pub current_version: Option<String>,
    /// Read locally, even when the remote update check fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_version: Option<String>,
    pub latest_version: Option<String>,
    pub source: String,
    pub message: Option<String>,
    pub can_upgrade: bool,
    pub checked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_use: Option<Confirmation>,
}

#[derive(Clone, Debug)]
pub enum Plan {
    Native(&'static str),
    Npm {
        node: PathBuf,
        npm: PathBuf,
        prefix: PathBuf,
        package: &'static str,
    },
    NpmLocal {
        prefix: PathBuf,
        package: &'static str,
    },
    StandaloneCli {
        prefix: PathBuf,
        package: &'static str,
    },
    Brew {
        executable: PathBuf,
        package: &'static str,
        cask: bool,
    },
    Host(PathBuf),
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpgradeResource {
    Npm(PathBuf),
    Brew(PathBuf),
    // Native updaters can share provider-wide state even across launcher aliases.
    Native(String),
}

pub fn upgrade_resource(i: &InstallationSummary, plan: &Plan) -> Result<UpgradeResource, String> {
    let canonical = |path: &Path| {
        path.canonicalize()
            .map_err(|_| "backendText.updatePathChanged".to_string())
    };
    match plan {
        Plan::Npm { prefix, .. } | Plan::NpmLocal { prefix, .. } => {
            Ok(UpgradeResource::Npm(canonical(prefix)?))
        }
        Plan::Brew { executable, .. } => Ok(UpgradeResource::Brew(canonical(executable)?)),
        Plan::Native(_) | Plan::StandaloneCli { .. } => {
            Ok(UpgradeResource::Native(i.provider_id.clone()))
        }
        Plan::Host(_) | Plan::Unsupported => Err("backendText.updateInHost".into()),
    }
}

pub fn package(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("@openai/codex"),
        "claude-code" => Some("@anthropic-ai/claude-code"),
        "codebuddy" => Some("@tencent-ai/codebuddy-code"),
        "kimi" => Some("@moonshot-ai/kimi-code"),
        "pi" => Some("@earendil-works/pi-coding-agent"),
        "opencode" => Some("opencode-ai"),
        "deepseek-harness" => Some("@deepseek-ai/dsh"),
        _ => None,
    }
}
pub fn host_app(provider: &str, executable: &Path) -> Option<PathBuf> {
    if provider == "opencode"
        && let Some(app) = electron_host::opencode_app(executable)
    {
        return Some(app);
    }
    if provider == "zcode"
        && executable.file_name().is_some_and(|n| n == "ZCode.exe")
        && let Some(root) = executable.parent()
        && root.join("resources/glm/zcode.cjs").is_file()
    {
        return Some(root.into());
    }
    if provider == "codex"
        && let Some(app) = codex_host::desktop_app(executable)
    {
        return Some(app.into());
    }
    executable
        .ancestors()
        .find(|p| p.extension().is_some_and(|v| v == "app"))
        .map(Path::to_path_buf)
}
pub fn plan(i: &InstallationSummary) -> Plan {
    let path = Path::new(&i.executable);
    let Ok(real) = path.canonicalize() else {
        return Plan::Unsupported;
    };
    if let Some(prefix) = managed_cli::existing_prefix(i) {
        return Plan::StandaloneCli {
            prefix,
            package: package(&i.provider_id).unwrap(),
        };
    }
    if managed_cli::desktop_cli(&i.provider_id, &real)
        && let Some(prefix) = managed_cli::prefix(
            &agent_registry::DiscoveryContext::from_current_process(),
            &i.provider_id,
        )
    {
        return Plan::StandaloneCli {
            prefix,
            package: package(&i.provider_id).unwrap(),
        };
    }
    // Kiro CLI's own updater replaces its signed CLI app bundle. This is not
    // a bundled CLI that must defer to a separate IDE updater.
    if i.provider_id == "kiro" && kiro::bundle(&real).is_some() {
        return Plan::Native("Kiro CLI 原生更新器");
    }
    // Bundled packages belong to their host, even when they contain node_modules.
    if let Some(app) = host_app(&i.provider_id, &real) {
        return Plan::Host(app);
    }
    if cfg!(windows)
        && let Some(plan) = windows::npm_plan(&i.provider_id, &real)
    {
        return plan;
    }
    if i.provider_id == "kiro" && kiro::standalone(&real) {
        return Plan::Native("Kiro 原生更新器");
    }
    if i.provider_id == "pi" && new_agents::pi_managed_root(&real).is_some() {
        return Plan::Native("Pi 原生更新器");
    }
    // Homebrew owns its bundled npm packages; do not mutate their private prefix.
    let real_text = real.to_string_lossy();
    for (marker, cask) in [("/Cellar/", false), ("/Caskroom/", true)] {
        if let Some((prefix, tail)) = real_text.split_once(marker) {
            let pkg = match i.provider_id.as_str() {
                "codex" => "codex",
                "claude-code" => "claude-code",
                "kimi" => "kimi-code",
                "codebuddy" => "codebuddy-code",
                "opencode" => "opencode",
                _ => return Plan::Unsupported,
            };
            let brew = PathBuf::from(prefix).join("bin/brew");
            if tail.split('/').next() == Some(pkg) && brew.is_file() {
                return Plan::Brew {
                    executable: brew,
                    package: pkg,
                    cask,
                };
            }
        }
    }
    // Identify the package root and exact npm prefix. Never update a different Node installation.
    if let Some(pkg) = package(&i.provider_id) {
        for parent in real.ancestors().skip(1).take(9) {
            let info = std::fs::read(parent.join("package.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            if info.as_ref().and_then(|v| v["name"].as_str()) != Some(pkg) {
                continue;
            }
            let modules = parent
                .ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == "node_modules"));
            if let Some(modules) = modules {
                let container = modules.parent().unwrap_or(modules);
                if i.provider_id == "deepseek-harness"
                    && new_agents::is_dsh_cache(container, parent)
                {
                    return Plan::NpmLocal {
                        prefix: container.into(),
                        package: pkg,
                    };
                }
                let prefix = if container.file_name().is_some_and(|n| n == "lib") {
                    container.parent().unwrap_or(container)
                } else {
                    container
                };
                let bin = if cfg!(windows) {
                    prefix.to_path_buf()
                } else {
                    prefix.join("bin")
                };
                let node = bin.join(if cfg!(windows) { "node.exe" } else { "node" });
                let npm = modules.join("npm/bin/npm-cli.js");
                if node.is_file() && npm.is_file() {
                    return Plan::Npm {
                        node,
                        npm,
                        prefix: prefix.into(),
                        package: pkg,
                    };
                }
            }
            return Plan::Unsupported;
        }
    }
    match i.provider_id.as_str() {
        "cursor" => Plan::Native("Cursor CLI"),
        "grok" => Plan::Native("Grok 原生更新器"),
        "antigravity" => Plan::Native("Antigravity CLI"),
        "claude-code"
            if real
                .ancestors()
                .skip(1)
                .any(|p| p.ends_with("claude/versions"))
                || (cfg!(windows) && real.ends_with(".local/bin/claude.exe")) =>
        {
            Plan::Native("Claude 原生更新器")
        }
        "kimi"
            if real
                .ancestors()
                .skip(1)
                .any(|p| p.ends_with(".kimi-code/bin")) =>
        {
            Plan::Native("Kimi 原生更新器")
        }
        "opencode" if new_agents::is_opencode_native(&real) => Plan::Native("OpenCode 原生更新器"),
        _ => Plan::Unsupported,
    }
}
// Native installers may replace a stable launcher with a new version-directory target.
// Remember that exact launcher before mutation so rescan can find the new installation ID.
pub fn stable_launcher(i: &InstallationSummary) -> PathBuf {
    let original = PathBuf::from(&i.executable);
    if i.provider_id == "pi"
        && let Some(root) = original
            .canonicalize()
            .ok()
            .and_then(|p| new_agents::pi_managed_root(&p))
    {
        return new_agents::pi_launcher(&root);
    }
    let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    else {
        return original;
    };
    let relative = match i.provider_id.as_str() {
        "cursor" => ".local/bin/cursor-agent",
        "claude-code" => ".local/bin/claude",
        "kimi" => ".kimi-code/bin/kimi",
        "grok" => ".local/bin/grok",
        "antigravity" => ".local/bin/agy",
        "pi" => ".local/bin/pi",
        "opencode" => ".opencode/bin/opencode",
        "kiro" => ".local/bin/kiro-cli",
        _ => return original,
    };
    let mut launcher = home.join(relative);
    if cfg!(windows) && matches!(i.provider_id.as_str(), "claude-code" | "kimi") {
        launcher.set_extension("exe");
    }
    if launcher
        .canonicalize()
        .ok()
        .zip(original.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
    {
        launcher
    } else {
        original
    }
}
pub fn normalize_build(v: &str) -> String {
    let (head, suffix) = v
        .trim()
        .split_once('-')
        .map_or((v.trim(), ""), |(a, b)| (a, b));
    let normalized = head
        .split('.')
        .map(|p| {
            p.parse::<u64>()
                .map_or_else(|_| p.into(), |v| v.to_string())
        })
        .collect::<Vec<_>>()
        .join(".");
    if suffix.is_empty() {
        normalized
    } else {
        format!("{normalized}-{suffix}")
    }
}
pub fn verified_version(provider: &str, actual: &str, target: &str) -> bool {
    if provider == "cursor" {
        normalize_build(actual) == normalize_build(target)
    } else {
        version(actual)
            .zip(version(target))
            .is_some_and(|(a, b)| a >= b)
    }
}
pub fn key(i: &InstallationSummary) -> String {
    format!(
        "{}|{}|{:?}|{:?}",
        i.fingerprint,
        i.executable,
        i.version,
        desktop_version(i)
    )
}
pub fn desktop_version(i: &InstallationSummary) -> Option<String> {
    if !matches!(i.provider_id.as_str(), "codex" | "zcode" | "opencode") {
        return None;
    }
    let executable = Path::new(&i.executable).canonicalize().ok()?;
    let app = host_app(&i.provider_id, &executable)?;
    if matches!(i.provider_id.as_str(), "zcode" | "opencode")
        && app.extension().is_none_or(|e| e != "app")
    {
        return zcode_host::version(&app);
    }
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    info.as_dictionary()?
        .get("CFBundleShortVersionString")?
        .as_string()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
pub fn base(i: &InstallationSummary) -> AgentUpdate {
    AgentUpdate {
        installation_id: i.installation_id.to_string(),
        status: "checking".into(),
        current_version: i.version.clone(),
        desktop_version: desktop_version(i),
        latest_version: None,
        source: String::new(),
        message: None,
        can_upgrade: false,
        checked_at: chrono::Utc::now().timestamp_millis(),
        in_use: None,
    }
}
pub fn version(s: &str) -> Option<semver::Version> {
    s.split_whitespace().find_map(|p| {
        semver::Version::parse(
            p.trim_start_matches('v')
                .trim_matches(|c| c == '(' || c == ')'),
        )
        .ok()
    })
}
pub fn newer(current: &str, latest: &str) -> Result<bool, String> {
    let a = version(current).ok_or("backendText.updateLocalUnknownNoAuto")?;
    let b = version(latest).ok_or("backendText.updateSourceVersionUnknown")?;
    Ok(b > a)
}
pub fn spec(executable: &Path, args: Vec<String>, updating: bool) -> ProcessSpec {
    // canonicalize() is for installation identity. Node, cmd and native
    // installer scripts need ordinary drive/UNC paths at the process boundary.
    let mut p = ProcessSpec::probe(
        agent_runtime::external_path(executable),
        args.into_iter()
            .map(|arg| {
                agent_runtime::external_path(&arg)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect(),
    );
    p.timeout = Duration::from_secs(if updating { 600 } else { 35 });
    p.env_allowlist = [
        "PATH",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "SystemRoot",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "NODE_EXTRA_CA_CERTS",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    p.cwd = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(agent_runtime::external_path);
    p.env_overrides.insert("CI".into(), "1".into());
    p.env_overrides.insert("NO_COLOR".into(), "1".into());
    // Explicit updater subcommands still work. Passive version probes must never auto-install.
    p.env_overrides
        .insert("AGY_CLI_DISABLE_AUTO_UPDATE".into(), "true".into());
    p
}
pub async fn command(p: ProcessSpec) -> Result<String, String> {
    let out = run_bounded(&p).await.map_err(|error| match error {
        ProcessError::TimedOut { .. } => "backendText.updateCommandTimeout".to_string(),
        ProcessError::Spawn(_) => "backendText.updateCommandNoStart".to_string(),
        _ => "backendText.updateCommandInterrupted".to_string(),
    })?;
    if !out.success {
        return Err(command_failure(out.exit_code, &out.stdout, &out.stderr));
    }
    // Never forward arbitrary CLI output (may contain account details) to the webview.
    Ok(out.stdout)
}
pub fn command_failure(code: Option<i32>, stdout: &str, stderr: &str) -> String {
    // Match known diagnostics, but never return the raw output: it may contain secrets.
    let output = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if output.contains("another cli update is already in progress") {
        return "backendText.updateCursorLocked".into();
    }
    if [
        "connection dropped while downloading",
        "download stalled",
        "econnreset",
        "fetch failed",
    ]
    .iter()
    .any(|needle| output.contains(needle))
    {
        return "backendText.updateDownloadInterrupted".into();
    }
    format!(
        "更新命令未成功（退出码 {}），请检查网络和安装权限后重试",
        code.map_or("未知".into(), |v| v.to_string())
    )
}
pub use crate::http::get;
pub fn json(s: &str) -> Result<Value, String> {
    serde_json::from_str(s).map_err(|_| "backendText.updateSourceInvalid".into())
}
pub fn field(v: &Value, name: &str) -> Result<String, String> {
    v[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 128)
        .map(str::to_owned)
        .ok_or("backendText.updateSourceNoVersion".into())
}
pub fn claude_channel() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .and_then(|h| std::fs::read(PathBuf::from(h).join(".claude/settings.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["autoUpdatesChannel"].as_str().map(str::to_owned))
        .filter(|v| v == "stable")
        .unwrap_or("latest".into())
}

// Knowing the official release feed does not imply that we can safely mutate
// this installation (Windows npm shims, desktop payloads and custom prefixes).
// Keep that distinction even when a newer version is available.
pub async fn inspect_external(
    i: &InstallationSummary,
    fetch: impl AsyncFn(&str) -> Result<String, String>,
) -> Result<AgentUpdate, String> {
    let mut out = base(i);
    out.status = "unsupported".into();
    out.source = "外部安装".into();
    let (url, is_json) = match i.provider_id.as_str() {
        "claude-code" => (
            format!(
                "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/{}",
                claude_channel()
            ),
            false,
        ),
        "kimi" => ("https://code.kimi.com/kimi-code/latest".into(), false),
        provider => {
            let Some(package) = package(provider) else {
                return Ok(out);
            };
            out.source = "npm".into();
            (format!("https://registry.npmjs.org/{package}/latest"), true)
        }
    };
    let body = fetch(&url).await?;
    let latest = if is_json {
        field(&json(&body)?, "version")?
    } else {
        body.trim().to_string()
    };
    complete_check(out, latest, false)
}

pub fn complete_check(
    mut out: AgentUpdate,
    latest: String,
    can_upgrade: bool,
) -> Result<AgentUpdate, String> {
    let available = newer(
        out.current_version
            .as_deref()
            .ok_or("backendText.updateLocalUnknown")?,
        &latest,
    )?;
    out.can_upgrade = available && can_upgrade;
    out.status = if available { "available" } else { "up_to_date" }.into();
    out.latest_version = Some(latest);
    Ok(out)
}

pub async fn inspect(i: &InstallationSummary, p: &Plan) -> Result<AgentUpdate, String> {
    let mut out = base(i);
    let latest = match p {
        Plan::StandaloneCli { .. } => {
            let mut checked = inspect_external(i, get).await?;
            checked.can_upgrade = checked.status == "available";
            checked.source = "npm".into();
            return Ok(checked);
        }
        Plan::Host(app) => {
            out.status = "managed".into();
            out.source = "桌面应用".into();
            out.current_version = out.desktop_version.clone();
            out.message = Some("backendText.updateCliWithHost".into());
            if i.provider_id == "codex" {
                return codex_host::inspect(app, out).await;
            }
            if i.provider_id == "opencode" {
                // Desktop-owned 2.x runtimes must never use the npm 1.x feed.
                if out.current_version.is_some() && electron_host::official_opencode_feed(app) {
                    let channel = if app.extension().is_some_and(|e| e == "app") {
                        "latest-mac.yml"
                    } else {
                        "latest.yml"
                    };
                    let body = get(&format!(
                        "https://opencode.ai/update/api/latest/desktop/opencode/{channel}"
                    ))
                    .await?;
                    return electron_host::check_opencode(out, &body);
                }
                return Ok(out);
            }
            if i.provider_id == "zcode" {
                out.current_version = out.desktop_version.clone();
                if out.current_version.is_none() {
                    return Err("backendText.zcodeVersionUnreadable".into());
                }
                out.source = "ZCode 桌面应用".into();
                // The desktop's update API uses host versions, never compare against zcode-cli versions.
                let arch = if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else {
                    "x86_64"
                };
                let os = if app.join("ZCode.exe").is_file() {
                    "windows"
                } else {
                    "darwin"
                };
                let body=get(&format!("https://zcode.z.ai/api/v1/releases/electron/manifest?platform={os}-{arch}&channel=1")).await?;
                let latest = body
                    .lines()
                    .find_map(|l| l.strip_prefix("version:"))
                    .map(|s| s.trim().trim_matches(['\'', '"']).to_string())
                    .ok_or("backendText.zcodeSourceNoVersion")?;
                out.latest_version = Some(latest.clone());
                if newer(
                    out.current_version.as_deref().ok_or("backendText.appVersionUnknown")?,
                    &latest,
                )? {
                    out.message = Some(user_text("backendText.zcodeAppUpgrade", &[&latest]));
                } else {
                    out.message = Some("桌面应用已是最新版本".into());
                }
            }
            return Ok(out);
        }
        Plan::Unsupported if i.provider_id == "kiro" => {
            out.source = "Kiro 原生更新器".into();
            let output = command(spec(
                Path::new(&i.executable),
                vec!["update".into(), "--check".into()],
                false,
            ))
            .await?;
            kiro::latest_from_check(&output)?
        }
        Plan::Unsupported => {
            return inspect_external(i, get).await;
        }
        Plan::Npm { package, .. } | Plan::NpmLocal { package, .. } => {
            out.source = if matches!(p, Plan::NpmLocal { .. }) {
                "npm（当前 npx 安装）"
            } else {
                "npm"
            }
            .into();
            field(
                &json(&get(&format!("https://registry.npmjs.org/{package}/latest")).await?)?,
                "version",
            )?
        }
        Plan::Brew { package, cask, .. } => {
            out.source = "Homebrew".into();
            let data = json(
                &get(&format!(
                    "https://formulae.brew.sh/api/{}/{package}.json",
                    if *cask { "cask" } else { "formula" }
                ))
                .await?,
            )?;
            if *cask {
                field(&data, "version")?
            } else {
                field(&data["versions"], "stable")?
            }
        }
        Plan::Native(source) => {
            out.source = (*source).into();
            match i.provider_id.as_str() {
                "cursor" => {
                    let v=json(&command(spec(Path::new(&i.executable),vec!["about".into(),"--format".into(),"json".into()],false)).await?)?;
                    let latest=field(&v,"latestVersion")?;
                    let status=field(&v,"latestStatus")?;
                    // Cursor uses date/hash builds, not semver; trust its native comparator.
                    out.status=match status.as_str(){"up_to_date"=>"up_to_date","update_available"=>"available",_=>return Err("backendText.updateCursorUnconfirmed".into())}.into();
                    out.latest_version=Some(latest);out.can_upgrade=out.status=="available";return Ok(out);
                },
                "grok"=> {let v=json(&command(spec(Path::new(&i.executable),vec!["update".into(),"--check".into(),"--json".into()],false)).await?)?;if !v["error"].is_null(){return Err("backendText.updateGrokFailed".into());} field(&v,"latestVersion")?},
                "antigravity"=> {let os=if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"windows"}else{"linux"};let arch=if cfg!(target_arch="aarch64"){"arm64"}else{"amd64"};let suffix=if cfg!(target_env="musl"){"_musl"}else{""};field(&json(&get(&format!("https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{os}_{arch}{suffix}.json")).await?)?,"version")?},
                "kimi"=>get("https://code.kimi.com/kimi-code/latest").await?.trim().to_string(),
                "pi"=>field(&json(&get("https://pi.dev/api/latest-version").await?)?,"version")?,
                "opencode"=>field(&json(&get("https://registry.npmjs.org/opencode-ai/latest").await?)?,"version")?,
                "kiro"=>if kiro::bundle(Path::new(&i.executable)).is_some() { kiro::latest(i).await? } else { kiro::latest_from_check(&command(spec(Path::new(&i.executable), vec!["update".into(), "--check".into()], false)).await?)? },
                "claude-code"=>get(&format!("https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/{}",claude_channel())).await?.trim().to_string(),
                _=>return Err("backendText.updateSourceUnsupported".into()),
            }
        }
    };
    complete_check(out, latest, !matches!(p, Plan::Unsupported))
}
pub fn install_spec(
    i: &InstallationSummary,
    p: &Plan,
    latest: &str,
) -> Result<ProcessSpec, String> {
    match p {
        Plan::Npm {
            node,
            npm,
            prefix,
            package,
        } => {
            let v = version(latest).ok_or("backendText.updateVersionInvalid")?;
            Ok(spec(
                node,
                vec![
                    npm.to_string_lossy().into(),
                    "install".into(),
                    "--global".into(),
                    "--prefix".into(),
                    prefix.to_string_lossy().into(),
                    format!("{package}@{v}"),
                    "--no-audit".into(),
                    "--no-fund".into(),
                ],
                true,
            ))
        }
        Plan::Brew {
            executable,
            package,
            cask,
        } => {
            let mut args = vec!["upgrade".into()];
            if *cask {
                args.push("--cask".into());
            }
            args.push((*package).into());
            Ok(spec(executable, args, true))
        }
        Plan::Native(_) => {
            let args = match i.provider_id.as_str() {
                "kimi" => vec![
                    "__update_download".into(),
                    version(latest).ok_or("backendText.updateVersionInvalid")?.to_string(),
                    "--manual".into(),
                ],
                "pi" => vec!["update".into(), "--self".into()],
                "kiro" if kiro::bundle(Path::new(&i.executable)).is_some() => {
                    vec!["update".into(), "--non-interactive".into()]
                }
                "cursor" | "grok" | "antigravity" | "claude-code" | "kiro" => vec!["update".into()],
                "opencode" => vec![
                    "upgrade".into(),
                    version(latest).ok_or("backendText.updateVersionInvalid")?.to_string(),
                    "--method".into(),
                    "curl".into(),
                ],
                _ => return Err("backendText.updateMethodNoAuto".into()),
            };
            Ok(spec(Path::new(&i.executable), args, true))
        }
        _ => Err("backendText.updateInHost".into()),
    }
}

pub async fn wait_for_updated_version(
    executable: &Path,
    target: &str,
    limit: Duration,
) -> Result<(), String> {
    tokio::time::timeout(limit, async {
        loop {
            let mut probe = spec(executable, vec!["--version".into()], false);
            // Kimi's Windows staged swap can run a 30-second startup smoke check.
            probe.timeout = Duration::from_secs(35);
            if let Ok(actual) = command(probe).await
                && verified_version("native", &actual, target)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| "backendText.updateUnconfirmed".into())
}

#[must_use]
pub fn desktop_update_available(update: &AgentUpdate) -> bool {
    update.status == "managed"
        && update
            .current_version
            .as_deref()
            .zip(update.latest_version.as_deref())
            .is_some_and(|(current, latest)| newer(current, latest) == Ok(true))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn installation_for(provider: &str, path: &Path, v: &str) -> InstallationSummary {
        InstallationSummary {
            target_id: None,
            installation_id: agent_domain::InstallationId::new(),
            provider_id: provider.into(),
            executable: path.to_string_lossy().into(),
            fingerprint: "test".into(),
            status: agent_protocol::InstallationStatus::Ready,
            version: Some(v.into()),
            version_text: None,
            diagnostic: None,
            probe_elapsed_millis: 0,
            probed_at: chrono::Utc::now(),
        }
    }
}
