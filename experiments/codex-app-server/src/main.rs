//! Isolated, credential-opaque fact probe for the installed Codex App Server.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use agent_runtime::{ProcessError, ProcessSpec, run_bounded};
use codex_app_server_probe::{
    diagnostics::{FixtureError, check_fixture, export_fixture},
    normalize::{NormalizedEvent, normalize},
    transport::{IncomingFrame, JsonlTransport, RequestExchange, TransportError},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{sleep, timeout},
};

const OUTPUT_LIMIT: usize = 64 * 1_024;
const SCHEMA_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const TURN_TIMEOUT: Duration = Duration::from_mins(2);
const SMOKE_MARKER: &str = "DOGCODES_SMOKE_OK";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Codex App Server probe failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ProbeError> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, input] if command == "check-sanitized-fixture" => {
            print_report(&check_fixture(Path::new(input))?)?;
        }
        [command, input, output] if command == "sanitize-fixture" => {
            let report = export_fixture(Path::new(input), Path::new(output))?;
            print_report(&report)?;
        }
        [command, executable, output] if command == "schema" => {
            let report = generate_schema(executable.into(), output.into(), false).await?;
            print_report(&report)?;
        }
        [command, executable, output, flag] if command == "schema" && flag == "--experimental" => {
            let report = generate_schema(executable.into(), output.into(), true).await?;
            print_report(&report)?;
        }
        [command, executable] if command == "handshake" => {
            let report = lifecycle_probe(executable.into(), LifecycleMode::Handshake).await?;
            print_report(&report)?;
        }
        [command, executable] if command == "thread-start" => {
            let report = lifecycle_probe(executable.into(), LifecycleMode::ThreadStart).await?;
            print_report(&report)?;
        }
        [command, executable] if command == "resume-smoke" => {
            let report = persistent_resume_probe(executable.into()).await?;
            print_report(&report)?;
        }
        [command, executable, permission]
            if command == "turn-smoke" && permission == "--allow-model-usage" =>
        {
            let report = lifecycle_probe(executable.into(), LifecycleMode::TurnSmoke).await?;
            print_report(&report)?;
        }
        [command, executable, permission]
            if command == "interrupt-smoke" && permission == "--allow-model-usage" =>
        {
            let report = lifecycle_probe(executable.into(), LifecycleMode::InterruptSmoke).await?;
            print_report(&report)?;
        }
        [command, bundle] if command == "fingerprint" => {
            let report = fingerprint_bundle(bundle.into())?;
            print_report(&report)?;
        }
        _ => return Err(ProbeError::Usage),
    }
    Ok(())
}

fn fingerprint_bundle(bundle: PathBuf) -> Result<FingerprintReport, ProbeError> {
    require_absolute(&bundle)?;
    let bytes = std::fs::read(bundle)?;
    Ok(FingerprintReport {
        kind: "fingerprint",
        bundle_bytes: bytes.len(),
        bundle_raw_sha256: hex::encode(Sha256::digest(&bytes)),
        bundle_canonical_sha256: canonical_json_sha256(&bytes)?,
    })
}

async fn generate_schema(
    executable: PathBuf,
    output: PathBuf,
    experimental: bool,
) -> Result<SchemaReport, ProbeError> {
    require_absolute(&executable)?;
    require_empty_output_directory(&output)?;
    std::fs::create_dir_all(&output)?;

    let mut args = vec![
        "app-server".into(),
        "generate-json-schema".into(),
        "--out".into(),
        output.to_string_lossy().into_owned(),
    ];
    if experimental {
        args.push("--experimental".into());
    }
    let generated = run_bounded(&process_spec(executable.clone(), args, SCHEMA_TIMEOUT)).await?;
    if !generated.success {
        return Err(ProbeError::CommandFailed {
            action: "schema generation",
            exit_code: generated.exit_code,
        });
    }

    let version = run_bounded(&process_spec(
        executable,
        vec!["--version".into()],
        HANDSHAKE_TIMEOUT,
    ))
    .await?;
    if !version.success {
        return Err(ProbeError::CommandFailed {
            action: "version probe",
            exit_code: version.exit_code,
        });
    }
    let bundle = output.join("codex_app_server_protocol.v2.schemas.json");
    let bytes = std::fs::read(&bundle)?;
    Ok(SchemaReport {
        kind: "schema",
        codex_version: version.stdout.trim().to_owned(),
        experimental,
        json_file_count: count_json_files(&output)?,
        bundle_bytes: bytes.len(),
        bundle_raw_sha256: hex::encode(Sha256::digest(&bytes)),
        bundle_canonical_sha256: canonical_json_sha256(&bytes)?,
    })
}

type ProbeTransport = JsonlTransport<ChildStdout, ChildStdin>;

struct AppServerProcess {
    child: Child,
    transport: ProbeTransport,
    stderr_task: JoinHandle<Result<(Vec<u8>, bool), ProbeError>>,
}

impl AppServerProcess {
    fn spawn(executable: &Path, isolated_codex_home: Option<&Path>) -> Result<Self, ProbeError> {
        let mut command = Command::new(executable);
        command
            .args(["app-server", "--listen", "stdio://"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        if let Some(codex_home) = isolated_codex_home {
            command.env("CODEX_HOME", codex_home);
        }
        let mut child = command
            .spawn()
            .map_err(|error| ProbeError::Spawn(error.to_string()))?;
        let stdin = child.stdin.take().ok_or(ProbeError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ProbeError::MissingPipe("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(ProbeError::MissingPipe("stderr"))?;
        Ok(Self {
            child,
            transport: JsonlTransport::new(stdout, stdin, OUTPUT_LIMIT),
            stderr_task: tokio::spawn(read_capped(stderr)),
        })
    }

    async fn shutdown(self) -> Result<StderrObservation, ProbeError> {
        let Self {
            mut child,
            transport,
            stderr_task,
        } = self;
        drop(transport);
        let _ = child.kill().await;
        let _ = child.wait().await;
        let (stderr, truncated) = stderr_task
            .await
            .map_err(|error| ProbeError::Io(error.to_string()))??;
        Ok(StderrObservation {
            present: !stderr.is_empty(),
            truncated,
        })
    }
}

async fn initialize_app_server(transport: &mut ProbeTransport) -> Result<Value, ProbeError> {
    let initialize = request_with_timeout(
        transport,
        json!(1),
        "initialize",
        json!({
            "clientInfo": {
                "name": "dogcodes-m0-probe",
                "title": "dogcodes M0 Probe",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": false }
        }),
    )
    .await?;
    let result = response_result(&initialize)?.clone();
    transport.send_notification("initialized", None).await?;
    Ok(result)
}

async fn lifecycle_probe(
    executable: PathBuf,
    mode: LifecycleMode,
) -> Result<HandshakeReport, ProbeError> {
    require_absolute(&executable)?;
    let mut server = AppServerProcess::spawn(&executable, None)?;
    let lifecycle = async {
        let initialize_result = initialize_app_server(&mut server.transport).await?;

        let (thread_start, thread_resume, turn_smoke, interrupt_smoke) =
            if mode == LifecycleMode::Handshake {
                (None, None, None, None)
            } else {
                let exchange = request_with_timeout(
                    &mut server.transport,
                    json!(2),
                    "thread/start",
                    json!({
                        "cwd": "/tmp",
                        "approvalPolicy": "never",
                        "sandbox": "read-only",
                        "ephemeral": true
                    }),
                )
                .await?;
                let (thread_id, _, thread_report) = summarize_thread_start(&exchange)?;
                let resume_exchange = request_with_timeout(
                    &mut server.transport,
                    json!(3),
                    "thread/resume",
                    json!({ "threadId": thread_id.clone() }),
                )
                .await?;
                let resume_report = summarize_thread_resume(&resume_exchange, &thread_id)?;
                let turn_report = if mode == LifecycleMode::TurnSmoke {
                    Some(run_turn_smoke(&mut server.transport, &thread_id).await?)
                } else {
                    None
                };
                let interrupt_report = if mode == LifecycleMode::InterruptSmoke {
                    Some(run_interrupt_smoke(&mut server.transport, &thread_id).await?)
                } else {
                    None
                };
                (
                    Some(thread_report),
                    Some(resume_report),
                    turn_report,
                    interrupt_report,
                )
            };
        Ok::<_, ProbeError>((
            initialize_result,
            thread_start,
            thread_resume,
            turn_smoke,
            interrupt_smoke,
        ))
    }
    .await;
    let stderr = server.shutdown().await?;

    let (result, thread_start, thread_resume, turn_smoke, interrupt_smoke) = lifecycle?;
    Ok(HandshakeReport {
        kind: "handshake",
        initialize: InitializeReport {
            response_received: true,
            notification_sent: true,
        },
        user_agent_product: result
            .get("userAgent")
            .and_then(Value::as_str)
            .map(sanitize_user_agent),
        platform_family: result
            .get("platformFamily")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        platform_os: result
            .get("platformOs")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        observations: RuntimeObservations {
            codex_home_present: result.get("codexHome").and_then(Value::as_str).is_some(),
            stderr_present: stderr.present,
            stderr_truncated: stderr.truncated,
        },
        thread_start,
        thread_resume,
        turn_smoke,
        interrupt_smoke,
    })
}

async fn persistent_resume_probe(
    executable: PathBuf,
) -> Result<PersistentResumeReport, ProbeError> {
    require_absolute(&executable)?;
    let isolated_home = tempfile::Builder::new()
        .prefix("dogcodes-codex-resume-")
        .tempdir_in(std::env::temp_dir())?;

    let mut starter = AppServerProcess::spawn(&executable, Some(isolated_home.path()))?;
    let start_result = async {
        let initialize = initialize_app_server(&mut starter.transport).await?;
        require_isolated_codex_home(&initialize, isolated_home.path())?;
        let exchange = request_with_timeout(
            &mut starter.transport,
            json!(2),
            "thread/start",
            json!({
                "cwd": "/tmp",
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "ephemeral": false
            }),
        )
        .await?;
        let (thread_id, thread_path, start) = summarize_thread_start(&exchange)?;
        let persistence_exchange = request_with_timeout(
            &mut starter.transport,
            json!(3),
            "thread/name/set",
            json!({
                "threadId": thread_id.clone(),
                "name": "dogcodes M0 resume probe"
            }),
        )
        .await?;
        response_result(&persistence_exchange)?;
        let state_evidence = wait_for_isolated_thread_path(
            thread_path.as_deref(),
            isolated_home.path(),
            Duration::from_secs(1),
        )
        .await;
        Ok::<_, ProbeError>((
            thread_id,
            start,
            PersistenceTriggerReport {
                method: "thread/name/set",
                response_received: true,
                interleaved_methods: interleaved_methods(&persistence_exchange),
            },
            state_evidence,
        ))
    }
    .await;
    let start_process = starter.shutdown().await?;
    let (thread_id, start, persistence_trigger, state_evidence) = start_result?;

    let mut resumer = AppServerProcess::spawn(&executable, Some(isolated_home.path()))?;
    let resume_result = async {
        let initialize = initialize_app_server(&mut resumer.transport).await?;
        require_isolated_codex_home(&initialize, isolated_home.path())?;
        let exchange = request_with_timeout(
            &mut resumer.transport,
            json!(2),
            "thread/resume",
            json!({ "threadId": thread_id.clone() }),
        )
        .await?;
        summarize_thread_resume(&exchange, &thread_id)
    }
    .await;
    let resume_process = resumer.shutdown().await?;
    let resume = resume_result?;

    Ok(PersistentResumeReport {
        kind: "persistent_resume",
        isolated_codex_home: true,
        initialize_count: 2,
        start,
        persistence_trigger,
        state_evidence,
        start_process,
        resume,
        resume_process,
    })
}

async fn run_turn_smoke<R, W>(
    transport: &mut JsonlTransport<R, W>,
    thread_id: &str,
) -> Result<TurnSmokeReport, ProbeError>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let exchange = request_with_timeout(
        transport,
        json!(4),
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": "Reply with exactly DOGCODES_SMOKE_OK. Do not call tools."
            }]
        }),
    )
    .await?;
    let (_, initial_status) = turn_start_identity(&exchange)?;
    let observations = timeout(
        TURN_TIMEOUT,
        collect_turn_observations(transport, exchange.interleaved),
    )
    .await
    .map_err(|_| ProbeError::TurnTimedOut)??;
    Ok(TurnSmokeReport {
        response_received: true,
        initial_status,
        observations,
    })
}

async fn run_interrupt_smoke<R, W>(
    transport: &mut JsonlTransport<R, W>,
    thread_id: &str,
) -> Result<InterruptSmokeReport, ProbeError>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let start_exchange = request_with_timeout(
        transport,
        json!(4),
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": "Write consecutive integers, one per line, until stopped. Do not call tools."
            }]
        }),
    )
    .await?;
    let (turn_id, initial_status) = turn_start_identity(&start_exchange)?;
    let interrupt_exchange = request_with_timeout(
        transport,
        json!(5),
        "turn/interrupt",
        json!({ "threadId": thread_id, "turnId": turn_id }),
    )
    .await?;
    response_result(&interrupt_exchange)?;
    let interrupt_interleaved_methods = interleaved_methods(&interrupt_exchange);
    let mut interleaved = start_exchange.interleaved;
    interleaved.extend(interrupt_exchange.interleaved);
    let observations = timeout(
        TURN_TIMEOUT,
        collect_turn_observations(transport, interleaved),
    )
    .await
    .map_err(|_| ProbeError::TurnTimedOut)??;
    Ok(InterruptSmokeReport {
        turn_start_response_received: true,
        interrupt_response_received: true,
        initial_status,
        interrupt_interleaved_methods,
        observations,
    })
}

fn turn_start_identity(exchange: &RequestExchange) -> Result<(String, Option<String>), ProbeError> {
    let turn = response_result(exchange)?
        .get("turn")
        .ok_or_else(|| ProbeError::Protocol("turn/start response has no turn".into()))?;
    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProbeError::Protocol("turn/start response has no id".into()))?
        .to_owned();
    let status = turn
        .get("status")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok((turn_id, status))
}

async fn collect_turn_observations<R, W>(
    transport: &mut JsonlTransport<R, W>,
    interleaved: Vec<IncomingFrame>,
) -> Result<TurnObservations, ProbeError>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut collector = TurnCollector::default();
    for frame in interleaved {
        collector.observe(&frame);
    }
    while collector.final_status.is_none()
        && !collector
            .observed_capabilities
            .contains("interaction_request")
    {
        collector.observe(&transport.read_frame().await?);
    }
    Ok(collector.finish())
}

#[derive(Default)]
struct TurnCollector {
    method_counts: BTreeMap<String, u64>,
    text_delta_bytes: u64,
    text_tail: String,
    observed_capabilities: BTreeSet<String>,
    final_status: Option<String>,
}

impl TurnCollector {
    fn observe(&mut self, frame: &IncomingFrame) {
        if let Some(method) = frame_method(frame) {
            *self.method_counts.entry(method.to_owned()).or_default() += 1;
        }
        let Some(event) = normalize(frame) else {
            return;
        };
        match event {
            NormalizedEvent::TextDelta { text, .. } => self.observe_text(&text),
            NormalizedEvent::DiffUpdated { .. } => {
                self.observed_capabilities
                    .insert("diff_notification".into());
            }
            NormalizedEvent::UsageUpdated { .. } => {
                self.observed_capabilities
                    .insert("usage_notification".into());
            }
            NormalizedEvent::InteractionRequested { .. } => {
                self.observed_capabilities
                    .insert("interaction_request".into());
            }
            NormalizedEvent::TurnCompleted { outcome, .. } => self.final_status = outcome,
            NormalizedEvent::TurnStarted { .. }
            | NormalizedEvent::VendorExtension { .. }
            | NormalizedEvent::ProtocolWarning { .. } => {}
        }
    }

    fn observe_text(&mut self, text: &str) {
        self.text_delta_bytes = self
            .text_delta_bytes
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
        let combined = format!("{}{text}", self.text_tail);
        if combined.contains(SMOKE_MARKER) {
            self.observed_capabilities
                .insert("expected_text_marker".into());
        }
        self.text_tail = combined
            .chars()
            .rev()
            .take(SMOKE_MARKER.len().saturating_sub(1))
            .collect::<String>()
            .chars()
            .rev()
            .collect();
    }

    fn finish(self) -> TurnObservations {
        TurnObservations {
            method_counts: self.method_counts,
            text_delta_bytes: self.text_delta_bytes,
            observed_capabilities: self.observed_capabilities,
            final_status: self.final_status,
        }
    }
}

fn frame_method(frame: &IncomingFrame) -> Option<&str> {
    match frame {
        IncomingFrame::Notification { method, .. }
        | IncomingFrame::ServerRequest { method, .. } => Some(method),
        IncomingFrame::Response { .. } | IncomingFrame::Unknown { .. } => None,
    }
}

async fn request_with_timeout<R, W>(
    transport: &mut JsonlTransport<R, W>,
    id: Value,
    method: &str,
    params: Value,
) -> Result<RequestExchange, ProbeError>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    timeout(HANDSHAKE_TIMEOUT, transport.request(id, method, params))
        .await
        .map_err(|_| ProbeError::TimedOut)?
        .map_err(ProbeError::from)
}

fn response_result(exchange: &RequestExchange) -> Result<&Value, ProbeError> {
    match &exchange.response {
        IncomingFrame::Response {
            result: Some(result),
            error: None,
            ..
        } => Ok(result),
        IncomingFrame::Response { error: Some(_), .. } => Err(ProbeError::Protocol(
            "app-server returned a JSON-RPC error".into(),
        )),
        _ => Err(ProbeError::Protocol(
            "app-server response has neither result nor error".into(),
        )),
    }
}

fn summarize_thread_start(
    exchange: &RequestExchange,
) -> Result<(String, Option<String>, ThreadStartReport), ProbeError> {
    let result = response_result(exchange)?;
    let thread = result
        .get("thread")
        .ok_or_else(|| ProbeError::Protocol("thread/start response has no thread".into()))?;
    let thread_id = thread
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProbeError::Protocol("thread/start response has no id".into()))?
        .to_owned();
    let thread_path = thread
        .get("path")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok((
        thread_id,
        thread_path,
        ThreadStartReport {
            response_received: true,
            thread_id_present: true,
            ephemeral: thread.get("ephemeral").and_then(Value::as_bool),
            path_present: thread.get("path").and_then(Value::as_str).is_some(),
            status: thread_status(thread),
            interleaved_methods: interleaved_methods(exchange),
        },
    ))
}

async fn wait_for_isolated_thread_path(
    thread_path: Option<&str>,
    isolated_home: &Path,
    deadline: Duration,
) -> ThreadStateEvidence {
    let Some(thread_path) = thread_path.map(Path::new) else {
        return ThreadStateEvidence {
            path_scoped_to_isolated_home: false,
            path_existed_before_shutdown: false,
        };
    };
    let canonical_home = std::fs::canonicalize(isolated_home).ok();
    let scoped = thread_path.starts_with(isolated_home)
        || canonical_home
            .as_deref()
            .is_some_and(|home| thread_path.starts_with(home));
    if !scoped {
        return ThreadStateEvidence {
            path_scoped_to_isolated_home: false,
            path_existed_before_shutdown: false,
        };
    }
    let appeared = timeout(deadline, async {
        loop {
            if thread_path.is_file() {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    ThreadStateEvidence {
        path_scoped_to_isolated_home: true,
        path_existed_before_shutdown: appeared,
    }
}

fn require_isolated_codex_home(
    initialize_result: &Value,
    expected_home: &Path,
) -> Result<(), ProbeError> {
    let reported_home = initialize_result
        .get("codexHome")
        .and_then(Value::as_str)
        .ok_or_else(|| ProbeError::Protocol("initialize response has no codexHome".into()))?;
    let reported_home = std::fs::canonicalize(reported_home).map_err(|_| {
        ProbeError::Protocol("reported codexHome could not be canonicalized".into())
    })?;
    let expected_home = std::fs::canonicalize(expected_home).map_err(|_| {
        ProbeError::Protocol("isolated codexHome could not be canonicalized".into())
    })?;
    if reported_home != expected_home {
        return Err(ProbeError::Protocol(
            "app-server did not honor the isolated codexHome".into(),
        ));
    }
    Ok(())
}

fn summarize_thread_resume(
    exchange: &RequestExchange,
    expected_thread_id: &str,
) -> Result<ThreadResumeReport, ProbeError> {
    let interleaved_methods = interleaved_methods(exchange);
    match &exchange.response {
        IncomingFrame::Response {
            result: Some(result),
            error: None,
            ..
        } => {
            let thread = result.get("thread").ok_or_else(|| {
                ProbeError::Protocol("thread/resume response has no thread".into())
            })?;
            let thread_id = thread
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ProbeError::Protocol("thread/resume response has no id".into()))?;
            Ok(ThreadResumeReport {
                response_received: true,
                accepted: true,
                error_code: None,
                error_kind: None,
                thread_id_present: Some(true),
                identity_matched: Some(thread_id == expected_thread_id),
                status: thread_status(thread),
                turn_count: thread.get("turns").and_then(Value::as_array).map(Vec::len),
                interleaved_methods,
            })
        }
        IncomingFrame::Response {
            error: Some(error), ..
        } => Ok(ThreadResumeReport {
            response_received: true,
            accepted: false,
            error_code: safe_json_rpc_error_code(error),
            error_kind: safe_json_rpc_error_kind(error).map(ToOwned::to_owned),
            thread_id_present: None,
            identity_matched: None,
            status: None,
            turn_count: None,
            interleaved_methods,
        }),
        _ => Err(ProbeError::Protocol(
            "thread/resume response has neither result nor error".into(),
        )),
    }
}

fn safe_json_rpc_error_code(error: &Value) -> Option<String> {
    let code = error.get("code")?;
    code.as_str()
        .map(ToOwned::to_owned)
        .or_else(|| code.as_i64().map(|value| value.to_string()))
        .or_else(|| code.as_u64().map(|value| value.to_string()))
}

fn safe_json_rpc_error_kind(error: &Value) -> Option<&'static str> {
    let message = error.get("message")?.as_str()?.to_ascii_lowercase();
    if message.contains("already loaded") || message.contains("already active") {
        Some("already_loaded")
    } else if message.contains("not found") || message.contains("no rollout") {
        Some("not_found")
    } else if message.contains("no turn") || message.contains("empty thread") {
        Some("empty_thread")
    } else if message.contains("failed to load") || message.contains("load thread") {
        Some("load_failed")
    } else if message.contains("state db") || message.contains("state database") {
        Some("state_database")
    } else if message.contains("invalid") {
        Some("invalid_request")
    } else {
        Some("other")
    }
}

fn interleaved_methods(exchange: &RequestExchange) -> Vec<String> {
    exchange
        .interleaved
        .iter()
        .filter_map(|frame| match frame {
            IncomingFrame::Notification { method, .. }
            | IncomingFrame::ServerRequest { method, .. } => Some(method.clone()),
            IncomingFrame::Response { .. } | IncomingFrame::Unknown { .. } => None,
        })
        .collect()
}

fn thread_status(thread: &Value) -> Option<String> {
    let status = thread.get("status")?;
    status.as_str().map(ToOwned::to_owned).or_else(|| {
        status
            .get("type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn sanitize_user_agent(value: &str) -> String {
    value.split(" (").next().unwrap_or(value).to_owned()
}

fn process_spec(executable: PathBuf, args: Vec<String>, deadline: Duration) -> ProcessSpec {
    ProcessSpec {
        executable,
        args,
        env_allowlist: BTreeSet::from(["PATH".into()]),
        env_overrides: BTreeMap::default(),
        cwd: None,
        timeout: deadline,
        stdout_limit: OUTPUT_LIMIT,
        stderr_limit: OUTPUT_LIMIT,
    }
}

async fn read_capped<R>(reader: R) -> Result<(Vec<u8>, bool), ProbeError>
where
    R: AsyncRead + Unpin,
{
    let mut reader = reader;
    let mut kept = Vec::with_capacity(8 * 1_024);
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1_024];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok((kept, truncated));
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
        let retained = remaining.min(count);
        kept.extend_from_slice(&buffer[..retained]);
        truncated |= retained < count;
    }
}

fn require_absolute(path: &Path) -> Result<(), ProbeError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ProbeError::InvalidPath(path.to_path_buf()))
    }
}

fn require_empty_output_directory(path: &Path) -> Result<(), ProbeError> {
    require_absolute(path)?;
    if path.exists() && std::fs::read_dir(path)?.next().is_some() {
        return Err(ProbeError::OutputNotEmpty(path.to_path_buf()));
    }
    Ok(())
}

fn count_json_files(root: &Path) -> Result<usize, std::io::Error> {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                count += 1;
            }
        }
    }
    Ok(count)
}

fn canonical_json_sha256(bytes: &[u8]) -> Result<String, ProbeError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let mut canonical = Vec::with_capacity(bytes.len());
    write_canonical_json(&value, &mut canonical)?;
    Ok(hex::encode(Sha256::digest(&canonical)))
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), ProbeError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => serde_json::to_writer(output, value)?,
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn print_report(report: &impl Serialize) -> Result<(), ProbeError> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaReport {
    kind: &'static str,
    codex_version: String,
    experimental: bool,
    json_file_count: usize,
    bundle_bytes: usize,
    bundle_raw_sha256: String,
    bundle_canonical_sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandshakeReport {
    kind: &'static str,
    initialize: InitializeReport,
    user_agent_product: Option<String>,
    platform_family: Option<String>,
    platform_os: Option<String>,
    observations: RuntimeObservations,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_start: Option<ThreadStartReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_resume: Option<ThreadResumeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_smoke: Option<TurnSmokeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interrupt_smoke: Option<InterruptSmokeReport>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistentResumeReport {
    kind: &'static str,
    isolated_codex_home: bool,
    initialize_count: u8,
    start: ThreadStartReport,
    persistence_trigger: PersistenceTriggerReport,
    state_evidence: ThreadStateEvidence,
    start_process: StderrObservation,
    resume: ThreadResumeReport,
    resume_process: StderrObservation,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistenceTriggerReport {
    method: &'static str,
    response_received: bool,
    interleaved_methods: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadStateEvidence {
    path_scoped_to_isolated_home: bool,
    path_existed_before_shutdown: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeReport {
    response_received: bool,
    notification_sent: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeObservations {
    codex_home_present: bool,
    stderr_present: bool,
    stderr_truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StderrObservation {
    present: bool,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadStartReport {
    response_received: bool,
    thread_id_present: bool,
    ephemeral: Option<bool>,
    path_present: bool,
    status: Option<String>,
    interleaved_methods: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResumeReport {
    response_received: bool,
    accepted: bool,
    error_code: Option<String>,
    error_kind: Option<String>,
    thread_id_present: Option<bool>,
    identity_matched: Option<bool>,
    status: Option<String>,
    turn_count: Option<usize>,
    interleaved_methods: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnSmokeReport {
    response_received: bool,
    initial_status: Option<String>,
    observations: TurnObservations,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InterruptSmokeReport {
    turn_start_response_received: bool,
    interrupt_response_received: bool,
    initial_status: Option<String>,
    interrupt_interleaved_methods: Vec<String>,
    observations: TurnObservations,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnObservations {
    method_counts: BTreeMap<String, u64>,
    text_delta_bytes: u64,
    observed_capabilities: BTreeSet<String>,
    final_status: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleMode {
    Handshake,
    ThreadStart,
    TurnSmoke,
    InterruptSmoke,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintReport {
    kind: &'static str,
    bundle_bytes: usize,
    bundle_raw_sha256: String,
    bundle_canonical_sha256: String,
}

#[derive(Debug, Error)]
enum ProbeError {
    #[error(transparent)]
    Fixture(#[from] FixtureError),
    #[error(
        "usage: codex-app-server-probe sanitize-fixture <absolute-input-jsonl> <absolute-new-output-jsonl> | check-sanitized-fixture <absolute-input-jsonl> | schema <absolute-executable> <absolute-empty-output-dir> [--experimental] | handshake <absolute-executable> | thread-start <absolute-executable> | resume-smoke <absolute-executable> | turn-smoke <absolute-executable> --allow-model-usage | interrupt-smoke <absolute-executable> --allow-model-usage | fingerprint <absolute-schema-bundle>"
    )]
    Usage,
    #[error("path must be absolute: {0}")]
    InvalidPath(PathBuf),
    #[error("schema output directory must be empty: {0}")]
    OutputNotEmpty(PathBuf),
    #[error("failed to spawn app-server: {0}")]
    Spawn(String),
    #[error("app-server did not expose {0}")]
    MissingPipe(&'static str),
    #[error("app-server handshake timed out")]
    TimedOut,
    #[error("app-server turn did not complete within the deadline")]
    TurnTimedOut,
    #[error("app-server protocol error: {0}")]
    Protocol(String),
    #[error("{action} exited unsuccessfully with code {exit_code:?}")]
    CommandFailed {
        action: &'static str,
        exit_code: Option<i32>,
    },
    #[error("process error: {0}")]
    Process(#[from] ProcessError),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
}

impl From<std::io::Error> for ProbeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_schema_hash_ignores_object_key_order() {
        let first = br#"{"definitions":{"B":2,"A":1},"type":"object"}"#;
        let second = br#"{"type":"object","definitions":{"A":1,"B":2}}"#;
        assert_eq!(
            canonical_json_sha256(first).unwrap(),
            canonical_json_sha256(second).unwrap()
        );
    }

    #[test]
    fn user_agent_report_drops_machine_details() {
        assert_eq!(
            sanitize_user_agent("probe/1.2.3 (Mac OS 14.7; arm64) details"),
            "probe/1.2.3"
        );
    }

    #[test]
    fn turn_collector_detects_a_marker_split_across_deltas() {
        let mut collector = TurnCollector::default();
        for frame in [
            json!({
                "method":"item/agentMessage/delta",
                "params":{"itemId":"item-1","delta":"DOGCODES_"}
            }),
            json!({
                "method":"item/agentMessage/delta",
                "params":{"itemId":"item-1","delta":"SMOKE_OK"}
            }),
            json!({
                "method":"thread/tokenUsage/updated",
                "params":{"tokenUsage":{"totalTokens":12}}
            }),
            json!({
                "method":"turn/completed",
                "params":{"turn":{"status":"completed","error":null}}
            }),
        ] {
            collector.observe(&codex_app_server_probe::transport::classify_frame(frame).unwrap());
        }
        let report = collector.finish();
        assert_eq!(report.text_delta_bytes, 17);
        assert!(
            report
                .observed_capabilities
                .contains("expected_text_marker")
        );
        assert!(report.observed_capabilities.contains("usage_notification"));
        assert_eq!(report.final_status.as_deref(), Some("completed"));
        assert_eq!(report.method_counts["item/agentMessage/delta"], 2);
    }

    #[test]
    fn rejected_resume_report_retains_only_the_error_code() {
        let exchange = RequestExchange {
            response: IncomingFrame::Response {
                id: json!(3),
                result: None,
                error: Some(json!({
                    "code": -32602,
                    "message": "thread secret-id not found"
                })),
                raw: Value::Null,
            },
            interleaved: vec![],
        };

        let report = summarize_thread_resume(&exchange, "secret-id").unwrap();
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!report.accepted);
        assert_eq!(report.error_code.as_deref(), Some("-32602"));
        assert_eq!(report.error_kind.as_deref(), Some("not_found"));
        assert!(!serialized.contains("secret-id"));
    }

    #[test]
    fn isolated_codex_home_must_match_the_initialize_response() {
        let expected = tempfile::tempdir().unwrap();
        let different = tempfile::tempdir().unwrap();
        let result = json!({ "codexHome": expected.path() });

        require_isolated_codex_home(&result, expected.path()).unwrap();
        assert!(require_isolated_codex_home(&result, different.path()).is_err());
    }

    #[tokio::test]
    async fn canonical_thread_path_is_accepted_inside_the_isolated_home() {
        let home = tempfile::tempdir().unwrap();
        let thread_path = home.path().join("thread.jsonl");
        std::fs::write(&thread_path, b"fixture").unwrap();
        let canonical_path = std::fs::canonicalize(&thread_path).unwrap();

        let evidence = wait_for_isolated_thread_path(
            canonical_path.to_str(),
            home.path(),
            Duration::from_millis(10),
        )
        .await;
        assert!(evidence.path_scoped_to_isolated_home);
        assert!(evidence.path_existed_before_shutdown);
    }
}
