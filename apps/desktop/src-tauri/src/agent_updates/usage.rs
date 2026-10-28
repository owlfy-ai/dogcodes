//! Upgrade consent is bound to an installation and a snapshot of running tasks/processes.
use super::{AgentUpdates, Plan, key};
use agent_protocol::{CancelTurn, CommandEnvelope, CoreCommand, InstallationSummary, TaskSummary};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, RefreshKind, Signal, System, UpdateKind};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub token: String,
    pub task_count: usize,
    pub process_count: usize,
}
pub(super) struct Ticket {
    key: String,
    at: Instant,
    processes: Vec<Identity>,
    runs: BTreeSet<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    pid: Pid,
    start: u64,
    exe: PathBuf,
}

fn system() -> Result<System, String> {
    let s = System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::Always)
                .with_cmd(UpdateKind::Always)
                .with_user(UpdateKind::Always),
        ),
    );
    if s.process(Pid::from_u32(std::process::id()))
        .and_then(|p| p.user_id())
        .is_none()
    {
        return Err("无法检查正在运行的 Agent，暂未执行升级".into());
    }
    Ok(s)
}
fn identity(p: &sysinfo::Process) -> Option<Identity> {
    if p.status() == ProcessStatus::Zombie {
        return None;
    }
    Some(Identity {
        pid: p.pid(),
        start: p.start_time(),
        exe: p.exe()?.to_path_buf(),
    })
}

struct Scope {
    executable: PathBuf,
    directory: Option<PathBuf>,
}
impl Scope {
    fn new(i: &InstallationSummary, plan: &Plan) -> Result<Self, String> {
        let executable = Path::new(&i.executable)
            .canonicalize()
            .map_err(|_| "Agent 安装已变化，请重新检测")?;
        let directory = match plan {
            Plan::Npm { package, .. } | Plan::NpmLocal { package, .. } => executable
                .ancestors()
                .find(|p| {
                    std::fs::read(p.join("package.json"))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                        .is_some_and(|v| v["name"].as_str() == Some(package))
                })
                .map(Path::to_path_buf),
            Plan::Native(_) => {
                if i.provider_id == "kiro" {
                    // The native updater replaces the whole CLI app, including
                    // its desktop helper; surface all its processes for consent.
                    super::kiro::bundle(&executable)
                } else {
                    // Version directories belong to one installation, unlike shared ~/.local/bin.
                    super::new_agents::pi_managed_root(&executable)
                        .map(|root| root.join("releases"))
                        .or_else(|| {
                            executable
                                .ancestors()
                                .find(|p| p.file_name().is_some_and(|n| n == "versions"))
                                .map(Path::to_path_buf)
                                .or_else(|| {
                                    executable
                                        .ancestors()
                                        .find(|p| p.file_name().is_some_and(|n| n == ".kimi-code"))
                                        .map(Path::to_path_buf)
                                })
                        })
                }
            }
            Plan::Brew { .. } => executable.parent().map(Path::to_path_buf),
            _ => None,
        };
        Ok(Self {
            executable,
            directory,
        })
    }
    fn contains(&self, path: &Path) -> bool {
        if !path.is_absolute() {
            return false;
        }
        let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        real == self.executable || self.directory.as_ref().is_some_and(|d| real.starts_with(d))
    }
    fn matches(&self, p: &sysinfo::Process) -> bool {
        self.matches_command(p.exe(), p.cmd())
    }
    fn matches_command(&self, exe: Option<&Path>, args: &[std::ffi::OsString]) -> bool {
        if exe.is_some_and(|e| self.contains(e)) {
            return true;
        }
        let interpreter = exe
            .and_then(Path::file_stem)
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if ![
            "node",
            "nodejs",
            "bun",
            "python",
            "python3",
            "bash",
            "sh",
            "zsh",
            "cmd",
            "pwsh",
            "powershell",
        ]
        .contains(&interpreter)
        {
            return false;
        }
        // Inspect only the interpreter's script operand. Never match a prompt, shell command,
        // or a later argument merely mentioning an Agent installation path.
        for arg in args.iter().skip(1) {
            let arg = arg.to_string_lossy();
            if ["-c", "-e", "--eval", "-Command", "-EncodedCommand"]
                .iter()
                .any(|a| arg.eq_ignore_ascii_case(a))
            {
                return false;
            }
            if arg.starts_with('-') {
                continue;
            }
            return self.contains(Path::new(arg.as_ref()));
        }
        false
    }
}

fn collect(s: &System, scope: &Scope) -> Vec<Identity> {
    let own = Pid::from_u32(std::process::id());
    let user = s.process(own).and_then(|p| p.user_id());
    let mut ancestors = BTreeSet::new();
    let mut cursor = Some(own);
    while let Some(pid) = cursor {
        if !ancestors.insert(pid) {
            break;
        }
        cursor = s.process(pid).and_then(sysinfo::Process::parent);
    }
    let eligible = |p: &&sysinfo::Process| {
        p.user_id() == user && !ancestors.contains(&p.pid()) && identity(p).is_some()
    };
    let mut pids: BTreeSet<_> = s
        .processes()
        .values()
        .filter(eligible)
        .filter(|p| scope.matches(p))
        .map(sysinfo::Process::pid)
        .collect();
    loop {
        let children: Vec<_> = s
            .processes()
            .values()
            .filter(eligible)
            .filter(|p| {
                !pids.contains(&p.pid()) && p.parent().is_some_and(|pid| pids.contains(&pid))
            })
            .map(sysinfo::Process::pid)
            .collect();
        if children.is_empty() {
            break;
        }
        pids.extend(children);
    }
    pids.into_iter()
        .filter_map(|pid| s.process(pid).and_then(identity))
        .collect()
}
async fn scan(i: &InstallationSummary, p: &Plan) -> Result<Vec<Identity>, String> {
    let scope = Scope::new(i, p)?;
    tokio::task::spawn_blocking(move || {
        let system = system()?;
        let mut cursor = Some(Pid::from_u32(std::process::id()));
        let mut visited = BTreeSet::new();
        while let Some(pid) = cursor {
            if !visited.insert(pid) {
                break;
            }
            let Some(process) = system.process(pid) else {
                break;
            };
            if scope.matches(process) {
                return Err(
                    "当前应用由此 Agent 启动，结束它可能关闭应用；请独立启动本软件后升级".into(),
                );
            }
            cursor = process.parent();
        }
        Ok(collect(&system, &scope))
    })
    .await
    .map_err(|_| "Agent 进程检测中断")?
}
fn target_task(t: &TaskSummary, i: &InstallationSummary) -> bool {
    t.active_primary_run.as_ref().is_some_and(|run| {
        run.executor.provider_id.to_string() == i.provider_id
            && run
                .executor
                .installation_id
                .as_ref()
                .is_none_or(|id| *id == i.installation_id.as_str())
    })
}
fn run_ids(tasks: &[TaskSummary]) -> BTreeSet<String> {
    tasks
        .iter()
        .filter_map(|t| t.active_primary_run.as_ref().map(|r| r.run_id.to_string()))
        .collect()
}
fn authorized(
    ticket: &Ticket,
    installation: &InstallationSummary,
    processes: &[Identity],
    runs: &BTreeSet<String>,
) -> bool {
    ticket.key == key(installation)
        && ticket.at.elapsed() < Duration::from_mins(5)
        && processes.iter().all(|p| ticket.processes.contains(p))
        && runs.is_subset(&ticket.runs)
}

async fn stop_processes(processes: Vec<Identity>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        for force in [false, true] {
            let s = system()?;
            for expected in &processes {
                if let Some(p) = s
                    .process(expected.pid)
                    .filter(|p| identity(p).as_ref() == Some(expected))
                {
                    if force {
                        p.kill();
                    } else {
                        p.kill_with(Signal::Term).unwrap_or_else(|| p.kill());
                    }
                }
            }
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(100));
                let s = system()?;
                if processes
                    .iter()
                    .all(|e| s.process(e.pid).and_then(identity).as_ref() != Some(e))
                {
                    return Ok(());
                }
            }
        }
        Err("无法结束部分 Agent 进程，未执行升级。请手动退出后重试".into())
    })
    .await
    .map_err(|_| "结束 Agent 进程失败")?
}

pub(super) async fn prepare(
    core: &crate::daemon::CoreClient,
    updates: &AgentUpdates,
    i: &InstallationSummary,
    p: &Plan,
    token: Option<&str>,
) -> Result<Option<Confirmation>, String> {
    let snapshot = core.bootstrap().await?;
    if snapshot
        .tasks
        .iter()
        .any(|t| t.active_primary_run.is_some() && !target_task(t, i))
    {
        return Err(
            "其他 Agent 仍在执行任务，请等待其结束后再升级；不会结束其他 Agent 的任务".into(),
        );
    }
    let tasks: Vec<_> = snapshot
        .tasks
        .into_iter()
        .filter(|t| target_task(t, i))
        .collect();
    let processes = scan(i, p).await?;
    let runs = run_ids(&tasks);
    let ticket = match token {
        Some(token) => updates.confirmations.lock().await.remove(token),
        None => None,
    };
    if processes.is_empty() && runs.is_empty() {
        return Ok(None);
    }
    if !ticket
        .as_ref()
        .is_some_and(|t| authorized(t, i, &processes, &runs))
    {
        let token = uuid::Uuid::new_v4().to_string();
        let confirmation = Confirmation {
            token: token.clone(),
            task_count: tasks.len(),
            process_count: processes.len(),
        };
        let mut tickets = updates.confirmations.lock().await;
        tickets.retain(|_, t| t.at.elapsed() < Duration::from_mins(5) && t.key != key(i));
        tickets.insert(
            token,
            Ticket {
                key: key(i),
                at: Instant::now(),
                processes,
                runs,
            },
        );
        return Ok(Some(confirmation));
    }
    // Request normal cancellation first so the conversation records an interruption.
    for task in &tasks {
        if let Some(run) = &task.active_primary_run {
            let ack = core
                .execute(CommandEnvelope::local(CoreCommand::CancelTurn(
                    CancelTurn {
                        task_id: task.task_id.clone(),
                        turn_id: run.turn_id.clone(),
                        run_id: run.run_id.clone(),
                    },
                )))
                .await?;
            if !ack.accepted {
                return Err("任务未接受取消请求，暂未执行升级".into());
            }
        }
    }
    if !tasks.is_empty() {
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
    // Terminate only the identities explicitly shown in this confirmation. A restarted
    // process or a newly started task requires fresh consent, even if its PID was reused.
    stop_processes(processes).await?;
    for _ in 0..60 {
        if !core
            .bootstrap()
            .await?
            .tasks
            .iter()
            .any(|t| target_task(t, i))
        {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("任务仍在结束处理中，暂未升级，请稍后重试".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn spawn_fixture(command: &mut std::process::Command) -> std::process::Child {
        // A concurrent fork can briefly retain a copied executable's writable
        // descriptor. Only retry ETXTBSY, for this owned fixture and at most 1s;
        // other launch errors and a persistent busy file still fail the test.
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            match command.spawn() {
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && Instant::now() < until =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result.unwrap(),
            }
        }
    }
    #[test]
    fn kiro_scope_includes_cli_app_helpers_but_not_the_separate_ide() {
        let dir = tempfile::tempdir().unwrap();
        let executable = super::super::kiro::tests::fixture_app(dir.path());
        std::fs::write(executable.with_file_name("kiro_cli_desktop"), "helper").unwrap();
        let i = super::super::tests::installation_for("kiro", &executable, "2.24.1");
        let scope = Scope::new(&i, &Plan::Native("Kiro")).unwrap();
        assert!(scope.contains(&executable));
        assert!(
            scope.contains(
                &dir.path()
                    .join("Kiro CLI.app/Contents/MacOS/kiro_cli_desktop")
            )
        );
        assert!(!scope.contains(&dir.path().join("Kiro.app/Contents/MacOS/Kiro")));
    }
    #[test]
    fn managed_pi_matches_running_release_scripts_without_matching_other_agents() {
        let temp = tempfile::tempdir().unwrap();
        let agent = temp.path().join("pi/agent");
        let root = agent.join("install");
        let script =
            root.join("releases/0.87.1/node_modules/@earendil-works/pi-coding-agent/dist/cli.js");
        for (path, content) in [
            (agent.join("bin/pi"), "launcher"),
            (
                root.join("managed-install.json"),
                r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
            ),
            (root.join("current-version"), "0.87.1"),
            (
                script
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("package.json"),
                r#"{"name":"@earendil-works/pi-coding-agent"}"#,
            ),
            (script.clone(), ""),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let i = super::super::tests::installation_for("pi", &agent.join("bin/pi"), "0.87.1");
        let scope = Scope::new(&i, &Plan::Native("Pi")).unwrap();
        assert!(scope.matches_command(
            Some(Path::new("/usr/bin/node")),
            &["node".into(), script.into_os_string()]
        ));
        assert!(!scope.contains(&temp.path().join("other-agent/cli.js")));
        assert!(!scope.contains(&agent.join("settings.json")));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn upgrade_preflight_requires_fresh_consent_and_does_not_kill_on_cancel() {
        use std::sync::Arc;
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-worker");
        std::fs::copy("/bin/sleep", &binary).unwrap();
        let mut target = spawn_fixture(std::process::Command::new(&binary).arg("60"));
        let core = crate::daemon::CoreClient::local(Arc::new(agent_core::Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        ))));
        let updates = AgentUpdates::default();
        let i = super::super::tests::installation_for("cursor", &binary, "1.0.0");
        let plan = Plan::Native("cursor");
        let first = prepare(&core, &updates, &i, &plan, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.process_count, 1);
        assert!(target.try_wait().unwrap().is_none());
        // Closing the dialog issues no mutation; a later attempt gets a new consent token.
        let retry = prepare(&core, &updates, &i, &plan, None)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first.token, retry.token);
        assert!(target.try_wait().unwrap().is_none());
        let mut second = spawn_fixture(std::process::Command::new(&binary).arg("60"));
        let renewed = prepare(&core, &updates, &i, &plan, Some(&retry.token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renewed.process_count, 2);
        assert_ne!(renewed.token, retry.token);
        assert!(target.try_wait().unwrap().is_none());
        assert!(second.try_wait().unwrap().is_none());
        assert!(
            prepare(&core, &updates, &i, &plan, Some(&renewed.token))
                .await
                .unwrap()
                .is_none()
        );
        target.wait().unwrap();
        second.wait().unwrap();
    }
    #[test]
    fn script_matching_never_uses_prompt_text_or_other_installations() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("node/lib/node_modules/agent");
        let script = package.join("bin/agent.js");
        let script_text = script.to_str().unwrap();
        let scope = Scope {
            executable: script.clone(),
            directory: Some(package),
        };
        let node = root.path().join("node/bin/node");
        let args = |args: &[&str]| {
            args.iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert!(scope.matches_command(
            Some(&node),
            &args(&["node", "--use-system-ca", script_text, "--acp"])
        ));
        assert!(!scope.matches_command(
            Some(&node),
            &args(&[
                "node",
                root.path().join("other/app.js").to_str().unwrap(),
                script_text
            ])
        ));
        assert!(!scope.matches_command(Some(&node), &args(&["node", "-e", script_text])));
        assert!(!scope.matches_command(
            Some(Path::new("/bin/sh")),
            &args(&["sh", "-c", script_text])
        ));
        assert!(
            !scope.contains(
                &root
                    .path()
                    .join("node/lib/node_modules/agent-other/bin/agent.js")
            )
        );
    }

    #[test]
    fn consent_rejects_reused_pids_new_runs_expiry_and_changed_installations() {
        let mut i = super::super::tests::installation_for("cursor", Path::new("/cursor"), "1.0.0");
        let original = Identity {
            pid: Pid::from_u32(999),
            start: 12,
            exe: "/cursor".into(),
        };
        let mut ticket = Ticket {
            key: key(&i),
            at: Instant::now(),
            processes: vec![original.clone()],
            runs: BTreeSet::from(["run1".into()]),
        };
        assert!(authorized(
            &ticket,
            &i,
            std::slice::from_ref(&original),
            &ticket.runs
        ));
        assert!(authorized(&ticket, &i, &[], &BTreeSet::new()));
        assert!(!authorized(
            &ticket,
            &i,
            &[Identity {
                start: 13,
                ..original
            }],
            &ticket.runs
        ));
        assert!(!authorized(
            &ticket,
            &i,
            &[],
            &BTreeSet::from(["run2".into()])
        ));
        i.fingerprint = "changed".into();
        assert!(!authorized(&ticket, &i, &[], &BTreeSet::new()));
        ticket.key = key(&i);
        ticket.at = Instant::now()
            .checked_sub(Duration::from_secs(301))
            .unwrap();
        assert!(!authorized(&ticket, &i, &[], &BTreeSet::new()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn termination_includes_the_agents_child_processes() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-shell");
        // macOS /bin/sh is a launcher that execs a different binary; copy the shell itself.
        std::fs::copy("/bin/bash", &binary).unwrap();
        let mut parent =
            spawn_fixture(std::process::Command::new(&binary).args(["-c", "/bin/sleep 60 & wait"]));
        let scope = Scope {
            executable: binary.canonicalize().unwrap(),
            directory: None,
        };
        let mut found = Vec::new();
        for _ in 0..30 {
            found = collect(&system().unwrap(), &scope);
            if found.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(found.len() >= 2, "expected parent and its sleep child");
        stop_processes(found.clone()).await.unwrap();
        parent.wait().unwrap();
        let system = system().unwrap();
        assert!(
            found
                .iter()
                .all(|p| system.process(p.pid).and_then(identity).as_ref() != Some(p))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_process_detection_and_termination_leave_unrelated_process_alive() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-worker");
        std::fs::copy("/bin/sleep", &binary).unwrap();
        let mut target = spawn_fixture(std::process::Command::new(&binary).arg("60"));
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let scope = Scope {
            executable: binary.canonicalize().unwrap(),
            directory: None,
        };
        let found = collect(&system().unwrap(), &scope);
        assert!(found.iter().any(|p| p.pid.as_u32() == target.id()));
        assert!(!found.iter().any(|p| p.pid.as_u32() == unrelated.id()));
        stop_processes(found).await.unwrap();
        assert!(target.try_wait().unwrap().is_some());
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }
}
