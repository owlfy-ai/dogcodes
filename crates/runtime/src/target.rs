//! Where an agent process runs. Drivers describe the process they want and the target
//! decides what that means: environment values and temporary directories belong to
//! the machine the process runs on, never to whichever machine hosts this app.
use crate::{
    OwnedChild, agent_command, apply_platform_environment, apply_proxy_environment, external_path,
    inherited_variable, installed_node_path,
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock},
};

/// Whether a process stream is delivered to the driver or discarded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Pipe {
    #[default]
    Piped,
    Null,
}

impl From<Pipe> for Stdio {
    fn from(value: Pipe) -> Self {
        match value {
            Pipe::Piped => Self::piped(),
            Pipe::Null => Self::null(),
        }
    }
}

/// A process description with nothing inherited implicitly: the environment holds
/// only what the driver named, so a target cannot leak its host's variables.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessRequest {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<OsString, OsString>,
    pub cwd: Option<PathBuf>,
    pub stdin: Pipe,
    pub stdout: Pipe,
    pub stderr: Pipe,
}

/// A process under a terminal, whose size can change while it runs.
pub struct TerminalChild {
    pub child: OwnedChild,
    pub resize: Box<dyn Fn(u16, u16) -> io::Result<()> + Send + Sync>,
}

pub trait ExecutionTarget: Send + Sync {
    fn is_local(&self) -> bool;
    /// The value this variable has on the target.
    fn inherited(&self, name: &str) -> Option<OsString>;
    fn temp_directory(&self) -> PathBuf;
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild>;
    /// Starts `request` under a new terminal on the target. This machine's terminals
    /// have their own host, so only another machine answers this.
    fn spawn_terminal(
        &self,
        _request: ProcessRequest,
        _cols: u16,
        _rows: u16,
    ) -> io::Result<TerminalChild> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// Starts an operation on the target, spoken over its stdin and stdout. Work on
    /// this machine's projects is called directly and never comes through here.
    fn spawn_operation(&self) -> io::Result<OwnedChild> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// The whole login environment on the target, for processes a person works in.
    fn login_environment(&self) -> BTreeMap<OsString, OsString> {
        BTreeMap::new()
    }
    /// A command an agent on this target can run to reach `command` on the machine
    /// hosting this app. A local agent can simply run it.
    fn app_tool_command(
        &self,
        command: &str,
        args: &[String],
    ) -> io::Result<(String, Vec<String>)> {
        Ok((command.to_owned(), args.to_vec()))
    }
    /// Only meaningful remotely: a local process checks its own account directly.
    fn runs_as_root(&self) -> bool {
        false
    }
}

pub type SharedTarget = Arc<dyn ExecutionTarget>;

impl std::fmt::Debug for dyn ExecutionTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionTarget")
            .field("local", &self.is_local())
            .finish()
    }
}

struct LocalTarget;

impl ExecutionTarget for LocalTarget {
    fn is_local(&self) -> bool {
        true
    }
    fn inherited(&self, name: &str) -> Option<OsString> {
        inherited_variable(name)
    }
    fn temp_directory(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        let mut command = agent_command(&request.program);
        command
            .args(&request.args)
            .env_clear()
            .stdin(Stdio::from(request.stdin))
            .stdout(Stdio::from(request.stdout))
            .stderr(Stdio::from(request.stderr))
            .kill_on_drop(true);
        if let Some(cwd) = &request.cwd {
            command.current_dir(external_path(cwd));
        }
        apply_platform_environment(&mut command);
        command.envs(&request.env);
        // A proxy set for this machine describes this machine's network.
        apply_proxy_environment(&mut command);
        OwnedChild::spawn(command)
    }
}

static LOCAL: LazyLock<SharedTarget> = LazyLock::new(|| Arc::new(LocalTarget));

/// Writes a private file on the target, creating its directory, through a shell so
/// it needs nothing but the running connection.
pub async fn write_target_file(target: &SharedTarget, path: &str, bytes: &[u8]) -> io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut command = TargetCommand::new(target.clone(), "/bin/sh");
    command
        .args([
            "-c",
            r#"umask 077; mkdir -p "$(dirname "$1")" && cat > "$1""#,
            "sh",
            path,
        ])
        .inherit("PATH")
        .stdio(Pipe::Piped, Pipe::Null, Pipe::Piped);
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("stdin unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("stderr unavailable"))?;
    let writer = async {
        stdin.write_all(bytes).await?;
        stdin.shutdown().await
    };
    let mut message = String::new();
    let mut limited = (&mut stderr).take(4096);
    let (written, _, status) =
        tokio::join!(writer, limited.read_to_string(&mut message), child.wait());
    written?;
    if status?.success() {
        Ok(())
    } else {
        Err(io::Error::other(message.trim().to_owned()))
    }
}

#[must_use]
pub fn local_target() -> SharedTarget {
    LOCAL.clone()
}

/// An executable together with the machine it lives on.
#[derive(Clone, Debug)]
pub struct TargetExecutable {
    pub target: SharedTarget,
    pub path: PathBuf,
}

impl TargetExecutable {
    #[must_use]
    pub fn new(target: SharedTarget, path: impl Into<PathBuf>) -> Self {
        Self {
            target,
            path: path.into(),
        }
    }
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::new(local_target(), path)
    }
    #[must_use]
    pub fn command(&self) -> TargetCommand {
        TargetCommand::new(self.target.clone(), &self.path)
    }
}

impl From<&TargetExecutable> for TargetExecutable {
    fn from(executable: &TargetExecutable) -> Self {
        executable.clone()
    }
}

impl AsRef<Path> for TargetExecutable {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl From<PathBuf> for TargetExecutable {
    fn from(path: PathBuf) -> Self {
        Self::local(path)
    }
}

impl From<&Path> for TargetExecutable {
    fn from(path: &Path) -> Self {
        Self::local(path)
    }
}

impl From<&PathBuf> for TargetExecutable {
    fn from(path: &PathBuf) -> Self {
        Self::local(path.clone())
    }
}

impl From<String> for TargetExecutable {
    fn from(path: String) -> Self {
        Self::local(path)
    }
}

impl From<&str> for TargetExecutable {
    fn from(path: &str) -> Self {
        Self::local(path)
    }
}

pub struct TargetCommand {
    target: SharedTarget,
    request: ProcessRequest,
}

impl TargetCommand {
    #[must_use]
    pub fn new(target: SharedTarget, program: impl AsRef<OsStr>) -> Self {
        Self {
            target,
            request: ProcessRequest {
                program: program.as_ref().to_owned(),
                ..ProcessRequest::default()
            },
        }
    }
    pub fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.request.args.push(value.as_ref().to_owned());
        self
    }
    pub fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.request
            .args
            .extend(values.into_iter().map(|value| value.as_ref().to_owned()));
        self
    }
    pub fn env(&mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.request
            .env
            .insert(name.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }
    /// Copies this variable from the target's own environment, if it is set there.
    pub fn inherit(&mut self, name: &str) -> &mut Self {
        if let Some(value) = self.target.inherited(name) {
            self.request.env.insert(name.into(), value);
        }
        self
    }
    pub fn current_dir(&mut self, directory: impl AsRef<Path>) -> &mut Self {
        self.request.cwd = Some(directory.as_ref().to_owned());
        self
    }
    pub fn stdio(&mut self, stdin: Pipe, stdout: Pipe, stderr: Pipe) -> &mut Self {
        self.request.stdin = stdin;
        self.request.stdout = stdout;
        self.request.stderr = stderr;
        self
    }
    /// An npm-installed CLI needs its own Node even when started from a GUI. A remote
    /// target already resolves this through its login environment.
    pub fn prefer_installed_node(&mut self, executable: &Path) -> &mut Self {
        if self.target.is_local()
            && let Some(path) = installed_node_path(executable)
        {
            self.env("PATH", path);
        }
        self
    }
    pub fn spawn(self) -> io::Result<OwnedChild> {
        self.target.spawn(self.request)
    }
    #[must_use]
    pub fn request(&self) -> &ProcessRequest {
        &self.request
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records what would have run on another machine.
    #[derive(Default)]
    struct Recording(Mutex<Vec<ProcessRequest>>);

    impl ExecutionTarget for Recording {
        fn is_local(&self) -> bool {
            false
        }
        fn inherited(&self, name: &str) -> Option<OsString> {
            (name == "HOME").then(|| "/home/remote".into())
        }
        fn temp_directory(&self) -> PathBuf {
            "/remote/tmp".into()
        }
        fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
            self.0.lock().unwrap().push(request);
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    #[test]
    fn a_remote_process_sees_only_values_from_its_own_machine() {
        let target = Arc::new(Recording::default());
        let mut command = TargetCommand::new(target.clone(), "/usr/bin/agent");
        command
            .args(["--mode", "rpc"])
            .inherit("HOME")
            .inherit("PATH")
            .env("EXPLICIT", "1")
            .prefer_installed_node(Path::new("/usr/bin/agent"));
        let _ = command.spawn();
        let recorded = target.0.lock().unwrap();
        let env = &recorded[0].env;
        assert_eq!(env.get(OsStr::new("HOME")).unwrap(), "/home/remote");
        assert_eq!(env.get(OsStr::new("EXPLICIT")).unwrap(), "1");
        // Set here but not there: this machine's value must not cross over.
        assert!(!env.contains_key(OsStr::new("PATH")));
        assert_eq!(env.len(), 2);
        assert_eq!(recorded[0].args, ["--mode", "rpc"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_local_process_gets_a_clean_environment_and_the_named_values() {
        use tokio::io::AsyncReadExt;
        let mut command = TargetCommand::new(local_target(), "/bin/sh");
        command
            .args([
                "-c",
                "printf '%s:%s' \"$NAMED\" \"${SHOULD_NOT_EXIST-unset}\"",
            ])
            .env("NAMED", "value")
            .stdio(Pipe::Null, Pipe::Piped, Pipe::Null);
        let mut child = command.spawn().unwrap();
        assert!(child.stderr.is_none());
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .await
            .unwrap();
        child.wait().await.unwrap();
        assert_eq!(output, "value:unset");
    }
}
