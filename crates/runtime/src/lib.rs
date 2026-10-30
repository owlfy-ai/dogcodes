//! Bounded, structured child-process execution for agent probes and runtimes.

mod electron;
mod environment;
mod proxy;
pub use process_scope::OwnedChild;

pub use electron::{electron_node_executable, electron_resources_directory};
pub use environment::apply_platform_environment;
pub use proxy::apply_proxy_environment;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::PathBuf,
    process::ExitStatus,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const READ_DRAIN_GRACE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    /// Names of variables copied from the host environment after `env_clear`.
    pub env_allowlist: BTreeSet<String>,
    /// Explicit non-secret environment values supplied by trusted adapters.
    pub env_overrides: BTreeMap<String, OsString>,
    pub cwd: Option<PathBuf>,
    pub timeout: Duration,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
}

impl ProcessSpec {
    #[must_use]
    pub fn probe(executable: PathBuf, args: Vec<String>) -> Self {
        Self {
            executable,
            args,
            env_allowlist: if cfg!(windows) {
                ["SystemRoot", "WINDIR", "TEMP", "TMP"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                BTreeSet::new()
            },
            env_overrides: BTreeMap::new(),
            cwd: None,
            timeout: Duration::from_secs(3),
            stdout_limit: 64 * 1_024,
            stderr_limit: 64 * 1_024,
        }
    }

    pub fn validate(&self) -> Result<(), ProcessError> {
        if !self.executable.is_absolute() {
            return Err(ProcessError::InvalidSpec(
                "executable must be an absolute path".into(),
            ));
        }
        if self.timeout.is_zero() {
            return Err(ProcessError::InvalidSpec(
                "timeout must be greater than zero".into(),
            ));
        }
        if self.stdout_limit == 0 || self.stderr_limit == 0 {
            return Err(ProcessError::InvalidSpec(
                "output limits must be greater than zero".into(),
            ));
        }
        for name in &self.env_allowlist {
            validate_environment_name(name)?;
        }
        for name in self.env_overrides.keys() {
            validate_environment_name(name)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub elapsed_millis: u64,
}

/// Lossless output for binary payloads. Text probes can keep using `run_bounded`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessBytesOutput {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub elapsed_millis: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProcessError {
    #[error("invalid process specification: {0}")]
    InvalidSpec(String),
    #[error("failed to spawn process: {0}")]
    Spawn(String),
    #[error("process timed out after {timeout_millis}ms")]
    TimedOut { timeout_millis: u64 },
    #[error("process I/O failed: {0}")]
    Io(String),
}

pub async fn run_bounded(spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
    run_bounded_bytes(spec).await.map(text_output)
}

/// Agent probes share the Core-owned process lifetime, including host crashes.
pub async fn run_scoped_bounded(spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
    run_bytes(spec, None, true).await.map(text_output)
}

fn text_output(output: ProcessBytesOutput) -> ProcessOutput {
    ProcessOutput {
        exit_code: output.exit_code,
        success: output.success,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        elapsed_millis: output.elapsed_millis,
    }
}

pub async fn run_bounded_bytes(spec: &ProcessSpec) -> Result<ProcessBytesOutput, ProcessError> {
    run_bounded_bytes_with_input(spec, None).await
}

/// Optional bounded stdin, written concurrently with output draining and waiting.
/// Input bytes are never included in errors or process metadata.
pub async fn run_bounded_bytes_with_input(
    spec: &ProcessSpec,
    input: Option<&[u8]>,
) -> Result<ProcessBytesOutput, ProcessError> {
    run_bytes(spec, input, false).await
}

async fn run_bytes(
    spec: &ProcessSpec,
    input: Option<&[u8]>,
    scoped: bool,
) -> Result<ProcessBytesOutput, ProcessError> {
    spec.validate()?;
    if input.is_some_and(|bytes| bytes.len() > 1024 * 1024) {
        return Err(ProcessError::InvalidSpec("stdin budget exceeded".into()));
    }
    let started_at = Instant::now();
    let executable = if spec
        .env_overrides
        .get("ELECTRON_RUN_AS_NODE")
        .is_some_and(|v| v == "1")
    {
        electron_node_executable(&spec.executable)?
    } else {
        spec.executable.clone()
    };
    let mut command = Command::new(executable);
    command
        .args(&spec.args)
        .env_clear()
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    apply_platform_environment(&mut command);
    for name in &spec.env_allowlist {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(&spec.env_overrides);
    if let Some(path) = installed_node_path(&spec.executable) {
        command.env("PATH", path);
    }

    let mut child = if scoped {
        OwnedChild::spawn(command).map(Process::Scoped)
    } else {
        command.spawn().map(Process::Direct)
    }
    .map_err(|error| ProcessError::Spawn(error.to_string()))?;
    let (stdin, stdout, stderr) = match &mut child {
        Process::Direct(child) => (child.stdin.take(), child.stdout.take(), child.stderr.take()),
        Process::Scoped(child) => (child.stdin.take(), child.stdout.take(), child.stderr.take()),
    };
    let stdout = stdout.ok_or_else(|| ProcessError::Io("stdout pipe was not created".into()))?;
    let stderr = stderr.ok_or_else(|| ProcessError::Io("stderr pipe was not created".into()))?;
    let stdout_task = tokio::spawn(read_capped(stdout, spec.stdout_limit));
    let stderr_task = tokio::spawn(read_capped(stderr, spec.stderr_limit));

    let execution = async {
        let writer = async {
            if let Some(mut stdin) = stdin {
                stdin.write_all(input.unwrap_or_default()).await?;
                stdin.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        };
        let (status, ()) = tokio::try_join!(child.wait(), writer)?;
        Ok::<_, std::io::Error>(status)
    };
    let status = if let Ok(result) = timeout(spec.timeout, execution).await {
        result.map_err(|error| ProcessError::Io(error.to_string()))?
    } else {
        let _ = child.terminate().await;
        let _ = child.wait().await;
        return Err(ProcessError::TimedOut {
            timeout_millis: millis(spec.timeout),
        });
    };
    let stdout = join_reader(stdout_task).await?;
    let stderr = join_reader(stderr_task).await?;
    Ok(output(status, stdout, stderr, started_at.elapsed()))
}

// Generic commands retain their original lifetime (e.g. external terminal
// launchers); only managed Agent probes opt into the Core-owned boundary.
enum Process {
    Direct(tokio::process::Child),
    Scoped(OwnedChild),
}
impl Process {
    async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        match self {
            Self::Direct(child) => child.wait().await,
            Self::Scoped(child) => child.wait().await,
        }
    }
    async fn terminate(&mut self) -> std::io::Result<()> {
        match self {
            Self::Direct(child) => child.kill().await,
            Self::Scoped(child) => child.terminate().await,
        }
    }
}

async fn read_capped<R>(mut reader: R, limit: usize) -> Result<CappedBytes, std::io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut kept = Vec::with_capacity(limit.min(8 * 1_024));
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1_024];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        let retained = remaining.min(count);
        kept.extend_from_slice(&buffer[..retained]);
        truncated |= retained < count;
    }
    Ok(CappedBytes { kept, truncated })
}

async fn join_reader(
    task: tokio::task::JoinHandle<Result<CappedBytes, std::io::Error>>,
) -> Result<CappedBytes, ProcessError> {
    timeout(READ_DRAIN_GRACE, task)
        .await
        .map_err(|_| ProcessError::Io("output pipe did not close".into()))?
        .map_err(|error| ProcessError::Io(error.to_string()))?
        .map_err(|error| ProcessError::Io(error.to_string()))
}

fn output(
    status: ExitStatus,
    stdout: CappedBytes,
    stderr: CappedBytes,
    elapsed: Duration,
) -> ProcessBytesOutput {
    ProcessBytesOutput {
        exit_code: status.code(),
        success: status.success(),
        stdout: stdout.kept,
        stderr: stderr.kept,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        elapsed_millis: millis(elapsed),
    }
}

struct CappedBytes {
    kept: Vec<u8>,
    truncated: bool,
}

fn validate_environment_name(name: &str) -> Result<(), ProcessError> {
    let upper = name.to_ascii_uppercase();
    let secret_markers = ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "COOKIE"];
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || secret_markers.iter().any(|marker| upper.contains(marker))
    {
        return Err(ProcessError::InvalidSpec(format!(
            "environment variable {name:?} is not allowed"
        )));
    }
    Ok(())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_executables_and_secret_environment_names() {
        let relative = ProcessSpec::probe("agent".into(), vec![]);
        assert!(matches!(
            relative.validate(),
            Err(ProcessError::InvalidSpec(_))
        ));

        let mut secret = ProcessSpec::probe("/bin/echo".into(), vec![]);
        secret.env_allowlist.insert("AGENT_API_KEY".into());
        assert!(matches!(
            secret.validate(),
            Err(ProcessError::InvalidSpec(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn managed_probe_preserves_bounded_output_and_its_deadline() {
        let mut spec = ProcessSpec::probe("/bin/echo".into(), vec!["managed probe".into()]);
        spec.stdout_limit = 7;
        let output = run_scoped_bounded(&spec).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, "managed");
        assert!(output.stdout_truncated);
        spec.executable = "/bin/sleep".into();
        spec.args = vec!["5".into()];
        spec.timeout = Duration::from_millis(30);
        assert!(matches!(
            run_scoped_bounded(&spec).await,
            Err(ProcessError::TimedOut { .. })
        ));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn executes_structured_argv_without_a_shell() {
        let spec = ProcessSpec::probe(
            "/bin/echo".into(),
            vec!["literal;$HOME".into(), "second".into()],
        );
        let output = run_bounded(&spec).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.trim(), "literal;$HOME second");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn passes_only_validated_explicit_environment_overrides() {
        let mut spec = ProcessSpec::probe("/usr/bin/env".into(), vec![]);
        spec.env_overrides
            .insert("DOGCODES_TEST_VALUE".into(), "literal;$HOME".into());
        let output = run_bounded(&spec).await.unwrap();
        assert_eq!(output.stdout.trim(), "DOGCODES_TEST_VALUE=literal;$HOME");

        spec.env_overrides
            .insert("PRIVATE_TOKEN".into(), "must-not-pass".into());
        assert!(matches!(spec.validate(), Err(ProcessError::InvalidSpec(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminates_a_probe_at_its_deadline() {
        let mut spec = ProcessSpec::probe("/bin/sleep".into(), vec!["5".into()]);
        spec.timeout = Duration::from_millis(30);
        assert!(matches!(
            run_bounded(&spec).await,
            Err(ProcessError::TimedOut { .. })
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn drains_but_truncates_large_output() {
        let mut spec = ProcessSpec::probe("/usr/bin/seq".into(), vec!["1".into(), "10000".into()]);
        spec.stdout_limit = 128;
        let output = run_bounded(&spec).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout.len(), 128);
        assert!(output.stdout_truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_stdin_roundtrips_binary_and_times_out_a_nonreader() {
        let input = vec![255; 256 * 1024];
        let mut spec = ProcessSpec::probe("/bin/cat".into(), vec![]);
        spec.stdout_limit = input.len();
        let output = run_bounded_bytes_with_input(&spec, Some(&input))
            .await
            .unwrap();
        assert_eq!(output.stdout, input);
        assert!(output.success);
        spec.stdout_limit = 32;
        let output = run_bounded_bytes_with_input(&spec, Some(&input))
            .await
            .unwrap();
        assert_eq!(output.stdout.len(), 32);
        assert!(output.stdout_truncated);
        let mut sleeping = ProcessSpec::probe("/bin/sleep".into(), vec!["5".into()]);
        sleeping.timeout = Duration::from_millis(30);
        assert!(matches!(
            run_bounded_bytes_with_input(&sleeping, Some(&input)).await,
            Err(ProcessError::TimedOut { .. })
        ));
        assert!(matches!(
            run_bounded_bytes_with_input(&spec, Some(&vec![0; 1024 * 1024 + 1])).await,
            Err(ProcessError::InvalidSpec(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preserves_non_utf8_bytes_and_caps_by_bytes() {
        let mut spec =
            ProcessSpec::probe("/usr/bin/printf".into(), vec![r"\377\000\303\251".into()]);
        let output = run_bounded_bytes(&spec).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, [255, 0, 195, 169]);
        assert!(!output.stdout_truncated);
        spec.stdout_limit = 3;
        let capped = run_bounded_bytes(&spec).await.unwrap();
        assert_eq!(capped.stdout, [255, 0, 195]);
        assert!(capped.stdout_truncated);
    }
}

/// Use the Node runtime belonging to an npm-installed CLI even from a GUI launch.
#[must_use]
pub fn installed_node_path(executable: &std::path::Path) -> Option<std::ffi::OsString> {
    let executable = std::fs::canonicalize(executable).ok()?;
    let root = executable.ancestors().find(|root| {
        executable.starts_with(root.join("lib/node_modules")) && root.join("bin/node").is_file()
    })?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(root.join("bin")).chain(std::env::split_paths(&inherited)))
        .ok()
}

#[cfg(test)]
mod installed_node_tests {
    #[test]
    fn discovers_sibling_node_for_npm_script() {
        let root = std::env::temp_dir().join(format!("dogcodes-node-{}", std::process::id()));
        let script = root.join("lib/node_modules/example/bin/cli");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(&script, "#!/usr/bin/env node").unwrap();
        std::fs::write(root.join("bin/node"), "node").unwrap();
        let path = super::installed_node_path(&script).unwrap();
        assert_eq!(
            std::env::split_paths(&path).next().unwrap(),
            std::fs::canonicalize(&root).unwrap().join("bin")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
