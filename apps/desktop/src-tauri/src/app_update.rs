//! Updates for dog.codes itself; Agent CLIs have their own flow in `agent_updates`.
//!
//! Each published GitHub Release carries a `latest.json` updater manifest and minisign
//! signatures made with the maintainer's updater key (see `docs/implementation/app-auto-update.md`). The official
//! Tauri updater plugin reads the manifest of the newest published, non-pre-release Release,
//! picks this platform's package and checks its signature. Here we schedule the checks,
//! download quietly, keep the verified package until the person chooses to install it, and
//! never install while the background service still has work running.
use base64::Engine as _;
use semver::{BuildMetadata, Version};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tauri::{Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

pub(crate) const READY_EVENT: &str = "app-update-ready";
/// Leaves startup work (Agent scans, the background service) alone first.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(90);
const CHECK_INTERVAL: Duration = Duration::from_hours(6);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(30);
/// Debug builds only check when pointed at a test server, and never install.
const TEST_ENDPOINT_ENV: &str = "DOGCODES_APP_UPDATE_ENDPOINT";
const PACKAGE_FILE: &str = "package";

/// A release is an update only when it is a stable version above the running one.
/// Build metadata does not order versions.
pub(crate) fn is_update(current: &Version, remote: &Version) -> bool {
    let precedence = |version: &Version| Version {
        build: BuildMetadata::EMPTY,
        ..version.clone()
    };
    remote.pre.is_empty() && precedence(remote) > precedence(current)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNotice {
    pub version: String,
    pub current_version: String,
}

struct Ready {
    notice: UpdateNotice,
    update: Update,
    package: PathBuf,
}

#[derive(Default)]
pub(crate) struct AppUpdates {
    ready: Mutex<Option<Ready>>,
}

impl AppUpdates {
    async fn notice(&self) -> Option<UpdateNotice> {
        self.ready
            .lock()
            .await
            .as_ref()
            .map(|ready| ready.notice.clone())
    }
}

/// `plugins.updater.pubkey` in `tauri.conf.json`; empty until the maintainer sets up a key.
fn configured_pubkey<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

fn test_endpoint() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(TEST_ENDPOINT_ENV)
            .ok()
            .filter(|url| !url.is_empty())
    } else {
        None
    }
}

/// Starts the periodic check. Problems are logged and retried on the next round, never shown.
pub(crate) fn start(app: &tauri::AppHandle) {
    if configured_pubkey(app).is_none() {
        eprintln!("app updates are off: tauri.conf.json has no updater public key");
        return;
    }
    if cfg!(debug_assertions) && test_endpoint().is_none() {
        return;
    }
    let Ok(cache) = app.path().app_cache_dir() else {
        eprintln!("app updates are off: no cache directory");
        return;
    };
    let cache = cache.join("app-update");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let updates = app.state::<AppUpdates>();
            match check(&app, &updates, &cache).await {
                Ok(Some(notice)) => eprintln!("app update {} is ready", notice.version),
                Ok(None) => {}
                Err(error) => eprintln!("app update check failed: {error}"),
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

fn updater<R: Runtime>(
    app: &tauri::AppHandle<R>,
    endpoint: Option<String>,
) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app
        .updater_builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .version_comparator(|current, remote| is_update(&current, &remote.version));
    if let Some(endpoint) = endpoint {
        let url = endpoint.parse().map_err(|error| format!("{error}"))?;
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    builder.build().map_err(|error| error.to_string())
}

/// One round: ask for the newest release, download it if needed and announce it.
/// Returns the notice only when a new package became ready in this round.
pub(crate) async fn check<R: Runtime>(
    app: &tauri::AppHandle<R>,
    updates: &AppUpdates,
    cache: &Path,
) -> Result<Option<UpdateNotice>, String> {
    let pubkey = configured_pubkey(app).ok_or("no updater public key")?;
    let client = updater(app, test_endpoint())?;
    let Some(update) = client.check().await.map_err(|error| error.to_string())? else {
        // Already current: drop packages left over from earlier downloads.
        remove_other_versions(cache, None).await;
        return Ok(None);
    };
    if updates
        .notice()
        .await
        .is_some_and(|notice| notice.version == update.version)
    {
        return Ok(None);
    }
    let package = cache.join(&update.version).join(PACKAGE_FILE);
    let cached = match tokio::fs::read(&package).await {
        Ok(bytes) => verify_package(&bytes, &update.signature, &pubkey, &update.version).is_ok(),
        Err(_) => false,
    };
    if !cached {
        // The plugin verifies the signature before handing the bytes over.
        let bytes = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|error| error.to_string())?;
        write_package(&package, &bytes).await?;
    }
    remove_other_versions(cache, Some(&update.version)).await;
    let notice = UpdateNotice {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
    };
    *updates.ready.lock().await = Some(Ready {
        notice: notice.clone(),
        update,
        package,
    });
    let _ = app.emit(READY_EVENT, &notice);
    Ok(Some(notice))
}

async fn write_package(package: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = package.parent().ok_or("invalid package path")?;
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| error.to_string())?;
    let partial = package.with_extension("partial");
    tokio::fs::write(&partial, bytes)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::rename(&partial, package)
        .await
        .map_err(|error| error.to_string())
}

async fn remove_other_versions(cache: &Path, keep: Option<&str>) {
    let Ok(mut entries) = tokio::fs::read_dir(cache).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if keep.is_some_and(|keep| entry.file_name() == keep) {
            continue;
        }
        let _ = tokio::fs::remove_dir_all(entry.path()).await;
    }
}

/// The same check the plugin makes after downloading, repeated for a package read back from
/// disk: a valid signature from the release key, made for exactly this version.
pub(crate) fn verify_package(
    bytes: &[u8],
    signature: &str,
    pubkey: &str,
    version: &str,
) -> Result<(), String> {
    let decode = |value: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    let key = decode(pubkey)
        .and_then(|key| minisign_verify::PublicKey::decode(&key).ok())
        .ok_or("invalid updater public key")?;
    let signature = decode(signature)
        .and_then(|signature| minisign_verify::Signature::decode(&signature).ok())
        .ok_or("invalid package signature")?;
    key.verify(bytes, &signature, false)
        .map_err(|_| "package signature does not match")?;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or("package signature has no version")?;
    let same = match (
        Version::parse(signed.trim_start_matches('v')),
        Version::parse(version.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(version)) => signed == version,
        _ => signed == version,
    };
    if same {
        Ok(())
    } else {
        Err(format!("package was signed for {signed}, not {version}"))
    }
}

/// The verified update waiting for the person, if any. The UI also listens for `READY_EVENT`.
#[tauri::command]
pub(crate) async fn app_update_ready(
    window: tauri::WebviewWindow,
    updates: tauri::State<'_, AppUpdates>,
) -> Result<Option<UpdateNotice>, String> {
    crate::require_main_artifact_window(window.label())?;
    Ok(updates.notice().await)
}

/// Installs the waiting update and restarts. Error codes for the UI: `busy` (work is still
/// running), `unavailable` (nothing verified to install) and `failed`.
#[tauri::command]
pub(crate) async fn app_update_install(
    window: tauri::WebviewWindow,
    updates: tauri::State<'_, AppUpdates>,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let app = window.app_handle().clone();
    if cfg!(debug_assertions) {
        eprintln!("app update install skipped: development builds are not replaced");
        return Err("unavailable".into());
    }
    let service = app
        .try_state::<Arc<crate::daemon::lifecycle::BackgroundService>>()
        .map(|service| service.inner().clone());
    if let Some(service) = &service
        && service.busy().await
    {
        return Err("busy".into());
    }
    let pubkey = configured_pubkey(&app).ok_or("unavailable")?;
    let ready = updates.ready.lock().await;
    let Some(ready) = ready.as_ref() else {
        return Err("unavailable".into());
    };
    let bytes = tokio::fs::read(&ready.package).await.map_err(|error| {
        eprintln!("app update package is missing: {error}");
        "unavailable".to_string()
    })?;
    if let Err(error) = verify_package(
        &bytes,
        &ready.update.signature,
        &pubkey,
        &ready.update.version,
    ) {
        eprintln!("app update package rejected: {error}");
        return Err("unavailable".into());
    }
    // Windows: the installer replaces running files and the plugin exits right after starting
    // it, so the background service stops first. macOS/Linux swap the bundle in place.
    let stop = |service: Option<Arc<crate::daemon::lifecycle::BackgroundService>>| async move {
        if let Some(service) = service {
            let _ = tokio::task::spawn_blocking(move || service.shutdown()).await;
        }
    };
    if cfg!(windows) {
        stop(service.clone()).await;
    }
    let update = ready.update.clone();
    let installed = tokio::task::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result.map_err(|error| error.to_string()));
    if let Err(error) = installed {
        eprintln!("app update install failed: {error}");
        if cfg!(windows) {
            // The service is already stopped; restarting the current version brings it back.
            app.restart();
        }
        return Err("failed".into());
    }
    stop(service).await;
    app.restart();
}

#[cfg(test)]
mod tests;
