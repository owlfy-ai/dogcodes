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

const COLORS_VARIABLE: &str = "DOGCODES_SHELL_COLORS";
const COLORS_HOOK: &str = r#"eval "$DOGCODES_SHELL_COLORS""#;
/// Many servers, Debian's root account for one, leave a shell without colors. Bash runs
/// this just before its first prompt, after every startup file, so it can fill in only
/// what they left out. It writes nothing on the server and then removes itself. A
/// prompt built by a command is left alone, since it may well draw its own colors.
const BASH_COLORS: &str = r#"
case "$PS1" in
  *'\e'*|*'\E'*|*'\033'*|*'\x1b'*|*$'\e'*|*'$('*|*'`'*) ;;
  *) PS1='${debian_chroot:+($debian_chroot)}\[\e[32m\]\u@\h\[\e[0m\]:\[\e[34m\]\w\[\e[0m\]\$ ' ;;
esac
if ! alias ls >/dev/null 2>&1; then
  if command ls --color=auto / >/dev/null 2>&1; then alias ls='ls --color=auto'
  else export CLICOLOR=1
  fi
fi
unset DOGCODES_SHELL_COLORS
if [ "$PROMPT_COMMAND" = 'eval "$DOGCODES_SHELL_COLORS"' ]; then unset PROMPT_COMMAND
else PROMPT_COMMAND=${PROMPT_COMMAND//'eval "$DOGCODES_SHELL_COLORS"'/:}
fi
"#;

fn shell_request(target: &SharedTarget, command: Option<&str>, cwd: &Path) -> ProcessRequest {
    let shell = target
        .inherited("SHELL")
        // A path on the server, which `Path` would judge by this machine's rules.
        .filter(|shell| shell.to_string_lossy().starts_with('/'));
    let known = shell.as_ref().filter(|shell| {
        matches!(
            Path::new(shell).file_name().and_then(|name| name.to_str()),
            Some("zsh" | "bash" | "sh" | "fish")
        )
    });
    let mut env = target.login_environment();
    env.insert("TERM".into(), "xterm-256color".into());
    let is_bash = known.is_some_and(|shell| {
        Path::new(shell)
            .file_name()
            .is_some_and(|name| name == "bash")
    });
    if command.is_none() && is_bash {
        env.insert(COLORS_VARIABLE.into(), BASH_COLORS.into());
        env.insert("PROMPT_COMMAND".into(), COLORS_HOOK.into());
    }
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
        return Err("backendText.targetTerminalUnavailable".into());
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

#[cfg(all(test, unix))]
mod color_tests {
    use super::{BASH_COLORS, COLORS_HOOK, COLORS_VARIABLE};
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    /// An interactive bash as a server would start it, reporting what it ended up with.
    /// `startup` stands in for the server's startup files.
    fn bash_after_startup(prompt: &str, startup: &str) -> String {
        let directory = tempfile::tempdir().unwrap();
        let rcfile = directory.path().join("rc");
        std::fs::write(&rcfile, startup).unwrap();
        let mut bash = Command::new("bash")
            .args(["--noprofile", "--rcfile"])
            .arg(&rcfile)
            .arg("-i")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("TERM", "xterm-256color")
            .env("PS1", prompt)
            .env(COLORS_VARIABLE, BASH_COLORS)
            .env("PROMPT_COMMAND", COLORS_HOOK)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Read a line at a time, as at a prompt, so the hook has run before these do.
        let report = r#"printf 'ps1=%s\n' "$PS1"
alias ls 2>/dev/null || echo no-ls-alias
printf 'clicolor=%s\n' "${CLICOLOR-}"
printf 'hook=%s\n' "${PROMPT_COMMAND-unset}"
printf 'script=%s\n' "${DOGCODES_SHELL_COLORS-unset}"
exit
"#;
        bash.stdin
            .take()
            .unwrap()
            .write_all(report.as_bytes())
            .unwrap();
        String::from_utf8(bash.wait_with_output().unwrap().stdout).unwrap()
    }

    #[test]
    fn a_shell_without_colors_gets_them_and_the_hook_removes_itself() {
        let report = bash_after_startup(r"\u@\h:\w\$ ", "");
        assert!(report.contains(r"\[\e[32m\]\u@\h"), "{report}");
        assert!(
            report.contains("ls --color=auto") || report.contains("clicolor=1"),
            "{report}"
        );
        assert!(report.contains("hook=unset"), "{report}");
        assert!(report.contains("script=unset"), "{report}");
    }

    #[test]
    fn colors_the_server_chose_are_left_alone() {
        let prompt = r"\[\033[01;31m\]\u\[\033[00m\]\$ ";
        let report = bash_after_startup(prompt, "alias ls='ls -F'\n");
        assert!(report.contains(&format!("ps1={prompt}")), "{report}");
        assert!(report.contains("ls -F"), "{report}");
    }

    #[test]
    fn a_prompt_drawn_by_a_command_is_left_alone() {
        let prompt = "$(my-prompt) ";
        let report = bash_after_startup(prompt, "");
        assert!(report.contains(&format!("ps1={prompt}")), "{report}");
    }

    #[test]
    fn a_prompt_command_from_the_server_keeps_working() {
        let report = bash_after_startup("$ ", "PROMPT_COMMAND=\"history -a;$PROMPT_COMMAND\"\n");
        assert!(report.contains("hook=history -a;:"), "{report}");
        assert!(report.contains("script=unset"), "{report}");
    }
}
