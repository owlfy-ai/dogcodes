//! Managing agents where they are installed. A server's agents are detected, read and
//! upgraded by the shim running on it: this machine only asks and shows the answer, so
//! its paths, its credentials and its network never stand in for the server's.
pub mod account;
pub mod detect;
pub mod http;
pub mod install;
pub mod operation;
pub mod provider_account;
pub mod updates;

use std::path::{Path, PathBuf};

pub fn user_data_dir(home: &Path, name: &str, fallback: &str) -> PathBuf {
    // Fixture/custom homes must not pick up the running user's folders.
    (agent_registry::home_directory().as_deref() == Some(home))
        .then(|| std::env::var_os(name))
        .flatten()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}

/// The Node and npm on `PATH`. The shim runs with the login environment, so that is
/// what a person's shell on the server would find.
pub async fn npm_runtime_on_path() -> Option<(PathBuf, PathBuf)> {
    let context = agent_registry::DiscoveryContext::from_current_process();
    let find = |name: &str| {
        context
            .path_entries
            .iter()
            .flat_map(|directory| agent_registry::command_paths(directory, name, context.platform))
            .find_map(|path| path.canonicalize().ok().filter(|real| real.is_file()))
    };
    let (node, npm) = (find("node")?, find("npm")?);
    let root = npm.parent()?.parent()?;
    let info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("package.json")).ok()?).ok()?;
    (info["name"] == "npm" && npm.file_name()?.to_str()? == "npm-cli.js").then_some((node, npm))
}
