//! A process under its own terminal, for interactive shells and project commands.
//! Reads, writes and the wait block, so each has its own thread and only bytes meet
//! the frame loop. The session ends with this shim: losing the master hangs it up,
//! and a held `Terminal` also signals the group directly.
use crate::serve::{Outbound, send_message};
use nix::{sys::signal::Signal, unistd::Pid};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use target_wire::{
    Channel, STREAM_CHUNK, STREAM_WINDOW, ShimMessage, SpawnRequest, credit_frame, data_frame,
    eof_frame, stream_id,
};
use tokio::sync::{Semaphore, mpsc, oneshot};

const QUEUED_CHUNKS: usize = 2;

pub(crate) struct Terminal {
    master: Box<dyn MasterPty + Send>,
    group: Option<Pid>,
    running: Arc<AtomicBool>,
}

impl Terminal {
    pub(crate) fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(size(cols, rows));
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.running.load(Ordering::Acquire) {
            kill_group(self.group);
        }
    }
}

pub(crate) struct Parts {
    pub stdin: mpsc::UnboundedSender<Vec<u8>>,
    pub credit: Arc<Semaphore>,
    pub terminate: oneshot::Sender<()>,
    pub terminal: Terminal,
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn kill_group(group: Option<Pid>) {
    if let Some(group) = group {
        let _ = nix::sys::signal::killpg(group, Signal::SIGKILL);
    }
}

fn failed(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub(crate) fn start(
    request: SpawnRequest,
    outbound: Outbound,
    exited: mpsc::UnboundedSender<u32>,
) -> io::Result<Parts> {
    let process = request.process;
    let dimensions = request
        .terminal
        .ok_or_else(|| failed("a terminal size is required"))?;
    let pair = native_pty_system()
        .openpty(size(dimensions.cols, dimensions.rows))
        .map_err(failed)?;
    let mut command = CommandBuilder::new(&request.program);
    command.args(&request.args);
    command.env_clear();
    for (name, value) in &request.env {
        command.env(name, value);
    }
    if let Some(cwd) = &request.cwd {
        // A missing directory would otherwise fall back to the home directory.
        if !std::path::Path::new(cwd).is_dir() {
            return Err(io::ErrorKind::NotFound.into());
        }
        command.cwd(cwd);
    }
    let reader = pair.master.try_clone_reader().map_err(failed)?;
    let writer = pair.master.take_writer().map_err(failed)?;
    let mut child =
        pair.slave
            .spawn_command(command)
            .map_err(|error| match error.downcast::<io::Error>() {
                Ok(error) => error,
                Err(error) => failed(error),
            })?;
    drop(pair.slave);
    let group = child
        .process_id()
        .and_then(|pid| i32::try_from(pid).ok())
        .map(Pid::from_raw);
    let running = Arc::new(AtomicBool::new(true));

    let (stdin, input) = mpsc::unbounded_channel();
    pump_input(
        writer,
        input,
        outbound.clone(),
        stream_id(process, Channel::Stdin),
    );
    let credit = Arc::new(Semaphore::new(STREAM_WINDOW as usize));
    let output = pump_output(
        reader,
        outbound.clone(),
        stream_id(process, Channel::Stdout),
        credit.clone(),
    );
    let stderr = eof_frame(stream_id(process, Channel::Stderr));

    let (status, finished) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = status.send(child.wait());
    });
    let (terminate, mut terminated) = oneshot::channel::<()>();
    let held = running.clone();
    tokio::spawn(async move {
        let _ = outbound.send(stderr).await;
        tokio::pin!(finished);
        let status = tokio::select! {
            status = &mut finished => status,
            _ = &mut terminated => {
                kill_group(group);
                (&mut finished).await
            }
        };
        held.store(false, Ordering::Release);
        // Output first, as for any process. A background job that kept the terminal
        // open cannot delay the proof that the process itself has ended.
        let _ = tokio::time::timeout(Duration::from_secs(1), output).await;
        let (code, error) = match status {
            Ok(Ok(status)) => (i32::try_from(status.exit_code()).ok(), None),
            Ok(Err(error)) => (None, Some(error.to_string())),
            Err(_) => (None, Some("the terminal stopped being watched".to_owned())),
        };
        let _ = send_message(
            &outbound,
            &ShimMessage::Exited {
                process,
                code,
                signal: None,
                error,
            },
        )
        .await;
        let _ = exited.send(process);
    });
    Ok(Parts {
        stdin,
        credit,
        terminate,
        terminal: Terminal {
            master: pair.master,
            group,
            running,
        },
    })
}

/// Credit returns once the terminal took the bytes, so a program that stops reading
/// also stops the desktop's writer. After a failure input is only acknowledged.
fn pump_input(
    mut writer: Box<dyn Write + Send>,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    outbound: Outbound,
    stream: u32,
) {
    std::thread::spawn(move || {
        let mut broken = false;
        while let Some(bytes) = input.blocking_recv() {
            if !broken {
                broken = writer.write_all(&bytes).is_err() || writer.flush().is_err();
            }
            let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
            if outbound
                .blocking_send(credit_frame(stream, length))
                .is_err()
            {
                return;
            }
        }
    });
}

fn pump_output(
    mut reader: Box<dyn Read + Send>,
    outbound: Outbound,
    stream: u32,
    credit: Arc<Semaphore>,
) -> tokio::task::JoinHandle<()> {
    let (bytes, mut received) = mpsc::channel::<Vec<u8>>(QUEUED_CHUNKS);
    std::thread::spawn(move || {
        let mut buffer = vec![0; STREAM_CHUNK as usize];
        // A closed terminal reads as an error on Linux and as end of file elsewhere.
        while let Ok(count @ 1..) = reader.read(&mut buffer) {
            if bytes.blocking_send(buffer[..count].to_vec()).is_err() {
                return;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(chunk) = received.recv().await {
            let length = u32::try_from(chunk.len()).unwrap_or(STREAM_CHUNK);
            let Ok(permit) = credit.acquire_many(length).await else {
                return;
            };
            permit.forget();
            if outbound.send(data_frame(stream, chunk)).await.is_err() {
                return;
            }
        }
        let _ = outbound.send(eof_frame(stream)).await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use target_wire::{Frame, FrameKind, TerminalSize};

    fn request(script: &str) -> SpawnRequest {
        SpawnRequest {
            process: 1,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: BTreeMap::from([("MARK".into(), "seen".into())]),
            cwd: None,
            stdin: true,
            stdout: true,
            stderr: false,
            terminal: Some(TerminalSize { cols: 91, rows: 17 }),
            operation: false,
        }
    }

    async fn output_until(frames: &mut mpsc::Receiver<Frame>, text: &str) -> String {
        let mut output = String::new();
        while !output.contains(text) {
            let frame = tokio::time::timeout(Duration::from_secs(10), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if frame.kind == FrameKind::Data && frame.stream == stream_id(1, Channel::Stdout) {
                output.push_str(&String::from_utf8_lossy(&frame.payload));
            }
        }
        output
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_program_sees_a_terminal_of_the_requested_size_and_only_named_values() {
        let (outbound, mut frames) = mpsc::channel(64);
        let (exited, _finished) = mpsc::unbounded_channel();
        let _parts = start(
            request("[ -t 0 ] && stty size; echo \"$MARK:${HOME-none}\"; echo done"),
            outbound,
            exited,
        )
        .unwrap();
        let output = output_until(&mut frames, "done").await;
        assert!(output.contains("17 91"), "{output}");
        assert!(output.contains("seen:none"), "{output}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn input_resize_and_termination_reach_the_process_group() {
        let (outbound, mut frames) = mpsc::channel(64);
        let (exited, mut finished) = mpsc::unbounded_channel();
        let parts = start(
            request("sleep 60 & read line; stty size; echo \"got:$line\"; wait"),
            outbound,
            exited,
        )
        .unwrap();
        parts.terminal.resize(120, 40);
        parts.stdin.send(b"hello\n".to_vec()).unwrap();
        let output = output_until(&mut frames, "got:hello").await;
        assert!(output.contains("40 120"), "{output}");
        parts.terminate.send(()).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(10), finished.recv()).await;
        assert_eq!(ended.unwrap(), Some(1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_directory_is_reported_instead_of_starting_elsewhere() {
        let (outbound, _frames) = mpsc::channel(64);
        let (exited, _finished) = mpsc::unbounded_channel();
        let mut missing = request("true");
        missing.cwd = Some("/nonexistent/dogcodes".into());
        let error = start(missing, outbound, exited).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
