//! Screen video: ScreenCaptureKit → VideoToolbox on macOS; bounded screenshot
//! capture → OpenH264 software fallback on Windows and Linux. Wayland uses a
//! connection-owned portal/PipeWire source; accelerated backends remain pending.
//!
//! Encoders emit low-latency Annex-B access units with on-demand keyframes,
//! shared by the WebRTC and authenticated relay paths.
use crate::Result;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// One encoded access unit.
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    /// Annex-B NAL units; a keyframe starts with SPS and PPS.
    pub data: Vec<u8>,
    pub keyframe: bool,
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
}
pub type Source = Option<Arc<dyn ScreenSource>>;

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
    last_encode: Mutex<Instant>,
    refreshed: AtomicBool,
    stopped: AtomicBool,
    /// Last lock-state check: (when, whether capture may be published).
    unlocked: Mutex<(Instant, bool)>,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn encode(&self, frame: &platform::Frame, captured: Instant, refresh: bool) {
        let force = self.keyframe.swap(false, Ordering::AcqRel);
        let mut encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
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
                    *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                    return;
                }
            }
        }
        let Some(encoder) = encoder.as_mut() else {
            return;
        };
        encoder.set_rate(
            self.bitrate.load(Ordering::Acquire),
            self.fps.load(Ordering::Acquire),
        );
        self.encoded(encoder.encode(frame, captured, force), force, refresh);
    }

    /// A rate-control skip is not successful output. In particular, a static
    /// screen must retain its IDR/refresh request until an actual submission.
    fn encoded(&self, result: Result<bool>, force: bool, refresh: bool) {
        let accepted = match result {
            Ok(accepted) => accepted,
            Err(error) => {
                *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                self.keyframe.store(true, Ordering::Release);
                false
            }
        };
        if accepted {
            *self.last_encode.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        } else if force {
            self.keyframe.store(true, Ordering::Release);
        }
        if !refresh || !accepted {
            self.refreshed.store(false, Ordering::Release);
        }
    }

    /// Called on the capture queue for every changed frame.
    /// Like the JPEG path's per-frame check, but throttled for 30 fps: no
    /// frame is encoded while the console is locked or its state unknown.
    fn unlocked(&self) -> bool {
        let mut state = self.unlocked.lock().unwrap_or_else(|e| e.into_inner());
        if state.0.elapsed() > Duration::from_millis(200) {
            *state = (Instant::now(), crate::require_unlocked().is_ok());
        }
        state.1
    }

    fn captured(&self, frame: platform::Frame) {
        if self.stopped.load(Ordering::Acquire) || !self.unlocked() {
            return;
        }
        self.encode(&frame, Instant::now(), false);
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
    }

    /// Ticks while the screen is still: answers keyframe requests from the
    /// last frame, and re-encodes it once after motion stops so the picture
    /// sharpens (RustDesk's static refresh) instead of staying at the
    /// quality the motion could afford.
    fn tick(&self) {
        if !self.unlocked() {
            return;
        }
        let idle = self
            .last_encode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        let want_key = self.keyframe.load(Ordering::Acquire) && idle > Duration::from_millis(60);
        let want_refresh =
            idle > Duration::from_millis(400) && !self.refreshed.swap(true, Ordering::AcqRel);
        if !(want_key || want_refresh) {
            return;
        }
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(frame) = last {
            self.encode(&frame, Instant::now(), true);
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
    ticker: Option<std::thread::JoinHandle<()>>,
}

impl VideoStream {
    pub fn start(config: VideoConfig, sink: Sink) -> Result<Self> {
        Self::start_from(config, sink, None)
    }

    pub fn start_from(mut config: VideoConfig, sink: Sink, source: Source) -> Result<Self> {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            config = software::normalize(config);
        }
        (config.bitrate, config.fps) = config.profile.rate(config.bitrate, config.fps);
        let shared = Arc::new(Shared {
            profile: config.profile,
            sink,
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(true),
            bitrate: AtomicU32::new(config.bitrate.clamp(100_000, 50_000_000)),
            fps: AtomicU32::new(config.fps.clamp(1, 60)),
            last: Mutex::new(None),
            last_encode: Mutex::new(Instant::now()),
            refreshed: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            unlocked: Mutex::new((Instant::now(), crate::require_unlocked().is_ok())),
            error: Mutex::new(None),
        });
        let sink = shared.clone();
        let callback = Box::new(move |frame| sink.captured(frame));
        #[cfg(target_os = "linux")]
        let (capture, size) = match source {
            Some(source) => software::Capture::from_source(config, source, callback)?,
            None => platform::Capture::start(config, callback)?,
        };
        #[cfg(not(target_os = "linux"))]
        let (capture, size) = {
            if source.is_some() {
                return Err("此平台不支持指定的屏幕来源".into());
            }
            platform::Capture::start(config, callback)?
        };
        // Capture readiness alone does not prove that an encoder is available.
        let init = {
            let mut encoder = shared.encoder.lock().unwrap_or_else(|e| e.into_inner());
            if encoder.is_none() {
                platform::Encoder::with_profile(
                    size.0,
                    size.1,
                    config.fps,
                    config.bitrate,
                    shared.sink.clone(),
                    config.profile,
                )
                .map(|created| *encoder = Some(created))
            } else {
                Ok(())
            }
        };
        if let Err(error) = init {
            shared.stopped.store(true, Ordering::Release);
            capture.stop();
            return Err(error);
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
                .map_err(|_| "无法启动视频线程")?
        };
        Ok(Self {
            shared,
            capture,
            size,
            ticker: Some(ticker),
        })
    }

    /// Encoded frame size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
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
        if self.shared.fps.swap(fps, Ordering::AcqRel) != fps {
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
        SCStreamOutput, SCStreamOutputType,
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
    use std::sync::Mutex;
    use std::{
        ffi::c_void,
        ptr::NonNull,
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn failure(what: &str, status: i32) -> String {
        format!("{what}（{status}）")
    }

    /// A captured, IOSurface-backed NV12 frame.
    #[derive(Clone)]
    pub struct Frame(CFRetained<CVPixelBuffer>);
    // SAFETY: CVPixelBuffer is a reference-counted CF object whose retain and
    // release are thread-safe; the pixels are only read by the encoder.
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
        sink: Sink,
    }

    pub struct Encoder {
        profile: VideoProfile,
        session: CFRetained<VTCompressionSession>,
        size: (u32, u32),
        rate: (u32, u32),
        start: Instant,
        // Boxed so its address, the callback's refcon, outlives the session.
        _callback: Box<Callback>,
    }
    // SAFETY: VideoToolbox compression sessions may be used from any thread;
    // every use is serialised by the owning Mutex.
    unsafe impl Send for Encoder {}

    unsafe extern "C-unwind" fn output(
        refcon: *mut c_void,
        frame_refcon: *mut c_void,
        status: i32,
        flags: VTEncodeInfoFlags,
        sample: *mut CMSampleBuffer,
    ) {
        // SAFETY: refcon is the Box<Callback> owned by the Encoder, which
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
        if status == 0 && flags.contains(VTEncodeInfoFlags::FrameDropped) {
            return;
        }
        let result = if status != 0 {
            Err(failure("视频编码回调失败", status))
        } else {
            sample
                .ok_or_else(|| "视频编码未返回画面".to_owned())
                .and_then(annexb)
                .and_then(|frame| {
                    super::validate_profile(&frame, callback.profile)?;
                    Ok(frame)
                })
        };
        match result {
            Ok(frame) => (callback.sink)(EncodedFrame {
                latency: captured.elapsed(),
                ..frame
            }),
            Err(error) => {
                callback
                    .error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert(error);
            }
        }
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
            let (bitrate, fps) = profile.rate(bitrate, fps);
            let callback = Box::new(Callback {
                sink,
                profile,
                error: Mutex::new(None),
            });
            let mut raw: *mut VTCompressionSession = std::ptr::null_mut();
            // SAFETY: arguments are valid CF objects or null; `raw` receives
            // a +1 session adopted by CFRetained::from_raw below; the refcon
            // points at the boxed callback this Encoder keeps alive.
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
                size: (width, height),
                rate: (0, 0),
                start: Instant::now(),
                _callback: callback,
            };
            encoder.set_rate(bitrate, fps);
            // SAFETY: valid session.
            unsafe { encoder.session.prepare_to_encode_frames() };
            Ok(encoder)
        }

        pub fn size(&self) -> (u32, u32) {
            self.size
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

    impl Drop for Encoder {
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

    // ── Capture ───────────────────────────────────────────────────────────

    pub type OnFrame = Box<dyn Fn(Frame) + Send + Sync>;

    pub struct Ivars {
        on_frame: OnFrame,
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
                    (self.ivars().on_frame)(Frame(image));
                }
            }
        }
    );

    impl Output {
        fn new(on_frame: OnFrame) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Ivars { on_frame });
            // SAFETY: NSObject's designated initialiser on a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub struct Capture {
        stream: Retained<SCStream>,
        _output: Retained<Output>,
        _queue: dispatch2::DispatchRetained<DispatchQueue>,
        size: (u32, u32),
        native: (u32, u32),
    }

    fn wait<T>(rx: &mpsc::Receiver<std::result::Result<T, String>>, what: &str) -> Result<T> {
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| format!("{what}超时，请检查屏幕录制权限"))?
    }

    fn error_text(error: *mut NSError) -> String {
        // SAFETY: a non-null NSError passed to a completion handler is valid
        // for the duration of that handler.
        unsafe { error.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_default()
    }

    fn configuration(size: (u32, u32), fps: u32) -> Retained<SCStreamConfiguration> {
        // SAFETY: plain property setters on a fresh configuration object.
        unsafe {
            let config = SCStreamConfiguration::new();
            config.setWidth(size.0 as usize);
            config.setHeight(size.1 as usize);
            config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            config.setMinimumFrameInterval(CMTime::new(1, i32::try_from(fps).unwrap_or(30)));
            config.setShowsCursor(true);
            // One frame is held for still-screen keyframes; leave room for
            // the capture side to keep going.
            config.setQueueDepth(6);
            config
        }
    }

    impl Capture {
        pub fn error(&self) -> Option<String> {
            None
        }
        pub fn start(config: VideoConfig, on_frame: OnFrame) -> Result<(Self, (u32, u32))> {
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
            let content: Retained<SCShareableContent> = wait(&rx, "读取屏幕")?;
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
            let output = Output::new(on_frame);
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
                    &configuration(size, config.fps),
                    None,
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
            let (tx, rx) = mpsc::sync_channel(1);
            let started = RcBlock::new(move |error: *mut NSError| {
                let _ = tx.send(if error.is_null() {
                    Ok(())
                } else {
                    Err(format!(
                        "无法开始录屏，请检查屏幕录制权限：{}",
                        error_text(error)
                    ))
                });
            });
            // SAFETY: the block is retained until ScreenCaptureKit calls it.
            unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
            wait(&rx, "开始录屏")?;
            Ok((
                Self {
                    stream,
                    _output: output,
                    _queue: queue,
                    size,
                    native,
                },
                size,
            ))
        }

        pub fn native_size(&self) -> (u32, u32) {
            self.native
        }
        pub fn set_fps(&self, fps: u32) {
            // SAFETY: valid stream and configuration; no completion needed.
            unsafe {
                self.stream
                    .updateConfiguration_completionHandler(&configuration(self.size, fps), None);
            }
        }

        pub fn stop(&self) {
            let (tx, rx) = mpsc::sync_channel(1);
            let stopped = RcBlock::new(move |_: *mut NSError| {
                let _ = tx.send(Ok(()));
            });
            // SAFETY: the block is retained until ScreenCaptureKit calls it.
            unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&stopped)) };
            let _ = wait::<()>(&rx, "停止录屏");
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod software;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod platform {
    pub(super) use super::software::{Capture, Encoder, Frame};
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

    #[test]
    fn skipped_static_output_retains_keyframe_refresh_and_output_clock() {
        let before = Instant::now() - Duration::from_secs(1);
        let shared = Shared {
            profile: VideoProfile::ConstrainedBaseline31,
            sink: Arc::new(|_| {}),
            encoder: Mutex::new(None),
            keyframe: AtomicBool::new(false),
            bitrate: AtomicU32::new(400_000),
            fps: AtomicU32::new(30),
            last: Mutex::new(None),
            last_encode: Mutex::new(before),
            refreshed: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            unlocked: Mutex::new((Instant::now(), true)),
            error: Mutex::new(None),
        };
        shared.encoded(Ok(false), true, true);
        assert!(shared.keyframe.load(Ordering::Acquire));
        assert!(!shared.refreshed.load(Ordering::Acquire));
        assert_eq!(*shared.last_encode.lock().unwrap(), before);
        shared.keyframe.store(false, Ordering::Release);
        shared.encoded(Ok(true), true, true);
        assert!(!shared.keyframe.load(Ordering::Acquire));
        assert!(*shared.last_encode.lock().unwrap() > before);
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
