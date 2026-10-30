//! Installation-scoped updates. Only trusted adapters construct commands; the UI supplies an ID.
mod codex_host;
mod kiro;
mod new_agents;
mod usage;
use agent_protocol::InstallationSummary;
use agent_runtime::{ProcessError, ProcessSpec, run_bounded};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, Semaphore};

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
    pub in_use: Option<usage::Confirmation>,
}
struct Entry {
    key: String,
    at: Instant,
    value: AgentUpdate,
}
pub struct AgentUpdates {
    entries: Mutex<HashMap<String, Entry>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    checks: Semaphore,
    upgrade_resources: Mutex<HashMap<UpgradeResource, Arc<Mutex<()>>>>,
    execution_lease: Mutex<Weak<crate::daemon::WorkspaceLease>>,
    confirmations: Mutex<HashMap<String, usage::Ticket>>,
}
impl Default for AgentUpdates {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            checks: Semaphore::new(3),
            upgrade_resources: Mutex::new(HashMap::new()),
            execution_lease: Mutex::new(Weak::new()),
            confirmations: Mutex::new(HashMap::new()),
        }
    }
}
#[derive(Clone, Debug)]
enum Plan {
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
    Brew {
        executable: PathBuf,
        package: &'static str,
        cask: bool,
    },
    Host(PathBuf),
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum UpgradeResource {
    Npm(PathBuf),
    Brew(PathBuf),
    // Native updaters can share provider-wide state even across launcher aliases.
    Native(String),
}

fn upgrade_resource(i: &InstallationSummary, plan: &Plan) -> Result<UpgradeResource, String> {
    let canonical = |path: &Path| {
        path.canonicalize()
            .map_err(|_| "升级安装路径已变化，请重新检测 Agent".to_string())
    };
    match plan {
        Plan::Npm { prefix, .. } | Plan::NpmLocal { prefix, .. } => {
            Ok(UpgradeResource::Npm(canonical(prefix)?))
        }
        Plan::Brew { executable, .. } => Ok(UpgradeResource::Brew(canonical(executable)?)),
        Plan::Native(_) => Ok(UpgradeResource::Native(i.provider_id.clone())),
        Plan::Host(_) | Plan::Unsupported => Err("请在宿主应用或原安装渠道中升级".into()),
    }
}

fn package(provider: &str) -> Option<&'static str> {
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
fn host_app<'a>(provider: &str, executable: &'a Path) -> Option<&'a Path> {
    if provider == "codex"
        && let Some(app) = codex_host::desktop_app(executable)
    {
        return Some(app);
    }
    executable
        .ancestors()
        .find(|p| p.extension().is_some_and(|v| v == "app"))
}
fn plan(i: &InstallationSummary) -> Plan {
    let path = Path::new(&i.executable);
    let Ok(real) = path.canonicalize() else {
        return Plan::Unsupported;
    };
    // Kiro CLI's own updater replaces its signed CLI app bundle. This is not
    // a bundled CLI that must defer to a separate IDE updater.
    if i.provider_id == "kiro" && kiro::bundle(&real).is_some() {
        return Plan::Native("Kiro CLI 原生更新器");
    }
    // Bundled packages belong to their host, even when they contain node_modules.
    if let Some(app) = host_app(&i.provider_id, &real) {
        return Plan::Host(app.into());
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
        "claude-code" if real_text.contains("/claude/versions/") => {
            Plan::Native("Claude 原生更新器")
        }
        "kimi" if real_text.contains("/.kimi-code/bin/") => Plan::Native("Kimi 原生更新器"),
        "opencode" if new_agents::is_opencode_native(&real) => Plan::Native("OpenCode 原生更新器"),
        _ => Plan::Unsupported,
    }
}
// Native installers may replace a stable launcher with a new version-directory target.
// Remember that exact launcher before mutation so rescan can find the new installation ID.
fn stable_launcher(i: &InstallationSummary) -> PathBuf {
    let original = PathBuf::from(&i.executable);
    if i.provider_id == "pi"
        && let Some(root) = original
            .canonicalize()
            .ok()
            .and_then(|p| new_agents::pi_managed_root(&p))
        && let Some(agent) = root.parent()
    {
        return agent.join("bin/pi");
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
    let launcher = home.join(relative);
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
fn normalize_build(v: &str) -> String {
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
fn verified_version(provider: &str, actual: &str, target: &str) -> bool {
    if provider == "cursor" {
        normalize_build(actual) == normalize_build(target)
    } else {
        version(actual)
            .zip(version(target))
            .is_some_and(|(a, b)| a >= b)
    }
}
fn key(i: &InstallationSummary) -> String {
    format!(
        "{}|{}|{:?}|{:?}",
        i.fingerprint,
        i.executable,
        i.version,
        desktop_version(i)
    )
}
fn desktop_version(i: &InstallationSummary) -> Option<String> {
    if !matches!(i.provider_id.as_str(), "codex" | "zcode") {
        return None;
    }
    let executable = Path::new(&i.executable).canonicalize().ok()?;
    let app = host_app(&i.provider_id, &executable)?;
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    info.as_dictionary()?
        .get("CFBundleShortVersionString")?
        .as_string()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
fn base(i: &InstallationSummary) -> AgentUpdate {
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
fn version(s: &str) -> Option<semver::Version> {
    s.split_whitespace().find_map(|p| {
        semver::Version::parse(
            p.trim_start_matches('v')
                .trim_matches(|c| c == '(' || c == ')'),
        )
        .ok()
    })
}
fn newer(current: &str, latest: &str) -> Result<bool, String> {
    let a = version(current).ok_or("无法识别本机版本，暂不自动升级")?;
    let b = version(latest).ok_or("官方更新源返回了无法识别的版本")?;
    Ok(b > a)
}
fn spec(executable: &Path, args: Vec<String>, updating: bool) -> ProcessSpec {
    let mut p = ProcessSpec::probe(executable.into(), args);
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
        .map(PathBuf::from);
    p.env_overrides.insert("CI".into(), "1".into());
    p.env_overrides.insert("NO_COLOR".into(), "1".into());
    // Explicit updater subcommands still work. Passive version probes must never auto-install.
    p.env_overrides
        .insert("AGY_CLI_DISABLE_AUTO_UPDATE".into(), "true".into());
    p
}
async fn command(p: ProcessSpec) -> Result<String, String> {
    let out = run_bounded(&p).await.map_err(|error| match error {
        ProcessError::TimedOut { .. } => "更新命令等待超时，请重新检测版本后重试升级".to_string(),
        ProcessError::Spawn(_) => "无法启动更新命令，请重新检测安装路径和权限".to_string(),
        _ => "更新命令执行中断，请重新检测版本后重试".to_string(),
    })?;
    if !out.success {
        return Err(command_failure(out.exit_code, &out.stdout, &out.stderr));
    }
    // Never forward arbitrary CLI output (may contain account details) to the webview.
    Ok(out.stdout)
}
fn command_failure(code: Option<i32>, stdout: &str, stderr: &str) -> String {
    // Match known diagnostics, but never return the raw output: it may contain secrets.
    let output = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if output.contains("another cli update is already in progress") {
        return "Cursor 原生更新器存在升级锁，请等待当前更新结束后重试升级".into();
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
        return "更新下载连接中断，请检查代理或网络后重试升级".into();
    }
    format!(
        "更新命令未成功（退出码 {}），请检查网络和安装权限后重试",
        code.map_or("未知".into(), |v| v.to_string())
    )
}
async fn get(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("dogcodes/agent-updates")
        .build()
        .map_err(|_| "无法初始化版本检查")?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "连接官方更新源失败，请稍后重试")?
        .error_for_status()
        .map_err(|_| "官方更新源暂时不可用")?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取版本信息失败")? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("版本信息过大".into());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| "版本信息编码无效".into())
}
fn json(s: &str) -> Result<Value, String> {
    serde_json::from_str(s).map_err(|_| "更新源未返回有效的版本信息".into())
}
fn field(v: &Value, name: &str) -> Result<String, String> {
    v[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 128)
        .map(str::to_owned)
        .ok_or("更新源缺少版本信息".into())
}
fn claude_channel() -> String {
    std::env::var_os("HOME")
        .and_then(|h| std::fs::read(PathBuf::from(h).join(".claude/settings.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["autoUpdatesChannel"].as_str().map(str::to_owned))
        .filter(|v| v == "stable")
        .unwrap_or("latest".into())
}
async fn inspect(i: &InstallationSummary, p: &Plan) -> Result<AgentUpdate, String> {
    let mut out = base(i);
    let latest = match p {
        Plan::Host(app) => {
            out.status = "managed".into();
            out.source = "桌面应用".into();
            out.message = Some("此 CLI 随宿主应用更新，请在应用内检查更新".into());
            if i.provider_id == "codex" {
                return codex_host::inspect(app, out).await;
            }
            if i.provider_id == "zcode" {
                let info = plist::Value::from_file(app.join("Contents/Info.plist"))
                    .map_err(|_| "无法读取 ZCode 应用版本")?;
                out.current_version = info
                    .as_dictionary()
                    .and_then(|d| d.get("CFBundleShortVersionString"))
                    .and_then(plist::Value::as_string)
                    .map(str::to_owned);
                out.source = "ZCode 桌面应用".into();
                // The desktop's update API uses host versions, never compare against zcode-cli versions.
                let arch = if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else {
                    "x86_64"
                };
                let body=get(&format!("https://zcode.z.ai/api/v1/releases/electron/manifest?platform=darwin-{arch}&channel=1")).await?;
                let latest = body
                    .lines()
                    .find_map(|l| l.strip_prefix("version:"))
                    .map(|s| s.trim().trim_matches(['\'', '"']).to_string())
                    .ok_or("ZCode 更新源缺少应用版本")?;
                out.latest_version = Some(latest.clone());
                if newer(
                    out.current_version.as_deref().ok_or("应用版本未知")?,
                    &latest,
                )? {
                    out.message = Some(format!("桌面应用可升级至 {latest}，请在 ZCode 中完成升级"));
                } else {
                    out.message = Some("桌面应用已是最新版本".into());
                }
            }
            return Ok(out);
        }
        Plan::Unsupported => {
            out.status = "unsupported".into();
            out.source = "外部安装".into();
            out.message = Some("暂不支持此安装来源的自动更新，请通过原安装渠道升级".into());
            return Ok(out);
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
                    out.status=match status.as_str(){"up_to_date"=>"up_to_date","update_available"=>"available",_=>return Err("Cursor 暂未确认更新状态".into())}.into();
                    out.latest_version=Some(latest);out.can_upgrade=out.status=="available";return Ok(out);
                },
                "grok"=> {let v=json(&command(spec(Path::new(&i.executable),vec!["update".into(),"--check".into(),"--json".into()],false)).await?)?;if !v["error"].is_null(){return Err("Grok 官方更新检查失败".into());} field(&v,"latestVersion")?},
                "antigravity"=> {let os=if cfg!(target_os="macos"){"darwin"}else if cfg!(windows){"windows"}else{"linux"};let arch=if cfg!(target_arch="aarch64"){"arm64"}else{"amd64"};let suffix=if cfg!(target_env="musl"){"_musl"}else{""};field(&json(&get(&format!("https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{os}_{arch}{suffix}.json")).await?)?,"version")?},
                "kimi"=>get("https://code.kimi.com/kimi-code/latest").await?.trim().to_string(),
                "pi"=>field(&json(&get("https://pi.dev/api/latest-version").await?)?,"version")?,
                "opencode"=>field(&json(&get("https://registry.npmjs.org/opencode-ai/latest").await?)?,"version")?,
                "kiro"=>kiro::latest(i).await?,
                "claude-code"=>get(&format!("https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/{}",claude_channel())).await?.trim().to_string(),
                _=>return Err("尚未接入此更新源".into()),
            }
        }
    };
    out.can_upgrade = newer(
        i.version
            .as_deref()
            .ok_or("本机版本未知，请先重新检测 Agent")?,
        &latest,
    )?;
    out.status = if out.can_upgrade {
        "available"
    } else {
        "up_to_date"
    }
    .into();
    out.latest_version = Some(latest);
    Ok(out)
}
fn install_spec(i: &InstallationSummary, p: &Plan, latest: &str) -> Result<ProcessSpec, String> {
    match p {
        Plan::Npm {
            node,
            npm,
            prefix,
            package,
        } => {
            let v = version(latest).ok_or("更新版本无效")?;
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
                    version(latest).ok_or("更新版本无效")?.to_string(),
                    "--manual".into(),
                ],
                "cursor" | "grok" | "antigravity" | "claude-code" => vec!["update".into()],
                "pi" => vec!["update".into(), "--self".into()],
                "kiro" => vec!["update".into(), "--non-interactive".into()],
                "opencode" => vec![
                    "upgrade".into(),
                    version(latest).ok_or("更新版本无效")?.to_string(),
                    "--method".into(),
                    "curl".into(),
                ],
                _ => return Err("此安装方式不支持自动升级".into()),
            };
            Ok(spec(Path::new(&i.executable), args, true))
        }
        _ => Err("请在宿主应用或原安装渠道中升级".into()),
    }
}
impl AgentUpdates {
    async fn resource_lock(&self, resource: UpgradeResource) -> Arc<Mutex<()>> {
        self.upgrade_resources
            .lock()
            .await
            .entry(resource)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    // Independent installers share execution exclusion, not an upgrade mutex.
    // The weak cache releases the service-owned gate after the last job exits.
    async fn execution_lease(
        &self,
        core: &crate::daemon::CoreClient,
    ) -> Result<Arc<crate::daemon::WorkspaceLease>, String> {
        let mut lease = self.execution_lease.lock().await;
        if let Some(active) = lease.upgrade() {
            return Ok(active);
        }
        let active = Arc::new(core.try_workspace_mutation().await?);
        *lease = Arc::downgrade(&active);
        Ok(active)
    }
    async fn lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    async fn save(&self, i: &InstallationSummary, value: AgentUpdate) {
        self.entries.lock().await.insert(
            i.installation_id.to_string(),
            Entry {
                key: key(i),
                at: Instant::now(),
                value,
            },
        );
    }
    async fn cached(&self, i: &InstallationSummary, force: bool) -> Option<AgentUpdate> {
        self.entries
            .lock()
            .await
            .get(&i.installation_id.to_string())
            .filter(|e| {
                e.value.status == "upgrading"
                    || (!force
                        && e.key == key(i)
                        && e.at.elapsed()
                            < Duration::from_secs(if e.value.status == "error" {
                                60
                            } else {
                                21600
                            }))
            })
            .map(|e| e.value.clone())
    }
    async fn check(&self, i: &InstallationSummary, force: bool) -> AgentUpdate {
        if let Some(v) = self.cached(i, force).await {
            return v;
        }
        let lock = self.lock(&i.installation_id.to_string()).await;
        let _guard = lock.lock().await;
        if let Some(v) = self.cached(i, force).await {
            return v;
        }
        let _permit = self.checks.acquire().await;
        let value = inspect(i, &plan(i)).await.unwrap_or_else(|e| {
            let mut v = base(i);
            v.status = "error".into();
            v.message = Some(e);
            v
        });
        self.save(i, value.clone()).await;
        value
    }
}
async fn installation(
    core: &crate::daemon::CoreClient,
    id: &str,
) -> Result<InstallationSummary, String> {
    core.bootstrap()
        .await?
        .installations
        .into_iter()
        .find(|i| i.installation_id.to_string() == id)
        .ok_or("此 Agent 安装已不存在，请重新检测".into())
}
#[tauri::command]
pub async fn core_check_agent_update(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::DesktopState>,
    updates: tauri::State<'_, AgentUpdates>,
    installation_id: String,
    force: Option<bool>,
) -> Result<AgentUpdate, String> {
    crate::require_main_artifact_window(window.label())?;
    let i = installation(&state.core, &installation_id).await?;
    Ok(updates.check(&i, force.unwrap_or(false)).await)
}
#[tauri::command]
pub async fn core_upgrade_agent(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::DesktopState>,
    installation_id: String,
    confirmation_token: Option<String>,
) -> Result<AgentUpdate, String> {
    crate::require_main_artifact_window(window.label())?;
    let core = Arc::clone(&state.core);
    // A detached owned job survives navigation / a dropped IPC reply. The gate is held until rescan.
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<AgentUpdates>();
        let mut i = installation(&core, &installation_id).await?;
        let lock = updates.lock(&installation_id).await;
        let _guard = lock.lock().await;
        let p = plan(&i);
        if matches!(p, Plan::Host(_) | Plan::Unsupported) {
            return Err("请在宿主应用或原安装渠道中升级".into());
        }
        let resource = upgrade_resource(&i, &p)?;
        let resource_lock = updates.resource_lock(resource).await;
        let _upgrade = resource_lock
            .try_lock()
            .map_err(|_| "另一个升级正在使用相同的安装目录或更新器，请等待其完成后重试")?;
        let launcher = stable_launcher(&i);
        if !matches!(p, Plan::Host(_) | Plan::Unsupported) {
            // Re-read the actual binary: a native auto-updater may have run since startup.
            let actual = command(new_agents::version_spec(&i, &p).await?).await?;
            i.version = Some(if i.provider_id == "cursor" {
                actual.trim().to_string()
            } else {
                version(&actual)
                    .ok_or("升级前无法验证当前版本")?
                    .to_string()
            });
        }
        let mut value = inspect(&i, &p).await?;
        if !value.can_upgrade {
            updates.save(&i, value.clone()).await;
            return Ok(value);
        }
        if let Some(confirmation) =
            usage::prepare(&core, &updates, &i, &p, confirmation_token.as_deref()).await?
        {
            let mut value = base(&i);
            value.status = "in_use".into();
            value.in_use = Some(confirmation);
            return Ok(value);
        }
        let _lease = updates
            .execution_lease(&core)
            .await
            .map_err(|_| "请等待正在执行的任务结束后再升级 Agent")?;
        if core
            .bootstrap()
            .await?
            .tasks
            .iter()
            .any(|t| t.active_primary_run.is_some())
        {
            return Err("请等待正在执行的任务结束后再升级 Agent".into());
        }
        // Recheck after acquiring the execution gate; external processes can start at any time.
        if let Some(confirmation) = usage::prepare(&core, &updates, &i, &p, None).await? {
            let mut value = base(&i);
            value.status = "in_use".into();
            value.in_use = Some(confirmation);
            return Ok(value);
        }
        let target = value.latest_version.clone().ok_or("缺少目标版本")?;
        let command_spec = new_agents::install_spec(&i, &p, &target).await?;
        value.status = "upgrading".into();
        value.can_upgrade = false;
        value.message = Some("正在下载并安装，较大的更新可能需要几分钟".into());
        updates.save(&i, value.clone()).await;
        let _ = app.emit("agent-update-changed", &value);
        let result = async {
            if i.provider_id == "kimi" && matches!(p, Plan::Native(_)) {
                let help = command(spec(
                    Path::new(&i.executable),
                    vec!["__update_download".into(), "--help".into()],
                    false,
                ))
                .await?;
                if !help.contains("--manual") {
                    return Err("此 Kimi 版本不支持非交互更新，请在终端执行 kimi upgrade".into());
                }
            }
            command(command_spec).await?;
            value.message = Some("正在重新检测并验证新版本".into());
            updates.save(&i, value.clone()).await;
            let _ = app.emit("agent-update-changed", &value);
            if i.provider_id == "kiro" && matches!(p, Plan::Native(_)) {
                kiro::verify_updated(&launcher, &target).await?;
            }
            let snapshot = core.scan_agent(&i.provider_id).await?;
            let resolved = launcher
                .canonicalize()
                .map_err(|_| "升级后无法找到 Agent 启动文件")?;
            let fresh = snapshot
                .installations
                .iter()
                .find(|v| {
                    v.provider_id == i.provider_id
                        && Path::new(&v.executable)
                            .canonicalize()
                            .is_ok_and(|p| p == resolved)
                })
                .ok_or("升级后未找到原安装，请重新检测 Agent")?;
            let now = fresh.version.as_deref().ok_or("升级后无法读取版本")?;
            let verified = verified_version(&i.provider_id, now, &target);
            if !verified {
                return Err(
                    "更新器已退出，但尚未确认目标版本生效。请重新检测或在原安装渠道完成升级".into(),
                );
            }
            value.current_version.clone_from(&fresh.version);
            value.status = "updated".into();
            value.message = Some("升级完成，已重新加载 Agent".into());
            updates.save(&i, value.clone()).await;
            let mut current = value.clone();
            current.installation_id = fresh.installation_id.to_string();
            updates.save(fresh, current.clone()).await;
            let _ = app.emit("agent-update-changed", &current);
            Ok::<(), String>(())
        }
        .await;
        if let Err(e) = result {
            value.status = "error".into();
            value.can_upgrade = true;
            value.message = Some(e);
            updates.save(&i, value.clone()).await;
        }
        let _ = app.emit("agent-update-changed", &value);
        Ok(value)
    })
    .await
    .map_err(|_| "升级任务意外中断".to_string())?
}

#[tauri::command]
pub async fn core_open_agent_updater(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, crate::DesktopState>,
    installation_id: String,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let i = installation(&state.core, &installation_id).await?;
    let Plan::Host(app) = plan(&i) else {
        return Err("此 Agent 没有宿主应用更新入口".into());
    };
    #[cfg(target_os = "macos")]
    {
        command(spec(
            Path::new("/usr/bin/open"),
            vec!["-a".into(), app.to_string_lossy().into()],
            false,
        ))
        .await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("请在宿主应用内检查更新".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn independent_native_upgrades_do_not_share_an_installation_lock() {
        let state = AgentUpdates::default();
        let kimi = installation_for("kimi", Path::new("/kimi"), "1.0.0");
        let cursor = installation_for("cursor", Path::new("/cursor"), "1.0.0");
        let first = state
            .resource_lock(upgrade_resource(&kimi, &Plan::Native("kimi")).unwrap())
            .await;
        let other = state
            .resource_lock(upgrade_resource(&cursor, &Plan::Native("cursor")).unwrap())
            .await;
        let same = state
            .resource_lock(upgrade_resource(&kimi, &Plan::Native("kimi")).unwrap())
            .await;
        let guard = first.try_lock().unwrap();
        assert!(other.try_lock().is_ok());
        assert!(same.try_lock().is_err());
        drop(guard);
        assert!(same.try_lock().is_ok());
    }

    #[tokio::test]
    async fn npm_upgrades_share_a_prefix_lock_but_different_prefixes_can_upgrade() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("node-a");
        let b = tmp.path().join("node-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let kimi = installation_for("kimi", Path::new("/kimi"), "1.0.0");
        let claude = installation_for("claude-code", Path::new("/claude"), "1.0.0");
        let npm = |prefix: PathBuf| Plan::Npm {
            node: prefix.join("bin/node"),
            npm: prefix.join("npm-cli.js"),
            prefix,
            package: "@moonshot-ai/kimi-code",
        };
        let state = AgentUpdates::default();
        let first = state
            .resource_lock(upgrade_resource(&kimi, &npm(a.clone())).unwrap())
            .await;
        let same = state
            .resource_lock(
                upgrade_resource(
                    &claude,
                    &Plan::NpmLocal {
                        prefix: a,
                        package: "@anthropic-ai/claude-code",
                    },
                )
                .unwrap(),
            )
            .await;
        let other = state
            .resource_lock(upgrade_resource(&kimi, &npm(b)).unwrap())
            .await;
        let _guard = first.try_lock().unwrap();
        assert!(same.try_lock().is_err());
        assert!(other.try_lock().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn installation_resource_paths_resolve_aliases_for_npm_and_homebrew() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        let alias = tmp.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        std::fs::write(real.join("brew"), "").unwrap();
        let i = installation_for("codex", Path::new("/codex"), "1.0.0");
        let npm = |prefix| Plan::NpmLocal {
            prefix,
            package: "@openai/codex",
        };
        let brew = |executable| Plan::Brew {
            executable,
            package: "codex",
            cask: false,
        };
        assert_eq!(
            upgrade_resource(&i, &npm(real.clone())).unwrap(),
            upgrade_resource(&i, &npm(alias.clone())).unwrap()
        );
        assert_eq!(
            upgrade_resource(&i, &brew(real.join("brew"))).unwrap(),
            upgrade_resource(&i, &brew(alias.join("brew"))).unwrap()
        );
    }

    #[tokio::test]
    async fn parallel_upgrades_keep_execution_blocked_until_the_last_job_exits() {
        let core = Arc::new(agent_core::Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let client = crate::daemon::CoreClient::local(core.clone());
        let state = AgentUpdates::default();
        let first = state.execution_lease(&client).await.unwrap();
        let second = state.execution_lease(&client).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(core.try_workspace_mutation().is_err());
        drop(first);
        assert!(core.try_workspace_mutation().is_err());
        drop(second);
        assert!(core.try_workspace_mutation().is_ok());
        let held_by_another_operation = core.try_workspace_mutation().unwrap();
        assert!(state.execution_lease(&client).await.is_err());
        drop(held_by_another_operation);
        assert!(state.execution_lease(&client).await.is_ok());
    }

    #[tokio::test]
    async fn nested_codex_cli_uses_desktop_host_and_tracks_its_version() {
        let root = tempfile::tempdir().unwrap();
        // Resolve the host by bundle identity, even if the user renamed the app.
        let app = root.path().join("Renamed.app");
        let cli_app = app.join("Contents/Resources/codex-cli/CodexCLI.app");
        let nested = cli_app.join("Contents/MacOS/codex");
        let launcher = app.join("Contents/Resources/codex-cli/bin/codex");
        let legacy = app.join("Contents/Resources/codex");
        for executable in [&nested, &launcher, &legacy] {
            std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
            std::fs::write(executable, "same CLI").unwrap();
        }
        let write_info = |bundle: &Path, id: &str, version: Option<&str>| {
            let mut info = plist::Dictionary::new();
            info.insert("CFBundleIdentifier".into(), plist::Value::String(id.into()));
            if let Some(version) = version {
                info.insert(
                    "CFBundleShortVersionString".into(),
                    plist::Value::String(version.into()),
                );
            }
            plist::Value::Dictionary(info)
                .to_file_xml(bundle.join("Contents/Info.plist"))
                .unwrap();
        };
        write_info(&cli_app, "com.openai.codex.cli", None);
        write_info(&app, "com.openai.codex", Some("26.1023.50603"));
        let host = app.canonicalize().unwrap();
        for executable in [&nested, &launcher, &legacy] {
            let i = installation_for("codex", executable, "0.165.0-alpha.1.2");
            let p = plan(&i);
            assert!(matches!(&p, Plan::Host(path) if path == &host), "{p:?}");
            assert_eq!(base(&i).desktop_version.as_deref(), Some("26.1023.50603"));
            assert!(install_spec(&i, &p, "26.1027.30824").is_err());
        }
        let i = installation_for("codex", &nested, "0.165.0-alpha.1.2");
        let cache = AgentUpdates::default();
        let mut failed = base(&i);
        failed.status = "error".into();
        cache.save(&i, failed).await;
        assert!(cache.cached(&i, false).await.is_some());
        write_info(&app, "com.openai.codex", Some("26.1027.30824"));
        assert!(cache.cached(&i, false).await.is_none());
        assert_eq!(base(&i).desktop_version.as_deref(), Some("26.1027.30824"));

        // A different host must not gain access to the official Codex updater.
        write_info(&app, "com.example.other", Some("1.0.0"));
        assert!(matches!(plan(&i), Plan::Host(path) if path == cli_app.canonicalize().unwrap()));
    }

    #[tokio::test]
    async fn desktop_version_survives_offline_checks_and_invalidates_cache_without_cli_change() {
        let root = tempfile::tempdir().unwrap();
        let contents = root.path().join("ChatGPT.app/Contents");
        let executable = contents.join("Resources/codex");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "same CLI").unwrap();
        let i = installation_for("codex", &executable, "0.155.0-alpha.16.4");
        let write_version = |version: &str| {
            let mut info = plist::Dictionary::new();
            info.insert(
                "CFBundleShortVersionString".into(),
                plist::Value::String(version.into()),
            );
            plist::Value::Dictionary(info)
                .to_file_xml(contents.join("Info.plist"))
                .unwrap();
        };
        write_version("26.915.31945");
        let cache = AgentUpdates::default();
        let mut failed = base(&i);
        failed.status = "error".into();
        assert_eq!(failed.desktop_version.as_deref(), Some("26.915.31945"));
        assert_eq!(failed.current_version, i.version);
        cache.save(&i, failed).await;
        assert!(cache.cached(&i, false).await.is_some());
        write_version("26.917.71314");
        assert!(cache.cached(&i, false).await.is_none());
        assert_eq!(base(&i).desktop_version.as_deref(), Some("26.917.71314"));
        let standalone = root.path().join("codex");
        std::fs::write(&standalone, "CLI").unwrap();
        assert!(
            base(&installation_for("codex", &standalone, "0.155.0"))
                .desktop_version
                .is_none()
        );
    }
    pub(super) fn installation_for(provider: &str, path: &Path, v: &str) -> InstallationSummary {
        InstallationSummary {
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
    #[test]
    fn versions_do_not_downgrade_or_guess() {
        assert_eq!(newer("2.1.269", "2.1.272"), Ok(true));
        assert_eq!(newer("2.1.272", "2.1.269"), Ok(false));
        assert_eq!(newer("v2.0.0", "2.0.0"), Ok(false));
        assert_eq!(newer("2.0.0", "2.1.0-beta.1"), Ok(true));
        assert!(newer("unknown", "1.0.0").is_err());
        assert!(newer("1.0.0", "<!doctype html>").is_err());
    }
    #[test]
    fn success_requires_the_target_build_to_be_installed() {
        assert!(!verified_version("kimi", "0.42.0", "0.43.0"));
        assert!(verified_version("kimi", "0.43.0", "0.43.0"));
        assert!(verified_version(
            "cursor",
            "2026.9.10-fd3934a",
            "2026.09.10-fd3934a"
        ));
        assert!(!verified_version(
            "cursor",
            "2026.9.10-old",
            "2026.09.10-new"
        ));
    }
    #[test]
    fn interrupted_download_diagnostic_is_actionable_without_exposing_cli_output() {
        let message = command_failure(
            Some(1),
            "private-account",
            "TelemetrySafeError: The connection dropped while downloading the update (attempt 3/3: aborted). private-token",
        );
        assert!(message.contains("下载连接中断"));
        assert!(message.contains("重试升级"));
        assert!(!message.contains("private"));
        assert!(command_failure(Some(2), "", "unknown private-token").contains("退出码 2"));
    }
    #[test]
    fn cursor_install_lock_has_a_specific_safe_diagnostic() {
        let message = command_failure(
            Some(1),
            "private-account",
            "Error: Update failed: Another CLI update is already in progress. remove /private/path/.install.lock private-token",
        );
        assert!(message.contains("Cursor 原生更新器存在升级锁"));
        assert!(message.contains("重试升级"));
        assert!(!message.contains("private"));
        assert!(!message.contains("remove"));
    }
    #[cfg(unix)]
    #[test]
    fn codebuddy_homebrew_uses_official_formula() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = tmp.path().join(
            "Cellar/codebuddy-code/2.0/libexec/lib/node_modules/@tencent-ai/codebuddy-code/cli.js",
        );
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(tmp.path().join("bin")).unwrap();
        std::fs::write(&executable, "CLI").unwrap();
        std::fs::write(
            executable.parent().unwrap().join("package.json"),
            r#"{"name":"@tencent-ai/codebuddy-code"}"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("bin/brew"), "brew").unwrap();
        assert!(matches!(
            plan(&installation_for("codebuddy", &executable, "2.0")),
            Plan::Brew {
                package: "codebuddy-code",
                cask: false,
                ..
            }
        ));
    }
    #[test]
    fn npm_updates_exact_prefix_and_rejects_foreign_package() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("node version");
        let modules = prefix.join(if cfg!(windows) {
            "node_modules"
        } else {
            "lib/node_modules"
        });
        let node = prefix.join(if cfg!(windows) {
            "node.exe"
        } else {
            "bin/node"
        });
        let pkg = modules.join("@tencent-ai/codebuddy-code");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(modules.join("npm/bin")).unwrap();
        for f in [
            pkg.join("cli.js"),
            node.clone(),
            modules.join("npm/bin/npm-cli.js"),
        ] {
            std::fs::write(f, "").unwrap();
        }
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"@tencent-ai/codebuddy-code"}"#,
        )
        .unwrap();
        let i = installation_for("codebuddy", &pkg.join("cli.js"), "2.0.0");
        let p = plan(&i);
        let s = install_spec(&i, &p, "2.1.0").unwrap();
        assert_eq!(s.executable, node.canonicalize().unwrap());
        assert!(
            s.args.contains(
                &prefix
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(s.args.contains(&"@tencent-ai/codebuddy-code@2.1.0".into()));
        assert!(install_spec(&i, &p, "2.1.0;touch /tmp/unwanted").is_err());
        std::fs::write(pkg.join("package.json"), r#"{"name":"foreign-package"}"#).unwrap();
        assert!(matches!(plan(&i), Plan::Unsupported));
    }
    #[test]
    fn bundled_cli_never_becomes_a_cli_install_command() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ChatGPT.app/Contents/Resources/codex");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        let i = installation_for("codex", &path, "0.153.4");
        let p = plan(&i);
        assert!(matches!(p, Plan::Host(_)));
        assert!(install_spec(&i, &p, "0.154.0").is_err());
    }
    #[tokio::test]
    async fn cache_tracks_installation_and_running_updates() {
        let state = AgentUpdates::default();
        let mut i = installation_for("cursor", Path::new("/none"), "1.0.0");
        let mut value = base(&i);
        value.status = "up_to_date".into();
        state.save(&i, value.clone()).await;
        assert!(state.cached(&i, false).await.is_some());
        assert!(state.cached(&i, true).await.is_none());
        i.fingerprint = "changed".into();
        assert!(state.cached(&i, false).await.is_none());
        value.status = "upgrading".into();
        state.save(&i, value).await;
        assert_eq!(state.cached(&i, true).await.unwrap().status, "upgrading");
        let first = state.lock("same-install").await;
        let second = state.lock("same-install").await;
        let _guard = first.lock().await;
        assert!(second.try_lock().is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn fake_updater_executes_without_shell_and_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent with spaces");
        let marker = tmp.path().join("updated");
        // A fixed executable avoids Linux ETXTBSY when another test forks
        // while a temporary script is open for writing. Keep the spaced path.
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-updater"),
            &path,
        )
        .unwrap();
        let i = installation_for("grok", &path, "1.0.0");
        let s = install_spec(&i, &Plan::Native("test"), "1.1.0").unwrap();
        assert_eq!(s.args, vec!["update"]);
        let result = command(s).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(marker.is_file());
        let error = command(spec(&path, vec!["fail".into()], true))
            .await
            .unwrap_err();
        assert!(!error.contains("private-token"));
        assert!(error.contains("退出码 1"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn updater_timeout_is_distinguished_from_launch_failure() {
        let mut slow = spec(Path::new("/bin/sleep"), vec!["30".into()], true);
        slow.timeout = Duration::from_millis(30);
        let timeout = command(slow).await.unwrap_err();
        assert!(timeout.contains("等待超时"));
        assert!(timeout.contains("重试升级"));
        let missing = command(spec(Path::new("/missing/update-command"), vec![], true))
            .await
            .unwrap_err();
        assert!(missing.contains("无法启动"));
        assert!(!missing.contains("超时"));
    }
    /// Read-only smoke test of installed executables and official feeds. No updater is executed.
    #[tokio::test]
    #[ignore = "real local installations and internet required"]
    async fn live_readonly_checks() {
        let home = std::env::var("HOME").unwrap();
        let entries = [
            ("cursor", format!("{home}/.local/bin/cursor-agent")),
            ("grok", format!("{home}/.local/bin/grok")),
            ("kimi", format!("{home}/.kimi-code/bin/kimi")),
            ("antigravity", format!("{home}/.local/bin/agy")),
            ("claude-code", format!("{home}/.local/bin/claude")),
            (
                "codebuddy",
                format!("{home}/.nvm-arm/versions/node/v22.23.2/bin/codebuddy"),
            ),
            (
                "codex",
                "/Applications/ChatGPT.app/Contents/Resources/codex".into(),
            ),
            (
                "zcode",
                "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
            ),
        ];
        for (provider, path) in entries {
            if !Path::new(&path).exists() {
                println!("{provider}: not installed");
                continue;
            }
            let actual = if provider == "zcode" {
                "0.16.5".into()
            } else {
                command(spec(Path::new(&path), vec!["--version".into()], false))
                    .await
                    .unwrap()
            };
            let v = version(&actual).map_or_else(|| actual.trim().to_string(), |v| v.to_string());
            let i = installation_for(provider, Path::new(&path), &v);
            let result = inspect(&i, &plan(&i)).await;
            match &result {
                Ok(v) => println!(
                    "{provider}: {} -> {:?}, {}, {}",
                    v.current_version.as_deref().unwrap_or("?"),
                    v.latest_version,
                    v.status,
                    v.source
                ),
                Err(e) => println!("{provider}: {e}"),
            }
            assert!(result.is_ok(), "{provider}: {result:?}");
        }
    }
}
