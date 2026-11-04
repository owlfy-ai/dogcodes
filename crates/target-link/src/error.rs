use serde::Serialize;

/// Stable kinds so the interface can explain and localize each failure itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectErrorKind {
    SshMissing,
    SshTooOld,
    AuthFailed,
    /// The saved key differs from the one presented. Never offered as ignorable.
    HostKeyChanged,
    HostKeyRejected,
    HostUnreachable,
    Timeout,
    Cancelled,
    /// Authenticated, but the account may not run commands (`git-shell`, `ForceCommand`).
    AccountRestricted,
    UnsupportedPlatform,
    ShimUnavailable,
    HomeMissing,
    DirectoryUnwritable,
    UnsafeDirectory,
    UploadFailed,
    /// Files cannot execute here: a noexec mount or a security policy.
    NotExecutable,
    /// The binary ran and reported a different digest, or could not run at all.
    BadBinary,
    ProtocolMismatch,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectError {
    pub kind: ConnectErrorKind,
    /// What `ssh` or the shim reported; shown only in the connection log.
    pub detail: String,
}

impl ConnectError {
    pub(crate) fn new(kind: ConnectErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ConnectError {}

/// `ssh` reports these only as text. Its own messages are not localized, while
/// system errors can be, so anything unrecognized stays `Failed` with its detail.
pub(crate) fn classify_ssh_failure(stderr: &str) -> ConnectErrorKind {
    let text = stderr.to_ascii_lowercase();
    let has = |needle: &str| text.contains(needle);
    if has("remote host identification has changed") {
        ConnectErrorKind::HostKeyChanged
    } else if has("host key verification failed") {
        ConnectErrorKind::HostKeyRejected
    } else if has("permission denied") || has("too many authentication failures") {
        ConnectErrorKind::AuthFailed
    } else if has("could not resolve hostname")
        || has("name or service not known")
        || has("nodename nor servname")
        || has("connection refused")
        || has("no route to host")
        || has("network is unreachable")
    {
        ConnectErrorKind::HostUnreachable
    } else if has("timed out") {
        ConnectErrorKind::Timeout
    } else {
        ConnectErrorKind::Failed
    }
}

/// Failure codes printed by the boot script (see `boot.rs`).
pub(crate) fn classify_boot_failure(code: &str) -> ConnectErrorKind {
    match code {
        "home-missing" => ConnectErrorKind::HomeMissing,
        "dir-unwritable" => ConnectErrorKind::DirectoryUnwritable,
        "unsafe-dir" => ConnectErrorKind::UnsafeDirectory,
        "upload-failed" => ConnectErrorKind::UploadFailed,
        "noexec" => ConnectErrorKind::NotExecutable,
        "bad-binary" => ConnectErrorKind::BadBinary,
        _ => ConnectErrorKind::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_messages_map_to_actionable_kinds() {
        for (text, kind) in [
            (
                "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
                ConnectErrorKind::HostKeyChanged,
            ),
            (
                "Host key verification failed.",
                ConnectErrorKind::HostKeyRejected,
            ),
            (
                "dev@box: Permission denied (publickey,password).",
                ConnectErrorKind::AuthFailed,
            ),
            (
                "ssh: Could not resolve hostname nope: nodename nor servname provided",
                ConnectErrorKind::HostUnreachable,
            ),
            (
                "ssh: connect to host a port 22: Connection refused",
                ConnectErrorKind::HostUnreachable,
            ),
            (
                "ssh: connect to host a port 22: Operation timed out",
                ConnectErrorKind::Timeout,
            ),
            (
                "kex_exchange_identification: read: Connection reset",
                ConnectErrorKind::Failed,
            ),
        ] {
            assert_eq!(classify_ssh_failure(text), kind, "{text}");
        }
    }

    #[test]
    fn a_changed_host_key_is_never_reported_as_merely_rejected() {
        let text = "REMOTE HOST IDENTIFICATION HAS CHANGED\nHost key verification failed.\nPermission denied";
        assert_eq!(classify_ssh_failure(text), ConnectErrorKind::HostKeyChanged);
    }
}
