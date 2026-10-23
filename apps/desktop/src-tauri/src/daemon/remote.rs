//! Paired-device gateway owned by the daemon, independently of desktop windows.
#![allow(clippy::too_many_lines, clippy::type_complexity)]
use super::{Core, ServiceControl, dispatch, project_scoped};
use agent_protocol::submission::SubmissionIntentCall as C;
use agent_protocol::{ActorRef, CommandEnvelope, CoreCommand};
use base64::Engine;
use dogcodes_endpoint::{Session, pairing::PairingBook};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

type WebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
enum Socket {
    Relay(Box<WebSocket>),
    Direct(dogcodes_remote_transport::Channel),
}
struct Established<'a> {
    socket: Socket,
    session: &'a mut Session,
    config: &'a Config,
    lease: dogcodes_endpoint::pairing::DeviceLease,
    peer: [u8; 32],
    /// The screen track of a direct `desktop` connection.
    video: Option<dogcodes_remote_transport::VideoSender>,
}
impl Socket {
    async fn send(&mut self, message: Message) -> Result<(), String> {
        match self {
            Self::Relay(socket) => socket.send(message).await.map_err(failure),
            Self::Direct(socket) => match message {
                Message::Binary(bytes) => socket.send(&bytes).await,
                Message::Close(_) => socket.close().await,
                Message::Ping(_) | Message::Pong(_) => Ok(()),
                _ => Err("direct transport requires binary messages".into()),
            },
        }
    }
    async fn next(&mut self) -> Option<Result<Message, String>> {
        match self {
            Self::Relay(socket) => socket.next().await.map(|value| value.map_err(failure)),
            Self::Direct(socket) => match socket.receive().await {
                Ok(Some(bytes)) => Some(Ok(Message::Binary(bytes.into()))),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            },
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    service: String,
    host_id: String,
    public_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secrets {
    token: String,
    private_key: String,
}
struct Ready {
    config: Config,
    secrets: Secrets,
    book: PairingBook,
}
#[derive(Default)]
struct State {
    ready: Option<Ready>,
    running: bool,
    listening: bool,
    error: Option<String>,
    invitation: Option<Value>,
    invitation_owner: Option<String>,
    canceled_dialogs: std::collections::BTreeSet<String>,
}
pub(super) struct RemoteHost {
    directory: PathBuf,
    state: Mutex<State>,
    native: Arc<native::Host>,
    power: Arc<power::Host>,
    direct_slots: Arc<tokio::sync::Semaphore>,
    active_sessions: std::sync::atomic::AtomicUsize,
    notifications_started: std::sync::atomic::AtomicBool,
    revocations_started: std::sync::atomic::AtomicBool,
}
fn now() -> u64 {
    chrono::Utc::now().timestamp().try_into().unwrap_or(0)
}
fn failure(e: impl std::fmt::Debug) -> String {
    format!("远程连接失败：{e:?}")
}
impl RemoteHost {
    pub fn new(directory: PathBuf) -> Self {
        let power = Arc::new(power::Host::new(directory.join("power-policy.json")));
        let native = Arc::new(native::Host::new(power.clone(), directory.join("wayland")));
        Self {
            directory,
            state: Mutex::default(),
            native,
            power,
            direct_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            active_sessions: std::sync::atomic::AtomicUsize::new(0),
            notifications_started: std::sync::atomic::AtomicBool::new(false),
            revocations_started: std::sync::atomic::AtomicBool::new(false),
        }
    }
    async fn initialize(&self, service: Option<&str>) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if let Some(ready) = &state.ready {
            if service.is_some_and(|s| s.trim_end_matches('/') != ready.config.service) {
                return Err("已配置其他接入服务，请先核实原设备绑定。".into());
            }
            return Ok(());
        }
        let path = self.directory.join("config.json");
        let (config, secrets) = if path.exists() {
            let config: Config =
                serde_json::from_slice(&std::fs::read(&path).map_err(|_| "无法读取远程配置")?)
                    .map_err(|_| "远程配置损坏")?;
            if service.is_some_and(|s| s.trim_end_matches('/') != config.service) {
                return Err("接入服务与已有配置不一致。".into());
            }
            let id = config.host_id.clone();
            let secrets = tokio::task::spawn_blocking(move || {
                let entry = keyring::Entry::new("ai.owlfy.dogcodes.remote", &id)
                    .map_err(|_| "无法访问系统密钥存储")?;
                let raw = entry
                    .get_password()
                    .map_err(|_| "电脑身份无法读取，请解锁系统密钥存储；未生成替代身份")?;
                serde_json::from_str::<Secrets>(&raw).map_err(|_| "电脑身份损坏")
            })
            .await
            .map_err(|_| "读取系统密钥存储失败")??;
            (config, secrets)
        } else {
            let service = service
                .ok_or("尚未配置接入服务")?
                .trim_end_matches('/')
                .to_owned();
            validate_service(&service)?;
            let registration: Value = http_client(&service)?
                .post(format!("{service}/v1/hosts"))
                .send()
                .await
                .map_err(|_| "无法连接接入服务，请确认服务已启动")?
                .error_for_status()
                .map_err(|_| "接入服务暂时无法登记电脑")?
                .json()
                .await
                .map_err(|_| "接入服务响应无效")?;
            let key = dogcodes_endpoint::keypair().map_err(|_| "无法生成电脑身份")?;
            let config = Config {
                service,
                host_id: registration["hostId"]
                    .as_str()
                    .ok_or("缺少电脑标识")?
                    .into(),
                public_key: hex::encode(key.public),
            };
            let secrets = Secrets {
                token: registration["hostToken"]
                    .as_str()
                    .ok_or("缺少路由凭证")?
                    .into(),
                private_key: hex::encode(key.private),
            };
            let (id, secret) = (
                config.host_id.clone(),
                serde_json::to_string(&secrets).map_err(|_| "无法保存电脑身份")?,
            );
            tokio::task::spawn_blocking(move || {
                keyring::Entry::new("ai.owlfy.dogcodes.remote", &id)
                    .and_then(|e| e.set_password(&secret))
            })
            .await
            .map_err(|_| "保存系统密钥失败")?
            .map_err(|_| "系统密钥存储不可用，尚未启用远程连接")?;
            std::fs::create_dir_all(&self.directory).map_err(|_| "无法保存远程配置")?;
            std::fs::write(
                &path,
                serde_json::to_vec(&config).map_err(|_| "无法保存远程配置")?,
            )
            .map_err(|_| "无法保存远程配置")?;
            (config, secrets)
        };
        validate_service(&config.service)?;
        let book = PairingBook::load(self.directory.join("devices.json"))
            .map_err(|_| "配对记录不可用；未自动清空")?;
        state.ready = Some(Ready {
            config,
            secrets,
            book,
        });
        Ok(())
    }
    pub async fn status(&self) -> Value {
        let state = self.state.lock().await;
        json!({"service":state.ready.as_ref().map(|r|r.config.service.clone()),"configured":state.ready.is_some() || self.directory.join("config.json").exists(),"connected":state.listening,"error":state.error,"invitation":state.invitation,
            "devices":state.ready.as_ref().map(|r|r.book.grants()).unwrap_or_default()})
    }
    pub async fn call(
        self: &Arc<Self>,
        core: &Arc<Core>,
        control: &Arc<ServiceControl>,
        call: Value,
    ) -> Result<Value, String> {
        match call["type"].as_str().unwrap_or("status") {
            "status" => Ok(self.status().await),
            "enable" | "invite" => {
                let dialog = call["dialogId"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 128)
                    .ok_or("缺少配对窗口标识")?
                    .to_owned();
                {
                    let state = self.state.lock().await;
                    if state.canceled_dialogs.contains(&dialog) {
                        return Err("配对窗口已关闭，请重新打开。".into());
                    }
                }
                self.initialize(call["service"].as_str()).await?;
                self.start_listener(core, control).await;
                if call["type"] == "enable" {
                    return Ok(self.status().await);
                }
                let mut state = self.state.lock().await;
                if state.canceled_dialogs.contains(&dialog) {
                    return Err("配对窗口已关闭。".into());
                }
                let ready = state.ready.as_mut().ok_or("电脑身份不可用")?;
                let grant: Value = http(
                    &ready.config,
                    &ready.secrets,
                    reqwest::Method::POST,
                    "invitations",
                )
                .await?;
                let id = grant["sessionId"]
                    .as_str()
                    .ok_or("邀请响应无效")?
                    .to_owned();
                let expires = grant["expiresAt"].as_u64().ok_or("邀请响应无效")?;
                let key = dogcodes_endpoint::keypair().map_err(|_| "无法生成配对凭证")?;
                let secret: [u8; 32] = key.private.try_into().map_err(|_| "配对凭证无效")?;
                let context = serde_json::to_vec(&json!([
                    "dogcodes-pairing",
                    1,
                    ready.config.service,
                    ready.config.host_id,
                    id,
                    expires.to_string()
                ]))
                .map_err(|_| "无法创建邀请")?;
                ready
                    .book
                    .issue(&id, secret, context, expires, now())
                    .map_err(failure)?;
                let qr = json!({"version":1,"service":ready.config.service,"hostId":ready.config.host_id,"hostKey":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hex::decode(&ready.config.public_key).map_err(|_|"电脑公钥无效")?),"invitationId":id,"secret":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),"expiresAt":expires.to_string(),"phoneTicket":grant["phoneTicket"]});
                let qr = format!(
                    "dogcodes://pair#{}",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(serde_json::to_vec(&qr).map_err(|_| "无法创建邀请")?)
                );
                state.invitation_owner = Some(dialog);
                state.error = None;
                state.invitation = Some(json!({"uri":qr,"expiresAt":expires,"sessionId":id}));
                let ticket = grant["hostTicket"]
                    .as_str()
                    .ok_or("邀请响应无效")?
                    .to_owned();
                drop(state);
                self.spawn_connection(core, control, id, ticket, None);
                Ok(self.status().await)
            }
            "cancel_invitation" => {
                let dialog = call["dialogId"].as_str().ok_or("缺少配对窗口标识")?;
                let mut state = self.state.lock().await;
                // Only the authenticated local UI can create dialog identities. Keep
                // tombstones for this daemon lifetime so a delayed IPC cannot reopen one.
                state.canceled_dialogs.insert(dialog.into());
                if state.invitation_owner.as_deref() == Some(dialog) {
                    if let Some(r) = &mut state.ready {
                        r.book.cancel_invitation();
                    }
                    state.invitation = None;
                    state.invitation_owner = None;
                }
                drop(state);
                Ok(self.status().await)
            }
            "revoke" => {
                let id = call["deviceId"].as_str().ok_or("缺少设备标识")?;
                self.revoke_device(id).await?;
                Ok(self.status().await)
            }
            _ => Err("未知的远程配置操作".into()),
        }
    }
    async fn revoke_device(&self, id: &str) -> Result<(), String> {
        let (config, secrets) = {
            let mut state = self.state.lock().await;
            let ready = state.ready.as_mut().ok_or("尚未启用远程连接")?;
            ready.book.revoke(id).map_err(failure)?;
            (ready.config.clone(), ready.secrets.clone())
        };
        let _ = http(
            &config,
            &secrets,
            reqwest::Method::DELETE,
            &format!("devices/{id}"),
        )
        .await;
        Ok(())
    }
    async fn start_listener(self: &Arc<Self>, core: &Arc<Core>, control: &Arc<ServiceControl>) {
        self.start_notifications(core, Arc::downgrade(control));
        self.start_revocation_sync(Arc::downgrade(control));
        let mut state = self.state.lock().await;
        if state.running {
            return;
        }
        state.running = true;
        let (host, core, weak) = (self.clone(), core.clone(), Arc::downgrade(control));
        tokio::spawn(async move {
            loop {
                let credentials = {
                    let s = host.state.lock().await;
                    s.ready
                        .as_ref()
                        .map(|r| (r.config.clone(), r.secrets.clone()))
                };
                let Some((config, secrets)) = credentials else {
                    break;
                };
                match socket(
                    &config.service,
                    &format!("/v1/hosts/{}/listen", config.host_id),
                    &secrets.token,
                )
                .await
                {
                    Ok(mut ws) => {
                        {
                            let mut state = host.state.lock().await;
                            state.error = None;
                            state.listening = true;
                        }
                        while let Some(Ok(message)) = ws.next().await {
                            match message {
                                Message::Text(text) => {
                                    if let Ok(v) = serde_json::from_str::<Value>(&text)
                                        && let (Some(id), Some(ticket), Some(device), Some(control)) = (
                                            v["sessionId"].as_str(),
                                            v["hostTicket"].as_str(),
                                            v["deviceId"].as_str(),
                                            weak.upgrade(),
                                        )
                                    {
                                        host.spawn_connection(
                                            &core,
                                            &control,
                                            id.into(),
                                            ticket.into(),
                                            Some(device.into()),
                                        );
                                    }
                                }
                                Message::Ping(v) => {
                                    if ws.send(Message::Pong(v)).await.is_err() {
                                        break;
                                    }
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                    }
                    Err(_) => {
                        host.state.lock().await.error = Some("接入服务连接中断，正在重连".into());
                    }
                }
                host.state.lock().await.listening = false;
                if weak.upgrade().is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }
    fn spawn_connection(
        self: &Arc<Self>,
        core: &Arc<Core>,
        control: &Arc<ServiceControl>,
        id: String,
        ticket: String,
        device: Option<String>,
    ) {
        let (host, core, control) = (self.clone(), core.clone(), control.clone());
        tokio::spawn(async move {
            if host
                .connection(&core, &control, &id, &ticket, device.as_deref())
                .await
                .is_err()
            {
                let mut state = host.state.lock().await;
                if device.is_none()
                    && state
                        .invitation
                        .as_ref()
                        .is_some_and(|v| v["sessionId"] == id)
                {
                    state.error = Some("手机配对连接未完成，请核实或重新生成邀请。".into());
                }
            }
        });
    }
    async fn connection(
        &self,
        core: &Arc<Core>,
        control: &Arc<ServiceControl>,
        id: &str,
        ticket: &str,
        device: Option<&str>,
    ) -> Result<(), String> {
        let (config, mut noise, mut candidate, first_wait) = {
            let state = self.state.lock().await;
            // A pairing QR stays valid for minutes and the phone joins only once
            // someone scans it; a reconnect is opened by the phone immediately.
            let first_wait = state
                .invitation
                .as_ref()
                .filter(|v| device.is_none() && v["sessionId"] == id)
                .and_then(|v| v["expiresAt"].as_u64())
                .map_or(HANDSHAKE_WAIT, |expires| {
                    Duration::from_secs(expires.saturating_sub(now()).clamp(15, 300))
                });
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            let key: [u8; 32] = hex::decode(&ready.secrets.private_key)
                .map_err(|_| "invalid key")?
                .try_into()
                .map_err(|_| "invalid key")?;
            if let Some(device) = device {
                let grant = ready
                    .book
                    .grants()
                    .into_iter()
                    .find(|g| g.device_id == device && !g.revoked)
                    .ok_or("device revoked")?;
                let peer = hex::decode(grant.public_key)
                    .map_err(|_| "invalid pin")?
                    .try_into()
                    .map_err(|_| "invalid pin")?;
                let context =
                    serde_json::to_vec(&json!(["dogcodes-reconnect", 1, ready.config.host_id, id]))
                        .map_err(failure)?;
                (
                    ready.config.clone(),
                    Some(Session::new(false, &key, Some(peer), None, &context).map_err(failure)?),
                    None,
                    first_wait,
                )
            } else {
                let attempt = ready.book.begin(id, &key, now()).map_err(failure)?;
                (ready.config.clone(), None, Some(attempt), first_wait)
            }
        };
        let mut ws = socket(&config.service, &format!("/v1/relay/{id}"), ticket).await?;
        let session = match candidate.as_mut() {
            Some(a) => a.session(),
            None => noise.as_mut().ok_or("missing session")?,
        };
        session
            .read_handshake(&receive(&mut ws, first_wait).await?)
            .map_err(failure)?;
        ws.send(Message::Binary(
            session.write_handshake().map_err(failure)?.into(),
        ))
        .await
        .map_err(|_| "connection closed")?;
        session
            .read_handshake(&receive(&mut ws, HANDSHAKE_WAIT).await?)
            .map_err(failure)?;
        let peer = session.authenticated_peer().map_err(failure)?;
        let binding = hex::encode(session.binding().map_err(failure)?);
        let (lease, ack) = {
            let mut state = self.state.lock().await;
            let ready = state.ready.as_mut().ok_or("remote unavailable")?;
            let grant = if let Some(attempt) = &candidate {
                ready.book.complete(attempt, now()).map_err(failure)?
            } else {
                ready
                    .book
                    .check(
                        &ready
                            .book
                            .authorize(noise.as_ref().ok_or("missing session")?)
                            .map_err(failure)?,
                    )
                    .map_err(failure)?
                    .clone()
            };
            let active = match candidate.as_mut() {
                Some(a) => a.session(),
                None => noise.as_mut().ok_or("missing session")?,
            };
            let lease = ready.book.authorize(active).map_err(failure)?;
            let ack = if candidate.is_some() {
                let route = http_body(
                    &ready.config,
                    &ready.secrets,
                    reqwest::Method::POST,
                    &format!("devices/{}", grant.device_id),
                    Some(&json!({"invitationId":id})),
                )
                .await?;
                state.invitation = None;
                json!({"type":"paired","hostId":config.host_id,"hostName":host_name(),"invitationId":id,"phonePublicKey":hex::encode(peer),"hostPublicKey":config.public_key,"binding":binding,"grantRevision":grant.revision.to_string(),"routeToken":route["routeToken"]})
            } else {
                json!({"type":"connected","hostId":config.host_id,"hostName":host_name(),"binding":binding})
            };
            (lease, ack)
        };
        let session = match candidate.as_mut() {
            Some(a) => a.session(),
            None => noise.as_mut().ok_or("missing session")?,
        };
        send_json(&mut ws, session, &ack).await?;
        self.serve(
            core,
            control,
            Established {
                socket: ws,
                session,
                config: &config,
                lease,
                peer,
                video: None,
            },
        )
        .await
    }
    pub(super) fn has_active_sessions(&self) -> bool {
        self.active_sessions
            .load(std::sync::atomic::Ordering::Acquire)
            > 0
    }
    fn serve<'a>(
        &'a self,
        core: &'a Arc<Core>,
        control: &'a Arc<ServiceControl>,
        established: Established<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            struct Count<'a>(&'a std::sync::atomic::AtomicUsize);
            impl Drop for Count<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            let Established {
                socket: ws,
                session,
                config,
                lease,
                peer,
                video,
            } = established;
            let lease = Arc::new(lease);
            self.active_sessions
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let _active = Count(&self.active_sessions);
            let actor = device_actor(&peer)?;
            let binding = hex::encode(session.binding().map_err(failure)?);
            // Binary messages (video access units) from the native worker;
            // bounded and refused when full, so a slow link drops frames
            // instead of buffering seconds of them.
            let (binary, binaries) = mpsc::channel::<(Value, Vec<u8>)>(8);
            let disconnect = native::Disconnect::authenticated(video, binary, peer);
            let previews = Arc::new(preview::Host::default());
            let (out, outgoing) = mpsc::channel::<Value>(64);
            let mut buffer = Vec::new();
            let permits = Arc::new(tokio::sync::Semaphore::new(16));
            let mut check = tokio::time::interval(Duration::from_millis(250));
            let mut subscriptions = workspace::Subscriptions::default();
            let noise = Arc::new(Mutex::new(session));
            let (mut reader, transport) = wire::split(ws);
            let (pong, pongs) = mpsc::channel(8);
            let (acknowledge, acknowledgements) = mpsc::channel(32);
            let mut writer = wire::Writer {
                transport,
                noise: noise.clone(),
                remote: self,
                lease: lease.clone(),
                connection: disconnect.0.clone(),
            };
            let read = async {
                loop {
                    tokio::select! {
                        _=check.tick()=>{let state=self.state.lock().await;state.ready.as_ref().ok_or("remote unavailable")?.book.check(&lease).map_err(|_|"device revoked")?;},
                        message=reader.next()=>{
                            let bytes=match message {Some(Ok(Message::Binary(bytes)))=>bytes,Some(Ok(Message::Ping(v)))=>{pong.try_send(Message::Pong(v)).map_err(|_|"ping queue full")?;continue;},_=>return Ok(())};
                            let plain=noise.lock().await.open(&bytes).map_err(failure)?;buffer.extend(plain);
                            while let Some(request)=take_json(&mut buffer)? {
                                {let state=self.state.lock().await;state.ready.as_ref().ok_or("remote unavailable")?.book.check(&lease).map_err(|_|"device revoked")?;}
                                if request["hostId"]!=config.host_id || request["version"]!=1{return Err("invalid request identity".into());}
                                if request["type"]=="fragment_ack" {
                                    acknowledge.try_send(wire::progress(request["acks"].clone())?).map_err(|_|"fragment acknowledgement queue full")?;
                                    continue;
                                }
                                let request_id=request["requestId"].as_str().ok_or("missing request id")?.to_owned();
                                if request_id.is_empty() || request_id.len()>128 {return Err("invalid request id".into());}

                                let method=request["method"].as_str().ok_or("missing method")?.to_owned();let args=request["params"].clone();
                                let (core,control,out,actor)=(core.clone(),control.clone(),out.clone(),actor.clone());
                                let (native,connection)=(self.native.clone(),disconnect.0.clone());
                                let (previews,lease)=(previews.clone(),lease.clone());
                                let binding=binding.clone();
                                if method=="transport_options" {
                                    out.try_send(response(&request_id,wire::configure(&connection,&args))).map_err(|_|"response queue full")?;
                                } else if method=="transport_status" {
                                    out.try_send(response(&request_id,reader.status().await)).map_err(|_|"response queue full")?;
                                } else if method=="unsubscribe" {
                                    let result=match args.as_str() {Some(id)=>Ok(json!({"unsubscribed":subscriptions.remove(id).await})),None=>Err("invalid subscription id".into())};
                                    out.try_send(response(&request_id,result)).map_err(|_|"response queue full")?;
                                } else if method=="subscribe_project" || workspace::STREAMS.contains(&method.as_str()) {
                                    let scope=if method=="subscribe_project" {format!("project:{}",args.get(1).map(Value::to_string).unwrap_or_default())} else {format!("watch:{request_id}")};
                                    if let Err(error)=subscriptions.prepare(&request_id,&scope).await {out.try_send(response(&request_id,Err(error))).map_err(|_|"response queue full")?;continue;}
                                    let tx=out.clone();let id=request_id.clone();
                                    subscriptions.insert(request_id,scope,tokio::spawn(async move {
                                        let result=if method=="subscribe_project" {subscribe(&core,args,&tx,&id).await.map(|_|())} else {workspace::subscribe(&core,&control,&method,args,&tx,&id).await};
                                        if let Err(error)=result {let _=tx.send(response(&id,Err(error))).await;}
                                        // An error after the subscribe reply, or a closed event bus, would
                                        // otherwise end the stream silently; the phone resubscribes on this.
                                        // An explicit unsubscribe aborts the task first, so it never arrives.
                                        if method=="subscribe_project" {let _=tx.send(json!({"type":"subscription_ended","subscriptionId":id})).await;}
                                    }));
                                } else {
                                    let Ok(permit)=permits.clone().try_acquire_owned() else {out.try_send(response(&request_id,Err("too many in-flight requests".into()))).map_err(|_|"response queue full")?;continue;};
                                    tokio::spawn(async move {
                                        let _permit=permit;
                                        let _request=control.requests.read().await;
                                        // Recheck after waiting for service access: queued requests
                                        // must not begin effects after disconnect or grant revocation.
                                        if !connection.alive() {return;}
                                        let allowed={let state=control.remote.state.lock().await;state.ready.as_ref().is_some_and(|ready|ready.book.check(&lease).is_ok())};
                                        if !allowed {let _=out.send(response(&request_id,Err("device revoked".into()))).await;return;}
                                        let result=if method=="pairing_receipt" {control.remote.pairing_receipt(&lease,peer,&binding,args).await} else if method=="direct_open" {control.remote.open_direct(&core,&control,peer,args).await} else if preview::METHODS.contains(&method.as_str()) {previews.call(&core,&method,args).await} else if power::METHODS.contains(&method.as_str()) {control.remote.power.call(&method,args).await} else if native::METHODS.contains(&method.as_str()) {native.call(connection,&method,args).await} else {remote_call(&core,&control,&method,args,actor).await};
                                        let _=out.send(response(&request_id,result)).await;
                                    });
                                }
                            }
                        }
                    }
                }
            };
            let result = tokio::select! {result=read=>result,result=writer.run(outgoing,binaries,pongs,acknowledgements)=>result};
            drop(subscriptions);
            drop(disconnect);
            writer.transport.close().await;
            result
        })
    }
}
const MAX_SUBSCRIPTIONS: usize = 32;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
mod business;
mod clipboard;
mod direct;
mod native;
mod notifications;
mod power;
mod preview;
mod recovery;
mod video;
mod wire;
mod workspace;

/// Name shown on the paired phone; never used for identity or authorization.
fn host_name() -> String {
    let name = sysinfo::System::host_name().unwrap_or_default();
    let name = name.trim();
    let name = name
        .len()
        .checked_sub(6)
        .filter(|&i| name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".local"))
        .map_or(name, |i| name[..i].trim());
    if name.is_empty() {
        "电脑".into()
    } else {
        name.chars().take(64).collect()
    }
}

fn response(id: &str, result: Result<Value, String>) -> Value {
    let value = match result {
        Ok(value) => json!({"type":"response","requestId":id,"result":value}),
        Err(message) => json!({"type":"response","requestId":id,"error":{"message":message}}),
    };
    if serde_json::to_vec(&wire_json(&value)).map_or(true, |bytes| bytes.len() > MAX_FRAME_BYTES) {
        return json!({"type":"response","requestId":id,"error":{
            "code":"response_too_large","message":"内容超过远程传输大小限制，请缩小查询范围后重试"
        }});
    }
    value
}
pub(super) async fn remote_call(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    mut args: Value,
    actor: ActorRef,
) -> Result<Value, String> {
    if !matches!(actor, ActorRef::PairedDevice(_)) {
        return Err("paired device required".into());
    }
    if method == "remote_capabilities" {
        return Ok(workspace::capabilities());
    }
    if method == "terminal_input" {
        let owner = serde_json::to_string(&actor).map_err(failure)?;
        return control.terminals.input_operation(&owner, args).await;
    }
    if workspace::METHODS.contains(&method) {
        return workspace::call(core, control, method, args).await;
    }
    if business::METHODS.contains(&method) {
        return business::call(core, control, method, args, actor).await;
    }
    if let Some(envelope) = args.get_mut(0) {
        for field in ["expectedStreamVersion", "requestedAtSequence"] {
            parse_sequence(envelope.get_mut(field))?;
        }
        parse_sequence(envelope.pointer_mut("/payload/data/requestedAtSequence"))?;
    }
    if method == "invalidate_approval_response" {
        parse_sequence(args.get_mut(1))?;
    }
    match method {
        "bootstrap"
        | "agent_preferences"
        | "registered_executors"
        | "model_catalog_for_task"
        | "permission_catalog"
        | "timeline_snapshot"
        | "approval_receipt"
        | "invalidate_approval_response"
        | "pending_approvals" => dispatch(core, method, args).await,
        // Read-only quota status; a phone never forces a vendor refresh.
        "quota_snapshots_for" => {
            let (task, installation, _): (Option<String>, Option<String>, Value) =
                serde_json::from_value(args).map_err(failure)?;
            dispatch(core, method, json!([task, installation, false])).await
        }
        "submission_intents" => {
            let call: agent_protocol::submission::SubmissionIntentCall =
                serde_json::from_value(args).map_err(failure)?;
            if !matches!(
                call,
                C::Start { .. }
                    | C::GetStart { .. }
                    | C::Get { .. }
                    | C::List { .. }
                    | C::Continue { .. }
                    | C::Retire { .. }
            ) {
                return Err("remote method unavailable".into());
            }
            control.submissions.call_as(core, call, actor).await
        }
        "execute" | "command_receipt" | "prepare_approval_response" => {
            let (mut envelope,): (CommandEnvelope<CoreCommand>,) =
                serde_json::from_value(args).map_err(failure)?;
            if !matches!(
                envelope.payload,
                CoreCommand::RespondToApproval(_)
                    | CoreCommand::CancelTurn(_)
                    | CoreCommand::RenameTask(_)
                    | CoreCommand::DeleteTask(_)
                    | CoreCommand::SetHistoryPreferences(_)
            ) {
                return Err("remote command unavailable".into());
            }
            envelope.actor = actor;
            if method == "command_receipt" {
                serde_json::to_value(core.command_receipt(&envelope).await.map_err(failure)?)
                    .map_err(failure)
            } else if method == "prepare_approval_response" {
                serde_json::to_value(core.prepare_approval_response(&envelope).map_err(failure)?)
                    .map_err(failure)
            } else if matches!(&envelope.payload, CoreCommand::DeleteTask(_)) {
                super::execute_task_delete(core, control, envelope).await
            } else {
                serde_json::to_value(core.execute(envelope).await.map_err(failure)?)
                    .map_err(failure)
            }
        }
        _ => Err("remote method unavailable".into()),
    }
}
async fn subscribe(
    core: &Arc<Core>,
    mut args: Value,
    out: &mpsc::Sender<Value>,
    request_id: &str,
) -> Result<Value, String> {
    parse_sequence(args.get_mut(0))?;
    let (after, project): (u64, agent_domain::ProjectId) =
        serde_json::from_value(args).map_err(failure)?;
    let snapshot = core.bootstrap().await.map_err(failure)?;
    if !snapshot.projects.iter().any(|p| p.project_id == project) {
        return Err("project unavailable".into());
    }
    let mut tasks = snapshot
        .tasks
        .iter()
        .filter(|t| t.project_id == project)
        .map(|t| t.task_id.to_string())
        .collect();
    let mut subscription = core.subscribe(after).await.map_err(failure)?;
    let mut cursor = after;
    for event in std::mem::take(&mut subscription.replay) {
        cursor = event.sequence;
        if project_scoped(&event, &project, &mut tasks) {
            out.send(json!({"type":"event","event":event}))
                .await
                .map_err(|_| "connection closed")?;
        }
    }
    out.send(response(
        request_id,
        Ok(json!({"subscribed":true,"sequence":cursor,"subscriptionId":request_id})),
    ))
    .await
    .map_err(|_| "connection closed")?;
    loop {
        let mut replaying = false;
        let mut events = match subscription.live.recv().await {
            Ok(event) => vec![event],
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                replaying = true;
                core.sync_events(cursor, 1000).await.map_err(failure)?
            }
            Err(_) => return Ok(Value::Null),
        };
        loop {
            let count = events.len();
            for event in events {
                if event.sequence <= cursor {
                    continue;
                }
                cursor = event.sequence;
                if project_scoped(&event, &project, &mut tasks) {
                    out.send(json!({"type":"event","event":event}))
                        .await
                        .map_err(|_| "connection closed")?;
                }
            }
            if !replaying || count < 1000 {
                break;
            }
            events = core.sync_events(cursor, 1000).await.map_err(failure)?;
        }
    }
}
fn device_actor(peer: &[u8; 32]) -> Result<ActorRef, String> {
    let hash = Sha256::digest(peer);
    let bytes: [u8; 16] = hash[..16].try_into().map_err(|_| "invalid identity")?;
    Ok(ActorRef::PairedDevice(
        agent_domain::DeviceId::parse(format!("dev_{}", ulid::Ulid::from_bytes(bytes)))
            .map_err(failure)?,
    ))
}
fn validate_service(service: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(service).map_err(|_| "接入服务地址无效")?;
    if (url.scheme() != "https"
        && !(cfg!(debug_assertions)
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("接入服务须使用 HTTPS；调试版本支持本机服务".into());
    }
    Ok(())
}
async fn http(
    config: &Config,
    secrets: &Secrets,
    method: reqwest::Method,
    path: &str,
) -> Result<Value, String> {
    http_body(config, secrets, method, path, None).await
}
async fn http_body(
    config: &Config,
    secrets: &Secrets,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut request = http_client(&config.service)?
        .request(
            method,
            format!("{}/v1/hosts/{}/{path}", config.service, config.host_id),
        )
        .bearer_auth(&secrets.token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "接入服务连接失败")?
        .error_for_status()
        .map_err(|_| "接入服务拒绝请求")?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        Ok(Value::Null)
    } else {
        response.json().await.map_err(|_| "接入服务响应无效".into())
    }
}
async fn socket(service: &str, path: &str, token: &str) -> Result<Socket, String> {
    let url = format!(
        "{}{path}",
        service
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    );
    let mut request = url.into_client_request().map_err(|_| "invalid endpoint")?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "invalid credential")?,
    );
    tokio::time::timeout(Duration::from_secs(10), connect_async(request))
        .await
        .map_err(|_| "connection timeout")?
        .map(|(ws, _)| Socket::Relay(Box::new(ws)))
        .map_err(|_| "connection failed".into())
}
const HANDSHAKE_WAIT: Duration = Duration::from_secs(15);

async fn receive(ws: &mut Socket, wait: Duration) -> Result<Vec<u8>, String> {
    tokio::time::timeout(wait, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Binary(v))) => return Ok(v.to_vec()),
                Some(Ok(Message::Ping(v))) => ws
                    .send(Message::Pong(v))
                    .await
                    .map_err(|_| "connection closed")?,
                _ => return Err("handshake interrupted".into()),
            }
        }
    })
    .await
    .map_err(|_| "handshake timeout")?
}
async fn send_json(ws: &mut Socket, noise: &mut Session, value: &Value) -> Result<(), String> {
    send_framed(ws, noise, &json_frame(value)?).await
}
fn json_frame(value: &Value) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(&wire_json(value)).map_err(failure)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("response too large".into());
    }
    let mut framed = u32::try_from(bytes.len())
        .map_err(failure)?
        .to_be_bytes()
        .to_vec();
    framed.extend(bytes);
    Ok(framed)
}

/// Marks a binary message in the length word. JSON messages never reach it
/// (they are capped at 8 MB), and a phone receives binary messages only for
/// streams it asked for, so older phones never see one.
const BINARY_FRAME: u32 = 0x8000_0000;
/// `[u32 length | BINARY_FRAME][u32 header length][header JSON][payload]`:
/// raw bytes beside a small JSON header, without base64's third more or a
/// JSON parse of the bytes.
fn binary_frame(header: &Value, payload: &[u8]) -> Result<Vec<u8>, String> {
    let header = serde_json::to_vec(&wire_json(header)).map_err(failure)?;
    let length = 4 + header.len() + payload.len();
    if length > MAX_FRAME_BYTES || header.len() > 64 * 1024 {
        return Err("binary message too large".into());
    }
    let mut framed = Vec::with_capacity(4 + length);
    framed.extend((u32::try_from(length).map_err(failure)? | BINARY_FRAME).to_be_bytes());
    framed.extend(u32::try_from(header.len()).map_err(failure)?.to_be_bytes());
    framed.extend(header);
    framed.extend(payload);
    Ok(framed)
}
async fn send_framed(ws: &mut Socket, noise: &mut Session, framed: &[u8]) -> Result<(), String> {
    for part in framed.chunks(dogcodes_remote_transport::PLAINTEXT_CHUNK) {
        let bytes = noise.seal(part).map_err(failure)?;
        tokio::time::timeout(
            Duration::from_secs(5),
            ws.send(Message::Binary(bytes.into())),
        )
        .await
        .map_err(|_| "slow consumer")?
        .map_err(|_| "connection closed")?;
    }
    Ok(())
}
fn take_json(buffer: &mut Vec<u8>) -> Result<Option<Value>, String> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let size = u32::from_be_bytes(buffer[..4].try_into().map_err(|_| "invalid frame")?) as usize;
    if size > MAX_FRAME_BYTES || size == 0 {
        return Err("invalid frame".into());
    }
    if buffer.len() < size + 4 {
        return Ok(None);
    }
    let value = serde_json::from_slice(&buffer[4..size + 4]).map_err(|_| "invalid JSON")?;
    buffer.drain(..size + 4);
    Ok(Some(value))
}

pub(super) fn restore(
    host: Arc<RemoteHost>,
    core: Arc<Core>,
    control: std::sync::Weak<ServiceControl>,
) {
    host.power.start(&core, control.clone());
    if host.directory.join("config.json").exists() {
        tokio::spawn(async move {
            match host.initialize(None).await {
                Ok(()) => {
                    if let Some(control) = control.upgrade() {
                        host.start_listener(&core, &control).await;
                    }
                }
                Err(message) => host.state.lock().await.error = Some(message),
            }
        });
    }
}

#[cfg(feature = "remote-acceptance")]
#[path = "remote_acceptance.rs"]
mod acceptance;

#[cfg(feature = "remote-acceptance")]
pub(super) async fn acceptance_run(service: String, directory: PathBuf) -> Result<(), String> {
    acceptance::run(service, directory).await
}

fn http_client(service: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none());
    if service.starts_with("http://127.0.0.1:") {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|_| "连接配置失败".into())
}

fn wire_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(wire_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if matches!(
                        key.as_str(),
                        "sequence"
                            | "lastSequence"
                            | "firstSequence"
                            | "streamVersion"
                            | "requestedAtSequence"
                            | "deletionSequence"
                            | "outputEnd"
                    ) && value.is_u64()
                    {
                        Value::String(value.as_u64().unwrap_or(0).to_string())
                    } else if matches!(key.as_str(), "payload" | "input") {
                        value.clone()
                    } else {
                        wire_json(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn parse_sequence(value: Option<&mut Value>) -> Result<(), String> {
    if let Some(value) = value
        && let Some(raw) = value.as_str()
    {
        *value = json!(raw.parse::<u64>().map_err(|_| "invalid sequence")?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_sequences_are_strings_and_user_payloads_are_preserved() {
        let original = json!({"sequence":u64::MAX,"task":{"streamVersion":u64::MAX},"payload":{"sequence":42},"input":{"prompt":"123"}});
        let wire = wire_json(&original);
        assert_eq!(wire["sequence"], u64::MAX.to_string());
        assert_eq!(wire["task"]["streamVersion"], u64::MAX.to_string());
        assert_eq!(wire["payload"], original["payload"]);
        assert_eq!(wire["input"], original["input"]);
    }
    #[test]
    fn host_name_is_display_only_and_bounded() {
        let name = host_name();
        assert!(!name.is_empty() && name.chars().count() <= 64);
        assert!(!name.to_ascii_lowercase().ends_with(".local"));
    }
    #[test]
    fn binary_frames_carry_a_json_header_and_raw_bytes() {
        let framed = binary_frame(&json!({"type":"video","seq":7}), &[0, 0, 0, 1, 0x65]).unwrap();
        let word = u32::from_be_bytes(framed[..4].try_into().unwrap());
        assert_ne!(word & BINARY_FRAME, 0);
        assert_eq!((word & !BINARY_FRAME) as usize, framed.len() - 4);
        let header_len = u32::from_be_bytes(framed[4..8].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&framed[8..8 + header_len]).unwrap();
        assert_eq!(header["seq"], 7);
        assert_eq!(&framed[8 + header_len..], &[0, 0, 0, 1, 0x65]);
        // A binary word is never mistaken for a JSON frame by the reader.
        let mut buffer = framed;
        assert!(take_json(&mut buffer).is_err());
        assert!(binary_frame(&json!({}), &vec![0; MAX_FRAME_BYTES]).is_err());
    }
    #[test]
    fn fragmented_message_reassembly_is_bounded() {
        let payload = serde_json::to_vec(&json!({"type":"fixture"})).unwrap();
        let mut bytes = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
        assert!(take_json(&mut bytes).unwrap().is_none());
        bytes.extend(payload);
        assert_eq!(take_json(&mut bytes).unwrap().unwrap()["type"], "fixture");
        assert!(bytes.is_empty());
        assert!(take_json(&mut (9_u32 * 1024 * 1024).to_be_bytes().to_vec()).is_err());
    }
    #[tokio::test]
    async fn closed_pairing_dialog_cannot_issue_a_late_invitation() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let control = Arc::new(ServiceControl::new(directory.path()));
        control
            .remote
            .call(
                &core,
                &control,
                json!({"type":"cancel_invitation","dialogId":"closed"}),
            )
            .await
            .unwrap();
        let result = control
            .remote
            .call(
                &core,
                &control,
                json!({"type":"invite","dialogId":"closed","service":"http://127.0.0.1:1"}),
            )
            .await;
        assert!(result.unwrap_err().contains("窗口已关闭"));
        assert!(control.remote.state.lock().await.ready.is_none());
        for i in 0..260 {
            control
                .remote
                .call(
                    &core,
                    &control,
                    json!({"type":"cancel_invitation","dialogId":format!("other-{i}")}),
                )
                .await
                .unwrap();
        }
        // Repeated ordinary opens/closes must not permanently disable new dialogs.
        let fresh = control
            .remote
            .call(&core, &control, json!({"type":"invite","dialogId":"fresh"}))
            .await;
        assert_eq!(fresh.unwrap_err(), "尚未配置接入服务");
    }
    #[tokio::test]
    async fn remote_gateway_rejects_host_and_internal_commands() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let control = ServiceControl::new(directory.path());
        let actor = ActorRef::PairedDevice(agent_domain::DeviceId::new());
        assert!(
            remote_call(&core, &control, "scan_agent", Value::Null, actor.clone())
                .await
                .is_err()
        );
        let command = CommandEnvelope::local(CoreCommand::RegisterProject(
            agent_protocol::RegisterProject {
                project_id: agent_domain::ProjectId::new(),
                display_name: "blocked".into(),
                path: directory.path().to_string_lossy().into(),
                additional_directories: vec![],
            },
        ));
        assert!(
            remote_call(&core, &control, "execute", json!([command]), actor)
                .await
                .is_err()
        );
        assert!(core.bootstrap().await.unwrap().projects.is_empty());
    }
}
