//! Screen video: ScreenCaptureKit → VideoToolbox on macOS; desktop duplication
//! or bounded GDI → hardware Media Foundation/OpenH264 on Windows. Linux
//! uses bounded GStreamer hardware/OpenH264 with screenshots or an authorized
//! continuous Wayland source.
//!
//! Encoders emit low-latency Annex-B access units with on-demand keyframes,
//! shared by the WebRTC and authenticated relay paths.
use crate::Result;
#[cfg(target_os = "macos")]
pub(crate) use platform::Picture;
#[cfg(target_os = "windows")]
pub(crate) use pull_picture::Picture;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod capture_source;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod capture_worker;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod display_changes;
#[cfg(target_os = "windows")]
mod dxgi;
mod flow;
#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
mod h264;
#[cfg(any(target_os = "windows", test))]
mod nv12;
mod publication;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod pull_picture;
mod refresh;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod scrgb;
mod worker;

/// One encoded access unit.
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    /// Whether the receiver should suppress a separate pointer overlay.
    /// None means the original capture has not supplied a valid determination.
    pub cursor_embedded: Option<bool>,
    /// Annex-B NAL units; a keyframe starts with SPS and PPS.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Original codec input time, retained through asynchronous callbacks.
    pub captured: Instant,
    /// Time between capture and encoder output.
    pub latency: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoProfile {
    ConstrainedBaselineAuto,
    ConstrainedBaseline31,
}

impl VideoProfile {
    fn rate(self, bitrate: u32, fps: u32) -> (u32, u32) {
        let (max_bitrate, max_fps) = match self {
            Self::ConstrainedBaselineAuto => (50_000_000, 60),
            // DataRateLimits permits 1.5 times the average, still below 14 Mb/s.
            Self::ConstrainedBaseline31 => (9_000_000, 30),
        };
        (bitrate.clamp(100_000, max_bitrate), fps.clamp(1, max_fps))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoConfig {
    pub display: u32,
    /// Encoded width in pixels; the height follows the display aspect.
    pub max_width: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub profile: VideoProfile,
}

pub type Sink = Arc<dyn Fn(EncodedFrame) + Send + Sync>;
/// Connection-owned continuous source (Wayland portal). The latest frame is
/// shared; encoders cannot create an unbounded screenshot/portal request queue.
pub trait ScreenSource: Send + Sync {
    fn frame(&self) -> Result<Arc<image::RgbaImage>>;
    fn size(&self) -> Result<(u32, u32)>;
    fn cursor_embedded(&self) -> bool;
    /// Revalidate this original source without fetching/advancing pixels.
    /// Each provider must explicitly describe its authorization lifetime.
    fn check_active(&self) -> Result<()>;
}
pub type Source = Option<Arc<dyn ScreenSource>>;

/// Rebuilding must reuse the original authorization and never trigger a new
/// system permission prompt. Initial connection preparation stays separate.
pub fn check_recovery_source(source: &Source) -> Result<()> {
    crate::require_unlocked()?;
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        // SAFETY: read-only, parameterless system permission check; no prompt.
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            return Err("屏幕录制权限已失效，请在电脑上重新授权".into());
        }
    }
    if let Some(source) = source {
        source.check_active()?;
    }
    Ok(())
}

/// Downstream credit checked before conversion/encoding. A denied input is
/// retained as the single latest picture, never queued as another codec job.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EncodeAdmission {
    Wait,
    Frame,
    Keyframe,
}
pub type EncodeGate = Arc<dyn Fn() -> EncodeAdmission + Send + Sync>;

/// Length-prefixed NAL units (AVCC, as VideoToolbox emits them) → Annex-B.
pub fn avcc_to_annexb(avcc: &[u8], length_size: usize, out: &mut Vec<u8>) -> Result<()> {
    if !(1..=4).contains(&length_size) {
        return Err("H.264 长度字段无效".into());
    }
    let mut rest = avcc;
    while !rest.is_empty() {
        if rest.len() < length_size {
            return Err("H.264 数据截断".into());
        }
        let (head, tail) = rest.split_at(length_size);
        let length = head
            .iter()
            .fold(0usize, |acc, byte| (acc << 8) | usize::from(*byte));
        if length == 0 || tail.len() < length {
            return Err("H.264 数据截断".into());
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&tail[..length]);
        rest = &tail[length..];
    }
    Ok(())
}

/// Encoded size for a display: at most `max_width`, never upscaled, even
/// dimensions (4:2:0), same aspect as the display.
pub fn stream_size(native_width: u32, native_height: u32, max_width: u32) -> (u32, u32) {
    let width = native_width.min(max_width).max(2);
    let height = (u64::from(native_height) * u64::from(width))
        .checked_div(u64::from(native_width.max(1)))
        .unwrap_or(2);
    let even = |v: u64| u32::try_from(v.max(2) & !1).unwrap_or(u32::MAX - 1);
    (even(u64::from(width)), even(height))
}

/// Level 3.1 at 30 fps allows 3,600 macroblocks per frame. Keep dimensions
/// below 2,560 as well, within its per-dimension bound. Search in even pixels
/// so extreme display geometry cannot create a long linear scan.
pub fn profile_size(
    width: u32,
    height: u32,
    max_width: u32,
    profile: VideoProfile,
) -> Result<(u32, u32)> {
    let fits = |size: (u32, u32)| match profile {
        VideoProfile::ConstrainedBaseline31 => {
            size.0 <= 2560
                && size.1 <= 2560
                && u64::from(size.0.div_ceil(16)) * u64::from(size.1.div_ceil(16)) <= 3600
        }
        // Bound raw-relay auto-level capture as well, especially on portrait
        // and extreme-aspect displays. This is an allocation bound, not a
        // declaration that an RTP receiver negotiated a higher H.264 level.
        VideoProfile::ConstrainedBaselineAuto => {
            size.0 <= 3840 && size.1 <= 3840 && u64::from(size.0) * u64::from(size.1) <= 3840 * 2160
        }
    };
    let size = stream_size(width, height, max_width);
    if fits(size) {
        return Ok(size);
    }
    if !fits(stream_size(width, height, 2)) {
        return Err("显示器比例超过视频接收能力".into());
    }
    let (mut low, mut high) = (1, size.0 / 2);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(stream_size(width, height, mid * 2)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok(stream_size(width, height, low * 2))
}

/// Shared between the capture queue, the encoder callback and the owner.
/// VideoToolbox exposes Constrained Baseline with automatic level only. Check
/// the actual SPS before publishing: setting a Baseline level alone does not
/// guarantee the Constrained Baseline subset. Out-of-budget encoders fall back.
fn validate_profile(frame: &EncodedFrame, profile: VideoProfile) -> Result<()> {
    if profile != VideoProfile::ConstrainedBaseline31 {
        return Ok(());
    }
    let mut found = false;
    for sps in frame
        .data
        .windows(8)
        .filter(|n| n[..4] == [0, 0, 0, 1] && n[4] & 31 == 7)
    {
        found = true;
        if sps[5] != 66
            || sps[6] & 0x40 == 0
            || !matches!(sps[7], 10 | 11 | 12 | 13 | 20 | 21 | 22 | 30 | 31)
        {
            return Err("编码器输出超过手机协商的视频能力，已使用图片模式".into());
        }
    }
    if frame.keyframe && !found {
        return Err("视频关键帧缺少编码参数".into());
    }
    Ok(())
}

struct Shared {
    profile: VideoProfile,
    sink: Sink,
    encoder: Mutex<Option<platform::Encoder>>,
    keyframe: AtomicBool,
    bitrate: AtomicU32,
    fps: AtomicU32,
    /// The last captured frame, kept so a keyframe can be produced while the
    /// screen is still (ScreenCaptureKit sends nothing then).
    last: Mutex<Option<platform::Frame>>,
    refresh: Mutex<refresh::Refresh>,
    output: Arc<Mutex<refresh::Output>>,
    flow: flow::Flow,
    latest_pending: AtomicBool,
    stopped: Arc<AtomicBool>,
    /// Last lock-state check: (when, whether capture may be published).
    unlocked: Mutex<(Instant, bool)>,
    error: Arc<Mutex<Option<String>>>,
}

impl Shared {
    /// Capture may deliver (and fail) its first input before startup reaches
    /// readiness. Under the same lock, never resurrect that retired stream.
    fn initialize(&self, size: (u32, u32)) -> Result<()> {
        let mut encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(Ordering::Acquire) {
            return Err(self
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| "视频连接已结束".into()));
        }
        if encoder.is_none() {
            *encoder = Some(platform::Encoder::with_profile(
                size.0,
                size.1,
                self.fps.load(Ordering::Acquire),
                self.bitrate.load(Ordering::Acquire),
                self.sink.clone(),
                self.profile,
            )?);
        }
        Ok(())
    }

    fn encode_locked(
        &self,
        encoder: &mut Option<platform::Encoder>,
        frame: &platform::Frame,
        captured: Instant,
    ) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        match self.flow.admission(captured) {
            EncodeAdmission::Wait => {
                self.keyframe.store(true, Ordering::Release);
                self.refresh
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .backpressured();
                return false;
            }
            EncodeAdmission::Keyframe => self.keyframe.store(true, Ordering::Release),
            EncodeAdmission::Frame => (),
        }
        if encoder
            .as_ref()
            .is_some_and(|encoder| !encoder.can_encode())
        {
            return false;
        }
        let force = self.keyframe.swap(false, Ordering::AcqRel);
        let (width, height) = frame.size();
        if encoder.as_ref().is_none_or(|e| e.size() != (width, height)) {
            match platform::Encoder::with_profile(
                width,
                height,
                self.fps.load(Ordering::Acquire),
                self.bitrate.load(Ordering::Acquire),
                self.sink.clone(),
                self.profile,
            ) {
                Ok(created) => *encoder = Some(created),
                Err(error) => {
                    self.fail(error);
                    return false;
                }
            }
        }
        let Some(encoder) = encoder.as_mut() else {
            return false;
        };
        encoder.set_rate(
            self.bitrate.load(Ordering::Acquire),
            self.fps.load(Ordering::Acquire),
        );
        let output = *self.output.lock().unwrap_or_else(|e| e.into_inner());
        self.refresh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .submitting(Instant::now(), output, force);
        let result = encoder.encode(frame, captured, force);
        let accepted = matches!(&result, Ok(true));
        self.encoded(result, force);
        accepted
    }

    fn fail(&self, error: String) {
        self.stopped.store(true, Ordering::Release);
        self.error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(error);
    }

    /// Submission acceptance does not prove asynchronous output. The watchdog
    /// observes the real sink, while a skipped IDR retains its request.
    fn encoded(&self, result: Result<bool>, force: bool) {
        match result {
            Err(error) => self.fail(error),
            Ok(false) if force => self.keyframe.store(true, Ordering::Release),
            _ => (),
        }
    }

    /// Called on the capture queue for every changed frame.
    /// Like the JPEG path's per-frame check, but throttled for 30 fps: no
    /// frame is encoded while the console is locked or its state unknown.
    fn unlocked(&self) -> bool {
        let mut state = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        if state.0.elapsed() > Duration::from_millis(200) {
            let result = crate::require_unlocked();
            if let Err(reason) = &result {
                self.stopped.store(true, Ordering::Release);
                self.error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(|| reason.clone());
            }
            *state = (Instant::now(), result.is_ok());
        }
        state.1
    }

    fn captured(&self, frame: platform::Frame) {
        if self.stopped.load(Ordering::Acquire) || !self.unlocked() {
            return;
        }
        let mut encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if self
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|last| last.cursor_embedded != frame.cursor_embedded)
        {
            // A metadata-only change still needs an admitted encoded frame;
            // codecs may otherwise skip identical pixels indefinitely.
            self.keyframe.store(true, Ordering::Release);
        }
        let now = Instant::now();
        self.refresh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .changed(now);
        let accepted = self.encode_locked(&mut encoder, &frame, now);
        if !self.stopped.load(Ordering::Acquire) {
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
            self.latest_pending.store(!accepted, Ordering::Release);
        }
    }

    /// Static refinement is paced and bounded separately from loss recovery.
    /// Serializing selection with capture prevents an old cached frame from
    /// being encoded after a newer changed frame.
    fn tick(&self) {
        if self.stopped.load(Ordering::Acquire) || !self.unlocked() {
            return;
        }
        let mut encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let output = *self.output.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let admission = self.flow.admission(now);
        if admission == EncodeAdmission::Keyframe {
            self.keyframe.store(true, Ordering::Release);
        }
        let due = {
            let mut refresh = self.refresh.lock().unwrap_or_else(|e| e.into_inner());
            if admission == EncodeAdmission::Wait {
                self.keyframe.store(true, Ordering::Release);
                refresh.backpressured();
            }
            refresh.tick_admitted(
                Instant::now(),
                output,
                self.keyframe.load(Ordering::Acquire),
                self.fps.load(Ordering::Acquire),
                admission != EncodeAdmission::Wait
                    && encoder.as_ref().is_none_or(platform::Encoder::can_encode),
                self.latest_pending.load(Ordering::Acquire),
            )
        };
        match due {
            Err(error) => self.fail(error),
            Ok(Some(force)) => {
                if force {
                    self.keyframe.store(true, Ordering::Release);
                }
                let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(frame) = last
                    && self.encode_locked(&mut encoder, &frame, Instant::now())
                {
                    self.latest_pending.store(false, Ordering::Release);
                }
            }
            Ok(None) => (),
        }
    }
}

/// The platform encoder fed a moving test pattern, for tests and
/// benchmarks on machines without screen-recording permission.
pub struct SyntheticEncoder {
    encoder: platform::Encoder,
    size: (u32, u32),
    next: u32,
}

impl SyntheticEncoder {
    pub fn new(size: (u32, u32), fps: u32, bitrate: u32, sink: Sink) -> Result<Self> {
        Ok(Self {
            encoder: platform::Encoder::new(size.0, size.1, fps, bitrate, sink)?,
            size,
            next: 0,
        })
    }

    pub fn with_profile(
        size: (u32, u32),
        fps: u32,
        bitrate: u32,
        sink: Sink,
        profile: VideoProfile,
    ) -> Result<Self> {
        Ok(Self {
            encoder: platform::Encoder::with_profile(size.0, size.1, fps, bitrate, sink, profile)?,
            size,
            next: 0,
        })
    }

    /// Encodes the next pattern frame, captured now; the first is an IDR.
    pub fn encode_next(&mut self, keyframe: bool) -> Result<()> {
        let started = Instant::now();
        let frame = platform::Frame::synthetic(self.size.0, self.size.1, self.next)?;
        self.encoder
            .encode(&frame, started, keyframe || self.next == 0)?;
        self.next += 1;
        Ok(())
    }

    pub fn set_rate(&mut self, bitrate: u32, fps: u32) {
        self.encoder.set_rate(bitrate, fps);
    }
}

/// Encodes `frames` synthetic frames at `fps` (real time) and returns the
/// output; the frames listed in `keyframes` are forced IDRs.
pub fn encode_synthetic(
    size: (u32, u32),
    frames: u32,
    fps: u32,
    bitrate: u32,
    keyframes: &[u32],
) -> Result<Vec<EncodedFrame>> {
    let out: Arc<Mutex<Vec<EncodedFrame>>> = Arc::default();
    let sink = out.clone();
    let mut encoder = SyntheticEncoder::new(
        size,
        fps,
        bitrate,
        Arc::new(move |frame| sink.lock().unwrap_or_else(|e| e.into_inner()).push(frame)),
    )?;
    let interval = Duration::from_secs(1) / fps.max(1);
    for t in 0..frames {
        let started = Instant::now();
        encoder.encode_next(keyframes.contains(&t))?;
        std::thread::sleep(interval.saturating_sub(started.elapsed()));
    }
    drop(encoder);
    Ok(std::mem::take(
        &mut *out.lock().unwrap_or_else(|e| e.into_inner()),
    ))
}

/// A running capture + encode pipeline for one display. Dropping it stops
/// capture; frames already handed to the sink stay valid.
pub struct VideoStream {
    shared: Arc<Shared>,
    capture: platform::Capture,
    size: (u32, u32),
    geometry_changed: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    geometry_pending: Arc<AtomicBool>,
    ticker: Option<std::thread::JoinHandle<()>>,
}

impl VideoStream {
    pub fn start(config: VideoConfig, sink: Sink) -> Result<Self> {
        Self::start_from(config, sink, None)
    }

    pub fn start_from(config: VideoConfig, sink: Sink, source: Source) -> Result<Self> {
        Self::start_from_with_admission(config, sink, source, Arc::new(|| EncodeAdmission::Frame))
    }

    /// Flow-controlled capture retains the newest source picture while the
    /// transport/decoder has no credit. No network wait holds the encoder.
    pub fn start_from_with_admission(
        config: VideoConfig,
        sink: Sink,
        source: Source,
        admission: EncodeGate,
    ) -> Result<Self> {
        Self::start_controlled(
            config,
            sink,
            source,
            admission,
            Arc::new(AtomicBool::new(false)),
        )
    }

    /// A lease-owned cancellation flag survives profile/size rebuilds. Dropping
    /// one stream never sets this parent flag; its local stop remains separate.
    pub fn start_controlled(
        mut config: VideoConfig,
        sink: Sink,
        source: Source,
        admission: EncodeGate,
        parent_stop: Arc<AtomicBool>,
    ) -> Result<Self> {
        if parent_stop.load(Ordering::Acquire) {
            return Err("视频请求已结束".into());
        }
        crate::require_unlocked()?;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            config = software::normalize(config);
        }
        (config.bitrate, config.fps) = config.profile.rate(config.bitrate, config.fps);
        let stopped = Arc::new(AtomicBool::new(false));
        let geometry_changed = Arc::new(AtomicBool::new(false));
        let geometry_pending = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let authorized_source = source.clone();
        let output = Arc::new(Mutex::new(refresh::Output::default()));
        let parent_gate = parent_stop.clone();
        let geometry_gate = geometry_changed.clone();
        let pending_gate = geometry_pending.clone();
        let flow = flow::Flow::new(Arc::new(move || {
            if parent_gate.load(Ordering::Acquire)
                || geometry_gate.load(Ordering::Acquire)
                || pending_gate.load(Ordering::Acquire)
            {
                EncodeAdmission::Wait
            } else {
                admission()
            }
        }));
        let observed = flow.track(sink, output.clone());
        let parent_check = parent_stop.clone();
        let geometry_gate = geometry_changed.clone();
        let pending_gate = geometry_pending.clone();
        let sink = publication::guard_when(
            observed,
            stopped.clone(),
            error.clone(),
            move || !geometry_gate.load(Ordering::Acquire) && !pending_gate.load(Ordering::Acquire),
            move || {
                if parent_check.load(Ordering::Acquire) {
                    return Err("视频请求已结束".into());
                }
                crate::require_unlocked()?;
                if let Some(source) = &authorized_source {
                    source.check_active()?;
                }
                Ok(())
            },
        );
        let shared = Arc::new(Shared {
            profile: config.profile,
            sink,
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(true),
            bitrate: AtomicU32::new(config.bitrate.clamp(100_000, 50_000_000)),
            fps: AtomicU32::new(config.fps.clamp(1, 60)),
            last: Mutex::new(None),
            refresh: Mutex::new(refresh::Refresh::new()),
            output,
            flow,
            latest_pending: AtomicBool::new(false),
            stopped,
            unlocked: Mutex::new((Instant::now(), crate::require_unlocked().is_ok())),
            error,
        });
        let sink = shared.clone();
        #[cfg(target_os = "windows")]
        let resuming = geometry_pending.clone();
        let callback = Box::new(move |frame| {
            sink.captured(frame);
            #[cfg(target_os = "windows")]
            resuming.store(false, Ordering::Release);
        });
        #[cfg(target_os = "linux")]
        let capture = match source {
            Some(source) => {
                software::Capture::from_source(config, source, callback, geometry_changed.clone())
            }
            None => {
                platform::Capture::start_with_geometry(config, callback, geometry_changed.clone())
            }
        };
        #[cfg(not(target_os = "linux"))]
        if source.is_some() {
            return Err("此平台不支持指定的屏幕来源".into());
        }
        #[cfg(target_os = "windows")]
        let capture = platform::Capture::start_windows(
            config,
            callback,
            geometry_changed.clone(),
            geometry_pending.clone(),
            parent_stop.clone(),
            shared.stopped.clone(),
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let capture = platform::Capture::start(config, callback);
        #[cfg(target_os = "macos")]
        let capture = {
            let stopped = shared.stopped.clone();
            let error = shared.error.clone();
            platform::Capture::start(
                config,
                callback,
                parent_stop.clone(),
                move |reason| {
                    stopped.store(true, Ordering::Release);
                    if let Some(reason) = reason {
                        error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_insert(reason);
                    }
                },
                geometry_pending.clone(),
                geometry_changed.clone(),
            )
        };
        let (capture, size) = match capture {
            Ok(capture) => capture,
            Err(error) => {
                shared.fail(error.clone());
                return Err(shared
                    .error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or(error));
            }
        };
        if parent_stop.load(Ordering::Acquire) {
            shared.fail("视频请求已结束".into());
            capture.stop();
            return Err("视频请求已结束".into());
        }
        // Capture readiness alone does not prove that an encoder is available.
        if let Err(error) = shared.initialize(size) {
            shared.stopped.store(true, Ordering::Release);
            capture.stop();
            return Err(error);
        }
        if parent_stop.load(Ordering::Acquire) {
            shared.fail("视频请求已结束".into());
            capture.stop();
            return Err("视频请求已结束".into());
        }
        let ticker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("remote-video-refresh".into())
                .spawn(move || {
                    while !shared.stopped.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(50));
                        shared.tick();
                    }
                })
        };
        let ticker = match ticker {
            Ok(ticker) => ticker,
            Err(_) => {
                shared.stopped.store(true, Ordering::Release);
                capture.stop();
                return Err("无法启动视频线程".into());
            }
        };
        Ok(Self {
            shared,
            capture,
            size,
            geometry_changed,
            #[cfg(target_os = "macos")]
            geometry_pending,
            ticker: Some(ticker),
        })
    }

    pub fn profile(&self) -> VideoProfile {
        self.shared.profile
    }

    /// Encoded frame size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// New source geometry is never encoded into this stream. The owner must
    /// fence the publication epoch and renegotiate before rebuilding it.
    pub fn needs_rebuild(&self) -> bool {
        self.geometry_changed.load(Ordering::Acquire)
    }
    /// Native configuration has begun but its authoritative commit has not
    /// arrived. Keep old output fenced; the owner waits with a bounded deadline.
    pub fn reconfiguring(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.geometry_pending.load(Ordering::Acquire) && !self.needs_rebuild()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn size_for_width(&self, width: u32) -> Result<(u32, u32)> {
        let native = self.capture.native_size();
        profile_size(native.0, native.1, width, self.shared.profile)
    }

    pub fn error(&self) -> Option<String> {
        let error = self
            .shared
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        error.or_else(|| self.capture.error()).or_else(|| {
            self.shared
                .encoder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(platform::Encoder::error)
        })
    }

    /// The next frame (or the last one, if the screen is still) is an IDR.
    pub fn request_keyframe(&self) {
        self.shared.keyframe.store(true, Ordering::Release);
    }

    /// Applied from the next encoded frame.
    pub fn set_bitrate(&self, bits_per_second: u32) {
        self.shared.bitrate.store(
            self.shared.profile.rate(bits_per_second, 1).0,
            Ordering::Release,
        );
    }

    pub fn set_fps(&self, fps: u32) {
        let fps = self.shared.profile.rate(100_000, fps).1;
        let changed = self.shared.fps.swap(fps, Ordering::AcqRel) != fps;
        #[cfg(target_os = "macos")]
        {
            let _ = changed;
            // The native worker throttles geometry probes and no-ops unchanged
            // FPS. This also works in a Core without a Cocoa event loop.
            self.capture.set_fps(fps);
        }
        #[cfg(not(target_os = "macos"))]
        if changed {
            self.capture.set_fps(fps);
        }
    }

    pub fn bitrate(&self) -> u32 {
        self.shared.bitrate.load(Ordering::Acquire)
    }

    pub fn fps(&self) -> u32 {
        self.shared.fps.load(Ordering::Acquire)
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.capture.stop();
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
        // Flush and invalidate the encoder before its callback context goes.
        self.shared
            .encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    mod picture;
    pub(crate) use picture::Picture;

    use super::{
        EncodedFrame, Result, Sink, VideoConfig, VideoProfile, avcc_to_annexb, profile_size,
    };
    use block2::RcBlock;
    use dispatch2::{DispatchQueue, DispatchQueueAttr};
    use objc2::{
        AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
    };
    use objc2_core_foundation::{
        CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_media::{
        CMFormatDescription, CMSampleBuffer, CMTime,
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex, kCMSampleAttachmentKey_NotSync,
        kCMVideoCodecType_H264,
    };
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    };
    use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
        SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
    };
    use objc2_video_toolbox::{
        VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
        kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
        kVTCompressionPropertyKey_DataRateLimits, kVTCompressionPropertyKey_ExpectedFrameRate,
        kVTCompressionPropertyKey_MaxAllowedFrameQP,
        kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
        kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
        kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
        kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{
        ffi::c_void,
        ptr::NonNull,
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn failure(what: &str, status: i32) -> String {
        format!("{what}（{status}）")
    }

    /// A captured, IOSurface-backed NV12 video or BGRA picture frame.
    #[derive(Clone)]
    pub struct Frame(CFRetained<CVPixelBuffer>);
    // SAFETY: CVPixelBuffer is a reference-counted CF object whose retain and
    // release are thread-safe; the pixels are read-only after capture delivery.
    unsafe impl Send for Frame {}
    impl Frame {
        pub fn size(&self) -> (u32, u32) {
            (
                u32::try_from(CVPixelBufferGetWidth(&self.0)).unwrap_or(0),
                u32::try_from(CVPixelBufferGetHeight(&self.0)).unwrap_or(0),
            )
        }

        /// A moving NV12 test pattern, for exercising the encoder without
        /// screen-recording permission.
        pub fn synthetic(width: u32, height: u32, t: u32) -> Result<Self> {
            let (w, h) = (width as usize, height as usize);
            let mut raw: *mut CVPixelBuffer = std::ptr::null_mut();
            // SAFETY: CVPixelBufferCreate returns a +1 buffer adopted by
            // CFRetained; plane pointers are written only inside the
            // lock/unlock pair and within bytes-per-row × plane height.
            unsafe {
                let status = CVPixelBufferCreate(
                    None,
                    w,
                    h,
                    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                    None,
                    NonNull::from(&mut raw),
                );
                if status != 0 {
                    return Err(failure("无法创建测试画面", status));
                }
                let buffer: CFRetained<CVPixelBuffer> =
                    CFRetained::from_raw(NonNull::new(raw).ok_or("无法创建测试画面")?);
                CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                let luma = CVPixelBufferGetBaseAddressOfPlane(&buffer, 0).cast::<u8>();
                let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 0);
                let t = t as usize;
                // Desktop-like: a static textured background, a window of
                // text scrolling 8 px per frame, and a pointer-sized block
                // crossing the screen.
                let (wx, wy) = (w / 8..w / 2, h / 6..h * 2 / 3);
                let pointer_y = h * 3 / 4..h * 3 / 4 + 24;
                let pointer_x = (t * 24) % w;
                for y in 0..h {
                    let row = std::slice::from_raw_parts_mut(luma.add(y * stride), w);
                    for (x, px) in row.iter_mut().enumerate() {
                        *px = 16 + ((x ^ y) & 0x3f) as u8;
                    }
                    if wy.contains(&y) {
                        let line = ((y + t * 8) / 12).is_multiple_of(2);
                        for x in wx.clone() {
                            let text = line && !(x / 5 + (y + t * 8) / 24).is_multiple_of(3);
                            row[x] = if text { 235 } else { 40 };
                        }
                    }
                    if pointer_y.contains(&y) {
                        row[pointer_x..(pointer_x + 24).min(w)].fill(235);
                    }
                }
                let chroma = CVPixelBufferGetBaseAddressOfPlane(&buffer, 1).cast::<u8>();
                let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, 1);
                for y in 0..h / 2 {
                    std::slice::from_raw_parts_mut(chroma.add(y * stride), w).fill(128);
                }
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                Ok(Self(buffer))
            }
        }
    }

    // ── Encoder ───────────────────────────────────────────────────────────

    struct Callback {
        profile: VideoProfile,
        error: Mutex<Option<String>>,
        completed: mpsc::SyncSender<Result<Option<EncodedFrame>>>,
        pending: std::sync::Arc<AtomicUsize>,
        #[cfg(test)]
        observing: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    }

    struct Pending<'a> {
        callback: &'a Callback,
        result: Option<Result<Option<EncodedFrame>>>,
    }
    impl Drop for Pending<'_> {
        fn drop(&mut self) {
            #[cfg(test)]
            if let Some(observing) = &self.callback.observing {
                observing();
            }
            // All callback processing precedes the result that releases the
            // worker's single input. No user sink runs on a VideoToolbox queue.
            self.callback.pending.fetch_sub(1, Ordering::AcqRel);
            if let Some(result) = self.result.take()
                && self.callback.completed.try_send(result).is_err()
            {
                self.callback
                    .error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(|| "视频输出未能交付".into());
            }
        }
    }

    struct Native {
        profile: VideoProfile,
        session: CFRetained<VTCompressionSession>,
        rate: (u32, u32),
        start: Instant,
        // Boxed so its address, the callback's refcon, outlives the session.
        _callback: Box<Callback>,
        completed: mpsc::Receiver<Result<Option<EncodedFrame>>>,
    }

    unsafe extern "C-unwind" fn output(
        refcon: *mut c_void,
        frame_refcon: *mut c_void,
        status: i32,
        flags: VTEncodeInfoFlags,
        sample: *mut CMSampleBuffer,
    ) {
        // SAFETY: refcon is the Box<Callback> owned by Native, which
        // invalidates the session (completing all callbacks) before dropping
        // it; frame_refcon is the Box<Instant> leaked in `encode`, reclaimed
        // here exactly once; sample is valid for the duration of the call.
        let (callback, captured, sample) = unsafe {
            (
                &*refcon.cast::<Callback>(),
                Box::from_raw(frame_refcon.cast::<Instant>()),
                sample.as_ref(),
            )
        };
        let mut pending = Pending {
            callback,
            result: None,
        };
        let result = if status == 0 && flags.contains(VTEncodeInfoFlags::FrameDropped) {
            Ok(None)
        } else if status != 0 {
            Err(failure("视频编码回调失败", status))
        } else {
            sample
                .ok_or_else(|| "视频编码未返回画面".to_owned())
                .and_then(annexb)
                .and_then(|frame| {
                    super::validate_profile(&frame, callback.profile)?;
                    Ok(Some(EncodedFrame {
                        captured: *captured,
                        latency: captured.elapsed(),
                        ..frame
                    }))
                })
        };
        if let Err(error) = &result {
            callback
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(|| error.clone());
        }
        pending.result = Some(result);
    }

    fn annexb(sample: &CMSampleBuffer) -> Result<EncodedFrame> {
        // SAFETY: plain CoreMedia accessors on a valid sample buffer; the
        // parameter-set pointers stay valid while `format` is retained, and
        // copy_data_bytes writes at most `length` bytes into `avcc`.
        unsafe {
            let keyframe = sample
                .sample_attachments_array(false)
                .and_then(|array| {
                    let array: &CFArray<CFDictionary<CFString, CFType>> = &*(&*array
                        as *const CFArray
                        as *const CFArray<CFDictionary<CFString, CFType>>);
                    array.get(0)
                })
                .and_then(|attachments| attachments.get(kCMSampleAttachmentKey_NotSync))
                .and_then(|value| value.downcast_ref::<CFBoolean>().map(CFBoolean::as_bool))
                .is_none_or(|not_sync| !not_sync);
            let block = sample.data_buffer().ok_or("H.264 输出为空")?;
            let length = block.data_length();
            let mut avcc = vec![0u8; length];
            let status =
                block.copy_data_bytes(0, length, NonNull::new_unchecked(avcc.as_mut_ptr().cast()));
            if status != 0 {
                return Err(failure("读取 H.264 输出失败", status));
            }
            let mut data = Vec::with_capacity(length + 64);
            let mut length_size = 4usize;
            if keyframe {
                let format: CFRetained<CMFormatDescription> =
                    sample.format_description().ok_or("缺少 H.264 参数集")?;
                let mut count = 0usize;
                let mut header: i32 = 4;
                for index in 0..2usize {
                    let (mut pointer, mut size) = (std::ptr::null(), 0usize);
                    let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        &format,
                        index,
                        &mut pointer,
                        &mut size,
                        &mut count,
                        &mut header,
                    );
                    if status != 0 || pointer.is_null() {
                        return Err(failure("读取 H.264 参数集失败", status));
                    }
                    data.extend_from_slice(&[0, 0, 0, 1]);
                    data.extend_from_slice(std::slice::from_raw_parts(pointer, size));
                    if index + 1 >= count {
                        break;
                    }
                }
                length_size = usize::try_from(header).unwrap_or(4);
            }
            avcc_to_annexb(&avcc, length_size, &mut data)?;
            Ok(EncodedFrame {
                cursor_embedded: Some(true),
                captured: std::time::Instant::now(),
                data,
                keyframe,
                latency: Duration::ZERO,
            })
        }
    }

    fn set(session: &VTCompressionSession, key: &CFString, value: &CFType) -> i32 {
        // SAFETY: valid session, key and value; VideoToolbox copies or
        // retains the value.
        unsafe { VTSessionSetProperty(session, key, Some(value)) }
    }

    impl Native {
        pub fn with_profile(
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            profile: VideoProfile,
        ) -> Result<Self> {
            if profile_size(width, height, width, profile)? != (width, height) {
                return Err("画面尺寸超过视频协商等级".into());
            }
            let (bitrate, fps) = profile.rate(bitrate, fps);
            let (sender, completed) = mpsc::sync_channel(1);
            let callback = Box::new(Callback {
                completed: sender,
                profile,
                error: Mutex::new(None),
                pending: std::sync::Arc::new(AtomicUsize::new(0)),
                #[cfg(test)]
                observing: None,
            });
            let mut raw: *mut VTCompressionSession = std::ptr::null_mut();
            // SAFETY: arguments are valid CF objects or null; `raw` receives
            // a +1 session adopted by CFRetained::from_raw below; the refcon
            // points at the boxed callback this thread-owned Native keeps alive.
            let session = unsafe {
                let low_latency = CFDictionary::<CFString, CFType>::from_slices(
                    &[kVTVideoEncoderSpecification_EnableLowLatencyRateControl],
                    &[CFBoolean::new(true).as_ref()],
                );
                let status = VTCompressionSession::create(
                    None,
                    i32::try_from(width).map_err(|_| "画面尺寸无效")?,
                    i32::try_from(height).map_err(|_| "画面尺寸无效")?,
                    kCMVideoCodecType_H264,
                    Some(low_latency.as_opaque()),
                    None,
                    None,
                    Some(output),
                    std::ptr::from_ref::<Callback>(&callback).cast_mut().cast(),
                    NonNull::from(&mut raw),
                );
                if status != 0 {
                    return Err(failure("硬件编码器不可用", status));
                }
                CFRetained::from_raw(NonNull::new(raw).ok_or("硬件编码器不可用")?)
            };
            // Loss recovery is by keyframe request; a periodic IDR only bounds
            // how long a receiver that never asks can stay broken.
            let interval = CFNumber::new_f64(20.0);
            let max_qp = CFNumber::new_i32(51);
            // SAFETY: immutable framework constants, valid for the session lifetime.
            let level = unsafe { kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel };
            // SAFETY: immutable framework key.
            let status = set(
                &session,
                unsafe { kVTCompressionPropertyKey_ProfileLevel },
                level.as_ref(),
            );
            if status != 0 {
                return Err(failure("无法设置协商的视频编码等级", status));
            }
            // SAFETY: VideoToolbox's key and value constants are immutable CFStrings.
            let fixed: [(&CFString, &CFType); 4] = unsafe {
                [
                    (
                        kVTCompressionPropertyKey_RealTime,
                        CFBoolean::new(true).as_ref(),
                    ),
                    (
                        kVTCompressionPropertyKey_AllowFrameReordering,
                        CFBoolean::new(false).as_ref(),
                    ),
                    (
                        kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                        interval.as_ref(),
                    ),
                    (kVTCompressionPropertyKey_MaxAllowedFrameQP, max_qp.as_ref()),
                ]
            };
            for (key, value) in fixed {
                set(&session, key, value);
            }
            let mut encoder = Self {
                profile,
                session,
                rate: (0, 0),
                start: Instant::now(),
                _callback: callback,
                completed,
            };
            encoder.set_rate(bitrate, fps);
            // Preparing allocates codec resources and may fail even after
            // session creation succeeded. Keep ownership until teardown and
            // do not report readiness for a codec that rejected preparation.
            // SAFETY: valid, thread-owned session.
            let status = unsafe { encoder.session.prepare_to_encode_frames() };
            if status != 0 {
                return Err(failure("系统编码器准备失败", status));
            }
            Ok(encoder)
        }

        pub fn can_encode(&self) -> bool {
            self._callback.pending.load(Ordering::Acquire) == 0
        }

        pub fn set_rate(&mut self, bitrate: u32, fps: u32) {
            let (bitrate, fps) = self.profile.rate(bitrate, fps);
            if self.rate == (bitrate, fps) {
                return;
            }
            self.rate = (bitrate, fps);
            let average = CFNumber::new_i64(i64::from(bitrate));
            // A hard ceiling over one second, so a burst cannot queue up
            // seconds of video in the pacer.
            let bytes = CFNumber::new_i64(i64::from(bitrate) * 3 / 2 / 8);
            let seconds = CFNumber::new_f64(1.0);
            let limits = CFArray::<CFNumber>::from_retained_objects(&[bytes, seconds]);
            let frames = CFNumber::new_i32(i32::try_from(fps).unwrap_or(30));
            // SAFETY: VideoToolbox's key constants are immutable CFStrings.
            let keys = unsafe {
                [
                    kVTCompressionPropertyKey_AverageBitRate,
                    kVTCompressionPropertyKey_DataRateLimits,
                    kVTCompressionPropertyKey_ExpectedFrameRate,
                ]
            };
            let values: [&CFType; 3] = [
                average.as_ref(),
                limits.as_opaque().as_ref(),
                frames.as_ref(),
            ];
            for (key, value) in keys.into_iter().zip(values) {
                set(&self.session, key, value);
            }
        }

        pub fn error(&self) -> Option<String> {
            self._callback
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        pub fn encode(&mut self, frame: &Frame, captured: Instant, keyframe: bool) -> Result<bool> {
            if let Some(error) = self.error() {
                return Err(error);
            }
            let micros = i64::try_from(self.start.elapsed().as_micros()).unwrap_or(i64::MAX);
            let refcon = Box::into_raw(Box::new(captured));
            self._callback.pending.fetch_add(1, Ordering::AcqRel);
            // SAFETY: valid session and pixel buffer; the options dictionary
            // is retained by VideoToolbox as needed; `refcon` is reclaimed by
            // `output`, or below when the encoder refuses the frame.
            let status = unsafe {
                let options = keyframe.then(|| {
                    CFDictionary::<CFString, CFType>::from_slices(
                        &[kVTEncodeFrameOptionKey_ForceKeyFrame],
                        &[CFBoolean::new(true).as_ref()],
                    )
                });
                let status = self.session.encode_frame(
                    &frame.0,
                    CMTime::new(micros, 1_000_000),
                    CMTime::new(0, 0),
                    options.as_deref().map(CFDictionary::as_opaque),
                    refcon.cast(),
                    std::ptr::null_mut(),
                );
                if status != 0 {
                    drop(Box::from_raw(refcon));
                    self._callback.pending.fetch_sub(1, Ordering::AcqRel);
                }
                status
            };
            if status == 0 {
                self.error().map_or(Ok(true), Err)
            } else {
                Err(failure("视频编码失败", status))
            }
        }
    }

    type Input = (Frame, Instant, bool, (u32, u32));

    /// Only the facade crosses threads. Native state, including teardown,
    /// remains on a bounded worker and never retains the publishing sink.
    pub struct Encoder {
        worker: super::worker::Worker<Input, Option<EncodedFrame>>,
        size: (u32, u32),
        rate: (u32, u32),
        profile: VideoProfile,
        sink: Sink,
        error: Option<String>,
    }

    impl Encoder {
        pub fn new(width: u32, height: u32, fps: u32, bitrate: u32, sink: Sink) -> Result<Self> {
            Self::with_profile(
                width,
                height,
                fps,
                bitrate,
                sink,
                VideoProfile::ConstrainedBaselineAuto,
            )
        }

        pub fn with_profile(
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            sink: Sink,
            profile: VideoProfile,
        ) -> Result<Self> {
            if profile_size(width, height, width, profile)? != (width, height) {
                return Err("画面尺寸超过视频协商等级".into());
            }
            let worker = super::worker::Worker::start_with_deadlines(
                move || Native::with_profile(width, height, fps, bitrate, profile),
                |native, (frame, captured, keyframe, rate): Input| {
                    if !native.can_encode() {
                        return Err("系统编码器的上一帧尚未结束".into());
                    }
                    native.set_rate(rate.0, rate.1);
                    native.encode(&frame, captured, keyframe)?;
                    native
                        .completed
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(|_| "系统编码未及时交付画面".to_owned())?
                },
                Duration::from_secs(5),
                Duration::from_secs(2),
            )?;
            Ok(Self {
                worker,
                size: (width, height),
                rate: profile.rate(bitrate, fps),
                profile,
                sink,
                error: None,
            })
        }

        pub fn size(&self) -> (u32, u32) {
            self.size
        }
        pub fn can_encode(&self) -> bool {
            self.error.is_none()
        }
        pub fn error(&self) -> Option<String> {
            self.error.clone()
        }

        pub fn set_rate(&mut self, bitrate: u32, fps: u32) {
            // Applying properties can also enter the driver. Keep it in the
            // worker's next bounded operation, not on the capture/owner thread.
            self.rate = self.profile.rate(bitrate, fps);
        }

        pub fn encode(&mut self, frame: &Frame, captured: Instant, keyframe: bool) -> Result<bool> {
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            if frame.size() != self.size {
                return Err("显示器尺寸变化，请重新建立视频连接".into());
            }
            match self
                .worker
                .run((frame.clone(), captured, keyframe, self.rate))
            {
                Ok(frame) => {
                    if let Some(frame) = frame {
                        (self.sink)(frame);
                    }
                    // A native dropped-frame reply completes this submission,
                    // but does not fabricate an actual output/IDR observation.
                    Ok(true)
                }
                Err(error) => {
                    self.error.get_or_insert_with(|| error.clone());
                    Err(error)
                }
            }
        }
    }

    impl Drop for Native {
        fn drop(&mut self) {
            // SAFETY: valid session; completing then invalidating delivers or
            // discards every pending callback before the refcon is freed.
            unsafe {
                self.session
                    .complete_frames(CMTime::new(i64::MAX, 1_000_000));
                self.session.invalidate();
            }
        }
    }

    #[cfg(test)]
    mod admission_tests {
        use super::*;

        #[test]
        fn repeated_real_codec_rebuilds_keep_decodable_idrs_with_one_abandoned_native_call() {
            let _serial = super::super::worker::isolated();
            let (release, blocked) = mpsc::channel();
            let mut abandoned = super::super::worker::Worker::start(
                || Ok(()),
                move |_, _: ()| {
                    let _ = blocked.recv();
                    Ok(())
                },
                Duration::from_millis(100),
            )
            .unwrap();
            assert!(abandoned.run(()).is_err());
            let frames = std::sync::Arc::new(Mutex::new(Vec::new()));
            for index in 0..8 {
                let output = frames.clone();
                let mut encoder = Encoder::with_profile(
                    480,
                    320,
                    30,
                    800_000,
                    std::sync::Arc::new(move |frame| output.lock().unwrap().push(frame)),
                    VideoProfile::ConstrainedBaseline31,
                )
                .unwrap();
                let pixels = Frame::synthetic(480, 320, index % 4).unwrap();
                let stamp = Instant::now();
                encoder.encode(&pixels, stamp, true).unwrap();
                drop(encoder);
                use openh264::formats::YUVSource;
                let frames = frames.lock().unwrap();
                assert_eq!(frames.len(), index as usize + 1);
                let frame = frames.last().unwrap();
                assert_eq!(frame.captured, stamp);
                assert!(frame.keyframe);
                let mut decoder = openh264::decoder::Decoder::new().unwrap();
                let decoded = decoder.decode(&frame.data).unwrap().unwrap();
                assert_eq!(decoded.dimensions(), (480, 320));
                let luma = decoded.y()[100 * decoded.strides().0 + 100];
                if index % 4 == 0 {
                    assert!(luma < 100, "rebuild retained an old bright picture");
                }
                if index % 4 == 3 {
                    assert!(luma > 190, "rebuild retained an old dark picture");
                }
            }
            drop(abandoned);
            release.send(()).unwrap();
        }

        fn facade(
            worker: super::super::worker::Worker<Input, Option<EncodedFrame>>,
            sink: Sink,
        ) -> Encoder {
            Encoder {
                worker,
                sink,
                size: (480, 320),
                rate: (800_000, 30),
                profile: VideoProfile::ConstrainedBaseline31,
                error: None,
            }
        }

        #[test]
        fn a_timed_out_native_call_releases_its_sink_and_a_new_codec_can_run_before_it_returns() {
            let _serial = super::super::worker::isolated();
            let (release, blocked) = mpsc::channel();
            let (exited, finished) = mpsc::channel();
            struct NativeCall(mpsc::Receiver<()>, mpsc::Sender<()>);
            impl Drop for NativeCall {
                fn drop(&mut self) {
                    let _ = self.1.send(());
                }
            }
            let native = super::super::worker::Worker::start(
                move || Ok(NativeCall(blocked, exited)),
                |native, (_, captured, _, _): Input| {
                    native.0.recv().unwrap();
                    Ok(Some(EncodedFrame {
                        cursor_embedded: None,
                        data: vec![0, 0, 0, 1, 0x65],
                        captured,
                        latency: captured.elapsed(),
                        keyframe: true,
                    }))
                },
                Duration::from_millis(200),
            )
            .unwrap();
            let publications = std::sync::Arc::new(AtomicUsize::new(0));
            let sent = publications.clone();
            let sink_owner = std::sync::Arc::new(());
            let weak_owner = std::sync::Arc::downgrade(&sink_owner);
            let mut encoder = facade(
                native,
                std::sync::Arc::new(move |_| {
                    std::hint::black_box(&sink_owner);
                    sent.fetch_add(1, Ordering::AcqRel);
                }),
            );
            let pixels = Frame::synthetic(480, 320, 0).unwrap();
            let error = encoder.encode(&pixels, Instant::now(), true).unwrap_err();
            assert_eq!(encoder.error().as_deref(), Some(error.as_str()));
            assert!(!encoder.can_encode());
            // Cached controls and errors cannot call into the still-blocked driver.
            let now = Instant::now();
            encoder.set_rate(1_000_000, 20);
            assert!(encoder.encode(&pixels, Instant::now(), false).is_err());
            drop(encoder);
            assert!(now.elapsed() < Duration::from_millis(100));
            assert!(
                weak_owner.upgrade().is_none(),
                "the blocked job retained the real sink"
            );

            // One abandoned call still holds its bounded slot. Another real
            // VideoToolbox instance must remain usable, without freeing that slot early.
            let frames = std::sync::Arc::new(Mutex::new(Vec::new()));
            let output = frames.clone();
            let mut next = Encoder::with_profile(
                480,
                320,
                30,
                800_000,
                std::sync::Arc::new(move |frame| output.lock().unwrap().push(frame)),
                VideoProfile::ConstrainedBaseline31,
            )
            .unwrap();
            let stamp = Instant::now();
            next.encode(&pixels, stamp, true).unwrap();
            {
                use openh264::formats::YUVSource;
                let frames = frames.lock().unwrap();
                assert_eq!(frames.len(), 1);
                assert!(frames[0].keyframe);
                assert_eq!(frames[0].captured, stamp);
                let mut decoder = openh264::decoder::Decoder::new().unwrap();
                let decoded = decoder.decode(&frames[0].data).unwrap().unwrap();
                assert_eq!(decoded.dimensions(), (480, 320));
            }
            drop(next);
            release.send(()).unwrap();
            finished.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                publications.load(Ordering::Acquire),
                0,
                "timed-out native output escaped"
            );
        }

        #[test]
        fn a_blocked_native_destructor_does_not_block_the_owner_or_free_its_slot_early() {
            let _serial = super::super::worker::isolated();
            let (release, blocked) = mpsc::channel();
            let (entered, destroying) = mpsc::channel();
            let (exited, finished) = mpsc::channel();
            struct NativeDrop(mpsc::Receiver<()>, mpsc::Sender<()>, mpsc::Sender<()>);
            impl Drop for NativeDrop {
                fn drop(&mut self) {
                    self.1.send(()).unwrap();
                    self.0.recv().unwrap();
                    let _ = self.2.send(());
                }
            }
            let worker = super::super::worker::Worker::start(
                move || Ok(NativeDrop(blocked, entered, exited)),
                |_, _: Input| Ok(None),
                Duration::from_secs(1),
            )
            .unwrap();
            let encoder = facade(worker, std::sync::Arc::new(|_| {}));
            let now = Instant::now();
            drop(encoder);
            assert!(now.elapsed() < Duration::from_millis(100));
            destroying.recv_timeout(Duration::from_secs(1)).unwrap();
            let mut second = super::super::worker::Worker::start(
                || Ok(()),
                |_, _: ()| Ok(7),
                Duration::from_secs(1),
            )
            .unwrap();
            assert_eq!(second.run(()).unwrap(), 7);
            // Both slots remain occupied even though the first facade is gone.
            assert!(
                super::super::worker::Worker::start(
                    || Ok(()),
                    |_, _: ()| Ok(()),
                    Duration::from_secs(1)
                )
                .is_err()
            );
            drop(second);
            release.send(()).unwrap();
            finished.recv_timeout(Duration::from_secs(1)).unwrap();
        }

        #[test]
        fn a_dropped_native_frame_returns_credit_without_fabricating_output_or_an_error() {
            let (completed, received) = mpsc::sync_channel(1);
            let callback = Box::new(Callback {
                profile: VideoProfile::ConstrainedBaseline31,
                error: Mutex::new(None),
                completed,
                pending: std::sync::Arc::new(AtomicUsize::new(1)),
                observing: None,
            });
            let captured = Box::into_raw(Box::new(Instant::now()));
            // SAFETY: test owns the exact context and per-frame box required by
            // output; a FrameDropped callback has no CMSampleBuffer to inspect.
            unsafe {
                output(
                    std::ptr::from_ref::<Callback>(&callback).cast_mut().cast(),
                    captured.cast(),
                    0,
                    VTEncodeInfoFlags::FrameDropped,
                    std::ptr::null_mut(),
                );
            }
            assert!(
                received
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
                    .unwrap()
                    .is_none()
            );
            assert_eq!(callback.pending.load(Ordering::Acquire), 0);
            assert!(callback.error.lock().unwrap().is_none());
        }

        #[test]
        fn asynchronous_credit_is_held_until_the_native_output_callback_finishes() {
            let _serial = super::super::worker::isolated();
            let (entered, observing) = mpsc::sync_channel(1);
            let (release, resume) = mpsc::sync_channel::<()>(1);
            let (credit, received_credit) = mpsc::sync_channel(1);
            // Even this direct native test creates, uses and drops the codec
            // on the same owned thread. Only the credit counter crosses it.
            let worker = std::thread::spawn(move || {
                let initializing = Instant::now();
                let mut encoder = Native::with_profile(
                    480,
                    320,
                    30,
                    800_000,
                    VideoProfile::ConstrainedBaseline31,
                )
                .unwrap();
                eprintln!("native fixture initialized in {:?}", initializing.elapsed());
                let resume = Mutex::new(resume);
                let first = std::sync::atomic::AtomicBool::new(true);
                encoder._callback.observing = Some(std::sync::Arc::new(move || {
                    if first.swap(false, Ordering::AcqRel) {
                        let _ = entered.send(());
                        let _ = resume.lock().unwrap().recv_timeout(Duration::from_secs(2));
                    }
                }));
                credit.send(encoder._callback.pending.clone()).unwrap();
                encoder
                    .encode(
                        &Frame::synthetic(480, 320, 0).unwrap(),
                        Instant::now(),
                        true,
                    )
                    .unwrap();
                let frame = encoder
                    .completed
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(frame.keyframe);
                assert!(encoder.can_encode());
            });
            let pending = received_credit
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            observing.recv_timeout(Duration::from_secs(2)).unwrap();
            let in_callback = pending.load(Ordering::Acquire);
            release.send(()).unwrap();
            worker.join().unwrap();
            assert_eq!(
                in_callback, 1,
                "native credit returned during callback processing"
            );
            assert_eq!(pending.load(Ordering::Acquire), 0);
        }
    }

    // ── Capture ───────────────────────────────────────────────────────────

    pub type OnFrame = Box<dyn Fn(Frame) + Send + Sync>;

    pub struct Ivars {
        delivery: std::sync::Arc<super::capture_worker::Delivery<Frame>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "DogCodesScreenOutput"]
        #[ivars = Ivars]
        pub struct Output;

        unsafe impl NSObjectProtocol for Output {}

        unsafe impl SCStreamOutput for Output {
            #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
            fn stream_did_output(
                &self,
                _stream: &SCStream,
                sample: &CMSampleBuffer,
                kind: SCStreamOutputType,
            ) {
                if kind != SCStreamOutputType::Screen {
                    return;
                }
                // Idle/blank status frames carry no pixels.
                // SAFETY: valid sample buffer for the duration of the call.
                if let Some(image) = unsafe { sample.image_buffer() } {
                    self.ivars().delivery.emit(Frame(image));
                }
            }
        }

        unsafe impl SCStreamDelegate for Output {
            #[unsafe(method(stream:didStopWithError:))]
            fn stream_did_stop(&self, _stream: &SCStream, error: &NSError) {
                self.ivars().delivery.stopped(Some(format!(
                    "屏幕采集已结束：{}",
                    error.localizedDescription()
                )));
            }
        }
    );

    impl Output {
        fn new(delivery: std::sync::Arc<super::capture_worker::Delivery<Frame>>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Ivars { delivery });
            // SAFETY: NSObject's designated initialiser on a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }
    }

    struct NativeCapture {
        stream: Retained<SCStream>,
        _output: Retained<Output>,
        _queue: dispatch2::DispatchRetained<DispatchQueue>,
        size: (u32, u32),
        native: (u32, u32),
        delivery: std::sync::Arc<super::capture_worker::Delivery<Frame>>,
        requested: bool,
        _display_changes: super::display_changes::Subscription,
        changes: std::sync::Arc<super::display_changes::Changes>,
        geometry: super::display_changes::Probe<DisplayGeometry>,
        display: u32,
        fps: u32,
        pixel_format: u32,
    }

    #[derive(PartialEq)]
    struct DisplayGeometry {
        bounds: [u64; 4],
        pixels: (usize, usize),
        rotation: u64,
    }
    fn display_geometry(display: u32) -> Result<DisplayGeometry> {
        use objc2_core_graphics::{
            CGDisplayBounds, CGDisplayIsActive, CGDisplayIsOnline, CGDisplayPixelsHigh,
            CGDisplayPixelsWide, CGDisplayRotation,
        };
        // These calls stay on the bounded native worker, never the OS callback.
        if !CGDisplayIsOnline(display) {
            return Err("视频显示器已断开，请重新选择屏幕".into());
        }
        if !CGDisplayIsActive(display) {
            return Err("视频显示器已停用，请重新选择屏幕".into());
        }
        let rect = CGDisplayBounds(display);
        let pixels = (CGDisplayPixelsWide(display), CGDisplayPixelsHigh(display));
        let rotation = CGDisplayRotation(display);
        let bounds = [
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ];
        if !bounds.iter().all(|v| v.is_finite())
            || rect.size.width <= 0.
            || rect.size.height <= 0.
            || pixels.0 == 0
            || pixels.1 == 0
            || !rotation.is_finite()
        {
            return Err("无法核实视频显示器配置".into());
        }
        Ok(DisplayGeometry {
            bounds: bounds.map(f64::to_bits),
            pixels,
            rotation: rotation.to_bits(),
        })
    }

    fn wait<T>(rx: &mpsc::Receiver<std::result::Result<T, String>>, what: &str) -> Result<T> {
        rx.recv().map_err(|_| format!("{what}未返回结果"))?
    }

    fn error_text(error: *mut NSError) -> String {
        // SAFETY: a non-null NSError passed to a completion handler is valid
        // for the duration of that handler.
        unsafe { error.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_default()
    }

    fn configuration(
        size: (u32, u32),
        fps: u32,
        pixel_format: u32,
    ) -> Retained<SCStreamConfiguration> {
        // SAFETY: plain property setters on a fresh configuration object.
        unsafe {
            let config = SCStreamConfiguration::new();
            config.setWidth(size.0 as usize);
            config.setHeight(size.1 as usize);
            config.setPixelFormat(pixel_format);
            config.setMinimumFrameInterval(CMTime::new(1, i32::try_from(fps).unwrap_or(30)));
            config.setShowsCursor(true);
            // One frame is held for still-screen keyframes; leave room for
            // the capture side to keep going.
            config.setQueueDepth(
                if pixel_format == objc2_core_video::kCVPixelFormatType_32BGRA {
                    3
                } else {
                    6
                },
            );
            config
        }
    }

    impl NativeCapture {
        pub fn start(
            config: VideoConfig,
            delivery: std::sync::Arc<super::capture_worker::Delivery<Frame>>,
            changes: std::sync::Arc<super::display_changes::Changes>,
            pixel_format: u32,
        ) -> Result<(Self, (u32, u32))> {
            delivery.check_active()?;
            let watch = super::display_changes::subscribe(config.display, changes.clone())?;
            let geometry = super::display_changes::Probe::new(
                display_geometry(config.display)?,
                Instant::now(),
            );
            let preparing = delivery.clone();
            preparing.after_prepare(
                || Self::content(&delivery),
                |content| {
                    Self::from_content(
                        config,
                        delivery.clone(),
                        content,
                        watch,
                        changes,
                        geometry,
                        pixel_format,
                    )
                },
            )
        }
        fn content(
            delivery: &super::capture_worker::Delivery<Frame>,
        ) -> Result<Retained<SCShareableContent>> {
            delivery.check_active()?;
            let (tx, rx) = mpsc::sync_channel(1);
            let handler = RcBlock::new(
                move |content: *mut SCShareableContent, error: *mut NSError| {
                    // SAFETY: the content object is valid during the handler;
                    // retaining it extends its life past it.
                    let content = unsafe { content.as_ref() }
                        .map(|c| unsafe { Retained::retain(std::ptr::from_ref(c).cast_mut()) });
                    let _ = tx.send(match content.flatten() {
                        Some(content) => Ok(content),
                        None => Err(format!("无法读取可录制的屏幕：{}", error_text(error))),
                    });
                },
            );
            // SAFETY: the block is retained by ScreenCaptureKit until called.
            unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
            wait(&rx, "读取屏幕")
        }
        fn from_content(
            config: VideoConfig,
            delivery: std::sync::Arc<super::capture_worker::Delivery<Frame>>,
            content: Retained<SCShareableContent>,
            watch: super::display_changes::Subscription,
            changes: std::sync::Arc<super::display_changes::Changes>,
            geometry: super::display_changes::Probe<DisplayGeometry>,
            pixel_format: u32,
        ) -> Result<(Self, (u32, u32))> {
            // SAFETY: accessors on a valid shareable-content object.
            let display: Retained<SCDisplay> = unsafe {
                content
                    .displays()
                    .iter()
                    .find(|d| d.displayID() == config.display)
                    .ok_or("显示器已断开，请刷新屏幕列表")?
            };
            let native = crate::displays()?
                .into_iter()
                .find(|d| d.id == config.display)
                .map(|d| {
                    (
                        (d.width as f32 * d.scale).round() as u32,
                        (d.height as f32 * d.scale).round() as u32,
                    )
                })
                .ok_or("显示器已断开，请刷新屏幕列表")?;
            let size = profile_size(native.0, native.1, config.max_width, config.profile)?;
            delivery.check_active()?;
            let output = Output::new(delivery.clone());
            let queue = DispatchQueue::new("dogcodes.screen", DispatchQueueAttr::SERIAL);
            // SAFETY: valid objects; the stream retains filter and
            // configuration, and holds the output weakly, so `Capture` keeps
            // the output and queue alive for as long as the stream.
            let stream = unsafe {
                let filter = SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::new(),
                );
                let stream = SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &filter,
                    &configuration(size, config.fps, pixel_format),
                    Some(ProtocolObject::from_ref(&*output)),
                );
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(
                        ProtocolObject::from_ref(&*output),
                        SCStreamOutputType::Screen,
                        Some(&queue),
                    )
                    .map_err(|e| format!("无法开始录屏：{}", e.localizedDescription()))?;
                stream
            };
            let mut capture = Self {
                stream,
                _output: output,
                _queue: queue,
                size,
                native,
                delivery: delivery.clone(),
                requested: false,
                _display_changes: watch,
                changes,
                geometry,
                display: config.display,
                fps: config.fps,
                pixel_format,
            };
            delivery.check_active()?;
            let starting = delivery.clone();
            let started = RcBlock::new(move |error: *mut NSError| {
                starting.started(if error.is_null() {
                    Ok(())
                } else {
                    Err(format!(
                        "无法开始录屏，请检查屏幕录制权限：{}",
                        error_text(error)
                    ))
                });
            });
            capture.requested = true;
            // SAFETY: valid stream; the framework retains this block until its
            // completion. The outer actor deadline never frees NativeCapture.
            unsafe {
                capture
                    .stream
                    .startCaptureWithCompletionHandler(Some(&started))
            };
            delivery.wait_started()?;
            delivery.check_active()?;
            Ok((capture, size))
        }

        pub fn set_fps(&mut self, fps: u32) -> Result<()> {
            self.delivery.check_active()?;
            if self.geometry.check(
                Instant::now(),
                || display_geometry(self.display),
                &self.changes,
            )? || self.changes.suspended()
                || self.fps == fps
            {
                return Ok(());
            }
            let (sender, reply) = mpsc::sync_channel(1);
            let applied = RcBlock::new(move |error: *mut NSError| {
                let _ = sender.send(if error.is_null() {
                    Ok(())
                } else {
                    Err(format!("无法更新屏幕采集：{}", error_text(error)))
                });
            });
            // SAFETY: valid stream/configuration, block retained by framework.
            unsafe {
                self.stream.updateConfiguration_completionHandler(
                    &configuration(self.size, fps, self.pixel_format),
                    Some(&applied),
                );
            }
            wait(&reply, "更新屏幕采集")?;
            self.fps = fps;
            Ok(())
        }
    }

    impl Drop for NativeCapture {
        fn drop(&mut self) {
            if !self.requested || self.delivery.is_stopped() {
                return;
            }
            // Start may have completed after the facade's deadline. Wait on the
            // worker for its actual result before asking that same stream to stop.
            if self.delivery.wait_started().is_err() || self.delivery.is_stopped() {
                return;
            }
            let delivery = self.delivery.clone();
            let stopped = RcBlock::new(move |error: *mut NSError| {
                if error.is_null() {
                    delivery.stopped(None);
                } else {
                    delivery.fail(format!("屏幕采集未确认停止：{}", error_text(error)));
                }
            });
            // SAFETY: the native stream and delegate remain alive on this
            // worker until successful stop acknowledgement or didStopWithError.
            unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&stopped)) };
            self.delivery.wait_stopped();
        }
    }

    pub struct Capture(
        super::capture_worker::Capture<Frame>,
        std::sync::Arc<super::display_changes::Changes>,
    );
    impl Capture {
        pub fn start(
            config: VideoConfig,
            on_frame: OnFrame,
            parent_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
            on_retire: impl Fn(Option<String>) + Send + Sync + 'static,
            pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
            changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        ) -> Result<(Self, (u32, u32))> {
            Self::start_with_format(
                config,
                on_frame,
                std::sync::Arc::new(move || !parent_stop.load(Ordering::Acquire)),
                on_retire,
                pending,
                changed,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            )
        }
        fn start_with_format(
            config: VideoConfig,
            on_frame: OnFrame,
            alive: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
            on_retire: impl Fn(Option<String>) + Send + Sync + 'static,
            pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
            changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
            pixel_format: u32,
        ) -> Result<(Self, (u32, u32))> {
            let changes =
                std::sync::Arc::new(super::display_changes::Changes::new(pending, changed));
            let observing = changes.clone();
            let capture = super::capture_worker::Capture::start_with_hook(
                move |delivery| {
                    let (native, size) =
                        NativeCapture::start(config, delivery, observing, pixel_format)?;
                    let info = super::capture_worker::Info {
                        size,
                        native: native.native,
                    };
                    Ok((native, info))
                },
                |native, fps| native.set_fps(fps),
                on_frame,
                on_retire,
                alive,
                Duration::from_secs(10),
                Duration::from_secs(2),
            )?;
            let size = capture.info().size;
            Ok((Self(capture, changes), size))
        }
        pub fn error(&self) -> Option<String> {
            self.0.error().or_else(|| self.1.error(Instant::now()))
        }
        pub fn native_size(&self) -> (u32, u32) {
            self.0.info().native
        }
        pub fn set_fps(&self, fps: u32) {
            self.0.set_fps(fps);
        }
        pub fn stop(&self) {
            self.0.stop();
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod software;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
mod accelerated;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod mf;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod platform {
    pub(super) use super::accelerated::Encoder;
    pub(super) use super::software::{Capture, Frame};
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) use super::accelerated::Encoder;
    pub(super) use super::software::{Capture, Frame};
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::{EncodedFrame, Result, Sink, VideoConfig};
    use std::time::Instant;
    #[derive(Clone)]
    pub struct Frame;
    impl Frame {
        pub fn size(&self) -> (u32, u32) {
            (0, 0)
        }
        pub fn synthetic(_: u32, _: u32, _: u32) -> Result<Self> {
            Err(UNSUPPORTED.into())
        }
    }
    pub struct Encoder;
    impl Encoder {
        pub fn error(&self) -> Option<String> {
            None
        }
        pub fn with_profile(
            _: u32,
            _: u32,
            _: u32,
            _: u32,
            _: Sink,
            _: super::VideoProfile,
        ) -> Result<Self> {
            Err(UNSUPPORTED.into())
        }
        pub fn new(_: u32, _: u32, _: u32, _: u32, _: Sink) -> Result<Self> {
            Err(UNSUPPORTED.into())
        }
        pub fn size(&self) -> (u32, u32) {
            (0, 0)
        }
        pub fn can_encode(&self) -> bool {
            false
        }
        pub fn set_rate(&mut self, _: u32, _: u32) {}
        pub fn encode(&mut self, _: &Frame, _: Instant, _: bool) -> Result<bool> {
            Err(UNSUPPORTED.into())
        }
    }
    pub struct Capture;
    const UNSUPPORTED: &str = "此系统暂不支持视频画面，已使用图片模式";
    impl Capture {
        pub fn error(&self) -> Option<String> {
            None
        }
        pub fn start(
            _: VideoConfig,
            _: Box<dyn Fn(Frame) + Send + Sync>,
        ) -> Result<(Self, (u32, u32))> {
            let _ = EncodedFrame {
                cursor_embedded: None,
                captured: std::time::Instant::now(),
                data: Vec::new(),
                keyframe: false,
                latency: std::time::Duration::ZERO,
            };
            Err(UNSUPPORTED.into())
        }
        pub fn native_size(&self) -> (u32, u32) {
            (0, 0)
        }
        pub fn set_fps(&self, _: u32) {}
        pub fn stop(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_parent_refuses_startup_before_any_os_capture_check() {
        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let output = delivered.clone();
        let result = VideoStream::start_controlled(
            VideoConfig {
                display: 0,
                max_width: 480,
                fps: 30,
                bitrate: 800_000,
                profile: VideoProfile::ConstrainedBaseline31,
            },
            Arc::new(move |_| {
                output.fetch_add(1, Ordering::AcqRel);
            }),
            None,
            Arc::new(|| EncodeAdmission::Frame),
            Arc::new(AtomicBool::new(true)),
        );
        assert_eq!(
            result.err().expect("cancelled startup succeeded"),
            "视频请求已结束"
        );
        assert_eq!(delivered.load(Ordering::Acquire), 0);
    }

    #[test]
    fn auto_relay_size_is_bounded_for_portrait_ultrawide_and_large_displays() {
        for (width, height) in [(8192, 8192), (3456, 8000), (8000, 3456), (100, 100_000)] {
            let size =
                profile_size(width, height, 3840, VideoProfile::ConstrainedBaselineAuto).unwrap();
            assert!(size.0 <= 3840 && size.1 <= 3840);
            assert!(u64::from(size.0) * u64::from(size.1) <= 3840 * 2160);
        }
        assert!(profile_size(1, u32::MAX, 3840, VideoProfile::ConstrainedBaselineAuto).is_err());
    }

    #[test]
    fn refuses_actual_bitstreams_exceeding_the_negotiated_profile() {
        let mut frame = EncodedFrame {
            cursor_embedded: None,
            captured: std::time::Instant::now(),
            data: vec![0, 0, 0, 1, 0x67, 66, 0xc0, 31],
            keyframe: true,
            latency: Duration::ZERO,
        };
        assert!(validate_profile(&frame, VideoProfile::ConstrainedBaseline31).is_ok());
        frame.data[7] = 40;
        assert!(validate_profile(&frame, VideoProfile::ConstrainedBaseline31).is_err());
        assert!(validate_profile(&frame, VideoProfile::ConstrainedBaselineAuto).is_ok());
        frame.data[7] = 31;
        frame.data[6] = 0x80;
        assert!(validate_profile(&frame, VideoProfile::ConstrainedBaseline31).is_err());
        frame.data.clear();
        assert!(validate_profile(&frame, VideoProfile::ConstrainedBaseline31).is_err());
    }

    #[test]
    fn negotiated_level_bounds_real_macroblocks_in_all_orientations() {
        for (width, height) in [
            (3456, 2234),
            (1280, 800),
            (1920, 1080),
            (1080, 1920),
            (7680, 2160),
            (200, 8000),
            (u32::MAX, 1),
        ] {
            let size =
                profile_size(width, height, 1920, VideoProfile::ConstrainedBaseline31).unwrap();
            assert!(
                u64::from(size.0.div_ceil(16)) * u64::from(size.1.div_ceil(16)) <= 3600,
                "{size:?}"
            );
            assert!(size.0 <= 2560 && size.1 <= 2560);
            assert_eq!((size.0 % 2, size.1 % 2), (0, 0));
        }
        assert!(profile_size(1, u32::MAX, 1920, VideoProfile::ConstrainedBaseline31).is_err());
        assert_eq!(
            profile_size(3456, 2234, 1920, VideoProfile::ConstrainedBaselineAuto).unwrap(),
            (1920, 1240)
        );
        assert_eq!(
            VideoProfile::ConstrainedBaseline31.rate(50_000_000, 60),
            (9_000_000, 30)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn negotiated_level_bounds_real_encoder_sps() {
        #[cfg(target_os = "macos")]
        let _serial = worker::isolated();
        let output: Arc<Mutex<Vec<EncodedFrame>>> = Arc::default();
        let sink = output.clone();
        let mut encoder = SyntheticEncoder::with_profile(
            (1280, 720),
            30,
            9_000_000,
            Arc::new(move |frame| sink.lock().unwrap().push(frame)),
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        encoder.encode_next(true).unwrap();
        drop(encoder);
        let frames = output.lock().unwrap();
        let frame = frames.first().expect("encoded IDR");
        let sps = frame
            .data
            .windows(8)
            .find(|n| n[..4] == [0, 0, 0, 1] && n[4] & 31 == 7)
            .expect("SPS");
        assert_eq!(sps[5], 66, "must actually encode Baseline");
        assert_ne!(
            sps[6] & 0x40,
            0,
            "must actually encode Constrained Baseline subset"
        );
        assert!(sps[7] <= 31, "must fit SDP receiver level: {}", sps[7]);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn pointer_mode_changes_force_an_encoded_frame_even_with_identical_pixels() {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let sent = frames.clone();
        let output = Arc::new(Mutex::new(refresh::Output::default()));
        let flow = flow::Flow::new(Arc::new(|| EncodeAdmission::Frame));
        let shared = Shared {
            profile: VideoProfile::ConstrainedBaseline31,
            sink: flow.track(
                Arc::new(move |frame| sent.lock().unwrap().push(frame)),
                output.clone(),
            ),
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(true),
            bitrate: AtomicU32::new(800_000),
            fps: AtomicU32::new(30),
            last: Mutex::new(None),
            refresh: Mutex::new(refresh::Refresh::new()),
            output,
            flow,
            latest_pending: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            // Synthetic test only: this cached proof prevents consulting the
            // real system bus while exercising the ordinary capture/encode path.
            unlocked: Mutex::new((Instant::now() + Duration::from_secs(60), true)),
            error: Arc::new(Mutex::new(None)),
        };
        for cursor_embedded in [None, Some(true), Some(false)] {
            shared.captured(platform::Frame {
                cursor_embedded,
                ..platform::Frame::synthetic(64, 32, 0).unwrap()
            });
        }
        let frames = frames.lock().unwrap();
        assert_eq!(
            frames.iter().map(|f| f.cursor_embedded).collect::<Vec<_>>(),
            [None, Some(true), Some(false)]
        );
        assert!(frames.iter().all(|f| f.keyframe));
    }

    #[test]
    fn skipped_output_retains_idr_and_terminal_errors_retire_encoding() {
        let shared = Shared {
            profile: VideoProfile::ConstrainedBaseline31,
            sink: Arc::new(|_| {}),
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(false),
            bitrate: AtomicU32::new(400_000),
            fps: AtomicU32::new(30),
            last: Mutex::new(None),
            refresh: Mutex::new(refresh::Refresh::new()),
            output: Arc::new(Mutex::new(refresh::Output::default())),
            flow: flow::Flow::new(Arc::new(|| EncodeAdmission::Frame)),
            latest_pending: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            unlocked: Mutex::new((Instant::now(), true)),
            error: Arc::new(Mutex::new(None)),
        };
        shared.encoded(Ok(false), true);
        assert!(shared.keyframe.load(Ordering::Acquire));
        assert_eq!(shared.output.lock().unwrap().sequence, 0);
        shared.encoded(Ok(true), false);
        assert_eq!(
            shared.output.lock().unwrap().sequence,
            0,
            "an accepted submission must not invent codec output"
        );
        shared.encoded(Err("native failure".into()), false);
        assert!(shared.stopped.load(Ordering::Acquire));
        shared.encoded(Err("later failure".into()), false);
        assert_eq!(
            shared.error.lock().unwrap().as_deref(),
            Some("native failure")
        );
        assert_eq!(shared.initialize((480, 320)).unwrap_err(), "native failure");
        assert!(
            shared.encoder.lock().unwrap().is_none(),
            "readiness resurrected a failed first input"
        );
    }

    #[test]
    fn downstream_backpressure_keeps_only_the_latest_picture_and_pending_idr() {
        #[cfg(target_os = "macos")]
        let _serial = worker::isolated();
        use openh264::formats::YUVSource;
        let gate = Arc::new(AtomicBool::new(false));
        let permission = gate.clone();
        let output = Arc::new(Mutex::new(refresh::Output::default()));

        let frames: Arc<Mutex<Vec<EncodedFrame>>> = Arc::default();
        let pictures = frames.clone();
        let submitted = gate.clone();
        let flow = flow::Flow::new(Arc::new(move || {
            if permission.load(Ordering::Acquire) {
                EncodeAdmission::Frame
            } else {
                EncodeAdmission::Wait
            }
        }));
        let sink = flow.track(
            Arc::new(move |frame| {
                if submitted.swap(false, Ordering::AcqRel) {
                    pictures
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(frame);
                }
            }),
            output.clone(),
        );
        let shared = Shared {
            profile: VideoProfile::ConstrainedBaseline31,
            sink,
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(true),
            bitrate: AtomicU32::new(800_000),
            fps: AtomicU32::new(30),
            last: Mutex::new(None),
            refresh: Mutex::new(refresh::Refresh::new()),
            output,
            flow,
            latest_pending: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            unlocked: Mutex::new((Instant::now(), true)),
            error: Arc::new(Mutex::new(None)),
        };
        for t in [0, 1, 3] {
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.captured(platform::Frame::synthetic(480, 320, t).unwrap());
        }
        assert!(
            shared.encoder.lock().unwrap().is_none(),
            "denied pixels reached the codec"
        );
        assert!(shared.latest_pending.load(Ordering::Acquire));
        *shared.encoder.lock().unwrap() = Some(
            platform::Encoder::with_profile(
                480,
                320,
                30,
                800_000,
                shared.sink.clone(),
                VideoProfile::ConstrainedBaseline31,
            )
            .unwrap(),
        );
        assert_eq!(frames.lock().unwrap().len(), 0);
        gate.store(true, Ordering::Release);
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let received = frames.lock().unwrap().len();
            if received >= 1 || Instant::now() >= end {
                break;
            }
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let failure = shared.error.lock().unwrap().clone();
        assert!(failure.is_none(), "{failure:?}");
        {
            let frames = frames.lock().unwrap();
            assert_eq!(frames.len(), 1);
            assert!(frames[0].keyframe);
            let image = decoder.decode(&frames[0].data).unwrap().unwrap();
            assert!(
                image.y()[100 * image.strides().0 + 100] > 190,
                "resumption encoded an obsolete cached picture"
            );
        }
        shared.keyframe.store(true, Ordering::Release);
        *shared.unlocked.lock().unwrap() = (Instant::now(), true);
        shared.captured(platform::Frame::synthetic(480, 320, 0).unwrap());
        shared.tick();
        assert_eq!(frames.lock().unwrap().len(), 1);
        assert!(
            shared.keyframe.load(Ordering::Acquire),
            "denied retry consumed the IDR request"
        );
        assert!(!shared.stopped.load(Ordering::Acquire));
        gate.store(true, Ordering::Release);
        *shared.unlocked.lock().unwrap() = (Instant::now(), true);
        shared.captured(platform::Frame::synthetic(480, 320, 0).unwrap());
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let received = frames.lock().unwrap().len();
            if received >= 2 || Instant::now() >= end {
                break;
            }
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames[1].keyframe);
        let image = decoder.decode(&frames[1].data).unwrap().unwrap();
        assert!(image.y()[100 * image.strides().0 + 100] < 100);
        assert_eq!(
            shared.output.lock().unwrap().sequence,
            2,
            "resumption discarded the first newly admitted source frame"
        );
    }

    #[test]
    fn a_static_synthetic_source_refines_then_stops_and_answers_a_later_idr() {
        #[cfg(target_os = "macos")]
        let _serial = worker::isolated();
        let output = Arc::new(Mutex::new(refresh::Output::default()));
        let progress = output.clone();
        let shared = Shared {
            profile: VideoProfile::ConstrainedBaseline31,
            sink: Arc::new(move |frame| {
                progress
                    .lock()
                    .unwrap()
                    .emitted(frame.data.len(), frame.keyframe);
            }),
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(true),
            bitrate: AtomicU32::new(400_000),
            fps: AtomicU32::new(30),
            last: Mutex::new(None),
            refresh: Mutex::new(refresh::Refresh::new()),
            output: output.clone(),
            flow: flow::Flow::new(Arc::new(|| EncodeAdmission::Frame)),
            latest_pending: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            // Synthetic pixels only; this test never queries the user's OS
            // desktop or uses a capture/authorization bypass in production.
            unlocked: Mutex::new((Instant::now(), true)),
            error: Arc::new(Mutex::new(None)),
        };
        shared.captured(platform::Frame::synthetic(480, 320, 0).unwrap());
        let end = Instant::now() + Duration::from_secs(4);
        while Instant::now() < end {
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
        let before = *output.lock().unwrap();
        assert!(before.sequence > 1, "static refinement produced no output");
        assert!(
            before.sequence <= 13,
            "automatic refinement exceeded its budget"
        );
        assert_eq!(before.keyframes, 1, "refinement must not force IDR bursts");
        for _ in 0..15 {
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(output.lock().unwrap().sequence, before.sequence);
        assert!(!shared.stopped.load(Ordering::Acquire));
        shared.keyframe.store(true, Ordering::Release);
        let end = Instant::now() + Duration::from_secs(1);
        while output.lock().unwrap().keyframes == before.keyframes && Instant::now() < end {
            *shared.unlocked.lock().unwrap() = (Instant::now(), true);
            shared.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(output.lock().unwrap().keyframes > before.keyframes);
        assert!(shared.error.lock().unwrap().is_none());
    }
    #[test]
    fn avcc_becomes_annexb() {
        let avcc = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41];
        let mut out = Vec::new();
        avcc_to_annexb(&avcc, 4, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41]);
        let mut out = Vec::new();
        assert!(avcc_to_annexb(&[0, 0, 0, 5, 1], 4, &mut out).is_err());
        assert!(avcc_to_annexb(&[0, 0, 0, 0], 4, &mut out).is_err());
        assert!(avcc_to_annexb(&[0, 1], 3, &mut out).is_err());
        let mut out = Vec::new();
        avcc_to_annexb(&[0, 1, 0x09], 2, &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0x09]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn hardware_encoder_emits_annexb_with_keyframes_on_request() {
        #[cfg(target_os = "macos")]
        let _serial = worker::isolated();
        let frames = encode_synthetic((640, 360), 20, 60, 1_000_000, &[12]).unwrap();
        assert!(frames.len() >= 18, "only {} frames", frames.len());
        let first = &frames[0];
        assert!(first.keyframe);
        // SPS (7), PPS (8), then the IDR slice (5), each after a start code.
        let types: Vec<u8> = first
            .data
            .windows(5)
            .filter(|w| w[..4] == [0, 0, 0, 1])
            .map(|w| w[4] & 0x1f)
            .collect();
        assert_eq!(&types[..2], &[7, 8]);
        assert!(types.contains(&5));
        let keys = frames.iter().filter(|f| f.keyframe).count();
        assert!(keys >= 2, "forced keyframe missing ({keys})");
        assert!(frames.iter().all(|f| f.data.starts_with(&[0, 0, 0, 1])));
    }

    #[test]
    fn stream_size_keeps_aspect_and_parity() {
        // 3456×2234 Retina panel capped at 1920 wide.
        assert_eq!(stream_size(3456, 2234, 1920), (1920, 1240));
        // Never upscaled.
        assert_eq!(stream_size(1280, 800, 1920), (1280, 800));
        // Odd sizes become even.
        assert_eq!(stream_size(1365, 767, 4096), (1364, 766));
        assert_eq!(stream_size(0, 0, 1920), (2, 2));
    }
}
