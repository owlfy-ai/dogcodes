//! Resolve npm's Windows launchers back to the selected installation's prefix.
use super::{Plan, package};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn npm_package(provider: &str, executable: &Path) -> Option<PathBuf> {
    if !executable.extension()?.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let package = package(provider)?;
    let directory = executable.parent()?;
    let modules = if directory.file_name()? == ".bin" {
        directory.parent()?.to_path_buf()
    } else {
        directory.join("node_modules")
    };
    if modules.file_name()? != "node_modules" {
        return None;
    }
    let root = modules.join(package);
    let manifest = root.join("package.json");
    if std::fs::metadata(&manifest).ok()?.len() > 65_536
        || std::fs::metadata(executable).ok()?.len() > 65_536
    {
        return None;
    }
    let info: Value = serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
    if info["name"].as_str()? != package {
        return None;
    }
    let shim = std::fs::read_to_string(executable).ok()?.replace('\\', "/");
    if !shim.contains(&format!("node_modules/{package}/"))
        && !shim.contains(&format!("../{package}/"))
    {
        return None;
    }
    root.canonicalize().ok()
}

pub(super) fn npm_plan(provider: &str, executable: &Path) -> Option<Plan> {
    let root = npm_package(provider, executable)?;
    let modules = root
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "node_modules"))?;
    let prefix = modules.parent()?;
    let package = package(provider)?;
    if provider == "deepseek-harness" && super::new_agents::is_dsh_cache(prefix, &root) {
        return Some(Plan::NpmLocal {
            prefix: prefix.into(),
            package,
        });
    }
    let node = prefix.join("node.exe");
    let npm = modules.join("npm/bin/npm-cli.js");
    (node.is_file() && npm.is_file()).then(|| Plan::Npm {
        node,
        npm,
        prefix: prefix.into(),
        package,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_updates::{install_spec, tests::installation_for};

    #[test]
    fn npm_launcher_updates_its_own_prefix_and_rejects_other_packages() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("node with spaces");
        let package = prefix.join("node_modules/@tencent-ai/codebuddy-code");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(prefix.join("node_modules/npm/bin")).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@tencent-ai/codebuddy-code"}"#,
        )
        .unwrap();
        std::fs::write(prefix.join("node.exe"), "").unwrap();
        std::fs::write(prefix.join("node_modules/npm/bin/npm-cli.js"), "").unwrap();
        let launcher = prefix.join("codebuddy.cmd");
        std::fs::write(
            &launcher,
            r#""%dp0%\node_modules\@tencent-ai\codebuddy-code\bin\codebuddy" %*"#,
        )
        .unwrap();
        let plan = npm_plan("codebuddy", &launcher).unwrap();
        let spec = install_spec(
            &installation_for("codebuddy", &launcher, "2.178.1"),
            &plan,
            "2.179.0",
        )
        .unwrap();
        assert_eq!(
            spec.executable,
            agent_runtime::external_path(prefix.join("node.exe").canonicalize().unwrap())
        );
        assert!(
            spec.args.contains(
                &agent_runtime::external_path(prefix.canonicalize().unwrap())
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(
            spec.args
                .contains(&"@tencent-ai/codebuddy-code@2.179.0".into())
        );
        std::fs::write(&launcher, "foreign launcher").unwrap();
        assert!(npm_plan("codebuddy", &launcher).is_none());
    }
}
