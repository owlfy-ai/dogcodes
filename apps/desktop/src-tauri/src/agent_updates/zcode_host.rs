//! Read the Windows desktop's version, never compare its feed to the bundled CLI.
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(super) fn version(app: &Path) -> Option<String> {
    let mut file = File::open(app.join("resources/app.asar")).ok()?;
    let mut header = [0_u8; 16];
    file.read_exact(&mut header).ok()?;
    let header_size = u32::from_le_bytes(header[4..8].try_into().ok()?);
    let json_size = u32::from_le_bytes(header[12..16].try_into().ok()?);
    if json_size > 8 * 1024 * 1024 || header_size < json_size.checked_add(8)? {
        return None;
    }
    let mut bytes = vec![0; usize::try_from(json_size).ok()?];
    file.read_exact(&mut bytes).ok()?;
    let index: Value = serde_json::from_slice(&bytes).ok()?;
    let package = &index["files"]["package.json"];
    if package["unpacked"] == true || !package["link"].is_null() {
        return None;
    }
    let size = package["size"].as_u64()?;
    if size > 65_536 {
        return None;
    }
    let offset = package["offset"].as_str()?.parse::<u64>().ok()?;
    let start = 8_u64
        .checked_add(u64::from(header_size))?
        .checked_add(offset)?;
    if start.checked_add(size)? > file.metadata().ok()?.len() {
        return None;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    bytes.resize(usize::try_from(size).ok()?, 0);
    file.read_exact(&mut bytes).ok()?;
    let package: Value = serde_json::from_slice(&bytes).ok()?;
    semver::Version::parse(package["version"].as_str()?)
        .ok()
        .map(|v| v.to_string())
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
