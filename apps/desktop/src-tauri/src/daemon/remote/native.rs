//! A bounded native worker: no capture, input or clipboard work runs on Tokio.
use super::{Arc, Duration, Value, failure, json};
use dogcodes_remote_desktop as os;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{SyncSender, sync_channel},
};
use std::time::Instant;

mod handoff;

pub(super) const METHODS: &[&str] = &[
    "device_status",
    "remote_desktop_displays",
    "remote_desktop_prepare",
    "remote_desktop_open",
    "remote_desktop_frame",
    "remote_desktop_select",
    "remote_desktop_input",
    "remote_desktop_close",
    "remote_desktop_video",
    "remote_desktop_handoff",
    "clipboard_transfer",
];
pub(super) struct Connection {
    id: String,
    peer: Option<[u8; 32]>,
    closed: AtomicBool,
    pub(super) chunked: AtomicBool,
    pub(super) video_epoch: Arc<std::sync::atomic::AtomicU64>,
    /// Screen track of a direct desktop connection, if negotiated.
    video: Option<dogcodes_remote_transport::VideoSender>,
    /// Binary messages on this connection (relay video).
    binary: super::video::BinaryOut,
}
impl Connection {
    pub(super) fn alive(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
}
pub(super) struct Disconnect(pub Arc<Connection>);
impl Disconnect {
    #[cfg(test)]
    pub(super) fn new(
        video: Option<dogcodes_remote_transport::VideoSender>,
        binary: super::video::BinaryOut,
    ) -> Self {
        Self::with_peer(video, binary, None)
    }
    pub(super) fn authenticated(
        video: Option<dogcodes_remote_transport::VideoSender>,
        binary: super::video::BinaryOut,
        peer: [u8; 32],
    ) -> Self {
        Self::with_peer(video, binary, Some(peer))
    }
    fn with_peer(
        video: Option<dogcodes_remote_transport::VideoSender>,
        binary: super::video::BinaryOut,
        peer: Option<[u8; 32]>,
    ) -> Self {
        Self(Arc::new(Connection {
            peer,
            id: uuid::Uuid::new_v4().to_string(),
            closed: AtomicBool::new(false),
            chunked: AtomicBool::new(false),
            video_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            video,
            binary,
        }))
    }
}
impl Drop for Disconnect {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
    }
}
struct Request {
    connection: Arc<Connection>,
    method: String,
    args: Value,
    reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
}
pub(super) struct Host {
    sender: OnceLock<Result<SyncSender<Request>, String>>,
    power: Arc<super::power::Host>,
    clipboard: super::clipboard::Host,
    #[cfg(target_os = "linux")]
    portal_store: Option<Arc<os::wayland::TokenStore>>,
}
impl Host {
    pub(super) fn new(
        power: Arc<super::power::Host>,
        portal_directory: std::path::PathBuf,
    ) -> Self {
        #[cfg(not(target_os = "linux"))]
        let _ = portal_directory;
        Self {
            sender: OnceLock::new(),
            power,
            clipboard: super::clipboard::Host::default(),
            #[cfg(target_os = "linux")]
            portal_store: Some(Arc::new(os::wayland::TokenStore::new(portal_directory))),
        }
    }
    #[cfg(test)]
    pub(super) fn with_clipboard(
        power: Arc<super::power::Host>,
        clipboard: super::clipboard::Host,
    ) -> Self {
        Self {
            sender: OnceLock::new(),
            power,
            clipboard,
            #[cfg(target_os = "linux")]
            portal_store: None,
        }
    }
    pub(super) async fn call(
        &self,
        connection: Arc<Connection>,
        method: &str,
        args: Value,
    ) -> Result<Value, String> {
        if super::clipboard::METHODS.contains(&method) {
            return self.clipboard.call(connection, method, args).await;
        }
        let sender = self
            .sender
            .get_or_init(|| {
                let power = self.power.clone();
                #[cfg(target_os = "linux")]
                let portal_store = self.portal_store.clone();
                let (tx, rx) = sync_channel::<Request>(32);
                std::thread::Builder::new()
                    .name("remote-desktop".into())
                    .spawn(move || {
                        let mut worker = Worker::default();
                        #[cfg(target_os = "linux")]
                        {
                            worker.portal_store = portal_store;
                        }
                        loop {
                            worker.expire();
                            power
                                .desktop
                                .store(worker.lease.is_some(), Ordering::Release);
                            match rx.recv_timeout(Duration::from_millis(100)) {
                                Ok(request) => {
                                    if request.reply.is_closed()
                                        || request.connection.closed.load(Ordering::Acquire)
                                    {
                                        continue;
                                    }
                                    let result = worker.call(
                                        request.connection,
                                        &request.method,
                                        request.args,
                                    );
                                    let _ = request.reply.send(result);
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        power.desktop.store(false, Ordering::Release);
                    })
                    .map_err(|_| "无法启动远控系统线程".to_string())?;
                Ok(tx)
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let (reply, result) = tokio::sync::oneshot::channel();
        sender
            .try_send(Request {
                connection,
                method: method.into(),
                args,
                reply,
            })
            .map_err(|_| "远控系统繁忙，请稍后重试")?;
        tokio::time::timeout(Duration::from_secs(15), result)
            .await
            .map_err(|_| "系统远控操作超时，请核实系统权限")?
            .map_err(|_| "远控系统线程已结束")?
    }
}
struct Lease {
    id: String,
    liveness: handoff::Liveness,
    handoff: Option<handoff::Ticket>,
    handoff_revision: u64,
    owner: Arc<Connection>,
    display: u32,
    expires: Instant,
    sequence: u64,
    controller: Option<os::Controller>,
    cursor: os::cursor::Reader,
    last_frame: Option<Instant>,
    capturer: Option<os::Capturer>,
    /// H.264 screen video on the connection's track; frame pulls remain
    /// available as a fallback while it runs.
    video: Option<super::video::Session>,
    /// Set when the video goes out as binary messages; takes the phone's acks.
    relay: Option<Arc<super::video::RelayVideo>>,
    portal: Option<PortalSession>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(portal) = &self.portal {
            close_portal(portal);
        }
        if self.relay.is_some() {
            self.owner.video_epoch.fetch_add(1, Ordering::AcqRel);
        }
    }
}
#[cfg(target_os = "linux")]
type PortalSession = os::wayland::Session;
#[cfg(not(target_os = "linux"))]
#[derive(Clone)]
struct PortalSession;
fn wayland_required() -> bool {
    #[cfg(target_os = "linux")]
    {
        os::wayland::available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
fn close_portal(portal: &PortalSession) {
    #[cfg(target_os = "linux")]
    portal.close();
    #[cfg(not(target_os = "linux"))]
    let _ = portal;
}
fn displays_for(portal: Option<&PortalSession>) -> Result<Vec<os::Display>, String> {
    #[cfg(target_os = "linux")]
    if let Some(portal) = portal {
        return portal.displays();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = portal;
    os::displays()
}
fn capturer_for(
    portal: Option<&PortalSession>,
    display: u32,
    owner: &handoff::Liveness,
) -> Result<os::Capturer, String> {
    #[cfg(target_os = "linux")]
    if let Some(portal) = portal {
        return os::Capturer::from_portal(portal, display);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = portal;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let owner = owner.clone();
        os::Capturer::new_owned(display, Arc::new(move || owner.alive()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = owner;
        os::Capturer::new(display)
    }
}
fn controller_for(portal: Option<&PortalSession>) -> Result<os::Controller, String> {
    #[cfg(target_os = "linux")]
    if let Some(portal) = portal {
        return Ok(os::Controller::from_portal(portal.clone()));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = portal;
    os::Controller::new()
}
#[cfg(target_os = "linux")]
struct Preparing {
    liveness: handoff::Liveness,
    owner: Arc<Connection>,
    session: PortalSession,
    expires: Instant,
    control: bool,
    fresh: bool,
    transferred: bool,
}
#[cfg(target_os = "linux")]
impl Drop for Preparing {
    fn drop(&mut self) {
        if !self.transferred {
            self.session.close();
        }
    }
}

/// Frames are pulled one at a time; this cap only bounds a misbehaving client.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(40);
const MAX_INPUT_BATCH: usize = 64;
const TRANSPORT: &str = "jpeg-pull-v2";
#[derive(Default)]
struct Worker {
    lease: Option<Lease>,
    checked_lock: Option<Instant>,
    #[cfg(target_os = "linux")]
    preparing: Option<Preparing>,
    #[cfg(target_os = "linux")]
    portal_store: Option<Arc<os::wayland::TokenStore>>,
}
impl Worker {
    // The portable contract is fallible on Linux and a no-op elsewhere.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(clippy::unused_self, clippy::unnecessary_wraps)
    )]
    fn portal_for(&self, connection: &Arc<Connection>) -> Result<Option<PortalSession>, String> {
        #[cfg(target_os = "linux")]
        if wayland_required() {
            if let Some(lease) = &self.lease {
                if lease.owner.id != connection.id {
                    return Err("电脑正在被其它连接远控".into());
                }
                return Ok(lease.portal.clone());
            }
            let pending = self
                .preparing
                .as_ref()
                .filter(|p| p.owner.id == connection.id)
                .ok_or("请先在电脑上完成 Wayland 屏幕共享授权")?;
            if !pending.session.status()? {
                return Err("请在电脑上完成屏幕共享授权".into());
            }
            return Ok(Some(pending.session.clone()));
        }
        let _ = connection;
        Ok(None)
    }
    #[cfg_attr(
        not(target_os = "linux"),
        allow(clippy::unused_self, clippy::unnecessary_wraps)
    )]
    fn prepare(&mut self, connection: Arc<Connection>, args: Value) -> Result<Value, String> {
        if !wayland_required() {
            return Ok(json!({"state":"ready","required":false}));
        }
        #[cfg(target_os = "linux")]
        {
            os::require_unlocked()?;
            if self.lease.is_some() {
                return Err("远控已经开启，请先结束原连接".into());
            }
            let control = args["control"].as_bool().unwrap_or(true);
            let fresh = args["fresh"].as_bool().unwrap_or(false);
            if self.preparing.is_none() {
                let store = self.portal_store.clone().ok_or("Wayland 授权存储未配置")?;
                let liveness = handoff::Liveness::new(&connection);
                let watch = liveness.clone();
                let alive = Arc::new(move || watch.alive());
                self.preparing = Some(Preparing {
                    liveness,
                    owner: connection.clone(),
                    session: os::wayland::Session::start_owned(store, control, fresh, alive)?,
                    expires: Instant::now() + Duration::from_secs(15),
                    control,
                    fresh,
                    transferred: false,
                });
            }
            let pending = self.preparing.as_mut().ok_or("Wayland 授权未开始")?;
            if pending.owner.id != connection.id
                || pending.control != control
                || pending.fresh != fresh
            {
                return Err("另一项电脑授权正在进行，请先结束原连接".into());
            }
            pending.expires = Instant::now() + Duration::from_secs(15);
            Ok(
                json!({"state":if pending.session.status()?{"ready"}else{"pending"},"required":true,"message":"请在电脑上允许屏幕共享和键鼠控制"}),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (connection, args);
            Ok(json!({"state":"ready","required":false}))
        }
    }
    fn expire(&mut self) {
        #[cfg(target_os = "linux")]
        if self
            .preparing
            .as_ref()
            .is_some_and(|p| p.expires <= Instant::now() || !p.owner.alive())
        {
            self.preparing = None;
        }
        if let Some(lease) = &mut self.lease {
            handoff::expire(lease);
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if lease
                .last_frame
                .is_some_and(|last| last.elapsed() > Duration::from_secs(5))
                && let Some(capturer) = &mut lease.capturer
            {
                capturer.release_idle_picture();
            }
        }
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires <= Instant::now() || !lease.liveness.alive())
        {
            self.lease = None;
        }
        let has_desktop = self.lease.is_some();
        #[cfg(target_os = "linux")]
        let has_desktop = has_desktop || self.preparing.is_some();
        if has_desktop
            && self
                .checked_lock
                .is_none_or(|last| last.elapsed() > Duration::from_secs(1))
        {
            self.checked_lock = Some(Instant::now());
            // Release held keys/buttons even when the client stops requesting frames.
            if os::require_unlocked().is_err() {
                self.lease = None;
                #[cfg(target_os = "linux")]
                {
                    self.preparing = None;
                }
            }
        }
    }
    fn call(
        &mut self,
        connection: Arc<Connection>,
        method: &str,
        mut args: Value,
    ) -> Result<Value, String> {
        self.expire();
        #[cfg(target_os = "linux")]
        if method.starts_with("remote_desktop_")
            && !matches!(
                method,
                "remote_desktop_prepare" | "remote_desktop_close" | "remote_desktop_handoff"
            )
        {
            let portal = self
                .lease
                .as_ref()
                .filter(|l| l.owner.id == connection.id)
                .and_then(|l| l.portal.as_ref())
                .or_else(|| {
                    self.preparing
                        .as_ref()
                        .filter(|p| p.owner.id == connection.id)
                        .map(|p| &p.session)
                });
            if let Some(portal) = portal
                && let Err(error) = portal.status()
            {
                return Ok(json!({"authorizationEnded":true,"error":error}));
            }
        }
        match method {
            "device_status" => {
                let lock = os::locked();
                Ok(
                    json!({"hostName":super::host_name(),"platform":std::env::consts::OS,
                    "locked":lock.as_ref().ok(),"lockStateError":lock.err(),"agentAvailable":true,
                    "desktopInUse":self.lease.is_some(),"desktopPreparationRequired":wayland_required(),"desktopTransport":TRANSPORT,"audio":false}),
                )
            }
            "remote_desktop_prepare" => self.prepare(connection, args),
            "remote_desktop_displays" => {
                let portal = self.portal_for(&connection)?;
                Ok(json!(displays_for(portal.as_ref())?))
            }
            "remote_desktop_handoff" => {
                let lease = self.lease.as_mut().ok_or("远控租约已结束，请重新连接")?;
                handoff::call(lease, connection, args)
            }
            "remote_desktop_open" => {
                if args["chunkedMessages"] == true {
                    connection.chunked.store(true, Ordering::Release);
                }
                os::require_unlocked()?;
                if self.lease.is_some() {
                    return Err("电脑正在被远程查看或控制，请先结束原连接".into());
                }
                let display = args["monitorId"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or("缺少显示器标识")?;
                let portal = self.portal_for(&connection)?;
                if !displays_for(portal.as_ref())?
                    .iter()
                    .any(|m| m.id == display)
                {
                    return Err("显示器已断开".into());
                }
                let control = args["control"].as_bool().unwrap_or(true);
                #[cfg(target_os = "linux")]
                if portal.is_some() && self.preparing.as_ref().is_none_or(|p| p.control != control)
                {
                    return Err("本次远控模式与电脑授权不一致，请重新连接".into());
                }
                let liveness = handoff::Liveness::new(&connection);
                #[cfg(target_os = "linux")]
                let liveness = self
                    .preparing
                    .as_ref()
                    .map_or(liveness, |p| p.liveness.clone());
                let capturer = capturer_for(portal.as_ref(), display, &liveness)?;
                let id = uuid::Uuid::new_v4().to_string();
                // Video failure (no track, no permission, no encoder) leaves
                // the JPEG path working; the reason goes back to the phone.
                // The track when this is a direct desktop connection; else
                // binary messages, for a phone that says it decodes them.
                let width = args["videoMaxWidth"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(1920)
                    .clamp(640, 3840);
                let mut relay = None;
                let out: Option<(Arc<dyn super::video::VideoOut>, &str)> =
                    match (&connection.video, args["video"] == true) {
                        (Some(sender), true) => Some((Arc::new(sender.clone()), "webrtc")),
                        (None, true) if args["videoTransport"] == "binary" => {
                            let out = Arc::new(super::video::RelayVideo::with_epoch(
                                connection.binary.clone(),
                                id.clone(),
                                connection.video_epoch.clone(),
                            ));
                            if args["deferVideo"] == true {
                                out.defer_until_ready();
                            }
                            relay = Some(out.clone());
                            Some((out, "binary"))
                        }
                        _ => None,
                    };
                let (video, video_error, transport) = match out {
                    Some((out, transport)) => {
                        match super::video::Session::start(
                            display,
                            width,
                            capturer.video_source(),
                            out,
                        ) {
                            Ok(session) => (Some(session), None, transport),
                            Err(error) => (None, Some(error), transport),
                        }
                    }
                    None if args["video"] == true => {
                        (None, Some("当前连接不支持视频画面".to_string()), "")
                    }
                    None => (None, None, ""),
                };
                if video.is_none() {
                    relay = None;
                }
                let mut described = video
                    .as_ref()
                    .map_or(Value::Null, super::video::Session::describe);
                if !described.is_null() {
                    described["transport"] = json!(transport);
                    if let Some(relay) = &relay {
                        if described.get("generation").is_none() {
                            described["generation"] = relay.stats()["generation"].clone();
                        }
                        described["clientReadyRequired"] = json!(
                            relay.stats()["clientReadyRequired"] == true
                                || described["clientReadyRequired"] == true
                        );
                    }
                }
                self.lease = Some(Lease {
                    liveness,
                    handoff: None,
                    handoff_revision: 0,
                    id: id.clone(),
                    owner: connection,
                    display,
                    expires: Instant::now() + Duration::from_secs(15),
                    sequence: 0,
                    cursor: os::cursor::Reader::default(),
                    controller: if control {
                        Some(controller_for(portal.as_ref())?)
                    } else {
                        None
                    },
                    last_frame: None,
                    capturer: Some(capturer),
                    video,
                    relay,
                    portal,
                });
                #[cfg(target_os = "linux")]
                {
                    if let Some(preparing) = &mut self.preparing {
                        preparing.transferred = true;
                    }
                    self.preparing = None;
                }
                let fps = Duration::from_secs(1).as_millis() / MIN_FRAME_INTERVAL.as_millis();
                // `inputBatch`: one sequence number covers an ordered list of events.
                Ok(
                    json!({"leaseId":id,"monitorId":display,"expiresInSeconds":15,"inputSequence":"0","transport":TRANSPORT,"maxFps":fps,"inputBatch":MAX_INPUT_BATCH,"cursor":true,
                        "video":described,"videoError":video_error,"handoff":self.lease.as_ref().is_some_and(|l|l.video.is_some()),"handoffRevision":"0"}),
                )
            }
            "remote_desktop_close" => {
                if let Some(lease) = &self.lease {
                    if lease.owner.id != connection.id || args["leaseId"] != lease.id {
                        return Err("远控租约不属于当前连接".into());
                    }
                    self.lease = None;
                }
                Ok(json!({"closed":true}))
            }
            // Keeps a video lease alive (it has no frame pulls) and carries
            // keyframe requests from the phone's decoder.
            "remote_desktop_video" => {
                let lease = self
                    .lease
                    .as_mut()
                    .filter(|lease| lease.owner.id == connection.id && args["leaseId"] == lease.id)
                    .ok_or("远控租约已结束，请重新连接")?;
                lease.expires = Instant::now() + Duration::from_secs(15);
                let video = lease.video.as_ref().ok_or("视频画面未开启")?;
                if let Some(width) = args.get("targetWidth").and_then(Value::as_u64) {
                    let width = u32::try_from(width).map_err(|_| "视频尺寸无效")?;
                    super::parse_sequence(args.get_mut("viewRequest"))?;
                    let seq = args["viewRequest"].as_u64().ok_or("缺少视频配置请求序号")?;
                    let instance = args["videoInstance"].as_str().ok_or("缺少视频实例身份")?;
                    video.request_width(instance, seq, width)?;
                }
                if args["ready"] == true {
                    super::parse_sequence(args.get_mut("generation"))?;
                    let generation = args["generation"].as_u64().ok_or("缺少视频代次")?;
                    if let Some(relay) = &lease.relay {
                        relay.activate(generation)?;
                    }
                    video.activate_rtp(generation)?;
                }
                if args["keyframe"] == true {
                    video.request_keyframe();
                }
                let mut stats = video.stats();
                let mut description = video.describe();
                description["transport"] = json!(if lease.relay.is_some() {
                    "binary"
                } else {
                    "webrtc"
                });
                if let Some(relay) = &lease.relay {
                    description["clientReadyRequired"] = json!(
                        relay.stats()["clientReadyRequired"] == true
                            || description["clientReadyRequired"] == true
                    );
                }
                stats["video"] = description;
                stats["handoffRevision"] = json!(lease.handoff_revision.to_string());

                // Cursor reads are independent from input control. Never query
                // unrestricted desktop APIs for a Portal-owned or embedded source.
                if args["includeCursor"] == true {
                    stats["cursor"] =
                        if lease.portal.is_none() && stats["video"]["cursorEmbedded"] == false {
                            lease
                                .capturer
                                .as_ref()
                                .and_then(|capturer| {
                                    lease.cursor.snapshot(
                                        &capturer.display,
                                        args["knownCursorShapeId"].as_str(),
                                    )
                                })
                                .unwrap_or(Value::Null)
                        } else {
                            Value::Null
                        };
                    os::require_unlocked()?;
                    if !lease.owner.alive() || Instant::now() >= lease.expires {
                        return Err("远控租约已结束，请重新连接".into());
                    }
                }
                if let Some(relay) = &lease.relay {
                    super::parse_sequence(args.get_mut("ack"))?;
                    if let Some(seq) = args["ack"].as_u64() {
                        relay.ack(seq);
                    }
                    stats["relay"] = relay.stats();
                }
                Ok(stats)
            }
            "remote_desktop_frame" | "remote_desktop_input" | "remote_desktop_select" => {
                let lease = self
                    .lease
                    .as_mut()
                    .filter(|lease| lease.owner.id == connection.id && args["leaseId"] == lease.id)
                    .ok_or("远控租约已结束，请重新连接")?;
                lease.expires = Instant::now() + Duration::from_secs(15);
                if method == "remote_desktop_frame"
                    && lease
                        .last_frame
                        .is_some_and(|last| last.elapsed() < MIN_FRAME_INTERVAL)
                {
                    return Err("画面请求过于频繁".into());
                }
                let result = (|| match method {
                    "remote_desktop_frame" => {
                        let width = args["maxWidth"].as_u64().unwrap_or(1920);
                        let quality = args["quality"].as_u64().unwrap_or(70);
                        lease.last_frame = Some(Instant::now());
                        // Reopen after a failed capture or a display change.
                        let capturer = match lease.capturer.take() {
                            Some(capturer) => capturer,
                            None => {
                                capturer_for(lease.portal.as_ref(), lease.display, &lease.liveness)?
                            }
                        };
                        let capturer = lease.capturer.insert(capturer);
                        let mut frame = capturer.frame(
                            u32::try_from(width).map_err(failure)?,
                            u8::try_from(quality).map_err(failure)?,
                            args["knownFrameId"].as_str(),
                        );
                        if frame.is_err() {
                            lease.capturer = None;
                        } else if let (Ok(frame), Some(capturer)) =
                            (frame.as_mut(), lease.capturer.as_ref())
                        {
                            frame["cursor"] =
                                if lease.portal.is_none() && frame["cursorEmbedded"] == false {
                                    lease
                                        .cursor
                                        .snapshot(
                                            &capturer.display,
                                            args["knownCursorShapeId"].as_str(),
                                        )
                                        .unwrap_or(Value::Null)
                                } else {
                                    Value::Null
                                };
                        }
                        if frame.is_ok() {
                            // Cursor queries/PNG conversion can outlive the
                            // capture's final check; revalidate before returning.
                            os::require_unlocked()?;
                            if !lease.owner.alive() || Instant::now() >= lease.expires {
                                return Err("远控租约已结束，请重新连接".into());
                            }
                        }
                        frame
                    }
                    "remote_desktop_select" => {
                        let display = args["monitorId"]
                            .as_u64()
                            .and_then(|v| u32::try_from(v).ok())
                            .ok_or("显示器标识无效")?;
                        if !displays_for(lease.portal.as_ref())?
                            .iter()
                            .any(|m| m.id == display)
                        {
                            return Err("显示器已断开".into());
                        }
                        let capturer =
                            capturer_for(lease.portal.as_ref(), display, &lease.liveness)?;
                        let controller = lease
                            .controller
                            .as_ref()
                            .map(|_| controller_for(lease.portal.as_ref()))
                            .transpose()?;
                        // Retire any drag or held modifiers on the previous
                        // screen before accepting coordinates on its replacement.
                        lease.controller = controller;
                        let source = capturer.video_source();
                        lease.capturer = Some(capturer);
                        lease.display = display;
                        let video = match lease.video.take() {
                            Some(video) => Some(video.restart(display, source)?),
                            None => None,
                        };
                        let mut described = video
                            .as_ref()
                            .map_or(Value::Null, super::video::Session::describe);
                        if !described.is_null() {
                            if let Some(relay) = &lease.relay {
                                described["transport"] = json!("binary");
                                if described.get("generation").is_none() {
                                    described["generation"] = relay.stats()["generation"].clone();
                                }
                                described["clientReadyRequired"] = json!(
                                    relay.stats()["clientReadyRequired"] == true
                                        || described["clientReadyRequired"] == true
                                );
                            } else {
                                described["transport"] = json!("webrtc");
                            }
                        }
                        lease.video = video;
                        Ok(json!({"monitorId":display,"video":described}))
                    }
                    _ => {
                        super::parse_sequence(args.get_mut("sequence"))?;
                        let sequence = args["sequence"]
                            .as_u64()
                            .filter(|seq| Some(*seq) == lease.sequence.checked_add(1))
                            .ok_or("输入序号冲突，禁止重放键鼠操作")?;
                        let events = input_events(&args)?;
                        lease.sequence = sequence; // Consumed even if the OS reports an uncertain effect.
                        let controller = lease.controller.as_mut().ok_or("当前连接为仅查看模式")?;
                        let display = match &lease.capturer {
                            Some(capturer) => capturer.display.clone(),
                            None => displays_for(lease.portal.as_ref())?
                                .into_iter()
                                .find(|m| m.id == lease.display)
                                .ok_or("显示器已断开")?,
                        };
                        // A slow native geometry reply must not turn an old
                        // gesture into a later action on a retired connection.
                        let (owner, expires) = (
                            lease.owner.clone(),
                            lease.expires.min(Instant::now() + Duration::from_secs(2)),
                        );
                        controller.send_batch_guarded(&display, events, || {
                            owner.alive() && Instant::now() < expires
                        })?;
                        Ok(json!({"sequence":sequence.to_string()}))
                    }
                })();
                if result.is_err() {
                    self.lease = None;
                }
                result
            }
            _ => Err("remote native operation unavailable".into()),
        }
    }
}

/// `event` (one) or `events` (an ordered batch); all are parsed before any runs.
fn input_events(args: &Value) -> Result<Vec<os::Input>, String> {
    let raw = match (&args["event"], &args["events"]) {
        (event, Value::Null) if !event.is_null() => vec![event.clone()],
        (Value::Null, Value::Array(events)) if (1..=MAX_INPUT_BATCH).contains(&events.len()) => {
            events.clone()
        }
        _ => return Err("输入事件无效".into()),
    };
    raw.into_iter()
        .map(|event| serde_json::from_value(event).map_err(failure))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ending_a_video_lease_retires_its_transport_epoch() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = Disconnect::new(None, tx.clone());
        let epoch = connection.0.video_epoch.clone();
        let relay = Arc::new(super::super::video::RelayVideo::with_epoch(
            tx,
            "fixture".into(),
            epoch.clone(),
        ));
        assert_eq!(epoch.load(Ordering::Acquire), 1);
        relay.reset();
        assert_eq!(epoch.load(Ordering::Acquire), 2);
        drop(Lease {
            liveness: handoff::Liveness::new(&connection.0),
            handoff: None,
            handoff_revision: 0,
            id: "fixture".into(),
            owner: connection.0.clone(),
            display: 0,
            expires: Instant::now(),
            sequence: 0,
            cursor: os::cursor::Reader::default(),
            controller: None,
            last_frame: None,
            capturer: None,
            video: None,
            relay: Some(relay),
            portal: None,
        });
        assert_eq!(epoch.load(Ordering::Acquire), 3);
    }
    #[test]
    fn input_accepts_one_event_or_a_bounded_batch() {
        let one = json!({"event":{"kind":"move","x":1,"y":2}});
        assert_eq!(input_events(&one).unwrap().len(), 1);
        let batch = json!({"events":[{"kind":"move","x":1,"y":2},
            {"kind":"button","button":"left","direction":"press"}]});
        assert_eq!(input_events(&batch).unwrap().len(), 2);
        let too_many =
            json!({"events":vec![json!({"kind":"move","x":1,"y":2}); MAX_INPUT_BATCH + 1]});
        assert!(input_events(&too_many).is_err());
        assert!(input_events(&json!({"events":[]})).is_err());
        assert!(input_events(&json!({})).is_err());
        // One bad event rejects the whole batch before anything is injected.
        let bad = json!({"events":[{"kind":"move","x":1,"y":2},{"kind":"teleport"}]});
        assert!(input_events(&bad).is_err());
        let both =
            json!({"event":{"kind":"text","text":"a"},"events":[{"kind":"text","text":"b"}]});
        assert!(input_events(&both).is_err());
    }
}
