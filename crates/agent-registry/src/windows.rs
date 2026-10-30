//! Native CLIs downloaded by Windows desktop apps, including MSIX installs.
use super::{DiscoveryContext, Platform, home_directory};
use semver::Version;
use std::path::{Path, PathBuf};

/// Candidates are only embedded CLI files, never the desktop GUI executable.
/// The caller must still run the provider's version and readiness probes.
#[must_use]
pub fn windows_desktop_executables(provider: &str, context: &DiscoveryContext) -> Vec<PathBuf> {
    if context.platform != Platform::Windows {
        return vec![];
    }
    let Some(home) = context.home_dir.as_deref() else {
        return vec![];
    };
    let data_dir = |name: &str, fallback: &str| {
        // Fixture/custom contexts must not discover the actual user's apps.
        if home_directory().as_deref() == Some(home)
            && let Some(path) = std::env::var_os(name).map(PathBuf::from)
            && path.is_absolute()
        {
            return path;
        }
        home.join(fallback)
    };
    let local = data_dir("LOCALAPPDATA", "AppData/Local");
    match provider {
        "cursor" => {
            // `agent` is also installed by Grok Build. Only use Cursor's
            // provider-specific launcher, including the native Windows .cmd
            // shim that selects its bundled runtime/version.
            ["cursor-agent.exe", "cursor-agent.cmd"]
                .into_iter()
                .map(|name| local.join("cursor-agent").join(name))
                .filter(|path| path.is_file())
                .collect()
        }
        "zcode" => {
            let binary = local.join("Programs/ZCode/ZCode.exe");
            if binary.is_file()
                && local
                    .join("Programs/ZCode/resources/glm/zcode.cjs")
                    .is_file()
            {
                vec![binary]
            } else {
                vec![]
            }
        }
        "claude-code" => {
            let roaming = data_dir("APPDATA", "AppData/Roaming");
            let mut roots = vec![roaming.join("Claude/claude-code")];
            for package in directories(&local.join("Packages"), 256) {
                if package
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("Claude_"))
                {
                    roots.push(package.join("LocalCache/Roaming/Claude/claude-code"));
                }
            }
            let mut candidates = Vec::new();
            for root in roots {
                for version in directories(&root, 64) {
                    let Some(number) = version
                        .file_name()
                        .and_then(|v| v.to_str())
                        .and_then(|v| Version::parse(v).ok())
                    else {
                        continue;
                    };
                    for payload in directories(&version, 16) {
                        let executable = payload.join("claude.exe");
                        if executable.is_file() {
                            candidates.push((number.clone(), executable));
                        }
                    }
                }
            }
            candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            candidates.into_iter().map(|(_, path)| path).collect()
        }
        "codex" => {
            let mut candidates = directories(&local.join("OpenAI/Codex/bin"), 64);
            candidates.sort_by_cached_key(|path| {
                std::cmp::Reverse(path.metadata().and_then(|m| m.modified()).ok())
            });
            candidates
                .into_iter()
                .map(|path| path.join("codex.exe"))
                .filter(|path| path.is_file())
                .collect()
        }
        _ => vec![],
    }
}

fn directories(root: &Path, limit: usize) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(limit)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_and_grok_keep_distinct_installations_when_agent_is_shadowed() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("中文 用户");
        let grok = file(&home, ".grok/bin/grok.exe");
        file(&home, ".grok/bin/agent.exe");
        let cursor = file(&home, "AppData/Local/cursor-agent/cursor-agent.cmd");
        let context = DiscoveryContext {
            platform: Platform::Windows,
            path_entries: vec![home.join(".grok/bin")],
            home_dir: Some(home),
        };
        for (source, expected) in [
            (
                include_str!("../../../drivers/manifests/cursor.toml"),
                cursor,
            ),
            (include_str!("../../../drivers/manifests/grok.toml"), grok),
        ] {
            let manifest = crate::ProviderManifest::parse_toml(source).unwrap();
            assert!(
                !manifest
                    .discovery
                    .candidate_executables
                    .iter()
                    .any(|name| name == "agent")
            );
            assert_eq!(
                crate::discover_executables(&manifest, &context).unwrap(),
                vec![std::fs::canonicalize(expected).unwrap()]
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn discovers_unicode_home_and_redirected_application_data() {
        // Run with child-only environment overrides; never mutate the test
        // runner's global environment or require a real alternate user account.
        if let Some(root) = std::env::var_os("DOGCODES_TEST_DISCOVERY_ROOT") {
            let root = PathBuf::from(root);
            // Keep this filesystem fixture independent of real registered PATH
            // entries; refreshing the OS PATH is tested in agent-runtime.
            let context = DiscoveryContext {
                platform: Platform::Windows,
                path_entries: vec![],
                home_dir: home_directory(),
            };
            assert_eq!(context.home_dir, Some(root.join("迁移主目录/中文 用户")));
            for (source, relative) in [
                (
                    include_str!("../../../drivers/manifests/codex.toml"),
                    "重定向Local/OpenAI/Codex/bin/hash/codex.exe",
                ),
                (
                    include_str!("../../../drivers/manifests/zcode.toml"),
                    "重定向Local/Programs/ZCode/ZCode.exe",
                ),
                (
                    include_str!("../../../drivers/manifests/claude-code.toml"),
                    "重定向Roaming/Claude/claude-code/2.1.309/hash/claude.exe",
                ),
                (
                    include_str!("../../../drivers/manifests/grok.toml"),
                    "迁移主目录/中文 用户/.grok/bin/grok.exe",
                ),
                (
                    include_str!("../../../drivers/manifests/kimi.toml"),
                    "迁移主目录/中文 用户/.kimi-code/bin/kimi.exe",
                ),
            ] {
                let manifest = crate::ProviderManifest::parse_toml(source).unwrap();
                assert_eq!(
                    crate::discover_executables(&manifest, &context).unwrap(),
                    vec![std::fs::canonicalize(root.join(relative)).unwrap()],
                    "{}",
                    manifest.id
                );
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        for relative in [
            "迁移主目录/中文 用户/.grok/bin/grok.exe",
            "迁移主目录/中文 用户/.kimi-code/bin/kimi.exe",
            "重定向Local/OpenAI/Codex/bin/hash/codex.exe",
            "重定向Local/Programs/ZCode/ZCode.exe",
            "重定向Local/Programs/ZCode/resources/glm/zcode.cjs",
            "重定向Roaming/Claude/claude-code/2.1.309/hash/claude.exe",
        ] {
            file(root.path(), relative);
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::tests::discovers_unicode_home_and_redirected_application_data",
                "--nocapture",
            ])
            .env("DOGCODES_TEST_DISCOVERY_ROOT", root.path())
            .env("USERPROFILE", root.path().join("迁移主目录/中文 用户"))
            .env("LOCALAPPDATA", root.path().join("重定向Local"))
            .env("APPDATA", root.path().join("重定向Roaming"))
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn file(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, []).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn finds_msix_and_unpacked_claude_clis_by_semver_without_selecting_the_gui() {
        let home = tempfile::tempdir().unwrap();
        let gui = file(home.path(), "AppData/Local/Programs/Claude/Claude.exe");
        let old = file(
            home.path(),
            "AppData/Roaming/Claude/claude-code/2.9.0/hash/claude.exe",
        );
        let new = file(
            home.path(),
            "AppData/Local/Packages/Claude_publisher/LocalCache/Roaming/Claude/claude-code/2.10.0/hash/claude.exe",
        );
        let context = DiscoveryContext {
            platform: Platform::Windows,
            path_entries: vec![],
            home_dir: Some(home.path().into()),
        };
        let paths = windows_desktop_executables("claude-code", &context);
        assert_eq!(paths, vec![new, old]);
        assert!(!paths.contains(&gui));
        assert!(windows_desktop_executables("cursor", &context).is_empty());
        assert!(
            windows_desktop_executables(
                "claude-code",
                &DiscoveryContext {
                    platform: Platform::Macos,
                    ..context
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn zcode_requires_the_embedded_cli_and_grok_uses_its_windows_home_path() {
        let home = tempfile::tempdir().unwrap();
        let binary = file(home.path(), "AppData/Local/Programs/ZCode/ZCode.exe");
        let context = DiscoveryContext {
            platform: Platform::Windows,
            path_entries: vec![],
            home_dir: Some(home.path().into()),
        };
        assert!(windows_desktop_executables("zcode", &context).is_empty());
        file(
            home.path(),
            "AppData/Local/Programs/ZCode/resources/glm/zcode.cjs",
        );
        assert_eq!(windows_desktop_executables("zcode", &context), vec![binary]);
        let grok = file(home.path(), ".grok/bin/grok.exe");
        let manifest = crate::ProviderManifest::parse_toml(include_str!(
            "../../../drivers/manifests/grok.toml"
        ))
        .unwrap();
        assert_eq!(
            crate::discover_executables(&manifest, &context).unwrap(),
            vec![std::fs::canonicalize(grok).unwrap()]
        );
    }

    #[test]
    fn codex_fallback_finds_the_embedded_cli_when_the_gui_path_omits_it() {
        let home = tempfile::tempdir().unwrap();
        let cli = file(home.path(), "AppData/Local/OpenAI/Codex/bin/hash/codex.exe");
        let context = DiscoveryContext {
            platform: Platform::Windows,
            path_entries: vec![],
            home_dir: Some(home.path().into()),
        };
        assert_eq!(windows_desktop_executables("codex", &context), vec![cli]);
    }
}
