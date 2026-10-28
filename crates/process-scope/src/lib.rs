//! A subprocess scope whose shutdown proof includes its owned OS process group/job.
use std::{io, process::ExitStatus, time::Duration};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub struct OwnedChild {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    child: Child,
    #[cfg(unix)]
    pid: nix::unistd::Pid,
    #[cfg(unix)]
    _parent_lifetime: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    job: windows::Job,
    signalled: bool,
    status: Option<ExitStatus>,
    confirmed: bool,
}
impl OwnedChild {
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        Self::spawn_owned(command)
    }
    /// Start the actual CLI as a controlling-terminal session/group leader.
    /// Unlike a `script` wrapper, this preserves its own PID/group reservation
    /// and the existing host-lifetime guard through all terminal I/O and exit.
    #[cfg(unix)]
    pub fn spawn_terminal(
        mut command: Command,
        columns: u16,
        rows: u16,
    ) -> io::Result<(Self, std::fs::File, std::fs::File)> {
        command.kill_on_drop(true);
        let (input, output) = unix::terminal(&mut command, columns, rows)?;
        Ok((Self::spawn_owned(command)?, input, output))
    }
    fn spawn_owned(mut command: Command) -> io::Result<Self> {
        #[cfg(unix)]
        let (parent_lifetime, _reader) = unix::attach(&mut command)?;
        #[cfg(windows)]
        command.creation_flags(0x0800_0000 | 0x0000_0004); // no window, suspended until job assignment
        let mut child = command.spawn()?;
        #[cfg(unix)]
        let pid = nix::unistd::Pid::from_raw(
            i32::try_from(
                child
                    .id()
                    .ok_or_else(|| io::Error::other("missing child id"))?,
            )
            .map_err(io::Error::other)?,
        );
        #[cfg(windows)]
        let job = windows::Job::assign(&mut child)?;
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            child,
            #[cfg(unix)]
            pid,
            #[cfg(unix)]
            _parent_lifetime: parent_lifetime,
            #[cfg(windows)]
            job,
            signalled: false,
            status: None,
            confirmed: false,
        })
    }
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    /// Collect bounded output under supervision of the original process tree.
    /// Output and read errors are delivered after confirmed retirement. Once
    /// collection starts, observer cancellation continues that shutdown in the
    /// background. Supervisor loss is an error, never a positive exit proof;
    /// Drop remains only the last-resort kill request.
    pub async fn wait_with_output(mut self, limit: usize) -> io::Result<std::process::Output> {
        let (mut reply, result) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            drop(self.stdin.take());
            let stdout = self.stdout.take();
            let stderr = self.stderr.take();
            let outcome = tokio::select! {
                biased;
                () = reply.closed() => None,
                output = async {
                    let (status, stdout, stderr) = tokio::try_join!(
                        self.wait(),
                        read_output(stdout, limit),
                        read_output(stderr, limit)
                    )?;
                    Ok(std::process::Output { status, stdout, stderr })
                } => Some(output),
            };
            // Reading can fail before the process ends. Retain its exact
            // group/job and PID reservation even if the caller stops waiting.
            // Retry only shutdown observation, never the original command.
            let mut delay = 1;
            while self.terminate().await.is_err() {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
            }
            if let Some(outcome) = outcome {
                let _ = reply.send(outcome);
            }
        });
        result.await.map_err(|_| {
            io::Error::other("managed output worker ended without confirmed retirement")
        })?
    }
    /// Wait naturally. On Unix, observe the leader without reaping it, retaining
    /// the PID/group reservation until remaining group members have been signalled.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if self.confirmed {
                return self
                    .status
                    .ok_or_else(|| io::Error::other("missing exit status"));
            }
            #[cfg(unix)]
            if self.signalled {
                self.terminate().await?;
                continue;
            }
            #[cfg(unix)]
            if self.leader_exited()? {
                self.terminate().await?;
                continue;
            }
            #[cfg(windows)]
            if self.status.is_none()
                && let Some(status) = self.child.try_wait()?
            {
                self.status = Some(status);
                self.terminate().await?;
                continue;
            }
            #[cfg(windows)]
            if self.job.empty()? {
                self.status = Some(self.child.wait().await?);
                self.confirmed = true;
                continue;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    /// Cancellation-safe: a subsequent caller continues the same termination,
    /// never resends a Unix signal after the leader has been reaped.
    pub async fn terminate(&mut self) -> io::Result<()> {
        if self.confirmed {
            return Ok(());
        }
        #[cfg(unix)]
        {
            if !self.signalled {
                self.signal(Some(nix::sys::signal::Signal::SIGTERM))?;
                // If cancelled during grace, the next call repeats only TERM;
                // no call has reaped the leader or released its identity.
                tokio::time::sleep(Duration::from_millis(300)).await;
                self.signal(Some(nix::sys::signal::Signal::SIGKILL))?;
                self.signalled = true;
            }
            if self.status.is_none() {
                self.status = Some(
                    tokio::time::timeout(Duration::from_secs(2), self.child.wait())
                        .await
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::TimedOut, "child reap pending")
                        })??,
                );
            }
            tokio::time::timeout(Duration::from_secs(2), async {
                while self.signal(None)? {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Ok::<_, io::Error>(())
            })
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "process group exit unconfirmed")
            })??;
        }
        #[cfg(windows)]
        {
            if !self.signalled {
                self.job.terminate()?;
                self.signalled = true;
            }
            tokio::time::timeout(Duration::from_secs(3), async {
                while !self.job.empty()? {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Ok::<_, io::Error>(())
            })
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "job exit unconfirmed"))??;
            self.status = Some(self.child.wait().await?);
        }
        self.confirmed = true;
        Ok(())
    }
    #[cfg(unix)]
    fn signal(&self, signal: Option<nix::sys::signal::Signal>) -> io::Result<bool> {
        match nix::sys::signal::killpg(self.pid, signal) {
            // Darwin can report EPERM for an exited, unreaped group leader.
            // This is presence/uncertainty, never proof that the group stopped.
            Ok(()) | Err(nix::errno::Errno::EPERM) => Ok(true),
            Err(nix::errno::Errno::ESRCH) => Ok(false),
            Err(e) => Err(io::Error::from(e)),
        }
    }
    #[cfg(unix)]
    fn leader_exited(&self) -> io::Result<bool> {
        use nix::libc;
        // SAFETY: a zeroed siginfo is writable and lives across waitid. P_PID is
        // our unreaped child only; WNOWAIT preserves its PID reservation.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let pid = libc::id_t::try_from(self.pid.as_raw()).map_err(io::Error::other)?;
        // SAFETY: valid owned PID, initialized siginfo, documented non-reaping flags.
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        Ok(info.si_signo != 0)
    }
}
async fn read_output<R: tokio::io::AsyncRead + Unpin>(
    reader: Option<R>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader
            .take(u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "process output budget exceeded",
            ));
        }
    }
    Ok(bytes)
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if !self.signalled && self.status.is_none() {
            let _ = self.signal(Some(nix::sys::signal::Signal::SIGKILL));
        }
        // Windows Job is kill-on-close; Child is also kill-on-drop. Neither
        // fallback is reported as a successful asynchronous shutdown proof.
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    fn spawn(script: &str) -> OwnedChild {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        OwnedChild::spawn(command).unwrap()
    }
    async fn child_pid(child: &mut OwnedChild) -> u32 {
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        line.trim().parse().unwrap()
    }
    async fn gone(pid: u32) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                match nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                    None,
                ) {
                    Err(nix::errno::Errno::ESRCH) => break,
                    Ok(()) | Err(nix::errno::Errno::EPERM) => {
                        tokio::time::sleep(Duration::from_millis(20)).await
                    }
                    Err(error) => panic!("process presence check failed: {error}"),
                }
            }
        })
        .await
        .expect("owned process must exit");
    }
    #[tokio::test]
    async fn bounded_output_preserves_exit_status_and_stops_an_over_budget_scope() {
        let child = spawn("printf stdout; printf stderr >&2; exit 9");
        let output = child.wait_with_output(128).await.unwrap();
        assert_eq!(output.status.code(), Some(9));
        assert_eq!(output.stdout, b"stdout");
        // spawn() intentionally discards stderr; absent pipes stay empty.
        assert!(output.stderr.is_empty());

        let child = spawn("while :; do printf 'too much output'; done");
        let pid = child.id().unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output(128))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        gone(pid).await;
    }
    fn present(pid: u32) -> bool {
        match nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None,
        ) {
            Err(nix::errno::Errno::ESRCH) => false,
            Ok(()) | Err(nix::errno::Errno::EPERM) => true,
            result => panic!("private process presence unconfirmed: {result:?}"),
        }
    }

    async fn output_fixture(stream: &str) -> (OwnedChild, u32) {
        use tokio::io::AsyncBufReadExt;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", r#"trap '' TERM; sleep 60 & echo $!; read start; while :; do if [ "$1" = stderr ]; then printf 'output above budget' >&2; else printf 'output above budget'; fi; done"#, "scope-fixture", stream])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = OwnedChild::spawn(command).unwrap();
        // The shell blocks on stdin after this header, so restoring the raw
        // stdout loses no buffered payload. Collection closes stdin to proceed.
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut header = String::new();
        reader.read_line(&mut header).await.unwrap();
        child.stdout = Some(reader.into_inner());
        (child, header.trim().parse().unwrap())
    }

    #[tokio::test]
    async fn output_budget_failure_is_not_returned_before_the_original_group_is_gone() {
        for stream in ["stdout", "stderr"] {
            let (child, descendant) = output_fixture(stream).await;
            let leader = child.id().unwrap();
            let error = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output(128))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(
                !present(leader),
                "output failure returned before original leader was reaped"
            );
            assert!(
                !present(descendant),
                "output failure returned before descendant exit was confirmed"
            );
        }
    }

    #[tokio::test]
    async fn cancelled_output_collection_still_retires_its_tree_and_preserves_an_unrelated_process()
    {
        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "trap '' TERM; sleep 60 & echo $!; read start; wait"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = OwnedChild::spawn(command).unwrap();
        let leader = child.id().unwrap();
        let descendant = child_pid(&mut child).await;
        let mut collecting = tokio::spawn(child.wait_with_output(128));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut collecting)
                .await
                .is_err()
        );
        collecting.abort();
        assert!(collecting.await.unwrap_err().is_cancelled());
        gone(leader).await;
        gone(descendant).await;
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().await.unwrap();
        unrelated.wait().await.unwrap();
    }

    #[tokio::test]
    async fn natural_exit_keeps_its_status_and_shutdown_is_idempotent() {
        let mut child = spawn("exit 7");
        assert_eq!(child.wait().await.unwrap().code(), Some(7));
        child.terminate().await.unwrap();
        assert_eq!(child.wait().await.unwrap().code(), Some(7));
    }
    #[tokio::test]
    async fn cancelled_termination_resumes_the_owned_tree_and_leaves_a_sibling_running() {
        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut child = spawn("trap '' TERM; sleep 60 & echo $!; wait");
        let descendant = child_pid(&mut child).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), child.terminate())
                .await
                .is_err()
        );
        child.terminate().await.unwrap();
        gone(descendant).await;
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().await.unwrap();
        unrelated.wait().await.unwrap();
    }
    #[tokio::test]
    async fn an_exited_leader_does_not_leave_an_inherited_pipe_process_behind() {
        let mut child = spawn("sleep 60 & echo $!; exit 0");
        let descendant = child_pid(&mut child).await;
        assert!(child.wait().await.unwrap().success());
        gone(descendant).await;
    }
    #[tokio::test]
    async fn dropping_the_scope_requests_kill_for_the_owned_group() {
        let mut child = spawn("sleep 60 & echo $!; wait");
        let descendant = child_pid(&mut child).await;
        let leader = child.id().unwrap();
        drop(child);
        gone(leader).await;
        gone(descendant).await;
    }
}
