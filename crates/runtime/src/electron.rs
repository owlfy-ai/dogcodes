use std::path::{Path, PathBuf};

use crate::ProcessError;

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
