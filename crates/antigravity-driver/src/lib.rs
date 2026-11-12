//! Antigravity CLI (`agy`) headless stream-json product driver.
//!
//! Drives the user-installed `agy` over its documented headless protocol
//! (`--input-format stream-json --output-format stream-json`), following the
//! official spec at antigravity.google/docs/cli/headless: one NDJSON line per
//! message, `{"event":"user"}` prompts on stdin, and `init` / `step_update` /
//! `result` events on stdout. One process owns one conversation; stdin stays
//! open so multi-turn runs share a single `conversation_id`.
//!
//! Native headless permission prompts are denied by configured rules. The
//! opt-in desktop policy uses a verified `PreToolUse` Hook and explicit per-call
//! decisions; it is not native stdin approval support. Resume capability is
//! recorded separately from permission support.

use agent_runtime::{Pipe, ProcessInput, TargetExecutable};
#[cfg(test)]
use std::path::PathBuf;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
// Only the Unix fixtures start processes with piped stdio.
#[cfg(all(test, unix))]
use std::process::Stdio;

use agent_domain::{ExecutorRef, InstallationId, ProviderId};
use agent_runtime::OwnedChild as Child;
use async_trait::async_trait;
use driver_api::{
    AgentDriver, ContextDelivery, ContextDeliveryKind, DriverError, DriverEvent, DriverEventStream,
    PromptRequest as HubPromptRequest, SessionHandle, SessionRequest, SessionResumeCapability,
    explicitly_reports_quota_exhaustion,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
pub mod approval_bridge;
mod catalog;
mod quota;
mod scratch;
mod session_open;
mod session_usage;
mod sessions;
use sessions::{Startup, stale_scope};

/// Per-line frame cap; `tool_info` payloads can be large but never unbounded.
const FRAME_LIMIT: usize = 2 * 1024 * 1024;
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// agy persists its active account in shared state (a token file plus the OS
/// keyring) and every process refreshes it on startup. Concurrent spawns race
/// on that single file and a refresh interrupted mid-write leaves the CLI
/// "not authenticated", whose silent fallback then adopts whatever account the
/// keyring holds — an account switch with no prompt (2026-09 incident). agy
/// 1.5.3 authenticates from the keychain instead, and six simultaneous token
/// refreshes kept one valid account (2026-11-09). Session processes therefore
/// start and run in parallel; short helpers stay serialized with each other.
/// Probes back off after auth failures instead of hammering the chain.
///
/// That shared state belongs to a machine, so each machine has its own of both.
struct Account {
    spawn: SpawnLock,
    backoff: Backoff,
}

fn account_of(target: &agent_runtime::SharedTarget) -> Arc<Account> {
    type Known = Vec<(
        std::sync::Weak<dyn agent_runtime::ExecutionTarget>,
        Arc<Account>,
    )>;
    static ACCOUNTS: std::sync::Mutex<Known> = std::sync::Mutex::new(Vec::new());
    let mut known = ACCOUNTS.lock().expect("agy accounts");
    // A closed connection's machine is met again as a new target.
    known.retain(|(machine, _)| machine.strong_count() > 0);
    if let Some((_, account)) = known.iter().find(|(machine, _)| {
        machine
            .upgrade()
            .is_some_and(|machine| Arc::ptr_eq(&machine, target))
    }) {
        return account.clone();
    }
    let account = Arc::new(Account {
        spawn: SpawnLock::default(),
        backoff: Backoff::default(),
    });
    known.push((Arc::downgrade(target), account.clone()));
    account
}

/// Cooperative backoff after auth/eligibility failures, so a broken account
/// state is not re-probed in a tight loop (each attempt touches the shared
/// credential chain).
#[derive(Default)]
struct AuthBackoffState {
    failures: u32,
    last_failure: Option<std::time::Instant>,
    kind: Option<&'static str>,
}

impl AuthBackoffState {
    fn active_kind(&self) -> Option<&'static str> {
        self.last_failure
            .filter(|last| self.failures > 0 && last.elapsed() < backoff_duration(self.failures))
            .and(self.kind)
    }
}

type Backoff = std::sync::Arc<tokio::sync::Mutex<AuthBackoffState>>;

const AGY_BACKOFF_MAX: u32 = 4;

type SpawnLock = std::sync::Arc<tokio::sync::Mutex<()>>;

async fn acquire_spawn_slot(lock: &SpawnLock) -> tokio::sync::OwnedMutexGuard<()> {
    lock.clone().lock_owned().await
}

/// Holds the spawn guard for a short agy helper (readiness probe, hook check,
/// model catalog, quota) until that process has exited. Session processes do
/// not take it: they run in parallel (see `Account`).
struct SpawnSlot {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

async fn enter_spawn_slot(lock: &SpawnLock) -> Result<SpawnSlot, DriverError> {
    let started = std::time::Instant::now();
    let guard = tokio::time::timeout(std::time::Duration::from_secs(8), acquire_spawn_slot(lock))
        .await
        .map_err(|_| {
            DriverError::CapabilityUnavailable(
                "backendText.mfa29bf87".into(),
            )
        })?;
    eprintln!(
        "antigravity timing stage=spawn_lock elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(SpawnSlot { _guard: guard })
}

/// Preserve the account diagnostic while suppressing repeated native probes.
async fn auth_backoff_kind(backoff: &Backoff) -> Option<&'static str> {
    backoff.lock().await.active_kind()
}

fn backoff_duration(failures: u32) -> std::time::Duration {
    std::time::Duration::from_secs(60_u64.saturating_pow(failures.min(AGY_BACKOFF_MAX)))
}

async fn record_auth_failure(backoff: &Backoff, kind: &'static str) {
    let mut state = backoff.lock().await;
    state.failures = (state.failures + 1).min(AGY_BACKOFF_MAX + 1);
    state.last_failure = Some(std::time::Instant::now());
    state.kind = Some(kind);
}

async fn clear_auth_failures(backoff: &Backoff) {
    *backoff.lock().await = AuthBackoffState::default();
}

/// Environment variables handed to the CLI; everything else is stripped with
/// `env_clear` so credentials never leak into the agent process.
const ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "LANG", "TZ", "TMPDIR", "TERM"];

/// Proxies set on another machine describe its network. This machine's own are applied
/// when a process starts here, and never cross over.
const REMOTE_ENV_ALLOWLIST: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
];

/// What every `agy` process is given, from the machine it runs on.
fn prepare_command(command: &mut agent_runtime::TargetCommand, remote: bool) {
    // This alone doesn't stop print mode from opening a browser: see `prepare_print_command`.
    command.env("AGY_CLI_NONINTERACTIVE_HEADLESS", "1");
    // The native updater starts another agy.exe without our Windows creation
    // flags. Embedded calls must not launch it or replace the executable in use.
    // agy requires the literal "true" here, not "1".
    command.env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
    for variable in ENV_ALLOWLIST {
        command.inherit(variable);
    }
    if remote {
        for variable in REMOTE_ENV_ALLOWLIST {
            command.inherit(variable);
        }
    }
}

/// Print mode answers a failed silent login with the interactive one, whatever it's told,
/// and a token refresh failing on a network that's still waking up is enough for that. So a
/// Google sign-in page would open on its own while nobody is there. It finds the program
/// that opens one through `PATH`, and nothing a print command does needs anything else there.
const NOTHING_TO_RUN: &str = "/var/empty";

/// For a command nobody asked to see, like reading quota. Conversations keep their `PATH`,
/// as their tools need it. A failed login then ends as an error, for signing in by hand.
fn prepare_print_command(command: &mut agent_runtime::TargetCommand, remote: bool) {
    prepare_command(command, remote);
    if cfg!(unix) {
        command.env("PATH", NOTHING_TO_RUN);
    }
}

pub struct AntigravityLaunch {
    pub executable: TargetExecutable,
}

/// Protocol arguments the driver owns so manifests cannot widen permissions.
fn session_args() -> Vec<String> {
    [
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Debug)]
enum Classified {
    /// `init` (or any event carrying a top-level `conversation_id`): the
    /// conversation identity for this process.
    Started { conversation_id: Option<String> },
    /// `step_update` with incremental `text_delta` on an `agent_response` step.
    Text(String),
    /// `step_update` describing a `tool` step transition.
    Tool { name: String, completed: bool },
    /// `result` carrying an account/eligibility error: distinct from a normal
    /// failure so the backoff can engage. Carries the kind and agy's own text.
    AuthFailure(&'static str, String),
    /// `result`: the authoritative per-turn terminal.
    Terminal(DriverEvent),
    /// Anything recognized but not mapped (`subagent_info`, checkpoints).
    Ignored,
}

// The CLI also wraps network failures in "Eligibility check failed". Those
// failures do not establish that credentials or account eligibility are invalid.
fn is_transport_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "tls handshake timeout",
        "context deadline exceeded",
        "i/o timeout",
        "connection reset by peer",
        "connection refused",
        "no such host",
        "network is unreachable",
        "unexpected eof",
    ]
    .iter()
    .any(|message| error.contains(message))
        || error.ends_with(": eof")
}

/// What a process says about an account with no login at all. It can also come from a
/// slow login with a valid one, so on this machine it's left for a real turn to settle.
fn signed_out(line: &str) -> bool {
    serde_json::from_str::<Value>(line).is_ok_and(|event| {
        event["event"] == "result"
            && event["result"]["status"] == "ERROR"
            && event["result"]["error"]
                .as_str()
                .is_some_and(|error| error.contains("authentication failed"))
    })
}

fn classify(line: &str) -> Option<Classified> {
    let event: Value = serde_json::from_str(line).ok()?;
    match event.get("event")?.as_str()? {
        "init" => Some(Classified::Started {
            conversation_id: event
                .get("conversation_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "step_update" => {
            let step = event.get("step_update")?;
            let step_type = step.get("step_type").and_then(Value::as_str);
            let state = step.get("state").and_then(Value::as_str);
            match step_type {
                Some("agent_response") => step
                    .get("text_delta")
                    .and_then(Value::as_str)
                    .map(|text| Classified::Text(text.to_owned()))
                    .or(Some(Classified::Ignored)),
                Some("tool") => Some(Classified::Tool {
                    name: step
                        .get("tool_name")
                        .or_else(|| step.pointer("/tool_info/name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    completed: state == Some("DONE"),
                }),
                // `user_input` echoes the prompt; checkpoint/subagent rows are
                // diagnostics, not timeline content.
                _ => Some(Classified::Ignored),
            }
        }
        "result" => {
            let result = event.get("result")?;
            let status = result.get("status").and_then(Value::as_str);
            let quota_exhausted = result
                .get("error_code")
                .or_else(|| result.get("errorCode"))
                .and_then(Value::as_str)
                .is_some_and(|code| matches!(code, "usage_limit_reached" | "quota_exceeded"))
                || result
                    .get("error")
                    .and_then(Value::as_str)
                    .is_some_and(explicitly_reports_quota_exhaustion);
            if status == Some("ERROR") && quota_exhausted {
                return Some(Classified::Terminal(DriverEvent::TurnQuotaExhausted {
                    resets_at_unix: result
                        .get("resets_at")
                        .or_else(|| result.get("resetsAt"))
                        .and_then(Value::as_i64),
                }));
            }
            // Eligibility/auth failures change the account state; the driver
            // backs off instead of re-hammering the credential chain.
            if status == Some("ERROR")
                && result
                    .get("error")
                    .and_then(Value::as_str)
                    .is_some_and(|error| {
                        !is_transport_error(error)
                            && (error.contains("Eligibility check failed")
                                || error.contains("authentication required")
                                || error.contains("not eligible"))
                    })
            {
                let error = result["error"].as_str().unwrap_or_default();
                return Some(Classified::AuthFailure(
                    if error.contains("authentication required") {
                        "authentication_failed"
                    } else {
                        "access_denied"
                    },
                    error.to_owned(),
                ));
            }
            let error = result
                .get("error")
                .and_then(Value::as_str)
                .filter(|error| !error.trim().is_empty());
            Some(Classified::Terminal(match status {
                Some("SUCCESS" | "WAITING") => DriverEvent::TurnCompleted {
                    summary: "Antigravity turn completed".into(),
                },
                Some("INTERRUPTED" | "CANCELED") => DriverEvent::TurnInterrupted {
                    summary: "Antigravity turn was interrupted".into(),
                },
                // Keep agy's own error text so the user sees what it reported.
                // Transport errors wrapped in "Eligibility check failed" are
                // connection failures, not account rejections.
                _ => DriverEvent::TurnFailed {
                    reason: match error {
                        Some(error) if is_transport_error(error) => {
                            driver_api::failure::with_detail("agent_connection_failed", error)
                        }
                        Some(error) => error.to_owned(),
                        None => "Antigravity turn ended with an error result".into(),
                    },
                },
            }))
        }
        _ => Some(Classified::Ignored),
    }
}

struct PendingQuestion {
    form: agent_domain::QuestionForm,
    sender: tokio::sync::oneshot::Sender<agent_domain::QuestionAnswer>,
}
struct SessionProcess {
    scope: std::sync::Mutex<SessionHandle>,
    turn: std::sync::Mutex<Option<uuid::Uuid>>,
    started: std::sync::Mutex<Option<uuid::Uuid>>,
    model: Option<String>,
    admission: tokio::sync::Mutex<()>,
    usage: std::sync::Mutex<Option<driver_api::usage::SessionUsageSnapshot>>,
    usage_log: Option<scratch::Scratch>,
    target: agent_runtime::SharedTarget,
    backoff: Backoff,
    pending_questions: std::sync::Mutex<HashMap<String, PendingQuestion>>,
    workspace_instruction: String,
    permission_mode: Option<String>,
    approval_children: std::sync::Mutex<std::collections::HashSet<String>>,
    pending_approvals: std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    seen_hooks: std::sync::Mutex<std::collections::HashSet<(String, u64)>>,
    native_id: std::sync::Mutex<Option<String>>,
    /// Keeps the global spawn guard held until this process ends: the auth
    /// refresh races are per-process-lifetime, not per-spawn-call.
    closing: tokio::sync::watch::Sender<bool>,
    cleanup: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
    background: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    lifecycle: std::sync::Mutex<()>,
    stop_requested: AtomicBool,
    input_failed: AtomicBool,
    // Drop the pipe to deliver EOF; AsyncWrite::shutdown alone does not close
    // this handle, leaving the native process and global spawn slot alive.
    stdin: tokio::sync::Mutex<Option<ProcessInput>>,
    updates: std::sync::Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>,
    turn_active: AtomicBool,
    interrupt_requested: AtomicBool,
    finished: Arc<AtomicBool>,
}

struct InputFrame<'a> {
    process: &'a SessionProcess,
    complete: bool,
}
impl Drop for InputFrame<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.process.fail_input();
        }
    }
}

impl SessionProcess {
    fn fail_input(&self) {
        let _lifecycle = self.lifecycle.lock().expect("session lifecycle");
        if !self.stop_requested.load(Ordering::Acquire) {
            self.input_failed.store(true, Ordering::Release);
        }
        self.closing.send_replace(true);
    }
    #[cfg(all(test, unix))]
    async fn close(&self) -> Result<(), DriverError> {
        self.close_if(false).await
    }
    #[cfg(all(test, unix))]
    async fn close_if(&self, only_idle: bool) -> Result<(), DriverError> {
        self.request_close(&self.handle(), only_idle, false)?;
        self.wait_closed().await
    }
    async fn wait_closed(&self) -> Result<(), DriverError> {
        let mut cleanup = self.cleanup.clone();
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if let Some(result) = cleanup.borrow_and_update().clone() {
                    return result.map_err(DriverError::Disconnected);
                }
                cleanup.changed().await.map_err(|_| {
                    DriverError::Disconnected(
                        "Antigravity cleanup actor ended without confirmation".into(),
                    )
                })?;
            }
        })
        .await
        .map_err(|_| DriverError::Disconnected("Antigravity process shutdown timed out".into()))?
    }
    async fn send_line(&self, value: &Value) -> Result<(), DriverError> {
        if *self.closing.borrow() {
            return Err(DriverError::Disconnected(
                "Antigravity session is closing".into(),
            ));
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut stdin = self.stdin.lock().await;
            if *self.closing.borrow() {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            let stdin = stdin
                .as_mut()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            let mut frame = InputFrame {
                process: self,
                complete: false,
            };
            stdin.write_all(value.to_string().as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            frame.complete = true;
            Ok::<_, std::io::Error>(())
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            Ok(())
        } else {
            self.fail_input();
            Err(DriverError::Disconnected(
                "Antigravity input failed; cleanup continues".into(),
            ))
        }
    }
}

pub struct AntigravityDriver {
    admission: tokio::sync::Mutex<()>,
    stopping: tokio::sync::watch::Sender<bool>,
    retired: std::sync::Mutex<HashMap<String, sessions::Retired>>,
    models: tokio::sync::Mutex<Vec<(String, String)>>,
    quota_cache: tokio::sync::Mutex<Option<driver_api::QuotaSnapshot>>,
    executor: ExecutorRef,
    executable: TargetExecutable,
    native_version: Option<String>,
    resume_capability: SessionResumeCapability,
    processes: tokio::sync::Mutex<HashMap<String, Arc<SessionProcess>>>,
    /// The machine-wide spawn lock; tests give each driver its own so they
    /// can run in parallel without sharing one machine-wide credential chain.
    spawn: SpawnLock,
    backoff: Backoff,
}

impl Drop for AntigravityDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}

impl AntigravityDriver {
    async fn retire_idle_sessions(&self) -> Result<(), DriverError> {
        let processes: Vec<_> = {
            let map = self.processes.lock().await;
            if map.values().any(|p| p.turn_active.load(Ordering::Acquire)) {
                return Err(DriverError::CapabilityUnavailable(
                    "backendText.m9ce27b85".into(),
                ));
            }
            map.iter()
                .map(|(key, p)| (key.clone(), p.clone(), p.handle()))
                .collect()
        };
        for (key, process, handle) in processes {
            process.request_close(&handle, true, false)?;
            process.confirmed().await;
            self.remove_retired(&key, &process, &handle).await;
        }
        Ok(())
    }
    /// Creates a driver for one concrete installation. `resume_capability`
    /// must come from real runtime evidence, never from documentation.
    pub fn new(
        provider_id: &str,
        installation_id: &str,
        executable: impl Into<TargetExecutable>,
        native_version: Option<String>,
        resume_capability: SessionResumeCapability,
    ) -> Result<Self, agent_domain::DomainError> {
        let executable = executable.into();
        let account = account_of(&executable.target);
        Ok(Self {
            admission: tokio::sync::Mutex::new(()),
            stopping: tokio::sync::watch::channel(false).0,
            retired: std::sync::Mutex::new(HashMap::new()),
            models: tokio::sync::Mutex::new(vec![]),
            quota_cache: tokio::sync::Mutex::new(None),
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
            spawn: account.spawn.clone(),
            backoff: account.backoff.clone(),
        })
    }

    /// Spawns `agy` with stream-json args and verifies it emits an `init`
    /// event (or a well-formed terminal) within the probe budget. A missing
    /// executable reports `NotConfigured`; an unauthenticated account is left
    /// for the real turn to surface honestly.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NotConfigured`] when the executable cannot start
    /// and [`DriverError::Protocol`] when the handshake times out.
    pub async fn probe_ready(executable: impl Into<TargetExecutable>) -> Result<(), DriverError> {
        let executable = executable.into();
        let lock = account_of(&executable.target).spawn.clone();
        Self::probe_ready_with(executable, &lock).await
    }

    /// A driver for an installation on `executable`'s machine, once it answers.
    ///
    /// # Errors
    ///
    /// As [`Self::probe_ready`], or when the identifiers are invalid.
    pub async fn connect_ready(
        provider_id: &str,
        installation_id: &str,
        executable: TargetExecutable,
        native_version: Option<String>,
    ) -> Result<Self, DriverError> {
        Self::probe_ready(&executable).await?;
        Self::new(
            provider_id,
            installation_id,
            executable,
            native_version,
            // Cross-process resume via `--conversation` was verified at runtime.
            SessionResumeCapability::Supported,
        )
        .map_err(|error| DriverError::Protocol(error.to_string()))
    }

    async fn probe_ready_with(
        executable: impl Into<TargetExecutable>,
        lock: &SpawnLock,
    ) -> Result<(), DriverError> {
        let executable = executable.into();
        let backoff = account_of(&executable.target).backoff.clone();
        // Backed-off auth failures return immediately; every probe touches
        // the shared credential chain and must not retry in a loop.
        if let Some(kind) = auth_backoff_kind(&backoff).await {
            return Err(DriverError::NotConfigured(kind.into()));
        }
        // The slot is held for the whole process lifetime: the auth refresh
        // at startup is exactly the raced write this guard prevents.
        let slot = enter_spawn_slot(lock).await?;
        // Native cleanup/auth exclusion outlives a cancelled readiness caller.
        let (_observer, cancelled) = oneshot::channel::<()>();
        tokio::spawn(async move { Self::probe_owned(executable, backoff, slot, cancelled).await })
            .await
            .map_err(|_| {
                DriverError::Disconnected("Antigravity readiness actor ended unexpectedly".into())
            })?
    }
    async fn probe_owned(
        executable: TargetExecutable,
        backoff: Backoff,
        slot: SpawnSlot,
        mut cancelled: oneshot::Receiver<()>,
    ) -> Result<(), DriverError> {
        let abandoned = || DriverError::Disconnected("Antigravity readiness caller left".into());
        if cancelled.try_recv() == Err(oneshot::error::TryRecvError::Closed) {
            return Err(abandoned());
        }
        let spawn_started = std::time::Instant::now();
        let local = executable.target.is_local();
        let mut child = spawn(&AntigravityLaunch { executable }, None, None)?;
        eprintln!(
            "antigravity timing stage=process_spawn elapsed_ms={}",
            spawn_started.elapsed().as_millis()
        );
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DriverError::Disconnected("agy stdin is unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Disconnected("agy stdout is unavailable".into()))?;
        // Closing stdin asks the CLI to finish the (empty) session and exit
        // on its own; a stuck CLI is terminated within its owned process scope.
        drop(stdin);
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let outcome = tokio::select! {
            biased;
            _ = &mut cancelled => None,
            // Silent authentication plus eligibility checks can exceed the 20s
            // catalog budget on Windows. Let the native startup report its actual
            // account error instead of interrupting it and calling it broken.
            result = tokio::time::timeout(std::time::Duration::from_secs(45), reader.read_line(&mut line)) => Some(result),
        };
        drop(reader);
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
        if !matches!(closed, Ok(Ok(_))) && child.terminate().await.is_err() {
            // The probe caller may leave, but auth exclusion must remain until
            // this original scope has actually exited.
            tokio::spawn(async move {
                let _slot = slot;
                while child.terminate().await.is_err() {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });
            return Err(DriverError::Disconnected(
                "Antigravity probe cleanup remains pending".into(),
            ));
        }
        drop(slot);
        let Some(outcome) = outcome else {
            return Err(abandoned());
        };
        match outcome {
            // Any well-formed NDJSON line proves the protocol surface works.
            Ok(Ok(0)) => Err(DriverError::Protocol(
                "agy closed the stream before any event".into(),
            )),
            Ok(Ok(_)) => {
                if line.trim().is_empty() || serde_json::from_str::<Value>(&line).is_err() {
                    Err(DriverError::Protocol(
                        "agy did not emit a JSON event line".into(),
                    ))
                } else if !local && signed_out(&line) {
                    // A server has no window to sign in from, so it must be offered.
                    Err(DriverError::NotConfigured("authentication_failed".into()))
                } else if let Some(Classified::AuthFailure(kind, _)) = classify(line.trim_end()) {
                    // The very first event already reports an account problem.
                    record_auth_failure(&backoff, kind).await;
                    Err(DriverError::NotConfigured(kind.into()))
                } else {
                    // A healthy event surface proves the credential chain
                    // works again; clear any stale backoff.
                    clear_auth_failures(&backoff).await;
                    Ok(())
                }
            }
            Ok(Err(_)) => Err(DriverError::Disconnected(
                "agy stdout could not be read".into(),
            )),
            Err(_) => Err(DriverError::Protocol(
                "agy did not emit an event within the probe timeout".into(),
            )),
        }
    }
}

fn spawn(
    launch: &AntigravityLaunch,
    resume_conversation: Option<&str>,
    model: Option<&str>,
) -> Result<Child, DriverError> {
    spawn_workspace(launch, resume_conversation, model, None, None, None)
}

fn workspace_args(workspace: &SessionRequest, resume: bool) -> Vec<String> {
    let mut args = Vec::new();
    if !resume {
        args.push("--new-project".into());
    }
    for directory in
        std::iter::once(&workspace.working_directory).chain(&workspace.additional_directories)
    {
        args.extend([
            "--add-dir".into(),
            agent_runtime::external_path(directory)
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    args
}

#[cfg(test)]
mod multi_root_args_tests {
    #[test]
    fn new_project_is_isolated_and_resume_retains_all_roots() {
        let request = super::SessionRequest {
            app_mcps: vec![],
            permission_mode: None,
            working_directory: "/primary".into(),
            additional_directories: vec!["/extra folder".into()],
        };
        assert_eq!(
            super::workspace_args(&request, false),
            [
                "--new-project",
                "--add-dir",
                "/primary",
                "--add-dir",
                "/extra folder"
            ]
        );
        assert_eq!(
            super::workspace_args(&request, true),
            ["--add-dir", "/primary", "--add-dir", "/extra folder"]
        );
    }
}

fn spawn_workspace(
    launch: &AntigravityLaunch,
    resume_conversation: Option<&str>,
    model: Option<&str>,
    workspace: Option<&SessionRequest>,
    bridge: Option<&approval_bridge::Bridge>,
    usage_log: Option<&std::path::Path>,
) -> Result<Child, DriverError> {
    let target = &launch.executable.target;
    let mut command = launch.executable.command();
    command
        .args(session_args())
        .current_dir(target.temp_directory());
    if let Some(log) = usage_log {
        command
            .arg("--log-file")
            .arg(agent_runtime::external_path(log));
    }
    let mode = workspace.and_then(|w| w.permission_mode.as_deref());
    driver_api::permissions::antigravity().validate(mode)?;
    if matches!(mode, Some("desktop-approvals" | "plan")) && bridge.is_none() {
        return Err(DriverError::CapabilityUnavailable(
            "backendText.m2ad2df81".into(),
        ));
    }
    // Headless native permission prompts cannot consume approval replies.
    // Desktop modes instead gate every tool through the verified, live Hook.
    if matches!(
        mode,
        Some("desktop-approvals" | "plan" | "dangerously-skip-permissions")
    ) {
        command.arg("--dangerously-skip-permissions");
    }
    if mode == Some("plan") {
        command.arg("--mode").arg("plan");
    }
    if let Some(workspace) = workspace {
        command.current_dir(agent_runtime::external_path(&workspace.working_directory));
        // Keep user workspaces out of agy's shared default CLI project.
        // A resumed conversation carries its original project identity.
        command.args(workspace_args(workspace, resume_conversation.is_some()));
        if bridge.is_none()
            && let Some(mcp) = workspace.app_mcps.first()
        {
            command
                .arg("--add-dir")
                .arg(agent_runtime::external_path(&mcp.directory));
        }
    }
    if let Some(model) = model {
        if model.is_empty()
            || model.len() > 200
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(DriverError::CapabilityUnavailable(
                "invalid model identifier".into(),
            ));
        }
        command.arg("--model").arg(model);
    }
    if let Some(conversation_id) = resume_conversation {
        command.arg("--conversation").arg(conversation_id);
    }
    // Sessions and readiness probes use pipes. Interactive OAuth belongs to
    // the explicit native login action, outside the driver.
    command.stdio(Pipe::Piped, Pipe::Piped, Pipe::Piped);
    prepare_command(&mut command, !target.is_local());
    if let Some(bridge) = bridge {
        bridge.configure(&mut command);
    }
    command.spawn().map_err(|error| {
        if matches!(error.kind(), std::io::ErrorKind::NotFound) {
            DriverError::NotConfigured("agy executable is missing".into())
        } else {
            DriverError::Disconnected(format!("agy process failed to start: {error}"))
        }
    })
}

async fn run_session(
    mut child: Child,
    process: Arc<SessionProcess>,
    mut closing: tokio::sync::watch::Receiver<bool>,
    completed: tokio::sync::watch::Sender<Option<Result<(), String>>>,
) {
    let stderr = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
            }
        })
    });
    let reader = child.stdout.take().map(|stdout| {
        let process = process.clone();
        tokio::spawn(stream_session(stdout, process))
    });
    let requested = tokio::select! {
        ()=async{if let Some(reader)=&reader{while !reader.is_finished(){tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}}=>*closing.borrow(),
        ()=async{while !*closing.borrow_and_update(){if closing.changed().await.is_err(){break;}}}=>true,
        _=child.wait()=>*closing.borrow(),
    };
    {
        let _lifecycle = process.lifecycle.lock().expect("session lifecycle");
        process.closing.send_replace(true);
    }
    process.pending_approvals.lock().expect("approvals").clear();
    process.pending_questions.lock().expect("questions").clear();
    // Never wait for a blocked pipe writer before requesting native shutdown.
    if let Ok(mut stdin) = process.stdin.try_lock() {
        stdin.take();
    }
    let natural = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
    if !matches!(natural, Ok(Ok(_))) {
        let mut delay = 1;
        loop {
            match child.terminate().await {
                Ok(()) => break,
                Err(_) => {
                    completed.send_replace(Some(Err(
                        "Antigravity process shutdown remains unconfirmed".into(),
                    )));
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
    }
    process.stdin.lock().await.take();
    if let Some(mut reader) = reader
        && tokio::time::timeout(std::time::Duration::from_secs(1), &mut reader)
            .await
            .is_err()
    {
        reader.abort();
        let _ = reader.await;
    }
    let active = process.turn_active.swap(false, Ordering::AcqRel);
    if let Some(updates) = process.updates.lock().expect("updates sink").take()
        && active
    {
        let terminal = if requested && !process.input_failed.load(Ordering::Acquire) {
            DriverEvent::TurnInterrupted {
                summary: "Antigravity session closing".into(),
            }
        } else {
            DriverEvent::TurnFailed {
                reason: "Antigravity stream ended unexpectedly".into(),
            }
        };
        let _ = updates.send(terminal);
    }
    if let Some(stderr) = stderr {
        stderr.abort();
        let _ = stderr.await;
    }
    let tasks = std::mem::take(&mut *process.background.lock().expect("background tasks"));
    for task in tasks {
        let _ = task.await;
    }
    process.pending_approvals.lock().expect("approvals").clear();
    process.pending_questions.lock().expect("questions").clear();
    process.finished.store(true, Ordering::Release);
    completed.send_replace(Some(Ok(())));
}

async fn stream_session(stdout: agent_runtime::ProcessStream, process: Arc<SessionProcess>) {
    let startup_clock = std::time::Instant::now();
    let mut first_text_seen = false;
    let mut reader = BufReader::new(stdout);
    let mut permission_denied = false;
    let mut has_answer = false;
    loop {
        let mut bytes = Vec::new();
        match (&mut reader)
            .take(FRAME_LIMIT as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) if bytes.len() > FRAME_LIMIT => break,
            Ok(_) => {}
        }
        let Ok(line) = String::from_utf8(bytes) else {
            break;
        };
        if let Ok(raw) = serde_json::from_str::<Value>(&line) {
            let id = match raw["event"].as_str() {
                Some("init") => raw["conversation_id"].as_str(),
                Some("step_update") => raw
                    .pointer("/step_update/conversation_id")
                    .and_then(Value::as_str),
                Some("result") => raw
                    .pointer("/result/conversation_id")
                    .and_then(Value::as_str),
                _ => None,
            };
            if let Some(id) = id {
                let Ok(bound) = process.observe_native(id) else {
                    break;
                };
                if bound
                    && let Some(updates) = process.updates.lock().expect("updates sink").as_ref()
                {
                    let _ = updates.send(DriverEvent::SessionReady {
                        native_session_id: id.into(),
                    });
                }
            }
            session_usage::observe(&mut process.usage.lock().expect("usage"), &raw);
            if let Some(error) = raw.pointer("/step_update/tool_info/error") {
                let text = error.to_string().to_ascii_lowercase();
                permission_denied |= [
                    "permission check failed",
                    "denied permission",
                    "permission denied",
                ]
                .iter()
                .any(|s| text.contains(s));
            }

            if let Some(updates) = process.updates.lock().expect("updates sink").as_ref() {
                for event in driver_api::presentation::presentation_events("antigravity", "", &raw)
                {
                    let _ = updates.send(event);
                }
            }
        }
        if let Ok(raw) = serde_json::from_str::<Value>(line.trim_end())
            && let Some(children) = raw
                .pointer("/step_update/subagent_info/subagents")
                .and_then(Value::as_array)
        {
            let mut known = process.approval_children.lock().expect("children");
            for child in children.iter().take(64) {
                if let Some(id) = child["conversation_id"]
                    .as_str()
                    .filter(|id| id.len() <= 128)
                {
                    known.insert(id.into());
                }
            }
        }
        let Some(classified) = classify(line.trim_end()) else {
            continue;
        };
        // A long-lived CLI emits init only once. A subsequent fast turn may
        // finish before the prompt writer gets polled again.
        process.mark_started();
        match classified {
            Classified::Started { conversation_id } => {
                eprintln!(
                    "antigravity timing stage=native_init elapsed_ms={}",
                    startup_clock.elapsed().as_millis()
                );
                let _ = conversation_id;
                process.mark_started();
            }
            Classified::Text(text) => {
                if !first_text_seen {
                    first_text_seen = true;
                    eprintln!(
                        "antigravity timing stage=first_text_since_spawn elapsed_ms={}",
                        startup_clock.elapsed().as_millis()
                    );
                }
                has_answer |= !text.trim().is_empty();
                if let Some(updates) = process.updates.lock().expect("updates sink").as_ref() {
                    let _ = updates.send(DriverEvent::TextDelta {
                        item_id: "message".into(),
                        text,
                    });
                }
            }
            Classified::Tool { name, completed } => {
                let _ = (name, completed); /* Structured step metadata emitted above. */
            }
            Classified::AuthFailure(kind, error) => {
                // Count toward the backoff; report the stable kind together
                // with agy's own error text.
                record_auth_failure(&process.backoff, kind).await;
                let _gate = process.lifecycle.lock().expect("session lifecycle");
                if let Some(updates) = process.updates.lock().expect("updates sink").take() {
                    let _ = updates.send(DriverEvent::TurnFailed {
                        reason: driver_api::failure::with_detail(kind, &error),
                    });
                }
                process.turn_active.store(false, Ordering::Release);
                process.turn.lock().expect("turn scope").take();
                process.pending_approvals.lock().expect("approvals").clear();
                process.pending_questions.lock().expect("questions").clear();
            }
            Classified::Terminal(driver_event) => {
                let _gate = process.lifecycle.lock().expect("session lifecycle");
                let terminal = if process.input_failed.load(Ordering::Acquire) {
                    DriverEvent::TurnFailed {
                        reason: "Antigravity input failed before the turn completed".into(),
                    }
                } else if (process.interrupt_requested.load(Ordering::Acquire)
                    || process.stop_requested.load(Ordering::Acquire))
                    && matches!(driver_event, DriverEvent::TurnCompleted { .. })
                {
                    DriverEvent::TurnInterrupted {
                        summary: "Antigravity turn was interrupted".into(),
                    }
                } else if permission_denied
                    && !has_answer
                    && matches!(driver_event, DriverEvent::TurnCompleted { .. })
                {
                    DriverEvent::TurnFailed {reason:"backendText.md98b990e".into()}
                } else {
                    driver_event
                };
                permission_denied = false;
                has_answer = false;
                if let Some(updates) = process.updates.lock().expect("updates sink").take() {
                    let _ = updates.send(terminal);
                }
                process.turn_active.store(false, Ordering::Release);
                process.turn.lock().expect("turn scope").take();
                process.pending_approvals.lock().expect("approvals").clear();
                process.pending_questions.lock().expect("questions").clear();
                process.interrupt_requested.store(false, Ordering::Release);
            }
            Classified::Ignored => {}
        }
    }
    // EOF closes any active turn with a stable, non-leaking reason.
    let _gate = process.lifecycle.lock().expect("session lifecycle");
    process.turn.lock().expect("turn scope").take();
    let active = process.turn_active.swap(false, Ordering::AcqRel);
    if let Some(updates) = process.updates.lock().expect("updates sink").take() {
        let _ = updates.send(
            if active
                && !process.input_failed.load(Ordering::Acquire)
                && (process.interrupt_requested.load(Ordering::Acquire)
                    || process.stop_requested.load(Ordering::Acquire))
            {
                DriverEvent::TurnInterrupted {
                    summary: "Antigravity session closing".into(),
                }
            } else if active {
                DriverEvent::TurnFailed {
                    reason: "Antigravity stream ended before the turn completed".into(),
                }
            } else {
                DriverEvent::TurnCompleted {
                    summary: "Antigravity session closed".into(),
                }
            },
        );
    }
    process.pending_approvals.lock().expect("approvals").clear();
    process.pending_questions.lock().expect("questions").clear();
}

/// The documented input shape: one NDJSON `user` event per prompt; context
/// items ride along as text blocks in order before the user prompt.
// The private Hook directory is also passed through --add-dir so agy can load
// approvals. Identify business roots explicitly; native root ordering alone
// must not make that implementation directory the model's working directory.
fn workspace_instruction(workspace: &SessionRequest) -> String {
    format!(
        "[dogcodes workspace]\nPrimary working directory: {}\nAdditional project directories: {}\nResolve relative project paths against the primary working directory unless the user explicitly selects another project directory. Directories named dogcodes-agy-approval-* contain private approval infrastructure only; never choose them as a project workspace or write task output there.",
        serde_json::to_string(&workspace.working_directory).expect("path string"),
        serde_json::to_string(&workspace.additional_directories).expect("path strings")
    )
}

fn user_event(context: &[ContextDelivery], prompt: &str) -> Value {
    let mut blocks = Vec::with_capacity(context.len() + 1);
    for delivery in context {
        let kind = match delivery.kind {
            ContextDeliveryKind::Handoff => "Handoff",
            ContextDeliveryKind::Reconciliation => "Reconciliation",
        };
        let body = String::from_utf8_lossy(&delivery.content);
        blocks.push(json!({
            "type": "text",
            "text": format!(
                "[dogcodes {kind} {} sha256={}]\n{body}",
                delivery.artifact_id, delivery.payload_sha256
            ),
        }));
    }
    blocks.push(json!({ "type": "text", "text": prompt }));
    json!({ "event": "user", "message": { "content": blocks } })
}

#[async_trait]
impl AgentDriver for AntigravityDriver {
    fn runs_on_this_machine(&self) -> bool {
        self.executable.target.is_local()
    }
    fn prepare_app_mcp(&self, mcp: driver_api::AppMcp) -> Result<driver_api::AppMcp, DriverError> {
        mcp.for_target(&self.executable.target)
    }
    async fn prepare_attachments(
        &self,
        items: &mut [agent_domain::PromptAttachment],
    ) -> Result<(), DriverError> {
        if self.executable.target.is_local() {
            return Ok(());
        }
        driver_api::attachments::deliver_to_target(&self.executable.target, items).await
    }
    fn independent_session_driver(&self) -> Result<Option<Arc<dyn AgentDriver>>, DriverError> {
        let driver = Self::new(
            self.executor.provider_id.as_str(),
            self.executor.installation_id.as_deref().unwrap_or(""),
            self.executable.clone(),
            self.native_version.clone(),
            self.resume_capability,
        )
        .map_err(|_| DriverError::Protocol("cannot recreate Antigravity installation".into()))?;
        Ok(Some(Arc::new(driver)))
    }
    fn session_usage_provider(self: Arc<Self>) -> Option<Arc<dyn driver_api::QuotaProvider>> {
        Some(self)
    }
    async fn permission_catalog(
        &self,
        _workspace: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        Ok(driver_api::permissions::antigravity())
    }
    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        let mut catalog = driver_api::models::antigravity(self.available_models().await?);
        let native = self.models.lock().await;
        for model in &mut catalog.models {
            if let Some((_, label)) = native.iter().find(|(id, _)| {
                id == &model.id
                    || model
                        .thinking_options
                        .iter()
                        .any(|effort| id == &format!("{}-{}", model.id, effort.id))
            }) {
                model.label = label.clone();
                for suffix in [" (Low)", " (Medium)", " (High)"] {
                    if let Some(base) = model.label.strip_suffix(suffix) {
                        model.label = base.into();
                        break;
                    }
                }
            }
        }
        Ok(catalog)
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let native = model.map(|m| thinking.map_or_else(|| m.to_owned(), |e| format!("{m}-{e}")));
        self.create_session_with_model(request, native.as_deref())
            .await
    }
    async fn reconfigure_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let native = model.map(|m| thinking.map_or_else(|| m.to_owned(), |e| format!("{m}-{e}")));
        self.open(request, Some(id), native.as_deref(), true).await
    }

    async fn resume_selected_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let native = model.map(|m| thinking.map_or_else(|| m.to_owned(), |e| format!("{m}-{e}")));
        self.resume_workspace_session(id, request, native.as_deref())
            .await
    }
    async fn available_models(&self) -> Result<Vec<String>, DriverError> {
        let mut cache = self.models.lock().await;
        if !cache.is_empty() {
            return Ok(cache.iter().map(|(id, _)| id.clone()).collect());
        }
        let slot = SpawnSlot {
            _guard: self.spawn.clone().try_lock_owned().map_err(|_| {
                DriverError::CapabilityUnavailable("backendText.m6b7ac4c4".into())
            })?,
        };
        let result = catalog::read(self.executable.clone(), slot).await?;
        *cache = result;
        Ok(cache.iter().map(|(id, _)| id.clone()).collect())
    }
    async fn refresh_model_catalog_in_workspace(
        &self,
        model: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<driver_api::ModelCatalog, DriverError> {
        self.models.lock().await.clear();
        self.model_catalog_in_workspace(model, workspace).await
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

    fn supports_live_session_reuse(&self) -> bool {
        true
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        self.resume_capability
    }

    async fn read_subagent(&self, id: &str) -> Result<Vec<Value>, DriverError> {
        if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(DriverError::Protocol("invalid child id".into()));
        }
        let target = &self.executable.target;
        let home = target
            .inherited("HOME")
            .ok_or_else(|| DriverError::CapabilityUnavailable("home unavailable".into()))?;
        let path = std::path::PathBuf::from(home)
            .join(".gemini/antigravity-cli/brain")
            .join(id)
            .join(".system_generated/logs/transcript.jsonl");
        let bytes = scratch::read_file(target, &path, 1_048_577)
            .await
            .ok_or_else(|| DriverError::Protocol("log unavailable".into()))?;
        if bytes.len() > 1_048_576 {
            return Err(DriverError::Protocol("log unavailable".into()));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| DriverError::Protocol("log unavailable".into()))?;
        let mut output = Vec::new();
        for line in text.lines() {
            let Ok(row) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(calls) = row["tool_calls"].as_array() {
                for call in calls {
                    output.push(serde_json::json!({"type":"tool","name":call["name"],"input":call["args"],"status":"unknown"}));
                }
            }
            if row["type"] == "TOOL_RESULT" {
                output.push(serde_json::json!({"type":"tool","name":row["tool_name"],"output":row["content"],"status":"unknown"}));
            }
        }
        Ok(output)
    }

    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.create_session_with_model(request, None).await
    }

    async fn create_session_with_model(
        &self,
        request: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, None, model, false).await
    }

    async fn resume_session(&self, id: &str, cwd: &str) -> Result<SessionHandle, DriverError> {
        self.resume_session_with_model(id, cwd, None).await
    }
    async fn resume_session_with_model(
        &self,
        id: &str,
        cwd: &str,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_workspace_session(
            id,
            SessionRequest {
                app_mcps: vec![],
                permission_mode: None,
                working_directory: cwd.into(),
                additional_directories: vec![],
            },
            model,
        )
        .await
    }
    async fn resume_workspace_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(request, Some(id), model, false).await
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: HubPromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let process = self.process_for(handle).await?;
        let _admission = process.admission.lock().await;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        {
            let _lifecycle = process.lifecycle.lock().expect("session lifecycle");
            process.validate_scope(handle)?;
            if *process.closing.borrow() || process.finished.load(Ordering::Acquire) {
                return Err(DriverError::Disconnected(
                    "Antigravity session is closing".into(),
                ));
            }
            if process
                .turn_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(DriverError::CapabilityUnavailable(
                    "this agy session already has an active turn".into(),
                ));
            }
            *process.turn.lock().expect("turn scope") = Some(uuid::Uuid::new_v4());
            *process.started.lock().expect("started turn") = None;
            process.approval_children.lock().expect("children").clear();
            if let Some(id) = process.native_id.lock().expect("native id").as_ref() {
                let _ = event_tx.send(DriverEvent::SessionReady {
                    native_session_id: id.clone(),
                });
            }
            *process.updates.lock().expect("updates sink") = Some(event_tx);
        }
        let mut dispatch = Startup {
            process: process.clone(),
            claimed: false,
        };
        let mut event = user_event(&request.context, &request.prompt);
        let blocks = event["message"]["content"]
            .as_array_mut()
            .expect("content blocks");
        for item in &request.attachments {
            let mut block = driver_api::attachments::file_text(item);
            if driver_api::attachments::is_image(item) {
                block["text"] = json!(format!(
                    "Image attached by the user. Use view_file on the exact path to inspect its pixels: {}",
                    serde_json::json!({"name":item.name,"path":item.path})
                ));
            }
            blocks.push(block);
        }
        event["message"]["content"]
            .as_array_mut()
            .expect("content blocks")
            .insert(
                0,
                json!({"type": "text", "text": format!("{}\nClarification questions: ask_question is connected to dogcodes through a Hook. The actual user answer is returned in the Hook reason while the native auto-skip is blocked. Treat it as the user response, and do not retry just because the tool was blocked.", process.workspace_instruction)}),
            );
        if let Err(error) = process.send_line(&event).await {
            let _gate = process.lifecycle.lock().expect("session lifecycle");
            *process.updates.lock().expect("updates sink") = None;
            process.turn_active.store(false, Ordering::Release);
            process.turn.lock().expect("turn scope").take();
            return Err(error);
        }
        let stream: DriverEventStream =
            Box::pin(UnboundedReceiverStream::new(event_rx).map(Ok::<_, DriverError>));
        process.mark_started();
        dispatch.claimed = true;
        Ok(stream)
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        let process = self.process_for(handle).await?;
        let _gate = process.lifecycle.lock().expect("session lifecycle");
        process.validate_scope(handle)?;
        if *process.closing.borrow() || !process.turn_active.load(Ordering::Acquire) {
            return Err(DriverError::Protocol(
                "Antigravity interaction is no longer active".into(),
            ));
        }
        let mut pending = process.pending_questions.lock().expect("questions");
        let q = pending
            .get(request_id)
            .ok_or_else(|| DriverError::Protocol("Question expired".into()))?;
        q.form.validate(&answer).map_err(DriverError::Protocol)?;
        approval_bridge::validate_answer(&answer)?;
        pending
            .remove(request_id)
            .expect("pending question")
            .sender
            .send(answer)
            .map_err(|_| DriverError::Disconnected("Question closed".into()))
    }

    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        native_request_id: &str,
        option_id: &str,
    ) -> Result<(), DriverError> {
        if !["allow_once", "deny"].contains(&option_id) {
            return Err(DriverError::Protocol("backendText.mc3a0e8d8".into()));
        }
        let process = self.process_for(handle).await?;
        let _gate = process.lifecycle.lock().expect("session lifecycle");
        process.validate_scope(handle)?;
        if *process.closing.borrow() || !process.turn_active.load(Ordering::Acquire) {
            return Err(DriverError::Protocol(
                "Antigravity interaction is no longer active".into(),
            ));
        }
        let sender = process
            .pending_approvals
            .lock()
            .expect("approvals")
            .remove(native_request_id)
            .ok_or_else(|| DriverError::Protocol("backendText.m88b50b98".into()))?;
        sender
            .send(option_id == "allow_once")
            .map_err(|_| DriverError::Protocol("backendText.m88b50b98".into()))
    }

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        let process = self.process_for(handle).await?;
        process.request_close(handle, false, true)?;
        process.wait_closed().await
    }

    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.find(id).await.map(|(_, process)| process.handle())
    }

    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        if let Some((_, process)) = self.find(&handle.native_session_id).await {
            process.validate_scope(handle).ok()?;
            return Some(Box::pin(async move { process.confirmed().await }));
        }
        self.retired
            .lock()
            .expect("retired scopes")
            .get(&handle.native_session_id)
            .filter(|old| old.handle.same_scope(handle))
            .map(|_| Box::pin(async {}) as driver_api::SessionShutdownProof)
    }

    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        let Some((key, process)) = self.find(&handle.native_session_id).await else {
            return if self
                .retired
                .lock()
                .expect("retired scopes")
                .get(&handle.native_session_id)
                .is_some_and(|old| old.handle.same_scope(&handle))
            {
                Ok(())
            } else {
                Err(stale_scope())
            };
        };
        process.request_close(&handle, false, false)?;
        process.wait_closed().await?;
        self.remove_retired(&key, &process, &handle).await;
        Ok(())
    }
}

/// Local session key generator (ULID would add a dependency here; a process
/// -unique counter plus timestamp is enough for the handle namespace).
fn uuid_like_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{now:x}-{count:x}")
}

#[cfg(all(test, unix))]
mod cleanup_tests;
#[cfg(all(test, unix))]
mod metadata_tests;
#[cfg(all(test, unix))]
mod scope_tests;
#[cfg(test)]
mod tests {
    use super::*;

    // The fake agy is a shell script.
    #[cfg(unix)]
    #[tokio::test]
    async fn verifying_the_approval_hook_cannot_open_a_browser() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_agy(directory.path());
        let driver = test_driver(executable.clone());
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: Some("desktop-approvals".into()),
                additional_directories: vec![],
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        driver.shutdown(session).await.unwrap();
        assert!(!executable.with_extension("browser").exists());
    }

    #[test]
    fn only_a_failed_login_counts_as_signed_out() {
        let event = |error: &str| {
            json!({"event":"result","result":{"status":"ERROR","error":error}}).to_string()
        };
        assert!(signed_out(&event("authentication failed or timed out")));
        assert!(!signed_out(&event("no such host")));
        assert!(!signed_out(r#"{"event":"init","conversation_id":"c"}"#));
    }

    #[test]
    fn auth_backoff_preserves_the_account_failure_kind_until_expiry() {
        for kind in ["authentication_failed", "access_denied"] {
            let mut state = AuthBackoffState {
                failures: 1,
                last_failure: Some(std::time::Instant::now()),
                kind: Some(kind),
            };
            assert_eq!(state.active_kind(), Some(kind));
            state.last_failure = Some(
                std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(61))
                    .unwrap(),
            );
            assert_eq!(state.active_kind(), None);
        }
        assert_eq!(AuthBackoffState::default().active_kind(), None);
    }

    #[test]
    fn classifies_the_documented_event_shapes() {
        assert!(matches!(
            classify(
                r#"{"event":"init","init":{"cwd":"/tmp","tools":[],"permission_mode":"request-review"}}"#
            ),
            Some(Classified::Started { .. })
        ));
        match classify(
            r#"{"event":"step_update","step_update":{"step_type":"agent_response","state":"ACTIVE","text_delta":"hi"}}"#,
        ) {
            Some(Classified::Text(text)) => assert_eq!(text, "hi"),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            classify(
                r#"{"event":"step_update","step_update":{"step_type":"tool","state":"DONE","tool_name":"run_command"}}"#,
            ),
            Some(Classified::Tool {
                completed: true,
                ..
            })
        ));
        assert!(matches!(
            classify(
                r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"ok"}}"#,
            ),
            Some(Classified::Terminal(DriverEvent::TurnCompleted { .. }))
        ));
        // A CLI eligibility wrapper around a transport error must not trigger
        // credential backoff (e.g. profile-picture TLS timeouts during login).
        for cause in ["TLS handshake timeout", "context deadline exceeded", "EOF"] {
            let event = json!({"event":"result","result":{"status":"ERROR","error":format!("Eligibility check failed: failed to get profile picture: {cause}")}});
            let Some(Classified::Terminal(DriverEvent::TurnFailed { reason })) =
                classify(&event.to_string())
            else {
                panic!("transport failure must stay a turn failure");
            };
            assert_eq!(
                driver_api::failure::split_turn_failure(&reason).0,
                Some("agent_connection_failed")
            );
        }
        // Any other error result keeps agy's own text for the user.
        let error =
            "FAILED_PRECONDITION (code 400): User location is not supported for the API use.";
        let event = json!({"event":"result","result":{"status":"ERROR","error":error}});
        let Some(Classified::Terminal(DriverEvent::TurnFailed { reason })) =
            classify(&event.to_string())
        else {
            panic!("error result must fail the turn");
        };
        assert_eq!(
            driver_api::failure::split_turn_failure(&reason),
            (None, Some(error))
        );
        // Eligibility/auth errors are their own classification for backoff.
        assert!(matches!(
            classify(
                r#"{"event":"result","result":{"status":"ERROR","error":"authentication required"}}"#
            ),
            Some(Classified::AuthFailure("authentication_failed", _))
        ));
        assert!(matches!(
            classify(
                r#"{"event":"result","result":{"status":"ERROR","error":"Eligibility check failed"}}"#,
            ),
            Some(Classified::AuthFailure("access_denied", _))
        ));
        assert!(matches!(
            classify(r#"{"event":"result","result":{"status":"INTERRUPTED"}}"#,),
            Some(Classified::Terminal(DriverEvent::TurnInterrupted { .. }))
        ));
        assert!(matches!(
            classify(
                r#"{"event":"result","result":{"status":"ERROR","error":"Quota exceeded for this account"}}"#
            ),
            Some(Classified::Terminal(DriverEvent::TurnQuotaExhausted { .. }))
        ));
        assert!(matches!(
            classify(
                r#"{"event":"result","result":{"status":"ERROR","error":"tool failed","response":"user text: quota exceeded"}}"#
            ),
            Some(Classified::Terminal(DriverEvent::TurnFailed { .. }))
        ));
        // Unmapped but valid events stay ignored; junk lines are skipped.
        assert!(matches!(
            classify(
                r#"{"event":"step_update","step_update":{"step_type":"checkpoint","state":"DONE"}}"#,
            ),
            Some(Classified::Ignored)
        ));
        assert!(classify("not json").is_none());
    }

    #[test]
    fn context_blocks_precede_the_user_prompt() {
        let delivery = ContextDelivery {
            artifact_id: agent_domain::ArtifactId::new(),
            kind: ContextDeliveryKind::Handoff,
            payload_sha256: "abc".into(),
            content: b"{}".to_vec(),
        };
        let event = user_event(&[delivery], "do work");
        let blocks = event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(
            blocks[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("[dogcodes Handoff")
        );
        assert_eq!(blocks[1]["text"], "do work");
    }

    #[cfg(unix)]
    pub(super) fn write_fake_agy(directory: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = directory.join("agy");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in *--print=/hooks*)
command -v open >/dev/null 2>&1 || command -v xdg-open >/dev/null 2>&1 && touch "${0}.browser"
;; esac
printf '%s\n' "$PATH" > "${0}.path"
PATH="$PATH:/usr/bin:/bin"
case "$*" in *--print=/hooks*)
python3 - "$@" <<'PYHOOK'
import sys,json,pathlib
args=sys.argv[1:]
root=[args[i+1] for i,v in enumerate(args[:-1]) if v=='--add-dir'][-1]
source=pathlib.Path(root)/'.agents/hooks.json'
a=json.loads(source.read_text())['dogcodes-approvals']['PreToolUse'][0]
print(json.dumps({'event':'command_result','command':{'name':'hooks','data':{'hooks':[{'name':'dogcodes-approvals','enabled':True,'source':str(source),'actions':[{'event':'PreToolUse','matcher':a['matcher'],'command':a['hooks'][0]['command']}]}]}}}))
PYHOOK
exit 0;; esac
printf '%s\n' "$@" > "${0}.args"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "${0}.input"
  case "$line" in
    *'"event":"user"'*)
      printf '%s\n' '{"event":"init","conversation_id":"conv-fixture","init":{"cwd":"/tmp","tools":[],"permission_mode":"request-review"}}'
      printf '%s\n' '{"event":"step_update","step_update":{"step_type":"agent_response","state":"ACTIVE","text_delta":"streamed"}}'
      printf '%s\n' '{"event":"step_update","step_update":{"step_type":"agent_response","state":"DONE"}}'
      printf '%s\n' '{"event":"result","result":{"conversation_id":"conv-fixture","status":"SUCCESS","response":"streamed"}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    pub(super) fn test_driver(executable: PathBuf) -> AntigravityDriver {
        test_driver_with(executable, SpawnLock::default())
    }

    /// Fixtures share a spawn lock only when a test is about that lock.
    #[cfg(unix)]
    pub(super) fn test_driver_with(executable: PathBuf, lock: SpawnLock) -> AntigravityDriver {
        let mut driver = AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            Some("1.1.26".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        driver.spawn = lock;
        driver
    }

    /// Conversations run in parallel: session processes never hold the slot
    /// that serializes short agy helpers.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_session_does_not_keep_other_conversations_from_starting() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_agy(directory.path());
        let lock = SpawnLock::default();
        let request = || SessionRequest {
            app_mcps: vec![],
            permission_mode: Some("dangerously-skip-permissions".into()),
            additional_directories: vec![],
            working_directory: "/tmp".into(),
        };
        let prompt = || HubPromptRequest {
            attachments: vec![],
            permission_mode: Some("dangerously-skip-permissions".into()),
            additional_directories: vec![],
            prompt: "hello".into(),
            working_directory: "/tmp".into(),
            context: vec![],
        };
        let first = test_driver_with(executable.clone(), lock.clone());
        let session = first.create_session(request()).await.unwrap();
        // Before the first process has produced any output, the slot is free.
        assert!(lock.clone().try_lock_owned().is_ok());
        let events: Vec<_> = first
            .prompt(&session, prompt())
            .await
            .unwrap()
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        // The first agy process is still alive and idle, yet the slot is free.
        assert!(!first.processes.lock().await.is_empty());
        assert!(lock.clone().try_lock_owned().is_ok());
        let second = test_driver_with(executable, lock);
        let other = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            second.create_session(request()),
        )
        .await
        .expect("second conversation waited for the first process to exit")
        .unwrap();
        let events: Vec<_> = second
            .prompt(&other, prompt())
            .await
            .unwrap()
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        first.shutdown(session).await.unwrap();
        second.shutdown(other).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_model_reaches_launch_and_live_session_reuses_it() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_agy(directory.path());
        let mut driver = test_driver(executable.clone());
        driver.resume_capability = SessionResumeCapability::Supported;
        let session = driver
            .create_session_with_model(
                SessionRequest {
                    app_mcps: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    additional_directories: vec![],
                    working_directory: "/tmp".into(),
                },
                Some("gemini-fixture"),
            )
            .await
            .unwrap();
        let events: Vec<_> = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    additional_directories: vec![],
                    prompt: "hello".into(),
                    working_directory: "/tmp".into(),
                    context: vec![],
                },
            )
            .await
            .unwrap()
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        let args = std::fs::read_to_string(executable.with_extension("args")).unwrap();
        assert!(args.contains("--model\ngemini-fixture\n"));
        let resumed = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            driver.resume_workspace_session(
                "conv-fixture",
                SessionRequest {
                    app_mcps: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    working_directory: "/tmp".into(),
                    additional_directories: vec![],
                },
                Some("gemini-fixture"),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resumed.native_session_id, "conv-fixture");
        let updated = driver
            .reconfigure_session(
                &resumed.native_session_id,
                SessionRequest {
                    app_mcps: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    working_directory: "/tmp".into(),
                    additional_directories: vec![],
                },
                Some("claude-fixture"),
                Some("high"),
            )
            .await
            .unwrap();
        assert_eq!(updated.native_session_id, resumed.native_session_id);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let args =
                    std::fs::read_to_string(executable.with_extension("args")).unwrap_or_default();
                if args.contains("--model\nclaude-fixture-high\n") {
                    assert!(args.contains("conv-fixture"));
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let with_attachment = driver
            .resume_workspace_session(
                &updated.native_session_id,
                SessionRequest {
                    app_mcps: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    working_directory: "/tmp".into(),
                    additional_directories: vec!["/tmp/attachment-fixture".into()],
                },
                Some("claude-fixture-high"),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let args =
                    std::fs::read_to_string(executable.with_extension("args")).unwrap_or_default();
                if args.contains("--add-dir\n/tmp/attachment-fixture\n") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        driver.shutdown(with_attachment).await.unwrap();
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            driver.create_session_with_model(
                SessionRequest {
                    app_mcps: vec![],
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    additional_directories: vec![],
                    working_directory: "/tmp".into(),
                },
                Some("claude-fixture"),
            ),
        )
        .await
        .expect("closing stdin must release the native process slot")
        .unwrap();
        driver.shutdown(next).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_ready_accepts_a_compliant_fake_cli() {
        let directory = tempfile::tempdir().unwrap();
        // The fake emits nothing until input; closing stdin ends it without
        // any event, which the probe treats as a protocol failure. Feed a
        // prompt through a pipe instead: use a shell wrapper.
        let wrapper = directory.path().join("agy-probe");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\nprintf '%s\\n' '{\"event\":\"init\",\"init\":{}}'\ncat > /dev/null\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        AntigravityDriver::probe_ready(&wrapper)
            .await
            .expect("fake cli should pass the event-surface probe");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_probe_preserves_native_profile_and_system_locations() {
        let directory = tempfile::tempdir().unwrap();
        let wrapper = directory.path().join("agy-probe.cmd");
        std::fs::write(
            &wrapper,
            "@echo off\r\nif not defined SystemRoot exit /b 41\r\nif not defined USERPROFILE exit /b 42\r\nif not defined LOCALAPPDATA exit /b 43\r\nif not defined APPDATA exit /b 44\r\necho {\"event\":\"init\",\"init\":{}}\r\n",
        )
        .unwrap();
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        AntigravityDriver::probe_ready_with(&wrapper, &lock)
            .await
            .expect("native readiness needs Windows system/profile directories after env_clear");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn full_turn_streams_init_text_and_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_fake_agy(directory.path());
        let driver = test_driver(executable);
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: Some("dangerously-skip-permissions".into()),
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
                    permission_mode: Some("dangerously-skip-permissions".into()),
                    additional_directories: vec![],
                    prompt: "do work".into(),
                    working_directory: "/tmp".into(),
                    context: vec![],
                },
            )
            .await
            .unwrap();
        let events: Vec<DriverEvent> = stream.map(|event| event.unwrap()).collect().await;
        let conversation = std::fs::read_to_string(directory.path().join("agy.path")).unwrap();
        assert_ne!(conversation.trim(), NOTHING_TO_RUN, "its tools need PATH");
        let input = std::fs::read_to_string(directory.path().join("agy.input")).unwrap();
        let sent: Value = serde_json::from_str(input.lines().next().unwrap()).unwrap();
        let blocks = sent["message"]["content"].as_array().unwrap();
        assert!(
            blocks[0]["text"]
                .as_str()
                .unwrap()
                .contains("Primary working directory: \"/tmp\"")
        );
        assert!(
            blocks[0]["text"]
                .as_str()
                .unwrap()
                .contains("private approval infrastructure only")
        );
        assert_eq!(blocks.last().unwrap()["text"], "do work");

        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnStarted { .. }))
        );
        assert!(events.iter().any(
            |event| matches!(event, DriverEvent::TextDelta { text, .. } if text == "streamed")
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
    }
}

/// Real-turn acceptance against the local installation; consumes real model
/// quota and requires the operator's explicit authorization.
#[cfg(test)]
mod real_turn_acceptance {
    use super::*;
    use driver_api::SessionRequest;

    const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(3);

    #[tokio::test]
    #[ignore = "two minimal real model turns; explicit operator authorization required"]
    async fn explicit_models_complete_real_turns() {
        let executable =
            PathBuf::from(std::env::var("AGY_MODEL_EXECUTABLE").expect("AGY_MODEL_EXECUTABLE"));
        let driver = AntigravityDriver::new(
            "antigravity",
            &InstallationId::new().to_string(),
            executable,
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let models = driver.available_models().await.unwrap();
        for model in ["gemini-3.1-pro-low", "claude-sonnet-4-6"] {
            assert!(models.iter().any(|id| id == model));
            let session = driver
                .create_session_with_model(
                    SessionRequest {
                        app_mcps: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                    },
                    Some(model),
                )
                .await
                .unwrap();
            let events = collect_turn(
                &driver,
                &session,
                "Reply with exactly OK. Do not use any tools.",
            )
            .await;
            driver.shutdown(session).await.unwrap();
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
                "selected model must complete"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, DriverEvent::Presentation { data, .. } if data["type"]=="tool"))
            );
        }
    }

    async fn collect_turn(
        driver: &AntigravityDriver,
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

    fn home_driver() -> AntigravityDriver {
        let home = std::env::var("HOME").expect("HOME");
        AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            PathBuf::from(format!("{home}/.local/bin/agy")),
            Some("1.1.26".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_streams_reuses_and_interrupts() {
        let driver = home_driver();
        let session = driver
            .create_session(SessionRequest {
                app_mcps: vec![],
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
                DriverEvent::TextDelta { text, .. } if text.trim() == "OK"
            )),
            "turn 1 must stream text: {first:?}"
        );
        assert!(
            first
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 1 must complete: {first:?}"
        );
        println!("turn 1 (stream + complete): ok");

        let second = collect_turn(&driver, &session, "Reply with exactly: DONE").await;
        assert!(
            second
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 2 must complete on the same live session: {second:?}"
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
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        driver
            .interrupt(&session)
            .await
            .expect("interrupt must send");
        let third = collector.await.unwrap();
        assert!(
            third
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. })),
            "turn 3 must end interrupted (or complete if it finished first): {third:?}"
        );
        println!("turn 3 (interrupt): ok");
    }
}

/// Real cross-process resume acceptance: turn 1 in one driver instance, then
/// a fresh instance must re-attach via `--conversation` and recall context.
#[cfg(test)]
mod real_resume_acceptance {
    use super::*;
    use driver_api::SessionRequest;
    use futures::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_reattaches_across_driver_instances() {
        let home = std::env::var("HOME").expect("HOME");
        let executable = PathBuf::from(format!("{home}/.local/bin/agy"));
        let workspace = tempfile::tempdir().unwrap();
        let first = AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            executable.clone(),
            Some("1.1.26".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let session = first
            .create_session(SessionRequest {
                app_mcps: vec![],
                permission_mode: None,
                additional_directories: vec![],
                working_directory: workspace.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
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
        // The vendor conversation id arrives via the SessionReady surface.
        let conversation_id = events
            .iter()
            .find_map(|event| match event {
                DriverEvent::SessionReady { native_session_id } => Some(native_session_id.clone()),
                _ => None,
            })
            .expect("conversation id must be surfaced");
        first.shutdown(session).await.unwrap();

        let second = AntigravityDriver::new(
            "antigravity",
            &agent_domain::InstallationId::new().to_string(),
            executable,
            Some("1.1.26".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let resumed = second
            .resume_session(
                &conversation_id,
                workspace.path().to_string_lossy().as_ref(),
            )
            .await
            .unwrap();
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
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        assert!(
            text.contains("dogcodes"),
            "resume must restore context: {text}"
        );
        println!("real resume (--conversation reattach + recall): ok");
    }
}
