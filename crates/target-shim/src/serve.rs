//! The frame loop and the processes it supervises. Each process sits in its own
//! scope, so its whole group ends when it is terminated or when this shim exits.
use process_scope::OwnedChild;
use std::{
    collections::{BTreeMap, HashMap},
    io,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use target_wire::{
    Channel, Frame, FrameKind, READY_PREFIX, Request, RequestEnvelope, Response, ResponseEnvelope,
    STREAM_CHUNK, STREAM_WINDOW, ShimMessage, SpawnRequest, control_frame, credit_frame,
    data_frame, eof_frame, parse_control, parse_credit, read_frame, split_stream, stream_id,
    write_frame,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::Command,
    sync::{Semaphore, mpsc, oneshot},
};

/// The desktop pings every 15 seconds. A silently lost network can leave stdin
/// open for hours, so the shim also ends on its own when it stops hearing anything.
const IDLE_LIMIT: Duration = Duration::from_secs(90);
const LOGIN_ENVIRONMENT_LIMIT: Duration = Duration::from_secs(10);
const ENVIRONMENT_MARKER: &str = "__DOGCODES_ENVIRONMENT__";
const MAX_ENVIRONMENT_BYTES: usize = 512 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 2000;

type Outbound = mpsc::Sender<Frame>;

struct Supervised {
    stdin: Option<mpsc::UnboundedSender<Vec<u8>>>,
    stdout_credit: Arc<Semaphore>,
    stderr_credit: Arc<Semaphore>,
    terminate: Option<oneshot::Sender<()>>,
}

pub(crate) async fn serve(nonce: &str) -> io::Result<()> {
    let mut output = tokio::io::stdout();
    output
        .write_all(format!("{READY_PREFIX} {nonce}\n").as_bytes())
        .await?;
    output.flush().await?;
    let (outbound, mut pending) = mpsc::channel::<Frame>(64);
    let writer = tokio::spawn(async move {
        while let Some(frame) = pending.recv().await {
            write_frame(&mut output, &frame).await?;
        }
        Ok::<_, io::Error>(())
    });
    let (frames, mut incoming) = mpsc::channel::<io::Result<Option<Frame>>>(64);
    // One owner for the read position: `read_frame` must not be cancelled midway.
    tokio::spawn(async move {
        let mut input = tokio::io::stdin();
        loop {
            let frame = read_frame(&mut input).await;
            let ended = !matches!(frame, Ok(Some(_)));
            if frames.send(frame).await.is_err() || ended {
                return;
            }
        }
    });
    let (exited, mut finished) = mpsc::unbounded_channel::<u32>();
    let mut processes: HashMap<u32, Supervised> = HashMap::new();
    let result = loop {
        tokio::select! {
            frame = incoming.recv() => {
                let frame = match frame {
                    Some(Ok(Some(frame))) => frame,
                    Some(Ok(None)) | None => break Ok(()),
                    Some(Err(error)) => break Err(error),
                };
                if let Err(error) = route(frame, &outbound, &exited, &mut processes).await {
                    break Err(error);
                }
            }
            Some(process) = finished.recv() => {
                processes.remove(&process);
            }
            () = tokio::time::sleep(IDLE_LIMIT) => {
                break Err(io::Error::new(io::ErrorKind::TimedOut, "no message from the desktop"));
            }
        }
    };
    // Nothing keeps this scope alive past the desktop: dropping every supervisor
    // and leaving the runtime ends each owned process group.
    drop(outbound);
    writer.abort();
    result
}

async fn route(
    frame: Frame,
    outbound: &Outbound,
    exited: &mpsc::UnboundedSender<u32>,
    processes: &mut HashMap<u32, Supervised>,
) -> io::Result<()> {
    match frame.kind {
        FrameKind::Control => {
            let envelope: RequestEnvelope = parse_control(&frame)?;
            handle_request(envelope, outbound, exited, processes).await
        }
        FrameKind::Data => {
            if let Some((process, Channel::Stdin)) = split_stream(frame.stream)
                && let Some(stdin) = processes.get(&process).and_then(|p| p.stdin.as_ref())
            {
                let _ = stdin.send(frame.payload);
            }
            Ok(())
        }
        FrameKind::Eof => {
            if let Some((process, Channel::Stdin)) = split_stream(frame.stream)
                && let Some(entry) = processes.get_mut(&process)
            {
                entry.stdin = None;
            }
            Ok(())
        }
        FrameKind::Credit => {
            let bytes = parse_credit(&frame)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid credit"))?;
            if let Some((process, channel)) = split_stream(frame.stream)
                && let Some(entry) = processes.get(&process)
            {
                match channel {
                    Channel::Stdout => entry.stdout_credit.add_permits(bytes as usize),
                    Channel::Stderr => entry.stderr_credit.add_permits(bytes as usize),
                    Channel::Stdin => {}
                }
            }
            Ok(())
        }
    }
}

async fn handle_request(
    envelope: RequestEnvelope,
    outbound: &Outbound,
    exited: &mpsc::UnboundedSender<u32>,
    processes: &mut HashMap<u32, Supervised>,
) -> io::Result<()> {
    let id = envelope.id;
    match envelope.request {
        Request::Ping => respond(outbound, id, Response::Pong).await,
        Request::Info => respond(outbound, id, Response::Info(crate::target_info())).await,
        Request::LoginEnvironment => {
            // Slow shell profiles must not hold up the channel or its heartbeat.
            let outbound = outbound.clone();
            tokio::spawn(async move {
                let values = login_environment().await;
                let _ = respond(&outbound, id, Response::Environment { values }).await;
            });
            Ok(())
        }
        Request::Spawn(request) => {
            let process = request.process;
            match start(request, outbound.clone(), exited.clone()) {
                Ok(supervised) => {
                    processes.insert(process, supervised);
                }
                Err(error) => report_failed_start(outbound, process, &error).await?,
            }
            respond(outbound, id, Response::Ok).await
        }
        Request::ListDirectory { path } => {
            let response = match list_directory(&path).await {
                Ok(listing) => listing,
                Err(error) => Response::Error {
                    message: error.to_string(),
                },
            };
            respond(outbound, id, response).await
        }
        Request::Terminate { process } => {
            if let Some(terminate) = processes.get_mut(&process).and_then(|p| p.terminate.take()) {
                let _ = terminate.send(());
            }
            respond(outbound, id, Response::Ok).await
        }
    }
}

async fn respond(outbound: &Outbound, id: u64, response: Response) -> io::Result<()> {
    send_message(
        outbound,
        &ShimMessage::Response(ResponseEnvelope { id, response }),
    )
    .await
}

async fn send_message(outbound: &Outbound, message: &ShimMessage) -> io::Result<()> {
    outbound
        .send(control_frame(message)?)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
}

async fn report_failed_start(
    outbound: &Outbound,
    process: u32,
    error: &io::Error,
) -> io::Result<()> {
    for channel in [Channel::Stdout, Channel::Stderr] {
        outbound
            .send(eof_frame(stream_id(process, channel)))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
    }
    send_message(
        outbound,
        &ShimMessage::Exited {
            process,
            code: Some(if error.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            }),
            signal: None,
            error: Some(error.to_string()),
        },
    )
    .await
}

fn start(
    request: SpawnRequest,
    outbound: Outbound,
    exited: mpsc::UnboundedSender<u32>,
) -> io::Result<Supervised> {
    let process = request.process;
    let pipe = |piped: bool| {
        if piped { Stdio::piped() } else { Stdio::null() }
    };
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .env_clear()
        .envs(&request.env)
        .stdin(pipe(request.stdin))
        .stdout(pipe(request.stdout))
        .stderr(pipe(request.stderr));
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    let mut child = OwnedChild::spawn(command)?;

    let stdout_credit = Arc::new(Semaphore::new(STREAM_WINDOW as usize));
    let stderr_credit = Arc::new(Semaphore::new(STREAM_WINDOW as usize));
    let stdin_sender = child.stdin.take().map(|stdin| {
        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(pump_stdin(
            stdin,
            receiver,
            outbound.clone(),
            stream_id(process, Channel::Stdin),
        ));
        sender
    });
    let mut pumps = Vec::new();
    for (reader, channel, credit) in [
        (child.stdout.take(), Channel::Stdout, &stdout_credit),
        (child.stderr.take(), Channel::Stderr, &stderr_credit),
    ] {
        let stream = stream_id(process, channel);
        if let Some(reader) = reader {
            pumps.push(tokio::spawn(pump_output(
                reader,
                outbound.clone(),
                stream,
                credit.clone(),
            )));
        } else {
            // Never piped: the desktop's reader still needs its end of stream.
            let outbound = outbound.clone();
            pumps.push(tokio::spawn(async move {
                let _ = outbound.send(eof_frame(stream)).await;
            }));
        }
    }
    let (terminate, terminated) = oneshot::channel();
    tokio::spawn(async move {
        let status = supervise(&mut child, terminated).await;
        // Output first, so "exited" means nothing more will arrive. A surviving
        // holder of the pipe cannot delay the proof: the group has already ended.
        for pump in pumps {
            let _ = tokio::time::timeout(Duration::from_secs(1), pump).await;
        }
        let message = exit_message(process, status);
        let _ = send_message(&outbound, &message).await;
        let _ = exited.send(process);
    });
    Ok(Supervised {
        stdin: stdin_sender,
        stdout_credit,
        stderr_credit,
        terminate: Some(terminate),
    })
}

async fn supervise(
    child: &mut OwnedChild,
    mut terminated: oneshot::Receiver<()>,
) -> io::Result<std::process::ExitStatus> {
    tokio::select! {
        status = child.wait() => status,
        _ = &mut terminated => {
            child.terminate().await?;
            child.wait().await
        }
    }
}

fn exit_message(process: u32, status: io::Result<std::process::ExitStatus>) -> ShimMessage {
    match status {
        Ok(status) => {
            #[cfg(unix)]
            let signal = std::os::unix::process::ExitStatusExt::signal(&status);
            #[cfg(not(unix))]
            let signal = None;
            ShimMessage::Exited {
                process,
                code: status.code(),
                signal,
                error: None,
            }
        }
        Err(error) => ShimMessage::Exited {
            process,
            code: None,
            signal: None,
            error: Some(error.to_string()),
        },
    }
}

/// Credit is returned only after the bytes reached the process, so a process that
/// stops reading its input also stops the desktop's writer. A process that has
/// gone keeps being credited: nobody should wait on input it will never read.
async fn pump_stdin<W: AsyncWrite + Unpin>(
    mut stdin: W,
    mut receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: Outbound,
    stream: u32,
) {
    let mut broken = false;
    while let Some(bytes) = receiver.recv().await {
        if !broken {
            broken = stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err();
        }
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        if outbound.send(credit_frame(stream, length)).await.is_err() {
            return;
        }
    }
    let _ = stdin.shutdown().await;
}

async fn pump_output<R: AsyncRead + Unpin>(
    mut reader: R,
    outbound: Outbound,
    stream: u32,
    credit: Arc<Semaphore>,
) {
    let mut buffer = vec![0; STREAM_CHUNK as usize];
    loop {
        let Ok(permit) = credit.acquire_many(STREAM_CHUNK).await else {
            return;
        };
        permit.forget();
        let count = reader.read(&mut buffer).await.unwrap_or(0);
        credit.add_permits(STREAM_CHUNK as usize - count);
        if count == 0 {
            let _ = outbound.send(eof_frame(stream)).await;
            return;
        }
        if outbound
            .send(data_frame(stream, buffer[..count].to_vec()))
            .await
            .is_err()
        {
            return;
        }
    }
}

/// The resolved path comes back so the desktop stores what actually exists.
async fn list_directory(path: &str) -> io::Result<Response> {
    let resolved = tokio::fs::canonicalize(path).await?;
    let mut entries = tokio::fs::read_dir(&resolved).await?;
    let mut directories = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if directories.len() >= MAX_DIRECTORY_ENTRIES {
            break;
        }
        // Follows symbolic links: a link to a directory is a place one can open.
        if tokio::fs::metadata(entry.path())
            .await
            .is_ok_and(|metadata| metadata.is_dir())
            && let Ok(name) = entry.file_name().into_string()
        {
            directories.push(name);
        }
    }
    directories.sort_unstable();
    Ok(Response::Directory {
        path: resolved.to_string_lossy().into_owned(),
        directories,
    })
}

/// `sshd` commands usually skip the files that set `PATH` for version managers and
/// `~/.local/bin`, so this asks the login shell itself. A failure falls back to the
/// environment the shim started with.
async fn login_environment() -> BTreeMap<String, String> {
    let fallback = || std::env::vars().collect::<BTreeMap<_, _>>();
    let Some(shell) = std::env::var_os("SHELL").filter(|shell| !shell.is_empty()) else {
        return fallback();
    };
    let script = format!("printf '\\n{ENVIRONMENT_MARKER}\\n'; env -0");
    let mut command = Command::new(shell);
    command
        .args(["-lic", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(LOGIN_ENVIRONMENT_LIMIT, command.output()).await {
        Ok(Ok(output)) => {
            let values = parse_environment(&output.stdout);
            if values.is_empty() {
                fallback()
            } else {
                values
            }
        }
        _ => fallback(),
    }
}

/// Everything before the marker is whatever the shell's startup files printed.
fn parse_environment(output: &[u8]) -> BTreeMap<String, String> {
    let marker = format!("{ENVIRONMENT_MARKER}\n");
    let Some(start) = output
        .windows(marker.len())
        .rposition(|window| window == marker.as_bytes())
    else {
        return BTreeMap::new();
    };
    let mut total = 0;
    output[start + marker.len()..]
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter_map(|entry| entry.split_once('='))
        .take_while(|(name, value)| {
            total += name.len() + value.len();
            total <= MAX_ENVIRONMENT_BYTES
        })
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_noise_before_the_marker_is_ignored() {
        let mut output = b"welcome\nPATH=/wrong\0\n".to_vec();
        output.extend_from_slice(format!("\n{ENVIRONMENT_MARKER}\n").as_bytes());
        output.extend_from_slice(b"PATH=/right:/bin\0HOME=/home/dev\0MULTI=a\nb\0");
        let values = parse_environment(&output);
        assert_eq!(values["PATH"], "/right:/bin");
        assert_eq!(values["HOME"], "/home/dev");
        assert_eq!(values["MULTI"], "a\nb");
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn output_without_the_marker_yields_nothing() {
        assert!(parse_environment(b"PATH=/x\0").is_empty());
    }

    #[test]
    fn an_oversized_environment_is_cut_short_instead_of_exceeding_a_frame() {
        let mut output = format!("{ENVIRONMENT_MARKER}\n").into_bytes();
        for index in 0..10 {
            output.extend_from_slice(format!("V{index}=").as_bytes());
            output.extend_from_slice(&vec![b'x'; 100 * 1024]);
            output.push(0);
        }
        let values = parse_environment(&output);
        assert!(values.len() < 10);
        assert!(serde_json::to_vec(&values).unwrap().len() < target_wire::MAX_PAYLOAD);
    }

    fn request(script: &str) -> SpawnRequest {
        SpawnRequest {
            process: 1,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: BTreeMap::new(),
            cwd: None,
            stdin: true,
            stdout: true,
            stderr: true,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_is_held_back_until_the_desktop_grants_credit() {
        let (outbound, mut frames) = mpsc::channel(1024);
        let (exited, _finished) = mpsc::unbounded_channel();
        // Far more than one window, on a process nobody acknowledges.
        let supervised = start(request("head -c 2000000 /dev/zero"), outbound, exited).unwrap();
        let mut received = 0_usize;
        while let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(300), frames.recv()).await
        {
            if frame.kind == FrameKind::Data && frame.stream == stream_id(1, Channel::Stdout) {
                received += frame.payload.len();
            }
        }
        assert!(received <= STREAM_WINDOW as usize, "{received}");
        assert!(received > 0);
        supervised.stdout_credit.add_permits(STREAM_WINDOW as usize);
        let mut more = 0_usize;
        while let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(300), frames.recv()).await
        {
            if frame.kind == FrameKind::Data && frame.stream == stream_id(1, Channel::Stdout) {
                more += frame.payload.len();
            }
        }
        assert!(more > 0);
    }
}
