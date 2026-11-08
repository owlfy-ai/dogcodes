//! Paths at an external program boundary, distinct from canonical installation identity.
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Use the ordinary Windows spelling when it denotes the same Win32-compatible
/// path. Never canonicalize, rewrite a device namespace, or change Unix paths.
#[must_use]
pub fn external_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    #[cfg(windows)]
    if let Some(value) = path.to_str().and_then(windows_plain_path) {
        return value.into();
    }
    path.to_path_buf()
}

/// Start building a command that runs without a visible console window.
/// Release builds use the Windows GUI subsystem, so a console child has no
/// console to share and would otherwise open its own window. Debug builds keep
/// a console, which is why a missing flag only shows up in the installed app.
#[must_use]
pub fn background_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

/// Start building an Agent command without exposing canonical Windows prefixes
/// to its launcher. Arguments remain untouched: callers convert known path
/// arguments explicitly, rather than changing prompts, JSON or shell source.
#[must_use]
pub fn agent_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    background_command(external_path(Path::new(program.as_ref())))
}

#[cfg(windows)]
fn windows_plain_path(value: &str) -> Option<String> {
    let candidate = if let Some(tail) = value.strip_prefix(r"\\?\UNC\") {
        let mut parts = tail.split('\\');
        if parts.next()?.is_empty() || parts.next()?.is_empty() {
            return None;
        }
        format!(r"\\{tail}")
    } else {
        let tail = value.strip_prefix(r"\\?\")?;
        if !tail.as_bytes().first()?.is_ascii_alphabetic()
            || tail.as_bytes().get(1..3) != Some(b":\\")
        {
            return None;
        }
        tail.to_owned()
    };
    // These forms need verbatim semantics. Removing the prefix could select a
    // different file/device or reintroduce the legacy path-length restriction.
    if candidate.encode_utf16().count() >= 260
        || candidate.split('\\').any(|part| {
            part.ends_with(['.', ' ']) || {
                let stem = part
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                ) || ["COM", "LPT"].into_iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|tail| {
                        matches!(
                            tail,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
                })
            }
        })
    {
        return None;
    }
    Some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_paths_and_command_payloads_are_preserved() {
        for path in ["/Users/张 三/agent", "../bin/agent", r"C:\用户\Agent.exe"] {
            assert_eq!(external_path(path), Path::new(path));
        }
        let payload = r#"{"prompt":"\\\\?\\C:\\literal user text"}"#;
        let mut command = agent_command("agent");
        command.arg(payload);
        assert_eq!(
            command.as_std().get_args().next(),
            Some(OsStr::new(payload))
        );
    }

    #[cfg(windows)]
    #[test]
    fn converts_drive_and_unc_paths_without_changing_canonical_identity() {
        for (canonical, ordinary) in [
            (
                r"\\?\C:\Program Files\agent.cmd",
                r"C:\Program Files\agent.cmd",
            ),
            (
                r"\\?\D:\用户资料\张 三\node.exe",
                r"D:\用户资料\张 三\node.exe",
            ),
            (
                r"\\?\UNC\server\share\目录\agent.exe",
                r"\\server\share\目录\agent.exe",
            ),
        ] {
            let identity = PathBuf::from(canonical);
            assert_eq!(external_path(&identity), Path::new(ordinary));
            assert_eq!(identity, Path::new(canonical));
            assert_eq!(
                agent_command(&identity).as_std().get_program(),
                OsStr::new(ordinary)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn retains_paths_that_need_verbatim_or_device_semantics() {
        for path in [
            r"\\?\Volume{abc}\agent.exe",
            r"\\.\pipe\agent",
            r"\\?\C:\folder.\agent.exe",
            r"\\?\C:\folder \agent.exe",
            r"\\?\C:\NUL.txt",
            r"\\?\UNC\server",
        ] {
            assert_eq!(external_path(path), Path::new(path));
        }
        let long = format!(r"\\?\C:\{}\agent.exe", "segment\\".repeat(40));
        assert_eq!(external_path(&long), Path::new(&long));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn canonical_batch_launchers_run_in_direct_and_owned_probes() {
        let directory = std::env::temp_dir().join(format!(
            "dogcodes-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let launcher = directory.join("Agent with spaces.cmd");
        std::fs::write(&launcher, "@echo off\r\necho CLI_PATH_OK\r\n").unwrap();
        let identity = launcher.canonicalize().unwrap();
        let mut spec = crate::ProcessSpec::probe(identity.clone(), vec![]);
        spec.cwd = Some(directory.canonicalize().unwrap());
        let direct = crate::run_bounded(&spec).await;
        let owned = crate::run_scoped_bounded(&spec).await;
        std::fs::remove_file(launcher).unwrap();
        std::fs::remove_dir(directory).unwrap();
        for result in [direct, owned] {
            let result = result.unwrap();
            assert!(result.success, "{}", result.stderr);
            assert_eq!(result.stdout.trim(), "CLI_PATH_OK");
        }
        assert_eq!(spec.executable, identity);
        assert!(spec.executable.to_string_lossy().starts_with(r"\\?\"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn children_of_a_consoleless_host_show_no_console_window() {
        use std::os::windows::process::CommandExt;
        const HOST: &str = "DOGCODES_CONSOLELESS_HOST";
        const PROBE: &str = r#"Add-Type -Namespace Probe -Name Console -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);'; [Probe.Console]::IsWindowVisible([Probe.Console]::GetConsoleWindow())"#;
        if std::env::var_os(HOST).is_some() {
            let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
            let mut spec = crate::ProcessSpec::probe(
                powershell,
                ["-NoProfile", "-NonInteractive", "-Command", PROBE]
                    .map(str::to_owned)
                    .to_vec(),
            );
            // `Add-Type` compiles C# on every run, which a busy CI machine has
            // taken over a minute to do. A window that opens doesn't slow this.
            spec.timeout = std::time::Duration::from_mins(3);
            for result in [
                crate::run_bounded(&spec).await,
                crate::run_scoped_bounded(&spec).await,
            ] {
                let result = result.expect("PowerShell probe did not finish");
                assert!(
                    result.success,
                    "PowerShell probe failed\nstdout: {}\nstderr: {}",
                    result.stdout, result.stderr
                );
                assert_eq!(result.stdout.trim(), "False", "a console window is visible");
            }
            return;
        }
        // Rerun this test detached, matching a release build's GUI subsystem
        // host. A child with no window flag would then open a visible console.
        let host = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "external_path::tests::children_of_a_consoleless_host_show_no_console_window",
                "--nocapture",
            ])
            .env(HOST, "1")
            .creation_flags(0x0000_0008) // DETACHED_PROCESS
            .output()
            .unwrap();
        // The detached host's own assertion goes to its stderr.
        assert!(
            host.status.success(),
            "detached host failed ({})\nstdout: {}\nstderr: {}",
            host.status,
            String::from_utf8_lossy(&host.stdout),
            String::from_utf8_lossy(&host.stderr)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_does_not_interpret_windows_prefixes() {
        let path = r"\\?\C:\a POSIX filename";
        assert_eq!(external_path(path), Path::new(path));
    }
}
