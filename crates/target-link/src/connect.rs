//! One connection attempt: start `ssh`, answer its questions, deploy the shim if
//! needed and finish the handshake. Dropping the future ends the attempt, because
//! the `ssh` process and the prompt listener are both owned by it.
use crate::{
    askpass::{AskpassPrompt, AskpassServer, PromptHandler},
    boot,
    error::{ConnectError, ConnectErrorKind, classify_boot_failure, classify_ssh_failure},
    link::TargetLink,
    program::SshProgram,
    shims::ShimCatalog,
};
use agent_protocol::{SshAuth, SshConnection};
use agent_runtime::OwnedChild;
use agent_runtime::{ProcessInput, ProcessStream};
use serde::Serialize;
use std::{
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use target_wire::READY_PREFIX;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

/// Time without progress before giving up; a pending question pauses it.
const STALL_LIMIT: Duration = Duration::from_secs(45);
const MAX_LINE: u64 = 64 * 1024;
const MAX_NOISE: usize = 1024 * 1024;
const MAX_STDERR: usize = 16 * 1024;
const UPLOAD_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum ConnectStep {
    Connecting,
    Inspecting,
    Deploying { sent: u64, total: u64 },
    Starting,
    Ready,
}

#[async_trait::async_trait]
pub trait ConnectObserver: Send + Sync {
    fn step(&self, step: ConnectStep);
    fn log(&self, line: &str);
    async fn prompt(&self, prompt: AskpassPrompt) -> Option<String>;
}

pub struct ConnectRequest {
    pub connection: SshConnection,
    pub program: SshProgram,
    /// Started by `ssh` to ask questions; normally this very executable.
    pub askpass_program: PathBuf,
    pub shims: ShimCatalog,
    /// Replaces `~/.ssh/config`. Used by tests, never by the product.
    pub config_file: Option<PathBuf>,
}

/// Pending questions pause the stall timer: a person is not a network stall.
struct Prompts {
    observer: Arc<dyn ConnectObserver>,
    pending: AtomicUsize,
    touched: Mutex<Instant>,
}

impl Prompts {
    fn touch(&self) {
        *self.touched.lock().unwrap() = Instant::now();
    }
    fn stalled(&self) -> bool {
        self.pending.load(Ordering::SeqCst) == 0
            && self.touched.lock().unwrap().elapsed() > STALL_LIMIT
    }
}

#[async_trait::async_trait]
impl PromptHandler for Prompts {
    async fn answer(&self, prompt: AskpassPrompt) -> Option<String> {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let answer = self.observer.prompt(prompt).await;
        self.pending.fetch_sub(1, Ordering::SeqCst);
        self.touch();
        answer
    }
}

/// Arguments before the destination. `--` then guarantees the destination cannot
/// be read as an option, on top of the validation done when it was saved.
pub(crate) fn ssh_arguments(
    connection: &SshConnection,
    batch: bool,
    config_file: Option<&PathBuf>,
    remote_command: &str,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-T".into()];
    if let Some(file) = config_file {
        args.push("-F".into());
        args.push(file.into());
    }
    for option in [
        if batch {
            "BatchMode=yes"
        } else {
            "BatchMode=no"
        },
        "StrictHostKeyChecking=ask",
        "ConnectTimeout=15",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        // Forwarding to a server would let its agents use this machine's keys.
        "ForwardAgent=no",
        "ForwardX11=no",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        // A forked master would keep our pipes open past the end of this connection.
        "ControlMaster=no",
        "ControlPath=none",
    ] {
        args.push("-o".into());
        args.push(option.into());
    }
    if let Some(port) = connection.port {
        args.push("-p".into());
        args.push(port.to_string().into());
    }
    if let Some(user) = &connection.user {
        args.push("-l".into());
        args.push(user.into());
    }
    if let (SshAuth::PrivateKey, Some(file)) = (connection.auth, &connection.identity_file) {
        args.push("-i".into());
        args.push(file.into());
        args.push("-o".into());
        args.push("IdentitiesOnly=yes".into());
    }
    args.push("--".into());
    args.push((&connection.destination).into());
    args.push(remote_command.into());
    args
}

pub async fn connect(
    request: ConnectRequest,
    observer: Arc<dyn ConnectObserver>,
) -> Result<TargetLink, ConnectError> {
    let ConnectRequest {
        connection,
        program,
        askpass_program,
        shims,
        config_file,
    } = request;
    if !program.supports_askpass() && connection.auth == SshAuth::Password {
        return Err(ConnectError::new(
            ConnectErrorKind::SshTooOld,
            format!("OpenSSH {}.{}", program.version.0, program.version.1),
        ));
    }
    observer.step(ConnectStep::Connecting);
    let prompts = Arc::new(Prompts {
        observer: observer.clone(),
        pending: AtomicUsize::new(0),
        touched: Mutex::new(Instant::now()),
    });
    let askpass = AskpassServer::start(prompts.clone())
        .await
        .map_err(|error| ConnectError::new(ConnectErrorKind::Failed, error.to_string()))?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let batch = !program.supports_askpass();
    let mut command = agent_runtime::agent_command(&program.path);
    command
        .args(ssh_arguments(
            &connection,
            batch,
            config_file.as_ref(),
            &boot::remote_command(&nonce),
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if !batch {
        askpass.apply(&mut command, &askpass_program);
    }
    let mut child = OwnedChild::spawn(command)
        .map_err(|error| ConnectError::new(ConnectErrorKind::SshMissing, error.to_string()))?;
    let stderr_tail = collect_stderr(&mut child, observer.clone());
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
        return Err(ConnectError::new(
            ConnectErrorKind::Failed,
            "ssh pipes unavailable",
        ));
    };
    let mut session = Boot {
        reader: BufReader::new(stdout),
        writer: stdin,
        prompts: prompts.clone(),
        nonce,
        observer: observer.clone(),
    };
    match session.run(&connection, &shims).await {
        Ok(()) => {
            drop(askpass);
            let Boot { reader, writer, .. } = session;
            let link = TargetLink::start(child, reader, writer, stderr_tail).await?;
            observer.step(ConnectStep::Ready);
            Ok(link)
        }
        Err(failure) => Err(explain(failure, &mut child, &stderr_tail).await),
    }
}

fn collect_stderr(
    child: &mut OwnedChild,
    observer: Arc<dyn ConnectObserver>,
) -> Arc<Mutex<String>> {
    let tail = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let shared = tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match (&mut reader)
                    .take(MAX_LINE)
                    .read_until(b'\n', &mut bytes)
                    .await
                {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&bytes);
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                observer.log(line);
                let mut tail = shared.lock().unwrap();
                tail.push_str(line);
                tail.push('\n');
                if tail.len() > MAX_STDERR {
                    let mut start = tail.len() - MAX_STDERR;
                    while !tail.is_char_boundary(start) {
                        start += 1;
                    }
                    tail.drain(..start);
                }
            }
        });
    }
    tail
}

enum BootFailure {
    /// `ssh` closed its output: why is decided by its exit status and stderr.
    Ended,
    Reported(ConnectError),
}

/// Attributes an ended session to its cause, once `ssh` has really finished.
async fn explain(
    failure: BootFailure,
    child: &mut OwnedChild,
    stderr: &Arc<Mutex<String>>,
) -> ConnectError {
    if let BootFailure::Reported(error) = failure {
        let _ = child.terminate().await;
        return error;
    }
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    let code = if let Ok(Ok(status)) = status {
        status.code()
    } else {
        let _ = child.terminate().await;
        None
    };
    // Let the stderr task drain what the process wrote before it exited.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let detail = stderr.lock().unwrap().trim().to_owned();
    ConnectError::new(attribute_end(code, &detail), detail)
}

/// 255 is `ssh` itself failing. Any other status means authentication succeeded
/// and the account's shell refused or failed to run our command.
fn attribute_end(code: Option<i32>, stderr: &str) -> ConnectErrorKind {
    match code {
        Some(255) | None => classify_ssh_failure(stderr),
        Some(_) => ConnectErrorKind::AccountRestricted,
    }
}

struct Boot {
    reader: BufReader<ProcessStream>,
    writer: ProcessInput,
    prompts: Arc<Prompts>,
    nonce: String,
    observer: Arc<dyn ConnectObserver>,
}

impl Boot {
    async fn run(
        &mut self,
        connection: &SshConnection,
        shims: &ShimCatalog,
    ) -> Result<(), BootFailure> {
        self.await_marker(&format!("DOGCODES-BOOT {}", self.nonce))
            .await?;
        self.observer.step(ConnectStep::Inspecting);
        let platform = self.next_line().await?;
        let mut parts = platform.strip_prefix("PLATFORM ").unwrap_or("").split(' ');
        let (os, machine) = (
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
        );
        let home = self.next_line().await?;
        if !home.starts_with("HOME ") {
            return Err(protocol_failure(&home));
        }
        self.observer.log(&format!("platform: {os} {machine}"));
        let payload = shims
            .payload(&os, &machine)
            .map_err(BootFailure::Reported)?;
        let root = connection.install_directory.as_deref().unwrap_or("-");
        let header = format!(
            "{root}\n{}\n{}\n{}\n",
            payload.version_directory,
            payload.digest,
            payload.bytes.len()
        );
        self.write_all(header.as_bytes()).await?;
        let decision = self.next_line().await?;
        match decision.as_str() {
            "PRESENT" => self.observer.log("shim: already installed"),
            "NEED" => self.upload(&payload.bytes).await?,
            other => return Err(protocol_failure(other)),
        }
        self.observer.step(ConnectStep::Starting);
        self.await_marker(&format!("{READY_PREFIX} {}", self.nonce))
            .await
    }

    async fn upload(&mut self, bytes: &[u8]) -> Result<(), BootFailure> {
        let total = bytes.len() as u64;
        let mut sent = 0;
        self.observer.step(ConnectStep::Deploying { sent, total });
        for chunk in bytes.chunks(UPLOAD_CHUNK) {
            self.write_all(chunk).await?;
            sent += chunk.len() as u64;
            self.prompts.touch();
            self.observer.step(ConnectStep::Deploying { sent, total });
        }
        self.observer.log("shim: uploaded");
        Ok(())
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), BootFailure> {
        // A write error means the session ended; the reader explains why.
        self.writer
            .write_all(bytes)
            .await
            .map_err(|_| BootFailure::Ended)?;
        self.writer.flush().await.map_err(|_| BootFailure::Ended)
    }

    async fn await_marker(&mut self, marker: &str) -> Result<(), BootFailure> {
        let mut noise = 0;
        loop {
            let line = self.read_line().await?;
            noise += line.len();
            if line.contains(marker) {
                return Ok(());
            }
            check_failure(&line)?;
            if noise > MAX_NOISE {
                return Err(BootFailure::Reported(ConnectError::new(
                    ConnectErrorKind::AccountRestricted,
                    "the login shell produced too much output",
                )));
            }
        }
    }

    async fn next_line(&mut self) -> Result<String, BootFailure> {
        let line = self.read_line().await?;
        check_failure(&line)?;
        Ok(line)
    }

    async fn read_line(&mut self) -> Result<String, BootFailure> {
        loop {
            let read = read_line(&mut self.reader);
            tokio::pin!(read);
            let line = loop {
                tokio::select! {
                    line = &mut read => break line,
                    () = tokio::time::sleep(Duration::from_secs(1)) => {
                        if self.prompts.stalled() {
                            return Err(BootFailure::Reported(ConnectError::new(
                                ConnectErrorKind::Timeout,
                                "no progress from the connection",
                            )));
                        }
                    }
                }
            };
            self.prompts.touch();
            let Some(line) = line else {
                return Err(BootFailure::Ended);
            };
            if !line.is_empty() {
                return Ok(line);
            }
        }
    }
}

async fn read_line<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Option<String> {
    let mut bytes = Vec::new();
    let count = (&mut *reader)
        .take(MAX_LINE)
        .read_until(b'\n', &mut bytes)
        .await
        .ok()?;
    if count == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).trim_end().to_owned())
}

fn check_failure(line: &str) -> Result<(), BootFailure> {
    match line.strip_prefix("DOGCODES-ERR ") {
        Some(code) => Err(BootFailure::Reported(ConnectError::new(
            classify_boot_failure(code),
            code,
        ))),
        None => Ok(()),
    }
}

fn protocol_failure(line: &str) -> BootFailure {
    BootFailure::Reported(ConnectError::new(
        ConnectErrorKind::ProtocolMismatch,
        format!("unexpected boot line: {line}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> SshConnection {
        SshConnection {
            destination: "box".into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_config_alias_is_left_for_ssh_to_resolve() {
        let args = strings(&ssh_arguments(&connection(), false, None, "cmd"));
        assert!(
            !args
                .iter()
                .any(|arg| arg == "-p" || arg == "-l" || arg == "-i")
        );
        assert_eq!(&args[args.len() - 3..], ["--", "box", "cmd"]);
    }

    #[test]
    fn the_destination_always_follows_the_end_of_options() {
        let mut value = connection();
        value.port = Some(2222);
        value.user = Some("dev".into());
        value.auth = SshAuth::PrivateKey;
        value.identity_file = Some("~/.ssh/id_ed25519".into());
        let args = strings(&ssh_arguments(&value, false, None, "cmd"));
        let end = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(args[end + 1], "box");
        for flag in ["-p", "-l", "-i"] {
            assert!(args.iter().position(|arg| arg == flag).unwrap() < end);
        }
    }

    #[test]
    fn forwarding_is_off_whatever_the_user_config_says() {
        let args = strings(&ssh_arguments(&connection(), false, None, "cmd"));
        for option in [
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "StrictHostKeyChecking=ask",
        ] {
            assert!(args.iter().any(|arg| arg == option), "{option}");
        }
    }

    #[test]
    fn a_client_that_cannot_prompt_through_us_never_waits_for_input() {
        let args = strings(&ssh_arguments(&connection(), true, None, "cmd"));
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
    }

    #[test]
    fn exit_status_separates_ssh_failures_from_restricted_accounts() {
        assert_eq!(
            attribute_end(Some(255), "Permission denied (publickey)."),
            ConnectErrorKind::AuthFailed
        );
        assert_eq!(
            attribute_end(Some(1), "fatal: unrecognized command"),
            ConnectErrorKind::AccountRestricted
        );
        assert_eq!(
            attribute_end(Some(127), "sh: not found"),
            ConnectErrorKind::AccountRestricted
        );
    }
}
