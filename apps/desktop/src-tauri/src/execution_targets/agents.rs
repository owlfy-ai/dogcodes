//! Registers drivers for the agents a connected machine reports. The shim finds them,
//! by the same manifests and version rules as this machine; only the driver lives here.
use agent_core::Core;
use agent_domain::{ProviderId, TargetId};
use agent_management::{
    detect::{Detected, SUPPORTED_PROTOCOLS},
    operation::{self, Request},
};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, InstallationSummary,
    ReconcileMissingInstallation, RecordInstallationProbe,
};
use agent_registry::{BUILTIN_MANIFESTS, ProviderManifest};
use agent_runtime::{SharedTarget, TargetExecutable};
use driver_api::{AgentDriver, QuotaProvider};
use serde::Serialize;
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetAgent {
    pub(super) provider_id: String,
    display_name: String,
    executable: String,
    version: Option<String>,
    status: InstallationStatus,
    diagnostic: Option<String>,
}

async fn installation_of(
    core: &Core,
    target: &TargetId,
    provider: &ProviderId,
    executable: &str,
) -> Option<InstallationSummary> {
    core.bootstrap()
        .await
        .ok()?
        .installations
        .into_iter()
        .find(|installation| {
            installation.target_id.as_ref() == Some(target)
                && installation.provider_id == provider.as_str()
                && installation.executable == executable
        })
}

type Connected = (Arc<dyn AgentDriver>, Arc<dyn QuotaProvider>);

fn connected<D: AgentDriver + QuotaProvider + 'static>(driver: D) -> Connected {
    let driver = Arc::new(driver);
    (driver.clone(), driver)
}

async fn connect_driver(
    manifest: &ProviderManifest,
    installation: &InstallationSummary,
    executable: TargetExecutable,
) -> Result<Connected, driver_api::DriverError> {
    let id = installation.installation_id.as_str();
    let invalid = || driver_api::DriverError::Protocol("invalid installation".into());
    Ok(match manifest.protocol.kind.as_str() {
        "app_server_json_rpc" => connected(
            codex_driver::CodexDriver::connect_ready(
                executable,
                id.to_owned(),
                installation.version.clone().ok_or_else(invalid)?,
            )
            .await?,
        ),
        "claude_stream_json" => connected(
            claude_driver::ClaudeDriver::connect_ready(
                manifest.id.as_str(),
                id,
                executable,
                installation.version.clone(),
                driver_api::SessionResumeCapability::Supported,
            )
            .await?,
        ),
        "pi_rpc" => connected(
            pi_driver::PiDriver::connect_ready(id, executable, installation.version.clone())
                .await?,
        ),
        "acp" => {
            let mut args = manifest.discovery.launch_args.clone();
            // Same reason as locally: no background installer beside a session.
            if manifest.id.as_str() == "cursor" {
                args.insert(0, "--disable-auto-update".into());
            }
            connected(
                acp_driver::AcpDriver::connect_ready(
                    manifest.id.as_str(),
                    id,
                    acp_driver::AcpLaunch { executable, args },
                    installation.version.clone(),
                )
                .await?,
            )
        }
        "antigravity_stream_json" => connected(
            antigravity_driver::AntigravityDriver::connect_ready(
                manifest.id.as_str(),
                id,
                executable,
                installation.version.clone(),
            )
            .await?,
        ),
        other => {
            return Err(driver_api::DriverError::CapabilityUnavailable(format!(
                "{other} cannot run on a target yet"
            )));
        }
    })
}

async fn record(core: &Arc<Core>, probe: RecordInstallationProbe) {
    let mut envelope = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(probe));
    envelope.actor = ActorRef::SystemRecovery;
    let _ = core.execute(envelope).await;
}

/// Installations with a conversation running right now. Replacing a driver under one
/// would cut it off, so a rescan leaves them exactly as they are.
async fn busy_installations(core: &Core) -> HashSet<String> {
    let Ok(snapshot) = core.bootstrap().await else {
        return HashSet::new();
    };
    snapshot
        .tasks
        .iter()
        .filter_map(|task| {
            task.active_primary_run
                .as_ref()?
                .executor
                .installation_id
                .clone()
        })
        .collect()
}

fn view_of(manifest: &ProviderManifest, installation: &InstallationSummary) -> TargetAgent {
    TargetAgent {
        provider_id: manifest.id.to_string(),
        display_name: manifest.display_name.clone(),
        executable: installation.executable.clone(),
        version: installation.version.clone(),
        status: installation.status,
        diagnostic: installation.diagnostic.clone(),
    }
}

async fn scan_one(
    core: &Arc<Core>,
    target_id: &TargetId,
    target: &SharedTarget,
    manifest: &ProviderManifest,
    probed: RecordInstallationProbe,
    busy: &HashSet<String>,
) -> Option<TargetAgent> {
    let executable = probed.executable.clone();
    let executable = executable.as_str();
    if let Some(installation) = installation_of(core, target_id, &manifest.id, executable).await
        && busy.contains(installation.installation_id.as_str())
    {
        return Some(view_of(manifest, &installation));
    }
    let mut probed = probed;
    probed.target_id = Some(target_id.clone());
    let runnable = !matches!(
        probed.status,
        InstallationStatus::Broken | InstallationStatus::UnsupportedVersion
    );
    record(core, probed.clone()).await;
    let mut view = TargetAgent {
        provider_id: manifest.id.to_string(),
        display_name: manifest.display_name.clone(),
        executable: executable.to_owned(),
        version: probed.version.clone(),
        status: probed.status,
        diagnostic: probed.diagnostic.clone(),
    };
    if !runnable {
        return Some(view);
    }
    let installation = installation_of(core, target_id, &manifest.id, executable).await?;
    let started = std::time::Instant::now();
    let executable = TargetExecutable::new(target.clone(), executable);
    // Its account can be read whether or not the agent turns out to be able to run.
    if let Some(monitor) = crate::provider_account_monitor(&installation, &executable, None) {
        core.register_target_quota_provider(target_id.clone(), monitor)
            .await;
    }
    let connected = connect_driver(manifest, &installation, executable.clone()).await;
    let (status, diagnostic) = match connected {
        Ok((driver, quota)) => {
            let executor = driver.executor().clone();
            core.register_target_driver(target_id.clone(), driver).await;
            match crate::provider_account_monitor(&installation, &executable, Some(quota.clone())) {
                Some(monitor) => core.register_quota_provider(monitor).await,
                None => core.register_quota_provider(quota).await,
            }
            core.warm_agent_catalogs(&executor).await;
            (InstallationStatus::Ready, None)
        }
        Err(error @ driver_api::DriverError::NotConfigured(_)) => (
            InstallationStatus::NotConfigured,
            Some(driver_api::failure::kind(&error).to_owned()),
        ),
        Err(error) => (InstallationStatus::Broken, Some(error.to_string())),
    };
    crate::record_readiness(
        core,
        &installation,
        status,
        diagnostic.clone(),
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    )
    .await;
    view.status = status;
    view.diagnostic = diagnostic;
    Some(view)
}

/// An installation that has gone from the machine is forgotten here too, but only on a
/// complete answer: a dropped link must not look like everything uninstalled. `None`
/// when the machine couldn't be asked at all.
pub(super) async fn scan(
    core: &Arc<Core>,
    target_id: &TargetId,
    target: &SharedTarget,
) -> Option<Vec<TargetAgent>> {
    let detected: Detected = operation::call(target, &Request::Detect).await.ok()?;
    let manifests: Vec<ProviderManifest> = BUILTIN_MANIFESTS
        .iter()
        .filter_map(|source| ProviderManifest::parse_toml(source).ok())
        .filter(|manifest| SUPPORTED_PROTOCOLS.contains(&manifest.protocol.kind.as_str()))
        .collect();
    let busy = busy_installations(core).await;
    let scans = detected.probes.into_iter().filter_map(|probed| {
        let manifest = manifests
            .iter()
            .find(|manifest| manifest.id.as_str() == probed.provider_id)?;
        Some(scan_one(core, target_id, target, manifest, probed, &busy))
    });
    let results = futures_util::future::join_all(scans).await;
    let complete = detected.complete && results.iter().all(Option::is_some);
    let agents: Vec<TargetAgent> = results.into_iter().flatten().collect();
    if complete {
        forget_missing(core, target_id, &agents, &busy).await;
    }
    Some(agents)
}

async fn forget_missing(
    core: &Arc<Core>,
    target_id: &TargetId,
    agents: &[TargetAgent],
    busy: &HashSet<String>,
) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations {
        let current = agents.iter().any(|agent| {
            agent.provider_id == installation.provider_id
                && agent.executable == installation.executable
        });
        if installation.target_id.as_ref() != Some(target_id)
            || current
            || busy.contains(installation.installation_id.as_str())
        {
            continue;
        }
        let mut envelope = CommandEnvelope::local(CoreCommand::ReconcileMissingInstallation(
            ReconcileMissingInstallation {
                installation_id: installation.installation_id,
                provider_id: installation.provider_id,
                executable: installation.executable,
            },
        ));
        envelope.actor = ActorRef::SystemRecovery;
        let _ = core.execute(envelope).await;
    }
}
