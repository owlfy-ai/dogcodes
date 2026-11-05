//! The live channel to a started shim: requests, a heartbeat, and the processes
//! running on the target. Each process stream has its own credit window, so a slow
//! reader stops its writer without holding up control messages or other processes.
use crate::error::{ConnectError, ConnectErrorKind};
use agent_runtime::{
    ExecutionTarget, OwnedChild, Pipe, ProcessFuture, ProcessRequest, RemoteProcess, SharedTarget,
    TargetCommand, TerminalChild, local_target,
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};
use target_wire::{
    Channel, Frame, FrameKind, PROTOCOL_VERSION, Request, RequestEnvelope, Response, STREAM_CHUNK,
    STREAM_WINDOW, ShimMessage, SpawnRequest, TargetInfo, TerminalSize, control_frame,
    credit_frame, data_frame, eof_frame, parse_control, parse_credit, read_frame, split_stream,
    stream_id, write_frame,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Notify, Semaphore, mpsc, oneshot, watch},
};

const PING_INTERVAL: Duration = Duration::from_secs(15);
/// Long enough to ride out what `ServerAlive*` already tolerates.
const REPLY_LIMIT: Duration = Duration::from_mins(1);
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(20);
const TERMINATE_LIMIT: Duration = Duration::from_secs(10);
const PIPE_BUFFER: usize = 64 * 1024;

#[derive(Clone, Debug)]
enum ExitState {
    Running,
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// Contact is gone. The process may still be running: this proves nothing.
    Lost,
}

struct ProcessRoute {
    stdin_credit: Arc<Semaphore>,
    stdout: Option<mpsc::UnboundedSender<Vec<u8>>>,
    stderr: Option<mpsc::UnboundedSender<Vec<u8>>>,
    exit: watch::Sender<ExitState>,
}

struct Core {
    outbound: mpsc::UnboundedSender<Frame>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    processes: Mutex<HashMap<u32, ProcessRoute>>,
    next_request: AtomicU64,
    next_process: AtomicU32,
    shutdown: Notify,
    closed: watch::Receiver<Option<String>>,
    runtime: tokio::runtime::Handle,
    /// Local programs agents may reach, by the token handed to them. The target only
    /// ever names a token: it cannot choose what runs on this machine.
    app_tools: Mutex<HashMap<String, AppTool>>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct AppTool {
    command: String,
    args: Vec<String>,
}

impl Core {
    fn send_request(&self, request: Request) -> oneshot::Receiver<Response> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, reply);
        match control_frame(&RequestEnvelope { id, request }) {
            Ok(frame) => {
                let _ = self.outbound.send(frame);
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
            }
        }
        answer
    }

    /// Fire and forget: a failed process reports itself through its exit message.
    fn send_command(&self, request: Request) {
        drop(self.send_request(request));
    }
}

pub struct TargetLink {
    info: TargetInfo,
    core: Arc<Core>,
    target: SharedTarget,
}

impl Drop for TargetLink {
    /// Ends the connection for everything using it, including running processes.
    fn drop(&mut self) {
        self.core.shutdown.notify_one();
    }
}

impl TargetLink {
    pub(crate) async fn start<R, W>(
        child: OwnedChild,
        reader: R,
        writer: W,
        stderr: Arc<Mutex<String>>,
    ) -> Result<Self, ConnectError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outbound, pending_frames) = mpsc::unbounded_channel();
        let (close, closed) = watch::channel(None);
        let core = Arc::new(Core {
            outbound,
            pending: Mutex::default(),
            processes: Mutex::default(),
            next_request: AtomicU64::new(1),
            next_process: AtomicU32::new(1),
            shutdown: Notify::new(),
            closed,
            runtime: tokio::runtime::Handle::current(),
            app_tools: Mutex::default(),
        });
        tokio::spawn(run(
            core.clone(),
            child,
            reader,
            writer,
            pending_frames,
            close,
            stderr,
        ));
        let failed = |detail: &str| ConnectError::new(ConnectErrorKind::ProtocolMismatch, detail);
        let response = tokio::time::timeout(HANDSHAKE_LIMIT, core.send_request(Request::Info))
            .await
            .map_err(|_| failed("shim did not answer"))?
            .map_err(|_| failed("shim channel closed"))?;
        let Response::Info(info) = response else {
            return Err(failed("unexpected reply to the first request"));
        };
        if info.protocol != PROTOCOL_VERSION {
            return Err(failed("shim protocol differs from this app"));
        }
        // A slow or broken profile only costs the login environment, not the link.
        let environment = match tokio::time::timeout(
            HANDSHAKE_LIMIT,
            core.send_request(Request::LoginEnvironment),
        )
        .await
        {
            Ok(Ok(Response::Environment { values })) => values,
            _ => BTreeMap::new(),
        };
        let target: SharedTarget = Arc::new(LinkTarget {
            core: core.clone(),
            environment,
            root: info.root,
            shim: info.executable.clone(),
            relay_socket: info.relay_socket.clone(),
        });
        Ok(Self { info, core, target })
    }

    #[must_use]
    pub fn info(&self) -> &TargetInfo {
        &self.info
    }

    /// Runs processes on this machine for as long as the link lives.
    #[must_use]
    pub fn target(&self) -> SharedTarget {
        self.target.clone()
    }

    /// Becomes `Some(reason)` once, when the channel is gone for any reason.
    #[must_use]
    pub fn closed(&self) -> watch::Receiver<Option<String>> {
        self.core.closed.clone()
    }

    /// The resolved path and its subdirectories, read on the target.
    pub async fn list_directory(&self, path: &str) -> Result<(String, Vec<String>), String> {
        let request = Request::ListDirectory {
            path: path.to_owned(),
        };
        match tokio::time::timeout(REPLY_LIMIT, self.core.send_request(request)).await {
            Ok(Ok(Response::Directory { path, directories })) => Ok((path, directories)),
            Ok(Ok(Response::Error { message })) => Err(message),
            Ok(Ok(_)) => Err("unexpected reply".into()),
            Ok(Err(_)) => Err("connection closed".into()),
            Err(_) => Err("timed out".into()),
        }
    }

    pub async fn ping(&self) -> Result<(), String> {
        match tokio::time::timeout(REPLY_LIMIT, self.core.send_request(Request::Ping)).await {
            Ok(Ok(Response::Pong)) => Ok(()),
            Ok(Ok(_)) => Err("unexpected reply".into()),
            Ok(Err(_)) => Err("connection closed".into()),
            Err(_) => Err("timed out".into()),
        }
    }
}

async fn run<R, W>(
    core: Arc<Core>,
    mut child: OwnedChild,
    mut reader: R,
    mut writer: W,
    mut pending_frames: mpsc::UnboundedReceiver<Frame>,
    close: watch::Sender<Option<String>>,
    stderr: Arc<Mutex<String>>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (ended, mut reader_ended) = oneshot::channel::<String>();
    let routing = core.clone();
    // One owner for the read position: `read_frame` must not be cancelled midway.
    let reader_task = tokio::spawn(async move {
        let reason = loop {
            match read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    if let Err(error) = route(&routing, frame) {
                        break error;
                    }
                }
                Ok(None) => break "connection closed".to_owned(),
                Err(error) => break error.to_string(),
            }
        };
        let _ = ended.send(reason);
    });
    let mut heartbeat = tokio::time::interval(PING_INTERVAL);
    heartbeat.tick().await;
    let mut unanswered_since: Option<tokio::time::Instant> = None;
    let mut heartbeat_reply: Option<oneshot::Receiver<Response>> = None;
    let reason = loop {
        tokio::select! {
            reason = &mut reader_ended => break reason.unwrap_or_else(|_| "connection closed".into()),
            () = core.shutdown.notified() => break "closed by this app".into(),
            frame = pending_frames.recv() => {
                let Some(frame) = frame else {
                    break "closed by this app".into();
                };
                if let Err(error) = write_frame(&mut writer, &frame).await {
                    break error.to_string();
                }
            }
            _ = heartbeat.tick() => {
                if let Some(receiver) = &mut heartbeat_reply {
                    if receiver.try_recv().is_ok() {
                        unanswered_since = None;
                    } else if unanswered_since.is_some_and(|since| since.elapsed() > REPLY_LIMIT) {
                        break "the target stopped responding".into();
                    }
                }
                heartbeat_reply = Some(core.send_request(Request::Ping));
                unanswered_since.get_or_insert_with(tokio::time::Instant::now);
            }
        }
    };
    reader_task.abort();
    // Every local reader sees its end of stream and every waiter learns contact is
    // lost. None of them is told the process exited, because nobody knows that.
    for (_, process) in core.processes.lock().unwrap().drain() {
        process.exit.send_replace(ExitState::Lost);
    }
    core.pending.lock().unwrap().clear();
    // Closing stdin lets the shim end by itself before the process is signalled.
    drop(writer);
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    let _ = child.terminate().await;
    let detail = stderr.lock().unwrap().trim().to_owned();
    let message = if detail.is_empty() {
        reason
    } else {
        format!("{reason}: {detail}")
    };
    let _ = close.send(Some(message));
}

/// Everything the shim sends is untrusted input: a frame for an unknown process is
/// dropped, and a malformed one ends the connection rather than being guessed at.
fn route(core: &Core, frame: Frame) -> Result<(), String> {
    match frame.kind {
        FrameKind::Control => {
            let message: ShimMessage = parse_control(&frame).map_err(|e| e.to_string())?;
            match message {
                ShimMessage::Response(envelope) => {
                    if let Some(reply) = core.pending.lock().unwrap().remove(&envelope.id) {
                        let _ = reply.send(envelope.response);
                    }
                }
                ShimMessage::RelayOpened { relay, token } => open_relay(core, relay, &token),
                ShimMessage::Exited {
                    process,
                    code,
                    signal,
                    ..
                } => {
                    if let Some(route) = core.processes.lock().unwrap().remove(&process) {
                        route.exit.send_replace(ExitState::Exited { code, signal });
                    }
                }
            }
        }
        FrameKind::Data | FrameKind::Eof => {
            let (process, channel) = split_stream(frame.stream).ok_or("invalid stream")?;
            let mut processes = core.processes.lock().unwrap();
            let Some(route) = processes.get_mut(&process) else {
                return Ok(());
            };
            let output = match channel {
                Channel::Stdout => &mut route.stdout,
                Channel::Stderr => &mut route.stderr,
                Channel::Stdin => return Err("output on an input stream".into()),
            };
            if frame.kind == FrameKind::Eof {
                *output = None;
            } else if let Some(output) = output {
                let _ = output.send(frame.payload);
            }
        }
        FrameKind::Credit => {
            let (process, channel) = split_stream(frame.stream).ok_or("invalid stream")?;
            let bytes = parse_credit(&frame).ok_or("invalid credit")?;
            if channel == Channel::Stdin
                && let Some(route) = core.processes.lock().unwrap().get(&process)
            {
                route.stdin_credit.add_permits(bytes as usize);
            }
        }
    }
    Ok(())
}

/// Connects one helper on the target to a fresh copy of the local program, using
/// the same streams and credit windows as a process running over there.
fn open_relay(core: &Core, relay: u32, token: &str) {
    let refuse = |core: &Core| {
        let _ = core
            .outbound
            .send(eof_frame(stream_id(relay, Channel::Stdin)));
    };
    let tool = core.app_tools.lock().unwrap().get(token).cloned();
    let Some(tool) = tool else {
        return refuse(core);
    };
    let mut command = TargetCommand::new(local_target(), &tool.command);
    command
        .args(&tool.args)
        .stdio(Pipe::Piped, Pipe::Piped, Pipe::Null);
    let Ok(mut child) = command.spawn() else {
        return refuse(core);
    };
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        return refuse(core);
    };
    let credit = Arc::new(Semaphore::new(STREAM_WINDOW as usize));
    let (requests, receiver) = mpsc::unbounded_channel();
    let (exit, _) = watch::channel(ExitState::Running);
    core.processes.lock().unwrap().insert(
        relay,
        ProcessRoute {
            stdin_credit: credit.clone(),
            stdout: Some(requests),
            stderr: None,
            exit,
        },
    );
    core.runtime.spawn(pump_output(
        input,
        receiver,
        core.outbound.clone(),
        stream_id(relay, Channel::Stdout),
    ));
    let outbound = core.outbound.clone();
    core.runtime.spawn(async move {
        // Ends with the local program, which ends when the helper's input does.
        pump_stdin(output, outbound, stream_id(relay, Channel::Stdin), credit).await;
        let _ = child.wait().await;
        let _ = child.terminate().await;
    });
}

struct LinkTarget {
    core: Arc<Core>,
    environment: BTreeMap<String, String>,
    root: bool,
    shim: String,
    relay_socket: Option<String>,
}

impl ExecutionTarget for LinkTarget {
    fn is_local(&self) -> bool {
        false
    }
    fn inherited(&self, name: &str) -> Option<OsString> {
        self.environment.get(name).map(OsString::from)
    }
    fn temp_directory(&self) -> PathBuf {
        self.environment
            .get("TMPDIR")
            .filter(|path| path.starts_with('/'))
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        spawn(&self.core, request, Start::Program).map(|(child, _)| child)
    }
    fn spawn_operation(&self) -> io::Result<OwnedChild> {
        let request = ProcessRequest {
            stdin: Pipe::Piped,
            stdout: Pipe::Piped,
            stderr: Pipe::Null,
            ..ProcessRequest::default()
        };
        spawn(&self.core, request, Start::Operation).map(|(child, _)| child)
    }
    fn spawn_terminal(
        &self,
        request: ProcessRequest,
        cols: u16,
        rows: u16,
    ) -> io::Result<TerminalChild> {
        let (child, process) = spawn(
            &self.core,
            request,
            Start::Terminal(TerminalSize { cols, rows }),
        )?;
        let core = self.core.clone();
        Ok(TerminalChild {
            child,
            resize: Box::new(move |cols, rows| {
                core.send_command(Request::Resize {
                    process,
                    cols,
                    rows,
                });
                Ok(())
            }),
        })
    }
    fn login_environment(&self) -> BTreeMap<OsString, OsString> {
        self.environment
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    }
    fn runs_as_root(&self) -> bool {
        self.root
    }
    /// The same tool keeps one token, so its command compares equal between turns
    /// and a live session is not rebuilt merely because a turn started.
    fn app_tool_command(
        &self,
        command: &str,
        args: &[String],
    ) -> io::Result<(String, Vec<String>)> {
        let socket = self.relay_socket.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "the target has no relay socket")
        })?;
        let tool = AppTool {
            command: command.to_owned(),
            args: args.to_vec(),
        };
        let mut tools = self.core.app_tools.lock().unwrap();
        let existing = tools
            .iter()
            .find(|(_, known)| **known == tool)
            .map(|(token, _)| token.clone());
        let token = existing.unwrap_or_else(|| {
            let token = uuid::Uuid::new_v4().simple().to_string();
            tools.insert(token.clone(), tool);
            token
        });
        Ok((
            self.shim.clone(),
            vec!["mcp-relay".into(), socket.clone(), token],
        ))
    }
}

fn text(value: &OsStr) -> io::Result<String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "value is not UTF-8"))
}

#[derive(Clone, Copy)]
enum Start {
    Program,
    Terminal(TerminalSize),
    Operation,
}

fn spawn(core: &Arc<Core>, request: ProcessRequest, start: Start) -> io::Result<(OwnedChild, u32)> {
    if core.closed.borrow().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "the target connection is closed",
        ));
    }
    let spawn = SpawnRequest {
        process: core.next_process.fetch_add(1, Ordering::Relaxed),
        program: text(&request.program)?,
        args: request
            .args
            .iter()
            .map(|arg| text(arg))
            .collect::<io::Result<_>>()?,
        env: request
            .env
            .iter()
            .map(|(name, value)| Ok((text(name)?, text(value)?)))
            .collect::<io::Result<_>>()?,
        cwd: request
            .cwd
            .as_deref()
            .map(|cwd| text(cwd.as_os_str()))
            .transpose()?,
        stdin: request.stdin == Pipe::Piped,
        stdout: request.stdout == Pipe::Piped,
        stderr: request.stderr == Pipe::Piped,
        terminal: match start {
            Start::Terminal(size) => Some(size),
            _ => None,
        },
        operation: matches!(start, Start::Operation),
    };
    let process = spawn.process;
    let stdin_credit = Arc::new(Semaphore::new(STREAM_WINDOW as usize));
    let stdin = (request.stdin == Pipe::Piped).then(|| {
        let (local, pump) = tokio::io::duplex(PIPE_BUFFER);
        core.runtime.spawn(pump_stdin(
            pump,
            core.outbound.clone(),
            stream_id(process, Channel::Stdin),
            stdin_credit.clone(),
        ));
        local
    });
    let output = |piped: Pipe, channel: Channel| {
        if piped != Pipe::Piped {
            return (None, None);
        }
        let (local, pump) = tokio::io::duplex(PIPE_BUFFER);
        let (sender, receiver) = mpsc::unbounded_channel();
        core.runtime.spawn(pump_output(
            pump,
            receiver,
            core.outbound.clone(),
            stream_id(process, channel),
        ));
        (Some(local), Some(sender))
    };
    let (stdout, stdout_route) = output(request.stdout, Channel::Stdout);
    let (stderr, stderr_route) = output(request.stderr, Channel::Stderr);
    let (exit, exit_state) = watch::channel(ExitState::Running);
    // Registered first: output can arrive before anything else happens here.
    core.processes.lock().unwrap().insert(
        process,
        ProcessRoute {
            stdin_credit,
            stdout: stdout_route,
            stderr: stderr_route,
            exit,
        },
    );
    core.send_command(Request::Spawn(spawn));
    Ok((
        OwnedChild::remote(
            Box::new(TargetProcess {
                core: core.clone(),
                process,
                exit: exit_state,
                termination_sent: false,
            }),
            stdin,
            stdout,
            stderr,
        ),
        process,
    ))
}

async fn pump_stdin<R: AsyncRead + Unpin>(
    mut local: R,
    outbound: mpsc::UnboundedSender<Frame>,
    stream: u32,
    credit: Arc<Semaphore>,
) {
    let mut buffer = vec![0; STREAM_CHUNK as usize];
    loop {
        let Ok(permit) = credit.acquire_many(STREAM_CHUNK).await else {
            return;
        };
        permit.forget();
        let count = local.read(&mut buffer).await.unwrap_or(0);
        credit.add_permits(STREAM_CHUNK as usize - count);
        if count == 0 {
            let _ = outbound.send(eof_frame(stream));
            return;
        }
        if outbound
            .send(data_frame(stream, buffer[..count].to_vec()))
            .is_err()
        {
            return;
        }
    }
}

/// Credit goes back once the local reader has taken the bytes, so a driver that
/// stops reading stops the remote process, exactly as a local pipe would.
async fn pump_output<W: AsyncWrite + Unpin>(
    mut local: W,
    mut receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: mpsc::UnboundedSender<Frame>,
    stream: u32,
) {
    while let Some(bytes) = receiver.recv().await {
        if local.write_all(&bytes).await.is_err() {
            // The reader went away: keep the remote side moving and discard.
            let _ = outbound.send(credit_frame(stream, STREAM_WINDOW));
            continue;
        }
        let length = u32::try_from(bytes.len()).unwrap_or(STREAM_CHUNK);
        if outbound.send(credit_frame(stream, length)).is_err() {
            return;
        }
    }
    let _ = local.shutdown().await;
}

struct TargetProcess {
    core: Arc<Core>,
    process: u32,
    exit: watch::Receiver<ExitState>,
    termination_sent: bool,
}

impl TargetProcess {
    async fn settled(&mut self) -> io::Result<ExitStatus> {
        let state = self
            .exit
            .wait_for(|state| !matches!(state, ExitState::Running))
            .await
            .map_err(|_| lost())?
            .clone();
        match state {
            ExitState::Exited { code, signal } => Ok(exit_status(code, signal)),
            ExitState::Lost | ExitState::Running => Err(lost()),
        }
    }
}

fn lost() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "contact with the target was lost, so the process exit is unconfirmed",
    )
}

impl RemoteProcess for TargetProcess {
    fn id(&self) -> Option<u32> {
        Some(self.process)
    }
    fn wait(&mut self) -> ProcessFuture<'_, ExitStatus> {
        Box::pin(self.settled())
    }
    fn terminate(&mut self) -> ProcessFuture<'_, ()> {
        Box::pin(async move {
            if matches!(*self.exit.borrow(), ExitState::Exited { .. }) {
                return Ok(());
            }
            if !self.termination_sent {
                self.termination_sent = true;
                self.core.send_command(Request::Terminate {
                    process: self.process,
                });
            }
            tokio::time::timeout(TERMINATE_LIMIT, self.settled())
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "remote exit is unconfirmed"))?
                .map(|_| ())
        })
    }
}

impl Drop for TargetProcess {
    /// The same last resort as a local scope. Not reported as a confirmed exit.
    fn drop(&mut self) {
        if !self.termination_sent && matches!(*self.exit.borrow(), ExitState::Running) {
            self.core.send_command(Request::Terminate {
                process: self.process,
            });
        }
    }
}

#[cfg(unix)]
fn exit_status(code: Option<i32>, signal: Option<i32>) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(signal.unwrap_or_else(|| code.unwrap_or(1) << 8))
}

#[cfg(windows)]
fn exit_status(code: Option<i32>, _signal: Option<i32>) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(code.map_or(1, i32::cast_unsigned))
}
