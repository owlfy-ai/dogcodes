use std::path::{Path, PathBuf};

use crate::ProcessError;

/// Resource directory beside a Windows/Linux Electron executable or within a
/// macOS app bundle. Resolve from the discovered installation, never a fixed app path.
#[must_use]
pub fn electron_resources_directory(executable: &Path) -> Option<PathBuf> {
    // Electron's Node loader rejects verbatim script paths with EISDIR on
    // Windows. Keep the executable identity canonical, but pass ordinary paths
    // to its JavaScript entry point and bundled provider metadata.
    #[cfg(windows)]
    let normalized = {
        let value = executable.to_str()?;
        if let Some(tail) = value.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{tail}"))
        } else if let Some(tail) = value.strip_prefix(r"\\?\")
            && tail.as_bytes().get(1..3) == Some(b":\\")
        {
            PathBuf::from(tail)
        } else {
            executable.to_path_buf()
        }
    };
    #[cfg(windows)]
    let executable = normalized.as_path();
    let parent = executable.parent()?;
    if parent.file_name() == Some(std::ffi::OsStr::new("MacOS"))
        && parent.parent()?.file_name() == Some(std::ffi::OsStr::new("Contents"))
    {
        Some(parent.parent()?.join("Resources"))
    } else {
        Some(parent.join("resources"))
    }
}

/// Run bundled Node scripts with Electron's background helper. Even in Node
/// mode, the main macOS executable registers as a regular Dock application.
/// Keep the original main executable as the installation identity.
///
/// # Errors
/// Returns an error when a macOS bundle has no verified background helper.
pub fn electron_node_executable(executable: &Path) -> Result<PathBuf, ProcessError> {
    #[cfg(target_os = "macos")]
    {
        let macos = executable.parent();
        let contents = macos.and_then(Path::parent);
        if macos.and_then(Path::file_name) == Some(std::ffi::OsStr::new("MacOS"))
            && contents.and_then(Path::file_name) == Some(std::ffi::OsStr::new("Contents"))
            && let Some(contents) = contents
        {
            let name = executable
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    ProcessError::InvalidSpec("invalid Electron executable name".into())
                })?;
            let helper_name = format!("{name} Helper");
            let helper_contents = contents
                .join("Frameworks")
                .join(format!("{helper_name}.app/Contents"));
            let helper = helper_contents.join("MacOS").join(helper_name);
            let background = plist::Value::from_file(helper_contents.join("Info.plist"))
                .ok()
                .and_then(|value| {
                    value
                        .as_dictionary()
                        .and_then(|dict| dict.get("LSUIElement"))
                        .and_then(plist::Value::as_boolean)
                })
                == Some(true);
            if background && helper.is_file() {
                return Ok(helper);
            }
            // Never fall back to the foreground app and flash a Dock icon.
            return Err(ProcessError::InvalidSpec(
                "Electron background helper is unavailable".into(),
            ));
        }
    }
    Ok(executable.to_path_buf())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundled_runtime_requires_a_background_helper() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("dogcodes-electron-{}", std::process::id()));
        let main = root.join("Example.app/Contents/MacOS/Example");
        let helper_contents =
            root.join("Example.app/Contents/Frameworks/Example Helper.app/Contents");
        let helper = helper_contents.join("MacOS/Example Helper");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, "#!/bin/sh\nprintf background").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(main.parent().unwrap()).unwrap();
        std::fs::write(&main, "#!/bin/sh\nprintf foreground").unwrap();
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(electron_node_executable(&main).is_err());
        let mut metadata = plist::Dictionary::new();
        metadata.insert("LSUIElement".into(), plist::Value::Boolean(false));
        plist::Value::Dictionary(metadata.clone())
            .to_file_xml(helper_contents.join("Info.plist"))
            .unwrap();
        assert!(electron_node_executable(&main).is_err());
        metadata.insert("LSUIElement".into(), plist::Value::Boolean(true));
        plist::Value::Dictionary(metadata)
            .to_file_xml(helper_contents.join("Info.plist"))
            .unwrap();
        assert_eq!(electron_node_executable(&main).unwrap(), helper);
        let mut spec = crate::ProcessSpec::probe(main.clone(), vec![]);
        spec.env_overrides
            .insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
        let result = crate::run_bounded(&spec).await.unwrap();
        assert!(result.success);
        assert_eq!(result.stdout, "background");
        std::fs::remove_file(helper).unwrap();
        assert!(electron_node_executable(&main).is_err());
        assert!(crate::run_bounded(&spec).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(
            electron_node_executable(Path::new("/usr/bin/node")).unwrap(),
            Path::new("/usr/bin/node")
        );
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn node_script_paths_do_not_retain_windows_verbatim_prefixes() {
        for (binary, expected) in [
            (
                r"\\?\C:\Programs\ZCode\ZCode.exe",
                r"C:\Programs\ZCode\resources",
            ),
            (
                r"\\?\D:\用户资料\张 三\本地应用\ZCode\ZCode.exe",
                r"D:\用户资料\张 三\本地应用\ZCode\resources",
            ),
            (
                r"\\?\UNC\server\share\ZCode\ZCode.exe",
                r"\\server\share\ZCode\resources",
            ),
        ] {
            assert_eq!(
                electron_resources_directory(Path::new(binary)),
                Some(PathBuf::from(expected))
            );
        }
    }

    #[test]
    fn resolves_resources_from_each_installation_layout() {
        let root = std::env::temp_dir().join("迁移主目录/中文 用户/dogcodes-electron-layout");
        for (binary, resources) in [
            ("Programs/ZCode/ZCode.exe", "Programs/ZCode/resources"),
            (
                "Applications/ZCode.app/Contents/MacOS/ZCode",
                "Applications/ZCode.app/Contents/Resources",
            ),
        ] {
            assert_eq!(
                electron_resources_directory(&root.join(binary)),
                Some(root.join(resources))
            );
        }
    }
}
