//! End-to-end against a private `sshd` started by this test: the real `ssh` client,
//! real authentication and the real shim. Nothing touches `~/.ssh` or `~/.dogcodes`.
//! Needs OpenSSH server binaries, so it is opt-in:
//!
//! ```sh
//! cargo build -p target-shim -p target-link --examples --bins
//! cargo test -p target-link --test local_sshd -- --ignored --test-threads=1
//! ```
#![cfg(unix)]
use agent_protocol::{SshAuth, SshConnection};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use target_link::{
    AskpassPrompt, ConnectError, ConnectErrorKind, ConnectObserver, ConnectRequest, ConnectStep,
    PromptKind, ShimCatalog, TargetLink, connect, find_ssh,
};

const SSHD_CANDIDATES: [&str; 3] = [
    "/usr/sbin/sshd",
    "/usr/local/sbin/sshd",
    "/opt/homebrew/sbin/sshd",
];

fn run(program: &str, args: &[&str]) {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn current_user() -> String {
    let output = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Sshd {
    root: tempfile::TempDir,
    port: u16,
    process: Child,
    client_key: PathBuf,
    known_hosts: PathBuf,
    config: PathBuf,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Sshd {
    /// `passphrase` protects the client key; `extra` is appended to `sshd_config`.
    fn start(passphrase: &str, extra: &str) -> Self {
        let sshd = SSHD_CANDIDATES
            .into_iter()
            .find(|path| Path::new(path).exists())
            .expect("an OpenSSH server binary");
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        let text = |file: &Path| file.to_str().unwrap().to_owned();
        let host_key = path("host_key");
        let client_key = path("client_key");
        run(
            "ssh-keygen",
            &["-q", "-t", "ed25519", "-N", "", "-f", &text(&host_key)],
        );
        run(
            "ssh-keygen",
            &[
                "-q",
                "-t",
                "ed25519",
                "-N",
                passphrase,
                "-f",
                &text(&client_key),
            ],
        );
        std::fs::copy(path("client_key.pub"), path("authorized_keys")).unwrap();
        let port = free_port();
        let sshd_config = path("sshd_config");
        std::fs::write(
            &sshd_config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nLogLevel ERROR\n{extra}\n",
                text(&host_key),
                text(&path("sshd.pid")),
                text(&path("authorized_keys")),
            ),
        )
        .unwrap();
        let process = Command::new(sshd)
            .args(["-D", "-e", "-f", &text(&sshd_config)])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let known_hosts = path("known_hosts");
        let host_public = std::fs::read_to_string(path("host_key.pub")).unwrap();
        let trusted = host_public
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(&known_hosts, format!("[127.0.0.1]:{port} {trusted}\n")).unwrap();
        let config = path("ssh_config");
        std::fs::write(
            &config,
            format!(
                "Host testbox\n  HostName 127.0.0.1\n  Port {port}\n  User {}\n  IdentityFile {}\n  IdentitiesOnly yes\n  IdentityAgent none\n  PreferredAuthentications publickey\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n",
                current_user(),
                text(&client_key),
                text(&known_hosts),
            ),
        )
        .unwrap();
        Self {
            root,
            port,
            process,
            client_key,
            known_hosts,
            config,
        }
    }

    fn install_directory(&self) -> String {
        self.root
            .path()
            .join("install")
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn connection(&self) -> SshConnection {
        SshConnection {
            destination: "testbox".into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: Some(self.install_directory()),
            proxy: None,
        }
    }
}

fn built(relative: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    let path = debug.join(relative);
    assert!(
        path.exists(),
        "{} is missing: run `cargo build -p target-shim -p target-link --examples --bins` first",
        path.display()
    );
    path
}

/// A catalog holding the shim built for this machine, as packaging would lay it out.
fn catalog() -> (tempfile::TempDir, ShimCatalog) {
    let directory = tempfile::tempdir().unwrap();
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        other => panic!("unsupported test platform {other:?}"),
    };
    let folder = directory.path().join(triple);
    std::fs::create_dir(&folder).unwrap();
    std::fs::copy(built("dogcodes-shim"), folder.join("dogcodes-shim")).unwrap();
    let catalog = ShimCatalog::new(vec![directory.path().into()]);
    (directory, catalog)
}

#[derive(Default)]
struct Recorder {
    steps: Mutex<Vec<ConnectStep>>,
    log: Mutex<Vec<String>>,
    prompts: Mutex<Vec<AskpassPrompt>>,
    answers: Mutex<Vec<Option<String>>>,
}

impl Recorder {
    fn answering(answers: &[Option<&str>]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().rev().map(|a| a.map(str::to_owned)).collect()),
            ..Self::default()
        })
    }
    fn logged(&self, text: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(text))
    }
    fn deployed(&self) -> bool {
        self.steps
            .lock()
            .unwrap()
            .iter()
            .any(|step| matches!(step, ConnectStep::Deploying { .. }))
    }
}

#[async_trait::async_trait]
impl ConnectObserver for Recorder {
    fn step(&self, step: ConnectStep) {
        self.steps.lock().unwrap().push(step);
    }
    fn log(&self, line: &str) {
        self.log.lock().unwrap().push(line.to_owned());
    }
    async fn prompt(&self, prompt: AskpassPrompt) -> Option<String> {
        self.prompts.lock().unwrap().push(prompt);
        self.answers.lock().unwrap().pop().flatten()
    }
}

async fn attempt(
    server: &Sshd,
    connection: SshConnection,
    shims: ShimCatalog,
    observer: Arc<Recorder>,
) -> Result<TargetLink, ConnectError> {
    let request = ConnectRequest {
        connection,
        program: find_ssh(&[]).await.unwrap(),
        askpass_program: built("examples/askpass_helper"),
        shims,
        config_file: Some(server.config.clone()),
    };
    tokio::time::timeout(Duration::from_mins(1), connect(request, observer))
        .await
        .expect("the attempt finishes")
}

async fn expect_failure(
    server: &Sshd,
    connection: SshConnection,
    observer: Arc<Recorder>,
) -> ConnectError {
    let (_directory, shims) = catalog();
    let Err(error) = attempt(server, connection, shims, observer).await else {
        panic!("the connection should have failed");
    };
    error
}

fn shims_running(marker: &str) -> bool {
    let output = Command::new("pgrep").args(["-f", marker]).output().unwrap();
    !output.stdout.is_empty()
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn the_first_connection_deploys_and_the_next_reuses_without_uploading() {
    let server = Sshd::start("", "");
    let (_directory, shims) = catalog();
    let first = Recorder::answering(&[]);
    let link = attempt(&server, server.connection(), shims.clone(), first.clone())
        .await
        .unwrap();
    assert!(first.deployed());
    assert!(first.prompts.lock().unwrap().is_empty());
    assert_eq!(link.info().protocol, target_wire::PROTOCOL_VERSION);
    assert_eq!(link.info().machine_identity.len(), 32);
    link.ping().await.unwrap();

    let second = Recorder::answering(&[]);
    let again = attempt(&server, server.connection(), shims, second.clone())
        .await
        .unwrap();
    assert!(!second.deployed());
    assert!(second.logged("already installed"));
    assert_eq!(again.info().machine_identity, link.info().machine_identity);
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn the_install_directory_is_private_to_the_account() {
    use std::os::unix::fs::PermissionsExt;
    let server = Sshd::start("", "");
    let (_directory, shims) = catalog();
    let _link = attempt(
        &server,
        server.connection(),
        shims,
        Recorder::answering(&[]),
    )
    .await
    .unwrap();
    let mode = std::fs::metadata(server.install_directory())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn dropping_the_link_ends_the_shim_on_the_server() {
    let server = Sshd::start("", "");
    let (_directory, shims) = catalog();
    let link = attempt(
        &server,
        server.connection(),
        shims,
        Recorder::answering(&[]),
    )
    .await
    .unwrap();
    let marker = server.install_directory();
    assert!(shims_running(&marker));
    let mut closed = link.closed();
    drop(link);
    tokio::time::timeout(Duration::from_secs(10), closed.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();
    for _ in 0..50 {
        if !shims_running(&marker) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the shim kept running after the connection ended");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn killing_the_server_side_reports_a_closed_link() {
    let mut server = Sshd::start("", "");
    let (_directory, shims) = catalog();
    let link = attempt(
        &server,
        server.connection(),
        shims,
        Recorder::answering(&[]),
    )
    .await
    .unwrap();
    let marker = server.install_directory();
    run("pkill", &["-f", &marker]);
    let mut closed = link.closed();
    tokio::time::timeout(Duration::from_secs(10), closed.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();
    let _ = server.process.kill();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_protected_key_asks_for_its_passphrase_through_askpass() {
    let server = Sshd::start("correct horse", "");
    let observer = Recorder::answering(&[Some("correct horse")]);
    let (_directory, shims) = catalog();
    let _link = attempt(&server, server.connection(), shims, observer.clone())
        .await
        .unwrap();
    let prompts = observer.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].kind, PromptKind::Passphrase);
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn cancelling_a_passphrase_is_an_authentication_failure() {
    let server = Sshd::start("correct horse", "");
    let observer = Recorder::answering(&[None, None, None]);
    let error = expect_failure(&server, server.connection(), observer).await;
    assert_eq!(error.kind, ConnectErrorKind::AuthFailed, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_unknown_host_is_confirmed_by_the_person() {
    let server = Sshd::start("", "");
    std::fs::write(&server.known_hosts, "").unwrap();
    let (_directory, shims) = catalog();
    let observer = Recorder::answering(&[Some("yes")]);
    let _link = attempt(&server, server.connection(), shims, observer.clone())
        .await
        .unwrap();
    assert_eq!(
        observer.prompts.lock().unwrap()[0].kind,
        PromptKind::HostKey
    );
    let saved = std::fs::read_to_string(&server.known_hosts).unwrap();
    assert!(saved.contains(&server.port.to_string()), "{saved}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn declining_an_unknown_host_stops_the_connection() {
    let server = Sshd::start("", "");
    std::fs::write(&server.known_hosts, "").unwrap();
    let observer = Recorder::answering(&[Some("no")]);
    let error = expect_failure(&server, server.connection(), observer).await;
    assert_eq!(error.kind, ConnectErrorKind::HostKeyRejected, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_changed_host_key_is_refused_and_never_asked_about() {
    let server = Sshd::start("", "");
    let other = tempfile::tempdir().unwrap();
    let key = other.path().join("other");
    run(
        "ssh-keygen",
        &["-q", "-t", "ed25519", "-N", "", "-f", key.to_str().unwrap()],
    );
    let public = std::fs::read_to_string(other.path().join("other.pub")).unwrap();
    let forged = public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        &server.known_hosts,
        format!("[127.0.0.1]:{} {forged}\n", server.port),
    )
    .unwrap();
    let observer = Recorder::answering(&[Some("yes")]);
    let error = expect_failure(&server, server.connection(), observer.clone()).await;
    assert_eq!(error.kind, ConnectErrorKind::HostKeyChanged, "{error}");
    assert!(
        !observer
            .prompts
            .lock()
            .unwrap()
            .iter()
            .any(|prompt| prompt.kind == PromptKind::HostKey)
    );
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_key_the_server_does_not_accept_is_an_authentication_failure() {
    let server = Sshd::start("", "");
    std::fs::write(server.root.path().join("authorized_keys"), "").unwrap();
    let observer = Recorder::answering(&[]);
    let error = expect_failure(&server, server.connection(), observer).await;
    assert_eq!(error.kind, ConnectErrorKind::AuthFailed, "{error}");
    assert!(!server.client_key.to_str().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_account_that_cannot_run_commands_is_named_as_such() {
    let server = Sshd::start("", "ForceCommand /usr/bin/false");
    let observer = Recorder::answering(&[]);
    let error = expect_failure(&server, server.connection(), observer).await;
    assert_eq!(error.kind, ConnectErrorKind::AccountRestricted, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_noisy_login_shell_does_not_break_the_exchange() {
    let server = Sshd::start(
        "",
        r#"ForceCommand /bin/sh -c 'echo WELCOME TO THE SERVER; printf "no newline banner"; eval "$SSH_ORIGINAL_COMMAND"'"#,
    );
    let (_directory, shims) = catalog();
    let link = attempt(
        &server,
        server.connection(),
        shims,
        Recorder::answering(&[]),
    )
    .await
    .unwrap();
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_unsafe_install_directory_is_refused() {
    let server = Sshd::start("", "");
    let real = server.root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = server.root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut connection = server.connection();
    connection.install_directory = Some(link.to_str().unwrap().to_owned());
    let error = expect_failure(&server, connection, Recorder::answering(&[])).await;
    assert_eq!(error.kind, ConnectErrorKind::UnsafeDirectory, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_unwritable_install_directory_is_reported() {
    let server = Sshd::start("", "");
    let mut connection = server.connection();
    connection.install_directory = Some("/nonexistent-root/dogcodes".into());
    let error = expect_failure(&server, connection, Recorder::answering(&[])).await;
    assert_eq!(error.kind, ConnectErrorKind::DirectoryUnwritable, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_build_without_this_platform_says_so() {
    let server = Sshd::start("", "");
    let empty = tempfile::tempdir().unwrap();
    let shims = ShimCatalog::new(vec![empty.path().into()]);
    let Err(error) = attempt(
        &server,
        server.connection(),
        shims,
        Recorder::answering(&[]),
    )
    .await
    else {
        panic!("no shim is available");
    };
    assert_eq!(error.kind, ConnectErrorKind::ShimUnavailable, "{error}");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn the_system_config_alias_resolves_through_ssh_g() {
    let server = Sshd::start("", "");
    let program = find_ssh(&[]).await.unwrap();
    // `-G` reads the user's real config; a plain name still resolves to itself.
    let resolved = target_link::ssh_config::resolve(&program, "example-host-for-test")
        .await
        .unwrap();
    assert_eq!(resolved.host, "example-host-for-test");
    assert!(server.port > 0);
}

/// Starts a process on the target through the same path a driver uses.
/// A proxy in front of anything: it records what it was asked to reach and relays.
async fn test_proxy(
    kind: agent_protocol::SshProxyKind,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use agent_protocol::SshProxyKind;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = asked.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let destination = match kind {
                    SshProxyKind::Socks5 => {
                        let mut greeting = [0; 3];
                        socket.read_exact(&mut greeting).await.unwrap();
                        socket.write_all(&[5, 0]).await.unwrap();
                        let mut head = [0; 4];
                        socket.read_exact(&mut head).await.unwrap();
                        let host = match head[3] {
                            1 => {
                                let mut address = [0; 4];
                                socket.read_exact(&mut address).await.unwrap();
                                std::net::Ipv4Addr::from(address).to_string()
                            }
                            3 => {
                                let length = socket.read_u8().await.unwrap();
                                let mut name = vec![0; usize::from(length)];
                                socket.read_exact(&mut name).await.unwrap();
                                String::from_utf8(name).unwrap()
                            }
                            other => panic!("unexpected address type {other}"),
                        };
                        let port = socket.read_u16().await.unwrap();
                        socket
                            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                            .await
                            .unwrap();
                        format!("{host}:{port}")
                    }
                    SshProxyKind::Http => {
                        let mut reader = BufReader::new(&mut socket);
                        let mut request = String::new();
                        reader.read_line(&mut request).await.unwrap();
                        let mut line = String::new();
                        while line != "\r\n" {
                            line.clear();
                            reader.read_line(&mut line).await.unwrap();
                        }
                        drop(reader);
                        socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
                        request.split_whitespace().nth(1).unwrap().to_owned()
                    }
                };
                recorded.lock().unwrap().push(destination.clone());
                let mut server = tokio::net::TcpStream::connect(destination).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut server).await;
            });
        }
    });
    (port, asked)
}

async fn connected(server: &Sshd) -> (TargetLink, tempfile::TempDir) {
    let (directory, shims) = catalog();
    let link = attempt(server, server.connection(), shims, Recorder::answering(&[]))
        .await
        .unwrap();
    (link, directory)
}

fn shell(link: &TargetLink, script: &str) -> agent_runtime::TargetCommand {
    let mut command = agent_runtime::TargetCommand::new(link.target(), "/bin/sh");
    command.args(["-c", script]);
    command
}

async fn read_all(stream: &mut Option<agent_runtime::ProcessStream>) -> String {
    use tokio::io::AsyncReadExt;
    let mut text = String::new();
    stream
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .await
        .unwrap();
    text
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_process_runs_on_the_target_with_separate_streams_and_exit_code() {
    use tokio::io::AsyncWriteExt;
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let mut child = shell(&link, "cat; echo problem >&2; exit 7")
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"hello over ssh\n").await.unwrap();
    stdin.shutdown().await.unwrap();
    assert_eq!(read_all(&mut child.stdout).await, "hello over ssh\n");
    assert_eq!(read_all(&mut child.stderr).await, "problem\n");
    assert_eq!(child.wait().await.unwrap().code(), Some(7));
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_process_sees_only_the_environment_it_was_given() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let mut command = shell(&link, "echo \"$NAMED:${HOME-unset}\"; pwd");
    command.env("NAMED", "value").current_dir("/tmp");
    let mut child = command.spawn().unwrap();
    let output = read_all(&mut child.stdout).await;
    let mut lines = output.lines();
    assert_eq!(lines.next(), Some("value:unset"));
    assert!(lines.next().unwrap().ends_with("/tmp"));
    child.wait().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn large_output_survives_a_reader_slower_than_the_window() {
    use tokio::io::AsyncReadExt;
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let mut child = shell(&link, "head -c 3000000 /dev/zero").spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut buffer = vec![0; 16 * 1024];
    let mut total = 0;
    let mut pauses = 0;
    loop {
        let count = stdout.read(&mut buffer).await.unwrap();
        if count == 0 {
            break;
        }
        total += count;
        pauses += 1;
        if pauses % 40 == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    assert_eq!(total, 3_000_000);
    assert!(child.wait().await.unwrap().success());
    // Control traffic kept working through all of it.
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_blocked_process_does_not_block_another() {
    use tokio::io::AsyncReadExt;
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    // Nobody reads this one, so it stalls on its window.
    let _stalled = shell(&link, "head -c 5000000 /dev/zero; sleep 30")
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut other = shell(&link, "echo through").spawn().unwrap();
    let mut text = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        other.stdout.take().unwrap().read_to_string(&mut text),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(text, "through\n");
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn terminating_ends_the_whole_process_group() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let marker = format!("dogcodes-test-{}", server.port);
    let script = format!("sh -c 'sleep 60; echo {marker}' & sleep 60; echo {marker}");
    let mut child = shell(&link, &script).spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(shims_running(&format!("echo {marker}")));
    child.terminate().await.unwrap();
    child.wait().await.unwrap();
    assert!(!shims_running(&format!("echo {marker}")));
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_missing_program_is_reported_through_its_exit_status() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let mut child = agent_runtime::TargetCommand::new(link.target(), "/nonexistent/agent")
        .spawn()
        .unwrap();
    assert_eq!(read_all(&mut child.stdout).await, "");
    assert_eq!(child.wait().await.unwrap().code(), Some(127));
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn losing_the_link_is_an_error_and_never_an_exit_proof() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let marker = format!("dogcodes-lost-{}", server.port);
    let mut child = shell(&link, &format!("sleep 60; echo {marker}"))
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut closed = link.closed();
    drop(link);
    closed.wait_for(Option::is_some).await.unwrap();
    assert_eq!(read_all(&mut child.stdout).await, "");
    assert!(child.wait().await.is_err());
    // The shim ended with its connection, and took the process group with it.
    for _ in 0..50 {
        if !shims_running(&format!("echo {marker}")) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the remote process outlived its connection");
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn the_connection_goes_through_a_chosen_proxy() {
    use agent_protocol::{SshProxy, SshProxyKind};
    for kind in [SshProxyKind::Socks5, SshProxyKind::Http] {
        let server = Sshd::start("", "");
        let (port, asked) = test_proxy(kind).await;
        let mut connection = server.connection();
        connection.proxy = Some(SshProxy::Custom {
            kind,
            host: "127.0.0.1".into(),
            port,
        });
        let (_directory, shims) = catalog();
        let link = attempt(&server, connection, shims, Recorder::answering(&[]))
            .await
            .unwrap();
        // The connection really works, and it really went through the proxy.
        let mut child = shell(&link, "printf through").spawn().unwrap();
        assert_eq!(read_all(&mut child.stdout).await, "through");
        assert!(!asked.lock().unwrap().is_empty(), "{kind:?}");
    }
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_proxy_that_cannot_be_used_is_reported_as_such_and_never_bypassed() {
    use agent_protocol::{SshProxy, SshProxyKind};
    let server = Sshd::start("", "");
    // Bound and released, so nothing is listening there.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut connection = server.connection();
    connection.proxy = Some(SshProxy::Custom {
        kind: SshProxyKind::Socks5,
        host: "127.0.0.1".into(),
        port,
    });
    let (_directory, shims) = catalog();
    let Err(error) = attempt(&server, connection, shims, Recorder::answering(&[])).await else {
        panic!("connected directly instead of through the proxy");
    };
    assert_eq!(
        error.kind,
        target_link::ConnectErrorKind::ProxyUnavailable,
        "{error:?}"
    );
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn the_target_reports_the_account_login_environment() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let target = link.target();
    assert!(!target.is_local());
    assert!(target.inherited("PATH").is_some());
    assert!(target.inherited("DEFINITELY_NOT_SET_ANYWHERE").is_none());
    assert!(target.temp_directory().is_absolute());
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn a_private_directory_comes_and_goes_on_the_target() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let target = link.target();
    let path = agent_runtime::create_target_directory(&target)
        .await
        .unwrap();
    assert!(path.starts_with(target.temp_directory()), "{path:?}");
    assert!(path.is_dir());
    agent_runtime::remove_target_directory(&target, &path)
        .await
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn directories_are_listed_and_resolved_on_the_target() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let root = server.root.path().join("listing");
    std::fs::create_dir_all(root.join("beta")).unwrap();
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::write(root.join("file.txt"), "not a directory").unwrap();
    let (path, directories) = link
        .list_directory(root.join("alpha/../.").to_str().unwrap())
        .await
        .unwrap();
    assert!(path.ends_with("/listing"));
    assert_eq!(directories, ["alpha", "beta"]);
    assert!(link.list_directory("/nonexistent-directory").await.is_err());
    // A failed read is an answer, not a broken connection.
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_agent_reaches_a_local_program_through_the_relay() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let target = link.target();
    // A local program standing in for the app's tool server, marking what it serves.
    let (command, args) = target
        .app_tool_command("/bin/sh", &["-c".into(), "echo from-local; cat".into()])
        .unwrap();
    assert_eq!(args[0], "mcp-relay");
    let (_, again) = target
        .app_tool_command("/bin/sh", &["-c".into(), "echo from-local; cat".into()])
        .unwrap();
    assert_eq!(args, again, "the same tool must keep comparing equal");
    let mut helper = agent_runtime::TargetCommand::new(target.clone(), &command);
    helper.args(&args);
    let mut helper = helper.spawn().unwrap();
    let mut stdin = helper.stdin.take().unwrap();
    let mut stdout = helper.stdout.take().unwrap();
    stdin.write_all(b"request\n").await.unwrap();
    let mut seen = Vec::new();
    let mut buffer = [0; 256];
    tokio::time::timeout(Duration::from_secs(20), async {
        while !String::from_utf8_lossy(&seen).contains("request") {
            let count = stdout.read(&mut buffer).await.unwrap();
            assert!(
                count > 0,
                "ended early: {:?}",
                String::from_utf8_lossy(&seen)
            );
            seen.extend_from_slice(&buffer[..count]);
        }
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&seen).starts_with("from-local\n"));
    // Ending the input ends the local program, and then the helper.
    stdin.shutdown().await.unwrap();
    drop(stdin);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), helper.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn an_unknown_relay_token_cannot_run_anything_locally() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let target = link.target();
    let (command, mut args) = target.app_tool_command("/bin/cat", &[]).unwrap();
    *args.last_mut().unwrap() = "not-a-real-token".into();
    let mut helper = agent_runtime::TargetCommand::new(target.clone(), &command);
    helper.args(&args);
    let mut helper = helper.spawn().unwrap();
    let output = tokio::time::timeout(Duration::from_secs(20), read_all(&mut helper.stdout)).await;
    assert_eq!(output.unwrap(), "");
    link.ping().await.unwrap();
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn files_are_written_privately_on_the_target() {
    use std::os::unix::fs::PermissionsExt;
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let file = server.root.path().join("uploads/deep/report.txt");
    agent_runtime::write_target_file(&link.target(), file.to_str().unwrap(), b"contents")
        .await
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"contents");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        agent_runtime::write_target_file(&link.target(), "/nonexistent-root/a/b", b"x")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "needs an OpenSSH server binary"]
async fn file_attachments_reach_the_target_and_images_stay_in_the_prompt() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let local = tempfile::tempdir().unwrap();
    let file = local.path().join("report.txt");
    std::fs::write(&file, b"attached contents").unwrap();
    let image = local.path().join("picture.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
    let attachment = |name: &str, path: &Path, mime: &str| agent_domain::PromptAttachment {
        delivered_path: None,
        name: name.into(),
        path: path.to_str().unwrap().into(),
        mime_type: mime.into(),
        integrity: None,
    };
    let mut items = [
        // A name must not choose where the file lands.
        attachment("../../outside.txt", &file, "text/plain"),
        attachment("picture.png", &image, "image/png"),
    ];
    driver_api::attachments::deliver_to_target(&link.target(), &mut items)
        .await
        .unwrap();
    let delivered = items[0].delivered_path.clone().unwrap();
    assert!(delivered.contains("/dogcodes-attachments/"));
    assert!(delivered.ends_with("/outside.txt"));
    assert_eq!(std::fs::read(&delivered).unwrap(), b"attached contents");
    assert_eq!(items[1].delivered_path, None);
    let reference = driver_api::attachments::file_text(&items[0]);
    assert!(reference["text"].as_str().unwrap().contains(&delivered));
    assert!(
        !reference["text"]
            .as_str()
            .unwrap()
            .contains(file.to_str().unwrap())
    );
    std::fs::remove_file(&delivered).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an OpenSSH server binary"]
async fn a_terminal_on_the_target_has_input_output_size_and_ends_when_stopped() {
    use agent_runtime::{Pipe, ProcessRequest};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let request = ProcessRequest {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "stty size; read line; stty size; echo \"got:$line\"; sleep 100".into(),
        ],
        env: std::collections::BTreeMap::from([("TERM".into(), "xterm".into())]),
        cwd: Some(server.root.path().to_owned()),
        stdin: Pipe::Piped,
        stdout: Pipe::Piped,
        stderr: Pipe::Null,
    };
    let mut terminal = link.target().spawn_terminal(request, 91, 17).unwrap();
    let mut input = terminal.child.stdin.take().unwrap();
    let mut output = terminal.child.stdout.take().unwrap();
    let mut seen = String::new();
    let until = async |output: &mut agent_runtime::ProcessStream, seen: &mut String, text: &str| {
        let mut buffer = [0; 4096];
        while !seen.contains(text) {
            let count = tokio::time::timeout(Duration::from_secs(20), output.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert!(count > 0, "{seen}");
            seen.push_str(&String::from_utf8_lossy(&buffer[..count]));
        }
    };
    until(&mut output, &mut seen, "17 91").await;
    (terminal.resize)(120, 40).unwrap();
    // The resize frame goes first on the same channel as this input.
    input.write_all(b"hello\n").await.unwrap();
    until(&mut output, &mut seen, "got:hello").await;
    assert!(seen.contains("40 120"), "{seen}");
    tokio::time::timeout(Duration::from_secs(20), terminal.child.terminate())
        .await
        .unwrap()
        .unwrap();
    let status = terminal.child.wait().await.unwrap();
    assert!(!status.success());
}

mod workspace_operations {
    use super::*;
    use agent_core::Core;
    use agent_domain::{ProjectId, ProviderId, TargetId, TaskId};
    use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject};
    use agent_store::SqliteStore;
    use serde_json::{Value, json};
    use workspace_ops::{
        operation::{self, Operation, RemoteWorkspace, RunningOperation},
        review::{ReviewQuery, service::ReviewCall},
        workspace_service::DirectoryRequest,
    };

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A task whose project lives on the target, in a real repository there.
    async fn project(path: &Path) -> (Arc<Core>, TaskId) {
        let (core, task, _) = project_with_store(path).await;
        (core, task)
    }

    async fn project_with_store(path: &Path) -> (Arc<Core>, TaskId, Arc<SqliteStore>) {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "-b", "main"]);
        std::fs::write(path.join("kept.txt"), "before\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "base"]);
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        let task = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: Some(TargetId::new()),
                project_id: project_id.clone(),
                display_name: "Remote".into(),
                path: path.to_str().unwrap().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id,
                title: "Conversation".into(),
                preferred_executor: agent_domain::ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        (core, task, store)
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn a_commit_runs_on_the_target_and_reports_what_git_said() {
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task) = project(&repository).await;
        git(&repository, &["config", "user.name", "Committer"]);
        git(&repository, &["config", "user.email", "c@example.com"]);
        git(&repository, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repository.join("kept.txt"), "after\n").unwrap();
        let review = |call: ReviewCall| {
            let target = target.clone();
            let core = core.clone();
            async move { operation::call(&target, &core, &Operation::Review(call)).await }
        };
        let snapshot = review(ReviewCall::Query(
            serde_json::from_value::<ReviewQuery>(json!({
                "taskId": task.as_str(), "rootId": null, "mode": "uncommitted",
                "reference": null, "parent": null,
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
        let root_id = snapshot["roots"][0]["root"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let snapshot_id = snapshot["id"].as_str().unwrap().to_owned();
        let commit = |id: &str, message: &str, snapshot: &str| {
            ReviewCall::GitAction(
                serde_json::from_value(json!({
                    "operationId": id, "taskId": task.as_str(), "rootId": root_id,
                    "action": "commit", "message": message, "stageAll": true,
                    "snapshotId": snapshot, "workflow": null,
                }))
                .unwrap(),
            )
        };
        let id = uuid::Uuid::new_v4().to_string();
        let committed = review(commit(&id, "From the app", &snapshot_id)).await;
        assert_eq!(committed, Ok(json!(true)), "{committed:?}");
        let log = Command::new("git")
            .args(["-C"])
            .arg(&repository)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "From the app");

        // A failure from Git itself is recorded with its own words.
        let hooks = repository.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho 'refused by the hook' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::write(repository.join("kept.txt"), "again\n").unwrap();
        let snapshot = review(ReviewCall::Query(
            serde_json::from_value::<ReviewQuery>(json!({
                "taskId": task.as_str(), "rootId": null, "mode": "uncommitted",
                "reference": null, "parent": null,
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let refused = review(commit(&id, "Refused", snapshot["id"].as_str().unwrap())).await;
        assert!(refused.is_err(), "{refused:?}");
        let receipt = review(ReviewCall::GitOperation {
            task_id: task.to_string(),
            operation_id: id,
        })
        .await
        .unwrap();
        assert_eq!(receipt["outcome"]["status"], "failed", "{receipt}");
        assert!(
            receipt["outcome"]["message"]
                .as_str()
                .unwrap()
                .contains("refused by the hook"),
            "{receipt}"
        );
    }

    /// The desktop's side: answers what Git asked, and everything else from the core.
    struct Answering {
        core: std::sync::Arc<Core>,
        answer: Option<String>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl workspace_ops::ReviewFacts for Answering {
        fn on_hub(&self) -> bool {
            false
        }
        async fn bootstrap(&self) -> Result<agent_protocol::BootstrapSnapshot, String> {
            workspace_ops::ReviewFacts::bootstrap(&self.core).await
        }
        async fn try_workspace_mutation(&self) -> Result<workspace_ops::MutationGuard, String> {
            workspace_ops::ReviewFacts::try_workspace_mutation(&self.core).await
        }
        async fn task_artifacts(
            &self,
            task: &TaskId,
            before: Option<&agent_protocol::ArtifactCursor>,
            limit: usize,
        ) -> Result<agent_protocol::ArtifactPage, String> {
            workspace_ops::ReviewFacts::task_artifacts(&self.core, task, before, limit).await
        }
        async fn verified_artifact(
            &self,
            task: &TaskId,
            artifact: &agent_domain::ArtifactId,
            sha256: &str,
        ) -> Result<workspace_ops::VerifiedArtifact, String> {
            workspace_ops::ReviewFacts::verified_artifact(&self.core, task, artifact, sha256).await
        }
        async fn summarize_commit(
            &self,
            _: agent_domain::ExecutorRef,
            _: String,
            _: Option<String>,
        ) -> Result<String, String> {
            Err("unused".into())
        }
        async fn review_run_exists(
            &self,
            _: &TaskId,
            _: &agent_domain::RunId,
        ) -> Result<bool, String> {
            Err("unused".into())
        }
        async fn start_project_review(
            &self,
            _: serde_json::Value,
            _: Option<serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            Err("unused".into())
        }
        async fn ask_git_credential(
            &self,
            _: &TaskId,
            prompt: &str,
        ) -> Result<Option<String>, String> {
            self.asked.lock().unwrap().push(prompt.to_owned());
            Ok(self.answer.clone())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn what_git_asks_on_the_target_reaches_the_desktop_and_its_answer_returns() {
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task) = project(&repository).await;
        git(&repository, &["config", "user.name", "Committer"]);
        git(&repository, &["config", "user.email", "c@example.com"]);
        git(&repository, &["config", "commit.gpgsign", "false"]);
        // A remote is not needed to run the program Git would ask through.
        let hooks = repository.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        std::fs::write(
            &hook,
            "#!/bin/sh\nanswer=$(\"$GIT_ASKPASS\" \"Username for 'https://example.test': \") || exit 1\nprintf %s \"$answer\" > .git/answered\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let commit = |answer: Option<&str>, content: &str, message: &str| {
            std::fs::write(repository.join("kept.txt"), content).unwrap();
            let facts = Answering {
                core: core.clone(),
                answer: answer.map(str::to_owned),
                asked: std::sync::Mutex::default(),
            };
            let (target, task, message) = (target.clone(), task.clone(), message.to_owned());
            async move {
                let snapshot = operation::call(
                    &target,
                    &facts,
                    &Operation::Review(ReviewCall::Query(
                        serde_json::from_value::<ReviewQuery>(json!({
                            "taskId": task.as_str(), "rootId": null, "mode": "uncommitted",
                            "reference": null, "parent": null,
                        }))
                        .unwrap(),
                    )),
                )
                .await
                .unwrap();
                let id = uuid::Uuid::new_v4().to_string();
                let result = operation::call(
                    &target,
                    &facts,
                    &Operation::Review(ReviewCall::GitAction(
                        serde_json::from_value(json!({
                            "operationId": id, "taskId": task.as_str(),
                            "rootId": snapshot["roots"][0]["root"]["id"],
                            "action": "commit", "message": message, "stageAll": true,
                            "snapshotId": snapshot["id"], "workflow": null,
                        }))
                        .unwrap(),
                    )),
                )
                .await;
                (result, facts.asked.into_inner().unwrap())
            }
        };
        let (result, asked) = commit(Some("me"), "answered\n", "Answered").await;
        assert_eq!(result, Ok(json!(true)));
        assert_eq!(asked, ["Username for 'https://example.test': "]);
        assert_eq!(
            std::fs::read_to_string(repository.join(".git/answered")).unwrap(),
            "me"
        );

        let (result, asked) = commit(None, "declined\n", "Declined").await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(asked.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn git_review_runs_on_the_target_and_snapshots_last_between_calls() {
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task) = project(&repository).await;
        std::fs::write(repository.join("kept.txt"), "after\n").unwrap();
        let review = |call: ReviewCall| {
            let target = target.clone();
            let core = core.clone();
            async move { operation::call(&target, &core, &Operation::Review(call)).await }
        };
        let query = |mode: &str| {
            ReviewCall::Query(
                serde_json::from_value::<ReviewQuery>(json!({
                    "taskId": task.as_str(), "rootId": null, "mode": mode,
                    "reference": null, "parent": null,
                }))
                .unwrap(),
            )
        };
        let snapshot = review(query("uncommitted")).await.unwrap();
        let root = &snapshot["roots"][0];
        assert_eq!(root["files"][0]["path"], "kept.txt", "{snapshot}");
        assert_eq!(root["statistics"]["kept.txt"]["additions"], 1);
        let root_id = root["root"]["id"].as_str().unwrap().to_owned();
        let snapshot_id = snapshot["id"].as_str().unwrap().to_owned();
        // The frozen snapshot lives on the target, in the shim, across operations.
        let file = review(ReviewCall::File {
            task_id: task.to_string(),
            snapshot_id: snapshot_id.clone(),
            root_id: root_id.clone(),
            path: "kept.txt".into(),
        })
        .await
        .unwrap();
        assert_eq!(file["text"]["after"][0], "after\n");
        assert_eq!(file["text"]["before"][0], "before\n");
        // A mutation changes the server's repository, under this side's guard.
        let stage = serde_json::from_value::<ReviewCall>(json!({
            "operation": "index",
            "arguments": {
                "operationId": uuid::Uuid::new_v4().to_string(),
                "taskId": task.as_str(), "snapshotId": snapshot_id, "rootId": root_id,
                "paths": ["kept.txt"], "action": "stage",
            },
        }))
        .unwrap();
        review(stage).await.unwrap();
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "kept.txt");
        assert!(
            core.try_workspace_mutation().is_ok(),
            "the guard was released"
        );
        // While an agent holds the workspace, nothing mutates it over there either.
        let held = core.try_workspace_mutation().unwrap();
        let snapshot = review(query("staged")).await.unwrap();
        let root = &snapshot["roots"][0];
        let blocked = serde_json::from_value::<ReviewCall>(json!({
            "operation": "index",
            "arguments": {
                "operationId": uuid::Uuid::new_v4().to_string(),
                "taskId": task.as_str(), "snapshotId": snapshot["id"], "rootId": root["root"]["id"],
                "paths": ["kept.txt"], "action": "unstage",
            },
        }))
        .unwrap();
        assert!(review(blocked).await.is_err());
        drop(held);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn files_are_listed_previewed_found_and_watched_on_the_target() {
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task) = project(&repository).await;
        let call = |method: &str, args: Value| {
            let operation = Operation::Workspace {
                method: method.to_owned(),
                args,
            };
            let target = target.clone();
            let core = core.clone();
            async move { operation::call(&target, &core, &operation).await }
        };
        let request = json!({"taskId": task.as_str(), "root": 0, "path": ""});
        let entries = call("workspace_directory", request.clone()).await.unwrap();
        assert!(
            entries
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["name"] == "kept.txt"),
            "{entries}"
        );
        let preview = call(
            "workspace_preview",
            json!({"taskId": task.as_str(), "root": 0, "path": "kept.txt"}),
        )
        .await
        .unwrap();
        assert_eq!(preview["text"], "before\n");
        let found = call("workspace_find_files", json!((task.as_str(), "kept", true)))
            .await
            .unwrap();
        assert_eq!(found["files"][0]["path"], "kept.txt");
        // Reading out of the project is refused over there, as it is here.
        assert!(
            call(
                "workspace_preview",
                json!({"taskId": task.as_str(), "root": 0, "path": "../sshd_config"}),
            )
            .await
            .is_err()
        );
        let operation =
            Operation::WatchDirectory(serde_json::from_value::<DirectoryRequest>(request).unwrap());
        let mut watch = RunningOperation::start(&target, &operation).await.unwrap();
        assert_eq!(watch.next_event(&core).await.unwrap(), json!("ready"));
        std::fs::write(repository.join("new.txt"), "x").unwrap();
        let event = tokio::time::timeout(Duration::from_secs(20), watch.next_event(&core))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(event, json!("ready"));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn the_core_sees_turns_and_snapshots_of_directories_on_the_target() {
        use agent_protocol::workspace_context::WorkspaceContextProvider;
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let repository = server.root.path().join("repo");
        let _ = project(&repository).await;
        let path = repository.to_str().unwrap();
        let workspace = RemoteWorkspace::new(link.target());
        assert!(
            workspace
                .validate_workspace_roots(&[path.to_owned()])
                .await
                .is_ok()
        );
        let capture = workspace.begin_turn(path).await.unwrap();
        std::fs::write(repository.join("kept.txt"), "changed by a turn\n").unwrap();
        let finished = capture.finish().await.unwrap();
        assert!(finished.to_string().contains("kept.txt"), "{finished}");
        let snapshot = workspace.collect(path).await;
        assert!(snapshot.git_snapshot.is_some());
        let diff = workspace.capture_diff(path).await.unwrap();
        assert!(String::from_utf8_lossy(&diff.content).contains("changed by a turn"));
        assert!(
            workspace
                .validate_workspace_roots(&["/nonexistent/dogcodes".to_owned()])
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn a_fork_restores_a_worktree_on_the_target_and_a_turn_can_be_undone_there() {
        use agent_protocol::workspace_context::WorkspaceContextProvider;
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task, store) = project_with_store(&repository).await;
        let path = repository.to_str().unwrap().to_owned();
        let workspace = RemoteWorkspace::new(target.clone());
        std::fs::write(repository.join("kept.txt"), "in the fork\n").unwrap();
        let snapshot = workspace
            .capture_fork_snapshot(std::slice::from_ref(&path))
            .await
            .unwrap();
        let paths = workspace
            .fork_worktrees(snapshot, TaskId::new().as_str())
            .await
            .unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            std::fs::read_to_string(Path::new(&paths[0]).join("kept.txt")).unwrap(),
            "in the fork\n"
        );
        std::fs::write(repository.join("kept.txt"), "before\n").unwrap();

        // Undo reads the turn's record through this side and rewrites files over there.
        let capture = workspace.begin_turn(&path).await.unwrap();
        std::fs::write(repository.join("kept.txt"), "by the turn\n").unwrap();
        let mut record = capture.finish().await.unwrap();
        // The core adds which directories a turn covered when it records the changes.
        record["workspacePaths"] = json!([path]);
        let content = serde_json::to_vec(&record).unwrap();
        let sha256 = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(&content))
        };
        let artifact = agent_domain::ArtifactId::new();
        let media_type = if record["schemaVersion"] == 2 {
            "application/vnd.dogcodes.turn-changes.v2+json"
        } else {
            "application/vnd.dogcodes.turn-changes.v1+json"
        };
        let metadata = agent_protocol::ArtifactMetadata {
            artifact_id: artifact.clone(),
            media_type: media_type.into(),
            payload_sha256: sha256.clone(),
            byte_length: content.len() as u64,
            created_at: chrono::Utc::now(),
            redaction_ruleset_version: 0,
            redaction_count: 0,
        };
        agent_core::CoreRepository::append(
            store.as_ref(),
            agent_core::AppendRequest {
                request_fingerprint: None,
                command_id: agent_domain::CommandId::new(),
                idempotency_key: agent_domain::CommandId::new().to_string(),
                expected_stream_version: None,
                artifacts: vec![agent_core::NewArtifact {
                    metadata: metadata.clone(),
                    content,
                }],
                events: vec![agent_protocol::NewEvent::hub(
                    agent_protocol::StreamRef {
                        stream_type: agent_protocol::StreamType::Task,
                        stream_id: task.to_string(),
                    },
                    "workspace.turn_changes_captured",
                    json!({"taskId": task, "turnId": agent_domain::TurnId::new(), "runId": agent_domain::RunId::new(), "artifact": metadata}),
                )],
            },
        )
        .await
        .unwrap();
        let undo = serde_json::from_value::<ReviewCall>(json!({
            "operation": "undo",
            "arguments": {
                "operationId": uuid::Uuid::new_v4().to_string(),
                "request": {
                    "taskId": task, "artifactId": artifact,
                    "expectedPayloadSha256": sha256, "confirmed": true,
                },
            },
        }))
        .unwrap();
        operation::call(&target, &core, &Operation::Review(undo))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(repository.join("kept.txt")).unwrap(),
            "before\n"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs an OpenSSH server binary"]
    async fn an_answer_never_arrives_after_the_connection_ends() {
        let server = Sshd::start("", "");
        let (link, _directory) = connected(&server).await;
        let target = link.target();
        let repository = server.root.path().join("repo");
        let (core, task) = project(&repository).await;
        drop(link);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let operation = Operation::Workspace {
            method: "workspace_roots".into(),
            args: json!(task.as_str()),
        };
        assert!(operation::call(&target, &core, &operation).await.is_err());
    }
}
