//! Finds which agents are installed on this machine, for the desktop asking from afar.
use agent_protocol::{InstallationStatus, RecordInstallationProbe};
use agent_registry::{
    BUILTIN_MANIFESTS, InstallationStatus as ProbeStatus, Platform, ProviderManifest,
    target_candidates, version_verdict,
};
use agent_runtime::{ProcessError, ProcessOutput, ProcessSpec, run_bounded};
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

/// Safe display messages for failed installations; command output stays on the server.
#[must_use]
pub fn installation_failure(
    status: InstallationStatus,
    diagnostic: Option<&str>,
) -> Option<&'static str> {
    match status {
        InstallationStatus::UnsupportedVersion => Some("backendText.installVersionUnsupported"),
        InstallationStatus::Broken => Some(match diagnostic {
            Some("agent_runtime_incompatible") => "backendText.installRuntimeIncompatible",
            Some("agent_probe_timeout") => "backendText.installProbeTimedOut",
            _ => "backendText.installCommandFailed",
        }),
        _ => None,
    }
}

fn command_diagnostic(output: &ProcessOutput) -> &'static str {
    let text = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    if [
        "glibc_",
        "glibcxx_",
        "cxxabi_",
        "libstdc++",
        "error while loading shared libraries",
    ]
    .iter()
    .any(|marker| text.contains(marker))
        && ["not found", "cannot open shared object", "version mismatch"]
            .iter()
            .any(|marker| text.contains(marker))
    {
        "agent_runtime_incompatible"
    } else {
        "agent_command_failed"
    }
}

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
) -> Result<ProcessOutput, ProcessError> {
    let mut spec = ProcessSpec::probe(PathBuf::from(executable), args.to_vec());
    spec.timeout = Duration::from_millis(manifest.discovery.timeout_millis) + EXTRA_TIME;
    spec.stdout_limit = manifest.discovery.output_limit_bytes;
    spec.stderr_limit = manifest.discovery.output_limit_bytes;
    spec.env_allowlist = ["PATH", "HOME", "LANG"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    run_bounded(&spec).await
}

async fn probe(manifest: &ProviderManifest, executable: &str) -> (RecordInstallationProbe, bool) {
    let started = std::time::Instant::now();
    let mut record = RecordInstallationProbe {
        target_id: None,
        provider_id: manifest.id.to_string(),
        executable: executable.to_owned(),
        // The local fingerprint hashes file metadata. Here the version identifies it.
        fingerprint: "target:unknown".into(),
        status: InstallationStatus::Broken,
        version: None,
        version_text: None,
        diagnostic: None,
        elapsed_millis: 0,
    };
    let inspected = async {
        let identity = if manifest.discovery.identity_args.is_empty() {
            String::new()
        } else {
            let identity = run(manifest, executable, &manifest.discovery.identity_args).await?;
            if !identity.success {
                record.diagnostic = Some(command_diagnostic(&identity).into());
                return Ok::<_, ProcessError>(());
            }
            format!("{}{}", identity.stdout, identity.stderr)
        };
        let version = run(manifest, executable, &manifest.discovery.version_args).await?;
        let (status, version_number, version_text, diagnostic) = version_verdict(
            manifest,
            &identity,
            version.success,
            version.exit_code,
            &version.stdout,
            &version.stderr,
        );
        record.status = match status {
            ProbeStatus::Installed => InstallationStatus::Installed,
            ProbeStatus::NotConfigured => InstallationStatus::NotConfigured,
            ProbeStatus::Ready => InstallationStatus::Ready,
            ProbeStatus::UnsupportedVersion => InstallationStatus::UnsupportedVersion,
            ProbeStatus::Broken => InstallationStatus::Broken,
        };
        record.fingerprint = format!("target:{}", version_text.as_deref().unwrap_or("unknown"));
        record.version = version_number.map(|version| version.to_string());
        record.version_text = version_text;
        record.diagnostic = if version.success {
            diagnostic
        } else {
            Some(command_diagnostic(&version).into())
        };
        Ok(())
    }
    .await;
    if let Err(error) = &inspected {
        record.diagnostic = Some(
            match error {
                ProcessError::TimedOut { .. } => "agent_probe_timeout",
                _ => "agent_command_failed",
            }
            .into(),
        );
    }
    record.elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    (record, inspected.is_ok())
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
        complete: results.iter().all(|(_, answered)| *answered),
        probes: results.into_iter().map(|(record, _)| record).collect(),
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
    fn failed_commands_report_system_incompatibility_without_exposing_output() {
        for stderr in [
            "kimi: /lib64/libc.so.6: version `GLIBC_2.28' not found",
            "cursor-agent: version `GLIBCXX_3.4.21' not found",
            "cursor-agent: version `CXXABI_1.3.11' not found",
            "error while loading shared libraries: libstdc++.so.6: cannot open shared object file",
            "private-token: command failed",
        ] {
            let output = ProcessOutput {
                exit_code: Some(1),
                success: false,
                stdout: String::new(),
                stderr: stderr.into(),
                stdout_truncated: false,
                stderr_truncated: false,
                elapsed_millis: 1,
            };
            let code = command_diagnostic(&output);
            assert_eq!(
                code == "agent_runtime_incompatible",
                !stderr.contains("private-token")
            );
            let message = installation_failure(InstallationStatus::Broken, Some(code)).unwrap();
            assert!(!message.contains("private-token"));
        }
        for status in [
            InstallationStatus::Installed,
            InstallationStatus::Ready,
            InstallationStatus::NotConfigured,
        ] {
            assert_eq!(
                installation_failure(status, Some("authentication_failed")),
                None
            );
        }
        assert!(installation_failure(InstallationStatus::UnsupportedVersion, None).is_some());
    }

    #[tokio::test]
    async fn a_failed_spawn_is_recorded_instead_of_leaving_a_stale_installation() {
        let directory = tempfile::tempdir().unwrap();
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/kimi.toml"))
                .unwrap();
        let executable = directory.path().join("missing.exe");
        let (record, answered) = probe(&manifest, executable.to_str().unwrap()).await;
        assert!(!answered);
        assert_eq!(record.status, InstallationStatus::Broken);
        assert_eq!(record.version, None);
        assert_eq!(record.diagnostic.as_deref(), Some("agent_command_failed"));
    }

    #[tokio::test]
    async fn identity_and_version_commands_must_both_run() {
        let directory = tempfile::tempdir().unwrap();
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/kimi.toml"))
                .unwrap();
        for (name, identity_code, version_code) in
            [("identity", 1, 0), ("version", 0, 1), ("working", 0, 0)]
        {
            let path = directory.path().join(if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.into()
            });
            let failure = "version GLIBC_2.28 not found";
            let identity = if identity_code == 0 {
                "kimi-code as an agent client protocol"
            } else {
                failure
            };
            let version = if version_code == 0 { "2.5.0" } else { failure };
            let script = if cfg!(windows) {
                format!(
                    "@echo off\r\nif \"%1\"==\"acp\" goto identity\r\necho {version}\r\nexit /b {version_code}\r\n:identity\r\necho {identity}\r\nexit /b {identity_code}\r\n"
                )
            } else {
                format!(
                    "#!/bin/sh\nif [ \"$1\" = acp ]; then echo '{identity}'; exit {identity_code}; fi\necho '{version}'; exit {version_code}\n"
                )
            };
            std::fs::write(&path, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let (record, answered) = probe(&manifest, path.to_str().unwrap()).await;
            assert!(answered);
            if name == "working" {
                assert_eq!(record.status, InstallationStatus::Installed);
                assert_eq!(record.version.as_deref(), Some("2.5.0"));
                assert_eq!(record.diagnostic, None);
            } else {
                assert_eq!(record.status, InstallationStatus::Broken);
                assert_eq!(record.version, None);
                assert_eq!(
                    record.diagnostic.as_deref(),
                    Some("agent_runtime_incompatible")
                );
            }
        }
    }

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
