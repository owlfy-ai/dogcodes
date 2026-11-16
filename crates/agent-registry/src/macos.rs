//! Agent runtimes bundled with or downloaded by macOS desktop apps.
use super::{DiscoveryContext, Platform, home_directory, windows::claude_code_payloads};
use std::path::PathBuf;

/// Candidates are agent runtimes; Electron apps require their bundled script in Node mode.
/// The caller must still run the provider's version and readiness probes.
#[must_use]
pub fn macos_desktop_executables(provider: &str, context: &DiscoveryContext) -> Vec<PathBuf> {
    if context.platform != Platform::Macos {
        return vec![];
    }
    let Some(home) = context.home_dir.as_deref() else {
        return vec![];
    };
    let mut apps = vec![home.join("Applications")];
    // A private discovery context must never pick up the machine's real apps.
    if home_directory().as_deref() == Some(home) {
        apps.push("/Applications".into());
    }
    if provider == "claude-code" {
        return claude_code_payloads(
            &[home.join("Library/Application Support/Claude/claude-code")],
            "claude.app/Contents/MacOS/claude",
        );
    }
    let relative: &[&str] = match provider {
        "codex" => &[
            "ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
            "ChatGPT.app/Contents/Resources/codex",
            "Codex.app/Contents/Resources/codex-cli/bin/codex",
            "Codex.app/Contents/Resources/codex",
        ],
        "zcode" => &["ZCode.app/Contents/MacOS/ZCode"],
        "deepseek-harness" => &["DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh"],
        "opencode" => &["OpenCode.app/Contents/Resources/opencode-cli"],
        _ => &[],
    };
    apps.into_iter()
        .flat_map(|root| relative.iter().map(move |path| root.join(path)))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
    fn claude_desktop_cli_is_found_by_semver_without_overriding_the_terminal_install() {
        let home = tempfile::tempdir().unwrap();
        let root = "Library/Application Support/Claude/claude-code";
        let old = file(
            home.path(),
            &format!("{root}/2.1.304/aaa/claude.app/Contents/MacOS/claude"),
        );
        let new = file(
            home.path(),
            &format!("{root}/2.1.309/5af865b9901c/claude.app/Contents/MacOS/claude"),
        );
        file(
            home.path(),
            &format!("{root}/not-a-version/x/claude.app/Contents/MacOS/claude"),
        );
        file(home.path(), "Applications/Claude.app/Contents/MacOS/Claude");
        let context = DiscoveryContext {
            platform: Platform::Macos,
            path_entries: vec![],
            home_dir: Some(home.path().into()),
        };
        assert_eq!(
            macos_desktop_executables("claude-code", &context),
            vec![new.clone(), old]
        );
        assert!(macos_desktop_executables("codex", &context).is_empty());
        assert!(
            macos_desktop_executables(
                "claude-code",
                &DiscoveryContext {
                    platform: Platform::Windows,
                    ..context.clone()
                }
            )
            .is_empty()
        );

        // Both installations are reported; terminal precedence is the caller's choice.
        let terminal = file(home.path(), ".local/bin/claude");
        let mut manifest = crate::ProviderManifest::parse_toml(include_str!(
            "../../../drivers/manifests/claude-code.toml"
        ))
        .unwrap();
        manifest.discovery.platform_paths.clear();
        let found = crate::discover_executables(&manifest, &context).unwrap();
        assert!(found.contains(&std::fs::canonicalize(terminal).unwrap()));
        assert!(found.contains(&std::fs::canonicalize(new).unwrap()));
    }
}
