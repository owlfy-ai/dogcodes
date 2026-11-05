//! Locates the OpenSSH client. A GUI-launched service can have a thin `PATH`, so
//! the usual install locations are also tried.
use crate::error::{ConnectError, ConnectErrorKind};
use agent_runtime::{ProcessSpec, run_bounded};
use std::{ffi::OsString, path::PathBuf};

/// `SSH_ASKPASS_REQUIRE` arrived in 8.4; earlier clients cannot be made to prompt
/// through us, so they are limited to authentication that needs no prompt.
const ASKPASS_REQUIRE_MINIMUM: (u32, u32) = (8, 4);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshProgram {
    pub path: PathBuf,
    pub version: (u32, u32),
}

impl SshProgram {
    #[must_use]
    pub fn supports_askpass(&self) -> bool {
        self.version >= ASKPASS_REQUIRE_MINIMUM
    }
}

fn file_name() -> &'static str {
    if cfg!(windows) { "ssh.exe" } else { "ssh" }
}

fn candidates(
    path: Option<OsString>,
    system_root: Option<OsString>,
    bundled: &[PathBuf],
) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = path
        .iter()
        .flat_map(std::env::split_paths)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(file_name()))
        .collect();
    if cfg!(windows) {
        if let Some(root) = system_root {
            found.push(PathBuf::from(root).join("System32/OpenSSH/ssh.exe"));
        }
    } else {
        for fixed in [
            "/usr/bin/ssh",
            "/usr/local/bin/ssh",
            "/opt/homebrew/bin/ssh",
        ] {
            found.push(fixed.into());
        }
    }
    // Last: what the person has installed keeps their config and agent working as they
    // expect. This is for a machine with no client, or one too old to prompt through us.
    found.extend(bundled.iter().map(|directory| directory.join(file_name())));
    found
}

/// The first usable client wins, matching what the user's own shell would run.
/// An absolute path also matters on Windows, where `ProxyJump` respawns `argv[0]`.
pub async fn find_ssh(bundled: &[PathBuf]) -> Result<SshProgram, ConnectError> {
    let mut too_old = None;
    for path in candidates(
        agent_runtime::current_path(),
        std::env::var_os("SystemRoot"),
        bundled,
    ) {
        if !path.is_file() {
            continue;
        }
        if let Some(version) = program_version(&path).await {
            let program = SshProgram { path, version };
            if program.supports_askpass() {
                return Ok(program);
            }
            too_old.get_or_insert(program);
        }
    }
    too_old.ok_or_else(|| {
        ConnectError::new(ConnectErrorKind::SshMissing, "no OpenSSH client was found")
    })
}

async fn program_version(path: &std::path::Path) -> Option<(u32, u32)> {
    let output = run_bounded(&ProcessSpec::probe(path.to_path_buf(), vec!["-V".into()]))
        .await
        .ok()?;
    parse_version(&output.stderr).or_else(|| parse_version(&output.stdout))
}

/// `OpenSSH_10.3p1, LibreSSL 3.3.6` or `OpenSSH_for_Windows_8.1p1, ...`.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let tail = text.split("OpenSSH_").nth(1)?;
    let tail = tail.strip_prefix("for_Windows_").unwrap_or(tail);
    let mut numbers = tail.split(|c: char| !c.is_ascii_digit());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_each_known_banner() {
        assert_eq!(
            parse_version("OpenSSH_10.3p1, LibreSSL 3.3.6"),
            Some((10, 3))
        );
        assert_eq!(
            parse_version("OpenSSH_for_Windows_8.1p1, LibreSSL 3.0.2"),
            Some((8, 1))
        );
        assert_eq!(
            parse_version("OpenSSH_9.6p1 Ubuntu-3ubuntu13"),
            Some((9, 6))
        );
        assert_eq!(parse_version("Dropbear v2022.83"), None);
    }

    #[test]
    fn the_askpass_boundary_is_8_4() {
        let at = |version| SshProgram {
            path: "/usr/bin/ssh".into(),
            version,
        };
        assert!(!at((8, 1)).supports_askpass());
        assert!(!at((7, 7)).supports_askpass());
        assert!(at((8, 4)).supports_askpass());
        assert!(at((10, 0)).supports_askpass());
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_entries_are_ignored() {
        let found = candidates(Some("relative:/opt/bin".into()), None, &[]);
        assert_eq!(found[0], PathBuf::from("/opt/bin/ssh"));
        assert!(found.iter().all(|path| path.is_absolute()));
    }

    #[cfg(unix)]
    #[test]
    fn a_bundled_client_is_only_a_last_resort() {
        let found = candidates(
            Some(std::env::join_paths(["/opt/bin"]).unwrap()),
            Some("/windows".into()),
            &["/bundled".into()],
        );
        assert!(found[0].starts_with("/opt/bin"));
        assert!(found.last().unwrap().starts_with("/bundled"));
        assert_eq!(
            found
                .iter()
                .filter(|path| path.starts_with("/bundled"))
                .count(),
            1
        );
    }
}
