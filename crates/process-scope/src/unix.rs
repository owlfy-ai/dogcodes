//! A private lifetime channel survives a host's loss of Rust destructors.
//! The guard stays in its original group, so it never signals a reused PID.
use nix::libc;
use std::{io, os::fd::AsRawFd, os::unix::net::UnixStream};
use tokio::process::Command;

pub(super) fn terminal(
    command: &mut Command,
    columns: u16,
    rows: u16,
) -> io::Result<(std::fs::File, std::fs::File)> {
    use nix::{
        fcntl::{FcntlArg, FdFlag, fcntl},
        pty::{Winsize, openpty},
    };
    use std::{fs::File, process::Stdio};
    if columns == 0 || rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty terminal size",
        ));
    }
    let terminal = openpty(
        Some(&Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .map_err(io::Error::from)?;
    // Only the slave's explicit stdio copies survive exec. In particular, the
    // CLI must not inherit a master that would prevent EOF after scope exit.
    for fd in [&terminal.master, &terminal.slave] {
        fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
    }
    let input = File::from(terminal.master);
    let output = input.try_clone()?;
    let slave = File::from(terminal.slave);
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: child-only pre-exec code uses async-signal-safe syscalls and
    // initialized stdio fd 0. It runs before attach installs the group guard.
    unsafe {
        command.pre_exec(|| {
            // Darwin's header constant is u32 although ioctl takes c_ulong.
            #[cfg(target_os = "macos")]
            let request = libc::c_ulong::from(libc::TIOCSCTTY);
            #[cfg(not(target_os = "macos"))]
            let request = libc::TIOCSCTTY;
            if libc::setsid() < 0 || libc::ioctl(0, request, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((input, output))
}

/// Keep the peer in the host; the read end must remain open through spawn.
pub(super) fn attach(command: &mut Command) -> io::Result<(UnixStream, UnixStream)> {
    let (lifetime, reader) = UnixStream::pair()?;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: initialized writable rlimit; queried before any fork.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let limit =
        i32::try_from(limit.rlim_cur).map_err(|_| io::Error::from_raw_os_error(libc::EMFILE))?;
    let fd = reader.as_raw_fd();
    // SAFETY: start uses only stack values and async-signal-safe OS operations.
    // Both stream owners remain alive until Command::spawn returns.
    unsafe {
        command.pre_exec(move || start(fd, limit));
    }
    Ok((lifetime, reader))
}

fn start(lifetime: i32, limit: i32) -> io::Result<()> {
    // SAFETY: runs only in the forked child before exec; no allocation, locks,
    // Rust destructors, logging or runtime operations occur on this path.
    unsafe {
        if libc::getpgrp() != libc::getpid() {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        let mut ready = [-1; 2];
        if libc::pipe(ready.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let launcher = libc::fork();
        if launcher == 0 {
            // Reparent the guard before starting the CLI: it must not become
            // an unexpected child that a CLI's wait-all-children can wait for.
            match libc::fork() {
                -1 => libc::_exit(127),
                0 => exec_guard(lifetime, ready[1], limit),
                _ => libc::_exit(0),
            }
        }
        let fork_error = io::Error::last_os_error();
        libc::close(ready[1]);
        if launcher < 0 {
            libc::close(ready[0]);
            return Err(fork_error);
        }
        let result = ready_guard(ready[0]);
        libc::close(ready[0]);
        // The launcher does nothing after fork; reap only this exact child.
        let mut status = 0;
        loop {
            if libc::waitpid(launcher, &raw mut status, 0) >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
            return Err(io::Error::from_raw_os_error(libc::ECHILD));
        }
        result
    }
}

fn ready_guard(fd: i32) -> io::Result<()> {
    let until = monotonic_ms()? + 5_000;
    loop {
        let remaining = until - monotonic_ms()?;
        if remaining <= 0 {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: live pipe and one initialized pollfd; SIGCHLD retries retain
        // the original absolute deadline instead of extending startup.
        let ready =
            unsafe { libc::poll(&raw mut poll, 1, i32::try_from(remaining).unwrap_or(5_000)) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        let mut byte = [0_u8];
        // SAFETY: initialized one-byte output and still-owned readiness pipe.
        let count = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return if count == 1 && byte[0] == b'R' {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(libc::EPIPE))
        };
    }
}

fn monotonic_ms() -> io::Result<i128> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: initialized writable timespec; clock_gettime is async-signal-safe.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(i128::from(time.tv_sec) * 1_000 + i128::from(time.tv_nsec) / 1_000_000)
}

fn exec_guard(lifetime: i32, ready: i32, limit: i32) -> ! {
    // SAFETY: this grandchild has not returned to Rust after fork. Duplicate
    // above 4 first, avoiding collisions with the fixed interpreter descriptors.
    // Close all other inherited descriptors, including the spawn error pipe,
    // Core endpoint/lease files and other scopes' lifetime writers.
    unsafe {
        let lifetime_copy = libc::fcntl(lifetime, libc::F_DUPFD_CLOEXEC, 5);
        let ready_copy = libc::fcntl(ready, libc::F_DUPFD_CLOEXEC, 5);
        if lifetime_copy < 0
            || ready_copy < 0
            || libc::dup2(lifetime_copy, 3) < 0
            || libc::dup2(ready_copy, 4) < 0
        {
            libc::_exit(127);
        }
        for fd in 0..3 {
            libc::close(fd);
        }
        for fd in 5..limit {
            libc::close(fd);
        }
        let arguments = [c"sh".as_ptr(), c"-c".as_ptr(),
            c"trap '' HUP INT TERM; printf R >&4; exec 4>&-; while IFS= read -r _ <&3; do :; done; kill -KILL 0".as_ptr(),
            std::ptr::null()];
        let environment = [std::ptr::null()];
        // Fixed code, no user arguments or inherited shell startup environment.
        // Exec immediately also releases the inherited Core address space.
        libc::execve(
            c"/bin/sh".as_ptr(),
            arguments.as_ptr(),
            environment.as_ptr(),
        );
        libc::_exit(127);
    }
}
