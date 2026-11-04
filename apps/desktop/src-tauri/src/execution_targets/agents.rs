//! Finds agents on a connected machine and registers drivers for them. Discovery uses
//! the same manifests and version rules as this machine; only the place changes.
use agent_core::Core;
use agent_domain::{ProviderId, TargetId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, InstallationStatus, InstallationSummary,
    ReconcileMissingInstallation, RecordInstallationProbe,
};
use agent_registry::{
    InstallationStatus as ProbeStatus, Platform, ProviderManifest, target_candidates,
    version_verdict,
};
use agent_runtime::{Pipe, SharedTarget, TargetCommand, TargetExecutable};
use driver_api::AgentDriver;
use serde::Serialize;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;

/// A slow link adds to whatever the manifest allows for a local start.
const NETWORK_ALLOWANCE: Duration = Duration::from_secs(10);
/// Which protocols can run on a target today. Antigravity needs a local hook and
/// `ZCode` is packaged as a desktop application, so they wait for a later stage.
const SUPPORTED_PROTOCOLS: [&str; 4] =
    ["app_server_json_rpc", "acp", "pi_rpc", "claude_stream_json"];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetAgent {
    provider_id: String,
    display_name: String,
    executable: String,
    version: Option<String>,
    status: InstallationStatus,
    diagnostic: Option<String>,
}

fn platform_of(os: &str) -> Option<Platform> {
    match os {
        "linux" => Some(Platform::Linux),
        "macos" => Some(Platform::Macos),
        _ => None,
    }
}

struct Captured {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Bounded output and time: a broken CLI on the other end must not hold up the scan.
async fn capture(
    target: &SharedTarget,
    program: &str,
    args: &[String],
    limit: Duration,
    output_limit: usize,
) -> Option<Captured> {
    let mut command = TargetCommand::new(target.clone(), program);
    command
        .args(args)
        .inherit("PATH")
        .inherit("HOME")
        .inherit("LANG")
        .stdio(Pipe::Null, Pipe::Piped, Pipe::Piped);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let capped = u64::try_from(output_limit).unwrap_or(u64::MAX);
    let finished = tokio::time::timeout(limit, async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout = stdout.take(capped);
        let mut stderr = stderr.take(capped);
        let (read_out, read_err, status) = tokio::join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait(),
        );
        read_out.ok()?;
        read_err.ok()?;
        let status = status.ok()?;
        Some(Captured {
            success: status.success(),
            code: status.code(),
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    })
    .await;
    if let Ok(captured) = finished {
        captured
    } else {
        let _ = child.terminate().await;
        None
    }
}

/// One round trip for every candidate of every agent, rather than one per path.
async fn existing_executables(target: &SharedTarget, candidates: &[String]) -> HashSet<String> {
    if candidates.is_empty() {
        return HashSet::new();
    }
    let script = r#"for path in "$@"; do if [ -x "$path" ] && [ ! -d "$path" ]; then printf '%s\n' "$path"; fi; done"#;
    let mut args = vec!["-c".to_owned(), script.to_owned(), "sh".to_owned()];
    args.extend_from_slice(candidates);
    let Some(found) = capture(target, "/bin/sh", &args, NETWORK_ALLOWANCE, 1024 * 1024).await
    else {
        return HashSet::new();
    };
    found.stdout.lines().map(str::to_owned).collect()
}

async fn probe(
    target: &SharedTarget,
    manifest: &ProviderManifest,
    executable: &str,
) -> Option<RecordInstallationProbe> {
    let limit = Duration::from_millis(manifest.discovery.timeout_millis) + NETWORK_ALLOWANCE;
    let output_limit = manifest.discovery.output_limit_bytes;
    let identity = if manifest.discovery.identity_args.is_empty() {
        String::new()
    } else {
        let identity = capture(
            target,
            executable,
            &manifest.discovery.identity_args,
            limit,
            output_limit,
        )
        .await?;
        format!("{}{}", identity.stdout, identity.stderr)
    };
    let started = std::time::Instant::now();
    let version = capture(
        target,
        executable,
        &manifest.discovery.version_args,
        limit,
        output_limit,
    )
    .await?;
    let (status, version_number, version_text, diagnostic) = version_verdict(
        manifest,
        &identity,
        version.success,
        version.code,
        &version.stdout,
        &version.stderr,
    );
    Some(RecordInstallationProbe {
        target_id: None,
        provider_id: manifest.id.to_string(),
        executable: executable.to_owned(),
        // The local fingerprint hashes file metadata. Here the version identifies it.
        fingerprint: format!("target:{}", version_text.as_deref().unwrap_or("unknown")),
        status: match status {
            ProbeStatus::Installed => InstallationStatus::Installed,
            ProbeStatus::NotConfigured => InstallationStatus::NotConfigured,
            ProbeStatus::Ready => InstallationStatus::Ready,
            ProbeStatus::UnsupportedVersion => InstallationStatus::UnsupportedVersion,
            ProbeStatus::Broken => InstallationStatus::Broken,
        },
        version: version_number.map(|version| version.to_string()),
        version_text,
        diagnostic,
        elapsed_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
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

async fn connect_driver(
    manifest: &ProviderManifest,
    installation: &InstallationSummary,
    executable: TargetExecutable,
) -> Result<Arc<dyn AgentDriver>, driver_api::DriverError> {
    let id = installation.installation_id.as_str();
    let invalid = || driver_api::DriverError::Protocol("invalid installation".into());
    Ok(match manifest.protocol.kind.as_str() {
        "app_server_json_rpc" => Arc::new(
            codex_driver::CodexDriver::connect_ready(
                executable,
                id.to_owned(),
                installation.version.clone().ok_or_else(invalid)?,
            )
            .await?,
        ),
        "claude_stream_json" => Arc::new(
            claude_driver::ClaudeDriver::connect_ready(
                manifest.id.as_str(),
                id,
                executable,
                installation.version.clone(),
                driver_api::SessionResumeCapability::Supported,
            )
            .await?,
        ),
        "pi_rpc" => Arc::new(
            pi_driver::PiDriver::connect_ready(id, executable, installation.version.clone())
                .await?,
        ),
        "acp" => {
            let mut args = manifest.discovery.launch_args.clone();
            // Same reason as locally: no background installer beside a session.
            if manifest.id.as_str() == "cursor" {
                args.insert(0, "--disable-auto-update".into());
            }
            Arc::new(
                acp_driver::AcpDriver::connect_ready(
                    manifest.id.as_str(),
                    id,
                    acp_driver::AcpLaunch { executable, args },
                    installation.version.clone(),
                )
                .await?,
            )
        }
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

async fn scan_one(
    core: &Arc<Core>,
    target_id: &TargetId,
    target: &SharedTarget,
    manifest: &ProviderManifest,
    executable: &str,
) -> Option<TargetAgent> {
    let mut probed = probe(target, manifest, executable).await?;
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
    let connected = connect_driver(
        manifest,
        &installation,
        TargetExecutable::new(target.clone(), executable),
    )
    .await;
    let (status, diagnostic) = match connected {
        Ok(driver) => {
            let executor = driver.executor().clone();
            core.register_target_driver(target_id.clone(), driver).await;
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

/// Each provider gets its first location found, matching how a shell resolves a
/// command. An installation that has gone from the machine is forgotten here too.
pub(super) async fn scan(
    core: &Arc<Core>,
    target_id: &TargetId,
    target: &SharedTarget,
    os: &str,
) -> Vec<TargetAgent> {
    let Some(platform) = platform_of(os) else {
        return vec![];
    };
    let path_entries: Vec<String> = target
        .inherited("PATH")
        .and_then(|path| path.into_string().ok())
        .map(|path| path.split(':').map(str::to_owned).collect())
        .unwrap_or_default();
    let home = target
        .inherited("HOME")
        .and_then(|home| home.into_string().ok());
    let manifests: Vec<(ProviderManifest, Vec<String>)> = crate::BUILTIN_AGENT_MANIFESTS
        .iter()
        .filter_map(|source| ProviderManifest::parse_toml(source).ok())
        .filter(|manifest| SUPPORTED_PROTOCOLS.contains(&manifest.protocol.kind.as_str()))
        .map(|manifest| {
            let candidates = target_candidates(&manifest, platform, &path_entries, home.as_deref());
            (manifest, candidates)
        })
        .collect();
    let every_candidate: Vec<String> = manifests
        .iter()
        .flat_map(|(_, candidates)| candidates.iter().cloned())
        .collect();
    let existing = existing_executables(target, &every_candidate).await;
    let scans = manifests.iter().filter_map(|(manifest, candidates)| {
        let executable = candidates.iter().find(|path| existing.contains(*path))?;
        Some(scan_one(core, target_id, target, manifest, executable))
    });
    let agents: Vec<TargetAgent> = futures_util::future::join_all(scans)
        .await
        .into_iter()
        .flatten()
        .collect();
    forget_missing(core, target_id, &agents).await;
    agents
}

async fn forget_missing(core: &Arc<Core>, target_id: &TargetId, agents: &[TargetAgent]) {
    let Ok(snapshot) = core.bootstrap().await else {
        return;
    };
    for installation in snapshot.installations {
        let current = agents.iter().any(|agent| {
            agent.provider_id == installation.provider_id
                && agent.executable == installation.executable
        });
        if installation.target_id.as_ref() != Some(target_id) || current {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_posix_systems_are_scanned() {
        assert_eq!(platform_of("linux"), Some(Platform::Linux));
        assert_eq!(platform_of("macos"), Some(Platform::Macos));
        assert_eq!(platform_of("windows"), None);
    }

    #[test]
    fn manifests_for_supported_protocols_yield_posix_candidates() {
        let manifest = ProviderManifest::parse_toml(include_str!(
            "../../../../../drivers/manifests/kimi.toml"
        ))
        .unwrap();
        assert!(SUPPORTED_PROTOCOLS.contains(&manifest.protocol.kind.as_str()));
        let candidates = target_candidates(
            &manifest,
            Platform::Linux,
            &["/usr/local/bin".into(), "relative".into()],
            Some("/root"),
        );
        assert_eq!(
            candidates,
            ["/usr/local/bin/kimi", "/root/.kimi-code/bin/kimi"]
        );
    }
}
