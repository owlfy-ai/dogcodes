//! Lets an agent on this machine reach a program on the desktop without opening a
//! network port: the agent starts `mcp-relay`, which joins this shim over a Unix
//! socket inside a private directory, and the shim carries the bytes over SSH.
//! Nothing here understands the protocol, and a token only selects what the desktop
//! already agreed to run.
use crate::serve::OpenedRelay;
use std::{
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use target_wire::FIRST_RELAY;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};

const MAX_TOKEN_LINE: u64 = 256;
const TOKEN_LIMIT: Duration = Duration::from_secs(5);

pub(crate) struct Listener {
    path: String,
    socket: PathBuf,
    worker: tokio::task::JoinHandle<()>,
}

impl Listener {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.worker.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Socket paths have a short system limit, so this stays in the temporary directory
/// rather than under the install directory, which can be arbitrarily deep.
fn private_directory(base: &Path, account: u32) -> io::Result<PathBuf> {
    let directory = base.join(format!("dogcodes-{account}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // Another account could have created this name first in a shared directory.
    let metadata = std::fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.uid() != account || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the relay directory is not private to this account",
        ));
    }
    Ok(directory)
}

pub(crate) fn listen(
    nonce: &str,
    relays: mpsc::UnboundedSender<OpenedRelay>,
) -> io::Result<Listener> {
    let account = nix::unistd::geteuid().as_raw();
    let directory = private_directory(&std::env::temp_dir(), account)?;
    let short: String = nonce.chars().take(12).collect();
    let socket = directory.join(format!("{short}.sock"));
    let listener = UnixListener::bind(&socket)?;
    let path = socket.to_string_lossy().into_owned();
    let worker = tokio::spawn(accept(listener, relays));
    Ok(Listener {
        path,
        socket,
        worker,
    })
}

async fn accept(listener: UnixListener, relays: mpsc::UnboundedSender<OpenedRelay>) {
    let mut next = FIRST_RELAY;
    while let Ok((stream, _)) = listener.accept().await {
        let id = next;
        next = next.wrapping_add(1).max(FIRST_RELAY);
        let relays = relays.clone();
        tokio::spawn(async move {
            let (read, writer) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            let token = tokio::time::timeout(
                TOKEN_LIMIT,
                (&mut reader).take(MAX_TOKEN_LINE).read_line(&mut line),
            )
            .await;
            if !matches!(token, Ok(Ok(count)) if count > 0) {
                return;
            }
            let _ = relays.send(OpenedRelay {
                id,
                token: line.trim().to_owned(),
                reader: Box::new(reader),
                writer: Box::new(writer),
            });
        });
    }
}

/// The helper an agent starts: its stdio becomes the local program's stdio.
pub(crate) async fn client(socket: &str, token: &str) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket).await?;
    stream.write_all(format!("{token}\n").as_bytes()).await?;
    let (mut from_socket, mut to_socket) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let input = async {
        let result = tokio::io::copy(&mut stdin, &mut to_socket).await;
        let _ = to_socket.shutdown().await;
        result
    };
    let output = async {
        let result = tokio::io::copy(&mut from_socket, &mut stdout).await;
        let _ = stdout.flush().await;
        result
    };
    // The local program's end of output ends this helper, even while its stdin is
    // still open: the agent holds that open for as long as it likes.
    tokio::select! {
        result = output => result.map(|_| ()),
        result = async { input.await?; std::future::pending::<io::Result<u64>>().await } => result.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_open_to_others_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let account = nix::unistd::geteuid().as_raw();
        let directory = private_directory(base.path(), account).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A second start finds its own directory and accepts it.
        assert!(private_directory(base.path(), account).is_ok());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(base.path(), account).is_err());
    }

    #[test]
    fn a_symbolic_link_is_not_followed() {
        let base = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let account = nix::unistd::geteuid().as_raw();
        std::os::unix::fs::symlink(
            elsewhere.path(),
            base.path().join(format!("dogcodes-{account}")),
        )
        .unwrap();
        assert!(private_directory(base.path(), account).is_err());
    }
}
