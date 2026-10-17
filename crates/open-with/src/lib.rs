//! System file associations only. Never execute an application supplied by the UI.
use serde::Serialize;
mod icons;
use icons::cached_icon;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub icon: Option<String>,
}

#[derive(Clone, Debug)]
struct Candidate {
    app: Application,
    location: PathBuf,
    visible: bool,
    recommended: bool,
}

fn excluded(candidate: &Candidate) -> bool {
    if !candidate.visible
        || !candidate.recommended
        || candidate.app.id.is_empty()
        || candidate.app.name.trim().is_empty()
    {
        return true;
    }
    let path = candidate
        .location
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let identity = format!("{} {}", candidate.app.id, candidate.app.name).to_lowercase();
    // Component matches avoid accidentally excluding a user's 'testing-project'.
    let unwanted_directory = path.split('/').any(|part| {
        matches!(
            part,
            "updates"
                | "updater"
                | "updaters"
                | "edgeupdater"
                | "installed_versions"
                | "manual_installed_versions"
                | "staged"
                | "staging"
                | "pending"
                | "squirreltemp"
                | "cache"
                | "caches"
                | "ms-playwright"
                | "playwright"
                | "puppeteer"
                | "test"
                | "tests"
                | "testing"
                | "helpers"
                | "frameworks"
                | "xpcservices"
        )
    });
    unwanted_directory
        || path.contains(".app/contents/")
        || identity
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| matches!(word, "test" | "tests" | "testing" | "update" | "updates"))
        || [
            "for testing",
            "for.testing",
            "chrome-headless",
            "headless_shell",
            "helper",
            "updater",
            "crashpad",
            "crashreporter",
        ]
        .iter()
        .any(|marker| identity.contains(marker))
}

fn clean(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.retain(|app| !excluded(app));
    candidates.sort_by_key(|candidate| {
        let path = candidate.location.to_string_lossy().to_lowercase();
        let standard = path.starts_with("/applications/")
            || path.starts_with("/system/")
            || path.contains("/program files/");
        (
            !candidate.app.is_default,
            !standard,
            candidate.app.name.to_lowercase(),
            path,
        )
    });
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    candidates.retain(|candidate| {
        let id = candidate.app.id.to_lowercase();
        let path = candidate.location.to_string_lossy().to_lowercase();
        if ids.contains(&id) || paths.contains(&path) {
            return false;
        }
        ids.insert(id);
        paths.insert(path);
        true
    });
    candidates
}

fn file(path: &Path) -> Result<PathBuf, String> {
    let path = path.canonicalize().map_err(|_| "文件不存在或无法访问")?;
    if !path.is_file() {
        return Err("只能打开普通文件".into());
    }
    Ok(path)
}

pub fn applications(path: &Path) -> Result<Vec<Application>, String> {
    let path = file(path)?;
    let mut candidates = clean(platform::candidates(&path)?);
    platform::load_icons(&path, &mut candidates);
    Ok(candidates
        .into_iter()
        .map(|candidate| candidate.app)
        .collect())
}

pub fn open_with(path: &Path, id: &str) -> Result<(), String> {
    let path = file(path)?;
    let application = clean(platform::candidates(&path)?)
        .into_iter()
        .find(|candidate| candidate.app.id == id)
        .ok_or("此应用已不可用，请重新打开应用列表")?;
    platform::launch(&path, &application)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::*;
    pub fn load_icons(_: &Path, _: &mut [Candidate]) {}
    pub fn candidates(_: &Path) -> Result<Vec<Candidate>, String> {
        Err("此平台暂不支持查询打开方式".into())
    }
    pub fn launch(_: &Path, _: &Candidate) -> Result<(), String> {
        Err("此平台暂不支持指定打开方式".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(id: &str, name: &str, path: &str) -> Candidate {
        Candidate {
            app: Application {
                id: id.into(),
                name: name.into(),
                is_default: false,
                icon: None,
            },
            location: path.into(),
            visible: true,
            recommended: true,
        }
    }
    #[test]
    fn filters_helpers_updates_tests_hidden_and_non_recommended_apps() {
        let mut hidden = candidate("hidden", "Hidden", "/Applications/Hidden.app");
        hidden.visible = false;
        let mut fallback = candidate("fallback", "Fallback", "/usr/bin/fallback");
        fallback.recommended = false;
        let apps = clean(vec![
            candidate("edge", "Edge", "/Applications/Microsoft Edge.app"),
            candidate(
                "edge",
                "Edge",
                "/Users/me/Library/Application Support/Microsoft/EdgeUpdater/apps/Edge.app",
            ),
            candidate(
                "doubao.browser",
                "Doubao Browser",
                "/Applications/Doubao.app/Contents/Helpers/Browser.app",
            ),
            candidate(
                "chrome.for.testing",
                "Google Chrome for Testing",
                "/Applications/Chrome Testing.app",
            ),
            candidate("updater", "App Updater", "C:\\Apps\\Update.exe"),
            candidate("update", "Update", "C:\\Apps\\Update.exe"),
            candidate("org.example.test", "Example Test", "/opt/example/test-app"),
            hidden,
            fallback,
        ]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app.id, "edge");
    }
    #[test]
    fn deduplicates_identity_and_location_and_sorts_default_first() {
        let mut default = candidate("pdf", "Z Reader", "/Applications/Z Reader.app");
        default.app.is_default = true;
        let apps = clean(vec![
            candidate("code", "Code", "/Users/me/Code.app"),
            candidate("code", "Code", "/Applications/Code.app"),
            candidate("alias", "Code", "/Applications/Code.app"),
            default,
        ]);
        assert_eq!(apps.len(), 2);
        assert!(apps[0].app.is_default);
        assert_eq!(apps[1].location, PathBuf::from("/Applications/Code.app"));
    }
}
