//! Claude's own authentication settings belong only to its native process.
use agent_runtime::{ExecutionTarget, OwnedChild, ProcessRequest, SharedTarget, TargetExecutable};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub(super) const AUTH_VARIABLES: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
];
const MARKER: &str = "\0DOGCODES_CLAUDE_ENV_V1\0";

struct ClaudeTarget {
    inner: SharedTarget,
    missing: BTreeMap<OsString, OsString>,
}
impl ExecutionTarget for ClaudeTarget {
    fn is_local(&self) -> bool {
        self.inner.is_local()
    }
    fn inherited(&self, name: &str) -> Option<OsString> {
        self.inner
            .inherited(name)
            .or_else(|| self.missing.get(OsStr::new(name)).cloned())
    }
    fn temp_directory(&self) -> PathBuf {
        self.inner.temp_directory()
    }
    fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
        self.inner.spawn(request)
    }
    fn login_environment(&self) -> BTreeMap<OsString, OsString> {
        self.inner.login_environment()
    }
    fn spawn_terminal(
        &self,
        request: ProcessRequest,
        cols: u16,
        rows: u16,
    ) -> io::Result<agent_runtime::TerminalChild> {
        self.inner.spawn_terminal(request, cols, rows)
    }
    fn spawn_operation(&self) -> io::Result<OwnedChild> {
        self.inner.spawn_operation()
    }
    fn app_tool_command(
        &self,
        command: &str,
        args: &[String],
    ) -> io::Result<(String, Vec<String>)> {
        self.inner.app_tool_command(command, args)
    }
    fn shim_program(&self) -> Option<String> {
        self.inner.shim_program()
    }
    fn runs_as_root(&self) -> bool {
        self.inner.runs_as_root()
    }
}

/// Resolve once when the installation connects, never in the middle of a turn.
/// An explicit host value wins; shell output and credentials are never logged or persisted.
pub(super) async fn prepare(mut executable: TargetExecutable) -> TargetExecutable {
    if !executable.target.is_local() || cfg!(windows) {
        return executable;
    }
    let target = executable.target.clone();
    let Some(home) = target.inherited("HOME") else {
        return executable;
    };
    let shell = target.inherited("SHELL").unwrap_or_else(|| {
        OsString::from(if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        })
    });
    if !matches!(
        std::path::Path::new(&shell)
            .file_name()
            .and_then(OsStr::to_str),
        Some("zsh" | "bash" | "fish")
    ) {
        return executable;
    }
    let mut command = agent_runtime::TargetCommand::new(target, shell);
    command
        .args([
            "-ilc",
            "printf '\\0DOGCODES_CLAUDE_ENV_V1\\0'; /usr/bin/env -0",
        ])
        .env("HOME", home)
        // `-i` makes zsh claim the foreground terminal. From `pnpm test:local`
        // that stops the shell with SIGTTOU before `.zshrc` can export credentials.
        .without_terminal();
    for key in super::ENV_ALLOWLIST
        .iter()
        .copied()
        .chain(["SHELL", "ZDOTDIR"])
        .chain(AUTH_VARIABLES.iter().copied())
    {
        command.inherit(key);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return executable,
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.terminate().await;
        return executable;
    };
    // Drop stdin so shell startup cannot consume a later native protocol request.
    drop(child.stdin.take());
    // Drain stderr while collecting a bounded, explicitly marked environment.
    let stderr = child.stderr.take().map(|mut stream| {
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
        })
    });
    let mut bytes = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(3), async {
        stdout
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        child.wait().await
    })
    .await;
    let success = matches!(read, Ok(Ok(status)) if status.success());
    // The native scope owns physical cleanup even if startup did not finish.
    let _ = child.terminate().await;
    if let Some(stderr) = stderr {
        stderr.abort();
        let _ = stderr.await;
    }
    if !success || bytes.len() > 2 * 1024 * 1024 {
        return executable;
    }
    let missing = parse(&bytes);
    executable.target = Arc::new(ClaudeTarget {
        inner: executable.target,
        missing,
    });
    executable
}
fn parse(bytes: &[u8]) -> BTreeMap<OsString, OsString> {
    let Some(start) = bytes
        .windows(MARKER.len())
        .position(|s| s == MARKER.as_bytes())
    else {
        return BTreeMap::new();
    };
    bytes[start + MARKER.len()..]
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let text = std::str::from_utf8(entry).ok()?;
            let (key, value) = text.split_once('=')?;
            (AUTH_VARIABLES.contains(&key) || ["USER", "LOGNAME"].contains(&key))
                .then(|| (OsString::from(key), OsString::from(value)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marked_shell_output_only_supplies_claude_authentication_names() {
        let value = parse(b"startup noise\0DOGCODES_CLAUDE_ENV_V1\0CLAUDE_CONFIG_DIR=/private/config\0CLAUDE_CODE_OAUTH_TOKEN=fixture=token\0UNRELATED_SECRET=private\0");
        assert_eq!(value.len(), 2);
        assert_eq!(
            value[OsStr::new("CLAUDE_CODE_OAUTH_TOKEN")],
            "fixture=token"
        );
        assert!(parse(b"CLAUDE_CODE_OAUTH_TOKEN=unmarked\0").is_empty());
    }
}

#[cfg(test)]
pub(super) fn isolated(executable: impl Into<PathBuf>, home: &std::path::Path) -> TargetExecutable {
    struct PrivateTarget(PathBuf);
    impl ExecutionTarget for PrivateTarget {
        fn is_local(&self) -> bool {
            true
        }
        fn inherited(&self, name: &str) -> Option<OsString> {
            match name {
                "HOME" => Some(self.0.clone().into_os_string()),
                "SHELL" => Some("/bin/sh".into()),
                "USER" | "LOGNAME" => Some("dogcodes-fixture".into()),
                "PATH" => std::env::var_os("PATH"),
                _ => None,
            }
        }
        fn temp_directory(&self) -> PathBuf {
            self.0.clone()
        }
        fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
            agent_runtime::local_target().spawn(request)
        }
    }
    TargetExecutable::new(Arc::new(PrivateTarget(home.into())), executable)
}

#[cfg(all(test, target_os = "macos"))]
mod shell_tests {
    use super::*;
    struct PrivateShell {
        home: PathBuf,
        explicit: bool,
    }
    impl ExecutionTarget for PrivateShell {
        fn is_local(&self) -> bool {
            true
        }
        fn inherited(&self, name: &str) -> Option<OsString> {
            match name {
                "HOME" | "ZDOTDIR" => Some(self.home.clone().into_os_string()),
                "SHELL" => Some("/bin/zsh".into()),
                "PATH" => Some("/usr/bin:/bin".into()),
                "CLAUDE_CODE_OAUTH_TOKEN" if self.explicit => Some("host-fixture".into()),
                _ => None,
            }
        }
        fn temp_directory(&self) -> PathBuf {
            self.home.clone()
        }
        fn spawn(&self, request: ProcessRequest) -> io::Result<OwnedChild> {
            agent_runtime::local_target().spawn(request)
        }
    }
    #[tokio::test]
    async fn private_login_shell_supplies_missing_auth_without_overriding_explicit_credentials() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshrc"), "export CLAUDE_CONFIG_DIR=\"$HOME/account\"\nexport CLAUDE_CODE_OAUTH_TOKEN=shell-fixture\nexport UNRELATED_SECRET=fixture-private\n").unwrap();
        for explicit in [false, true] {
            let executable = TargetExecutable::new(
                Arc::new(PrivateShell {
                    home: home.path().into(),
                    explicit,
                }),
                "unused",
            );
            let prepared = prepare(executable).await;
            assert_eq!(
                prepared
                    .target
                    .inherited("CLAUDE_CODE_OAUTH_TOKEN")
                    .unwrap(),
                if explicit {
                    "host-fixture"
                } else {
                    "shell-fixture"
                }
            );
            assert_eq!(
                prepared.target.inherited("CLAUDE_CONFIG_DIR").unwrap(),
                home.path().join("account")
            );
            assert!(prepared.target.inherited("UNRELATED_SECRET").is_none());
        }
    }
}
