//! Read the Windows desktop's version, never compare its feed to the bundled CLI.
use std::path::Path;

pub(super) fn version(app: &Path) -> Option<String> {
    super::electron_host::version(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_updates::{Plan, base, plan, tests::installation_for};

    #[test]
    fn windows_host_uses_packaged_desktop_version_and_rejects_truncated_archives() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("ZCode");
        std::fs::create_dir_all(app.join("resources/glm")).unwrap();
        let executable = app.join("ZCode.exe");
        std::fs::write(&executable, "").unwrap();
        std::fs::write(app.join("resources/glm/zcode.cjs"), "").unwrap();
        let package = br#"{"version":"3.16.3"}"#;
        let json = format!(
            r#"{{"files":{{"package.json":{{"offset":"0","size":{}}}}}}}"#,
            package.len()
        );
        let mut archive = Vec::new();
        let json_size = u32::try_from(json.len()).unwrap();
        for size in [4, json_size + 8, json_size + 4, json_size] {
            archive.extend_from_slice(&size.to_le_bytes());
        }
        archive.extend_from_slice(json.as_bytes());
        archive.extend_from_slice(package);
        let path = app.join("resources/app.asar");
        std::fs::write(&path, &archive).unwrap();
        let installation = installation_for("zcode", &executable, "0.17.0");
        assert!(matches!(plan(&installation), Plan::Host(_)));
        assert_eq!(
            base(&installation).desktop_version.as_deref(),
            Some("3.16.3")
        );
        archive.pop();
        std::fs::write(&path, &archive).unwrap();
        assert!(version(&app).is_none());
        archive[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &archive).unwrap();
        assert!(version(&app).is_none());
    }
}
