use agent_core::Core;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) fn configure(core: Core, state: &Path) -> Core {
    // The pinned universal executable declares macOS 13.0 in LC_BUILD_VERSION;
    // the rest of dog.codes can still run on its older supported baseline.
    #[cfg(target_os = "macos")]
    if plist::Value::from_file("/System/Library/CoreServices/SystemVersion.plist")
        .ok()
        .and_then(|value| {
            value
                .as_dictionary()?
                .get("ProductVersion")?
                .as_string()?
                .split('.')
                .next()?
                .parse::<u32>()
                .ok()
        })
        .is_some_and(|major| major < 13)
    {
        return core.with_computer_use_unavailable_reason("macos_version");
    }
    let Ok(proxy) = std::env::current_exe() else {
        return core;
    };
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .expect("bundled application config");
    let package = tauri::utils::PackageInfo {
        name: config["productName"]
            .as_str()
            .unwrap_or(env!("CARGO_PKG_NAME"))
            .into(),
        version: env!("CARGO_PKG_VERSION").parse().expect("Cargo version"),
        authors: env!("CARGO_PKG_AUTHORS"),
        description: env!("CARGO_PKG_DESCRIPTION"),
        crate_name: env!("CARGO_PKG_NAME"),
    };
    let mut candidates = vec![];
    if let Ok(resources) =
        tauri::utils::platform::resource_dir(&package, &tauri::utils::Env::default())
    {
        candidates.push(resources.join("computer-use"));
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/computer-use");
    candidates.push(development.join(env!("DOGCODES_BUILD_TARGET")));
    let Some(directory) = candidates.into_iter().find(|path| verified(path)) else {
        return core;
    };
    let executable = if cfg!(windows) {
        "cua-driver.exe"
    } else {
        "cua-driver"
    };
    let core = core.with_computer_use(
        proxy,
        directory.join(executable),
        state.join("computer-use"),
    );
    let core = if cfg!(target_os = "macos") {
        core.with_computer_use_bundle_id(
            config["identifier"]
                .as_str()
                .unwrap_or("codes.dog.desktop")
                .into(),
        )
    } else {
        core
    };
    core.with_computer_use_notice()
}

fn verified(directory: &Path) -> bool {
    let Some(manifest) = std::fs::read(directory.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return false;
    };
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    if manifest["version"] != "0.38.0"
        || manifest["platform"] != platform
        || !(manifest["arch"] == arch || (platform == "darwin" && manifest["arch"] == "universal"))
    {
        return false;
    }
    let Some(files) = manifest["files"].as_object() else {
        return false;
    };
    let required = if cfg!(windows) {
        &[
            "cua-driver.exe",
            "cua-driver-uia.exe",
            "cua-cursor-theme.exe",
        ][..]
    } else {
        &["cua-driver", "cua-cursor-theme"][..]
    };
    required.iter().all(|name| files.contains_key(*name))
        && files.iter().all(|(name, digest)| {
            Path::new(name).components().count() == 1
                && std::fs::read(directory.join(name))
                    .is_ok_and(|bytes| *digest == hex::encode(Sha256::digest(bytes)))
        })
}
