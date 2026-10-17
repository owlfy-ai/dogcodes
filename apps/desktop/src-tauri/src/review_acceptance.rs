//! Opt-in native acceptance. Reopening is restricted to seeded temporary fixtures.
#[path = "review_acceptance_directory.rs"]
mod fixture_directory;
#[path = "review_acceptance_live.rs"]
pub mod live;
#[path = "review_acceptance_multi.rs"]
mod multi;
use agent_workspace::review::{ReviewStore, SnapshotBudget};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct AcceptanceDirectory(pub PathBuf);

fn rename_fixture_text() -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for line in 0..200 {
        write!(text, "rename line {line}\r\n").expect("writing to String cannot fail");
    }
    text
}

fn added_fixture_text() -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for line in 0..200 {
        writeln!(text, "added reading line {line}").expect("writing to String cannot fail");
    }
    text
}

pub fn restart_test() -> bool {
    history_test()
}

pub fn history_test() -> bool {
    std::env::var_os("DOGCODES_REVIEW_HISTORY_REOPEN").is_some_and(|value| value == "1")
}

pub fn multi_test() -> bool {
    std::env::var_os("DOGCODES_REVIEW_MULTI_TEST").is_some_and(|value| value == "1")
}

pub fn live_test() -> bool {
    std::env::var_os("DOGCODES_REVIEW_LIVE_CODEX_PATH").is_some()
        || std::env::var_os("DOGCODES_REVIEW_LIVE_CLAUDE_PATH").is_some()
}

/// Fixed fixture only, absent from production builds. No caller-selected path
/// or content; refuse to overwrite anything except the two known test states.
#[tauri::command]
pub fn mutate_review_fixture(
    window: tauri::WebviewWindow,
    directory: tauri::State<'_, AcceptanceDirectory>,
    restore: bool,
    added: Option<bool>,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let added = added.unwrap_or(false);
    let path = directory.0.join(if added {
        "scratch/git-example/untracked.txt"
    } else {
        "scratch/git-example/renamed.txt"
    });
    if !std::fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("fixture is not a regular file".into());
    }
    let original = if added {
        added_fixture_text()
    } else {
        rename_fixture_text() + "edited after moving\r\n"
    };
    let changed = if added {
        format!("new prefix during review\n{original}")
    } else {
        original.replace(
            "rename line 150\r\n",
            "inserted during review\r\nrename line 150\r\n",
        )
    };
    let current = std::fs::read(&path).map_err(|e| e.to_string())?;
    if current != original.as_bytes() && current != changed.as_bytes() {
        return Err("fixture content changed unexpectedly".into());
    }
    std::fs::write(path, if restore { original } else { changed }).map_err(|e| e.to_string())
}

/// Only compiled into the isolated acceptance build; never accepts a file path.
#[tauri::command]
pub fn report(
    window: tauri::WebviewWindow,
    directory: tauri::State<'_, AcceptanceDirectory>,
    checks: Vec<String>,
    error: Option<String>,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let restored = std::fs::read(directory.0.join("scratch/review-example.txt"))
        .map_err(|error| error.to_string())?;
    let error = error.or_else(|| {
        (restored != b"after\nunchanged\nadded\n")
            .then(|| "native disk content was not restored".to_string())
    });
    let error = error.or_else(|| {
        (std::fs::read(directory.0.join("scratch/git-example/tracked.txt"))
            .ok()
            .as_deref()
            != Some(b"working\r\n".as_slice()))
        .then(|| "Git working bytes were not restored".to_string())
    });
    let result = serde_json::json!({ "checks": checks, "error": error, "passed": error.is_none() });
    let bytes = serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.0.join(if history_test() {
            format!(
                "history-report-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_nanos()
            )
        } else {
            "acceptance-report.json".into()
        }))
        .map_err(|error| error.to_string())?;
    std::io::Write::write_all(&mut file, &bytes).map_err(|error| error.to_string())?;
    eprintln!("ISOLATED REVIEW ACCEPTANCE result: {result}");
    Ok(())
}

pub fn new_data_directory() -> std::io::Result<PathBuf> {
    let directory = fixture_directory::allocate()?;
    eprintln!(
        "ISOLATED REVIEW ACCEPTANCE data: {} (retained for inspection)",
        directory.display()
    );
    Ok(directory)
}

/// Returns false on reopen: keep the persisted state and do not replay the
/// fresh-fixture automation over a partially completed recovery.
pub async fn seed(core: &Arc<agent_core::Core>, scratch: &Path) -> Result<bool, String> {
    let snapshot = core.bootstrap().await.map_err(|error| error.to_string())?;
    let task = snapshot.tasks.first().ok_or("acceptance task missing")?;
    let directory = scratch.parent().ok_or("acceptance root missing")?;
    // Only our freshly allocated scratch directory may receive fixture data.
    if !directory.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .starts_with("dogcodes-review-acceptance-")
    }) || scratch.file_name().is_none_or(|name| name != "scratch")
    {
        return Err("refusing to seed outside isolated acceptance root".into());
    }
    if fixture_directory::is_seeded(directory).map_err(|error| error.to_string())? {
        eprintln!("ISOLATED REVIEW ACCEPTANCE reopened: task {}", task.task_id);
        return Ok(false);
    }
    let file = scratch.join("review-example.txt");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .and_then(|mut file| std::io::Write::write_all(&mut file, b"before\nunchanged\n"))
        .map_err(|error| error.to_string())?;
    let store = ReviewStore::open(directory.join("review"))?;
    store.ensure_baseline(
        &task.task_id.to_string(),
        scratch,
        SnapshotBudget::default(),
    )?;
    std::fs::write(file, b"after\nunchanged\nadded\n").map_err(|error| error.to_string())?;
    seed_git(&scratch.join("git-example"))?;
    if live_test() {
        if live::mixed_test() {
            live::seed_mixed(core, directory).await?;
        } else {
            live::seed(core, directory).await?;
        }
    }
    if multi_test() {
        multi::seed(core, directory, task.preferred_executor.clone()).await?;
    }
    fixture_directory::mark_seeded(directory).map_err(|error| error.to_string())?;
    eprintln!("ISOLATED REVIEW ACCEPTANCE ready: task {}", task.task_id);
    Ok(true)
}

fn seed_git(root: &Path) -> Result<(), String> {
    std::fs::create_dir(root).map_err(|error| error.to_string())?;
    let git = |args: &[&str]| -> Result<(), String> {
        let result = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .args([
                "-c",
                "user.name=Review Acceptance",
                "-c",
                "user.email=review@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if result.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&result.stderr).into_owned())
        }
    };
    git(&["init", "--initial-branch=main", "--template="])?;
    std::fs::create_dir(root.join("child")).map_err(|e| e.to_string())?;
    git(&[
        "-C",
        "child",
        "init",
        "--initial-branch=main",
        "--template=",
    ])?;
    std::fs::write(root.join("child/source.txt"), "child base\n").map_err(|e| e.to_string())?;
    git(&["-C", "child", "add", "."])?;
    git(&["-C", "child", "commit", "-m", "child base"])?;
    git(&["add", "child"])?;
    std::fs::write(root.join(".gitattributes"), "*.txt text eol=crlf\n")
        .map_err(|e| e.to_string())?;
    std::fs::write(root.join("tracked.txt"), "base\r\n").map_err(|e| e.to_string())?;
    let rename_original = rename_fixture_text();
    std::fs::write(root.join("old.txt"), &rename_original).map_err(|e| e.to_string())?;
    git(&["add", "tracked.txt", "old.txt", ".gitattributes"])?;
    git(&["commit", "-m", "base"])?;
    git(&["branch", "comparison-base"])?;
    std::fs::write(root.join("tracked.txt"), "committed\r\n").map_err(|e| e.to_string())?;
    git(&["commit", "-am", "second"])?;
    std::fs::write(root.join("child/source.txt"), "child committed\n")
        .map_err(|e| e.to_string())?;
    git(&["-C", "child", "add", "-A"])?;
    git(&["-C", "child", "commit", "-am", "child second"])?;
    std::fs::write(root.join("child/source.txt"), "child working\n").map_err(|e| e.to_string())?;
    std::fs::write(root.join("child/.env"), "LOCAL_FIXTURE_ONLY=keep\n")
        .map_err(|e| e.to_string())?;
    std::fs::write(root.join("tracked.txt"), "staged\r\n").map_err(|e| e.to_string())?;
    git(&["add", "tracked.txt"])?;
    std::fs::write(root.join("tracked.txt"), "working\r\n").map_err(|e| e.to_string())?;
    std::fs::write(root.join("untracked.txt"), added_fixture_text()).map_err(|e| e.to_string())?;
    std::fs::rename(root.join("old.txt"), root.join("renamed.txt")).map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("renamed.txt"),
        format!("{rename_original}edited after moving\r\n"),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acceptance_data_is_always_a_fresh_directory() {
        let first = new_data_directory().unwrap();
        let second = new_data_directory().unwrap();
        assert_ne!(first, second);
        assert!(first.is_absolute());
        assert_eq!(std::fs::read_dir(&first).unwrap().count(), 0);
        std::fs::remove_dir(first).unwrap();
        std::fs::remove_dir(second).unwrap();
    }
}
