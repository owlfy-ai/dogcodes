//! Private files of a session, on the machine `agy` runs on, gone when the session is.
use agent_runtime::{Pipe, SharedTarget, TargetCommand};
use std::{
    io,
    path::{Path, PathBuf},
};

enum Owner {
    Local(#[allow(dead_code)] tempfile::TempDir),
    Target(SharedTarget),
}

pub(super) struct Scratch {
    path: PathBuf,
    owner: Owner,
}

impl Scratch {
    pub(super) async fn create(target: &SharedTarget, prefix: &str) -> io::Result<Self> {
        if target.is_local() {
            let directory = tempfile::Builder::new().prefix(prefix).tempdir()?;
            return Ok(Self {
                path: directory.path().to_owned(),
                owner: Owner::Local(directory),
            });
        }
        Ok(Self {
            path: agent_runtime::create_target_directory(target).await?,
            owner: Owner::Target(target.clone()),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) async fn write(&self, relative: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path.join(relative);
        match &self.owner {
            Owner::Local(_) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, bytes)
            }
            Owner::Target(target) => {
                agent_runtime::write_target_file(target, &path.to_string_lossy(), bytes).await
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Owner::Target(target) = &self.owner
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let (target, path) = (target.clone(), self.path.clone());
            runtime.spawn(async move {
                let _ = agent_runtime::remove_target_directory(&target, &path).await;
            });
        }
    }
}

/// At most `limit` bytes of a file on the machine `agy` runs on.
pub(super) async fn read_file(target: &SharedTarget, path: &Path, limit: usize) -> Option<Vec<u8>> {
    if target.is_local() {
        let path = path.to_owned();
        return tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .ok()?
                .take(limit as u64)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(bytes)
        })
        .await
        .ok()?;
    }
    let mut command = TargetCommand::new(target.clone(), "/bin/sh");
    command
        .args(["-c", r#"exec head -c "$1" -- "$2""#, "sh"])
        .arg(limit.to_string())
        .arg(path)
        .inherit("PATH")
        .stdio(Pipe::Null, Pipe::Piped, Pipe::Null);
    let output = command.spawn().ok()?.wait_with_output(limit).await.ok()?;
    output.status.success().then_some(output.stdout)
}
