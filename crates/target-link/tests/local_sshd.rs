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
        program: find_ssh().await.unwrap(),
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
    let program = find_ssh().await.unwrap();
    // `-G` reads the user's real config; a plain name still resolves to itself.
    let resolved = target_link::ssh_config::resolve(&program, "example-host-for-test")
        .await
        .unwrap();
    assert_eq!(resolved.host, "example-host-for-test");
    assert!(server.port > 0);
}

/// Starts a process on the target through the same path a driver uses.
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
async fn the_target_reports_the_account_login_environment() {
    let server = Sshd::start("", "");
    let (link, _directory) = connected(&server).await;
    let target = link.target();
    assert!(!target.is_local());
    assert!(target.inherited("PATH").is_some());
    assert!(target.inherited("DEFINITELY_NOT_SET_ANYWHERE").is_none());
    assert!(target.temp_directory().is_absolute());
}
