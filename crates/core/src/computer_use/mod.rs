//! Task-scoped host-desktop capability. Core owns driver lifetime and cancellation.
mod activity;
mod runtime;
pub use activity::{ActivityMode, ComputerActivity};

use crate::{AppendRequest, Core, CoreError, CoreRepository};
use agent_domain::{CommandId, TaskId};
use agent_protocol::{EventEnvelope, EventSource, NewEvent, StreamRef, StreamType};
use driver_api::{AppMcp, ApprovalSemanticRole, DriverEvent};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, OwnedMutexGuard, watch},
};

const SETTINGS_EVENT: &str = "computer_use.settings_changed";

#[derive(Clone)]
struct Config {
    proxy: PathBuf,
    driver: PathBuf,
    home: PathBuf,
}

#[derive(Default)]
pub(crate) struct Host {
    config: Option<Config>,
    endpoints: Mutex<BTreeMap<TaskId, Arc<Endpoint>>>,
    desktop: Arc<Mutex<()>>,
    settings: Mutex<()>,
    external_input: Arc<AtomicUsize>,
    activity: Arc<activity::ActivityHub>,
}

pub(crate) struct ComputerLease {
    endpoint: Arc<Endpoint>,
    generation: u64,
}
impl ComputerLease {
    pub fn descriptor(&self) -> AppMcp {
        self.endpoint.descriptor.clone()
    }
    pub fn automatic_approval<'a>(&self, event: &'a DriverEvent) -> Option<&'a str> {
        if !self.endpoint.active.load(Ordering::Acquire) {
            return None;
        }
        let DriverEvent::ApprovalRequested {
            tool_name,
            command_summary,
            options,
            ..
        } = event
        else {
            return None;
        };
        if !crate::history_bridge::is_app_tool(
            &self.endpoint.descriptor.name,
            runtime::TOOLS,
            tool_name,
            command_summary,
        ) {
            return None;
        }
        options
            .iter()
            .find(|option| option.semantic_role == ApprovalSemanticRole::ApproveOnce)
            .map(|option| option.option_id.as_str())
    }
}
impl Drop for ComputerLease {
    fn drop(&mut self) {
        if self.endpoint.lease_generation.load(Ordering::Acquire) != self.generation {
            return;
        }
        self.endpoint.revoke();
        let endpoint = self.endpoint.clone();
        let generation = self.generation;
        // Retain desktop ownership until the old native process tree retires.
        tokio::spawn(async move {
            endpoint.close_generation(Some(generation)).await;
        });
    }
}

impl Core {
    /// Desktop builds require a painted on-screen warning before foreground input.
    #[must_use]
    pub fn with_computer_use_notice(self) -> Self {
        self.computer_use
            .activity
            .notice_required
            .store(true, Ordering::Release);
        self
    }

    pub fn subscribe_computer_use_activity(&self) -> watch::Receiver<ComputerActivity> {
        self.computer_use.activity.state.subscribe()
    }

    pub async fn computer_use_activity(&self, call: Value) -> Result<Value, CoreError> {
        match call["type"].as_str().unwrap_or("status") {
            "status" => Ok(
                serde_json::to_value(self.computer_use.activity.state.borrow().clone())
                    .map_err(|e| CoreError::Internal(e.to_string()))?,
            ),
            "presented" => Ok(
                json!({"accepted": self.computer_use.activity.presented(call["operationId"].as_str().unwrap_or(""))}),
            ),
            "stop" => {
                let current = self.computer_use.activity.state.borrow().clone();
                if current.operation_id.as_deref() != call["operationId"].as_str() {
                    return Ok(json!({"accepted":false}));
                }
                if let Some(task) = current.task_id {
                    self.computer_use_control(&task, json!({"type":"set","enabled":false}))
                        .await?;
                }
                Ok(json!({"accepted":true}))
            }
            _ => Err(CoreError::InvalidArgument(
                "unknown computer activity operation".into(),
            )),
        }
    }

    #[must_use]
    pub fn with_computer_use(mut self, proxy: PathBuf, driver: PathBuf, home: PathBuf) -> Self {
        self.computer_use.config = Some(Config {
            proxy,
            driver,
            home,
        });
        self
    }

    /// Phone input preempts agent input; read-only phone viewing does not.
    pub async fn begin_computer_use_manual_input(&self) -> ManualInput {
        self.computer_use
            .external_input
            .fetch_add(1, Ordering::AcqRel);
        let permit = ManualInput(self.computer_use.external_input.clone());
        let endpoints: Vec<_> = self
            .computer_use
            .endpoints
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        for endpoint in &endpoints {
            endpoint.revoke();
        }
        for endpoint in endpoints {
            endpoint.close().await;
        }
        permit
    }

    pub async fn computer_use_control(
        &self,
        task: &TaskId,
        call: Value,
    ) -> Result<Value, CoreError> {
        let _settings = self.computer_use.settings.lock().await;
        let summary = self.require_task(task).await?;
        match call["type"].as_str().unwrap_or("status") {
            "status" => (),
            "set" => {
                let enabled = call["enabled"].as_bool().ok_or_else(|| {
                    CoreError::InvalidArgument("computer-use enabled must be a boolean".into())
                })?;
                if enabled && summary.active_primary_run.is_some() {
                    return Err(CoreError::Conflict(
                        "Enable computer use before starting the next turn".into(),
                    ));
                }
                if enabled
                    && self
                        .computer_use
                        .config
                        .as_ref()
                        .is_none_or(|config| !config.driver.is_file())
                {
                    return Err(CoreError::CapabilityUnavailable(
                        "Computer-use driver is not installed in this build".into(),
                    ));
                }
                if !enabled {
                    self.revoke_computer_use(task).await;
                }
                self.append_runtime_event(
                    task,
                    SETTINGS_EVENT,
                    json!({"taskId":task,"enabled":enabled}),
                    EventSource::Hub,
                )
                .await?;
            }
            "stop" => {
                self.revoke_computer_use(task).await;
            }
            _ => {
                return Err(CoreError::InvalidArgument(
                    "unknown computer-use operation".into(),
                ));
            }
        }
        let enabled = self.computer_use_enabled(task).await?;
        let endpoints = self.computer_use.endpoints.lock().await;
        let active = endpoints
            .get(task)
            .is_some_and(|endpoint| endpoint.active.load(Ordering::Acquire));
        Ok(
            json!({"available":self.computer_use.config.as_ref().is_some_and(|config| config.driver.is_file()),"enabled":enabled,"active":active,"externalInput":self.computer_use.external_input.load(Ordering::Acquire) > 0}),
        )
    }

    async fn computer_use_enabled(&self, task: &TaskId) -> Result<bool, CoreError> {
        let through = self.repository.event_head().await?;
        let mut after = 0;
        let mut enabled = false;
        loop {
            let page = self
                .repository
                .task_events_of_types(task, after, through, &[SETTINGS_EVENT], 128)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.sequence;
            enabled = last.payload["enabled"].as_bool().unwrap_or(false);
            if page.len() < 128 {
                break;
            }
        }
        Ok(enabled)
    }

    pub(crate) async fn computer_use_lease(
        &self,
        task: &TaskId,
    ) -> Result<Option<ComputerLease>, CoreError> {
        let Some(config) = &self.computer_use.config else {
            return Ok(None);
        };
        if !self.computer_use_enabled(task).await? {
            return Ok(None);
        }
        let mut endpoints = self.computer_use.endpoints.lock().await;
        let endpoint = if let Some(endpoint) = endpoints.get(task) {
            endpoint.clone()
        } else {
            let endpoint = Endpoint::new(
                task.clone(),
                config.clone(),
                self.repository.clone(),
                self.events.clone(),
                self.computer_use.desktop.clone(),
                self.computer_use.external_input.clone(),
                self.computer_use.activity.clone(),
            )
            .await?;
            endpoints.insert(task.clone(), endpoint.clone());
            endpoint
        };
        let generation = endpoint.lease_generation.fetch_add(1, Ordering::AcqRel) + 1;
        endpoint.cancel.send_modify(|value| *value += 1);
        endpoint.active.store(true, Ordering::Release);
        Ok(Some(ComputerLease {
            endpoint,
            generation,
        }))
    }

    pub(crate) async fn revoke_computer_use(&self, task: &TaskId) {
        let endpoint = self.computer_use.endpoints.lock().await.get(task).cloned();
        if let Some(endpoint) = endpoint {
            endpoint.revoke();
            endpoint.close().await;
        }
    }

    pub(crate) async fn forget_deleted_computer_use(&self, task: &TaskId) {
        if let Some(endpoint) = self.computer_use.endpoints.lock().await.remove(task) {
            endpoint.revoke();
            endpoint.close().await;
        }
    }
}

pub struct ManualInput(Arc<AtomicUsize>);
impl Drop for ManualInput {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Endpoint {
    activity: Arc<activity::ActivityHub>,
    lease_generation: AtomicU64,
    desktop: Arc<Mutex<()>>,
    desktop_owner: std::sync::Mutex<Option<OwnedMutexGuard<()>>>,
    external_input: Arc<AtomicUsize>,
    descriptor: AppMcp,
    active: AtomicBool,
    cancel: watch::Sender<u64>,
    runtime: Mutex<Option<runtime::Driver>>,
    // Small retry cache; serial access is protected by runtime's mutex.
    receipts: std::sync::Mutex<Vec<(String, Value)>>,
    in_flight: std::sync::Mutex<Option<(Value, Value)>>,
    config: Config,
    task: TaskId,
    repository: Arc<dyn CoreRepository>,
    events: tokio::sync::broadcast::Sender<EventEnvelope>,
    _directory: tempfile::TempDir,
}

impl Endpoint {
    async fn new(
        task: TaskId,
        config: Config,
        repository: Arc<dyn CoreRepository>,
        events: tokio::sync::broadcast::Sender<EventEnvelope>,
        desktop: Arc<Mutex<()>>,
        external_input: Arc<AtomicUsize>,
        activity: Arc<activity::ActivityHub>,
    ) -> Result<Arc<Self>, CoreError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let token = uuid::Uuid::new_v4().to_string();
        let directory = tempfile::tempdir().map_err(|e| CoreError::Internal(e.to_string()))?;
        let descriptor = AppMcp {
            name: format!(
                "dogcodes_cu_{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            ),
            command: config.proxy.to_string_lossy().into_owned(),
            args: vec![
                "--computer-mcp".into(),
                listener
                    .local_addr()
                    .map_err(|e| CoreError::Internal(e.to_string()))?
                    .to_string(),
                token.clone(),
            ],
            directory: directory.path().to_string_lossy().into_owned(),
        };
        let endpoint = Arc::new(Self {
            activity,
            lease_generation: AtomicU64::new(0),
            desktop,
            desktop_owner: std::sync::Mutex::new(None),
            external_input,
            descriptor,
            active: AtomicBool::new(false),
            cancel: watch::channel(0).0,
            runtime: Mutex::new(None),
            receipts: std::sync::Mutex::new(vec![]),
            in_flight: std::sync::Mutex::new(None),
            config,
            task,
            repository,
            events,
            _directory: directory,
        });
        let weak = Arc::downgrade(&endpoint);
        tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_secs(30), listener.accept()).await;
                let Some(endpoint) = weak.upgrade() else {
                    break;
                };
                let Ok(Ok((mut socket, _))) = accepted else {
                    continue;
                };
                let token = token.clone();
                tokio::spawn(async move {
                    let result = async {
                        let mut line = String::new();
                        tokio::time::timeout(
                            Duration::from_secs(5),
                            BufReader::new((&mut socket).take(runtime::MAX_FRAME as u64 + 1))
                                .read_line(&mut line),
                        )
                        .await
                        .map_err(|_| "computer_proxy_timeout")?
                        .map_err(|_| "computer_proxy_read")?;
                        if line.len() > runtime::MAX_FRAME {
                            return Err("computer_proxy_frame_too_large".to_owned());
                        }
                        let input: Value = serde_json::from_str(&line)
                            .map_err(|_| "computer_proxy_invalid_json")?;
                        if input["token"] != token {
                            return Err("computer_proxy_unauthorized".into());
                        }
                        if input["cancel"].as_bool() == Some(true) {
                            let matches = endpoint
                                .in_flight
                                .lock()
                                .expect("computer in-flight")
                                .as_ref()
                                .is_some_and(|(client, id)| {
                                    *client == input["client"]
                                        && (input.get("requestId").is_none()
                                            || *id == input["requestId"])
                                });
                            if matches {
                                endpoint.cancel.send_modify(|generation| *generation += 1);
                            }
                            return Ok(json!({}));
                        }
                        endpoint.handle(&input).await
                    }
                    .await;
                    let reply = match result {
                        Ok(value) => json!({"result":value}),
                        Err(error) => json!({"error":error}),
                    };
                    if let Ok(mut bytes) = serde_json::to_vec(&reply) {
                        bytes.push(b'\n');
                        let _ = socket.write_all(&bytes).await;
                    }
                });
            }
        });
        Ok(endpoint)
    }

    fn revoke(&self) {
        self.active.store(false, Ordering::Release);
        self.cancel.send_modify(|generation| *generation += 1);
        self.receipts.lock().expect("computer receipts").clear();
    }

    async fn close(&self) {
        self.close_generation(None).await;
    }

    async fn close_generation(&self, generation: Option<u64>) {
        let mut slot = self.runtime.lock().await;
        if generation
            .is_some_and(|generation| generation != self.lease_generation.load(Ordering::Acquire))
        {
            return;
        }
        if let Some(mut driver) = slot.take() {
            let _ = driver.close().await;
        }
        self.desktop_owner.lock().expect("desktop owner").take();
    }

    async fn handle(&self, input: &Value) -> Result<Value, String> {
        let request = &input["request"];
        let method = request["method"].as_str().unwrap_or("");
        if method == "initialize" {
            return Ok(
                json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),"capabilities":{"tools":{}},"serverInfo":{"name":"dogcodes-computer","version":"1"},"instructions":runtime::INSTRUCTIONS}),
            );
        }
        if method == "ping" {
            return Ok(json!({}));
        }
        if method == "tools/call" {
            runtime::validate_call(
                request["params"]["name"].as_str().unwrap_or(""),
                request["params"].get("arguments").unwrap_or(&json!({})),
            )?;
        } else if method != "tools/list" {
            return Err("unsupported_computer_method".into());
        }
        if !self.active.load(Ordering::Acquire) {
            return Err("computer_use_inactive; enable for this task and start a turn".into());
        }
        let mut cancelled = self.cancel.subscribe();
        let generation = *cancelled.borrow();
        let key = format!("{generation}:{}:{}", input["client"], request["id"]);
        let mut slot = self.runtime.lock().await;
        if !self.active.load(Ordering::Acquire) || *cancelled.borrow() != generation {
            return Err("computer_use_cancelled".into());
        }
        if method == "tools/call" {
            if self.external_input.load(Ordering::Acquire) > 0 {
                return Err("manual_input_active; pause computer operations while the user controls the desktop".into());
            }
            let mut owner = self.desktop_owner.lock().expect("desktop owner");
            if owner.is_none() {
                *owner = Some(self.desktop.clone().try_lock_owned().map_err(|_| "desktop_busy; another task is operating the desktop; continue other work or retry later")?);
            }
        }
        if let Some((_, reply)) = self
            .receipts
            .lock()
            .expect("computer receipts")
            .iter()
            .find(|(id, _)| id == &key)
        {
            return Ok(reply.clone());
        }
        let started = std::time::Instant::now();
        *self.in_flight.lock().expect("computer in-flight") =
            Some((input["client"].clone(), request["id"].clone()));
        let activity = (method == "tools/call").then(|| {
            self.activity.begin(
                &self.task,
                request["params"]["name"].as_str().unwrap_or("unknown"),
                &request["params"]["arguments"],
            )
        });
        let planned_activity = activity
            .as_ref()
            .map(|_| self.activity.state.borrow().clone());
        let execution = async {
            if let Some(activity) = &activity {
                activity.before_input().await?;
            }
            if slot.is_none() {
                std::fs::create_dir_all(&self.config.home)
                    .map_err(|_| "computer_driver_home_unavailable")?;
                *slot = Some(runtime::Driver::spawn(&self.config.driver, &self.config.home).await?);
            }
            slot.as_mut()
                .ok_or("computer_driver_unavailable")?
                .call(method, request.get("params").cloned().unwrap_or(json!({})))
                .await
        };
        let result = tokio::select! { biased;
            _ = cancelled.changed() => Err("computer_use_cancelled; action may be partially complete; observe before retrying".into()),
            result = execution => result,
        };
        if result.is_err()
            && let Some(mut driver) = slot.take()
        {
            let _ = driver.close().await;
        }
        *self.in_flight.lock().expect("computer in-flight") = None;
        // On cancellation the old process has retired before we announce idle.
        drop(activity);
        if let Ok(value) = &result {
            let mut receipts = self.receipts.lock().expect("computer receipts");
            if receipts.len() == 4 {
                receipts.remove(0);
            }
            receipts.push((key, value.clone()));
        }
        drop(slot);
        if method == "tools/call" {
            let tool = request["params"]["name"].as_str().unwrap_or("unknown");
            self.audit(
                tool,
                result.as_ref().ok(),
                started.elapsed().as_millis(),
                planned_activity.as_ref(),
            )
            .await;
        }
        result
    }

    async fn audit(
        &self,
        tool: &str,
        result: Option<&Value>,
        elapsed: u128,
        activity: Option<&ComputerActivity>,
    ) {
        let id = CommandId::new();
        let images = result
            .and_then(|result| result["content"].as_array())
            .map_or(0, |content| {
                content
                    .iter()
                    .filter(|block| block["type"] == "image")
                    .count()
            });
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: self.task.to_string(),
            },
            "computer_use.action",
            json!({"taskId":self.task,"tool":tool,"elapsedMs":elapsed,"imageCount":images,"transportOk":result.is_some(),"isError":result.and_then(|result| result["isError"].as_bool()),"effect":result.and_then(|result| result.pointer("/structuredContent/effect")).filter(|value| value.is_string()),"mode":activity.map(|activity|activity.mode),"foregroundPossible":activity.map(|activity|activity.foreground_possible)}),
        );
        if let Ok(outcome) = self
            .repository
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: id.clone(),
                idempotency_key: id.to_string(),
                expected_stream_version: None,
                artifacts: vec![],
                events: vec![event],
            })
            .await
        {
            for event in outcome.events {
                let _ = self.events.send(event);
            }
        }
    }
}

/// Same executable as the app, dispatched before Tauri and the database open.
pub fn run_proxy_if_requested() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--computer-mcp") {
        return false;
    }
    if args.len() != 4 {
        eprintln!("computer MCP expects endpoint and token");
        return true;
    }
    let Ok(address) = args[2].parse::<std::net::SocketAddr>() else {
        return true;
    };
    if !address.ip().is_loopback() {
        return true;
    }
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        return true;
    };
    runtime.block_on(async move {
        let client = uuid::Uuid::new_v4().to_string();
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let output = Arc::new(Mutex::new(tokio::io::stdout()));
        let mut pending = tokio::task::JoinSet::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.len() > runtime::MAX_FRAME { break; }
            let Ok(request) = serde_json::from_str::<Value>(&line) else { continue; };
            if request["method"] == "notifications/cancelled" {
                let _ = proxy_query(address, json!({"token":args[3],"client":client,"cancel":true,"requestId":request["params"]["requestId"]})).await;
                continue;
            }
            let Some(id) = request.get("id").cloned() else { continue; };
            let input = json!({"token":args[3],"client":client,"request":request});
            let output = output.clone();
            pending.spawn(async move {
                let response = match proxy_query(address, input).await {
                    Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                    Err(error) => json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":error}]}}),
                };
                if let Ok(mut bytes) = serde_json::to_vec(&response) { bytes.push(b'\n'); let mut output = output.lock().await; let _ = output.write_all(&bytes).await; let _ = output.flush().await; }
            });
            while pending.try_join_next().is_some() {}
        }
        pending.abort_all();
        let _ = proxy_query(address, json!({"token":args[3],"client":client,"cancel":true})).await;
    });
    true
}

async fn proxy_query(address: std::net::SocketAddr, input: Value) -> Result<Value, String> {
    tokio::time::timeout(Duration::from_secs(150), async {
        let mut socket = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|_| "computer_service_unavailable")?;
        let mut bytes = serde_json::to_vec(&input).map_err(|_| "computer_proxy_encode")?;
        bytes.push(b'\n');
        socket
            .write_all(&bytes)
            .await
            .map_err(|_| "computer_proxy_write")?;
        let mut line = String::new();
        BufReader::new(socket.take(runtime::MAX_FRAME as u64 + 1))
            .read_line(&mut line)
            .await
            .map_err(|_| "computer_proxy_read")?;
        if line.len() > runtime::MAX_FRAME {
            return Err("computer_proxy_frame_too_large".into());
        }
        let reply: Value =
            serde_json::from_str(&line).map_err(|_| "computer_proxy_invalid_result")?;
        if let Some(error) = reply["error"].as_str() {
            return Err(error.into());
        }
        Ok(reply["result"].clone())
    })
    .await
    .map_err(|_| {
        "computer_proxy_timeout; action outcome unknown; observe before retrying".to_owned()
    })?
}
