use std::ffi::OsString;

#[cfg(unix)]
static SHELL_PATH: std::sync::RwLock<Option<OsString>> = std::sync::RwLock::new(None);

/// Retain PATH returned by a successful login-shell discovery, without changing
/// the process environment or starting another shell from an Agent launcher.
pub fn record_shell_path(path: OsString) {
    #[cfg(unix)]
    if let Ok(mut current) = SHELL_PATH.write() {
        *current = Some(path);
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Read the current registered Windows PATH or the latest login-shell PATH.
/// Preserve launcher-private entries; never mutate the global process environment.
#[must_use]
pub fn current_path() -> Option<OsString> {
    #[cfg(windows)]
    return windows::current_path().or_else(|| std::env::var_os("PATH"));
    #[cfg(not(windows))]
    {
        if let Ok(current) = SHELL_PATH.read()
            && let Some(path) = current.as_ref()
        {
            return Some(path.clone());
        }
        std::env::var_os("PATH")
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        collections::BTreeSet,
        ffi::OsString,
        path::{Path, PathBuf},
        sync::OnceLock,
    };
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    struct PathSnapshot {
        private: Vec<PathBuf>,
    }

    impl PathSnapshot {
        fn new(inherited: Option<&std::ffi::OsStr>, registered: &[PathBuf]) -> Self {
            let registered: BTreeSet<_> = registered.iter().map(|path| path_key(path)).collect();
            Self {
                private: inherited
                    .into_iter()
                    .flat_map(std::env::split_paths)
                    .filter(|path| {
                        !path.as_os_str().is_empty() && !registered.contains(&path_key(path))
                    })
                    .collect(),
            }
        }

        fn resolve(&self, registered: &[PathBuf]) -> Option<OsString> {
            let mut seen = BTreeSet::new();
            std::env::join_paths(
                self.private
                    .iter()
                    .chain(registered)
                    .filter(|path| seen.insert(path_key(path))),
            )
            .ok()
        }
    }

    fn path_key(path: &Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    }

    pub(super) fn current_path() -> Option<OsString> {
        // Only the launcher's private additions are retained. Registered PATH
        // values and their variable references are read again on every call.
        static SNAPSHOT: OnceLock<PathSnapshot> = OnceLock::new();
        let machine = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
            .ok();
        let user = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Environment")
            .ok();
        let registered = registered_paths(machine.as_ref(), user.as_ref())?;
        SNAPSHOT
            .get_or_init(|| PathSnapshot::new(std::env::var_os("PATH").as_deref(), &registered))
            .resolve(&registered)
    }

    fn registered_paths(machine: Option<&RegKey>, user: Option<&RegKey>) -> Option<Vec<PathBuf>> {
        let lookup = |name: &str| {
            // PATH self-references cannot add useful directories and would
            // recurse forever. Other new installer variables are read lazily.
            if name.eq_ignore_ascii_case("PATH") {
                return None;
            }
            user.and_then(|key| key.get_value::<String, _>(name).ok())
                .or_else(|| machine.and_then(|key| key.get_value::<String, _>(name).ok()))
                .or_else(|| std::env::var(name).ok())
        };
        let values: Vec<String> = [machine, user]
            .into_iter()
            .flatten()
            .filter_map(|key| key.get_value::<String, _>("Path").ok())
            .collect();
        if values.is_empty() {
            return None;
        }
        Some(
            values
                .iter()
                .flat_map(|value| {
                    let expanded = expand(value, &lookup, 0);
                    std::env::split_paths(&expanded).collect::<Vec<_>>()
                })
                .collect(),
        )
    }

    fn expand(value: &str, lookup: &impl Fn(&str) -> Option<String>, depth: u8) -> String {
        if depth >= 8 {
            return value.to_owned();
        }
        let mut output = String::new();
        let mut remaining = value;
        while let Some(start) = remaining.find('%') {
            output.push_str(&remaining[..start]);
            let after = &remaining[start + 1..];
            let Some(end) = after.find('%') else {
                output.push_str(&remaining[start..]);
                return output;
            };
            if let Some(replacement) = lookup(&after[..end]) {
                output.push_str(&expand(&replacement, lookup, depth + 1));
            } else {
                output.push_str(&remaining[start..start + end + 2]);
            }
            remaining = &after[end + 1..];
        }
        output.push_str(remaining);
        output
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn repeated_refresh_observes_installs_removals_and_path_reordering() {
            let original = vec![
                PathBuf::from(r"C:\Windows\System32"),
                PathBuf::from(r"C:\OldAgent"),
            ];
            let inherited =
                std::env::join_paths([r"D:\Private Tools", r"C:\Windows\System32", r"C:\OldAgent"])
                    .unwrap();
            let snapshot = PathSnapshot::new(Some(&inherited), &original);
            let updated = vec![
                PathBuf::from(r"D:\新安装\Kimi\bin"),
                PathBuf::from(r"C:\Windows\System32"),
            ];
            assert_eq!(
                std::env::split_paths(&snapshot.resolve(&updated).unwrap()).collect::<Vec<_>>(),
                vec![
                    PathBuf::from(r"D:\Private Tools"),
                    PathBuf::from(r"D:\新安装\Kimi\bin"),
                    PathBuf::from(r"C:\Windows\System32")
                ]
            );
            let reordered = vec![
                updated[1].clone(),
                updated[0].clone(),
                PathBuf::from(r"d:\private tools"),
            ];
            assert_eq!(
                std::env::split_paths(&snapshot.resolve(&reordered).unwrap()).collect::<Vec<_>>(),
                vec![
                    PathBuf::from(r"D:\Private Tools"),
                    updated[1].clone(),
                    updated[0].clone()
                ]
            );
        }

        #[test]
        fn expands_new_unicode_environment_roots_and_bounds_cycles() {
            let lookup = |name: &str| match name {
                "AGENT_HOME" => Some("%PROFILE%\\新安装 Agent".into()),
                "PROFILE" => Some(r"D:\用户\张 三".into()),
                "CYCLE" => Some("%CYCLE%".into()),
                _ => None,
            };
            assert_eq!(
                expand("%AGENT_HOME%\\bin;%UNKNOWN%", &lookup, 0),
                r"D:\用户\张 三\新安装 Agent\bin;%UNKNOWN%"
            );
            assert_eq!(expand("%CYCLE%", &lookup, 0), "%CYCLE%");
        }

        #[test]
        fn registry_reader_sees_installer_changes_without_restarting() {
            struct Cleanup(String);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
                }
            }
            let name = format!(
                r"Software\dogcodes\path-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let _cleanup = Cleanup(name.clone());
            let (machine, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(format!(r"{name}\machine"))
                .unwrap();
            let (user, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(format!(r"{name}\user"))
                .unwrap();
            machine.set_value("Path", &r"C:\Windows\System32").unwrap();
            user.set_value("Path", &r"C:\OldAgent").unwrap();
            let original = registered_paths(Some(&machine), Some(&user)).unwrap();
            let snapshot =
                PathSnapshot::new(Some(&std::env::join_paths(&original).unwrap()), &original);
            user.set_value("AGENT_HOME", &r"D:\中文 用户\新 Agent")
                .unwrap();
            user.set_value("Path", &r"%AGENT_HOME%\bin").unwrap();
            let latest = registered_paths(Some(&machine), Some(&user)).unwrap();
            assert_eq!(
                std::env::split_paths(&snapshot.resolve(&latest).unwrap()).collect::<Vec<_>>(),
                vec![
                    PathBuf::from(r"C:\Windows\System32"),
                    PathBuf::from(r"D:\中文 用户\新 Agent\bin")
                ]
            );
        }
    }
}
