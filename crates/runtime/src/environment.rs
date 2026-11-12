use tokio::process::Command;

/// Copy only an explicitly requested variable, refreshing Windows PATH so a
/// newly discovered CLI and its helpers receive the same installation context.
#[must_use]
pub fn inherited_variable(name: &str) -> Option<std::ffi::OsString> {
    if name == "PATH" {
        return crate::current_path();
    }
    #[cfg(windows)]
    if name.eq_ignore_ascii_case("PATH") {
        return crate::current_path();
    }
    std::env::var_os(name)
}

/// Restore Windows OS/profile locations after `env_clear`, without inheriting
/// credentials or unrelated application settings. Call before adapter overrides.
pub fn apply_platform_environment(command: &mut Command) {
    #[cfg(windows)]
    for name in [
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        // Win32-OpenSSH exits with 255 before even handling `-V` without this.
        "PROGRAMDATA",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn cleared_commands_retain_windows_locations_without_other_host_variables() {
        let mut command = Command::new("unused.exe");
        command.env_clear();
        apply_platform_environment(&mut command);
        let names = [
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "PROGRAMDATA",
        ];
        let actual = command.as_std().get_envs().collect::<Vec<_>>();
        for name in names {
            if let Some(expected) = std::env::var_os(name) {
                assert!(actual.iter().any(|(key, value)| {
                    key.eq_ignore_ascii_case(name) && *value == Some(expected.as_os_str())
                }));
            }
        }
        assert!(
            actual
                .iter()
                .all(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
        );
    }

    #[tokio::test]
    async fn windows_openssh_runs_with_a_cleared_environment() {
        use std::path::PathBuf;

        let system =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/OpenSSH/ssh.exe");
        let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src-tauri/resources/openssh/ssh.exe");
        let mut checked = 0;
        for executable in [system, bundled].into_iter().filter(|path| path.is_file()) {
            // Neither command connects or reads the person's SSH config.
            for args in [vec!["-V"], vec!["-G", "-F", "NUL", "localhost"]] {
                let output = crate::run_bounded(&crate::ProcessSpec::probe(
                    executable.clone(),
                    args.iter().map(|arg| (*arg).to_owned()).collect(),
                ))
                .await
                .unwrap();
                assert!(
                    output.success,
                    "{} {args:?}: {output:?}",
                    executable.display()
                );
                if args == ["-V"] {
                    assert!(output.stderr.contains("OpenSSH_"), "{output:?}");
                } else {
                    assert!(output.stdout.contains("hostname localhost"), "{output:?}");
                }
            }
            checked += 1;
        }
        if checked == 0 {
            eprintln!("skipped: no system or bundled Windows OpenSSH client");
        }
    }
}
