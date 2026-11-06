//! Finds which agents are installed on this machine, for the desktop asking from afar.
use agent_protocol::{InstallationStatus, RecordInstallationProbe};
use agent_registry::{
    BUILTIN_MANIFESTS, InstallationStatus as ProbeStatus, Platform, ProviderManifest,
    target_candidates, version_verdict,
};
use agent_runtime::{ProcessSpec, run_bounded};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

/// Which protocols can run on a target today. `ZCode` is packaged as a desktop
/// application, so it waits for a later stage.
pub const SUPPORTED_PROTOCOLS: [&str; 5] = [
    "app_server_json_rpc",
    "acp",
    "pi_rpc",
    "claude_stream_json",
    "antigravity_stream_json",
];
/// A slow disk or profile adds to whatever the manifest allows for a local start.
const EXTRA_TIME: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, Serialize)]
pub struct Detected {
    pub probes: Vec<RecordInstallationProbe>,
    /// Every agent found answered. Otherwise absence proves nothing about the others.
    pub complete: bool,
}

/// The manifests an agent on a server can run under, each with its possible locations.
#[must_use]
pub fn candidates(
    platform: Platform,
    path_entries: &[String],
    home: Option<&str>,
) -> Vec<(ProviderManifest, Vec<String>)> {
    BUILTIN_MANIFESTS
        .iter()
        .filter_map(|source| ProviderManifest::parse_toml(source).ok())
        .filter(|manifest| SUPPORTED_PROTOCOLS.contains(&manifest.protocol.kind.as_str()))
        .map(|manifest| {
            let found = target_candidates(&manifest, platform, path_entries, home);
            (manifest, found)
        })
        .collect()
}

fn is_executable(path: &str) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

async fn run(
    manifest: &ProviderManifest,
    executable: &str,
    args: &[String],
) -> Option<agent_runtime::ProcessOutput> {
    let mut spec = ProcessSpec::probe(PathBuf::from(executable), args.to_vec());
    spec.timeout = Duration::from_millis(manifest.discovery.timeout_millis) + EXTRA_TIME;
    spec.stdout_limit = manifest.discovery.output_limit_bytes;
    spec.stderr_limit = manifest.discovery.output_limit_bytes;
    spec.env_allowlist = ["PATH", "HOME", "LANG"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    run_bounded(&spec).await.ok()
}

async fn probe(manifest: &ProviderManifest, executable: &str) -> Option<RecordInstallationProbe> {
    let identity = if manifest.discovery.identity_args.is_empty() {
        String::new()
    } else {
        let identity = run(manifest, executable, &manifest.discovery.identity_args).await?;
        format!("{}{}", identity.stdout, identity.stderr)
    };
    let started = std::time::Instant::now();
    let version = run(manifest, executable, &manifest.discovery.version_args).await?;
    let (status, version_number, version_text, diagnostic) = version_verdict(
        manifest,
        &identity,
        version.success,
        version.exit_code,
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

/// Each provider gets its first location found, matching how a shell resolves a command.
pub async fn detect() -> Detected {
    let path_entries: Vec<String> = std::env::var("PATH")
        .map(|path| path.split(':').map(str::to_owned).collect())
        .unwrap_or_default();
    let home = std::env::var("HOME").ok();
    let manifests = candidates(Platform::current(), &path_entries, home.as_deref());
    let probes = manifests.iter().filter_map(|(manifest, found)| {
        let executable = found.iter().find(|path| is_executable(path))?;
        Some(probe(manifest, executable))
    });
    let results: Vec<_> = futures_join(probes).await;
    Detected {
        complete: results.iter().all(Option::is_some),
        probes: results.into_iter().flatten().collect(),
    }
}

/// Probing one after another keeps this free of another dependency, and each is quick
/// unless something is broken, which is bounded above.
async fn futures_join<F: Future>(work: impl Iterator<Item = F>) -> Vec<F::Output> {
    let mut results = Vec::new();
    for item in work {
        results.push(item.await);
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_manifests_yield_posix_candidates() {
        let found = candidates(
            Platform::Linux,
            &["/usr/local/bin".into(), "relative".into()],
            Some("/home/dev"),
        );
        let (_, kimi) = found
            .iter()
            .find(|(manifest, _)| manifest.id.as_str() == "kimi")
            .unwrap();
        assert!(kimi.iter().any(|path| path == "/usr/local/bin/kimi"));
        assert!(kimi.iter().all(|path| path.starts_with('/')));
        assert!(
            found
                .iter()
                .all(|(manifest, _)| manifest.id.as_str() != "zcode")
        );
    }
}
