//! Installation-scoped updates. Only trusted adapters construct commands; the UI supplies an ID.
mod usage;
pub(crate) use agent_management::updates::*;
use agent_protocol::InstallationSummary;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, Semaphore};

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
        // Everything here judges paths by this machine. One on a server could name
        // something else entirely, so those go through its own host's page.
        .find(|i| i.installation_id.to_string() == id && i.target_id.is_none())
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
        let launcher = if let Plan::StandaloneCli { prefix, .. } = &p {
            managed_cli::launcher(prefix, &i.provider_id)
        } else {
            stable_launcher(&i)
        };
        if !matches!(p, Plan::Host(_) | Plan::Unsupported) {
            // Re-read the actual binary: a native auto-updater may have run since startup.
            let actual = command(
                new_agents::version_spec(&i, &p, crate::terminal_installation::npm_runtime).await?,
            )
            .await?;
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
            if matches!(p, Plan::StandaloneCli { .. })
                && launcher.canonicalize().is_ok_and(|target| {
                    Path::new(&i.executable).canonicalize().ok() != Some(target)
                })
            {
                // Recover a completed CLI install whose previous rescan was interrupted.
                // Read its version above instead of downgrading it to the current feed.
                core.scan_agent(&i.provider_id).await?;
            }
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
        let command_spec =
            new_agents::install_spec(&i, &p, &target, crate::terminal_installation::npm_runtime)
                .await?;
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
            managed_cli::activate(&i, &p, &target, crate::terminal_installation::npm_runtime)
                .await?;
            value.message = Some("正在重新检测并验证新版本".into());
            updates.save(&i, value.clone()).await;
            let _ = app.emit("agent-update-changed", &value);
            if matches!(i.provider_id.as_str(), "kiro" | "kimi") && matches!(p, Plan::Native(_)) {
                wait_for_updated_version(&launcher, &target, Duration::from_mins(1)).await?;
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
    updates: tauri::State<'_, AgentUpdates>,
    installation_id: String,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let i = installation(&state.core, &installation_id).await?;
    let Plan::Host(app) = plan(&i) else {
        return Err("此安装不是桌面应用更新".into());
    };
    let checked = updates.check(&i, false).await;
    if !desktop_update_available(&checked) {
        return Err("未检测到桌面应用新版本".into());
    }
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
    #[cfg(windows)]
    {
        let binary = match i.provider_id.as_str() {
            "zcode" => "ZCode.exe",
            "opencode" => "OpenCode.exe",
            _ => return Err("请在宿主应用内检查更新".into()),
        };
        std::process::Command::new(app.join(binary))
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| "无法打开桌面应用".to_string())?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = app;
        Err("请在宿主应用内检查更新".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_newer_desktop_versions_can_open_a_host_updater() {
        let mut update = base(&installation_for("codex", Path::new("/engine"), "0.164.0"));
        update.status = "available".into();
        update.latest_version = Some("0.166.0".into());
        assert!(!desktop_update_available(&update));
        update.status = "managed".into();
        update.current_version = Some("26.1027.3384".into());
        for latest in [None, Some("26.1027.3384"), Some("26.1020.1000")] {
            update.latest_version = latest.map(str::to_owned);
            assert!(!desktop_update_available(&update));
        }
        update.latest_version = Some("26.1031.1000".into());
        assert!(desktop_update_available(&update));
    }

    #[cfg(windows)]
    #[test]
    fn updater_execution_paths_are_plain_while_installation_identity_stays_canonical() {
        let command = spec(
            Path::new(r"\\?\C:\node with spaces\node.exe"),
            vec![
                r"\\?\C:\node with spaces\node_modules\npm\bin\npm-cli.js".into(),
                "--prefix".into(),
                r"\\?\UNC\server\share\node".into(),
            ],
            true,
        );
        assert_eq!(
            command.executable,
            Path::new(r"C:\node with spaces\node.exe")
        );
        assert_eq!(
            command.args,
            [
                r"C:\node with spaces\node_modules\npm\bin\npm-cli.js",
                "--prefix",
                r"\\server\share\node"
            ]
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn native_verification_waits_for_a_delayed_launcher_switch() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("native agent.cmd");
        let ready = dir.path().join("ready");
        let observed = dir.path().join("observed");
        std::fs::write(&executable, "@echo off\r\nif exist \"%~dp0ready\" (echo 1.1.0) else (echo old>\"%~dp0observed\" & echo 1.0.0)\r\n").unwrap();
        let launcher = executable.canonicalize().unwrap();
        assert!(
            wait_for_updated_version(&launcher, "1.1.0", Duration::from_millis(100))
                .await
                .is_err()
        );
        let _ = std::fs::remove_file(&observed);
        let switch = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(4), async {
                while !observed.is_file() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                std::fs::write(ready, "new version").unwrap();
            })
            .await
            .unwrap();
        });
        wait_for_updated_version(&launcher, "1.1.0", Duration::from_secs(5))
            .await
            .unwrap();
        switch.await.unwrap();
    }

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
    pub(crate) fn installation_for(provider: &str, path: &Path, v: &str) -> InstallationSummary {
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
    #[tokio::test]
    async fn external_installs_check_official_releases_without_enabling_upgrades() {
        let tmp = tempfile::tempdir().unwrap();
        // npm shims and desktop-managed binaries do not live beneath package.json.
        let executable = tmp.path().join("agent.cmd");
        std::fs::write(&executable, "").unwrap();
        let feeds = [
            ("codex", "https://registry.npmjs.org/@openai/codex/latest"),
            (
                "codebuddy",
                "https://registry.npmjs.org/@tencent-ai/codebuddy-code/latest",
            ),
            (
                "pi",
                "https://registry.npmjs.org/@earendil-works/pi-coding-agent/latest",
            ),
            ("opencode", "https://registry.npmjs.org/opencode-ai/latest"),
            (
                "deepseek-harness",
                "https://registry.npmjs.org/@deepseek-ai/dsh/latest",
            ),
            ("kimi", "https://code.kimi.com/kimi-code/latest"),
            (
                "claude-code",
                "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/",
            ),
        ];
        for (provider, expected_url) in feeds {
            let installation = installation_for(provider, &executable, "1.0.0");
            assert!(
                matches!(plan(&installation), Plan::Unsupported),
                "{provider}"
            );
            for (latest, expected_status) in [
                ("1.1.0", "available"),
                ("1.0.0", "up_to_date"),
                ("0.9.0", "up_to_date"),
            ] {
                let result = inspect_external(&installation, async |url: &str| {
                    if provider == "claude-code" {
                        assert!(
                            url == format!("{expected_url}latest")
                                || url == format!("{expected_url}stable")
                        );
                    } else {
                        assert_eq!(url, expected_url);
                    }
                    Ok(if url.starts_with("https://registry.npmjs.org/") {
                        format!(r#"{{"version":"{latest}"}}"#)
                    } else {
                        format!("{latest}\n")
                    })
                })
                .await
                .unwrap();
                assert_eq!(result.status, expected_status, "{provider}");
                assert_eq!(result.latest_version.as_deref(), Some(latest));
                assert!(!result.can_upgrade, "{provider}");
                assert!(result.message.is_none());
                assert!(install_spec(&installation, &plan(&installation), latest).is_err());
            }
        }
    }

    #[tokio::test]
    async fn external_checks_preserve_failures_and_leave_unknown_sources_unsupported() {
        let installation = installation_for("codebuddy", Path::new("/external"), "1.0.0");
        assert_eq!(
            inspect_external(&installation, async |_| Err("network failure".into()))
                .await
                .unwrap_err(),
            "network failure"
        );
        for body in ["<!doctype html>", "{}", r#"{"version":"invalid"}"#] {
            assert!(
                inspect_external(&installation, async |_| Ok(body.into()))
                    .await
                    .is_err()
            );
        }
        let installation = installation_for("unknown", Path::new("/external"), "1.0.0");
        let result = inspect_external(&installation, async |_| panic!("unknown release feed"))
            .await
            .unwrap();
        assert_eq!(result.status, "unsupported");
        assert!(result.latest_version.is_none());
        assert!(!result.can_upgrade);
    }

    #[test]
    fn native_paths_use_platform_components_and_desktop_payloads_use_standalone_cli_upgrades() {
        let tmp = tempfile::tempdir().unwrap();
        let mut paths = vec![
            ("claude-code", "claude/versions/2.1.309"),
            ("kimi", ".kimi-code/bin/kimi"),
        ];
        if cfg!(windows) {
            paths.extend([
                ("claude-code", ".local/bin/claude.exe"),
                ("kimi", ".kimi-code/bin/kimi.exe"),
            ]);
        }
        for (provider, relative) in paths {
            let path = tmp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            assert!(
                matches!(
                    plan(&installation_for(provider, &path, "1.0.0")),
                    Plan::Native(_)
                ),
                "{}",
                path.display()
            );
        }
        let desktop = tmp
            .path()
            .join("AppData/Roaming/Claude/claude-code/2.1.309/payload/claude.exe");
        std::fs::create_dir_all(desktop.parent().unwrap()).unwrap();
        std::fs::write(&desktop, "").unwrap();
        let plan = plan(&installation_for("claude-code", &desktop, "2.1.309"));
        assert!(matches!(
            plan,
            Plan::StandaloneCli {
                package: "@anthropic-ai/claude-code",
                ..
            }
        ));
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
        assert_eq!(
            s.executable,
            agent_runtime::external_path(node.canonicalize().unwrap())
        );
        assert!(
            s.args.contains(
                &agent_runtime::external_path(prefix.canonicalize().unwrap())
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
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "read-only checks of installations in the local desktop database and official feeds"]
    async fn live_windows_readonly_checks() {
        let db =
            PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("codes.dog.desktop/app-v5.db");
        let connection =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let installations: Vec<(String, String, String)> = connection
            .prepare("SELECT provider_id, executable, version FROM installations WHERE version IS NOT NULL")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(connection);
        assert!(!installations.is_empty());
        let mut failed = Vec::new();
        for (provider, executable, current) in installations {
            let i = installation_for(&provider, Path::new(&executable), &current);
            let p = plan(&i);
            match inspect(&i, &p).await {
                Ok(value) => {
                    println!(
                        "{provider}: {:?} -> {:?}, {}, upgrade={}",
                        value.current_version,
                        value.latest_version,
                        value.status,
                        value.can_upgrade
                    );
                    if value.status == "unsupported" {
                        failed.push(format!("{provider}: unsupported"));
                    }
                    if !matches!(p, Plan::Host(_) | Plan::Unsupported)
                        && let Some(latest) = &value.latest_version
                    {
                        match new_agents::install_spec(
                            &i,
                            &p,
                            latest,
                            crate::terminal_installation::npm_runtime,
                        )
                        .await
                        {
                            Ok(mut update)
                                if update.executable.is_file() && update.validate().is_ok() =>
                            {
                                // Exercise Node's loader using the generated updater
                                // paths, without running any install command.
                                if matches!(p, Plan::Npm { .. } | Plan::NpmLocal { .. }) {
                                    update.args.truncate(1);
                                    update.args.push("--version".into());
                                    if let Err(error) = command(update).await {
                                        failed.push(format!("{provider}: npm preflight: {error}"));
                                    }
                                }
                            }
                            Ok(_) => {
                                failed.push(format!("{provider}: invalid manual upgrade command"));
                            }
                            Err(error) => failed.push(format!("{provider}: {error}")),
                        }
                    }
                }
                Err(error) => failed.push(format!("{provider}: {error}")),
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
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
