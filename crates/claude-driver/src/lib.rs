//! Claude Code headless stream-json product driver.
//!
//! Drives the user-installed `claude` CLI directly over its documented
//! headless protocol (`-p --input-format stream-json --output-format
//! stream-json`), so Claude Code needs no extra npm dependencies — the same
//! dependency model as the Codex driver. One CLI process owns one native
//! session; turns are serialized per process and the process stays warm
//! between turns for live session reuse.
//!
//! Native `can_use_tool` requests are forwarded to the desktop approval flow.
//! Permission modes are process-scoped: a policy change closes the previous
//! process before resuming the same transcript with the selected native mode.

mod autonomous;
mod background_output;
mod child_events;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
mod models;
mod native;
#[cfg(all(test, unix))]
mod native_tests;
mod readiness;
#[cfg(all(test, unix))]
mod readiness_tests;
mod session_scope;
use session_scope::{NativeAttempt, stale_scope};
mod request_usage;
mod selection;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_domain::{ExecutorRef, InstallationId, ProviderId};
use agent_runtime::OwnedChild;
use async_trait::async_trait;
use driver_api::{
    AgentDriver, ApprovalOption, ApprovalSemanticRole, ContextDelivery, ContextDeliveryKind,
    ContextUsageSnapshot, DriverError, DriverEvent, DriverEventStream,
    PromptRequest as HubPromptRequest, QuotaProvider, QuotaSnapshot, QuotaWindow, SessionHandle,
    SessionRequest, SessionResumeCapability, explicitly_reports_quota_exhaustion,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::{mpsc, oneshot, watch},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Per-line frame cap; tool results and control responses can be large but
/// must never grow without bound.
const FRAME_LIMIT: usize = 2 * 1024 * 1024;
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Environment variables handed to the CLI; everything else is stripped with
/// `env_clear` so credentials never leak into the agent process. `USER` is not a
/// secret: on macOS Claude Code finds its keychain login by account name, and
/// without it reports "Not logged in" and cannot renew its token.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "TZ", "TMPDIR", "TERM",
];

pub struct ClaudeLaunch {
    pub executable: PathBuf,
}

/// Protocol arguments every session uses; the driver owns them so manifests
/// cannot widen permissions or disable the control plane.
fn session_args(session_uuid: &str, resume: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        // Ask-before-use: `--permission-prompt-tool stdio` (the same hidden
        // flag the official Agent SDK sets) routes tool approvals to the
        // can_use_tool control request; `manual` keeps every tool gated.
        "--permission-mode",
        "manual",
        "--permission-prompt-tool",
        "stdio",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if resume {
        // `--resume` keeps the original session id. Combining it with
        // `--session-id` makes the CLI exit at startup unless
        // `--fork-session` is also passed, which the hub never wants.
        args.push("--resume".into());
        args.push(session_uuid.into());
    } else {
        args.push("--session-id".into());
        args.push(session_uuid.into());
    }
    args
}

#[derive(Debug)]
enum Classified {
    Frames(Vec<DriverEvent>),
    /// `rate_limit_event`: session-level subscription window pushes (may
    /// arrive with no turn active). Account-scoped, shared across sessions.
    PlanLimit(Vec<QuotaWindow>),
    Terminal(DriverEvent, Option<ContextUsageSnapshot>),
    CanUseTool {
        request_id: String,
        tool_name: String,
        input: Value,
    },
    ControlResponse {
        request_id: String,
        success: bool,
        data: Value,
    },
    Ignored,
}

fn classify(line: &str) -> Option<Classified> {
    let event: Value = serde_json::from_str(line).ok()?;
    match event.get("type")?.as_str()? {
        "control_response" => {
            let response = event.get("response")?;
            Some(Classified::ControlResponse {
                request_id: response.get("request_id")?.as_str()?.to_owned(),
                success: response.get("subtype")?.as_str()? == "success",
                data: response.get("response").cloned().unwrap_or(Value::Null),
            })
        }
        "control_request" => {
            let request = event.get("request")?;
            if request.get("subtype")?.as_str()? == "can_use_tool" {
                Some(Classified::CanUseTool {
                    request_id: event.get("request_id")?.as_str()?.to_owned(),
                    tool_name: request
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    input: request.get("input").cloned().unwrap_or_else(|| json!({})),
                })
            } else {
                Some(Classified::Ignored)
            }
        }
        "stream_event" => {
            if event
                .get("parent_tool_use_id")
                .and_then(Value::as_str)
                .is_some()
            {
                return Some(Classified::Ignored);
            }
            let inner = event.get("event")?;
            if inner.get("type")?.as_str()? == "content_block_delta" {
                let delta = inner.get("delta")?;
                // Thinking deltas stay unrecorded: their visibility is
                // unverified, and the hub never infers hidden reasoning.
                if delta.get("type")?.as_str()? == "text_delta" {
                    return Some(Classified::Frames(vec![DriverEvent::TextDelta {
                        item_id: "message".into(),
                        text: delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    }]));
                }
            }
            Some(Classified::Ignored)
        }
        "assistant" => {
            let content = event.get("message")?.get("content")?.as_array()?;
            let frames = content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|block| DriverEvent::ToolStarted {
                    item_id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("claude-tool")
                        .to_owned(),
                    tool_name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    summary: format!(
                        "正在使用 {}",
                        block.get("name").and_then(Value::as_str).unwrap_or("工具")
                    ),
                })
                .collect::<Vec<_>>();
            Some(if frames.is_empty() {
                Classified::Ignored
            } else {
                Classified::Frames(frames)
            })
        }
        "user" => {
            let content = event
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)?;
            let frames = content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .map(|block| DriverEvent::ToolCompleted {
                    item_id: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("claude-tool")
                        .to_owned(),
                    tool_name: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    summary: if block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "工具调用失败".into()
                    } else {
                        "工具调用已完成".into()
                    },
                    success: !block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect::<Vec<_>>();
            Some(if frames.is_empty() {
                Classified::Ignored
            } else {
                Classified::Frames(frames)
            })
        }
        "rate_limit_event" => {
            // Shape verified by codex-host against real payloads:
            // rate_limit_info.unifiedWindows.{five_hour,seven_day} with
            // `utilization` as a 0-1 fraction (the SDK .d.ts comment saying
            // 0-100 is wrong) and `resetsAt` in unix seconds. A flat
            // rateLimitType + utilization fallback covers older builds.
            let info = event.get("rate_limit_info")?;
            let mut windows = Vec::new();
            let unified = info.get("unifiedWindows");
            for (node, minutes, id) in [
                (unified.and_then(|w| w.get("five_hour")), 300, "five_hour"),
                (
                    unified.and_then(|w| w.get("seven_day")),
                    10_080,
                    "seven_day",
                ),
            ] {
                if let Some(window) = parse_plan_window(node, minutes, id) {
                    windows.push(window);
                }
            }
            if windows.is_empty()
                && let (Some(window), Some(kind)) = (
                    parse_plan_window(Some(info), 0, "flat"),
                    info.get("rateLimitType").and_then(Value::as_str),
                )
            {
                let minutes = match kind {
                    "five_hour" => Some(300),
                    "seven_day" => Some(10_080),
                    _ => None,
                };
                if let Some(minutes) = minutes {
                    windows.push(QuotaWindow {
                        scope: None,
                        id: kind.to_owned(),
                        window_minutes: minutes,
                        ..window
                    });
                }
            }
            (!windows.is_empty()).then_some(Classified::PlanLimit(windows))
        }
        "result" => {
            let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("");
            let terminal = match subtype {
                "success" if event["is_error"] != true => DriverEvent::TurnCompleted {
                    summary: "Claude Code turn completed".into(),
                },
                _ if claude_result_reports_quota_exhaustion(&event) => {
                    DriverEvent::TurnQuotaExhausted {
                        resets_at_unix: event
                            .get("resets_at")
                            .or_else(|| event.get("resetsAt"))
                            .and_then(Value::as_i64),
                    }
                }
                _ => DriverEvent::TurnFailed {
                    reason: claude_result_failure(&event),
                },
            };
            Some(Classified::Terminal(terminal, context_from_result(&event)))
        }
        _ => Some(Classified::Ignored),
    }
}

fn claude_result_reports_quota_exhaustion(event: &Value) -> bool {
    let explicit_code = event
        .get("error_code")
        .or_else(|| event.get("errorCode"))
        .or_else(|| event.get("error_type"))
        .and_then(Value::as_str)
        .is_some_and(|code| matches!(code, "usage_limit_reached" | "quota_exceeded"));
    explicit_code
        || ["error", "error_message", "message"]
            .iter()
            .filter_map(|field| event.get(field).and_then(Value::as_str))
            .any(explicitly_reports_quota_exhaustion)
}

/// Push-driven claude quota only refreshes on turns; older readings are
/// marked stale after an hour so the UI never pretends they are live.
mod quota;
mod turn_failure;
const CLAUDE_QUOTA_STALE_MS: i64 = 60 * 60 * 1_000;

/// Account-scoped quota observations shared by every session process of
/// this driver: plan windows from `rate_limit_event` pushes and the latest
/// context reading from `result` events.
#[derive(Default)]
struct ClaudeQuotaState {
    usage: driver_api::usage::UsageStore,
    pull: tokio::sync::Mutex<quota::Cache>,
    plan_windows: std::sync::Mutex<Vec<QuotaWindow>>,
    windows_at_unix_ms: std::sync::atomic::AtomicI64,
    context: std::sync::Mutex<HashMap<String, (i64, ContextUsageSnapshot)>>,
}

struct SessionProcess {
    working_directory: PathBuf,
    quota: std::sync::Arc<ClaudeQuotaState>,
    session_id: String,
    scope: std::sync::Mutex<SessionHandle>,
    admission: tokio::sync::Mutex<()>,
    native: Arc<native::NativeProcess>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    permission_mode: Option<String>,
    updates: std::sync::Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    child_events: std::sync::Mutex<child_events::ChildEvents>,
    autonomous_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    autonomous_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<DriverEvent>>>,
    autonomous_enabled: AtomicBool,
    turn_active: AtomicBool,
    autonomous_pending: std::sync::atomic::AtomicUsize,
    interrupt_requested: AtomicBool,
    finished: Arc<AtomicBool>,
    closing: AtomicBool,
    cleanup_done: AtomicBool,
    commands: mpsc::UnboundedSender<lifecycle::Command>,
    agent_interactions: std::sync::Mutex<HashMap<String, String>>,
    /// Outstanding `can_use_tool` requests keyed by the native request id,
    /// holding the original tool input so an allow can echo it back.
    pending_approvals: std::sync::Mutex<HashMap<String, Value>>,
    pending_questions: std::sync::Mutex<HashMap<String, Value>>,
    pending_controls:
        std::sync::Mutex<HashMap<String, oneshot::Sender<Result<Value, DriverError>>>>,
}

impl SessionProcess {
    fn start_autonomous_if_root(&self, raw: &Value) {
        if !autonomous::is_root_output(raw) || !self.autonomous_enabled.load(Ordering::Acquire) {
            return;
        }
        let mut sink = self.updates.lock().expect("updates sink");
        if self.turn_active.load(Ordering::Acquire)
            || self.closing.load(Ordering::Acquire)
            || self.native.stopping()
        {
            return;
        }
        let Some(sender) = self
            .autonomous_tx
            .lock()
            .expect("autonomous sender")
            .clone()
        else {
            return;
        };
        self.turn_active.store(true, Ordering::Release);
        self.autonomous_pending.fetch_add(1, Ordering::AcqRel);
        let _ = sender.send(DriverEvent::TurnStarted {
            native_turn_id: None,
        });
        *sink = Some(sender);
    }

    async fn send_line(&self, value: &Value) -> Result<(), DriverError> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| DriverError::Disconnected("Claude stdin closed".into()))?;
        let mut attempt = NativeAttempt::new(self);
        let result = async {
            stdin.write_all(value.to_string().as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await
        .map_err(|_| DriverError::Disconnected("Claude Code stdin is closed".into()));
        if result.is_ok() {
            attempt.confirm();
        }
        result
    }
}

pub struct ClaudeDriver {
    executor: ExecutorRef,
    executable: PathBuf,
    native_version: Option<String>,
    resume_capability: SessionResumeCapability,
    processes: tokio::sync::Mutex<HashMap<String, Arc<SessionProcess>>>,
    stopping: watch::Sender<bool>,
    retired_handles: std::sync::Mutex<HashMap<String, SessionHandle>>,
    restore_gates: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    catalog_probe: Arc<tokio::sync::Mutex<()>>,
    /// Shared quota observations (plan windows + context), fed by every
    /// session process of this driver.
    quota: Arc<ClaudeQuotaState>,
}

impl ClaudeDriver {
    /// Creates a driver for one concrete installation. `resume_capability`
    /// must come from real runtime evidence, never from documentation.
    pub fn new(
        provider_id: &str,
        installation_id: &str,
        executable: PathBuf,
        native_version: Option<String>,
        resume_capability: SessionResumeCapability,
    ) -> Result<Self, agent_domain::DomainError> {
        Ok(Self {
            executor: ExecutorRef {
                provider_id: ProviderId::parse(provider_id)?,
                installation_id: Some(
                    InstallationId::parse(installation_id)
                        .map_err(|error| {
                            agent_domain::DomainError::InvalidProviderId(error.to_string())
                        })?
                        .to_string(),
                ),
            },
            executable,
            native_version,
            resume_capability,
            processes: tokio::sync::Mutex::new(HashMap::new()),
            stopping: watch::channel(false).0,
            retired_handles: std::sync::Mutex::new(HashMap::new()),
            restore_gates: std::sync::Mutex::new(HashMap::new()),
            catalog_probe: Arc::new(tokio::sync::Mutex::new(())),
            quota: Arc::new(ClaudeQuotaState::default()),
        })
    }

    async fn acquire_session(
        &self,
        native_session_id: &str,
        workspace: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
        configure: bool,
    ) -> Result<SessionHandle, DriverError> {
        if self.has_autonomous_turn(native_session_id).await {
            return Err(DriverError::AutonomousTurnBusy);
        }
        let gate = self
            .restore_gates
            .lock()
            .unwrap()
            .entry(native_session_id.into())
            .or_default()
            .clone();
        let _restoring = gate.lock().await;
        let existing = self.processes.lock().await.get(native_session_id).cloned();
        if let Some(process) = &existing {
            let _admission = process.admission.lock().await;
            let reusable = {
                let _sink = process.updates.lock().expect("updates sink");
                if process.turn_active.load(Ordering::Acquire)
                    || process.autonomous_pending.load(Ordering::Acquire) > 0
                {
                    return Err(DriverError::AutonomousTurnBusy);
                }
                !process.finished.load(Ordering::Acquire)
                    && !process.closing.load(Ordering::Acquire)
                    && !process.native.stopping()
                    && !process.commands.is_closed()
                    && process.permission_mode == workspace.permission_mode
                    && workspace.permission_mode.as_deref() != Some("plan")
            };
            if reusable {
                // Keep the original scope/continuation observer on a verified
                // configuration rollback. Only a successful admission rotates.
                if configure {
                    process.configure(model, thinking).await?;
                }
                let _sink = process.updates.lock().expect("updates sink");
                let mut scope = process.scope.lock().expect("Claude session scope");
                if process.turn_active.load(Ordering::Acquire)
                    || process.autonomous_pending.load(Ordering::Acquire) > 0
                {
                    if configure {
                        process.request_close();
                    }
                    return Err(DriverError::AutonomousTurnBusy);
                }
                if process.closing.load(Ordering::Acquire) || process.native.stopping() {
                    return Err(DriverError::Disconnected(
                        "Claude session is closing".into(),
                    ));
                }
                let handle = SessionHandle::scoped(native_session_id.into());
                let (sender, receiver) = mpsc::unbounded_channel();
                *process.autonomous_tx.lock().expect("autonomous sender") = Some(sender);
                *process.autonomous_rx.lock().expect("autonomous receiver") = Some(receiver);
                *scope = handle.clone();
                return Ok(handle);
            }
        }
        if self.resume_capability != SessionResumeCapability::Supported {
            return Err(DriverError::CapabilityUnavailable(
                "claude --resume has not been runtime verified for this hub".into(),
            ));
        }
        if workspace.permission_mode.as_deref() == Some("auto") {
            self.permission_catalog(Some(&workspace.working_directory))
                .await?
                .validate(Some("auto"))?;
        }
        if let Some(process) = existing {
            self.shutdown(process.handle()).await?;
        }
        // Resuming re-attaches to the stored transcript; the session id stays
        // the one the hub already recorded.
        let child = spawn_selected(
            &ClaudeLaunch {
                executable: self.executable.clone(),
            },
            native_session_id,
            true,
            Some(&workspace.working_directory),
            &workspace.additional_directories,
            SessionSelection {
                analysis_only: false,
                model,
                thinking,
                permission: workspace.permission_mode.as_deref(),
            },
            workspace.history_mcp.as_ref(),
        )?;
        let process = self
            .install_session(
                native_session_id,
                child,
                workspace.working_directory.into(),
                workspace.permission_mode,
                workspace.history_mcp.is_some(),
            )
            .await?;
        let mut admission = NativeAttempt::new(&process);
        if (model.is_some() || thinking.is_some())
            && let Err(error) = process.verify_selection(model, thinking).await
        {
            process.request_close();
            return Err(error);
        }
        admission.confirm();
        Ok(process.handle())
    }

    /// Runs the control-plane `initialize` handshake against the local CLI to
    /// prove it really is Claude Code with a live stream-json control channel.
    /// No model turn is started and no quota is consumed.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NotConfigured`] when the executable cannot start
    /// and [`DriverError::Protocol`] when the handshake fails or times out.
    pub async fn probe_ready(executable: &std::path::Path) -> Result<(), DriverError> {
        readiness::probe(executable).await
    }

    async fn install_session(
        &self,
        session_id: &str,
        child: OwnedChild,
        working_directory: PathBuf,
        permission_mode: Option<String>,
        initialize_history: bool,
    ) -> Result<Arc<SessionProcess>, DriverError> {
        let (native, stdin, stdout) = native::NativeProcess::start(child)?;
        native.observe_owner(self.stopping.subscribe());
        let (autonomous_tx, autonomous_rx) = mpsc::unbounded_channel();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let process = Arc::new(SessionProcess {
            working_directory,
            quota: Arc::clone(&self.quota),
            session_id: session_id.to_owned(),
            scope: std::sync::Mutex::new(SessionHandle::scoped(session_id.into())),
            admission: tokio::sync::Mutex::new(()),
            native,
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            permission_mode,
            updates: std::sync::Mutex::new(None),
            autonomous_tx: std::sync::Mutex::new(Some(autonomous_tx)),
            autonomous_rx: std::sync::Mutex::new(Some(autonomous_rx)),
            autonomous_enabled: AtomicBool::new(false),
            child_events: std::sync::Mutex::new(child_events::ChildEvents::default()),
            turn_active: AtomicBool::new(false),
            autonomous_pending: std::sync::atomic::AtomicUsize::new(0),
            interrupt_requested: AtomicBool::new(false),
            finished: Arc::new(AtomicBool::new(false)),
            closing: AtomicBool::new(false),
            cleanup_done: AtomicBool::new(false),
            commands,
            agent_interactions: std::sync::Mutex::new(HashMap::new()),
            pending_approvals: std::sync::Mutex::new(HashMap::new()),
            pending_questions: std::sync::Mutex::new(HashMap::new()),
            pending_controls: std::sync::Mutex::new(HashMap::new()),
        });
        let mut reader = BufReader::new(stdout);
        if initialize_history {
            process.send_line(&json!({"type":"control_request","request_id":"dogcodes-history-init","request":{"subtype":"initialize","hooks":{}}})).await?;
            tokio::time::timeout(PROBE_TIMEOUT, async {
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.map_err(|_| {
                        DriverError::Disconnected("claude initialization failed".into())
                    })? == 0
                        || line.len() > FRAME_LIMIT
                    {
                        return Err(DriverError::Disconnected(
                            "claude initialization ended".into(),
                        ));
                    }
                    if let Some(Classified::ControlResponse {
                        request_id,
                        success,
                        ..
                    }) = classify(line.trim_end())
                        && request_id == "dogcodes-history-init"
                    {
                        return if success {
                            Ok(())
                        } else {
                            Err(DriverError::Protocol(
                                "claude initialization rejected".into(),
                            ))
                        };
                    }
                }
            })
            .await
            .map_err(|_| DriverError::Disconnected("claude initialization timed out".into()))??;
        }
        let mut processes = self.processes.lock().await;
        if processes
            .get(session_id)
            .is_some_and(|old| !old.cleanup_done.load(Ordering::Acquire))
        {
            return Err(DriverError::CapabilityUnavailable(
                "Claude session is already owned".into(),
            ));
        }
        processes.insert(session_id.to_owned(), Arc::clone(&process));
        drop(processes);
        tokio::spawn(run_session(Arc::clone(&process), reader, command_rx));
        Ok(process)
    }
}

fn spawn(
    launch: &ClaudeLaunch,
    session_uuid: &str,
    resume: bool,
    working_directory: Option<&str>,
) -> Result<OwnedChild, DriverError> {
    spawn_workspace(launch, session_uuid, resume, working_directory, &[])
}

fn spawn_workspace(
    launch: &ClaudeLaunch,
    session_uuid: &str,
    resume: bool,
    working_directory: Option<&str>,
    additional_directories: &[String],
) -> Result<OwnedChild, DriverError> {
    spawn_selected(
        launch,
        session_uuid,
        resume,
        working_directory,
        additional_directories,
        SessionSelection::default(),
        None,
    )
}
#[derive(Default)]
struct SessionSelection<'a> {
    analysis_only: bool,
    model: Option<&'a str>,
    thinking: Option<&'a str>,
    permission: Option<&'a str>,
}

fn spawn_selected(
    launch: &ClaudeLaunch,
    session_uuid: &str,
    resume: bool,
    working_directory: Option<&str>,
    additional_directories: &[String],
    selection: SessionSelection<'_>,
    history_mcp: Option<&driver_api::HistoryMcp>,
) -> Result<OwnedChild, DriverError> {
    let SessionSelection {
        model,
        thinking,
        permission,
        analysis_only,
    } = selection;
    // The app retains the private configuration directory for the live session.
    let mut command = tokio::process::Command::new(&launch.executable);
    if analysis_only {
        command.args([
            "--tools",
            "Read,Glob,Grep,Bash",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            // Preserve account/model routing in user settings. Project/local
            // settings remain excluded; hooks and MCP stay disabled.
            "--setting-sources",
            "user",
            "--settings",
            "{\"disableAllHooks\":true}",
        ]);
    }
    if let Some(mcp) = history_mcp {
        // Only these five app tools load eagerly; keep other MCP servers' deferral policy.
        let mut server = mcp.server_config();
        server["alwaysLoad"] = json!(true);
        let path = std::path::Path::new(&mcp.directory).join("claude-mcp.json");
        std::fs::write(
            &path,
            json!({"mcpServers":{mcp.name.clone():server}}).to_string(),
        )
        .map_err(|_| DriverError::Disconnected("history MCP configuration unavailable".into()))?;
        command.arg("--mcp-config").arg(path);
    }
    if let Some(model) = model.filter(|m| *m != "default") {
        command.arg("--model").arg(model);
    }
    if let Some(thinking) = thinking {
        command
            .arg("--settings")
            .arg(json!({"alwaysThinkingEnabled": thinking != "off"}).to_string());
        if !["off", "auto"].contains(&thinking) {
            command.arg("--effort").arg(thinking);
        }
    }
    for directory in additional_directories {
        command.arg("--add-dir").arg(directory);
    }
    driver_api::permissions::claude_with_auto(true).validate(permission)?;
    let mut args = session_args(session_uuid, resume);
    if let Some(mode) = permission {
        let index = args.iter().position(|a| a == "--permission-mode").unwrap();
        args[index + 1] = mode.into();
        if mode == "bypassPermissions" {
            args.push("--allow-dangerously-skip-permissions".into());
        }
    }
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(directory) = working_directory {
        command.current_dir(directory);
    }
    agent_runtime::apply_platform_environment(&mut command);
    for variable in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(variable) {
            command.env(variable, value);
        }
    }
    agent_runtime::apply_proxy_environment(&mut command);
    OwnedChild::spawn(command).map_err(|error| {
        if matches!(error.kind(), std::io::ErrorKind::NotFound) {
            DriverError::NotConfigured("claude executable is missing".into())
        } else {
            DriverError::Disconnected(format!("claude process failed to start: {error}"))
        }
    })
}

async fn run_session(
    process: Arc<SessionProcess>,
    reader: BufReader<tokio::io::DuplexStream>,
    mut commands: mpsc::UnboundedReceiver<lifecycle::Command>,
) {
    let mut lines = reader.lines();
    let mut lifecycle = lifecycle::Lifecycle::default();
    let mut latest_message: Option<Value> = None;
    let mut usage_requests = request_usage::Requests::default();
    let mut first_output_reported = false;
    let mut turn_errors = turn_failure::TurnErrors::default();
    let mut background_outputs = background_output::BackgroundOutputs::default();
    let mut output_poll = tokio::time::interval(std::time::Duration::from_millis(250));
    output_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let actions = lifecycle.poll(
            !process
                .agent_interactions
                .lock()
                .expect("agent interactions")
                .is_empty(),
        );
        let mut force_close = false;
        for action in actions {
            match action {
                lifecycle::Action::Stop(id) => {
                    if process
                        .send_control(json!({"subtype":"stop_task","task_id":id}))
                        .await
                        .is_err()
                    {
                        force_close = true;
                        break;
                    }
                }
                lifecycle::Action::Interrupt => {
                    if process
                        .send_control(json!({"subtype":"interrupt"}))
                        .await
                        .is_err()
                    {
                        force_close = true;
                        break;
                    }
                }
                lifecycle::Action::Finish(event) => {
                    process.close_interactions(true).await;
                    process.finish_turn(event);
                }
                lifecycle::Action::Close => force_close = true,
            }
        }
        if force_close {
            break;
        }
        let line = tokio::select! {
            biased;
            command = commands.recv() => {
                match command {
                    Some(lifecycle::Command::Begin(handle)) => {
                        let current = process.scope.lock().expect("Claude session scope");
                        if current.same_scope(&handle) { lifecycle.begin(); first_output_reported = false; }
                    },
                    Some(lifecycle::Command::Cancel { handle, response }) => {
                        let result = {
                            let current = process.scope.lock().expect("Claude session scope");
                            if !current.same_scope(&handle) { Err(stale_scope()) }
                            else if !process.turn_active.load(Ordering::Acquire) {
                                Err(DriverError::CapabilityUnavailable("Claude has no active turn".into()))
                            } else {
                                lifecycle.cancel();
                                process.interrupt_requested.store(true, Ordering::Release);
                                Ok(())
                            }
                        };
                        if result.is_ok() { process.close_interactions(true).await; }
                        let _ = response.send(result);
                    }
                    Some(lifecycle::Command::Close) | None => break,
                }
                continue;
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(25)), if lifecycle.waiting() => continue,
            _ = output_poll.tick(), if !background_outputs.is_empty() => {
                background_outputs.poll().await;
                continue;
            }
            line = lines.next_line() => match line { Ok(Some(line)) => line, _ => break },
        };
        if line.len() > FRAME_LIMIT {
            break;
        }
        if let Ok(event) = serde_json::from_str::<Value>(&line)
            && event.get("type").and_then(Value::as_str) == Some("assistant")
            && event
                .get("message")
                .and_then(|message| message.get("usage"))
                .is_some()
        {
            latest_message = event.get("message").cloned();
        }
        if let Ok(mut raw) = serde_json::from_str::<Value>(&line) {
            let usage_request =
                usage_requests.observe(&raw, &process.quota.usage, &process.session_id);
            if !process.turn_active.load(Ordering::Acquire) && autonomous::is_root_output(&raw) {
                first_output_reported = false;
                lifecycle.begin();
            }
            process.start_autonomous_if_root(&raw);
            lifecycle.observe(&raw);
            turn_errors.observe(&raw);
            let current = process.updates.lock().expect("updates sink").clone();
            if raw["parent_tool_use_id"].as_str().is_none_or(str::is_empty)
                && raw["type"] == "stream_event"
            {
                if raw["event"]["type"] == "message_start" {
                    first_output_reported = false;
                }
                let delta = &raw["event"]["delta"];
                if !first_output_reported
                    && ["text", "thinking"]
                        .iter()
                        .any(|key| delta[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
                {
                    first_output_reported = true;
                    if let Some(sink) = &current {
                        let _ = sink.send(DriverEvent::FirstOutput);
                    }
                }
            }
            if let (Some(request), Some(sink)) = (usage_request, &current) {
                let _ = sink.send(DriverEvent::UsageRecorded { request });
            }
            if raw["type"] == "control_cancel_request" {
                if let Some(id) = raw["request_id"].as_str() {
                    process.expire_interaction(id);
                }
                continue;
            }
            if raw["subtype"] == "task_notification"
                && matches!(
                    raw["status"].as_str(),
                    Some("completed" | "failed" | "stopped" | "interrupted")
                )
            {
                for scope in
                    lifecycle.finished_agent_scopes(raw["task_id"].as_str().unwrap_or_default())
                {
                    process
                        .close_agent_interactions(Some(&scope), !lifecycle.has_agents())
                        .await;
                }
            }
            if raw["type"] == "system"
                && raw["subtype"] == "background_tasks_changed"
                && raw["tasks"].as_array().is_some_and(Vec::is_empty)
            {
                process.close_agent_interactions(None, true).await;
            }
            background_outputs.observe(
                &raw,
                &process.child_events.lock().expect("child events"),
                current.as_ref(),
            );
            background_outputs.settle(&mut raw).await;
            process
                .child_events
                .lock()
                .expect("child events")
                .dispatch(&raw, current.as_ref());
            // Child tools/messages belong only to their original presentation stream.
            if raw["type"] != "control_request"
                && raw["parent_tool_use_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
            {
                continue;
            }
        }
        let Some(classified) = classify(line.trim_end()) else {
            continue;
        };
        match classified {
            Classified::Frames(frames) => {
                if let Some(updates) = process.updates.lock().expect("updates sink").as_ref() {
                    for frame in frames {
                        if let DriverEvent::ToolCompleted { item_id, .. } = &frame
                            && process
                                .child_events
                                .lock()
                                .expect("child events")
                                .defers_tool_completion(item_id)
                        {
                            continue;
                        }
                        let _ = updates.send(frame);
                    }
                }
            }
            Classified::PlanLimit(windows) => {
                // Account-level push: record and stay silent on the stream
                // (subscription windows are not timeline content).
                *process.quota.plan_windows.lock().expect("plan windows") = windows;
                process.quota.windows_at_unix_ms.store(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
                    Ordering::Release,
                );
            }
            Classified::Terminal(driver_event, aggregate_context) => {
                let driver_event = turn_errors.finish(driver_event);
                if let Ok(result) = serde_json::from_str::<Value>(&line) {
                    process
                        .quota
                        .usage
                        .set(&process.session_id, driver_api::usage::claude(&result));
                }
                // result.usage sums multiple model calls in a turn. Context
                // occupancy must use the last assistant request, never that sum.
                let context = latest_message.take().and_then(|message| {
                    let mut result: Value = serde_json::from_str(&line).ok()?;
                    result["usage"] = message.get("usage")?.clone();
                    result["_contextModel"] = message.get("model").cloned().unwrap_or(Value::Null);
                    // Require a model window in the terminal result; only the
                    // token numerator is replaced by the last message usage.
                    aggregate_context?;
                    context_from_result(&result)
                });
                if let Some(context) = context {
                    process.quota.usage.merge(
                        &process.session_id,
                        driver_api::usage::SessionUsageSnapshot {
                            context_used_tokens: Some(context.used_tokens),
                            context_window_tokens: Some(context.window_tokens),
                            cache_hit_rate_percent: context.cache_hit_rate_percent,
                            ..driver_api::usage::SessionUsageSnapshot::new()
                        },
                    );
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                    process
                        .quota
                        .context
                        .lock()
                        .expect("context usage")
                        .insert(process.session_id.clone(), (now, context));
                }
                process.close_interactions(false).await;
                lifecycle.result(driver_event);
            }
            Classified::CanUseTool {
                request_id,
                tool_name,
                input,
            } => {
                let raw: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if lifecycle.cancelling() || !process.turn_active.load(Ordering::Acquire) {
                    let _ = process.deny_interaction(&request_id).await;
                    continue;
                }
                if let Some(agent) = raw["request"]["agent_id"]
                    .as_str()
                    .or_else(|| raw["parent_tool_use_id"].as_str())
                    .filter(|id| !id.is_empty())
                {
                    process
                        .agent_interactions
                        .lock()
                        .expect("agent interactions")
                        .insert(request_id.clone(), agent.into());
                }
                if tool_name == "AskUserQuestion" {
                    if let Ok(form) = driver_api::questions::native_form("claude", &input) {
                        process
                            .pending_questions
                            .lock()
                            .expect("questions")
                            .insert(request_id.clone(), input);
                        if let Some(updates) = process.updates.lock().expect("updates").as_ref() {
                            let _ = updates.send(DriverEvent::QuestionRequested {
                                native_request_id: request_id,
                                form,
                            });
                        }
                    } else {
                        process
                            .agent_interactions
                            .lock()
                            .expect("agent interactions")
                            .remove(&request_id);
                        let _=process.send_line(&json!({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":{"behavior":"deny","message":"Unsupported question format"}}})).await;
                    }
                    continue;
                }
                // Keep native input intact for the response; shorten only the
                // display path before the hub's normal secret redaction.
                process
                    .pending_approvals
                    .lock()
                    .expect("pending approvals")
                    .insert(request_id.clone(), input.clone());
                if let Some(updates) = process.updates.lock().expect("updates sink").as_ref() {
                    let _ = updates.send(DriverEvent::ApprovalRequested {
                        native_request_id: request_id,
                        command_summary: approval_summary(
                            &tool_name,
                            &input,
                            &process.working_directory,
                        ),
                        tool_name,
                        options: claude_approval_options(),
                    });
                }
            }
            // control_response frames belong to probe/interrupt bookkeeping;
            // the result event is the authoritative turn terminal.
            Classified::ControlResponse {
                request_id,
                success,
                data,
            } => {
                if let Some(reply) = process
                    .pending_controls
                    .lock()
                    .expect("controls")
                    .remove(&request_id)
                {
                    let _ = reply.send(if success {
                        Ok(data)
                    } else {
                        Err(DriverError::Protocol("selection_rejected".into()))
                    });
                }
            }
            Classified::Ignored => {}
        }
    }
    process.closing.store(true, Ordering::Release);
    process.native.request_close();
    process.stdin.lock().await.take();
    // The supervisor retains the exact OS scope through any timeout/failure.
    process.native.confirmed().await;
    background_outputs.finish().await;
    process.finished.store(true, Ordering::Release);
    // Only confirmed process shutdown may substitute for missing native task terminals.
    process.close_interactions(true).await;
    process.child_events.lock().expect("child events").close();
    process.finish_turn(if process.interrupt_requested.load(Ordering::Acquire) {
        DriverEvent::TurnInterrupted {
            summary: "Claude Code process stopped after cancellation".into(),
        }
    } else {
        DriverEvent::TurnFailed {
            reason: "Claude Code stream ended before the turn completed".into(),
        }
    });
    for (_, reply) in process.pending_controls.lock().expect("controls").drain() {
        let _ = reply.send(Err(DriverError::Disconnected(
            "Claude closed before confirming configuration".into(),
        )));
    }
    process
        .autonomous_tx
        .lock()
        .expect("autonomous sender")
        .take();
    process.cleanup_done.store(true, Ordering::Release);
}

async fn read_until(
    mut reader: BufReader<tokio::io::DuplexStream>,
    timeout: std::time::Duration,
    mut matches: impl FnMut(&Classified) -> bool,
) -> Result<bool, DriverError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let mut line = String::new();
        let mut capped = tokio::io::AsyncReadExt::take(
            &mut reader,
            u64::try_from(FRAME_LIMIT)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        );
        let read = tokio::time::timeout_at(deadline, capped.read_line(&mut line)).await;
        match read {
            Err(_) => return Err(DriverError::Protocol("timed out".into())),
            Ok(Err(error)) => return Err(DriverError::Disconnected(error.to_string())),
            Ok(Ok(0)) => return Ok(false),
            Ok(Ok(_)) => {
                if line.len() > FRAME_LIMIT {
                    return Err(DriverError::Protocol(
                        "frame exceeded the line budget".into(),
                    ));
                }
                if let Some(classified) = classify(line.trim_end())
                    && matches(&classified)
                {
                    return Ok(true);
                }
            }
        }
    }
}

/// The two behaviors Claude Code's control protocol accepts for a
/// `can_use_tool` request; the option ids below are the protocol contract.
fn claude_approval_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            option_id: "allow".into(),
            label: "Allow once".into(),
            semantic_role: ApprovalSemanticRole::ApproveOnce,
        },
        ApprovalOption {
            option_id: "deny".into(),
            label: "Deny".into(),
            semantic_role: ApprovalSemanticRole::Reject,
        },
    ]
}

/// Parses one plan-limit window node. `utilization` arrives as a 0-1
/// fraction on real payloads (codex-host verified); values above 1 are
/// defensively treated as percents and clamped either way.
fn parse_plan_window(node: Option<&Value>, window_minutes: u64, id: &str) -> Option<QuotaWindow> {
    let node = node?;
    let resets_at_unix = node.get("resetsAt").and_then(Value::as_i64);
    let Some(utilization) = node.get("utilization").and_then(Value::as_f64) else {
        return resets_at_unix.map(|reset| QuotaWindow {
            scope: None,
            id: id.to_owned(),
            window_minutes,
            used_percent: None,
            resets_at_unix: Some(reset),
        });
    };
    if !(0.0..=100.0).contains(&utilization) {
        return None;
    }
    let used_percent = if utilization <= 1.0 {
        (utilization * 100.0).clamp(0.0, 100.0)
    } else {
        utilization.clamp(0.0, 100.0)
    };
    Some(QuotaWindow {
        scope: None,
        id: id.to_owned(),
        window_minutes,
        used_percent: Some((used_percent * 100.0).round() / 100.0),
        resets_at_unix: node.get("resetsAt").and_then(Value::as_i64),
    })
}

/// Extracts the live context reading from a `result` event: the prompt-side
/// token sum over the model's context window, plus cache hit rate from
/// cache-read tokens (same formula codex-host reports).
fn claude_result_failure(event: &Value) -> String {
    turn_failure::result_failure(event)
}

fn context_from_result(event: &Value) -> Option<ContextUsageSnapshot> {
    let result = event
        .get("result")
        .filter(|value| value.is_object())
        .unwrap_or(event);
    let usage = result.get("usage")?;
    let input = usage.get("input_tokens").and_then(Value::as_u64)?;
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let window_tokens = result
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|models| {
            result
                .get("_contextModel")
                .and_then(Value::as_str)
                .and_then(|model| models.get(model))
                .or_else(|| {
                    (models.len() == 1)
                        .then(|| models.values().next())
                        .flatten()
                })
        })
        .and_then(|model| model.get("contextWindow"))
        .and_then(Value::as_u64)?;
    if window_tokens == 0 {
        return None;
    }
    let prompt_tokens = input
        .saturating_add(cache_read)
        .saturating_add(cache_creation);
    let cache_hit_rate_percent = if prompt_tokens > 0 {
        #[allow(clippy::cast_precision_loss)]
        Some(((cache_read as f64 / prompt_tokens as f64) * 100.0).clamp(0.0, 100.0))
    } else {
        None
    };
    Some(ContextUsageSnapshot {
        used_tokens: prompt_tokens,
        window_tokens,
        cache_hit_rate_percent,
    })
}

fn approval_summary(tool: &str, input: &Value, cwd: &std::path::Path) -> String {
    let mut display = input.clone();
    if matches!(tool, "Read" | "Write" | "Edit")
        && let Some(path) = input.get("file_path").and_then(Value::as_str)
        && let Ok(relative) = std::path::Path::new(path).strip_prefix(cwd)
        && !relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        display["file_path"] = Value::String(relative.to_string_lossy().into_owned());
    }
    display.to_string()
}

fn context_block(delivery: &ContextDelivery) -> Value {
    let kind = match delivery.kind {
        ContextDeliveryKind::Handoff => "Handoff",
        ContextDeliveryKind::Reconciliation => "Reconciliation",
    };
    let body = String::from_utf8_lossy(&delivery.content);
    json!({
        "type": "text",
        "text": format!(
            "[dogcodes {kind} {} sha256={}]\n{body}",
            delivery.artifact_id, delivery.payload_sha256
        ),
    })
}

fn user_message(blocks: Vec<Value>) -> Value {
    json!({
        "type": "user",
        "message": {"role": "user", "content": blocks},
        "parent_tool_use_id": null,
    })
}

#[async_trait]
impl AgentDriver for ClaudeDriver {
    fn supports_readonly_analysis(&self) -> bool {
        true
    }
    async fn create_analysis_session(&self, directory: &str) -> Result<SessionHandle, DriverError> {
        self.create_analysis_session_with_directories(directory, &[])
            .await
    }
    async fn create_analysis_session_with_directories(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        let id = uuid::Uuid::new_v4().to_string();
        let child = spawn_selected(
            &ClaudeLaunch {
                executable: self.executable.clone(),
            },
            &id,
            false,
            Some(directory),
            additional,
            SessionSelection {
                analysis_only: true,
                permission: Some("plan"),
                ..SessionSelection::default()
            },
            None,
        )?;
        let process = self
            .install_session(&id, child, directory.into(), Some("plan".into()), false)
            .await?;
        Ok(process.handle())
    }

    async fn create_review_session(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        let id = uuid::Uuid::new_v4().to_string();
        let child = spawn_selected(
            &ClaudeLaunch {
                executable: self.executable.clone(),
            },
            &id,
            false,
            Some(directory),
            additional,
            SessionSelection {
                analysis_only: true,
                permission: Some("bypassPermissions"),
                ..SessionSelection::default()
            },
            None,
        )?;
        let process = self
            .install_session(
                &id,
                child,
                directory.into(),
                Some("bypassPermissions".into()),
                false,
            )
            .await?;
        Ok(process.handle())
    }

    async fn permission_catalog(
        &self,
        workspace: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        let (_, auto) = models::read_with_auto(self, workspace).await?;
        Ok(driver_api::permissions::claude_with_auto(auto))
    }
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    fn driver_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn native_agent_version(&self) -> Option<&str> {
        self.native_version.as_deref()
    }

    fn supports_background_events(&self) -> bool {
        true
    }

    async fn has_autonomous_turn(&self, id: &str) -> bool {
        self.processes
            .lock()
            .await
            .get(id)
            .is_some_and(|p| p.autonomous_pending.load(Ordering::Acquire) > 0)
    }

    async fn acknowledge_autonomous_turn(&self, handle: &SessionHandle) {
        if let Ok(process) = self.process_for(handle).await {
            let _ = process.autonomous_pending.fetch_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |count| count.checked_sub(1),
            );
        }
    }

    async fn take_autonomous_events(&self, handle: &SessionHandle) -> Option<DriverEventStream> {
        let process = self.process_for(handle).await.ok()?;
        let _admission = process.admit(handle).await.ok()?;
        let receiver = process
            .autonomous_rx
            .lock()
            .expect("autonomous receiver")
            .take()?;
        process.autonomous_enabled.store(true, Ordering::Release);
        Some(Box::pin(
            UnboundedReceiverStream::new(receiver).map(Ok::<_, DriverError>),
        ))
    }

    fn supports_live_session_reuse(&self) -> bool {
        true
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        self.resume_capability
    }

    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.create_selected_session(request, None, None).await
    }
    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        models::read(self, None).await
    }
    async fn model_catalog_in_workspace(
        &self,
        _model: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<driver_api::ModelCatalog, DriverError> {
        models::read(self, workspace).await
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if request.permission_mode.as_deref() == Some("auto") {
            self.permission_catalog(Some(&request.working_directory))
                .await?
                .validate(Some("auto"))?;
        }
        let session_uuid = uuid::Uuid::new_v4().to_string();
        let child = spawn_selected(
            &ClaudeLaunch {
                executable: self.executable.clone(),
            },
            &session_uuid,
            false,
            Some(request.working_directory.as_str()),
            &request.additional_directories,
            SessionSelection {
                analysis_only: false,
                model,
                thinking,
                permission: request.permission_mode.as_deref(),
            },
            request.history_mcp.as_ref(),
        )?;
        let process = self
            .install_session(
                &session_uuid,
                child,
                request.working_directory.into(),
                request.permission_mode,
                request.history_mcp.is_some(),
            )
            .await?;
        let mut admission = NativeAttempt::new(&process);
        if (model.is_some() || thinking.is_some())
            && let Err(error) = process.verify_selection(model, thinking).await
        {
            process.request_close();
            return Err(error);
        }
        admission.confirm();
        Ok(process.handle())
    }

    async fn resume_session(
        &self,
        native_session_id: &str,
        working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_workspace_session(
            native_session_id,
            SessionRequest {
                history_mcp: None,
                permission_mode: None,
                working_directory: working_directory.into(),
                additional_directories: vec![],
            },
            None,
        )
        .await
    }

    async fn resume_workspace_session(
        &self,
        native_session_id: &str,
        workspace: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_selected_session(native_session_id, workspace, model, None)
            .await
    }
    async fn reconfigure_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.acquire_session(id, request, model, thinking, true)
            .await
    }

    async fn resume_selected_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.acquire_session(id, request, model, thinking, false)
            .await
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: HubPromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let process = self.process_for(handle).await?;
        let _admission = process.admit(handle).await?;
        let attachment_blocks = driver_api::attachments::claude(&request.attachments).await?;
        let mut blocks = Vec::with_capacity(request.context.len() + 1);
        // Port contract: every reviewed context item is delivered, in order,
        // before the user prompt inside the same native turn.
        for delivery in &request.context {
            blocks.push(context_block(delivery));
        }
        blocks.extend(attachment_blocks);
        if !request.prompt.is_empty() {
            blocks.push(json!({ "type": "text", "text": request.prompt }));
        }

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        {
            // One lock covers both admission and sink selection. An unsolicited
            // root frame cannot land on a newly admitted user request midway.
            let mut sink = process.updates.lock().expect("updates sink");
            if process.closing.load(Ordering::Acquire) {
                return Err(DriverError::Disconnected(
                    "Claude session is closing".into(),
                ));
            }
            if process.autonomous_pending.load(Ordering::Acquire) > 0 {
                return Err(DriverError::AutonomousTurnBusy);
            }
            if process.turn_active.swap(true, Ordering::AcqRel) {
                return Err(DriverError::CapabilityUnavailable(
                    "this claude session already has an active turn".into(),
                ));
            }
            process.interrupt_requested.store(false, Ordering::Release);
            *sink = Some(event_tx);
        }
        let mut dispatch = NativeAttempt::new(&process);
        process
            .commands
            .send(lifecycle::Command::Begin(handle.clone()))
            .map_err(|_| DriverError::Disconnected("Claude closed".into()))?;
        if let Err(error) = process.send_line(&user_message(blocks)).await {
            // A partial native write has unknown acceptance. Retire this scope
            // before any later prompt; do not make its actor appear reusable.
            process.request_close();
            return Err(error);
        }
        let stream: DriverEventStream =
            Box::pin(UnboundedReceiverStream::new(event_rx).map(Ok::<_, DriverError>));
        dispatch.confirm();
        Ok(stream)
    }

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        let process = self.process_for(handle).await?;
        let (response, reply) = oneshot::channel();
        {
            let current = process.scope.lock().expect("Claude session scope");
            if !current.same_scope(handle) {
                return Err(stale_scope());
            }
            // Cancellation must not wait behind the write it may need to stop.
            process
                .commands
                .send(lifecycle::Command::Cancel {
                    handle: handle.clone(),
                    response,
                })
                .map_err(|_| {
                    DriverError::Disconnected("Claude session closed before cancellation".into())
                })?;
        }
        reply
            .await
            .map_err(|_| DriverError::Disconnected("Claude cancellation unconfirmed".into()))?
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        let process = self.process_for(handle).await?;
        let _admission = process.admit(handle).await?;
        let input = {
            let mut questions = process.pending_questions.lock().expect("questions");
            let input = questions
                .get(request_id)
                .ok_or_else(|| DriverError::Protocol("Question expired".into()))?;
            driver_api::questions::native_form("claude", input)
                .and_then(|form| form.validate(&answer))
                .map_err(DriverError::Protocol)?;
            questions.remove(request_id).expect("pending question")
        };
        process
            .agent_interactions
            .lock()
            .expect("agent interactions")
            .remove(request_id);
        process.send_line(&json!({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":driver_api::questions::native_answer("claude",&input,&answer)}})).await
    }

    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        native_request_id: &str,
        option_id: &str,
    ) -> Result<(), DriverError> {
        let process = self.process_for(handle).await?;
        let _admission = process.admit(handle).await?;
        let input = process
            .pending_approvals
            .lock()
            .expect("pending approvals")
            .remove(native_request_id)
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable(
                    "this approval request is no longer pending".into(),
                )
            })?;
        // Only the two protocol behaviors are answerable; anything else is a
        // hub bug and must not reach the agent.
        let response = match option_id {
            "allow" => json!({"behavior": "allow", "updatedInput": input}),
            "deny" => json!({
                "behavior": "deny",
                "message": "denied by the operator in dog.codes",
            }),
            _ => {
                // Put the request back so a retry stays possible.
                process
                    .pending_approvals
                    .lock()
                    .expect("pending approvals")
                    .insert(native_request_id.to_owned(), input);
                return Err(DriverError::CapabilityUnavailable(
                    "unknown approval option id".into(),
                ));
            }
        };
        process
            .agent_interactions
            .lock()
            .expect("agent interactions")
            .remove(native_request_id);
        process
            .send_line(&json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": native_request_id,
                    "response": response,
                },
            }))
            .await
    }

    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.processes
            .lock()
            .await
            .get(id)
            .map(|process| process.handle())
    }

    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        let processes = self.processes.lock().await;
        if let Some(process) = processes.get(&handle.native_session_id) {
            process.validate_scope(handle).ok()?;
            let original = process.clone();
            return Some(Box::pin(async move { original.retired().await }));
        }
        self.retired_handles
            .lock()
            .expect("Claude retired scopes")
            .get(&handle.native_session_id)
            .filter(|retired| retired.same_scope(handle))
            .map(|_| Box::pin(async {}) as driver_api::SessionShutdownProof)
    }

    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        let process = {
            let processes = self.processes.lock().await;
            if let Some(process) = processes.get(&handle.native_session_id) {
                process.validate_scope(&handle)?;
                process.clone()
            } else {
                return if self
                    .retired_handles
                    .lock()
                    .expect("Claude retired scopes")
                    .get(&handle.native_session_id)
                    .is_some_and(|retired| retired.same_scope(&handle))
                {
                    Ok(())
                } else {
                    Err(stale_scope())
                };
            }
        };
        {
            let current = process.scope.lock().expect("Claude session scope");
            if !current.same_scope(&handle) {
                return Err(stale_scope());
            }
            // Atomically fence reuse, even if a prompt/configuration currently
            // owns admission and is blocked writing to the native stdin pipe.
            process.request_close();
        }
        tokio::time::timeout(std::time::Duration::from_secs(8), process.retired())
            .await
            .map_err(|_| {
                DriverError::Disconnected("Claude process shutdown remains unconfirmed".into())
            })?;
        self.remove_retired(&handle, &process).await;
        Ok(())
    }
}

#[async_trait]
impl QuotaProvider for ClaudeDriver {
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        self.quota.usage.get(Some(id))
    }

    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    async fn read_quota(
        &self,
        native_session_id: Option<&str>,
    ) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota_with_refresh(native_session_id, false).await
    }

    async fn read_quota_with_refresh(
        &self,
        native_session_id: Option<&str>,
        force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        let pulled = {
            let mut cache = self.quota.pull.lock().await;
            let pulled = cache.read(force, self.native_version.as_deref()).await;
            // Account onboarding must not mask a rejected OAuth credential with
            // an earlier stream push. Session context remains independently usable.
            if (force && cache.failure.is_some())
                || (native_session_id.is_none()
                    && cache.failure == Some("claude_quota_login_required"))
            {
                return Err(DriverError::CapabilityUnavailable(
                    cache.failure.unwrap_or("claude_quota_unavailable").into(),
                ));
            }
            // While rate limited the last pull is not current; report the limit so the
            // UI labels its remembered reading as stale instead of presenting it as fresh.
            pulled.filter(|_| !cache.withholds_reading())
        };
        // Claude's stream-json transport has no on-demand quota pull: windows
        // arrive only as `rate_limit_event` pushes during turns (absent for
        // custom-provider backends), and context is observed per result. Both
        // are account/session-scoped observations, honestly reported as such.
        let windows = self
            .quota
            .plan_windows
            .lock()
            .expect("plan windows")
            .clone();
        let context_observation = native_session_id.and_then(|id| {
            self.quota
                .context
                .lock()
                .expect("context usage")
                .get(id)
                .cloned()
        });
        let context = context_observation
            .as_ref()
            .map(|(_, context)| context.clone());
        if let Some(mut snapshot) = pulled.filter(|snapshot| {
            windows.is_empty()
                || snapshot.collected_at_unix_ms
                    >= self.quota.windows_at_unix_ms.load(Ordering::Acquire)
        }) {
            snapshot.context = context;
            return Ok(snapshot);
        }
        if windows.is_empty() && context.is_none() {
            return Err(DriverError::CapabilityUnavailable(
                self.quota
                    .pull
                    .lock()
                    .await
                    .failure
                    .unwrap_or("no claude quota observation has arrived yet")
                    .into(),
            ));
        }
        let collected_at_unix_ms = self.quota.windows_at_unix_ms.load(Ordering::Acquire);
        let collected_at_unix_ms = if windows.is_empty() {
            context_observation.map_or(collected_at_unix_ms, |(at, _)| at)
        } else {
            collected_at_unix_ms
        };
        Ok(QuotaSnapshot {
            session_usage: None,
            windows,
            context,
            collected_at_unix_ms,
            stale_after_unix_ms: collected_at_unix_ms.saturating_add(CLAUDE_QUOTA_STALE_MS),
            plan_type: None,
            source: "native_push".into(),
            confidence: "provider_reported".into(),
            parser_version: "claude-rate-limit-event-v1".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_the_account_name_so_claude_finds_its_keychain_login() {
        // Without USER, Claude Code on macOS reports "Not logged in" and never renews its token.
        assert!(super::ENV_ALLOWLIST.contains(&"USER"));
    }

    #[test]
    fn approval_display_uses_session_relative_path_without_changing_native_input() {
        let cwd = std::path::Path::new("/tmp/dogcodes-review-acceptance-BABdAy/live-project");
        let input = serde_json::json!({"file_path": cwd.join("sample.txt"), "content": "hello\n"});
        let summary: serde_json::Value =
            serde_json::from_str(&super::approval_summary("Write", &input, cwd)).unwrap();
        assert_eq!(summary["file_path"], "sample.txt");
        assert_eq!(summary["content"], input["content"]);
        assert_eq!(input["file_path"], cwd.join("sample.txt").to_str().unwrap());
        for path in [
            "/tmp/elsewhere/sample.txt",
            "/tmp/dogcodes-review-acceptance-BABdAy/live-project/../secret",
            "sample.txt",
        ] {
            let input = serde_json::json!({"file_path": path});
            let summary: serde_json::Value =
                serde_json::from_str(&super::approval_summary("Write", &input, cwd)).unwrap();
            assert_eq!(summary, input);
        }
        assert_eq!(
            super::approval_summary("Bash", &input, cwd),
            input.to_string()
        );
    }

    use super::*;
    #[cfg(unix)]
    use driver_api::ContextDeliveryKind;

    fn test_driver(executable: PathBuf) -> ClaudeDriver {
        let driver = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            Some("2.1.218".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        // Process fixtures must not query the developer's real account.
        driver.quota.pull.try_lock().unwrap().next_attempt = i64::MAX;
        driver
    }

    #[test]
    fn classifies_the_documented_stream_shapes() {
        assert!(matches!(
            classify(r#"{"type":"system","subtype":"init","session_id":"abc"}"#),
            Some(Classified::Ignored)
        ));
        match classify(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#,
        ) {
            Some(Classified::Frames(frames)) => {
                assert!(
                    matches!(frames[0], DriverEvent::TextDelta { ref text, .. } if text == "hi")
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            classify(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#,
            ),
            Some(Classified::Frames(_))
        ));
        assert!(matches!(
            classify(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#,
            ),
            Some(Classified::Frames(_))
        ));
        assert!(matches!(
            classify(r#"{"type":"result","subtype":"success","result":"done"}"#),
            Some(Classified::Terminal(DriverEvent::TurnCompleted { .. }, _))
        ));
        assert!(matches!(
            classify(r#"{"type":"result","subtype":"error_during_execution"}"#),
            Some(Classified::Terminal(DriverEvent::TurnFailed { .. }, _))
        ));
    }

    #[test]
    fn rate_limit_events_parse_both_windows_from_a_fraction() {
        // Real shape (codex-host verified): utilization is a 0-1 fraction.
        let event = r#"{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.42,"resetsAt":1788696164},"seven_day":{"utilization":0.18,"resetsAt":1789128164}}}}"#;
        match classify(event) {
            Some(Classified::PlanLimit(windows)) => {
                assert_eq!(windows.len(), 2);
                assert_eq!(windows[0].window_minutes, 300);
                assert_eq!(windows[0].used_percent, Some(42.0));
                assert_eq!(windows[0].resets_at_unix, Some(1_788_696_164));
                assert_eq!(windows[1].window_minutes, 10_080);
                assert_eq!(windows[1].used_percent, Some(18.0));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn explicit_usage_exhaustion_is_a_structured_terminal() {
        assert!(matches!(
            classify(
                r#"{"type":"result","subtype":"error_during_execution","error":"Usage limit reached"}"#
            ),
            Some(Classified::Terminal(
                DriverEvent::TurnQuotaExhausted { .. },
                _
            ))
        ));
        assert!(matches!(
            classify(
                r#"{"type":"result","subtype":"error_during_execution","error":"tool failed","result":"user text: quota exceeded"}"#
            ),
            Some(Classified::Terminal(DriverEvent::TurnFailed { .. }, _))
        ));
    }

    #[test]
    fn rate_limit_flat_fallback_and_percent_values_clamp() {
        let flat = r#"{"type":"rate_limit_event","rate_limit_info":{"rateLimitType":"five_hour","utilization":35.0,"resetsAt":1788696164}}"#;
        match classify(flat) {
            Some(Classified::PlanLimit(windows)) => {
                assert_eq!(windows.len(), 1);
                assert_eq!(windows[0].window_minutes, 300);
                assert_eq!(windows[0].used_percent, Some(35.0));
            }
            other => panic!("unexpected: {other:?}"),
        }
        // >1 values are treated as percents, not rejected.
        let percent = r#"{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"seven_day":{"utilization":83.7}}}}"#;
        match classify(percent) {
            Some(Classified::PlanLimit(windows)) => {
                assert_eq!(windows[0].used_percent, Some(83.7));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn result_events_carry_context_usage() {
        let result = r#"{"type":"result","subtype":"success","usage":{"input_tokens":228,"cache_read_input_tokens":28160,"cache_creation_input_tokens":0,"output_tokens":23},"modelUsage":{"claude-opus-4-8[1m]":{"contextWindow":1000000}}}"#;
        match classify(result) {
            Some(Classified::Terminal(_, Some(context))) => {
                assert_eq!(context.used_tokens, 28_388);
                assert_eq!(context.window_tokens, 1_000_000);
                let rate = context.cache_hit_rate_percent.unwrap();
                assert!((rate - 99.2).abs() < 0.1, "cache hit rate: {rate}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejected_login_is_not_hidden_by_previous_account_push() {
        let driver = test_driver(PathBuf::from("/unused"));
        driver.quota.pull.lock().await.failure = Some("claude_quota_login_required");
        driver.quota.plan_windows.lock().unwrap().push(QuotaWindow {
            id: "five_hour".into(),
            scope: None,
            window_minutes: 300,
            used_percent: Some(20.0),
            resets_at_unix: None,
        });
        assert!(matches!(driver.read_quota(None).await,
            Err(DriverError::CapabilityUnavailable(reason)) if reason == "claude_quota_login_required"));
    }

    #[tokio::test]
    async fn quota_context_is_session_scoped_and_staleness_uses_observation_time() {
        let driver = test_driver(PathBuf::from("/unused"));
        driver.quota.context.lock().unwrap().insert(
            "a".into(),
            (
                123,
                ContextUsageSnapshot {
                    used_tokens: 100,
                    window_tokens: 1000,
                    cache_hit_rate_percent: Some(50.0),
                },
            ),
        );
        assert!(driver.read_quota(Some("b")).await.is_err());
        assert!(driver.read_quota(None).await.is_err());
        let first = driver.read_quota(Some("a")).await.unwrap();
        let second = driver.read_quota(Some("a")).await.unwrap();
        assert_eq!(first.context.unwrap().used_tokens, 100);
        assert_eq!(first.collected_at_unix_ms, 123);
        assert_eq!(first.stale_after_unix_ms, 123 + CLAUDE_QUOTA_STALE_MS);
        assert_eq!(second.stale_after_unix_ms, first.stale_after_unix_ms);
    }

    #[test]
    fn thinking_and_unknown_events_never_reach_the_timeline() {
        assert!(matches!(
            classify(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"secret reasoning"}}}"#,
            ),
            Some(Classified::Ignored)
        ));
        assert!(matches!(
            classify(r#"{"type":"vendor_something","payload":{}}"#),
            Some(Classified::Ignored)
        ));
        assert!(classify("not json at all").is_none());
    }

    #[test]
    fn tool_permission_requests_are_matched_for_deny() {
        match classify(
            r#"{"type":"control_request","request_id":"req-9","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ) {
            Some(Classified::CanUseTool {
                request_id,
                tool_name,
                ..
            }) => {
                assert_eq!(request_id, "req-9");
                assert_eq!(tool_name, "Bash");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconfiguration_updates_a_warm_process_and_waits_for_matching_ack() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("claude-config");
        std::fs::write(&executable, r"#!/usr/bin/python3
import sys,json,os
state={'model':'old-model','effortLevel':'high','alwaysThinkingEnabled':None}
for line in sys.stdin:
    req=json.loads(line)
    if req['request']['subtype']=='get_settings':
        value={'effective':state,'applied':{'model':state.get('model') or 'old-model','effort':state.get('effortLevel') or 'high'}}
        print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':req['request_id'],'response':value}}),flush=True)
        continue
    settings=req['request']['settings']
    if settings.get('model')=='close': sys.exit(0)
    with open(sys.argv[0]+'.settings','a') as log:
        log.write(json.dumps({'pid':os.getpid(),'settings':settings})+'\n')
    if settings.get('model') not in ['reject','ignored']: state.update(settings)
    print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':'unrelated'}}),flush=True)
    print(json.dumps({'type':'control_response','response':{'subtype':'error' if settings.get('model')=='reject' else 'success','request_id':req['request_id']}}),flush=True)
").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = test_driver(executable.clone());
        let request = || SessionRequest {
            history_mcp: None,
            permission_mode: None,
            additional_directories: vec![],
            working_directory: "/tmp".into(),
        };
        let first = driver.create_session(request()).await.unwrap();
        let original = driver.processes.lock().await[&first.native_session_id].clone();
        for (model, thinking) in [
            (Some("new-model"), Some("high")),
            (Some("new-model"), Some("off")),
            (None, None),
        ] {
            let updated = driver
                .reconfigure_session(&first.native_session_id, request(), model, thinking)
                .await
                .unwrap();
            assert_eq!(updated.native_session_id, first.native_session_id);
            assert!(Arc::ptr_eq(
                &original,
                &driver.processes.lock().await[&first.native_session_id]
            ));
        }
        let records: Vec<Value> = std::fs::read_to_string(executable.with_extension("settings"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert!(
            records
                .iter()
                .all(|record| record["pid"] == records[0]["pid"])
        );
        assert_eq!(
            records[0]["settings"],
            json!({"model":"new-model","effortLevel":"high","alwaysThinkingEnabled":true})
        );
        assert_eq!(records[1]["settings"]["alwaysThinkingEnabled"], false);
        assert_eq!(
            records[2]["settings"],
            json!({"model":null,"effortLevel":null,"alwaysThinkingEnabled":null})
        );
        assert!(matches!(
            driver
                .reconfigure_session(&first.native_session_id, request(), Some("reject"), None)
                .await,
            Err(DriverError::Protocol(_))
        ));
        assert_eq!(
            driver
                .reconfigure_session(
                    &first.native_session_id,
                    request(),
                    Some("ignored"),
                    Some("low")
                )
                .await
                .unwrap_err(),
            DriverError::Protocol("selection_rejected".into())
        );
        assert!(!original.closing.load(Ordering::Acquire));
        assert!(matches!(
            driver
                .reconfigure_session(&first.native_session_id, request(), Some("close"), None)
                .await,
            Err(DriverError::Protocol(_))
        ));
        assert!(original.pending_controls.lock().unwrap().is_empty());
        let current = driver
            .current_session_handle(&first.native_session_id)
            .await
            .unwrap();
        driver.shutdown(current).await.unwrap();
    }

    #[cfg(unix)]
    fn write_fake_claude(directory: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = directory.join("claude");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"dogcodes-probe","response":{"commands":[]}}}'
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"streamed"}}}'
      printf '%s\n' '{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}'
      # wait for the approval control response
      while IFS= read -r approval; do
        case "$approval" in
          *'"behavior":"allow"'*)
            printf '%s\n' '{"type":"assistant","message":{"model":"test-model","usage":{"input_tokens":20,"cache_read_input_tokens":80},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}'
            printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}'
            printf '%s\n' '{"type":"result","subtype":"success","result":"approved","usage":{"input_tokens":9999},"modelUsage":{"test-model":{"contextWindow":200000}}}'
            break
            ;;
          *'"behavior":"deny"'*)
            printf '%s\n' '{"type":"result","subtype":"error_during_execution"}'
            break
            ;;
        esac
      done
      ;;
    *'"subtype":"interrupt"'*)
      printf '%s\n' '{"type":"result","subtype":"success"}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[test]
    fn success_subtype_with_error_flag_is_not_successful_completion() {
        let result = classify(
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
        );
        assert!(matches!(
            result,
            Some(Classified::Terminal(DriverEvent::TurnFailed { .. }, _))
        ));
    }

    #[test]
    fn authentication_failure_is_actionable_without_exposing_native_details() {
        let message = claude_result_failure(&json!({"error_code":"authentication_failed"}));
        assert!(message.contains("身份验证失败"));
        for event in [
            json!({"result":"Failed to authenticate. API Error: 403 status code (no body)"}),
            json!({"errors":["invalid API key: private-credential"]}),
            json!({"result":"other error"}),
        ] {
            let result = claude_result_failure(&event);
            assert!(result.contains("error result"));
            assert!(!result.contains("private-credential"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn analysis_allows_read_tools_without_mcp_or_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude-analysis");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" > argv.tmp\n/bin/mv argv.tmp argv.txt\nwhile IFS= read -r line; do :; done\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = test_driver(script);
        assert!(driver.supports_readonly_analysis());
        let extra = tempfile::tempdir().unwrap();
        let session = driver
            .create_analysis_session_with_directories(
                dir.path().to_str().unwrap(),
                &[extra.path().to_string_lossy().into_owned()],
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !dir.path().join("argv.txt").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let args = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
        assert!(args.contains("--tools\nRead,Glob,Grep,Bash\n"));
        assert!(args.contains("--strict-mcp-config\n--mcp-config\n{\"mcpServers\":{}}"));
        assert!(args.contains("--setting-sources\nuser\n"));
        assert!(args.contains("{\"disableAllHooks\":true}"));
        assert!(args.contains("--permission-mode\nplan"));
        assert!(args.contains(extra.path().to_str().unwrap()));
        driver.shutdown(session).await.unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn review_uses_bypass_permissions_and_passes_additional_directories() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude-analysis");
        std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" > argv.tmp\n/bin/mv argv.tmp argv.txt\nwhile IFS= read -r line; do :; done\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = test_driver(script);
        assert!(driver.supports_readonly_analysis());
        let extra = tempfile::tempdir().unwrap();
        let session = driver
            .create_review_session(
                dir.path().to_str().unwrap(),
                &[extra.path().to_string_lossy().into_owned()],
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !dir.path().join("argv.txt").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let args = std::fs::read_to_string(dir.path().join("argv.txt")).unwrap();
        assert!(args.contains("--tools\nRead,Glob,Grep,Bash\n"));
        assert!(args.contains("--strict-mcp-config\n--mcp-config\n{\"mcpServers\":{}}"));
        assert!(args.contains("--setting-sources\nuser\n"));
        assert!(args.contains("{\"disableAllHooks\":true}"));
        assert!(args.contains("--permission-mode\nbypassPermissions"));
        assert!(args.contains(extra.path().to_str().unwrap()));
        driver.shutdown(session).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_ready_accepts_a_compliant_fake_cli() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_claude(directory.path());
        ClaudeDriver::probe_ready(&executable)
            .await
            .expect("fake cli should pass the control-plane probe");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_ready_rejects_a_cli_without_a_control_plane() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("claude-broken");
        std::fs::write(&script, "#!/bin/sh\nexec sleep 60\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(ClaudeDriver::probe_ready(&script).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn full_turn_streams_deltas_tools_and_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_claude(directory.path());
        let driver = test_driver(executable);
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        let mut stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "do the thing".into(),
                    working_directory: "/tmp".into(),
                    context: vec![ContextDelivery {
                        artifact_id: agent_domain::ArtifactId::new(),
                        kind: ContextDeliveryKind::Handoff,
                        payload_sha256: "abc".into(),
                        content: b"{}".to_vec(),
                    }],
                },
            )
            .await
            .unwrap();
        let mut events: Vec<DriverEvent> = Vec::new();
        let mut answered = false;
        // The fake CLI asks for a tool approval mid-turn; answer it the way
        // the UI would, then keep collecting until the turn completes.
        while events
            .iter()
            .all(|event| !matches!(event, DriverEvent::TurnCompleted { .. }))
        {
            let event = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                futures::StreamExt::next(&mut stream),
            )
            .await
            {
                Ok(Some(Ok(event))) => event,
                other => panic!("stream ended unexpectedly: {other:?}"),
            };
            let request_id = match &event {
                DriverEvent::ApprovalRequested {
                    native_request_id, ..
                } => Some(native_request_id.clone()),
                _ => None,
            };
            events.push(event);
            if let Some(request_id) = request_id {
                driver
                    .respond_to_interaction(&session, &request_id, "allow")
                    .await
                    .expect("allow must reach the control plane");
                answered = true;
            }
        }
        assert!(answered);
        let quota = driver
            .read_quota(Some(&session.native_session_id))
            .await
            .unwrap();
        let context = quota.context.unwrap();
        assert_eq!(
            context.used_tokens, 100,
            "use last request, not accumulated result usage"
        );
        assert_eq!(context.cache_hit_rate_percent, Some(80.0));
        assert!(driver.read_quota(Some("other-session")).await.is_err());
        assert!(events.iter().any(
            |event| matches!(event, DriverEvent::TextDelta { text, .. } if text == "streamed")
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::ApprovalRequested { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::ToolStarted { tool_name, .. } if tool_name == "Bash"))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::ToolCompleted { success: true, .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_approval_surfaces_then_allow_completes_the_turn() {
        use futures::StreamExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_claude(directory.path());
        let driver = test_driver(executable);
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        let mut stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "run a tool".into(),
                    working_directory: "/tmp".into(),
                    context: vec![],
                },
            )
            .await
            .unwrap();

        // First meaningful frame is the vendor permission request.
        let first = loop {
            let event = stream.next().await.unwrap().unwrap();
            if !matches!(
                event,
                DriverEvent::TextDelta { .. } | DriverEvent::FirstOutput
            ) {
                break event;
            }
        };
        let DriverEvent::ApprovalRequested {
            native_request_id,
            tool_name,
            command_summary,
            options,
        } = first
        else {
            panic!("expected an approval request, got {first:?}");
        };
        assert_eq!(native_request_id, "perm-1");
        assert_eq!(tool_name, "Bash");
        assert!(command_summary.contains("ls"));
        assert_eq!(options.len(), 2);
        assert!(options.iter().any(|option| option.option_id == "allow"));

        // Only vendor option ids are answerable.
        assert!(
            driver
                .respond_to_interaction(&session, &native_request_id, "bogus")
                .await
                .is_err()
        );
        driver
            .respond_to_interaction(&session, &native_request_id, "allow")
            .await
            .expect("allow must reach the control plane");

        let rest: Vec<DriverEvent> = stream.map(|event| event.unwrap()).collect().await;
        assert!(
            rest.iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        // The answered request is no longer pending.
        assert!(
            driver
                .respond_to_interaction(&session, &native_request_id, "allow")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interrupt_flips_the_terminal_to_interrupted() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("claude-hold");
        // The fake emits the streaming delta for the prompt, then says nothing
        // until an interrupt control request arrives, which it answers with
        // the success result the real CLI sends after a cancellation.
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"dogcodes-probe"}}'
      ;;
    *'"type":"user"'*)
      printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}'
      ;;
    *'"subtype":"interrupt"'*)
      printf '%s\n' '{"type":"result","subtype":"success"}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let driver = test_driver(script);
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        let stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "long turn".into(),
                    working_directory: "/tmp".into(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        driver.interrupt(&session).await.unwrap();
        let events: Vec<DriverEvent> = stream.map(|event| event.unwrap()).collect().await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. }))
        );
    }
}

/// Real-installation probe; runs only with `--ignored` and consumes no model
/// quota (control-plane handshake only).
#[cfg(test)]
mod real_installations {
    use super::*;

    #[tokio::test]
    #[ignore = "requires the local Claude Code installation; run with --ignored"]
    async fn probes_the_current_users_claude_installation() {
        let home = std::env::var("HOME").expect("HOME");
        let executable = PathBuf::from(format!("{home}/.local/bin/claude"));
        match ClaudeDriver::probe_ready(&executable).await {
            Ok(()) => println!("claude-code: stream-json control plane ready"),
            Err(error) => panic!("claude-code: probe failed: {error}"),
        }
    }
}

/// Real-turn acceptance: consumes real model quota and requires the operator's
/// explicit authorization. Fixed no-tool prompts only; asserts streaming,
/// turn completion, live session reuse and interrupt handling.
#[cfg(test)]
mod real_turn_acceptance {
    use super::*;
    use driver_api::SessionRequest;

    const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(3);

    fn test_driver(executable: PathBuf) -> ClaudeDriver {
        ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            Some("2.1.218".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap()
    }

    async fn collect_turn(
        driver: &ClaudeDriver,
        session: &SessionHandle,
        prompt: &str,
    ) -> Vec<DriverEvent> {
        let stream = driver
            .prompt(
                session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: prompt.to_owned(),
                    working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .expect("turn must be accepted");
        tokio::time::timeout(
            TURN_TIMEOUT,
            stream.map(|event| event.unwrap()).collect::<Vec<_>>(),
        )
        .await
        .expect("turn must finish in time")
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_streams_reuses_and_interrupts() {
        let home = std::env::var("HOME").expect("HOME");
        let executable = PathBuf::from(format!("{home}/.local/bin/claude"));
        ClaudeDriver::probe_ready(&executable)
            .await
            .expect("claude must pass the control-plane probe");
        let driver = test_driver(executable);
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            })
            .await
            .expect("session must be created");

        let first = collect_turn(&driver, &session, "Reply with exactly: OK").await;
        assert!(
            first.iter().any(|event| matches!(
                event,
                DriverEvent::TextDelta { text, .. } if !text.is_empty()
            )),
            "turn 1 must stream text"
        );
        assert!(
            first
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 1 must complete"
        );
        assert!(
            !first
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnFailed { .. })),
            "turn 1 must not fail: {first:?}"
        );
        println!("turn 1 (stream + complete): ok");

        let second = collect_turn(&driver, &session, "Reply with exactly: DONE").await;
        assert!(
            second
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 2 must complete on the same live session"
        );
        println!("turn 2 (live session reuse): ok");

        let stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Count from 1 to 20 slowly, one number per line.".into(),
                    working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .expect("turn 3 must be accepted");
        let collector = tokio::spawn(async move {
            tokio::time::timeout(
                TURN_TIMEOUT,
                stream.map(|event| event.unwrap()).collect::<Vec<_>>(),
            )
            .await
            .expect("turn 3 must finish in time")
        });
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        driver
            .interrupt(&session)
            .await
            .expect("interrupt must send");
        let third = collector.await.unwrap();
        assert!(
            third
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. })),
            "turn 3 must end interrupted: {third:?}"
        );
        println!("turn 3 (interrupt): ok");
    }
}

/// Real approval acceptance: deny and allow the same fixed Bash command under
/// `--permission-mode manual`, with filesystem outcomes confined to temp dirs.
/// Consumes two real model turns.
#[cfg(test)]
mod real_approval_acceptance {
    use super::*;
    use driver_api::SessionRequest;
    use futures::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_approval_deny_then_allow_only_in_temporary_workspace() {
        let home = std::env::var("HOME").expect("HOME");
        let executable = PathBuf::from(format!("{home}/.local/bin/claude"));
        ClaudeDriver::probe_ready(&executable)
            .await
            .expect("probe must pass");
        for allow in [false, true] {
            let workspace = tempfile::tempdir().unwrap();
            let cwd = workspace.path().to_string_lossy().into_owned();
            let driver = ClaudeDriver::new(
                "claude-code",
                &agent_domain::InstallationId::new().to_string(),
                executable.clone(),
                Some("2.1.218".into()),
                SessionResumeCapability::Unknown,
            )
            .unwrap();
            let session = driver
                .create_session(SessionRequest {
                    history_mcp: None,
                    permission_mode: None,
                    additional_directories: vec![],
                    working_directory: cwd.clone(),
                })
                .await
                .unwrap();
            let mut stream = driver
                .prompt(
                    &session,
                    HubPromptRequest {
                                                attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: "Approval integration test. Use the Bash tool to run exactly: printf DOGCODES_APPROVAL_OK > approval.txt . Request approval. Do not use other tools or access other paths. If denied, stop and report denied."
                            .into(),
                        working_directory: cwd,
                        context: vec![],
                    },
                )
                .await
                .unwrap();

            let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), async {
                let mut approvals = 0;
                let mut allowed = 0;
                let mut completed = false;
                while let Some(event) = stream.next().await {
                    match event.unwrap() {
                        DriverEvent::ApprovalRequested {
                            native_request_id,
                            tool_name,
                            command_summary,
                            ..
                        } => {
                            approvals += 1;
                            let raw =
                                serde_json::from_str::<Value>(&command_summary).unwrap_or_default();
                            let command = raw
                                .get("command")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let safe = tool_name == "Bash"
                                && matches!(
                                    command.trim(),
                                    "printf DOGCODES_APPROVAL_OK > approval.txt"
                                        | "printf 'DOGCODES_APPROVAL_OK' > approval.txt"
                                        | "printf '%s' DOGCODES_APPROVAL_OK > approval.txt"
                                );
                            let option = if allow && safe {
                                allowed += 1;
                                "allow"
                            } else {
                                "deny"
                            };
                            driver
                                .respond_to_interaction(&session, &native_request_id, option)
                                .await
                                .expect("approval response must reach the control plane");
                        }
                        DriverEvent::TurnCompleted { .. } => completed = true,
                        DriverEvent::TurnFailed { .. } | DriverEvent::TurnQuotaExhausted { .. } => {
                            return (approvals, allowed, false);
                        }
                        _ => {}
                    }
                }
                (approvals, allowed, completed)
            })
            .await;
            driver.shutdown(session).await.unwrap();
            let (approvals, allowed, completed) = outcome.expect("approval turn timed out");
            assert!(approvals > 0, "Claude did not request approval");
            assert!(completed, "Claude approval turn did not complete");
            let target = workspace.path().join("approval.txt");
            if allow {
                assert_eq!(allowed, 1, "safe allow was not sent to Claude");
                assert_eq!(
                    std::fs::read_to_string(target).unwrap(),
                    "DOGCODES_APPROVAL_OK"
                );
            } else {
                assert!(!target.exists(), "denied Claude tool wrote the file");
            }
            println!("claude: allow={allow}, approval and filesystem outcome verified");
        }
    }
}

/// Cross-process resume acceptance: turn 1 in one driver instance, then a
/// brand-new instance must re-attach via `--resume` and complete turn 2.
#[cfg(test)]
mod real_resume_acceptance {
    use super::*;
    use driver_api::SessionRequest;
    use futures::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_reattaches_across_driver_instances() {
        let home = std::env::var("HOME").expect("HOME");
        let executable = PathBuf::from(format!("{home}/.local/bin/claude"));
        let workspace = tempfile::tempdir().unwrap();
        let first = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable.clone(),
            Some("2.1.218".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let session = first
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: workspace.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let native_session_id = session.native_session_id.clone();
        let stream = first
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Remember the word: dogcodes. Reply with exactly: remembered".into(),
                    working_directory: workspace.path().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let events: Vec<DriverEvent> = stream.map(|event| event.unwrap()).collect().await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        first.shutdown(session).await.unwrap();

        // A fresh instance (as after an app restart) re-attaches to the
        // recorded transcript via --resume and answers in the same session.
        let second = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            Some("2.1.218".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let resumed = second
            .resume_session(
                &native_session_id,
                workspace.path().to_string_lossy().as_ref(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.native_session_id, native_session_id);
        let stream = second
            .prompt(
                &resumed,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "What word did I ask you to remember? Reply with exactly that word."
                        .into(),
                    working_directory: workspace.path().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let events: Vec<DriverEvent> = stream.map(|event| event.unwrap()).collect().await;
        let text = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::TextDelta { text, .. } => Some(text.to_ascii_lowercase()),
                _ => None,
            })
            .collect::<String>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 2 events: {events:?}"
        );
        assert!(
            text.contains("dogcodes"),
            "resume must restore context: {text}"
        );
        println!("real resume (new instance reattaches and recalls context): ok");
    }
}
