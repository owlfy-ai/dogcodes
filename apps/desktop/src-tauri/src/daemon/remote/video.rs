//! One screen video session: capture + platform encode on its own thread,
//! encoder bitrate and frame rate following the output's rate estimate. The
//! output is the direct connection's H.264 track (WebRTC congestion control)
//! or, when WebRTC cannot connect, binary messages on the authenticated
//! connection itself with acknowledgement-paced flow control.
use dogcodes_remote_desktop::video::{
    EncodeAdmission, EncodedFrame, Source, VideoConfig, VideoProfile, VideoStream,
};
use dogcodes_remote_transport::VideoSender;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

mod output;
mod qos;
mod recovery;
mod resize;
use output::Output;
use qos::RelayQos;
use resize::{Request as ResizeRequest, Resize};

/// Where encoded frames go.
pub(super) trait VideoOut: Send + Sync {
    /// Nonblocking. `false` when not sent; the next frame should be an IDR.
    fn push(&self, frame: EncodedFrame, captured: Instant) -> bool;
    /// Bits per second the path can carry now.
    fn target_bitrate(&self) -> u64;
    fn take_keyframe_request(&self) -> bool;
    fn closed(&self) -> bool;
    /// Advisory credit before codec work. Stateful outputs retain recovery
    /// until a real IDR is accepted, including while no credit is available.
    fn admission(&self) -> EncodeAdmission {
        if self.ready() && !self.closed() {
            EncodeAdmission::Frame
        } else {
            EncodeAdmission::Wait
        }
    }
    fn begin_recovery(&self) {}
    fn reset(&self) {}
    fn retire(&self) {}
    fn codec_applied(&self, _profile: dogcodes_remote_desktop::video::VideoProfile) {}
    fn can_resize(&self) -> bool {
        true
    }
    fn ready(&self) -> bool {
        true
    }
    fn ready_required(&self) -> bool {
        false
    }
    fn video_epoch(&self) -> Option<u64> {
        None
    }
    fn profile(&self) -> dogcodes_remote_desktop::video::VideoProfile {
        dogcodes_remote_desktop::video::VideoProfile::ConstrainedBaselineAuto
    }
}

impl VideoOut for VideoSender {
    fn reset(&self) {
        VideoSender::reset(self);
    }
    fn retire(&self) {
        VideoSender::reset(self);
    }
    fn admission(&self) -> EncodeAdmission {
        if self.can_encode() {
            EncodeAdmission::Frame
        } else {
            EncodeAdmission::Wait
        }
    }
    fn profile(&self) -> dogcodes_remote_desktop::video::VideoProfile {
        dogcodes_remote_desktop::video::VideoProfile::ConstrainedBaseline31
    }
    fn push(&self, frame: EncodedFrame, captured: Instant) -> bool {
        VideoSender::push(self, frame.data, captured)
    }
    fn target_bitrate(&self) -> u64 {
        VideoSender::target_bitrate(self)
    }
    fn take_keyframe_request(&self) -> bool {
        VideoSender::take_keyframe_request(self)
    }
    fn closed(&self) -> bool {
        VideoSender::closed(self)
    }
}

pub(super) type BinaryOut = tokio::sync::mpsc::Sender<(Value, Vec<u8>)>;

struct Sent {
    seq: u64,
    at: Instant,
    bytes: usize,
}

struct Relay {
    out: BinaryOut,
    epoch: Arc<AtomicU64>,
    hold_until: Option<Instant>,
    seq: u64,
    generation: u64,
    require_ready: bool,
    ready: bool,
    acknowledged: Option<u64>,
    inflight: VecDeque<Sent>,
    qos: RelayQos,
    /// A frame was not sent: deltas are useless until the next IDR.
    resync: bool,
    stalled_at: Option<Instant>,
}

impl Relay {
    fn check_stall(&mut self, now: Instant) {
        if self.ready
            && self
                .inflight
                .front()
                .is_some_and(|sent| now.duration_since(sent.at) >= RelayQos::STALL)
            && self
                .stalled_at
                .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
        {
            self.stalled_at = Some(now);
            self.qos.on_stall(now);
        }
    }
}

/// Frames as binary messages on the connection, numbered; the phone acks the
/// highest one it handed to its decoder. At most [`Self::WINDOW`] frames and
/// about 0.4 s of data are ever unacknowledged, so a slow relay drops frames
/// here rather than queueing them on the way.
pub(super) struct RelayVideo {
    lease: String,
    state: Mutex<Relay>,
}

impl RelayVideo {
    const WINDOW: usize = 8;
    // A recovery picture can exceed the steady-state byte budget. Admit one
    // alone, with a separate hard bound, so requesting an IDR can make progress.
    const MAX_KEYFRAME_BYTES: usize = 4 * 1024 * 1024;

    #[cfg(test)]
    pub(super) fn new(out: BinaryOut, lease: String) -> Self {
        Self::with_generation(out, lease, Arc::new(AtomicU64::new(0)), 0)
    }
    pub(super) fn with_epoch(out: BinaryOut, lease: String, epoch: Arc<AtomicU64>) -> Self {
        let generation = epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        Self::with_generation(out, lease, epoch, generation)
    }
    fn with_generation(
        out: BinaryOut,
        lease: String,
        epoch: Arc<AtomicU64>,
        generation: u64,
    ) -> Self {
        Self {
            lease,
            state: Mutex::new(Relay {
                out,
                epoch,
                hold_until: None,
                seq: 0,
                generation,
                require_ready: false,
                ready: true,
                acknowledged: None,
                inflight: VecDeque::new(),
                qos: RelayQos::new(),
                resync: false,
                stalled_at: None,
            }),
        }
    }

    /// A new client promises to acknowledge readiness after decoder/subscription
    /// creation. Older clients keep the previous immediate streaming behavior.
    pub(super) fn defer_until_ready(&self) {
        let mut state = self.state();
        state.require_ready = true;
        state.ready = false;
        state.acknowledged = None;
        state.resync = true;
    }
    pub(super) fn activate(&self, generation: u64) -> Result<(), String> {
        let mut state = self.state();
        if generation != state.generation {
            return Err("视频代次已变化，请重新打开画面".into());
        }
        if !state.ready {
            state.ready = true;
            state.resync = true;
        }
        Ok(())
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Relay> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The phone has handed every frame up to `seq` to its decoder.
    pub(super) fn ack(&self, seq: u64) {
        let now = Instant::now();
        let mut state = self.state();
        if seq > state.seq {
            return;
        }
        let mut newest = None;
        while state.inflight.front().is_some_and(|s| s.seq <= seq) {
            newest = state.inflight.pop_front();
        }
        if let Some(sent) = newest {
            state.acknowledged = Some(state.generation);
            if state.inflight.is_empty() {
                state.stalled_at = None;
            }
            state.qos.on_ack(sent.at, now);
        }
    }

    pub(super) fn stats(&self) -> Value {
        let state = self.state();
        json!({"sentSeq":state.seq,"inflight":state.inflight.len(),"relayBitrate":state.qos.bitrate(),"generation":state.generation,"clientReadyRequired":state.require_ready,"ready":state.ready})
    }

    /// During a staged migration the authenticated candidate keeps the source
    /// alive if the old wire ends. No frames are queued for the candidate yet.
    pub(super) fn hold(&self, until: Option<Instant>) {
        self.state().hold_until = until;
    }
    pub(super) fn rebind(&self, out: BinaryOut, epoch: Arc<AtomicU64>) -> u64 {
        let mut state = self.state();
        state.epoch.fetch_add(1, Ordering::AcqRel); // retire old queued fragments
        epoch.fetch_max(state.generation, Ordering::AcqRel);
        state.generation = epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        state.epoch = epoch;
        state.out = out;
        state.hold_until = None;
        state.inflight.clear();
        state.qos = RelayQos::new();
        state.require_ready = true;
        state.ready = false;
        state.acknowledged = None;
        state.resync = true;
        state.stalled_at = None;
        state.generation
    }
    pub(super) fn reset(&self) {
        let mut state = self.state();
        state.generation = state.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        state.inflight.clear();
        if state.require_ready {
            state.ready = false;
            state.acknowledged = None;
        }
        state.resync = true;
        state.stalled_at = None;
    }
}

impl VideoOut for RelayVideo {
    fn begin_recovery(&self) {
        self.state().resync = true;
    }
    fn admission(&self) -> EncodeAdmission {
        let state = self.state();
        if !state.ready || state.out.is_closed() || state.out.capacity() == 0 {
            return EncodeAdmission::Wait;
        }
        // A recovery picture may use the lone-frame exception to the byte
        // budget. Wait for old cumulative ACKs instead of encoding IDRs that
        // cannot fit. Decoder failures/skips also ACK on the phone.
        if state.resync {
            return if state.inflight.is_empty() {
                EncodeAdmission::Keyframe
            } else {
                EncodeAdmission::Wait
            };
        }
        let queued: usize = state.inflight.iter().map(|sent| sent.bytes).sum();
        let budget = usize::try_from(state.qos.bitrate() / 8 * 2 / 5)
            .unwrap_or(usize::MAX)
            .max(256 * 1024);
        if state.inflight.len() >= Self::WINDOW || queued >= budget {
            EncodeAdmission::Wait
        } else {
            EncodeAdmission::Frame
        }
    }
    fn retire(&self) {
        let mut state = self.state();
        state.epoch.fetch_add(1, Ordering::AcqRel);
        state.ready = false;
    }

    fn ready(&self) -> bool {
        self.state().ready
    }
    fn ready_required(&self) -> bool {
        self.state().require_ready
    }
    fn video_epoch(&self) -> Option<u64> {
        Some(self.state().generation)
    }
    fn can_resize(&self) -> bool {
        let s = self.state();
        s.ready && (!s.require_ready || s.acknowledged == Some(s.generation))
    }
    fn reset(&self) {
        RelayVideo::reset(self);
    }
    fn push(&self, frame: EncodedFrame, _captured: Instant) -> bool {
        let now = Instant::now();
        let mut state = self.state();
        if !state.ready {
            return false;
        }
        state.check_stall(now);
        if state.resync && !frame.keyframe {
            return false;
        }
        let queued: usize = state.inflight.iter().map(|s| s.bytes).sum();
        let budget = usize::try_from(state.qos.bitrate() / 8 * 2 / 5)
            .unwrap_or(usize::MAX)
            .max(256 * 1024);
        let recovery = frame.keyframe
            && state.inflight.is_empty()
            && frame.data.len() <= Self::MAX_KEYFRAME_BYTES;
        if state.inflight.len() >= Self::WINDOW || (queued + frame.data.len() > budget && !recovery)
        {
            state.resync = true;
            return false;
        }
        let seq = state.seq + 1;
        let bytes = frame.data.len();
        let header = json!({"type":"video","leaseId":self.lease,"seq":seq,"generation":state.generation,"key":frame.keyframe});
        if state.out.try_send((header, frame.data)).is_err() {
            state.resync = true;
            return false;
        }
        state.seq = seq;
        state.inflight.push_back(Sent {
            seq,
            at: now,
            bytes,
        });
        if frame.keyframe {
            state.resync = false;
        }
        true
    }
    fn target_bitrate(&self) -> u64 {
        let mut state = self.state();
        // The owner's 100ms control tick continues even on a static desktop.
        state.check_stall(Instant::now());
        state.qos.bitrate()
    }
    fn take_keyframe_request(&self) -> bool {
        // Recovery is a retained admission state, not a delayed consumable
        // bit that could rearm an already delivered activation IDR.
        false
    }
    fn closed(&self) -> bool {
        let state = self.state();
        state.out.is_closed() && state.hold_until.is_none_or(|until| Instant::now() >= until)
    }
}

/// Encoder settings for a congestion-control target. The encoder aims a
/// little under the estimate so the pacer keeps no standing queue; frame rate
/// drops on thin links (fewer, sharper frames read better than many blurry
/// ones, as the `RustDesk` `QoS` also trades), and the bitrate is capped by what
/// the picture size can use.
pub(super) fn rate_policy(target: u64, pixels: u64) -> (u32, u32) {
    // ~0.1 bit per pixel per frame at 30 fps is plenty for screen content.
    let useful = (pixels * 3).max(1_000_000);
    let bitrate = (target * 85 / 100).min(useful);
    let fps = match bitrate {
        0..600_000 => 12,
        600_000..1_200_000 => 20,
        _ => 30,
    };
    (u32::try_from(bitrate).unwrap_or(u32::MAX), fps)
}

#[derive(Default)]
struct Stats {
    recovery: recovery::Recovery,
    frames: AtomicU64,
    keyframes: AtomicU64,
    bytes: AtomicU64,
    dropped: AtomicU64,
    latency_us: AtomicU64,
    bitrate: AtomicU32,
    fps: AtomicU32,
    target: AtomicU64,
    error: Mutex<Option<String>>,
}

/// A stopped source can finish a slow OS call later. Serialize stop with the
/// final nonblocking publish, before a monitor reset can reuse its transport.
struct Publication {
    stop: Arc<AtomicBool>,
    gate: Mutex<()>,
    epoch: AtomicU64,
    cursor_mode: AtomicU8,
}
impl Publication {
    #[cfg(test)]
    fn new(stop: Arc<AtomicBool>) -> Self {
        Self::with_cursor(stop, None)
    }
    fn with_cursor(stop: Arc<AtomicBool>, cursor: Option<bool>) -> Self {
        Self {
            stop,
            gate: Mutex::new(()),
            epoch: AtomicU64::new(0),
            cursor_mode: AtomicU8::new(match cursor {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }),
        }
    }
    fn cursor_embedded(&self) -> Option<bool> {
        match self.cursor_mode.load(Ordering::Acquire) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }
    #[cfg(test)]
    fn push(&self, sender: &dyn VideoOut, frame: EncodedFrame, captured: Instant) -> bool {
        self.push_epoch(0, sender, frame, captured)
    }
    fn push_epoch(
        &self,
        epoch: u64,
        sender: &dyn VideoOut,
        frame: EncodedFrame,
        captured: Instant,
    ) -> bool {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.stop.load(Ordering::Acquire)
            || self.epoch.load(Ordering::Acquire) != epoch
            || sender.closed()
        {
            return false;
        }
        let cursor = frame.cursor_embedded;
        if !sender.push(frame, captured) {
            return false;
        }
        // Under the same publication gate: dropped/retired frames cannot change
        // metadata after a newer stream has published its own pixels.
        self.cursor_mode.store(
            match cursor {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            },
            Ordering::Release,
        );
        true
    }
    fn advance(&self) -> u64 {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
    fn close(&self) {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.stop.store(true, Ordering::Release);
    }
}
static SESSION_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct WorkerSlot;
impl WorkerSlot {
    fn take() -> Result<Self, String> {
        SESSION_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "旧视频会话仍在停止，暂时使用图片模式".into())
    }
}
impl Drop for WorkerSlot {
    fn drop(&mut self) {
        SESSION_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Session {
    sender: Arc<Output>,
    resize: Arc<Resize>,
    publication: Arc<Publication>,
    stats: Arc<Stats>,
    thread: Option<std::thread::JoinHandle<()>>,
    started: Instant,
}

impl Session {
    /// Starts capture of `display` into `sender`; returns once the first
    /// capture call answered (or failed, e.g. without screen-recording
    /// permission).
    pub(super) fn start(
        display: u32,
        max_width: u32,
        source: Source,
        sender: Arc<dyn VideoOut>,
    ) -> Result<Self, String> {
        let sender = Arc::new(Output::new(sender));
        let slot = WorkerSlot::take()?;
        let cursor_embedded =
            source
                .as_ref()
                .map(|s| s.cursor_embedded())
                .or(if cfg!(target_os = "windows") {
                    None
                } else {
                    Some(cfg!(target_os = "macos"))
                });
        let spec = CaptureSpec {
            display,
            max_width,
            source,
        };
        let resize = Arc::new(Resize::new(max_width));
        let stop = Arc::new(AtomicBool::new(false));
        let publication = Arc::new(Publication::with_cursor(stop.clone(), cursor_embedded));
        let stats = Arc::new(Stats::default());
        let (ready, started) = mpsc::sync_channel(1);
        let thread = {
            let stats = stats.clone();
            let sender = sender.clone();
            let publication = publication.clone();
            let resize = resize.clone();
            std::thread::Builder::new()
                .name("remote-video".into())
                .spawn(move || {
                    let _slot = slot;
                    run(spec, &sender, &stats, &ready, publication, &resize);
                })
                .map_err(|_| "无法启动视频线程".to_string())?
        };
        let size = started
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "视频启动超时".to_string())
            .and_then(|r| r);
        match size {
            Ok(_) => Ok(Self {
                sender,
                resize,
                publication,
                stats,
                thread: Some(thread),
                started: Instant::now(),
            }),
            Err(error) => {
                publication.close();
                // The OS may still be initializing capture. Its bounded worker
                // retains ownership and cleans up later, without blocking input.
                drop(thread);
                Err(error)
            }
        }
    }

    /// The same track, now showing another display.
    pub(super) fn restart(self, display: u32, source: Source) -> Result<Self, String> {
        let (sender, max_width) = (self.sender.original(), self.resize.target());
        drop(self);
        sender.reset();
        Self::start(display, max_width, source, sender)
    }

    pub(super) fn request_keyframe(&self) {
        self.sender.begin_recovery();
    }

    #[cfg(test)]
    pub(super) fn fixture(sender: Arc<dyn VideoOut>) -> Self {
        let sender = Arc::new(Output::new(sender));
        let resize = Arc::new(Resize::new(640));
        resize.applied(
            ResizeRequest { seq: 0, width: 640 },
            (640, 400),
            1280,
            sender.video_epoch(),
        );
        Self {
            sender,
            resize,
            publication: Arc::new(Publication::with_cursor(
                Arc::new(AtomicBool::new(false)),
                Some(false),
            )),
            stats: Arc::new(Stats::default()),
            thread: None,
            started: Instant::now(),
        }
    }
    #[cfg(test)]
    pub(super) fn fixture_codec_ready(&self) {
        self.sender.codec_applied(self.sender.profile());
    }
    pub(super) fn migration_ready(&self) -> bool {
        !self.publication.stop.load(Ordering::Acquire)
            && !self.sender.closed()
            && self.sender.can_resize()
            && self.stats()["error"].is_null()
            && !self.stats.recovery.active()
            && self.resize.migration_ready()
    }
    pub(super) fn hold(&self, until: Option<Instant>) -> Result<(), String> {
        self.sender.hold(until)
    }
    pub(super) fn replace_output(&self, sender: Arc<dyn VideoOut>) -> Result<u64, String> {
        let generation = self.sender.replace(sender)?;
        self.migrated(generation);
        Ok(generation)
    }
    pub(super) fn activate_rtp(&self, generation: u64) -> Result<(), String> {
        self.sender.activate(generation)
    }
    pub(super) fn migrated(&self, generation: u64) {
        self.sender.migrated(generation);
        self.resize.migrated(generation);
    }
    pub(super) fn request_width(&self, instance: &str, seq: u64, width: u32) -> Result<(), String> {
        self.resize.request(instance, seq, width)
    }
    pub(super) fn describe(&self) -> Value {
        let mut value = self.resize.describe();
        value["cursorEmbedded"] = json!(self.publication.cursor_embedded());
        value["profile"] = json!(match self.sender.profile() {
            dogcodes_remote_desktop::video::VideoProfile::ConstrainedBaseline31 => "cb31",
            dogcodes_remote_desktop::video::VideoProfile::ConstrainedBaselineAuto => "cb-auto",
        });
        value["codecReady"] = json!(self.sender.codec_ready());
        value["clientReadyRequired"] = json!(self.sender.required());
        value["ready"] = json!(self.sender.ready());
        value
    }

    pub(super) fn stats(&self) -> Value {
        let s = &self.stats;
        let frames = s.frames.load(Ordering::Acquire);
        json!({
            "frames": frames,
            "recovery": s.recovery.describe(),
            "keyframes": s.keyframes.load(Ordering::Acquire),
            "dropped": s.dropped.load(Ordering::Acquire),
            "bytes": s.bytes.load(Ordering::Acquire).to_string(),
            "encodeLatencyMs": s.latency_us.load(Ordering::Acquire) / 1000,
            "bitrate": s.bitrate.load(Ordering::Acquire),
            "fps": s.fps.load(Ordering::Acquire),
            "targetBitrate": s.target.load(Ordering::Acquire),
            "seconds": self.started.elapsed().as_secs_f64(),
            "error": s.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
                .or_else(||s.recovery.timeout())
                .or_else(||if s.recovery.active(){None}else{self.resize.timeout()}),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.publication.close();
        // Never join a potentially blocked capture/encoder teardown on the
        // input worker. The worker slot bounds outstanding native operations.
        drop(self.thread.take());
    }
}

/// The session thread owns the capture stream (`ScreenCaptureKit` objects stay
/// on one thread) and runs the rate loop until stopped or disconnected.
struct CaptureSpec {
    display: u32,
    max_width: u32,
    source: Source,
}
fn publish_frame(
    stats: &Stats,
    publication: &Publication,
    epoch: u64,
    sender: &dyn VideoOut,
    frame: EncodedFrame,
) {
    let len = frame.data.len() as u64;
    let (keyframe, latency) = (frame.keyframe, frame.latency);
    let captured = frame.captured;
    if publication.push_epoch(epoch, sender, frame, captured) {
        stats.frames.fetch_add(1, Ordering::AcqRel);
        stats.bytes.fetch_add(len, Ordering::AcqRel);
        if keyframe {
            stats.keyframes.fetch_add(1, Ordering::AcqRel);
            stats.recovery.observed_idr();
        }
        // Smoothed encode latency (1/8 per frame).
        let sample = u64::try_from(latency.as_micros()).unwrap_or(u64::MAX);
        let old = stats.latency_us.load(Ordering::Acquire);
        stats
            .latency_us
            .store(old - old / 8 + sample / 8, Ordering::Release);
    } else {
        // Later frames reference the dropped one; restart from an IDR.
        stats.dropped.fetch_add(1, Ordering::AcqRel);
    }
}

fn open_stream(
    spec: &CaptureSpec,
    width: u32,
    sender: &Arc<Output>,
    stats: &Arc<Stats>,
    publication: Arc<Publication>,
    epoch: u64,
) -> Result<VideoStream, String> {
    let parent_stop = publication.stop.clone();
    let sink = {
        let (sender, stats) = (sender.clone(), stats.clone());
        Arc::new(move |frame: EncodedFrame| {
            publish_frame(&stats, &publication, epoch, sender.as_ref(), frame);
        })
    };
    let (bitrate, fps) = rate_policy(sender.target_bitrate(), 0);
    let downstream = sender.clone();
    VideoStream::start_controlled(
        VideoConfig {
            display: spec.display,
            max_width: width,
            profile: sender.profile(),
            fps,
            bitrate,
        },
        sink,
        spec.source.clone(),
        Arc::new(move || downstream.admission()),
        parent_stop,
    )
}
fn run(
    spec: CaptureSpec,
    sender: &Arc<Output>,
    stats: &Arc<Stats>,
    ready: &mpsc::SyncSender<Result<(u32, u32), String>>,
    publication: Arc<Publication>,
    resize: &Resize,
) {
    let stop = publication.stop.clone();
    let initial = ResizeRequest {
        seq: 0,
        width: spec.max_width,
    };
    let mut stream = match open_stream(&spec, initial.width, sender, stats, publication.clone(), 0)
    {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let size = stream.size();
    let mut limit = stream.size_for_width(3840).map_or(size.0, |s| s.0);
    let mut profile = sender.profile();
    resize.applied(initial, size, limit, sender.video_epoch());
    let _ = ready.send(Ok(size));
    let mut geometry_rebuilds = VecDeque::new();
    let mut waiting_for_geometry = false;
    while !stop.load(Ordering::Acquire) && !sender.closed() {
        if stream.reconfiguring() {
            if !waiting_for_geometry {
                resize.begin();
                waiting_for_geometry = true;
            }
            if let Some(error) = resize.timeout() {
                *stats
                    .error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                break;
            }
            // A temporary native stop during mode change does not end the
            // lease before Quartz has committed the new display configuration.
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        waiting_for_geometry = false;
        if profile != sender.profile() {
            resize.begin();
            let epoch = publication.advance();
            drop(stream);
            if stop.load(Ordering::Acquire) {
                return;
            }
            profile = sender.profile();
            let request = resize.latest();
            stream = match open_stream(
                &spec,
                request.width,
                sender,
                stats,
                publication.clone(),
                epoch,
            ) {
                Ok(stream) => stream,
                Err(error) => {
                    *stats
                        .error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                    return;
                }
            };
            let size = stream.size();
            limit = stream.size_for_width(3840).map_or(size.0, |s| s.0);
            resize.applied(request, size, limit, sender.video_epoch());
            sender.codec_applied(profile);
        }
        let geometry_changed = stream.needs_rebuild();
        // A source change cannot wait for an ACK of the old geometry. That
        // picture may never have reached the decoder. Reset starts a fresh
        // ready/IDR handshake, scoped to the same lease and source grant.
        let request = if geometry_changed {
            let now = Instant::now();
            geometry_rebuilds.retain(|at| now.duration_since(*at) < Duration::from_secs(10));
            if geometry_rebuilds.len() >= 8 {
                *stats
                    .error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some("显示器尺寸持续变化，请稳定显示设置后重试".into());
                break;
            }
            geometry_rebuilds.push_back(now);
            Some(resize.latest())
        } else {
            resize.pending().filter(|_| sender.can_resize())
        };
        if let Some(request) = request {
            let wanted = stream.size_for_width(request.width);
            if !geometry_changed && wanted.as_ref().is_ok_and(|size| *size == stream.size()) {
                let size = stream.size();
                limit = stream.size_for_width(3840).map_or(size.0, |s| s.0);
                resize.applied(request, size, limit, sender.video_epoch());
            } else {
                resize.begin();
                let epoch = publication.advance();
                // This may wait inside native teardown; the input/RPC worker
                // remains available, and every old callback is already fenced.
                drop(stream);
                if stop.load(Ordering::Acquire) {
                    return;
                }
                sender.reset();
                stream = match open_stream(
                    &spec,
                    request.width,
                    sender,
                    stats,
                    publication.clone(),
                    epoch,
                ) {
                    Ok(stream) => stream,
                    Err(error) => {
                        *stats
                            .error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                        return;
                    }
                };
                resize.applied(request, stream.size(), limit, sender.video_epoch());
            }
        }
        if let Some(error) = stream.error() {
            let rebuilt = recovery::rebuild(
                stream,
                error,
                recovery::Context {
                    sender,
                    stats,
                    resize,
                    publication: &publication,
                },
                || dogcodes_remote_desktop::video::check_recovery_source(&spec.source),
                |request, epoch| {
                    let stream = open_stream(
                        &spec,
                        request.width,
                        sender,
                        stats,
                        publication.clone(),
                        epoch,
                    )?;
                    let size = stream.size();
                    let limit = stream.size_for_width(3840).map_or(size.0, |s| s.0);
                    let profile = stream.profile();
                    Ok(recovery::Opened {
                        stream,
                        size,
                        limit,
                        profile,
                    })
                },
            );
            match rebuilt {
                Ok(opened) => {
                    stream = opened.stream;
                    limit = opened.limit;
                    profile = opened.profile;
                    continue;
                }
                Err(error) => {
                    *stats
                        .error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                    break;
                }
            }
        }
        if sender.take_keyframe_request() {
            sender.begin_recovery();
        }
        let size = stream.size();
        let pixels = u64::from(size.0) * u64::from(size.1);
        let target = sender.target_bitrate();
        let (bitrate, fps) = rate_policy(target, pixels);
        stream.set_bitrate(bitrate);
        stream.set_fps(fps);
        stats.target.store(target, Ordering::Release);
        stats.bitrate.store(bitrate, Ordering::Release);
        stats.fps.store(fps, Ordering::Release);
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::{EncodeAdmission, EncodedFrame, RelayQos, RelayVideo, VideoOut, rate_policy};
    use std::time::{Duration, Instant};

    fn frame(keyframe: bool, size: usize) -> EncodedFrame {
        EncodedFrame {
            cursor_embedded: None,
            captured: std::time::Instant::now(),
            data: vec![0; size],
            keyframe,
            latency: Duration::ZERO,
        }
    }

    #[test]
    fn relay_qos_climbs_on_clean_acks_and_cuts_on_queueing() {
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut q = RelayQos::new();
        for i in 0..40 {
            q.on_delay(ms(80), t0 + ms(i * 100));
        }
        let raised = q.bitrate();
        assert!(raised > RelayQos::new().bitrate(), "{raised}");
        // One late ack is noise; three in a row cut by a quarter.
        q.on_delay(ms(400), t0 + ms(5000));
        assert_eq!(q.bitrate(), raised);
        q.on_delay(ms(400), t0 + ms(5033));
        q.on_delay(ms(400), t0 + ms(5066));
        let reduced = q.bitrate();
        assert!(reduced.abs_diff(raised * 3 / 4) <= 1);
        // A second of excess halves, but not twice within 500 ms.
        q.on_delay(ms(1200), t0 + ms(5600));
        let halved = q.bitrate();
        assert!(halved.abs_diff(reduced / 2) <= 1);
        q.on_delay(ms(1500), t0 + ms(5700));
        assert_eq!(q.bitrate(), halved);
        for i in 0..20 {
            q.on_delay(ms(5000), t0 + ms(6200 + i * 600));
        }
        assert_eq!(q.bitrate(), 200_000);
    }

    #[test]
    fn relay_congestion_rising_queue_must_not_become_a_new_clean_baseline() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for i in 1..=120 {
            qos.on_delay(
                Duration::from_millis(80 + i * 3),
                start + Duration::from_millis(i * 50),
            );
        }
        assert!(
            qos.bitrate() < RelayQos::new().bitrate(),
            "a continuously growing queue was rewarded with a higher bitrate"
        );
    }

    #[test]
    fn relay_congestion_late_ack_bursts_cannot_immediately_undo_a_cut() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        qos.on_delay(Duration::from_millis(1200), start + Duration::from_secs(1));
        let cut = qos.bitrate();
        for i in 1..=20 {
            qos.on_delay(
                Duration::from_millis(80),
                start + Duration::from_millis(1000 + i),
            );
        }
        assert_eq!(
            qos.bitrate(),
            cut,
            "one burst bypassed recovery observation"
        );
    }

    #[test]
    fn relay_congestion_static_output_still_reduces_when_acks_stop() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        assert!(relay.push(frame(true, 1000), Instant::now()));
        relay.state().inflight.front_mut().unwrap().at =
            Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
        let target = relay.target_bitrate();
        assert!(
            target < RelayQos::new().bitrate(),
            "no new frame means no stall brake"
        );
        assert_eq!(
            relay.target_bitrate(),
            target,
            "polling must not repeatedly cut the same stall"
        );
    }

    #[test]
    fn relay_congestion_first_unacked_picture_cannot_set_a_stalled_baseline() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        assert!(relay.push(frame(true, 1000), Instant::now()));
        relay.state().inflight.front_mut().unwrap().at =
            Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
        assert!(relay.push(frame(false, 1000), Instant::now()));
        assert!(
            relay.state().qos.bitrate() < RelayQos::new().bitrate(),
            "an unanswered picture was treated as a good delay sample"
        );
    }

    #[test]
    fn relay_partial_late_ack_cannot_restart_the_stall_brake_clock() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        for i in 0..3 {
            assert!(relay.push(frame(i == 0, 1000), Instant::now()));
        }
        let now = Instant::now();
        {
            let mut state = relay.state();
            for sent in &mut state.inflight {
                sent.at = now.checked_sub(Duration::from_secs(3)).unwrap();
            }
            state.check_stall(now);
        }
        relay.ack(1);
        let mut state = relay.state();
        let cut = state.qos.bitrate();
        state.check_stall(now + Duration::from_millis(600));
        assert_eq!(
            state.qos.bitrate(),
            cut,
            "a partial late ACK repeated the same brake early"
        );
        state.check_stall(now + Duration::from_millis(1100));
        assert_eq!(state.qos.bitrate(), cut / 2);
    }

    #[test]
    fn deferred_video_waits_for_this_decoders_generation_and_restarts_with_idr() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        let now = Instant::now();
        relay.defer_until_ready();
        assert!(!relay.push(frame(true, 300 * 1024), now));
        assert_eq!(relay.stats()["inflight"], 0);
        assert!(rx.try_recv().is_err());
        relay.activate(0).unwrap();
        assert!(relay.admission() == EncodeAdmission::Keyframe);
        assert!(!relay.push(frame(false, 1000), now));
        assert!(relay.push(frame(true, 300 * 1024), now));
        assert_eq!(rx.try_recv().unwrap().0["seq"], 1);
        relay.activate(0).unwrap(); // A retry cannot discard the live window.
        assert_eq!(relay.stats()["inflight"], 1);
        assert!(!relay.take_keyframe_request());
        relay.reset();
        assert_eq!(relay.stats()["ready"], false);
        assert!(relay.activate(0).is_err());
        assert!(!relay.push(frame(true, 300 * 1024), now));
        relay.activate(1).unwrap();
        assert!(relay.push(frame(true, 300 * 1024), now));
        let header = rx.try_recv().unwrap().0;
        assert_eq!(header["generation"], 1);
        assert_eq!(header["seq"], 2);
    }

    #[test]
    fn a_new_configuration_waits_for_a_real_ack_in_its_own_generation() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        relay.defer_until_ready();
        assert!(!relay.can_resize());
        relay.activate(0).unwrap();
        assert!(!relay.can_resize());
        let now = Instant::now();
        assert!(relay.push(frame(true, 1000), now));
        rx.try_recv().unwrap();
        relay.ack(999);
        assert!(!relay.can_resize());
        relay.ack(1);
        assert!(relay.can_resize());
        relay.reset();
        relay.activate(1).unwrap();
        assert!(relay.push(frame(true, 1000), now));
        rx.try_recv().unwrap();
        relay.ack(1);
        assert!(!relay.can_resize());
        relay.ack(2);
        assert!(relay.can_resize());
    }

    #[test]
    fn relay_window_drops_until_the_next_keyframe() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let relay = RelayVideo::new(tx, "lease".into());
        let now = Instant::now();
        assert!(relay.push(frame(true, 1000), now));
        for _ in 0..7 {
            assert!(relay.push(frame(false, 1000), now));
        }
        // Window of 8 unacknowledged frames: refused, and deltas stay
        // refused even after acks until an IDR goes out.
        assert!(!relay.push(frame(false, 1000), now));
        relay.ack(8);
        assert!(!relay.push(frame(false, 1000), now));
        assert!(relay.push(frame(true, 1000), now));
        assert!(relay.push(frame(false, 1000), now));
        let (header, payload) = rx.try_recv().unwrap();
        assert_eq!(header["seq"], 1);
        assert_eq!(header["key"], true);
        assert_eq!(header["leaseId"], "lease");
        assert_eq!(payload.len(), 1000);
        assert_eq!(relay.stats()["sentSeq"], 10);
        // Byte budget: one oversized frame cannot be queued at 1.5 Mb/s.
        assert!(!relay.push(frame(true, 2_000_000), now));
    }

    #[test]
    fn full_window_pauses_encoding_and_recovery_credit_survives_partial_acks() {
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let relay = RelayVideo::new(tx, "lease".into());
        for i in 0..8 {
            assert!(relay.admission() == EncodeAdmission::Frame);
            assert!(relay.push(frame(i == 0, 1000), Instant::now()));
        }
        assert!(relay.admission() == EncodeAdmission::Wait);
        // An already-encoded asynchronous callback can race the credit check.
        assert!(!relay.push(frame(false, 1000), Instant::now()));
        relay.ack(4);
        assert!(relay.admission() == EncodeAdmission::Wait);
        relay.ack(8);
        for _ in 0..10 {
            assert!(relay.admission() == EncodeAdmission::Keyframe);
        }
        assert!(relay.push(frame(true, 300 * 1024), Instant::now()));
        assert!(relay.admission() == EncodeAdmission::Wait);
        relay.ack(9);
        assert!(relay.admission() == EncodeAdmission::Frame);
    }

    #[test]
    fn wire_queue_credit_and_explicit_keyframe_requests_do_not_discard_recovery() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let relay = RelayVideo::new(tx, "lease".into());
        assert!(relay.push(frame(true, 1000), Instant::now()));
        assert!(relay.admission() == EncodeAdmission::Wait);
        rx.try_recv().unwrap();
        relay.begin_recovery();
        assert!(relay.admission() == EncodeAdmission::Wait);
        relay.ack(1);
        assert!(relay.admission() == EncodeAdmission::Keyframe);
        relay.defer_until_ready();
        assert!(relay.admission() == EncodeAdmission::Wait);
        relay.activate(0).unwrap();
        assert!(relay.admission() == EncodeAdmission::Keyframe);
        assert!(relay.push(frame(true, 1000), Instant::now()));
        rx.try_recv().unwrap();
        relay.ack(2);
        assert!(relay.admission() == EncodeAdmission::Frame);
    }

    #[test]
    fn a_large_recovery_picture_can_progress_but_cannot_grow_the_queue() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let relay = RelayVideo::new(tx, "lease".into());
        let now = Instant::now();
        assert!(relay.push(frame(true, 300 * 1024), now));
        assert!(!relay.push(frame(false, 1000), now));
        assert!(!relay.push(frame(true, 300 * 1024), now));
        assert_eq!(rx.try_recv().unwrap().1.len(), 300 * 1024);
        relay.ack(1);
        assert!(!relay.push(frame(true, RelayVideo::MAX_KEYFRAME_BYTES + 1), now));
        assert!(relay.push(frame(true, 300 * 1024), now));
        assert_eq!(relay.stats()["sentSeq"], 2);
    }

    #[test]
    fn switching_screens_retires_old_acks_and_requires_a_new_keyframe() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let relay = RelayVideo::new(tx, "lease".into());
        let now = Instant::now();
        assert!(relay.push(frame(true, 1000), now));
        assert_eq!(rx.try_recv().unwrap().0["generation"], 0);
        relay.reset();
        assert_eq!(relay.stats()["inflight"], 0);
        assert!(!relay.push(frame(false, 1000), now));
        assert!(relay.push(frame(true, 1000), now));
        let header = rx.try_recv().unwrap().0;
        assert_eq!(header["seq"], 2);
        assert_eq!(header["generation"], 1);
        relay.ack(1);
        assert_eq!(relay.stats()["inflight"], 1);
        relay.ack(2);
        assert_eq!(relay.stats()["inflight"], 0);
    }

    #[test]
    fn encoder_follows_the_estimate_under_the_size_cap() {
        let pixels = 1920 * 1240;
        // Healthy link: capped by what the picture can use.
        assert_eq!(rate_policy(12_000_000, pixels), (7_142_400, 30));
        // Mid link: 85 % of the estimate.
        assert_eq!(rate_policy(2_000_000, pixels), (1_700_000, 30));
        assert_eq!(rate_policy(1_000_000, pixels), (850_000, 20));
        // Thin link: fewer frames.
        assert_eq!(rate_policy(400_000, pixels), (340_000, 12));
        // Small pictures still get a usable floor.
        assert_eq!(rate_policy(12_000_000, 320 * 200).0, 1_000_000);
    }
    #[derive(Default)]
    struct EpochOutput {
        generation: std::sync::atomic::AtomicU64,
        frames: std::sync::Mutex<Vec<(u64, u8)>>,
    }
    impl VideoOut for EpochOutput {
        fn push(&self, f: EncodedFrame, _: Instant) -> bool {
            self.frames.lock().unwrap().push((
                self.generation.load(std::sync::atomic::Ordering::Acquire),
                f.data[0],
            ));
            true
        }
        fn reset(&self) {
            self.generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        fn target_bitrate(&self) -> u64 {
            800_000
        }
        fn take_keyframe_request(&self) -> bool {
            false
        }
        fn closed(&self) -> bool {
            false
        }
    }
    #[test]
    fn only_an_accepted_current_frame_can_change_pointer_mode() {
        use super::{Arc, AtomicBool, Ordering, Publication};
        struct Target(AtomicBool);
        impl VideoOut for Target {
            fn push(&self, _: EncodedFrame, _: Instant) -> bool {
                self.0.load(Ordering::Acquire)
            }
            fn reset(&self) {}
            fn target_bitrate(&self) -> u64 {
                800_000
            }
            fn take_keyframe_request(&self) -> bool {
                false
            }
            fn closed(&self) -> bool {
                false
            }
        }
        let target = Target(AtomicBool::new(true));
        let publication = Publication::new(Arc::new(AtomicBool::new(false)));
        let sample = |cursor_embedded| EncodedFrame {
            cursor_embedded,
            ..frame(true, 1)
        };
        assert!(publication.push(&target, sample(Some(false)), Instant::now()));
        assert_eq!(publication.cursor_embedded(), Some(false));
        target.0.store(false, Ordering::Release);
        assert!(!publication.push(&target, sample(Some(true)), Instant::now()));
        assert_eq!(publication.cursor_embedded(), Some(false));
        target.0.store(true, Ordering::Release);
        let epoch = publication.advance();
        assert!(!publication.push_epoch(0, &target, sample(Some(true)), Instant::now()));
        assert_eq!(publication.cursor_embedded(), Some(false));
        assert!(publication.push_epoch(epoch, &target, sample(Some(true)), Instant::now()));
        assert_eq!(publication.cursor_embedded(), Some(true));
        assert!(publication.push_epoch(epoch, &target, sample(None), Instant::now()));
        assert_eq!(publication.cursor_embedded(), None);
        publication.close();
        assert!(!publication.push_epoch(epoch, &target, sample(Some(false)), Instant::now()));
        assert_eq!(publication.cursor_embedded(), None);
    }
    #[test]
    fn a_late_encoder_callback_cannot_publish_under_the_replacement_monitor_epoch() {
        use super::{Arc, AtomicBool, Publication};
        let output = EpochOutput::default();
        let old = Publication::new(Arc::new(AtomicBool::new(false)));
        assert!(old.push(&output, frame(true, 1), Instant::now()));
        old.close();
        output.reset();
        // This may run long after stop, when an OS capture or codec finishes.
        assert!(!old.push(&output, frame(true, 1), Instant::now()));
        let new = Publication::new(Arc::new(AtomicBool::new(false)));
        assert!(new.push(&output, frame(true, 1), Instant::now()));
        assert_eq!(*output.frames.lock().unwrap(), [(0, 0), (1, 0)]);
    }
    #[test]
    fn resizing_retires_old_callbacks_before_the_replacement_publishes() {
        use super::{Arc, AtomicBool, Publication};
        let sender = EpochOutput::default();
        let publication = Publication::new(Arc::new(AtomicBool::new(false)));
        assert!(publication.push_epoch(0, &sender, frame(true, 1), Instant::now()));
        let next = publication.advance();
        assert!(!publication.push_epoch(0, &sender, frame(true, 1), Instant::now()));
        assert!(publication.push_epoch(next, &sender, frame(true, 1), Instant::now()));
        publication.close();
        assert!(!publication.push_epoch(next, &sender, frame(true, 1), Instant::now()));
    }

    #[test]
    fn ending_a_session_does_not_join_a_blocked_native_worker() {
        use super::{Arc, AtomicBool, Output, Publication, Session, Stats};
        let (release, wait) = std::sync::mpsc::channel();
        let (ended, done) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _ = wait.recv_timeout(Duration::from_secs(2));
            let _ = ended.send(());
        });
        let publication = Arc::new(Publication::new(Arc::new(AtomicBool::new(false))));
        let session = Session {
            sender: Arc::new(Output::new(Arc::new(EpochOutput::default()))),
            resize: Arc::new(super::Resize::new(1280)),
            publication: publication.clone(),
            stats: Arc::new(Stats::default()),
            thread: Some(thread),
            started: Instant::now(),
        };
        let now = Instant::now();
        drop(session);
        let elapsed = now.elapsed();
        let _ = release.send(());
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(elapsed < Duration::from_millis(100));
        assert!(!publication.push(&EpochOutput::default(), frame(true, 1), Instant::now()));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod codec_switch_tests {
    use super::*;
    use dogcodes_remote_desktop::video::{ScreenSource, VideoProfile};
    use openh264::formats::YUVSource;
    struct Pixels(Arc<image::RgbaImage>);
    impl ScreenSource for Pixels {
        fn check_active(&self) -> Result<(), String> {
            Ok(())
        }
        fn frame(&self) -> Result<Arc<image::RgbaImage>, String> {
            Ok(self.0.clone())
        }
        fn size(&self) -> Result<(u32, u32), String> {
            Ok(self.0.dimensions())
        }
        fn cursor_embedded(&self) -> bool {
            false
        }
    }
    struct Encoded {
        profile: VideoProfile,
        frames: Mutex<Vec<EncodedFrame>>,
    }
    impl VideoOut for Encoded {
        fn profile(&self) -> VideoProfile {
            self.profile
        }
        fn push(&self, frame: EncodedFrame, _: Instant) -> bool {
            let mut frames = self.frames.lock().unwrap();
            if frames.len() < 12 {
                frames.push(frame);
            }
            true
        }
        fn target_bitrate(&self) -> u64 {
            1_500_000
        }
        fn take_keyframe_request(&self) -> bool {
            false
        }
        fn closed(&self) -> bool {
            false
        }
    }
    fn output(profile: VideoProfile) -> Arc<Encoded> {
        Arc::new(Encoded {
            profile,
            frames: Mutex::default(),
        })
    }
    fn wait(note: &str, mut predicate: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(8);
        while !predicate() {
            assert!(Instant::now() < until, "video worker deadline: {note}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn owned_session() {
        assert_eq!(
            std::env::var("DOGCODES_OWNED_SESSION_STATE").as_deref(),
            Ok("login1-fixture-v1")
        );
        let system = std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap();
        assert!(!system.is_empty());
        assert_eq!(system, std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap());
        let marker = std::env::var("DOGCODES_SESSION_STATE_MARKER").unwrap();
        assert!(!marker.is_empty());
        let root = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join(".fixture-owned")).unwrap(),
            marker
        );
    }
    #[test]
    #[ignore = "requires the owned login1/session-state launcher; uses synthetic pixels only"]
    fn same_synthetic_source_rebuilds_codec_in_both_directions_and_gates_real_h264() {
        owned_session();
        let pixels = Arc::new(Pixels(Arc::new(image::RgbaImage::from_fn(
            1600,
            1000,
            |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 0, 255]),
        ))));
        dogcodes_remote_desktop::require_unlocked().expect("isolated session is unlocked");
        let auto = output(VideoProfile::ConstrainedBaselineAuto);
        let video = Session::start(0, 1600, Some(pixels.clone()), auto.clone()).unwrap();
        wait("initial encoded output", || {
            assert!(video.stats()["error"].is_null(), "{}", video.stats());
            !auto.frames.lock().unwrap().is_empty()
        });
        let source = video.describe()["resize"]["instanceId"].clone();
        for profile in [
            VideoProfile::ConstrainedBaseline31,
            VideoProfile::ConstrainedBaselineAuto,
        ] {
            let target = output(profile);
            let generation = video.replace_output(target.clone()).unwrap();
            wait("codec rebuilt", || video.describe()["codecReady"] == true);
            assert_eq!(video.describe()["resize"]["instanceId"], source);
            assert!(target.frames.lock().unwrap().is_empty());
            video.activate_rtp(generation).unwrap();
            wait("target encoded output", || {
                !target.frames.lock().unwrap().is_empty()
            });
            let frames = target.frames.lock().unwrap();
            assert!(frames[0].keyframe);
            for frame in frames.iter().filter(|f| f.keyframe) {
                let mut decoder = openh264::decoder::Decoder::new().unwrap();
                let picture = decoder
                    .decode(&frame.data)
                    .unwrap()
                    .expect("an actual decodable keyframe");
                assert_eq!(
                    picture.dimensions(),
                    (
                        usize::try_from(video.describe()["width"].as_u64().unwrap()).unwrap(),
                        usize::try_from(video.describe()["height"].as_u64().unwrap()).unwrap()
                    )
                );
                let sps = frame
                    .data
                    .windows(8)
                    .find(|n| n[..4] == [0, 0, 0, 1] && n[4] & 31 == 7)
                    .expect("keyframe SPS");
                assert_eq!(sps[5], 66);
                assert_ne!(sps[6] & 0x40, 0);
                if profile == VideoProfile::ConstrainedBaseline31 {
                    assert!(sps[7] <= 31);
                }
            }
            assert!(video.stats()["error"].is_null());
        }
        drop(video);
        wait("source released", || Arc::strong_count(&pixels) == 1);
    }

    struct ChangingPixels {
        image: Mutex<Arc<image::RgbaImage>>,
        active: AtomicBool,
    }
    impl ScreenSource for ChangingPixels {
        fn check_active(&self) -> Result<(), String> {
            if self.active.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err("owned source revoked".into())
            }
        }
        fn size(&self) -> Result<(u32, u32), String> {
            Ok(self.frame()?.dimensions())
        }
        fn cursor_embedded(&self) -> bool {
            false
        }
        fn frame(&self) -> Result<Arc<image::RgbaImage>, String> {
            self.check_active()?;
            Ok(self.image.lock().unwrap().clone())
        }
    }

    #[test]
    #[ignore = "requires the owned login1/session-state launcher; uses synthetic pixels only"]
    fn source_geometry_rebuilds_without_old_acks_and_gates_current_decodable_idr() {
        use openh264::formats::YUVSource;
        owned_session();
        let picture = |w, h, gray| {
            Arc::new(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([gray, gray, gray, 255]),
            ))
        };
        let source = Arc::new(ChangingPixels {
            image: Mutex::new(picture(320, 240, 30)),
            active: AtomicBool::new(true),
        });
        let (wire, mut receiver) = tokio::sync::mpsc::channel(64);
        let relay = Arc::new(RelayVideo::new(wire, "geometry-fixture".into()));
        relay.defer_until_ready();
        let video = Session::start(0, 640, Some(source.clone()), relay.clone()).unwrap();
        let initial = video.describe();
        relay.activate(0).unwrap();
        wait("initial geometry output", || {
            video.stats()["frames"].as_u64().unwrap() > 0
        });
        // Deliberately withhold every ACK: geometry must supersede a stalled
        // old decoder, whereas ordinary user width requests wait for it.
        assert!(!relay.can_resize());
        for (generation, w, h, gray) in [(1, 240, 320, 235), (2, 320, 240, 30)] {
            *source.image.lock().unwrap() = picture(w, h, gray);
            wait("automatic geometry configuration", || {
                assert!(video.stats()["error"].is_null(), "{}", video.stats());
                let d = video.describe();
                d["generation"] == generation && d["resize"]["state"] == "ready"
            });
            let d = video.describe();
            assert_eq!(d["width"], w);
            assert_eq!(d["height"], h);
            assert_eq!(d["resize"]["applied"], initial["resize"]["applied"]);
            assert_eq!(d["resize"]["instanceId"], initial["resize"]["instanceId"]);
            assert_eq!(d["ready"], false);
            let frames = video.stats()["frames"].clone();
            while receiver.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(
                video.stats()["frames"],
                frames,
                "new codec published before ready"
            );
            assert!(relay.activate(generation - 1).is_err());
            relay.activate(generation).unwrap();
            video.activate_rtp(generation).unwrap();
            let mut first = None;
            wait("current generation IDR", || {
                first = receiver.try_recv().ok();
                first.is_some()
            });
            let (header, data) = first.unwrap();
            assert_eq!(header["generation"], generation);
            assert_eq!(header["key"], true);
            let mut decoder = openh264::decoder::Decoder::new().unwrap();
            let reconstructed = decoder
                .decode(&data)
                .unwrap()
                .expect("standalone current-size IDR");
            assert_eq!(reconstructed.dimensions(), (w as usize, h as usize));
            let mean = reconstructed.y().iter().map(|p| u64::from(*p)).sum::<u64>()
                / reconstructed.y().len() as u64;
            assert!(
                if gray > 128 { mean > 190 } else { mean < 80 },
                "stale picture: {mean}"
            );
        }
        source.active.store(false, Ordering::Release);
        wait("original source revocation", || {
            !video.stats()["error"].is_null()
        });
        let published = video.stats()["frames"].clone();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(video.stats()["frames"], published);
        drop(video);
        wait("same source released", || Arc::strong_count(&source) == 1);
    }

    struct MovingPixels {
        moving: AtomicBool,
        counter: AtomicUsize,
        value: AtomicU32,
        observed: AtomicU32,
    }
    impl ScreenSource for MovingPixels {
        fn check_active(&self) -> Result<(), String> {
            Ok(())
        }
        fn size(&self) -> Result<(u32, u32), String> {
            Ok((480, 320))
        }
        fn cursor_embedded(&self) -> bool {
            false
        }
        fn frame(&self) -> Result<Arc<image::RgbaImage>, String> {
            let value = if self.moving.load(Ordering::Acquire) {
                80 + u32::try_from(self.counter.fetch_add(1, Ordering::AcqRel) % 100).unwrap()
            } else {
                self.value.load(Ordering::Acquire)
            };
            self.observed.store(value, Ordering::Release);
            let gray = u8::try_from(value).unwrap();
            Ok(Arc::new(image::RgbaImage::from_pixel(
                480,
                320,
                image::Rgba([gray, gray, gray, 255]),
            )))
        }
    }

    #[test]
    #[ignore = "requires the owned login1/session-state launcher; uses synthetic pixels only"]
    fn actual_relay_backpressure_resumes_from_the_latest_picture_without_idr_churn() {
        owned_session();
        let source = Arc::new(MovingPixels {
            moving: AtomicBool::new(true),
            counter: AtomicUsize::new(0),
            value: AtomicU32::new(80),
            observed: AtomicU32::new(0),
        });
        let (wire, mut receiver) = tokio::sync::mpsc::channel(64);
        let relay = Arc::new(RelayVideo::new(wire, "owned-fixture".into()));
        relay.defer_until_ready();
        let video = Session::start(0, 480, Some(source.clone()), relay.clone()).unwrap();
        assert_eq!(relay.stats()["sentSeq"], 0);
        relay.activate(0).unwrap();
        wait("full actual ACK window", || {
            assert!(video.stats()["error"].is_null(), "{}", video.stats());
            relay.stats()["inflight"] == RelayVideo::WINDOW
        });
        video.request_keyframe();
        wait("recovery belongs to current output", || {
            relay.state().resync
        });
        source.value.store(235, Ordering::Release);
        source.moving.store(false, Ordering::Release);
        wait("source updated while paused", || {
            source.observed.load(Ordering::Acquire) == 235
        });
        std::thread::sleep(Duration::from_millis(150));
        let dropped = video.stats()["dropped"].clone();
        let sent = relay.stats()["sentSeq"].clone();
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(
            video.stats()["dropped"],
            dropped,
            "full-window IDRs kept entering the encoder"
        );
        assert_eq!(relay.stats()["sentSeq"], sent);
        relay.ack(4);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            relay.stats()["sentSeq"],
            sent,
            "partial ACK consumed recovery early"
        );
        assert_eq!(video.stats()["dropped"], dropped);
        for _ in 0..RelayVideo::WINDOW {
            receiver.try_recv().unwrap();
        }
        let before_keys = video.stats()["keyframes"].as_u64().unwrap();
        relay.ack(8);
        let mut resumed = None;
        wait("fresh actual IDR after ACK", || {
            if let Ok(frame) = receiver.try_recv() {
                resumed = Some(frame);
            }
            resumed.is_some()
        });
        let (header, bytes) = resumed.unwrap();
        assert_eq!(header["seq"], 9);
        assert_eq!(header["key"], true);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let image = decoder.decode(&bytes).unwrap().unwrap();
        assert_eq!(image.dimensions(), (480, 320));
        assert!(
            image.y()[100 * image.strides().0 + 100] > 200,
            "backpressure resumed from stale source pixels"
        );
        relay.ack(9);
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(
            video.stats()["keyframes"].as_u64().unwrap(),
            before_keys + 1,
            "a cleared lost flag rearmed the same recovery"
        );
        assert!(video.stats()["error"].is_null());
        drop(video);
        wait("flow-controlled source released", || {
            Arc::strong_count(&source) == 1
        });
    }
}
