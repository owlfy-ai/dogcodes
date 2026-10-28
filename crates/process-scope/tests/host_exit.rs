#![cfg(unix)]
use nix::{
    sys::signal::{Signal, kill, killpg},
    unistd::{Pid, getpgid},
};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    host: Child,
    sibling: Child,
    members: Vec<Pid>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.host.kill();
        let _ = self.host.wait();
        if let Some(&group) = self.members.first() {
            // Only an observed fixture member can establish its original group.
            if self
                .members
                .iter()
                .any(|&pid| getpgid(Some(pid)) == Ok(group))
            {
                let _ = killpg(group, Signal::SIGKILL);
            }
        }
        let _ = self.sibling.kill();
        let _ = self.sibling.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn present(pid: Pid) -> bool {
    match kill(pid, None) {
        Ok(()) | Err(nix::errno::Errno::EPERM) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        error => panic!("private process presence unknown: {error:?}"),
    }
}
fn wait_for(mut test: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(6);
    while !test() {
        assert!(
            Instant::now() < until,
            "private process lifetime unconfirmed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn host_fixture() {
    let Some(root) = std::env::var_os("DOGCODES_SCOPE_HOST_FIXTURE") else {
        return;
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut command = tokio::process::Command::new("/bin/sh");
            let terminal = std::env::var("DOGCODES_SCOPE_HOST_TERMINAL").as_deref() == Ok("1");
            let script = if terminal {
                "test -t 0 && test -t 1 && test -t 2 && test \"$(stty size)\" = '40 120' || exit 72; trap '' TERM HUP; sleep 60 & printf '%s %s\\n' \"$$\" \"$!\"; wait"
            } else {
                "trap '' TERM HUP; sleep 60 & printf '%s %s\\n' \"$$\" \"$!\"; wait"
            };
            command
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let (child, _terminal_input, line) = if terminal {
                let (child, input, output) = process_scope::OwnedChild::spawn_terminal(command, 120, 40).unwrap();
                let line = tokio::time::timeout(Duration::from_secs(3), tokio::task::spawn_blocking(move || {
                    use std::io::BufRead;
                    let mut line = String::new();
                    std::io::BufReader::new(output).read_line(&mut line).unwrap();
                    line
                })).await.unwrap().unwrap();
                let pid = Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap());
                assert_eq!(getpgid(Some(pid)).unwrap(), pid);
                assert_eq!(nix::unistd::getsid(Some(pid)).unwrap(), pid);
                (child, Some(input), line)
            } else {
                let mut child = process_scope::OwnedChild::spawn(command).unwrap();
                let mut line = String::new();
                BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).await.unwrap();
                (child, None, line)
            };
            assert_eq!(
                line.split_whitespace()
                    .next()
                    .unwrap()
                    .parse::<u32>()
                    .unwrap(),
                child.id().unwrap()
            );
            let root = PathBuf::from(root);
            std::fs::write(root.join("ready.pending"), line).unwrap();
            std::fs::rename(root.join("ready.pending"), root.join("ready")).unwrap();
            if std::env::var("DOGCODES_SCOPE_HOST_MODE").as_deref() == Ok("exit") {
                std::process::exit(73);
            }
            std::future::pending::<()>().await;
            drop(child);
        });
}

#[test]
fn loss_of_host_without_destructors_ends_the_original_group_and_leaves_siblings_alive() {
    for terminal in [false, true] {
        for mode in ["exit", "kill"] {
            let root = std::env::temp_dir().join(format!(
                "dogcodes-scope-host-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let sibling = Command::new("/bin/sleep").arg("60").spawn().unwrap();
            let host = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "host_fixture", "--nocapture"])
                .env("DOGCODES_SCOPE_HOST_FIXTURE", &root)
                .env("DOGCODES_SCOPE_HOST_MODE", mode)
                .env(
                    "DOGCODES_SCOPE_HOST_TERMINAL",
                    if terminal { "1" } else { "0" },
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let mut fixture = Fixture {
                root,
                host,
                sibling,
                members: vec![],
            };
            wait_for(|| {
                assert!(
                    fixture.host.try_wait().unwrap().is_none()
                        || fixture.root.join("ready").exists(),
                    "private host exited before readiness"
                );
                fixture.root.join("ready").exists()
            });
            fixture.members = std::fs::read_to_string(fixture.root.join("ready"))
                .unwrap()
                .split_whitespace()
                .map(|pid| Pid::from_raw(pid.parse().unwrap()))
                .collect();
            assert_eq!(fixture.members.len(), 2);
            if mode == "kill" {
                assert!(
                    fixture.members.iter().all(|&pid| present(pid)),
                    "live host prematurely lost its work"
                );
                fixture.host.kill().unwrap();
            }
            let status = fixture.host.wait().unwrap();
            assert_eq!(status.code(), if mode == "exit" { Some(73) } else { None });
            wait_for(|| fixture.members.iter().all(|&pid| !present(pid)));
            // The scoped signal must never target the parent test or its sibling.
            assert!(fixture.sibling.try_wait().unwrap().is_none());
            fixture.members.clear();
        }
    }
}

#[tokio::test]
async fn the_guard_is_not_a_child_that_the_native_cli_waits_for() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "wait; printf 'native done\\n'"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = process_scope::OwnedChild::spawn(command).unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(line.trim(), "native done");
    assert!(child.wait().await.unwrap().success());
}
