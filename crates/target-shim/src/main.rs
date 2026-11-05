//! Runs on the server under the SSH account, with no privileges and no service
//! registration. The desktop uploads it and starts it for each connection.
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    process::ExitCode,
};
use target_wire::{CHECK_PREFIX, PROTOCOL_VERSION, TargetInfo};

#[cfg(unix)]
mod relay;
mod serve;
#[cfg(unix)]
mod terminal;

fn main() -> ExitCode {
    workspace_ops::review::git_credentials::run_if_requested();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--self-check"] => self_check(),
        ["serve", "--nonce", nonce] if valid_nonce(nonce) => serve_main(nonce),
        #[cfg(unix)]
        ["mcp-relay", socket, token] => relay_main(socket, token),
        _ => {
            eprintln!("usage: dogcodes-shim --self-check | serve --nonce <hex>");
            ExitCode::from(64)
        }
    }
}

fn valid_nonce(nonce: &str) -> bool {
    !nonce.is_empty() && nonce.len() <= 64 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Reports which binary is really installed. This proves the upload is intact and
/// that the file can execute here: wrong architecture and noexec both fail it.
fn self_check() -> ExitCode {
    match executable_digest() {
        Ok(digest) => {
            println!("{CHECK_PREFIX} {digest}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("self-check failed: {error}");
            ExitCode::from(70)
        }
    }
}

fn executable_digest() -> io::Result<String> {
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..count]);
    }
}

#[cfg(unix)]
fn relay_main(socket: &str, token: &str) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(70);
    };
    let result = runtime.block_on(relay::client(socket, token));
    // A blocking read of stdin would otherwise hold up the runtime's shutdown forever.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("relay stopped: {error}");
            ExitCode::from(74)
        }
    }
}

fn serve_main(nonce: &str) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        eprintln!("runtime could not start");
        return ExitCode::from(70);
    };
    let result = runtime.block_on(serve::serve(nonce));
    // Giving up on a silent desktop must not wait on the read of its silent stdin.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("shim stopped: {error}");
            ExitCode::from(74)
        }
    }
}

pub(crate) fn target_info() -> TargetInfo {
    TargetInfo {
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        home: std::env::var("HOME").unwrap_or_default(),
        shell: std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.is_empty()),
        root: is_root(),
        executable: std::env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        relay_socket: serve::relay_socket(),
        machine_identity: machine_identity(),
    }
}

/// Hashed so the desktop can tell machines apart without storing their raw ids.
/// Includes the account because agents, logins and files differ per user.
fn machine_identity() -> String {
    let mut hasher = Sha256::new();
    hasher.update(machine_id().as_bytes());
    hasher.update(b"|");
    hasher.update(account_id().as_bytes());
    hex::encode(hasher.finalize())[..32].to_owned()
}

#[cfg(unix)]
fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[cfg(unix)]
fn account_id() -> String {
    nix::unistd::getuid().to_string()
}

#[cfg(not(unix))]
fn account_id() -> String {
    String::new()
}

fn machine_id() -> String {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    if let Some(uuid) = macos_platform_uuid() {
        return uuid;
    }
    host_name()
}

fn macos_platform_uuid() -> Option<String> {
    if std::env::consts::OS != "macos" {
        return None;
    }
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains("IOPlatformUUID"))?;
    line.split('"').nth(3).map(str::to_owned)
}

#[cfg(unix)]
fn host_name() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_default()
}

#[cfg(not(unix))]
fn host_name() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hexadecimal_nonces_are_accepted() {
        assert!(valid_nonce("00ffAB"));
        assert!(!valid_nonce(""));
        assert!(!valid_nonce("zz"));
        assert!(!valid_nonce("00; rm -rf /"));
        assert!(!valid_nonce(&"a".repeat(65)));
    }

    #[test]
    fn identity_is_stable_and_does_not_reveal_the_machine_id() {
        let first = machine_identity();
        assert_eq!(first, machine_identity());
        assert_eq!(first.len(), 32);
        let raw = machine_id();
        assert!(raw.is_empty() || !first.contains(&raw));
    }
}
