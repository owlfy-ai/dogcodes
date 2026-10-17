//! Private, fail-closed `PreToolUse` bridge for native headless Antigravity.
use super::{
    Arc, DriverError, DriverEvent, ENV_ALLOWLIST, Ordering, PendingQuestion, SessionProcess,
    SessionRequest, Stdio, Value, json,
};
use driver_api::{ApprovalOption, ApprovalSemanticRole};
use std::io::{Read, Write};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
const LIMIT: u64 = 131_072;
// Cold headless startup is close to 20 s on real installations. Keep Hook
// verification bounded without rejecting an otherwise healthy cold process.
const HOOK_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
const TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);
const FLAG: &str = "--dogcodes-approval-hook";

/// Called before desktop initialization when this executable is used as a Hook.
#[must_use]
pub fn run_hook_if_requested() -> bool {
    if std::env::args().nth(1).as_deref() != Some(FLAG) {
        return false;
    }
    let result=(|| -> Option<Value> {
        let address: std::net::SocketAddr=std::env::var("DOGCODES_APPROVAL_ADDR").ok()?.parse().ok()?;
        if !address.ip().is_loopback() {return None;}
        let token=std::env::var("DOGCODES_APPROVAL_TOKEN").ok()?;
        let mut input=Vec::new(); std::io::stdin().take(LIMIT+1).read_to_end(&mut input).ok()?;
        if input.len() as u64>LIMIT {return None;}
        let payload:Value=serde_json::from_slice(&input).ok()?;
        let mut stream=std::net::TcpStream::connect_timeout(&address,std::time::Duration::from_secs(5)).ok()?;
        stream.set_read_timeout(Some(TIMEOUT+std::time::Duration::from_secs(5))).ok()?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
        writeln!(stream,"{}",json!({"token":token,"payload":payload})).ok()?;
        let mut output=String::new(); stream.take(LIMIT+1).read_to_string(&mut output).ok()?;
        let value:Value=serde_json::from_str(&output).ok()?;
        if !matches!(value["decision"].as_str(),Some("allow"|"deny")) {return None;}
        Some(value)
    })().unwrap_or_else(||json!({"decision":"deny","reason":"Desktop approval bridge unavailable; no permission was granted."}));
    println!("{result}");
    true
}

// Cache only a verified private Hook directory. Every session still receives a
// fresh loopback listener/token; decisions are never cached.
struct VerifiedHook {
    directory: Arc<tempfile::TempDir>,
    key: u64,
    at: std::time::Instant,
    sources: Vec<(std::path::PathBuf, Vec<u8>)>,
}
static VERIFIED: std::sync::Mutex<Option<VerifiedHook>> = std::sync::Mutex::new(None);
const VERIFY_TTL: std::time::Duration = std::time::Duration::from_mins(5);

fn verification_key(
    executable: &std::path::Path,
    workspace: &SessionRequest,
    directory: &std::path::Path,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    fn tree(
        path: &std::path::Path,
        hash: &mut impl Hasher,
        budget: &mut usize,
        depth: usize,
    ) -> Option<()> {
        path.hash(hash);
        if depth > 12 {
            return None;
        }
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                0u8.hash(hash);
                return Some(());
            }
            Err(_) => return None,
        };
        if metadata.file_type().is_symlink() {
            return None;
        }
        metadata.modified().ok()?.hash(hash);
        if metadata.is_dir() {
            let mut paths = std::fs::read_dir(path)
                .ok()?
                .map(|entry| entry.ok().map(|e| e.path()))
                .collect::<Option<Vec<_>>>()?;
            if paths.len() > 1024 {
                return None;
            }
            paths.sort();
            for path in paths {
                tree(&path, hash, budget, depth + 1)?;
            }
        } else {
            let size = usize::try_from(metadata.len()).ok()?;
            *budget = budget.checked_sub(size)?;
            std::fs::read(path).ok()?.hash(hash);
        }
        Some(())
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut budget = 2 * 1024 * 1024usize;
    for exe in [executable.to_path_buf(), std::env::current_exe().ok()?] {
        let path = std::fs::canonicalize(exe).ok()?;
        path.hash(&mut hash);
        let m = std::fs::metadata(path).ok()?;
        m.len().hash(&mut hash);
        m.modified().ok()?.hash(&mut hash);
    }
    serde_json::to_string(&workspace.history_mcp)
        .ok()?
        .hash(&mut hash);
    workspace.working_directory.hash(&mut hash);
    workspace.additional_directories.hash(&mut hash);
    for dir in
        std::iter::once(&workspace.working_directory).chain(&workspace.additional_directories)
    {
        for ancestor in std::path::Path::new(dir).ancestors() {
            tree(
                &ancestor.join(".agents/hooks.json"),
                &mut hash,
                &mut budget,
                0,
            )?;
            tree(
                &ancestor.join(".agents/settings.json"),
                &mut hash,
                &mut budget,
                0,
            )?;
        }
    }
    for name in ENV_ALLOWLIST {
        std::env::var_os(name).hash(&mut hash);
    }
    if let Some(home) = std::env::var_os("HOME") {
        for config in [
            ".agents/hooks.json",
            ".agents/settings.json",
            ".gemini/settings.json",
            ".gemini/antigravity/hooks.json",
            ".gemini/antigravity-cli/settings.json",
            ".gemini/antigravity-cli/hooks.json",
            ".antigravity/settings.json",
            ".antigravity/hooks.json",
            ".config/antigravity/settings.json",
            ".config/antigravity/hooks.json",
        ] {
            tree(
                &std::path::PathBuf::from(&home).join(config),
                &mut hash,
                &mut budget,
                0,
            )?;
        }
    }
    tree(directory, &mut hash, &mut budget, 0)?;
    Some(hash.finish())
}

pub(super) struct Bridge {
    directory: Arc<tempfile::TempDir>,
    address: String,
    token: String,
    command: String,
    listener: tokio::net::TcpListener,
}
impl Bridge {
    pub async fn prepare(
        executable: &std::path::Path,
        workspace: &SessionRequest,
    ) -> Result<Self, DriverError> {
        let fail =
            || DriverError::CapabilityUnavailable("桌面审批 Hook 验证失败，未启动工具".into());
        let started = std::time::Instant::now();
        let cached = VERIFIED
            .lock()
            .expect("hook cache")
            .as_ref()
            .and_then(|entry| {
                (entry.at.elapsed() < VERIFY_TTL
                    && entry.sources.iter().all(|(path, bytes)| {
                        std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64)
                            && std::fs::read(path).is_ok_and(|current| current == *bytes)
                    })
                    && verification_key(executable, workspace, entry.directory.path())
                        == Some(entry.key))
                .then(|| entry.directory.clone())
            });
        let cache_hit = cached.is_some();
        let directory = if let Some(directory) = cached {
            directory
        } else {
            Arc::new(
                tempfile::Builder::new()
                    .prefix("dogcodes-agy-approval-")
                    .tempdir()
                    .map_err(|_| fail())?,
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| fail())?;
        let address = listener.local_addr().map_err(|_| fail())?.to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let exe = std::env::current_exe().map_err(|_| fail())?;
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let command = if cfg!(windows) {
            format!("\"%DOGCODES_APPROVAL_EXE%\" {FLAG}")
        } else {
            format!("{} {FLAG}", quote(&exe.to_string_lossy()))
        };
        if !cache_hit {
            std::fs::create_dir(directory.path().join(".agents")).map_err(|_| fail())?;
            std::fs::write(directory.path().join(".agents/hooks.json"),json!({"dogcodes-approvals":{"PreToolUse":[{"matcher":".*","hooks":[{"type":"command","command":command,"timeout":615}]}]}}).to_string()).map_err(|_|fail())?;
        }
        if !cache_hit && let Some(mcp) = &workspace.history_mcp {
            // AGY discovers configuration from the final private workspace root.
            // Keep history and approval hooks together; never rewrite a live cached root.
            std::fs::write(
                directory.path().join(".agents/mcp_config.json"),
                json!({"mcpServers":{mcp.name.clone():mcp.server_config()}}).to_string(),
            )
            .map_err(|_| fail())?;
        }
        let bridge = Self {
            directory,
            address,
            token,
            command,
            listener,
        };
        if cache_hit {
            eprintln!(
                "antigravity timing stage=hook_verification cache=hit elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return Ok(bridge);
        }
        *VERIFIED.lock().expect("hook cache") = None;
        let before_key = verification_key(executable, workspace, bridge.directory.path());
        let mut cmd = tokio::process::Command::new(executable);
        cmd.current_dir(&workspace.working_directory)
            .args(["--add-dir", &workspace.working_directory]);
        for dir in &workspace.additional_directories {
            cmd.arg("--add-dir").arg(dir);
        }
        cmd.arg("--add-dir")
            .arg(bridge.directory.path())
            .args(["--print=/hooks", "--output-format", "stream-json"])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        // Match the native session launch, including GUI-only system proxies.
        agent_runtime::apply_proxy_environment(&mut cmd);
        bridge.environment(&mut cmd);
        let output = tokio::time::timeout(HOOK_PROBE_TIMEOUT, async {
            use tokio::io::AsyncReadExt;
            let mut child = cmd.spawn().map_err(|_| fail())?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .ok_or_else(fail)?
                .take(LIMIT + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| fail())?;
            if bytes.len() as u64 > LIMIT {
                return Err(fail());
            }
            if !child.wait().await.map_err(|_| fail())?.success() {
                return Err(fail());
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| {
            DriverError::CapabilityUnavailable("桌面审批 Hook 验证超时，未启动工具".into())
        })??;
        if !bridge.verified(&output) {
            return Err(fail());
        }
        let sources = (|| -> Option<Vec<(std::path::PathBuf, Vec<u8>)>> {
            let mut sources = Vec::new();
            let mut budget = 2 * 1024 * 1024usize;
            for value in String::from_utf8_lossy(&output)
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            {
                for hook in value["command"]["data"]["hooks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(source) = hook["source"].as_str() {
                        let path = std::path::PathBuf::from(source);
                        let size = usize::try_from(std::fs::metadata(&path).ok()?.len()).ok()?;
                        budget = budget.checked_sub(size)?;
                        sources.push((path.clone(), std::fs::read(path).ok()?));
                    }
                }
            }
            Some(sources)
        })();
        if let (Some(key), Some(sources)) = (before_key, sources)
            && verification_key(executable, workspace, bridge.directory.path()) == Some(key)
        {
            *VERIFIED.lock().expect("hook cache") = Some(VerifiedHook {
                directory: bridge.directory.clone(),
                key,
                at: std::time::Instant::now(),
                sources,
            });
        }
        eprintln!(
            "antigravity timing stage=hook_verification cache=miss elapsed_ms={}",
            started.elapsed().as_millis()
        );
        Ok(bridge)
    }
    fn verified(&self, output: &[u8]) -> bool {
        String::from_utf8_lossy(output)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|v| {
                v["event"] == "command_result"
                    && v["command"]["name"] == "hooks"
                    && v["command"]["data"]["hooks"]
                        .as_array()
                        .is_some_and(|rows| {
                            rows.iter().any(|h| {
                                h["name"] == "dogcodes-approvals"
                                    && h["enabled"] == true
                                    && h["source"].as_str().is_some_and(|s| {
                                        std::path::Path::new(s)
                                            == self.directory.path().join(".agents/hooks.json")
                                    })
                                    && h["actions"].as_array().is_some_and(|a| {
                                        a.iter().any(|a| {
                                            a["event"] == "PreToolUse"
                                                && a["matcher"] == ".*"
                                                && a["command"] == self.command
                                        })
                                    })
                            })
                        })
            })
    }
    pub fn environment(&self, command: &mut tokio::process::Command) {
        command
            .env("DOGCODES_APPROVAL_ADDR", &self.address)
            .env("DOGCODES_APPROVAL_TOKEN", &self.token);
        if let Ok(exe) = std::env::current_exe() {
            command.env("DOGCODES_APPROVAL_EXE", format!("\"{}\"", exe.display()));
        }
    }
    pub fn configure(&self, command: &mut tokio::process::Command) {
        self.environment(command);
        command.arg("--add-dir").arg(self.directory.path());
    }
    pub fn start(self, process: &Arc<SessionProcess>) {
        let process = Arc::downgrade(process);
        let directory = self.directory;
        tokio::spawn(async move {
            // Keep Hook files alive for the entire native process lifetime.
            // Rust captures fields independently: listener/token alone do not
            // retain the TempDir, which removes the Hook files when dropped.
            let _directory = directory;
            let permits = Arc::new(tokio::sync::Semaphore::new(32));
            loop {
                let Some(p) = process.upgrade() else {
                    break;
                };
                if p.finished.load(Ordering::Acquire) {
                    break;
                }
                drop(p);
                let accepted =
                    tokio::time::timeout(std::time::Duration::from_secs(1), self.listener.accept())
                        .await;
                if let Ok(Ok((socket, _))) = accepted
                    && let Ok(permit) = Arc::clone(&permits).try_acquire_owned()
                {
                    let p = process.clone();
                    let token = self.token.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = serve(socket, p, &token).await;
                    });
                }
            }
        });
    }
}
async fn serve(
    socket: tokio::net::TcpStream,
    process: std::sync::Weak<SessionProcess>,
    token: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt;
    let (read, mut write) = socket.into_split();
    let mut line = String::new();
    let read_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::io::BufReader::new(read.take(LIMIT + 1)).read_line(&mut line),
    )
    .await;
    let mut allow = false;
    if matches!(read_result, Ok(Ok(_)))
        && line.len() as u64 <= LIMIT
        && let (Ok(v), Some(p)) = (serde_json::from_str::<Value>(&line), process.upgrade())
    {
        let payload = &v["payload"];
        let own = payload["conversationId"].as_str().is_some_and(|id| {
            p.native_id.lock().expect("native id").as_deref() == Some(id)
                || p.approval_children.lock().expect("children").contains(id)
        });
        if v["token"] == token
            && own
            && p.turn_active.load(Ordering::Acquire)
            && !p.interrupt_requested.load(Ordering::Acquire)
            && let (Some(step), Some(name)) = (
                payload["stepIdx"].as_u64(),
                payload["toolCall"]["name"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256),
            )
        {
            if name == "ask_question" {
                let id = format!(
                    "hook-question-{}-{step}",
                    payload["conversationId"].as_str().unwrap()
                );
                let params = &payload["toolCall"]["args"];
                let mut reason =
                    "The question could not be answered. Do not infer a user answer.".to_owned();
                if let Ok(form) = driver_api::questions::native_form("antigravity", params) {
                    let (sender, receiver) = oneshot::channel();
                    let fresh = {
                        let mut pending = p.pending_questions.lock().expect("questions");
                        if pending.contains_key(&id) {
                            false
                        } else {
                            pending.insert(
                                id.clone(),
                                PendingQuestion {
                                    form: form.clone(),
                                    sender,
                                },
                            );
                            true
                        }
                    };
                    if fresh {
                        if let Some(sink) = p.updates.lock().expect("updates").as_ref() {
                            let _ = sink.send(DriverEvent::QuestionRequested {
                                native_request_id: id.clone(),
                                form,
                            });
                        }
                        if let Ok(Ok(answer)) = tokio::time::timeout(TIMEOUT, receiver).await
                            && p.turn_active.load(Ordering::Acquire)
                            && !p.interrupt_requested.load(Ordering::Acquire)
                        {
                            reason = if answer.cancelled {
                                "The user cancelled this question. No answer was provided. Do not retry automatically.".into()
                            } else {
                                format!(
                                    "dogcodes collected the user's answers: {}. Native ask_question is blocked only to prevent automatic skipping. Continue using these answers; do not ask again.",
                                    answer.values
                                )
                            };
                        }
                        p.pending_questions.lock().expect("questions").remove(&id);
                        if let Some(sink) = p.updates.lock().expect("updates").as_ref() {
                            let _ = sink.send(DriverEvent::QuestionClosed {
                                native_request_id: id.clone(),
                            });
                        }
                    }
                }
                write
                    .write_all(
                        json!({"decision":"deny","reason":reason})
                            .to_string()
                            .as_bytes(),
                    )
                    .await?;
                return write.shutdown().await;
            }
            if p.permission_mode.as_deref() == Some("dangerously-skip-permissions") {
                allow = true;
            } else if name != "ask_permission" {
                let id = format!(
                    "hook-{}-{step}",
                    payload["conversationId"].as_str().unwrap()
                );
                let (tx, rx) = oneshot::channel();
                let fresh = {
                    let mut pending = p.pending_approvals.lock().expect("approvals");
                    if pending.contains_key(&id) {
                        false
                    } else {
                        pending.insert(id.clone(), tx);
                        true
                    }
                };
                if fresh {
                    let event = DriverEvent::ApprovalRequested {
                        native_request_id: id.clone(),
                        tool_name: name.into(),
                        command_summary: payload["toolCall"]["args"].to_string(),
                        options: vec![
                            ApprovalOption {
                                option_id: "allow_once".into(),
                                label: "允许一次".into(),
                                semantic_role: ApprovalSemanticRole::ApproveOnce,
                            },
                            ApprovalOption {
                                option_id: "deny".into(),
                                label: "拒绝".into(),
                                semantic_role: ApprovalSemanticRole::Reject,
                            },
                        ],
                    };
                    if let Some(sink) = p.updates.lock().expect("updates").as_ref() {
                        let _ = sink.send(event);
                    }
                    allow = matches!(tokio::time::timeout(TIMEOUT, rx).await, Ok(Ok(true)))
                        && p.turn_active.load(Ordering::Acquire)
                        && !p.interrupt_requested.load(Ordering::Acquire);
                    p.pending_approvals.lock().expect("approvals").remove(&id);
                }
            }
        }
    }
    write.write_all(json!({"decision":if allow{"allow"}else{"deny"},"reason":if allow{"Approved once in dogcodes"}else{"No desktop approval was granted"}}).to_string().as_bytes()).await?;
    write.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AntigravityDriver, SpawnSlot};
    use agent_domain::InstallationId;
    use driver_api::{AgentDriver, SessionHandle, SessionResumeCapability};
    use std::{collections::HashMap, sync::atomic::AtomicBool};
    use tokio::sync::mpsc;
    async fn process() -> (Arc<SessionProcess>, mpsc::UnboundedReceiver<DriverEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Arc::new(SessionProcess {
                usage: std::sync::Mutex::new(None),
                usage_log: None,
                workspace_instruction: String::new(),
                permission_mode: Some("desktop-approvals".into()),
                approval_children: std::sync::Mutex::new(std::collections::HashSet::default()),
                pending_approvals: std::sync::Mutex::new(HashMap::new()),
                pending_questions: std::sync::Mutex::new(HashMap::new()),
                native_id: std::sync::Mutex::new(Some("parent".into())),
                _spawn_slot: SpawnSlot {
                    _guard: Arc::new(tokio::sync::Mutex::new(())).lock_owned().await,
                },
                stdin: tokio::sync::Mutex::new(None),
                updates: std::sync::Mutex::new(Some(tx)),
                turn_active: AtomicBool::new(true),
                interrupt_requested: AtomicBool::new(false),
                finished: Arc::new(AtomicBool::new(false)),
            }),
            rx,
        )
    }
    async fn request(
        p: &Arc<SessionProcess>,
        token: &str,
        session: &str,
    ) -> tokio::task::JoinHandle<Value> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let weak = Arc::downgrade(p);
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            serve(socket, weak, "secret").await.unwrap();
        });
        let payload = json!({"token":token,"payload":{"conversationId":session,"stepIdx":1,"toolCall":{"name":"run_command","args":{"CommandLine":"echo fixture"}}}});
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream
                .write_all(format!("{payload}\n").as_bytes())
                .await
                .unwrap();
            let mut result = String::new();
            stream.read_to_string(&mut result).await.unwrap();
            serde_json::from_str(&result).unwrap()
        })
    }
    #[test]
    fn verification_cache_key_changes_with_workspace_and_hook_configuration() {
        let workspace = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        std::fs::write(private.path().join("hook"), "original").unwrap();
        let mut request = SessionRequest {
            history_mcp: None,
            working_directory: workspace.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            permission_mode: Some("desktop-approvals".into()),
        };
        let exe = std::env::current_exe().unwrap();
        let first =
            verification_key(&exe, &request, private.path()).expect("bounded configuration");
        assert_eq!(
            Some(first),
            verification_key(&exe, &request, private.path())
        );
        std::fs::create_dir(workspace.path().join(".agents")).unwrap();
        std::fs::write(workspace.path().join(".agents/hooks.json"), "{}").unwrap();
        assert_ne!(
            Some(first),
            verification_key(&exe, &request, private.path())
        );
        let before = verification_key(&exe, &request, private.path());
        request
            .additional_directories
            .push(private.path().to_string_lossy().into_owned());
        assert_ne!(before, verification_key(&exe, &request, private.path()));
        let before = verification_key(&exe, &request, private.path());
        std::fs::write(private.path().join("hook"), "changed").unwrap();
        assert_ne!(before, verification_key(&exe, &request, private.path()));
    }

    #[tokio::test]
    async fn hook_directory_lives_until_native_process_finishes() {
        let (p, _events) = process().await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_owned();
        std::fs::write(path.join("hook"), "fixture").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bridge = Bridge {
            directory: Arc::new(directory),
            address: listener.local_addr().unwrap().to_string(),
            token: "fixture".into(),
            command: "fixture".into(),
            listener,
        };
        bridge.start(&p);
        tokio::task::yield_now().await;
        assert!(
            path.join("hook").exists(),
            "starting the server must retain Hook files"
        );
        p.finished.store(true, Ordering::Release);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Hook files must be cleaned up after process exit");
    }

    #[tokio::test]
    async fn tool_waits_for_explicit_approval_and_accepts_native_session_alias() {
        for decision in ["allow_once", "deny"] {
            let (p, mut events) = process().await;
            let reply = request(&p, "secret", "parent").await;
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .unwrap();
            let DriverEvent::ApprovalRequested {
                native_request_id,
                options,
                ..
            } = event
            else {
                panic!("approval missing")
            };
            assert_eq!(options.len(), 2);
            assert!(!reply.is_finished());
            let driver = AntigravityDriver::new(
                "antigravity",
                &InstallationId::new().to_string(),
                "unused".into(),
                None,
                SessionResumeCapability::Unknown,
            )
            .unwrap();
            driver
                .processes
                .lock()
                .await
                .insert("local-handle".into(), p.clone());
            driver
                .respond_to_interaction(
                    &SessionHandle {
                        native_session_id: "parent".into(),
                    },
                    &native_request_id,
                    decision,
                )
                .await
                .unwrap();
            assert_eq!(
                reply.await.unwrap()["decision"],
                if decision == "allow_once" {
                    "allow"
                } else {
                    "deny"
                }
            );
            assert!(
                driver
                    .respond_to_interaction(
                        &SessionHandle {
                            native_session_id: "parent".into()
                        },
                        &native_request_id,
                        decision
                    )
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn child_requires_registration_and_cannot_be_approved_after_parent_ends() {
        let (p, mut events) = process().await;
        assert_eq!(
            request(&p, "secret", "child").await.await.unwrap()["decision"],
            "deny"
        );
        p.approval_children.lock().unwrap().insert("child".into());
        let reply = request(&p, "secret", "child").await;
        let event = events.recv().await.unwrap();
        let DriverEvent::ApprovalRequested {
            native_request_id, ..
        } = event
        else {
            panic!("missing request")
        };
        p.turn_active.store(false, Ordering::Release);
        p.pending_approvals
            .lock()
            .unwrap()
            .remove(&native_request_id)
            .unwrap()
            .send(true)
            .unwrap();
        assert_eq!(reply.await.unwrap()["decision"], "deny");
    }
    #[tokio::test]
    async fn rejects_foreign_session_bad_token_and_cancelled_approval() {
        let (p, mut events) = process().await;
        for (token, session) in [("wrong", "parent"), ("secret", "foreign")] {
            assert_eq!(
                request(&p, token, session).await.await.unwrap()["decision"],
                "deny"
            );
            assert!(events.try_recv().is_err());
        }
        let reply = request(&p, "secret", "parent").await;
        let _ = events.recv().await.unwrap();
        p.interrupt_requested.store(true, Ordering::Release);
        p.pending_approvals.lock().unwrap().clear();
        assert_eq!(reply.await.unwrap()["decision"], "deny");
    }
    #[tokio::test]
    async fn verifies_exact_enabled_all_tool_hook() {
        let b = Bridge {
            directory: Arc::new(tempfile::tempdir().unwrap()),
            address: "unused".into(),
            token: "secret".into(),
            command: "exact-command".into(),
            listener: tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        };
        let mut v = json!({"event":"command_result","command":{"name":"hooks","data":{"hooks":[{"name":"dogcodes-approvals","enabled":true,"source":b.directory.path().join(".agents/hooks.json"),"actions":[{"event":"PreToolUse","matcher":".*","command":"exact-command"}]}]}}});
        assert!(b.verified(v.to_string().as_bytes()));
        v["command"]["data"]["hooks"][0]["enabled"] = json!(false);
        assert!(!b.verified(v.to_string().as_bytes()));
        v["command"]["data"]["hooks"][0]["enabled"] = json!(true);
        v["command"]["data"]["hooks"][0]["actions"][0]["matcher"] = json!("run_command");
        assert!(!b.verified(v.to_string().as_bytes()));
    }
}
