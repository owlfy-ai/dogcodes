//! A terminal on another machine, presented through the same traits as a local one so
//! the host's reading, stopping and recording stay in one place. Losing contact never
//! reads as an exit: the wait reports an error, which the host records as unknown.
use super::Started;
use agent_runtime::{OwnedChild, Pipe, ProcessRequest, SharedTarget};
use portable_pty::{Child, ChildKiller, ExitStatus, MasterPty, PtySize};
use std::{
    fmt,
    io::{self, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::Handle,
    sync::mpsc,
};

const WAIT_INTERVAL: Duration = Duration::from_millis(10);

fn shell_request(target: &SharedTarget, command: Option<&str>, cwd: &Path) -> ProcessRequest {
    let shell = target
        .inherited("SHELL")
        .filter(|shell| Path::new(shell).is_absolute());
    let known = shell.as_ref().filter(|shell| {
        matches!(
            Path::new(shell).file_name().and_then(|name| name.to_str()),
            Some("zsh" | "bash" | "sh" | "fish")
        )
    });
    let mut env = target.login_environment();
    env.insert("TERM".into(), "xterm-256color".into());
    let (program, args): (std::ffi::OsString, Vec<std::ffi::OsString>) = match (command, known) {
        (None, _) => (
            shell.clone().unwrap_or_else(|| "/bin/sh".into()),
            vec!["-l".into()],
        ),
        (Some(command), Some(shell)) => {
            env.insert("DOGCODES_RUN_CWD".into(), cwd.as_os_str().to_owned());
            env.insert("DOGCODES_RUN_COMMAND".into(), command.into());
            (
                shell.clone(),
                vec![
                    "-lic".into(),
                    "exec /bin/sh -c 'cd \"$DOGCODES_RUN_CWD\" && eval \"$DOGCODES_RUN_COMMAND\"'"
                        .into(),
                ],
            )
        }
        (Some(command), None) => ("/bin/sh".into(), vec!["-lc".into(), command.into()]),
    };
    ProcessRequest {
        program,
        args,
        env,
        cwd: Some(cwd.to_owned()),
        stdin: Pipe::Piped,
        stdout: Pipe::Piped,
        stderr: Pipe::Null,
    }
}

pub(super) fn start(
    target: &SharedTarget,
    command: Option<&str>,
    cwd: &Path,
    dimensions: PtySize,
) -> Result<Started, String> {
    let runtime = Handle::current();
    let _entered = runtime.enter();
    let started = target
        .spawn_terminal(
            shell_request(target, command, cwd),
            dimensions.cols,
            dimensions.rows,
        )
        .map_err(|error| error.to_string())?;
    let mut child = started.child;
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("终端连接不可用".into());
    };
    Ok(Started {
        master: Box::new(RemoteMaster {
            size: Mutex::new(dimensions),
            resize: started.resize,
        }),
        child: Box::new(RemoteChild::watch(child, &runtime)),
        reader: Box::new(BlockingReader {
            runtime: runtime.clone(),
            stream: output,
        }),
        writer: Box::new(BlockingWriter {
            runtime,
            stream: input,
        }),
    })
}

struct RemoteMaster {
    size: Mutex<PtySize>,
    resize: Box<dyn Fn(u16, u16) -> io::Result<()> + Send + Sync>,
}

impl MasterPty for RemoteMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        (self.resize)(size.cols, size.rows)?;
        *self
            .size
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = size;
        Ok(())
    }
    fn get_size(&self) -> anyhow::Result<PtySize> {
        Ok(*self
            .size
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }
    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        anyhow::bail!("the output of a remote terminal has one reader")
    }
    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        anyhow::bail!("the input of a remote terminal has one writer")
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> {
        None
    }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<i32> {
        None
    }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

type Outcome = Arc<Mutex<Option<Result<u32, String>>>>;

struct RemoteChild {
    outcome: Outcome,
    kill: mpsc::UnboundedSender<()>,
}

impl fmt::Debug for RemoteChild {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteChild")
    }
}

impl RemoteChild {
    fn watch(mut child: OwnedChild, runtime: &Handle) -> Self {
        let outcome: Outcome = Arc::default();
        let recorded = outcome.clone();
        let (kill, mut killed) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = killed.recv() => match child.terminate().await {
                    Ok(()) => child.wait().await,
                    Err(error) => Err(error),
                },
            };
            let value = match status {
                Ok(status) => Ok(status
                    .code()
                    .and_then(|code| u32::try_from(code).ok())
                    .unwrap_or(1)),
                Err(error) => Err(error.to_string()),
            };
            *recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
        });
        Self { outcome, kill }
    }
}

#[derive(Clone)]
struct Killer(mpsc::UnboundedSender<()>);

impl fmt::Debug for Killer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteKiller")
    }
}

impl ChildKiller for Killer {
    fn kill(&mut self) -> io::Result<()> {
        // A closed channel means the outcome is already known.
        let _ = self.0.send(());
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl ChildKiller for RemoteChild {
    fn kill(&mut self) -> io::Result<()> {
        let _ = self.kill.send(());
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Killer(self.kill.clone()))
    }
}

impl Child for RemoteChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let outcome = self
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match outcome.as_ref() {
            None => Ok(None),
            Some(Ok(code)) => Ok(Some(ExitStatus::with_exit_code(*code))),
            Some(Err(error)) => Err(io::Error::other(error.clone())),
        }
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(WAIT_INTERVAL);
        }
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

struct BlockingReader {
    runtime: Handle,
    stream: agent_runtime::ProcessStream,
}

impl Read for BlockingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.runtime.block_on(self.stream.read(buffer))
    }
}

struct BlockingWriter {
    runtime: Handle,
    stream: agent_runtime::ProcessInput,
}

impl Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.runtime.block_on(self.stream.write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.runtime.block_on(self.stream.flush())
    }
}
