//! Saved SSH machines and their live connections. The service owns these so that
//! later phases can run agents over a connection after the window has gone away.
mod agent_installs;
mod agent_updates;
mod agents;

use agent_core::Core;
use agent_domain::{ProjectId, TargetId};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, ExecutionTarget, ExecutionTargets, InstallationStatus,
    RegisterProject, SshAuth, SshConnection,
};
use redaction::{DeterministicRedactor, SecretRedactor};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use target_link::{
    AskpassPrompt, ConnectError, ConnectErrorKind, ConnectObserver, ConnectRequest, ConnectStep,
    PromptKind, ShimCatalog, TargetInfo, TargetLink,
};
use tokio::{sync::oneshot, task::JoinHandle};

const MAX_LOG_LINES: usize = 200;
const MAX_LOG_LINE: usize = 512;
const SECRET_SERVICE: &str = "codes.dog.desktop.ssh";

/// Secrets pass through here but are never logged, so this type has no `Debug`.
#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TargetCall {
    List,
    Status {
        target_id: TargetId,
    },
    Save {
        target_id: Option<TargetId>,
        display_name: String,
        connection: SshConnection,
        /// The password or key passphrase typed in the form. Saving a host doesn't
        /// connect, so it is kept now; a wrong one still falls through to asking.
        #[serde(default)]
        secret: Option<String>,
    },
    Remove {
        target_id: TargetId,
    },
    Connect {
        target_id: TargetId,
        /// The password or key passphrase just typed in the connection form.
        secret: Option<String>,
    },
    Disconnect {
        target_id: TargetId,
    },
    /// Looks for agents again on an open connection, after installing one by hand.
    Rescan {
        target_id: TargetId,
    },
    AgentUpdate {
        target_id: TargetId,
        installation_id: String,
        #[serde(default)]
        force: bool,
    },
    UpgradeAgent {
        target_id: TargetId,
        installation_id: String,
    },
    InstallAgent {
        target_id: TargetId,
        provider_id: String,
    },
    AgentInstalls {
        target_id: TargetId,
    },
    Answer {
        target_id: TargetId,
        prompt_id: String,
        answer: Option<String>,
        remember: bool,
    },
    ConfigHosts,
    ResolveHost {
        alias: String,
    },
    /// Subdirectories on the target; no path starts at the account's home.
    Browse {
        target_id: TargetId,
        path: Option<String>,
    },
    CreateProject {
        target_id: TargetId,
        path: String,
        display_name: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    #[default]
    Idle,
    Connecting,
    Connected,
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptView {
    id: String,
    kind: PromptKind,
    text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorView {
    kind: ConnectErrorKind,
    detail: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetStatus {
    phase: Phase,
    step: Option<ConnectStep>,
    log: Vec<String>,
    prompt: Option<PromptView>,
    info: Option<TargetInfo>,
    error: Option<ErrorView>,
    /// Why an established connection ended.
    closed_reason: Option<String>,
    /// This route now reaches a different machine or account than before.
    identity_changed: bool,
    /// Looking for agents on the machine after connecting.
    scanning: bool,
    agents: Vec<agents::TargetAgent>,
}

/// What was last found on a host, kept while nothing is connected to it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostAgent {
    installation_id: String,
    provider_id: String,
    executable: String,
    version: Option<String>,
    status: InstallationStatus,
    diagnostic: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetEntry {
    target: ExecutionTarget,
    status: TargetStatus,
    agents: Vec<HostAgent>,
}

/// Separate slots so a saved password is never offered as a key passphrase.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum SecretSlot {
    Password,
    Passphrase,
}

impl SecretSlot {
    fn of(kind: PromptKind) -> Option<Self> {
        match kind {
            PromptKind::Password => Some(Self::Password),
            PromptKind::Passphrase => Some(Self::Passphrase),
            PromptKind::HostKey | PromptKind::Other => None,
        }
    }
    fn of_auth(auth: SshAuth) -> Option<Self> {
        match auth {
            SshAuth::Password => Some(Self::Password),
            SshAuth::PrivateKey => Some(Self::Passphrase),
            SshAuth::System => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Passphrase => "passphrase",
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait SecretStore: Send + Sync {
    async fn get(&self, target: &TargetId, slot: SecretSlotName) -> Option<String>;
    async fn set(&self, target: &TargetId, slot: SecretSlotName, value: String) -> bool;
    async fn delete(&self, target: &TargetId, slot: SecretSlotName);
}

pub(crate) type SecretSlotName = &'static str;

struct SystemSecrets;

impl SystemSecrets {
    fn entry(target: &TargetId, slot: SecretSlotName) -> Option<keyring::Entry> {
        keyring::Entry::new(SECRET_SERVICE, &format!("{target}:{slot}")).ok()
    }
}

#[async_trait::async_trait]
impl SecretStore for SystemSecrets {
    async fn get(&self, target: &TargetId, slot: SecretSlotName) -> Option<String> {
        let entry = Self::entry(target, slot)?;
        tokio::task::spawn_blocking(move || entry.get_password().ok())
            .await
            .ok()?
    }
    async fn set(&self, target: &TargetId, slot: SecretSlotName, value: String) -> bool {
        let Some(entry) = Self::entry(target, slot) else {
            return false;
        };
        tokio::task::spawn_blocking(move || entry.set_password(&value).is_ok())
            .await
            .unwrap_or(false)
    }
    async fn delete(&self, target: &TargetId, slot: SecretSlotName) {
        if let Some(entry) = Self::entry(target, slot) {
            let _ = tokio::task::spawn_blocking(move || entry.delete_credential()).await;
        }
    }
}

struct AnswerValue {
    answer: Option<String>,
    remember: bool,
}

#[derive(Default)]
struct Connection {
    status: Mutex<TargetStatus>,
    waiting: Mutex<HashMap<String, oneshot::Sender<AnswerValue>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    rescan: Mutex<Option<JoinHandle<()>>>,
    link: Mutex<Option<Arc<TargetLink>>>,
}

impl Connection {
    fn update(&self, change: impl FnOnce(&mut TargetStatus)) {
        change(&mut self.status.lock().unwrap());
    }
    fn snapshot(&self) -> TargetStatus {
        self.status.lock().unwrap().clone()
    }
    fn stop(&self) {
        if let Some(worker) = self.worker.lock().unwrap().take() {
            worker.abort();
        }
        self.stop_rescan();
        self.waiting.lock().unwrap().clear();
        self.link.lock().unwrap().take();
    }
}

impl Connection {
    /// It holds the link, and its drivers would outlive a connection that has ended.
    fn stop_rescan(&self) {
        if let Some(rescan) = self.rescan.lock().unwrap().take() {
            rescan.abort();
        }
    }
}

fn log_line(line: &str) -> String {
    let text = DeterministicRedactor.redact(line).text;
    if text.len() <= MAX_LOG_LINE {
        return text;
    }
    let mut end = MAX_LOG_LINE;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// One attempt's view of the secrets: each is offered once, so a wrong saved
/// password falls through to asking the person instead of locking the account.
struct Attempt {
    connection: Arc<Connection>,
    target_id: TargetId,
    secrets: Arc<dyn SecretStore>,
    supplied: Mutex<Option<(SecretSlot, String)>>,
    tried: Mutex<BTreeSet<SecretSlot>>,
    /// Typed secrets stay here until the connection proves them correct.
    to_save: Mutex<Vec<(SecretSlot, String)>>,
}

impl Attempt {
    async fn known_secret(&self, slot: SecretSlot) -> Option<String> {
        if !self.tried.lock().unwrap().insert(slot) {
            return None;
        }
        let supplied = {
            let mut guard = self.supplied.lock().unwrap();
            if guard.as_ref().is_some_and(|(owner, _)| *owner == slot) {
                guard.take().map(|(_, value)| value)
            } else {
                None
            }
        };
        if let Some(value) = supplied {
            self.to_save.lock().unwrap().push((slot, value.clone()));
            return Some(value);
        }
        self.secrets.get(&self.target_id, slot.name()).await
    }

    async fn ask_person(&self, prompt: AskpassPrompt) -> Option<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (reply, answer) = oneshot::channel();
        self.connection
            .waiting
            .lock()
            .unwrap()
            .insert(id.clone(), reply);
        self.connection.update(|status| {
            status.prompt = Some(PromptView {
                id: id.clone(),
                kind: prompt.kind,
                text: prompt.text.clone(),
            });
        });
        let answered = answer.await.ok();
        self.connection.update(|status| {
            if status.prompt.as_ref().is_some_and(|view| view.id == id) {
                status.prompt = None;
            }
        });
        let answered = answered?;
        if answered.remember
            && let (Some(slot), Some(value)) = (SecretSlot::of(prompt.kind), &answered.answer)
        {
            self.to_save.lock().unwrap().push((slot, value.clone()));
        }
        answered.answer
    }

    async fn save_proven_secrets(&self) {
        let values = std::mem::take(&mut *self.to_save.lock().unwrap());
        for (slot, value) in values {
            if !self.secrets.set(&self.target_id, slot.name(), value).await {
                self.log("credential store: the secret could not be saved");
            }
        }
    }

    fn log(&self, line: &str) {
        ConnectObserver::log(self, line);
    }
}

#[async_trait::async_trait]
impl ConnectObserver for Attempt {
    fn step(&self, step: ConnectStep) {
        self.connection.update(|status| status.step = Some(step));
    }
    fn log(&self, line: &str) {
        let line = log_line(line);
        self.connection.update(|status| {
            status.log.push(line);
            if status.log.len() > MAX_LOG_LINES {
                status.log.remove(0);
            }
        });
    }
    async fn prompt(&self, prompt: AskpassPrompt) -> Option<String> {
        if let Some(slot) = SecretSlot::of(prompt.kind)
            && let Some(secret) = self.known_secret(slot).await
        {
            return Some(secret);
        }
        self.ask_person(prompt).await
    }
}

pub(crate) struct TargetHost {
    connections: Mutex<HashMap<TargetId, Arc<Connection>>>,
    edits: tokio::sync::Mutex<()>,
    secrets: Arc<dyn SecretStore>,
    shims: ShimCatalog,
    config_file: Option<PathBuf>,
    agent_updates: Mutex<HashMap<String, agent_updates::Remembered>>,
    agent_installs: Mutex<HashMap<TargetId, agent_installs::Installs>>,
}

impl Default for TargetHost {
    fn default() -> Self {
        Self::new(
            Arc::new(SystemSecrets),
            ShimCatalog::new(crate::computer_use::resource_directories("shim")),
            None,
        )
    }
}

impl TargetHost {
    fn new(
        secrets: Arc<dyn SecretStore>,
        shims: ShimCatalog,
        config_file: Option<PathBuf>,
    ) -> Self {
        Self {
            connections: Mutex::default(),
            edits: tokio::sync::Mutex::default(),
            secrets,
            shims,
            config_file,
            agent_updates: Mutex::default(),
            agent_installs: Mutex::default(),
        }
    }

    /// What a paired phone may know about the servers: their names and how
    /// their connection is doing. Addresses, accounts, keys, logs and error
    /// details stay on this computer.
    pub(crate) async fn phone_list(&self, core: &Core) -> Result<serde_json::Value, String> {
        let snapshot = core.execution_targets().await.map_err(|e| e.to_string())?;
        Ok(serde_json::Value::Array(
            snapshot
                .targets
                .targets
                .into_iter()
                .map(|target| {
                    let status = self.status(&target.target_id);
                    serde_json::json!({
                        "targetId": target.target_id,
                        "name": target.display_name,
                        "phase": status.phase,
                        "scanning": status.scanning,
                        // Typing a password or trusting a key happens here only.
                        "needsInput": status.prompt.is_some(),
                        "error": status.error.as_ref().map(|error| error.kind),
                    })
                })
                .collect(),
        ))
    }

    /// Reconnects a saved server for a paired phone. Nothing typed on the
    /// phone is used: a server that asks for a password or a new host key
    /// waits for someone at this computer.
    pub(crate) async fn phone_reconnect(
        self: &Arc<Self>,
        core: &Arc<Core>,
        target_id: TargetId,
    ) -> Result<(), String> {
        self.connect(core, target_id, None).await
    }

    pub(crate) fn has_connecting(&self) -> bool {
        self.connections
            .lock()
            .unwrap()
            .values()
            .any(|connection| connection.snapshot().phase == Phase::Connecting)
    }

    pub(crate) async fn call(
        self: &Arc<Self>,
        core: &Arc<Core>,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let call: TargetCall = serde_json::from_value(args).map_err(|e| e.to_string())?;
        let json =
            |value: Result<serde_json::Value, serde_json::Error>| value.map_err(|e| e.to_string());
        match call {
            TargetCall::List => json(serde_json::to_value(self.list(core).await?)),
            TargetCall::Status { target_id } => json(serde_json::to_value(self.status(&target_id))),
            TargetCall::Save {
                target_id,
                display_name,
                connection,
                secret,
            } => {
                let slot = SecretSlot::of_auth(connection.auth);
                let id = self.save(core, target_id, display_name, connection).await?;
                if let (Some(slot), Some(secret)) = (slot, secret.filter(|value| !value.is_empty()))
                {
                    // A store that refuses only means being asked when connecting.
                    let _ = self.secrets.set(&id, slot.name(), secret).await;
                }
                json(serde_json::to_value(id))
            }
            TargetCall::Remove { target_id } => {
                self.remove(core, &target_id).await?;
                Ok(serde_json::Value::Null)
            }
            TargetCall::Connect { target_id, secret } => {
                self.connect(core, target_id, secret).await?;
                Ok(serde_json::Value::Null)
            }
            TargetCall::Disconnect { target_id } => {
                self.disconnect(core, &target_id).await;
                Ok(serde_json::Value::Null)
            }
            TargetCall::Rescan { target_id } => {
                self.rescan(core, &target_id)?;
                Ok(serde_json::Value::Null)
            }
            TargetCall::AgentUpdate {
                target_id,
                installation_id,
                force,
            } => json(serde_json::to_value(
                self.agent_update(core, &target_id, &installation_id, force)
                    .await?,
            )),
            TargetCall::UpgradeAgent {
                target_id,
                installation_id,
            } => json(serde_json::to_value(
                self.upgrade_agent(core, &target_id, &installation_id)
                    .await?,
            )),
            TargetCall::InstallAgent {
                target_id,
                provider_id,
            } => json(serde_json::to_value(self.install_agent(
                core,
                &target_id,
                &provider_id,
            )?)),
            TargetCall::AgentInstalls { target_id } => json(serde_json::to_value(
                self.agent_installs(core, &target_id).await,
            )),
            TargetCall::Answer {
                target_id,
                prompt_id,
                answer,
                remember,
            } => {
                self.answer(&target_id, &prompt_id, answer, remember)?;
                Ok(serde_json::Value::Null)
            }
            TargetCall::Browse { target_id, path } => {
                let (path, directories) = self.browse(&target_id, path).await?;
                json(Ok(
                    serde_json::json!({ "path": path, "directories": directories }),
                ))
            }
            TargetCall::CreateProject {
                target_id,
                path,
                display_name,
            } => json(serde_json::to_value(
                self.create_project(core, &target_id, &path, &display_name)
                    .await?,
            )),
            TargetCall::ConfigHosts => json(serde_json::to_value(self.config_hosts())),
            TargetCall::ResolveHost { alias } => {
                let program =
                    target_link::find_ssh(&crate::computer_use::resource_directories("openssh"))
                        .await
                        .map_err(|e| e.to_string())?;
                json(serde_json::to_value(
                    target_link::ssh_config::resolve(&program, &alias).await,
                ))
            }
        }
    }

    fn status(&self, target: &TargetId) -> TargetStatus {
        self.connections
            .lock()
            .unwrap()
            .get(target)
            .map(|connection| connection.snapshot())
            .unwrap_or_default()
    }

    async fn list(&self, core: &Core) -> Result<Vec<TargetEntry>, String> {
        let snapshot = core.execution_targets().await.map_err(|e| e.to_string())?;
        let installations = core
            .bootstrap()
            .await
            .map_err(|e| e.to_string())?
            .installations;
        Ok(snapshot
            .targets
            .targets
            .into_iter()
            .map(|target| TargetEntry {
                status: self.status(&target.target_id),
                agents: installations
                    .iter()
                    .filter(|installation| {
                        installation.target_id.as_ref() == Some(&target.target_id)
                    })
                    .map(|installation| HostAgent {
                        installation_id: installation.installation_id.to_string(),
                        provider_id: installation.provider_id.clone(),
                        executable: installation.executable.clone(),
                        version: installation.version.clone(),
                        status: installation.status,
                        diagnostic: installation.diagnostic.clone(),
                    })
                    .collect(),
                target,
            })
            .collect())
    }

    /// Answers at once; the page follows `scanning`. A second request during a scan
    /// is the same request.
    fn rescan(&self, core: &Arc<Core>, target: &TargetId) -> Result<(), String> {
        let link = self.link(target)?;
        let connection = self
            .connections
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .ok_or("backendText.targetConnectionMissing")?;
        let mut started = false;
        connection.update(|status| {
            if status.phase == Phase::Connected && !status.scanning {
                status.scanning = true;
                started = true;
            }
        });
        if !started {
            return Ok(());
        }
        let (core, target, running) = (core.clone(), target.clone(), connection.clone());
        let scan = tokio::spawn(async move {
            // An installer usually adds its directory to the login shell's `PATH`.
            link.refresh_environment().await;
            let found = agents::scan(&core, &target, &link.target()).await;
            running.update(|status| {
                status.scanning = false;
                if let Some(found) = found {
                    status.agents = found;
                }
            });
        });
        *connection.rescan.lock().unwrap() = Some(scan);
        Ok(())
    }

    /// Read and write under one lock: the service is the only writer of this stream.
    async fn change(
        &self,
        core: &Arc<Core>,
        change: impl FnOnce(&mut Vec<ExecutionTarget>) -> Result<(), String>,
    ) -> Result<(), String> {
        let _edit = self.edits.lock().await;
        let snapshot = core.execution_targets().await.map_err(|e| e.to_string())?;
        let mut targets = snapshot.targets.targets;
        change(&mut targets)?;
        let mut envelope =
            CommandEnvelope::local(CoreCommand::SetExecutionTargets(ExecutionTargets {
                targets,
            }));
        envelope.actor = ActorRef::LocalUser;
        envelope.expected_stream_version = Some(snapshot.revision);
        core.execute(envelope).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn save(
        &self,
        core: &Arc<Core>,
        target_id: Option<TargetId>,
        display_name: String,
        connection: SshConnection,
    ) -> Result<TargetId, String> {
        let id = target_id.unwrap_or_default();
        let saved = id.clone();
        self.change(core, move |targets| {
            let existing = targets.iter_mut().find(|target| target.target_id == id);
            match existing {
                // The identity stays: it is what later detects a route to another machine.
                Some(target) => {
                    target.display_name = display_name;
                    target.connection = connection;
                }
                None => targets.push(ExecutionTarget {
                    target_id: id,
                    display_name,
                    connection,
                    machine_identity: None,
                }),
            }
            Ok(())
        })
        .await?;
        Ok(saved)
    }

    async fn remove(&self, core: &Arc<Core>, target: &TargetId) -> Result<(), String> {
        self.disconnect(core, target).await;
        self.connections.lock().unwrap().remove(target);
        let id = target.clone();
        self.change(core, move |targets| {
            targets.retain(|existing| existing.target_id != id);
            Ok(())
        })
        .await?;
        for slot in [SecretSlot::Password, SecretSlot::Passphrase] {
            self.secrets.delete(target, slot.name()).await;
        }
        Ok(())
    }

    pub(crate) async fn disconnect(&self, core: &Core, target: &TargetId) {
        let connection = self.connections.lock().unwrap().get(target).cloned();
        if let Some(connection) = connection {
            connection.stop();
            connection.update(|status| {
                status.phase = Phase::Idle;
                status.step = None;
                status.prompt = None;
                status.info = None;
                status.scanning = false;
                status.agents.clear();
            });
        }
        core.unregister_target_workspace(target);
        core.unregister_target_drivers(target).await;
    }

    fn link(&self, target: &TargetId) -> Result<Arc<TargetLink>, String> {
        self.connections
            .lock()
            .unwrap()
            .get(target)
            .and_then(|connection| connection.link.lock().unwrap().clone())
            .ok_or_else(|| "backendText.targetDeviceNotConnected".to_owned())
    }

    /// The machine a project on this target runs things on, while it is connected.
    pub(crate) fn target(&self, target: &TargetId) -> Result<agent_runtime::SharedTarget, String> {
        self.link(target)
            .map(|link| link.target())
            .map_err(|_| "backendText.projectTargetNotConnected".to_owned())
    }

    /// Where a shell opened on the host itself runs: the machine and its home directory.
    pub(crate) fn shell_place(
        &self,
        target: &TargetId,
    ) -> Result<(agent_runtime::SharedTarget, std::path::PathBuf), String> {
        let not_connected = || "backendText.targetHostNotConnected".to_owned();
        let link = self.link(target).map_err(|_| not_connected())?;
        let home = self.status(target).info.ok_or_else(not_connected)?.home;
        Ok((link.target(), home.into()))
    }

    async fn browse(
        &self,
        target: &TargetId,
        path: Option<String>,
    ) -> Result<(String, Vec<String>), String> {
        let link = self.link(target)?;
        let path = path.unwrap_or_else(|| link.info().home.clone());
        if !path.starts_with('/') {
            return Err("backendText.targetAbsolutePathRequired".into());
        }
        link.list_directory(&path).await
    }

    /// Checked on the machine itself, where the path has a meaning, then stored as
    /// resolved there. The same directory opened twice reuses its project.
    async fn create_project(
        &self,
        core: &Arc<Core>,
        target: &TargetId,
        path: &str,
        display_name: &str,
    ) -> Result<ProjectId, String> {
        let (path, _) = self.browse(target, Some(path.to_owned())).await?;
        let _edit = self.edits.lock().await;
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        if let Some(project) = snapshot
            .projects
            .iter()
            .find(|project| project.target_id.as_ref() == Some(target) && project.path == path)
        {
            return Ok(project.project_id.clone());
        }
        let project_id = ProjectId::new();
        let name = display_name.trim();
        let display_name = if name.is_empty() {
            path.rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or(&path)
                .to_owned()
        } else {
            name.to_owned()
        };
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: Some(target.clone()),
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name,
                path,
            },
        )))
        .await
        .map_err(|e| e.to_string())?;
        Ok(project_id)
    }

    fn answer(
        &self,
        target: &TargetId,
        prompt_id: &str,
        answer: Option<String>,
        remember: bool,
    ) -> Result<(), String> {
        let connections = self.connections.lock().unwrap();
        let connection = connections
            .get(target)
            .ok_or("backendText.targetConnectionMissing")?;
        let waiting = connection
            .waiting
            .lock()
            .unwrap()
            .remove(prompt_id)
            .ok_or("backendText.targetPromptExpired")?;
        let _ = waiting.send(AnswerValue { answer, remember });
        Ok(())
    }

    fn config_hosts(&self) -> Vec<String> {
        let Some(home) = agent_registry::home_directory() else {
            return vec![];
        };
        let config = self
            .config_file
            .clone()
            .unwrap_or_else(|| home.join(".ssh").join("config"));
        target_link::ssh_config::list_hosts(&config, &home)
    }

    async fn connect(
        self: &Arc<Self>,
        core: &Arc<Core>,
        target_id: TargetId,
        secret: Option<String>,
    ) -> Result<(), String> {
        let snapshot = core.execution_targets().await.map_err(|e| e.to_string())?;
        let target = snapshot
            .targets
            .targets
            .into_iter()
            .find(|target| target.target_id == target_id)
            .ok_or("backendText.targetRemoteConnectionMissing")?;
        let askpass_program = std::env::current_exe().map_err(|e| e.to_string())?;
        let connection = {
            let mut connections = self.connections.lock().unwrap();
            if let Some(existing) = connections.get(&target_id)
                && matches!(
                    existing.snapshot().phase,
                    Phase::Connecting | Phase::Connected
                )
            {
                return Ok(());
            }
            let connection = Arc::new(Connection {
                status: Mutex::new(TargetStatus {
                    phase: Phase::Connecting,
                    ..TargetStatus::default()
                }),
                ..Connection::default()
            });
            connections.insert(target_id.clone(), connection.clone());
            connection
        };
        let attempt = Arc::new(Attempt {
            connection: connection.clone(),
            target_id: target_id.clone(),
            secrets: self.secrets.clone(),
            supplied: Mutex::new(
                SecretSlot::of_auth(target.connection.auth)
                    .zip(secret.filter(|value| !value.is_empty())),
            ),
            tried: Mutex::default(),
            to_save: Mutex::default(),
        });
        let host = self.clone();
        let core = core.clone();
        let worker = tokio::spawn(async move {
            let request = async {
                Ok::<_, ConnectError>(ConnectRequest {
                    program: target_link::find_ssh(&crate::computer_use::resource_directories(
                        "openssh",
                    ))
                    .await?,
                    connection: target.connection.clone(),
                    askpass_program,
                    shims: host.shims.clone(),
                    config_file: host.config_file.clone(),
                })
            };
            let attempted = match request.await {
                Ok(request) => target_link::connect(request, attempt.clone()).await,
                Err(error) => Err(error),
            };
            match attempted {
                Ok(link) => {
                    attempt.save_proven_secrets().await;
                    host.connected(&core, &target, &attempt.connection, link)
                        .await;
                }
                Err(error) => {
                    attempt.connection.update(|status| {
                        // The step stays: it shows the person where the attempt stopped.
                        status.phase = Phase::Failed;
                        status.prompt = None;
                        status.error = Some(ErrorView {
                            kind: error.kind,
                            detail: log_line(&error.detail),
                        });
                    });
                }
            }
        });
        *connection.worker.lock().unwrap() = Some(worker);
        Ok(())
    }

    /// Holds the link for as long as the connection lives.
    async fn connected(
        &self,
        core: &Arc<Core>,
        target: &ExecutionTarget,
        connection: &Arc<Connection>,
        link: TargetLink,
    ) {
        let link = Arc::new(link);
        let info = link.info().clone();
        let identity = info.machine_identity.clone();
        let identity_changed = target
            .machine_identity
            .as_ref()
            .is_some_and(|known| *known != identity);
        connection.update(|status| {
            status.phase = Phase::Connected;
            status.info = Some(info);
            status.identity_changed = identity_changed;
            status.scanning = true;
        });
        *connection.link.lock().unwrap() = Some(link.clone());
        core.register_target_workspace(
            target.target_id.clone(),
            Arc::new(workspace_ops::operation::RemoteWorkspace::new(
                link.target(),
            )),
        );
        let (cleaning, cleaned) = (core.clone(), target.target_id.clone());
        tokio::spawn(async move { cleaning.clean_deleted_tasks_on(&cleaned).await });
        if target.machine_identity.is_none() {
            let id = target.target_id.clone();
            let _ = self
                .change(core, move |targets| {
                    if let Some(target) = targets.iter_mut().find(|t| t.target_id == id) {
                        target.machine_identity = Some(identity);
                    }
                    Ok(())
                })
                .await;
        }
        let found = agents::scan(core, &target.target_id, &link.target()).await;
        connection.update(|status| {
            status.scanning = false;
            status.agents = found.unwrap_or_default();
        });
        let mut closed = link.closed();
        drop(link);
        let reason = closed
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|reason| reason.clone());
        connection.stop_rescan();
        connection.update(|status| {
            status.phase = Phase::Disconnected;
            status.step = None;
            status.info = None;
            status.scanning = false;
            status.agents.clear();
            status.closed_reason = reason.map(|reason| log_line(&reason));
        });
        connection.link.lock().unwrap().take();
        core.unregister_target_workspace(&target.target_id);
        core.unregister_target_drivers(&target.target_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    fn key(target: &TargetId, slot: SecretSlotName) -> String {
        format!("{target}:{slot}")
    }

    #[async_trait::async_trait]
    impl SecretStore for MemorySecrets {
        async fn get(&self, target: &TargetId, slot: SecretSlotName) -> Option<String> {
            self.0.lock().unwrap().get(&key(target, slot)).cloned()
        }
        async fn set(&self, target: &TargetId, slot: SecretSlotName, value: String) -> bool {
            self.0.lock().unwrap().insert(key(target, slot), value);
            true
        }
        async fn delete(&self, target: &TargetId, slot: SecretSlotName) {
            self.0.lock().unwrap().remove(&key(target, slot));
        }
    }

    fn attempt(secrets: Arc<MemorySecrets>, supplied: Option<(SecretSlot, &str)>) -> Arc<Attempt> {
        Arc::new(Attempt {
            connection: Arc::default(),
            target_id: TargetId::new(),
            secrets,
            supplied: Mutex::new(supplied.map(|(slot, value)| (slot, value.to_owned()))),
            tried: Mutex::default(),
            to_save: Mutex::default(),
        })
    }

    fn prompt(text: &str) -> AskpassPrompt {
        AskpassPrompt::classify(text)
    }

    /// Answers whatever the attempt publishes as a question.
    fn answer_when_asked(attempt: &Arc<Attempt>, value: &'static str, remember: bool) {
        let attempt = attempt.clone();
        tokio::spawn(async move {
            loop {
                let id = attempt.connection.snapshot().prompt.map(|prompt| prompt.id);
                if let Some(id) = id {
                    let reply = attempt.connection.waiting.lock().unwrap().remove(&id);
                    if let Some(reply) = reply {
                        let _ = reply.send(AnswerValue {
                            answer: Some(value.to_owned()),
                            remember,
                        });
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
    }

    #[tokio::test]
    async fn a_typed_secret_is_used_once_and_saved_only_after_it_is_proven() {
        let secrets = Arc::new(MemorySecrets::default());
        let attempt = attempt(secrets.clone(), Some((SecretSlot::Password, "typed")));
        let id = attempt.target_id.clone();
        assert_eq!(
            attempt
                .prompt(prompt("dev@box's password: "))
                .await
                .as_deref(),
            Some("typed")
        );
        assert!(secrets.get(&id, "password").await.is_none());
        answer_when_asked(&attempt, "second", false);
        assert_eq!(
            attempt
                .prompt(prompt("dev@box's password: "))
                .await
                .as_deref(),
            Some("second")
        );
        attempt.save_proven_secrets().await;
        assert_eq!(secrets.get(&id, "password").await.as_deref(), Some("typed"));
    }

    #[tokio::test]
    async fn a_saved_password_that_fails_falls_through_to_asking() {
        let secrets = Arc::new(MemorySecrets::default());
        let attempt = attempt(secrets.clone(), None);
        secrets
            .set(&attempt.target_id, "password", "saved".into())
            .await;
        assert_eq!(
            attempt.prompt(prompt("password: ")).await.as_deref(),
            Some("saved")
        );
        answer_when_asked(&attempt, "new", true);
        assert_eq!(
            attempt.prompt(prompt("password: ")).await.as_deref(),
            Some("new")
        );
        attempt.save_proven_secrets().await;
        assert_eq!(
            secrets.get(&attempt.target_id, "password").await.as_deref(),
            Some("new")
        );
    }

    #[tokio::test]
    async fn a_password_is_never_offered_as_a_passphrase_or_host_key_answer() {
        let secrets = Arc::new(MemorySecrets::default());
        let attempt = attempt(secrets.clone(), Some((SecretSlot::Password, "typed")));
        answer_when_asked(&attempt, "yes", false);
        assert_eq!(
            attempt
                .prompt(prompt(
                    "Are you sure you want to continue connecting (yes/no)? "
                ))
                .await
                .as_deref(),
            Some("yes")
        );
        answer_when_asked(&attempt, "phrase", false);
        assert_eq!(
            attempt
                .prompt(prompt("Enter passphrase for key '/k': "))
                .await
                .as_deref(),
            Some("phrase")
        );
        attempt.save_proven_secrets().await;
        assert!(secrets.0.lock().unwrap().is_empty());
    }

    #[test]
    fn log_lines_are_bounded_on_a_character_boundary() {
        let line = log_line(&"密".repeat(1000));
        assert!(line.len() <= MAX_LOG_LINE + "…".len());
        assert!(line.ends_with('…'));
    }

    #[tokio::test]
    async fn saving_keeps_the_identity_and_removing_forgets_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let secrets = Arc::new(MemorySecrets::default());
        let host = TargetHost::new(secrets.clone(), ShimCatalog::default(), None);
        let connection = |destination: &str| SshConnection {
            destination: destination.into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: None,
            proxy: None,
        };
        let id = host
            .save(&core, None, "Build".into(), connection("old"))
            .await
            .unwrap();
        let recorded = id.clone();
        host.change(&core, move |targets| {
            targets[0].machine_identity = Some(format!("{recorded}"));
            Ok(())
        })
        .await
        .unwrap();
        host.save(&core, Some(id.clone()), "Build 2".into(), connection("new"))
            .await
            .unwrap();
        let listed = host.list(&core).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].target.connection.destination, "new");
        assert_eq!(listed[0].target.machine_identity, Some(id.to_string()));
        secrets.set(&id, "password", "secret".into()).await;
        host.remove(&core, &id).await.unwrap();
        assert!(host.list(&core).await.unwrap().is_empty());
        assert!(secrets.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn saving_a_host_keeps_the_secret_typed_for_its_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let secrets = Arc::new(MemorySecrets::default());
        let host = Arc::new(TargetHost::new(
            secrets.clone(),
            ShimCatalog::default(),
            None,
        ));
        let save = |auth: &str, secret: Option<&str>| {
            host.call(
                &core,
                serde_json::json!({
                    "type": "save", "targetId": null, "displayName": "Box",
                    "connection": { "destination": "box", "auth": auth },
                    "secret": secret,
                }),
            )
        };
        let id: TargetId =
            serde_json::from_value(save("password", Some("s3cret")).await.unwrap()).unwrap();
        assert_eq!(
            secrets
                .get(&id, SecretSlot::Password.name())
                .await
                .as_deref(),
            Some("s3cret")
        );
        // Nothing to keep: left empty, or the authentication takes no secret.
        let id: TargetId =
            serde_json::from_value(save("password", Some("")).await.unwrap()).unwrap();
        assert_eq!(secrets.get(&id, SecretSlot::Password.name()).await, None);
        let id: TargetId =
            serde_json::from_value(save("system", Some("s3cret")).await.unwrap()).unwrap();
        assert_eq!(secrets.get(&id, SecretSlot::Password.name()).await, None);
    }

    /// The whole service path against a private `sshd`, as the window would drive it:
    /// `cargo build -p target-shim && cargo test -p desktop --lib execution_targets -- --ignored`
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "needs an OpenSSH server binary"]
    async fn the_service_connects_records_the_machine_and_disconnects() {
        use std::process::{Command, Stdio};
        let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
            .expect("an OpenSSH server binary");
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        let text = |name: &str| path(name).to_str().unwrap().to_owned();
        for key in ["host_key", "client_key"] {
            assert!(
                Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f", &text(key)])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::copy(path("client_key.pub"), path("authorized_keys")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        std::fs::write(
            path("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nStrictModes no\nPasswordAuthentication no\n",
                text("host_key"),
                text("sshd.pid"),
                text("authorized_keys"),
            ),
        )
        .unwrap();
        let mut server = Command::new(sshd)
            .args(["-D", "-e", "-f", &text("sshd_config")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let host_key = std::fs::read_to_string(path("host_key.pub")).unwrap();
        let host_key = host_key
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(
            path("known_hosts"),
            format!("[127.0.0.1]:{port} {host_key}\n"),
        )
        .unwrap();
        let user =
            String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap();
        std::fs::write(
            path("ssh_config"),
            format!(
                "Host box\n  HostName 127.0.0.1\n  Port {port}\n  User {}\n  IdentityFile {}\n  IdentitiesOnly yes\n  IdentityAgent none\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n",
                user.trim(),
                text("client_key"),
                text("known_hosts"),
            ),
        )
        .unwrap();
        let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "aarch64-apple-darwin",
            ("macos", _) => "x86_64-apple-darwin",
            ("linux", "aarch64") => "aarch64-unknown-linux-musl",
            _ => "x86_64-unknown-linux-musl",
        };
        std::fs::create_dir_all(path("shims").join(triple)).unwrap();
        let built = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("dogcodes-shim");
        std::fs::copy(built, path("shims").join(triple).join("dogcodes-shim")).unwrap();

        let store = agent_store::SqliteStore::open(path("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let host = Arc::new(TargetHost::new(
            Arc::new(MemorySecrets::default()),
            ShimCatalog::new(vec![path("shims")]),
            Some(path("ssh_config")),
        ));
        let connection = SshConnection {
            destination: "box".into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: Some(text("install")),
            proxy: None,
        };
        let id: TargetId = serde_json::from_value(
            host.call(
                &core,
                serde_json::json!({"type":"save","targetId":null,"displayName":"Box","connection":connection}),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        host.call(
            &core,
            serde_json::json!({"type":"connect","targetId":id,"secret":null}),
        )
        .await
        .unwrap();
        assert!(host.has_connecting());
        let mut status = TargetStatus::default();
        for _ in 0..300 {
            status = host.status(&id);
            if status.phase != Phase::Connecting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(status.phase, Phase::Connected, "{:?}", status.error);
        assert!(!status.identity_changed);
        let identity = status.info.as_ref().unwrap().machine_identity.clone();
        let mut recorded = None;
        for _ in 0..50 {
            recorded = host.list(&core).await.unwrap()[0]
                .target
                .machine_identity
                .clone();
            if recorded.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(recorded, Some(identity));

        // Looking again on the open link finishes and leaves the connection up.
        for _ in 0..300 {
            if !host.status(&id).scanning {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Asked of the shim over the real connection, not just tolerated when it fails.
        let detected: agent_management::detect::Detected = agent_management::operation::call(
            &host.target(&id).unwrap(),
            &agent_management::operation::Request::Detect,
        )
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.message));
        let found = host.status(&id).agents.len();
        assert_eq!(detected.probes.len(), found);
        host.call(&core, serde_json::json!({"type":"rescan","targetId":id}))
            .await
            .unwrap();
        assert!(host.status(&id).scanning);
        for _ in 0..300 {
            if !host.status(&id).scanning {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(host.status(&id).phase, Phase::Connected);
        assert_eq!(host.status(&id).agents.len(), found);
        assert_eq!(host.list(&core).await.unwrap()[0].agents.len(), found);

        // Refused by the shim over the real connection, so this account's home stays as it is.
        host.call(
            &core,
            serde_json::json!({"type":"install_agent","targetId":id,"providerId":"zcode"}),
        )
        .await
        .unwrap();
        for _ in 0..300 {
            if host.agent_installs(&core, &id).await[0].status != "installing" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(host.agent_installs(&core, &id).await[0].status, "failed");
        assert_eq!(host.status(&id).phase, Phase::Connected);

        // A shell on the host itself, outside any project, in the account's home.
        let terminals = crate::terminal_host::TerminalHost::default();
        let shell: crate::terminal_host::TerminalInfo = serde_json::from_value(
            crate::terminal_host::rpc::dispatch(
                &core,
                &terminals,
                &host,
                "terminal_start_on_target",
                serde_json::json!({
                    "targetId": id,
                    "operationId": uuid::Uuid::new_v4().to_string(),
                    "cols": 80,
                    "rows": 24,
                }),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            shell.cwd,
            std::path::PathBuf::from(&status.info.as_ref().unwrap().home)
        );
        let scope = format!("target:{}", id.as_str());
        assert_eq!(terminals.list(&scope).unwrap().len(), 1);
        let mut output = Vec::new();
        let mut after = 0;
        let mut wrote = false;
        for _ in 0..100 {
            let batch = terminals.read(&shell.id, after).await.unwrap();
            after = batch.next;
            output.extend(batch.bytes);
            if batch.info.status == crate::terminal_host::TerminalStatus::Running && !wrote {
                terminals
                    .write(
                        &shell.id,
                        b"printf 'term=%s \\033[31mhost-shell-%s\\033[0m\\n' \"$TERM\" $((40 + 2))\n"
                            .to_vec(),
                    )
                    .await
                    .unwrap();
                wrote = true;
            }
            if String::from_utf8_lossy(&output).contains("host-shell-42") {
                break;
            }
        }
        // Colors reach the terminal as the shell wrote them.
        assert!(
            String::from_utf8_lossy(&output)
                .contains("term=xterm-256color \u{1b}[31mhost-shell-42\u{1b}[0m"),
            "{:?}",
            String::from_utf8_lossy(&output)
        );

        // The connection stays while a shell needs it, and ends with the last one.
        let release = || {
            crate::terminal_host::rpc::dispatch(
                &core,
                &terminals,
                &host,
                "terminal_release_target",
                serde_json::json!({ "targetId": id }),
            )
        };
        assert_eq!(release().await.unwrap()["released"], false);
        assert_eq!(host.status(&id).phase, Phase::Connected);
        terminals.close(&shell.id, &shell.epoch).await.unwrap();
        assert_eq!(release().await.unwrap()["released"], true);
        assert_eq!(host.status(&id).phase, Phase::Idle);
        // Stopped on purpose, not lost, so its tab doesn't come back.
        assert_eq!(
            terminals.get(&shell.id).unwrap().status,
            crate::terminal_host::TerminalStatus::Exited
        );

        host.call(
            &core,
            serde_json::json!({"type":"disconnect","targetId":id}),
        )
        .await
        .unwrap();
        assert_eq!(host.status(&id).phase, Phase::Idle);
        let marker = text("install");
        let mut running = true;
        for _ in 0..50 {
            running = !Command::new("pgrep")
                .args(["-f", &marker])
                .output()
                .unwrap()
                .stdout
                .is_empty();
            if !running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = server.kill();
        let _ = server.wait();
        assert!(!running, "the shim kept running after disconnecting");
    }

    #[tokio::test]
    async fn closing_the_last_shell_leaves_the_connection_to_a_hosts_projects() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let host = Arc::new(TargetHost::default());
        let terminals = crate::terminal_host::TerminalHost::default();
        let target = TargetId::new();
        let release = || {
            crate::terminal_host::rpc::dispatch(
                &core,
                &terminals,
                &host,
                "terminal_release_target",
                serde_json::json!({ "targetId": target }),
            )
        };
        assert_eq!(release().await.unwrap()["released"], true);
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: Some(target.clone()),
                additional_directories: vec![],
                project_id: ProjectId::new(),
                display_name: "Remote".into(),
                path: "/srv/app".into(),
            },
        )))
        .await
        .unwrap();
        assert_eq!(release().await.unwrap()["released"], false);
    }

    #[test]
    fn a_shell_on_a_host_needs_the_host_connected() {
        let error = TargetHost::default()
            .shell_place(&TargetId::new())
            .err()
            .unwrap();
        assert!(error.contains("backendText.targetHostNotConnected"));
    }

    #[tokio::test]
    async fn local_features_refuse_a_remote_project_instead_of_reading_this_machine() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        // The same path exists here, which is exactly what must not be reached.
        let same_path = directory.path().to_str().unwrap().to_owned();
        let project = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: Some(TargetId::new()),
                additional_directories: vec![],
                project_id: project.clone(),
                display_name: "Remote".into(),
                path: same_path,
            },
        )))
        .await
        .unwrap();
        let task = agent_domain::TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            agent_protocol::CreateTask {
                task_id: task.clone(),
                project_id: project,
                title: "Task".into(),
                preferred_executor: agent_domain::ExecutorRef {
                    provider_id: agent_domain::ProviderId::parse("kimi").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        assert!(
            crate::workspace_service::project_paths(&core, task.as_str())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_agent_on_another_machine_cannot_be_assigned_to_a_project() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                target_id: Some(TargetId::new()),
                provider_id: "kimi".into(),
                executable: "/root/.kimi-code/bin/kimi".into(),
                fingerprint: "f".into(),
                status: agent_protocol::InstallationStatus::Ready,
                version: Some("2.5.0".into()),
                version_text: None,
                diagnostic: None,
                elapsed_millis: 1,
            },
        ));
        probe.actor = ActorRef::SystemRecovery;
        core.execute(probe).await.unwrap();
        let installation = core.bootstrap().await.unwrap().installations.remove(0);
        let project = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                target_id: None,
                additional_directories: vec![],
                project_id: project.clone(),
                display_name: "Local".into(),
                path: directory.path().to_str().unwrap().to_owned(),
            },
        )))
        .await
        .unwrap();
        let created = core
            .execute(CommandEnvelope::local(CoreCommand::CreateTask(
                agent_protocol::CreateTask {
                    task_id: agent_domain::TaskId::new(),
                    project_id: project,
                    title: "Task".into(),
                    preferred_executor: agent_domain::ExecutorRef {
                        provider_id: agent_domain::ProviderId::parse("kimi").unwrap(),
                        installation_id: Some(installation.installation_id.to_string()),
                    },
                },
            )))
            .await;
        assert!(created.is_err());
    }

    #[tokio::test]
    async fn invalid_routes_never_reach_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let store = agent_store::SqliteStore::open(directory.path().join("t.db")).unwrap();
        let core = Arc::new(Core::new(Arc::new(store)));
        let host = TargetHost::new(
            Arc::new(MemorySecrets::default()),
            ShimCatalog::default(),
            None,
        );
        let bad = SshConnection {
            destination: "-oProxyCommand=touch /tmp/x".into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: None,
            proxy: None,
        };
        assert!(host.save(&core, None, "Bad".into(), bad).await.is_err());
        assert!(host.list(&core).await.unwrap().is_empty());
    }
}
