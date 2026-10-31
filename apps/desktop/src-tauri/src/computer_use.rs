use agent_core::Core;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) fn configure(core: Core, state: &Path) -> Core {
    if !cfg!(target_os = "windows") {
        return core;
    }
    let Ok(proxy) = std::env::current_exe() else {
        return core;
    };
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/computer-use");
    let installed = proxy
        .parent()
        .unwrap_or(Path::new("."))
        .join("computer-use");
    let directory = if installed.is_dir() {
        installed
    } else {
        development
    };
    if !verified(&directory) {
        return core;
    }
    core.with_computer_use(
        proxy,
        directory.join("cua-driver.exe"),
        state.join("computer-use"),
    )
    .with_computer_use_notice()
}

fn verified(directory: &Path) -> bool {
    let Some(manifest) = std::fs::read(directory.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return false;
    };
    manifest["version"] == "0.38.0"
        && ["cua-driver.exe", "cua-driver-uia.exe"].iter().all(|name| {
            std::fs::read(directory.join(name))
                .is_ok_and(|bytes| manifest["files"][name] == hex::encode(Sha256::digest(bytes)))
        })
}
