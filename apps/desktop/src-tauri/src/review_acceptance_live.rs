use super::{Arc, Path, ReviewStore};
use agent_domain::{InstallationId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask};
use driver_api::AgentDriver;

pub async fn seed(core: &Arc<agent_core::Core>, directory: &Path) -> Result<(), String> {
    let claude = std::env::var_os("DOGCODES_REVIEW_LIVE_CLAUDE_PATH");
    if claude.is_some() && std::env::var_os("DOGCODES_REVIEW_LIVE_CODEX_PATH").is_some() {
        return Err("select exactly one live agent".into());
    }
    let is_claude = claude.is_some();
    let executable = std::path::PathBuf::from(
        claude
            .or_else(|| std::env::var_os("DOGCODES_REVIEW_LIVE_CODEX_PATH"))
            .ok_or("explicit executable required")?,
    );
    let driver = build_driver(executable, is_claude).await?;
    seed_project(core, directory, driver, "真实模型 Diff 验收", false).await
}

async fn build_driver(
    executable: std::path::PathBuf,
    is_claude: bool,
) -> Result<Arc<dyn AgentDriver>, String> {
    if !executable.is_absolute() {
        return Err("absolute agent executable required".into());
    }
    let version = tokio::process::Command::new(&executable)
        .arg("--version")
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !version.status.success() {
        return Err("agent version unavailable".into());
    }
    let version = String::from_utf8(version.stdout).map_err(|error| error.to_string())?;
    let driver: Arc<dyn AgentDriver> = if is_claude {
        let version = version
            .trim()
            .strip_suffix(" (Claude Code)")
            .ok_or("unexpected Claude version")?;
        claude_driver::ClaudeDriver::probe_ready(&executable)
            .await
            .map_err(|error| error.to_string())?;
        Arc::new(
            claude_driver::ClaudeDriver::new(
                "claude-code",
                &InstallationId::new().to_string(),
                executable,
                Some(version.into()),
                // Match the production desktop registration: Core resumes the
                // existing native session for the second composer submission.
                driver_api::SessionResumeCapability::Supported,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        let version = version
            .trim()
            .strip_prefix("codex-cli ")
            .ok_or("unexpected Codex version")?;
        codex_driver::CodexDriver::probe_ready(&executable)
            .await
            .map_err(|error| error.to_string())?;
        Arc::new(
            codex_driver::CodexDriver::new(
                &executable,
                InstallationId::new().to_string(),
                version.into(),
            )
            .map_err(|error| error.to_string())?,
        )
    };
    Ok(driver)
}

async fn seed_project(
    core: &Arc<agent_core::Core>,
    directory: &Path,
    driver: Arc<dyn AgentDriver>,
    title: &str,
    git: bool,
) -> Result<(), String> {
    let executor = driver.executor().clone();
    core.register_driver(driver).await;
    let path = directory.join(if git {
        "live-git-project"
    } else {
        "live-project"
    });
    std::fs::create_dir(&path).map_err(|error| error.to_string())?;
    std::fs::write(path.join("sample.txt"), b"before real model edit\n")
        .map_err(|error| error.to_string())?;
    if git {
        for args in [
            vec!["init"],
            vec!["add", "sample.txt"],
            vec![
                "-c",
                "user.name=Review Acceptance",
                "-c",
                "user.email=review@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "baseline",
            ],
        ] {
            let output = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(args)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err("isolated Git seed failed".into());
            }
        }
    }
    let project = crate::register_local_multi_project(
        &crate::daemon::CoreClient::local(core.clone()),
        "真实模型 Review 验收".into(),
        path.to_string_lossy().into_owned(),
        vec![],
    )
    .await?;
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: title.into(),
            preferred_executor: executor,
        },
    )))
    .await
    .map_err(|error| error.to_string())?;
    let store = ReviewStore::open(directory.join("review"))?;
    if store.load_baseline(&task.to_string(), &path).is_ok() {
        return Err("live baseline was prematurely created".into());
    }
    Ok(())
}

pub fn mixed_test() -> bool {
    std::env::var_os("DOGCODES_REVIEW_LIVE_MIXED").is_some_and(|value| value == "1")
}

pub async fn seed_mixed(core: &Arc<agent_core::Core>, directory: &Path) -> Result<(), String> {
    for (variable, claude, title, git) in [
        ("DOGCODES_REVIEW_LIVE_CODEX_PATH", false, "mixed-git", true),
        (
            "DOGCODES_REVIEW_LIVE_CLAUDE_PATH",
            true,
            "mixed-non-git",
            false,
        ),
    ] {
        let executable =
            std::env::var_os(variable).ok_or("both explicit agent executables required")?;
        let driver = build_driver(executable.into(), claude).await?;
        seed_project(core, directory, driver, title, git).await?;
    }
    Ok(())
}
