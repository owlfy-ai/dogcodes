//! A private `sshd` and a shim built for this machine, for tests over a real connection.
#![allow(dead_code)]
use agent_protocol::{SshAuth, SshConnection};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use target_link::ShimCatalog;

const SSHD_CANDIDATES: [&str; 3] = [
    "/usr/sbin/sshd",
    "/usr/local/sbin/sshd",
    "/opt/homebrew/sbin/sshd",
];

pub fn run(program: &str, args: &[&str]) {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn current_user() -> String {
    let output = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub struct Sshd {
    pub root: tempfile::TempDir,
    pub port: u16,
    pub process: Child,
    pub client_key: PathBuf,
    pub known_hosts: PathBuf,
    pub config: PathBuf,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Sshd {
    /// `passphrase` protects the client key; `extra` is appended to `sshd_config`.
    pub fn start(passphrase: &str, extra: &str) -> Self {
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

    pub fn install_directory(&self) -> String {
        self.root
            .path()
            .join("install")
            .to_str()
            .unwrap()
            .to_owned()
    }

    pub fn connection(&self) -> SshConnection {
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

pub fn built(relative: &str) -> PathBuf {
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
pub fn catalog() -> (tempfile::TempDir, ShimCatalog) {
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
