//! The shim binaries packaged with the app, one per supported target platform.
use crate::error::{ConnectError, ConnectErrorKind};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const SHIM_FILE: &str = "dogcodes-shim";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShimPayload {
    pub bytes: Vec<u8>,
    pub digest: String,
    /// Immutable directory name: a new build never overwrites a running shim.
    pub version_directory: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShimCatalog {
    directories: Vec<PathBuf>,
}

impl ShimCatalog {
    #[must_use]
    pub fn new(directories: Vec<PathBuf>) -> Self {
        Self { directories }
    }

    /// `uname -s` and `uname -m` as reported by the target.
    pub fn payload(&self, os: &str, machine: &str) -> Result<ShimPayload, ConnectError> {
        let triple = target_triple(os, machine).ok_or_else(|| {
            ConnectError::new(
                ConnectErrorKind::UnsupportedPlatform,
                format!("{os} {machine}"),
            )
        })?;
        for directory in &self.directories {
            if let Ok(bytes) = std::fs::read(directory.join(triple).join(SHIM_FILE)) {
                let digest = hex::encode(Sha256::digest(&bytes));
                let version_directory = format!("{}-{}", env!("CARGO_PKG_VERSION"), &digest[..16]);
                return Ok(ShimPayload {
                    bytes,
                    digest,
                    version_directory,
                });
            }
        }
        Err(ConnectError::new(
            ConnectErrorKind::ShimUnavailable,
            format!("no packaged shim for {triple}"),
        ))
    }
}

#[must_use]
pub fn target_triple(os: &str, machine: &str) -> Option<&'static str> {
    match (os, machine) {
        ("Linux", "x86_64" | "amd64") => Some("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64" | "arm64") => Some("aarch64-unknown-linux-musl"),
        ("Darwin", "x86_64") => Some("x86_64-apple-darwin"),
        ("Darwin", "arm64" | "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_values_map_to_packaged_targets() {
        assert_eq!(
            target_triple("Linux", "x86_64"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_triple("Linux", "aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            target_triple("Darwin", "arm64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(target_triple("FreeBSD", "amd64"), None);
        assert_eq!(target_triple("Linux", "riscv64"), None);
    }

    #[test]
    fn a_missing_platform_and_a_missing_file_are_different_failures() {
        let directory = tempfile::tempdir().unwrap();
        let catalog = ShimCatalog::new(vec![directory.path().into()]);
        assert_eq!(
            catalog.payload("FreeBSD", "amd64").unwrap_err().kind,
            ConnectErrorKind::UnsupportedPlatform
        );
        assert_eq!(
            catalog.payload("Linux", "x86_64").unwrap_err().kind,
            ConnectErrorKind::ShimUnavailable
        );
    }

    #[test]
    fn the_version_directory_changes_with_the_binary() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("x86_64-unknown-linux-musl");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join(SHIM_FILE), b"one").unwrap();
        let catalog = ShimCatalog::new(vec![directory.path().into()]);
        let first = catalog.payload("Linux", "x86_64").unwrap();
        std::fs::write(folder.join(SHIM_FILE), b"two").unwrap();
        let second = catalog.payload("Linux", "x86_64").unwrap();
        assert_ne!(first.version_directory, second.version_directory);
        assert_eq!(first.bytes, b"one");
    }
}
