//! Manifest-driven discovery and bounded, credential-free installation probes.

mod windows;
pub use windows::windows_desktop_executables;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use agent_domain::ProviderId;
use agent_runtime::{ProcessError, ProcessSpec, run_scoped_bounded};
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MANIFEST_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderManifest {
    pub schema_version: u16,
    pub id: ProviderId,
    pub display_name: String,
    pub homepage: String,
    pub runtime_kind: RuntimeKind,
    pub discovery: DiscoverySpec,
    pub supported_versions: SupportedVersions,
    pub protocol: ProtocolSpec,
    #[serde(default)]
    pub capability_expectations: BTreeMap<String, String>,
}

impl ProviderManifest {
    pub fn parse_toml(source: &str) -> Result<Self, RegistryError> {
        let manifest: Self = toml::from_str(source)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(RegistryError::InvalidManifest(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        if self.display_name.trim().is_empty() {
            return Err(RegistryError::InvalidManifest(
                "displayName must not be empty".into(),
            ));
        }
        if !self.homepage.starts_with("https://") {
            return Err(RegistryError::InvalidManifest(
                "homepage must use HTTPS".into(),
            ));
        }
        match self.runtime_kind {
            RuntimeKind::InProcess if !self.discovery.candidate_executables.is_empty() => {
                return Err(RegistryError::InvalidManifest(
                    "in-process providers cannot declare executable candidates".into(),
                ));
            }
            RuntimeKind::ExternalProcess if self.discovery.candidate_executables.is_empty() => {
                return Err(RegistryError::InvalidManifest(
                    "external providers require at least one executable candidate".into(),
                ));
            }
            RuntimeKind::ExternalProcess if self.discovery.version_args.is_empty() => {
                return Err(RegistryError::InvalidManifest(
                    "external providers require structured version arguments".into(),
                ));
            }
            RuntimeKind::ExternalProcess if self.discovery.version_output_markers.is_empty() => {
                return Err(RegistryError::InvalidManifest(
                    "external providers require at least one version output marker".into(),
                ));
            }
            _ => {}
        }
        for candidate in &self.discovery.candidate_executables {
            if candidate.is_empty() || Path::new(candidate).components().count() != 1 {
                return Err(RegistryError::InvalidManifest(format!(
                    "executable candidate {candidate:?} must be a bare filename"
                )));
            }
        }
        for (platform, paths) in &self.discovery.platform_paths {
            for path in paths {
                if !platform.absolute_path(path) {
                    return Err(RegistryError::InvalidManifest(format!(
                        "platform path {} must be absolute",
                        path.display()
                    )));
                }
            }
        }
        if let Some(script) = &self.discovery.bundled_script
            && (script.as_os_str().is_empty()
                || script
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || self
                    .discovery
                    .env_overrides
                    .get("ELECTRON_RUN_AS_NODE")
                    .map(String::as_str)
                    != Some("1"))
        {
            return Err(RegistryError::InvalidManifest(
                "bundled script must be a resource-relative path run by Electron in Node mode"
                    .into(),
            ));
        }
        for path in self.discovery.user_paths.values().flatten() {
            if path.is_absolute()
                || path.components().any(|component| {
                    component == std::path::Component::ParentDir
                        || component == std::path::Component::RootDir
                })
            {
                return Err(RegistryError::InvalidManifest(format!(
                    "user path {} must be relative to the home directory without `..`",
                    path.display()
                )));
            }
        }
        for (name, launch_argument) in self
            .discovery
            .requires_executables
            .iter()
            .map(|name| (name, false))
            .chain(self.discovery.launch_args.iter().map(|name| (name, true)))
        {
            // CodeBuddy selects the native protocol with --acp, not an acp
            // subcommand. Do not admit arbitrary permission-changing flags.
            let protocol_flag = launch_argument
                && (name == "--acp"
                    || match self.id.as_str() {
                        "pi" => self.discovery.launch_args == ["--mode", "rpc"],
                        "deepseek-harness" => self.discovery.launch_args == ["--profile", "acp"],
                        "kiro" => {
                            self.discovery.launch_args
                                == ["acp", "--agent-engine", "v3", "--auth-method", "cli"]
                        }
                        _ => false,
                    });
            if name.is_empty()
                || name.chars().any(char::is_control)
                || (name.starts_with('-') && !protocol_flag)
            {
                return Err(RegistryError::InvalidManifest(format!(
                    "required executable or launch argument {name:?} must be a plain non-flag token"
                )));
            }
        }
        for name in self.discovery.env_overrides.keys() {
            if name.is_empty() || name.chars().any(char::is_control) || name.starts_with('-') {
                return Err(RegistryError::InvalidManifest(format!(
                    "discovery env override {name:?} must be a plain non-flag token"
                )));
            }
        }
        for name in &self.discovery.requires_executables {
            if Path::new(name).components().count() != 1 {
                return Err(RegistryError::InvalidManifest(format!(
                    "required executable {name:?} must be a bare filename"
                )));
            }
        }
        for marker in &self.discovery.version_output_markers {
            if marker.trim().is_empty() || marker.len() > 64 || marker.chars().any(char::is_control)
            {
                return Err(RegistryError::InvalidManifest(
                    "version output markers must contain 1 to 64 printable characters".into(),
                ));
            }
        }
        let probe_spec = ProcessSpec {
            executable: PathBuf::from(if cfg!(windows) {
                r"C:\Windows\System32\cmd.exe"
            } else {
                "/usr/bin/true"
            }),
            args: vec![],
            env_allowlist: self.discovery.env_allowlist.clone(),
            env_overrides: BTreeMap::default(),
            cwd: None,
            timeout: Duration::from_millis(self.discovery.timeout_millis),
            stdout_limit: self.discovery.output_limit_bytes,
            stderr_limit: self.discovery.output_limit_bytes,
        };
        probe_spec
            .validate()
            .map_err(|error| RegistryError::InvalidManifest(error.to_string()))?;
        if let (Some(min), Some(recommended)) = (
            &self.supported_versions.min,
            &self.supported_versions.recommended,
        ) && recommended < min
        {
            return Err(RegistryError::InvalidManifest(
                "recommended version cannot be lower than min".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    InProcess,
    ExternalProcess,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverySpec {
    #[serde(default)]
    pub candidate_executables: Vec<String>,
    #[serde(default)]
    pub platform_paths: BTreeMap<Platform, Vec<PathBuf>>,
    /// Node entry point relative to the discovered Electron resource directory.
    #[serde(default)]
    pub bundled_script: Option<PathBuf>,
    #[serde(default)]
    pub version_args: Vec<String>,
    /// Optional additional structured argv probe whose output must carry the
    /// version output markers. Use when the version command alone cannot prove
    /// product identity (e.g. a bare `0.40.1`).
    #[serde(default)]
    pub identity_args: Vec<String>,
    /// Home-relative discovery paths (no leading separator, no `..`), joined to
    /// the user's home directory. Used for agents that install outside PATH.
    #[serde(default)]
    pub user_paths: BTreeMap<Platform, Vec<PathBuf>>,
    /// Extra environment variables forced into version/identity probes (for
    /// example `ELECTRON_RUN_AS_NODE` to run an app-bundled CLI through its
    /// own embedded runtime). Names still pass secret-marker validation.
    #[serde(default)]
    pub env_overrides: BTreeMap<String, String>,
    /// Bare executable names that must also be discoverable for the provider to
    /// be usable (e.g. an ACP ecosystem adapter next to the agent CLI).
    #[serde(default)]
    pub requires_executables: Vec<String>,
    /// Structured launch arguments handed to the driver integration (e.g. an
    /// ACP server mode). Never executed by the version probe itself.
    #[serde(default)]
    pub launch_args: Vec<String>,
    #[serde(default)]
    pub version_output_markers: BTreeSet<String>,
    #[serde(default)]
    pub env_allowlist: BTreeSet<String>,
    #[serde(default = "default_timeout_millis")]
    pub timeout_millis: u64,
    #[serde(default = "default_output_limit")]
    pub output_limit_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SupportedVersions {
    pub min: Option<Version>,
    pub recommended: Option<Version>,
    #[serde(default)]
    pub denylist: BTreeSet<Version>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolSpec {
    pub kind: String,
    pub version: String,
    pub framing: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    fn absolute_path(self, path: &Path) -> bool {
        let Some(path) = path.to_str() else {
            return false;
        };
        match self {
            Self::Macos | Self::Linux => path.starts_with('/'),
            Self::Windows => {
                let bytes = path.as_bytes();
                (bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'/' | b'\\'))
                    || path
                        .strip_prefix("\\\\")
                        .or_else(|| path.strip_prefix("//"))
                        .is_some_and(|tail| {
                            tail.split(['/', '\\'])
                                .filter(|part| !part.is_empty())
                                .count()
                                >= 2
                        })
            }
        }
    }

    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveryContext {
    pub platform: Platform,
    pub path_entries: Vec<PathBuf>,
    /// Home directory used to resolve home-relative manifest `user_paths`.
    pub home_dir: Option<PathBuf>,
}

impl DiscoveryContext {
    #[must_use]
    pub fn from_current_process() -> Self {
        Self {
            platform: Platform::current(),
            // Empty PATH entries (":/bin") resolve against the current working
            // directory, so a repo-local executable could hijack discovery;
            // only absolute directories are eligible.
            path_entries: agent_runtime::current_path()
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten()
                .filter(|entry| entry.is_absolute())
                .collect(),
            home_dir: home_directory(),
        }
    }
}

/// Resolves the current user's home directory without extra dependencies.
#[must_use]
pub fn home_directory() -> Option<PathBuf> {
    #[cfg(unix)]
    return std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    #[cfg(windows)]
    return std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationStatus {
    Installed,
    NotConfigured,
    Ready,
    UnsupportedVersion,
    Broken,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationProbe {
    pub provider_id: ProviderId,
    pub executable: PathBuf,
    pub fingerprint: String,
    pub status: InstallationStatus,
    pub version: Option<Version>,
    pub version_text: Option<String>,
    pub diagnostic: Option<String>,
    pub elapsed_millis: u64,
}

pub async fn discover_and_probe(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Result<Vec<InstallationProbe>, RegistryError> {
    manifest.validate()?;
    if manifest.runtime_kind == RuntimeKind::InProcess {
        return Ok(vec![]);
    }
    let executables = discover_executables(manifest, context)?;
    let mut reports = Vec::with_capacity(executables.len());
    for executable in executables {
        reports.push(probe_installation(manifest, executable).await?);
    }
    Ok(reports)
}

pub fn discover_executables(
    manifest: &ProviderManifest,
    context: &DiscoveryContext,
) -> Result<Vec<PathBuf>, RegistryError> {
    manifest.validate()?;
    let mut discovered = BTreeSet::new();
    for directory in &context.path_entries {
        // Defense in depth: a relative or empty directory would resolve the
        // candidate against the launcher's CWD and canonicalize it into an
        // absolute path, defeating the absolute-executable check downstream.
        if !directory.is_absolute() {
            continue;
        }
        for candidate in &manifest.discovery.candidate_executables {
            include_executable(&directory.join(candidate), &mut discovered)?;
            #[cfg(windows)]
            include_executable(&directory.join(format!("{candidate}.exe")), &mut discovered)?;
        }
    }
    if let Some(paths) = manifest.discovery.platform_paths.get(&context.platform) {
        for path in paths {
            include_executable(path, &mut discovered)?;
        }
    }
    if let Some(home) = context.home_dir.as_deref()
        && let Some(paths) = manifest.discovery.user_paths.get(&context.platform)
    {
        for relative in paths {
            include_executable(&home.join(relative), &mut discovered)?;
        }
    }
    for path in windows_desktop_executables(manifest.id.as_str(), context) {
        include_executable(&path, &mut discovered)?;
    }
    // GUI launches often omit shell-managed Node bins from PATH.
    if let Some(home) = &context.home_dir {
        for root in [".nvm/versions/node", ".nvm-arm/versions/node"] {
            if let Ok(versions) = std::fs::read_dir(home.join(root)) {
                for version in versions.flatten().take(64) {
                    let bin = version.path().join("bin");
                    for candidate in &manifest.discovery.candidate_executables {
                        include_executable(&bin.join(candidate), &mut discovered)?;
                    }
                }
            }
        }
    }
    Ok(discovered.into_iter().collect())
}

pub async fn probe_installation(
    manifest: &ProviderManifest,
    executable: PathBuf,
) -> Result<InstallationProbe, RegistryError> {
    let metadata = std::fs::metadata(&executable)?;
    let fingerprint = executable_fingerprint(&executable, &metadata);
    let timeout_millis = installation_probe_timeout(manifest.discovery.timeout_millis, &executable);
    let spec = ProcessSpec {
        executable: executable.clone(),
        args: probe_args(manifest, &executable, &manifest.discovery.version_args)?,
        env_allowlist: manifest.discovery.env_allowlist.clone(),
        env_overrides: manifest
            .discovery
            .env_overrides
            .iter()
            .map(|(name, value)| (name.clone(), std::ffi::OsString::from(value)))
            .collect(),
        cwd: None,
        timeout: Duration::from_millis(timeout_millis),
        stdout_limit: manifest.discovery.output_limit_bytes,
        stderr_limit: manifest.discovery.output_limit_bytes,
    };
    // Product identity and semantic version may need different probe surfaces;
    // markers are matched against the identity output joined with the version
    // output, while the version itself is parsed from the version command only.
    let identity_text = if manifest.discovery.identity_args.is_empty() {
        String::new()
    } else {
        let identity_spec = ProcessSpec {
            executable: executable.clone(),
            args: probe_args(manifest, &executable, &manifest.discovery.identity_args)?,
            env_allowlist: manifest.discovery.env_allowlist.clone(),
            env_overrides: manifest
                .discovery
                .env_overrides
                .iter()
                .map(|(name, value)| (name.clone(), std::ffi::OsString::from(value)))
                .collect(),
            cwd: None,
            timeout: Duration::from_millis(timeout_millis),
            stdout_limit: manifest.discovery.output_limit_bytes,
            stderr_limit: manifest.discovery.output_limit_bytes,
        };
        match run_scoped_bounded(&identity_spec).await {
            // Marker output on stdout proves product identity even when the
            // CLI exits non-zero (e.g. `agy models` warns on stderr and
            // returns 1 while listing Gemini models correctly).
            Ok(identity_output)
                if identity_output.success
                    || !identity_output.stdout.trim().is_empty()
                    || !identity_output.stderr.trim().is_empty() =>
            {
                let mut combined = identity_output.stdout;
                combined.push_str(&identity_output.stderr);
                combined
            }
            Ok(identity_output) => {
                return Ok(InstallationProbe {
                    provider_id: manifest.id.clone(),
                    executable,
                    fingerprint,
                    status: InstallationStatus::Broken,
                    version: None,
                    version_text: None,
                    diagnostic: Some(format!(
                        "identity probe exited with code {:?}",
                        identity_output.exit_code
                    )),
                    elapsed_millis: identity_output.elapsed_millis,
                });
            }
            Err(error) => {
                return Ok(InstallationProbe {
                    provider_id: manifest.id.clone(),
                    executable,
                    fingerprint,
                    status: InstallationStatus::Broken,
                    version: None,
                    version_text: None,
                    diagnostic: Some(safe_process_diagnostic(&error)),
                    elapsed_millis: timeout_millis,
                });
            }
        }
    };
    match run_scoped_bounded(&spec).await {
        Ok(output) => {
            let raw_version_text = first_nonempty(&output.stdout, &output.stderr);
            let signature_matches = raw_version_text.as_deref().is_some_and(|text| {
                let mut combined = identity_text.clone();
                combined.push_str(text);
                version_output_matches(&combined, &manifest.discovery.version_output_markers)
            });
            let version_text = raw_version_text.filter(|_| signature_matches);
            let version = version_text.as_deref().and_then(extract_version);
            let status = if !output.success || !signature_matches || version.is_none() {
                InstallationStatus::Broken
            } else if version
                .as_ref()
                .is_some_and(|version| !version_supported(version, &manifest.supported_versions))
            {
                InstallationStatus::UnsupportedVersion
            } else {
                // Authentication is deliberately not inferred from `--version`.
                InstallationStatus::Installed
            };
            let diagnostic = if !output.success {
                Some(format!(
                    "version probe exited with code {:?}",
                    output.exit_code
                ))
            } else if !signature_matches {
                Some("version probe output did not match the provider signature".into())
            } else if version.is_none() {
                Some("version probe did not report a semantic version".into())
            } else {
                None
            };
            Ok(InstallationProbe {
                provider_id: manifest.id.clone(),
                executable,
                fingerprint,
                status,
                version,
                version_text,
                diagnostic,
                elapsed_millis: output.elapsed_millis,
            })
        }
        Err(error) => Ok(InstallationProbe {
            provider_id: manifest.id.clone(),
            executable,
            fingerprint,
            status: InstallationStatus::Broken,
            version: None,
            version_text: None,
            diagnostic: Some(safe_process_diagnostic(&error)),
            elapsed_millis: timeout_millis,
        }),
    }
}

fn installation_probe_timeout(requested: u64, executable: &Path) -> u64 {
    // Native Windows launchers can start cmd, PowerShell and a bundled Node
    // runtime before answering --version. Keep those cold starts bounded while
    // leaving native executables and other platforms on their declared budget.
    #[cfg(windows)]
    if requested > 0
        && executable
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("cmd") || value.eq_ignore_ascii_case("bat")
            })
    {
        return requested.max(8000);
    }
    #[cfg(not(windows))]
    let _ = executable;
    requested
}

fn probe_args(
    manifest: &ProviderManifest,
    executable: &Path,
    args: &[String],
) -> Result<Vec<String>, RegistryError> {
    let mut resolved = Vec::new();
    if let Some(script) = &manifest.discovery.bundled_script {
        let resources =
            agent_runtime::electron_resources_directory(executable).ok_or_else(|| {
                RegistryError::InvalidManifest("Electron resource directory unavailable".into())
            })?;
        resolved.push(resources.join(script).to_string_lossy().into_owned());
    }
    resolved.extend_from_slice(args);
    Ok(resolved)
}

/// Returns the cache key for an installation's current executable image.
///
/// # Errors
///
/// Returns an I/O error when the executable cannot be inspected or canonicalized.
pub fn installation_fingerprint(path: &Path) -> Result<String, RegistryError> {
    let canonical = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&canonical)?;
    Ok(executable_fingerprint(&canonical, &metadata))
}

fn include_executable(
    path: &Path,
    discovered: &mut BTreeSet<PathBuf>,
) -> Result<(), RegistryError> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if !metadata.is_file() || !is_executable(&metadata) {
        return Ok(());
    }
    discovered.insert(std::fs::canonicalize(path)?);
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

fn executable_fingerprint(path: &Path, metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    format!("{}:{}:{modified}", path.display(), metadata.len())
}

fn first_nonempty(stdout: &str, stderr: &str) -> Option<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(512).collect())
}

fn extract_version(text: &str) -> Option<Version> {
    text.split_whitespace().find_map(|token| {
        let token = token.trim_matches(|character: char| {
            !(character.is_ascii_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '+')
        });
        let token = token.strip_prefix('v').unwrap_or(token);
        Version::parse(token)
            .ok()
            .or_else(|| parse_version_with_leading_zeros(token))
    })
}

/// Some vendors publish calendar versions with zero-padded numeric parts
/// (for example `2026.09.02-c22c1a3`), which strict semver rejects. Strip
/// leading zeros from the numeric core only; build metadata is untouched.
fn parse_version_with_leading_zeros(token: &str) -> Option<Version> {
    // Split the numeric core from the pre-release/build suffix.
    let core_end = token.find(['-', '+']).unwrap_or(token.len());
    let (core, suffix) = token.split_at(core_end);
    let normalized = core
        .split('.')
        .map(|part| part.trim_start_matches('0'))
        .map(|part| if part.is_empty() { "0" } else { part })
        .collect::<Vec<_>>()
        .join(".");
    Version::parse(&format!("{normalized}{suffix}")).ok()
}

fn version_output_matches(text: &str, markers: &BTreeSet<String>) -> bool {
    let text = text.to_ascii_lowercase();
    markers
        .iter()
        .any(|marker| text.contains(&marker.to_ascii_lowercase()))
}

fn version_supported(version: &Version, policy: &SupportedVersions) -> bool {
    !policy.denylist.contains(version)
        && policy.min.as_ref().is_none_or(|minimum| version >= minimum)
}

fn safe_process_diagnostic(error: &ProcessError) -> String {
    match error {
        ProcessError::TimedOut { timeout_millis } => {
            format!("version probe timed out after {timeout_millis}ms")
        }
        ProcessError::Spawn(_) => "version probe could not be started".into(),
        ProcessError::Io(_) => "version probe I/O failed".into(),
        ProcessError::InvalidSpec(message) => format!("invalid version probe: {message}"),
    }
}

const fn default_timeout_millis() -> u64 {
    3_000
}

const fn default_output_limit() -> usize {
    64 * 1_024
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("invalid provider manifest: {0}")]
    InvalidManifest(String),
    #[error("manifest parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("installation discovery I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODEX_MANIFEST: &str = include_str!("../../../drivers/manifests/codex.toml");

    #[test]
    #[cfg(windows)]
    fn batch_launcher_startup_budget_does_not_change_native_or_longer_probe_limits() {
        assert_eq!(
            installation_probe_timeout(3000, Path::new("cursor-agent.CMD")),
            8000
        );
        assert_eq!(
            installation_probe_timeout(3000, Path::new("kimi.exe")),
            3000
        );
        assert_eq!(
            installation_probe_timeout(12000, Path::new("cursor-agent.cmd")),
            12000
        );
        assert_eq!(installation_probe_timeout(0, Path::new("invalid.cmd")), 0);
    }

    #[test]
    fn macos_user_installations_follow_a_relocated_unicode_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("外部磁盘/用户资料/中文 用户");
        for (source, relative) in [
            (
                CODEX_MANIFEST,
                "Applications/Codex.app/Contents/Resources/codex",
            ),
            (
                include_str!("../../../drivers/manifests/claude-code.toml"),
                ".local/bin/claude",
            ),
            (
                include_str!("../../../drivers/manifests/grok.toml"),
                ".grok/bin/grok",
            ),
            (
                include_str!("../../../drivers/manifests/zcode.toml"),
                "Applications/ZCode.app/Contents/MacOS/ZCode",
            ),
        ] {
            let path = home.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, []).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mut manifest = ProviderManifest::parse_toml(source).unwrap();
            manifest.discovery.platform_paths.clear();
            let context = DiscoveryContext {
                platform: Platform::Macos,
                path_entries: vec![],
                home_dir: Some(home.clone()),
            };
            assert_eq!(
                discover_executables(&manifest, &context).unwrap(),
                vec![std::fs::canonicalize(path).unwrap()],
                "{}",
                manifest.id
            );
        }
    }

    #[test]
    fn bundled_zcode_probes_follow_the_discovered_app_on_windows_and_macos() {
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/zcode.toml"))
                .unwrap();
        let root = tempfile::tempdir().unwrap();
        for (binary, script) in [
            (
                "Programs/ZCode/ZCode.exe",
                "Programs/ZCode/resources/glm/zcode.cjs",
            ),
            (
                "Applications/ZCode.app/Contents/MacOS/ZCode",
                "Applications/ZCode.app/Contents/Resources/glm/zcode.cjs",
            ),
        ] {
            let args = probe_args(
                &manifest,
                &root.path().join(binary),
                &manifest.discovery.version_args,
            )
            .unwrap();
            assert_eq!(Path::new(&args[0]), root.path().join(script));
            assert_eq!(&args[1..], &["--version"]);
        }
        assert!(version_output_matches(
            "zcode doctor\nversion: 0.17.0",
            &manifest.discovery.version_output_markers
        ));
        assert!(version_output_matches(
            "zcode-cli",
            &manifest.discovery.version_output_markers
        ));
        assert!(!version_output_matches(
            "unrelated 0.17.0",
            &manifest.discovery.version_output_markers
        ));
    }

    #[test]
    fn bundled_script_cannot_escape_resources_or_launch_without_node_mode() {
        let mut manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/zcode.toml"))
                .unwrap();
        for script in ["", "../unrelated.js", "/unrelated.js"] {
            manifest.discovery.bundled_script = Some(script.into());
            assert!(manifest.validate().is_err());
        }
        manifest.discovery.bundled_script = Some("glm/zcode.cjs".into());
        manifest.discovery.env_overrides.clear();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_paths_use_the_declared_platforms_rules() {
        for platform in [Platform::Macos, Platform::Linux] {
            assert!(platform.absolute_path(Path::new("/usr/local/bin/agent")));
            assert!(!platform.absolute_path(Path::new("bin/agent")));
            assert!(!platform.absolute_path(Path::new(r"C:\Tools\agent.exe")));
        }
        for path in [
            r"C:\Tools\agent.exe",
            r"D:\用户资料\中文 用户\.grok\bin\grok.exe",
            "C:/Tools/agent.exe",
            r"\\server\share\agent.exe",
        ] {
            assert!(Platform::Windows.absolute_path(Path::new(path)));
        }
        for path in [
            "/usr/bin/agent",
            r"\Tools\agent.exe",
            "C:agent.exe",
            "agent.exe",
        ] {
            assert!(!Platform::Windows.absolute_path(Path::new(path)));
        }
    }

    #[test]
    fn parses_and_validates_the_codex_manifest() {
        let manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        assert_eq!(manifest.id.as_str(), "codex");
        assert_eq!(manifest.protocol.kind, "app_server_json_rpc");
        let macos_paths = manifest
            .discovery
            .platform_paths
            .get(&Platform::Macos)
            .unwrap();
        assert!(macos_paths.iter().any(|path| {
            path == Path::new("/Applications/ChatGPT.app/Contents/Resources/codex")
        }));
        assert!(
            macos_paths.iter().any(|path| {
                path == Path::new("/Applications/Codex.app/Contents/Resources/codex")
            })
        );
    }

    #[test]
    fn rejects_secret_environment_allowlist_entries() {
        let source = CODEX_MANIFEST.replace(
            "env_allowlist = [\"PATH\"]",
            "env_allowlist = [\"PATH\", \"OPENAI_API_KEY\"]",
        );
        assert!(matches!(
            ProviderManifest::parse_toml(&source),
            Err(RegistryError::InvalidManifest(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ignores_empty_and_relative_path_entries_to_prevent_cwd_hijacking() {
        use std::os::unix::fs::PermissionsExt;

        let mut manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        manifest.discovery.platform_paths.clear();
        manifest.discovery.user_paths.clear();
        let planted_directory = std::env::current_dir()
            .unwrap()
            .join("target/registry-probe-target");
        std::fs::create_dir_all(&planted_directory).unwrap();
        let planted = planted_directory.join("codex");
        std::fs::write(&planted, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755)).unwrap();

        // An empty entry and a relative entry both resolve against the CWD,
        // where the executable above has been planted.
        let context = DiscoveryContext {
            platform: Platform::current(),
            path_entries: vec![
                PathBuf::new(),
                PathBuf::from("target/registry-probe-target"),
            ],
            home_dir: None,
        };
        let discovered = discover_executables(&manifest, &context).unwrap();
        std::fs::remove_dir_all(&planted_directory).unwrap();
        assert!(
            discovered.is_empty(),
            "a CWD-relative candidate must never be discovered"
        );
    }

    #[test]
    fn rejects_external_manifest_without_a_product_signature() {
        let source = CODEX_MANIFEST.replace(
            "version_output_markers = [\"codex-cli\"]",
            "version_output_markers = []",
        );
        assert!(matches!(
            ProviderManifest::parse_toml(&source),
            Err(RegistryError::InvalidManifest(_))
        ));
    }

    #[test]
    fn codebuddy_protocol_flag_is_allowed_but_permission_overrides_are_not() {
        let source = include_str!("../../../drivers/manifests/codebuddy.toml");
        ProviderManifest::parse_toml(source).unwrap();
        assert!(
            ProviderManifest::parse_toml(&source.replace(
                "launch_args = [\"--acp\"]",
                "launch_args = [\"--dangerously-skip-permissions\"]"
            ))
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovers_codebuddy_in_nvm_arm_without_shell_path() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".nvm-arm/versions/node/v22.0.0/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("codebuddy");
        std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manifest =
            ProviderManifest::parse_toml(include_str!("../../../drivers/manifests/codebuddy.toml"))
                .unwrap();
        let result = discover_executables(
            &manifest,
            &DiscoveryContext {
                platform: Platform::current(),
                path_entries: vec![],
                home_dir: Some(home.path().into()),
            },
        )
        .unwrap();
        assert_eq!(result, vec![std::fs::canonicalize(exe).unwrap()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovers_symlinks_once_and_parses_a_version() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        symlink("/bin/echo", directory.path().join("codex")).unwrap();
        symlink("/bin/echo", directory.path().join("codex-alias")).unwrap();
        let mut manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        manifest.discovery.candidate_executables = vec!["codex".into(), "codex-alias".into()];
        manifest.discovery.version_args = vec!["codex-cli 1.2.3".into()];
        manifest.discovery.platform_paths.clear();
        manifest.discovery.user_paths.clear();
        let reports = discover_and_probe(
            &manifest,
            &DiscoveryContext {
                platform: Platform::current(),
                path_entries: vec![directory.path().to_path_buf()],
                home_dir: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].version, Some(Version::new(1, 2, 3)));
        assert_eq!(reports[0].status, InstallationStatus::Installed);
    }

    #[cfg(unix)]
    #[test]
    fn discovers_home_relative_user_paths_but_escapes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let bin = home.join(".kimi-code/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("kimi"), b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("kimi"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let mut manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        manifest.discovery.candidate_executables = vec!["kimi".into()];
        manifest.discovery.version_args = vec!["--version".into()];
        manifest.discovery.platform_paths.clear();
        manifest.discovery.user_paths = BTreeMap::from([(
            Platform::current(),
            vec![PathBuf::from(".kimi-code/bin/kimi")],
        )]);
        let context = DiscoveryContext {
            platform: Platform::current(),
            path_entries: vec![],
            home_dir: Some(home),
        };
        let discovered = discover_executables(&manifest, &context).unwrap();
        assert_eq!(discovered.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_probe_can_carry_the_product_marker_when_version_output_is_bare() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-agent");
        std::fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2.3.4\"; fi\nif [ \"$1\" = \"identify\" ]; then echo \"fake-agent-code as a protocol server\"; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        manifest.discovery.candidate_executables = vec!["fake-agent".into()];
        manifest.discovery.version_args = vec!["--version".into()];
        manifest.discovery.identity_args = vec!["identify".into()];
        manifest.discovery.version_output_markers = BTreeSet::from(["fake-agent-code".into()]);
        manifest.discovery.platform_paths.clear();
        manifest.discovery.user_paths.clear();

        let context = DiscoveryContext {
            platform: Platform::current(),
            path_entries: vec![directory.path().to_path_buf()],
            home_dir: None,
        };
        let reports = discover_and_probe(&manifest, &context).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, InstallationStatus::Installed);
        assert_eq!(reports[0].version, Some(Version::new(2, 3, 4)));

        // Without the marker surface the bare version proves nothing.
        manifest.discovery.identity_args = vec![];
        let reports = discover_and_probe(&manifest, &context).await.unwrap();
        assert_eq!(reports[0].status, InstallationStatus::Broken);
        assert_eq!(
            reports[0].diagnostic.as_deref(),
            Some("version probe output did not match the provider signature")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_different_product_that_uses_the_same_executable_name() {
        let mut manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        manifest.discovery.version_args = vec!["grok-build 1.2.3".into()];

        let report = probe_installation(&manifest, PathBuf::from("/bin/echo"))
            .await
            .unwrap();
        assert_eq!(report.status, InstallationStatus::Broken);
        assert!(report.version.is_none());
        assert!(report.version_text.is_none());
        assert_eq!(
            report.diagnostic.as_deref(),
            Some("version probe output did not match the provider signature")
        );
    }

    #[test]
    fn detects_unsupported_and_denied_versions() {
        let policy = SupportedVersions {
            min: Some(Version::new(1, 2, 0)),
            recommended: Some(Version::new(2, 0, 0)),
            denylist: BTreeSet::from([Version::new(1, 5, 0)]),
        };
        assert!(!version_supported(&Version::new(1, 1, 9), &policy));
        assert!(!version_supported(&Version::new(1, 5, 0), &policy));
        assert!(version_supported(&Version::new(1, 6, 0), &policy));
    }

    #[ignore = "requires a user-installed Codex executable"]
    #[tokio::test]
    async fn probes_the_current_users_codex_installation() {
        let manifest = ProviderManifest::parse_toml(CODEX_MANIFEST).unwrap();
        let reports = discover_and_probe(&manifest, &DiscoveryContext::from_current_process())
            .await
            .unwrap();
        assert!(!reports.is_empty(), "Codex was not found on PATH");
        assert!(
            reports
                .iter()
                .any(|report| report.status == InstallationStatus::Installed),
            "no runnable Codex installation found: {reports:#?}"
        );
        eprintln!("Codex probe: {reports:#?}");
    }

    /// Probes the manifests that rely on `identity_args`/`user_paths` against
    /// the real local installations.
    #[ignore = "requires the user-installed Kimi Code / Grok Build / ZCode / Claude Code"]
    #[tokio::test]
    async fn probes_the_current_users_acp_and_ecosystem_installations() {
        for (source, expected_provider) in [
            (include_str!("../../../drivers/manifests/kimi.toml"), "kimi"),
            (include_str!("../../../drivers/manifests/grok.toml"), "grok"),
            (
                include_str!("../../../drivers/manifests/zcode.toml"),
                "zcode",
            ),
            (
                include_str!("../../../drivers/manifests/claude-code.toml"),
                "claude-code",
            ),
            (
                include_str!("../../../drivers/manifests/cursor.toml"),
                "cursor",
            ),
            (
                include_str!("../../../drivers/manifests/antigravity.toml"),
                "antigravity",
            ),
        ] {
            let manifest = ProviderManifest::parse_toml(source).unwrap();
            assert_eq!(manifest.id.as_str(), expected_provider);
            let reports = discover_and_probe(&manifest, &DiscoveryContext::from_current_process())
                .await
                .unwrap();
            assert!(
                reports.iter().any(|report| matches!(
                    report.status,
                    InstallationStatus::Installed | InstallationStatus::NotConfigured
                )),
                "{expected_provider} discovery/probe found no usable installation: {reports:#?}"
            );
            eprintln!("{expected_provider} probe: {reports:#?}");
        }
    }
}
