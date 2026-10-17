use super::{Arc, Path, ReviewStore, SnapshotBudget};
use agent_domain::{ExecutorRef, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask};

pub async fn seed(
    core: &Arc<agent_core::Core>,
    directory: &Path,
    executor: ExecutorRef,
) -> Result<(), String> {
    let paths: Vec<_> = ["root-a", "root-b", "root-notes"]
        .map(|name| directory.join(name))
        .into();
    for (index, path) in paths.iter().enumerate() {
        std::fs::create_dir(path).map_err(|error| error.to_string())?;
        std::fs::write(path.join("shared.txt"), format!("base root {index}\n"))
            .map_err(|error| error.to_string())?;
        if index < 2 {
            for args in [
                vec!["init", "--initial-branch=main", "--template="],
                vec!["add", "--", "shared.txt"],
                vec!["commit", "-m", "fixture base"],
            ] {
                git(path, &args)?;
            }
        }
    }
    let merge_root = &paths[0];
    git(merge_root, &["checkout", "-b", "side"])?;
    std::fs::write(
        merge_root.join("side-only.txt"),
        b"side branch contribution\n",
    )
    .map_err(|error| error.to_string())?;
    git(merge_root, &["add", "--", "side-only.txt"])?;
    git(merge_root, &["commit", "-m", "side contribution"])?;
    git(merge_root, &["checkout", "main"])?;
    std::fs::write(
        merge_root.join("main-only.txt"),
        b"main branch contribution\n",
    )
    .map_err(|error| error.to_string())?;
    git(merge_root, &["add", "--", "main-only.txt"])?;
    git(merge_root, &["commit", "-m", "main contribution"])?;
    git(
        merge_root,
        &["merge", "--no-ff", "side", "-m", "fixture merge"],
    )?;
    let project = crate::register_local_multi_project(
        &crate::daemon::CoreClient::local(core.clone()),
        "多目录验收".into(),
        paths[0].to_string_lossy().into_owned(),
        paths[1..]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    )
    .await?;
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "多目录 Diff 验收".into(),
            preferred_executor: executor,
        },
    )))
    .await
    .map_err(|error| error.to_string())?;
    let store = ReviewStore::open(directory.join("review"))?;
    store.ensure_baseline(&task.to_string(), &paths[2], SnapshotBudget::default())?;
    for (index, path) in paths.iter().enumerate() {
        std::fs::write(path.join("shared.txt"), format!("changed root {index}\n"))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn git(path: &Path, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .current_dir(path)
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
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    Ok(())
}
