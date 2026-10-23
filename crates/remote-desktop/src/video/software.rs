//! Bounded CPU H.264 fallback for Windows and Linux. Native screenshots or an
//! authorized continuous Wayland source feed the same bounded capture loop.
//! Native accelerated capture/encoding remains a separate backend to add.
use super::{
    EncodedFrame, Result, Sink, VideoConfig, VideoProfile, profile_size, validate_profile,
};
use image::RgbaImage;
use openh264::{
    OpenH264API, Timestamp,
    encoder::{
        BitRate, Complexity, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
        RateControlMode, UsageType, VuiConfig,
    },
    formats::{RgbaSliceU8, YUVBuffer},
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// The software fallback shares the negotiated CB3.1 budget on all transports.
/// Higher-resolution accelerated backends must negotiate their own budget.
pub(super) fn normalize(mut config: VideoConfig) -> VideoConfig {
    config.profile = VideoProfile::ConstrainedBaseline31;
    (config.bitrate, config.fps) = config.profile.rate(config.bitrate, config.fps);
    config
}
#[derive(Clone)]
pub(super) struct Frame {
    image: Arc<RgbaImage>,
}
impl Frame {
    pub fn size(&self) -> (u32, u32) {
        self.image.dimensions()
    }
    pub fn synthetic(width: u32, height: u32, t: u32) -> Result<Self> {
        dimensions(width, height)?;
        let image = RgbaImage::from_fn(width, height, |x, y| {
            let text = (width / 8..width / 2).contains(&x)
                && (height / 6..height * 2 / 3).contains(&y)
                && ((y + t * 8) / 12).is_multiple_of(2)
                && !(x / 5 + (y + t * 8) / 24).is_multiple_of(3);
            let v = if text { 235 } else { 16 + ((x ^ y) & 63) as u8 };
            image::Rgba([v, v, v, 255])
        });
        Ok(Self {
            image: Arc::new(image),
        })
    }
}
fn dimensions(width: u32, height: u32) -> Result<()> {
    if width < 2
        || height < 2
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || width.max(height) > 3840
        || width.min(height) > 2160
    {
        return Err("软件视频尺寸无效".into());
    }
    Ok(())
}

pub(super) struct Encoder {
    codec: openh264::encoder::Encoder,
    yuv: YUVBuffer,
    size: (u32, u32),
    profile: VideoProfile,
    sink: Sink,
    started: bool,
    since: Instant,
    rate: (u32, u32),
    applied_rate: (u32, u32),
    requested_keyframe: bool,
    failure: Option<String>,
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
        dimensions(width, height)?;
        if profile_size(width, height, width, profile)? != (width, height) {
            return Err("视频尺寸超过协商的 H.264 等级".into());
        }
        let rate = profile.rate(bitrate, fps);
        let mut config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .profile(Profile::Baseline)
            .bitrate(BitRate::from_bps(rate.0))
            .max_frame_rate(FrameRate::from_hz(rate.1 as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .complexity(Complexity::Low)
            .num_threads(2)
            .long_term_reference(false)
            .adaptive_quantization(false)
            .background_detection(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(600))
            .vui(VuiConfig::bt601());
        if profile == VideoProfile::ConstrainedBaseline31 {
            config = config.level(openh264::encoder::Level::Level_3_1);
        }
        let mut codec =
            openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), config)
                .map_err(codec_error)?;
        let yuv = YUVBuffer::new(width as usize, height as usize);
        // The wrapper initializes lazily. Exercise the real native codec and
        // validate its SPS now, before advertising video to a phone. This
        // private warmup frame never reaches the sink; the first screen frame
        // is explicitly forced to a fresh IDR.
        let initial = codec
            .encode_at(&yuv, Timestamp::from_millis(0))
            .map_err(codec_error)?;
        let probe = EncodedFrame {
            data: initial.to_vec(),
            keyframe: initial.frame_type() == FrameType::IDR,
            latency: Duration::ZERO,
        };
        if !probe.keyframe || probe.data.is_empty() {
            return Err("软件编码器初始化未输出关键帧".into());
        }
        validate_profile(&probe, profile)?;
        let mut encoder = Self {
            codec,
            yuv,
            size: (width, height),
            profile,
            sink,
            started: true,
            since: Instant::now(),
            rate,
            applied_rate: (0, 0),
            requested_keyframe: true,
            failure: None,
        };
        encoder.apply_rate()?;
        Ok(encoder)
    }
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
    pub fn error(&self) -> Option<String> {
        self.failure.clone()
    }
    pub fn set_rate(&mut self, bitrate: u32, fps: u32) {
        self.rate = self.profile.rate(bitrate, fps);
    }
    fn apply_rate(&mut self) -> Result<()> {
        if !self.started || self.applied_rate == self.rate {
            return Ok(());
        }
        use openh264_sys2::{
            ENCODER_OPTION_BITRATE, ENCODER_OPTION_FRAME_RATE, ENCODER_OPTION_MAX_BITRATE,
            SBitrateInfo, SPATIAL_LAYER_0, SPATIAL_LAYER_ALL,
        };
        let (bitrate, fps) = self.rate;
        let mut maximum = SBitrateInfo {
            iLayer: SPATIAL_LAYER_ALL,
            iBitrate: (bitrate + bitrate / 2) as i32,
        };
        // OpenH264 keeps a distinct cap for each spatial layer. Updating only
        // ALL leaves layer 0 capped at the constructor's bitrate, so the first
        // congestion-control increase above that value is rejected. Lower the
        // target before lowering its cap; raise both caps before the target.
        let mut layer_maximum = SBitrateInfo {
            iLayer: SPATIAL_LAYER_0,
            ..maximum
        };
        let mut target = SBitrateInfo {
            iLayer: SPATIAL_LAYER_ALL,
            iBitrate: bitrate as i32,
        };
        let mut frame_rate = fps as f32;
        // SAFETY: the encoder is initialized, dimensions stay fixed, and these
        // documented SetOption values read their correctly typed stack values
        // synchronously. Neither pointers nor buffers outlive this call. The
        // wrapper only uses its config again if dimensions change, rejected by
        // encode below, so rate updates cannot silently trigger a stale reinit.
        let statuses = unsafe {
            let api = self.codec.raw_api();
            let mut statuses = Vec::with_capacity(4);
            if bitrate < self.applied_rate.0 {
                statuses.push(api.set_option(
                    ENCODER_OPTION_BITRATE,
                    std::ptr::from_mut(&mut target).cast(),
                ));
            }
            statuses.push(api.set_option(
                ENCODER_OPTION_MAX_BITRATE,
                std::ptr::from_mut(&mut layer_maximum).cast(),
            ));
            statuses.push(api.set_option(
                ENCODER_OPTION_MAX_BITRATE,
                std::ptr::from_mut(&mut maximum).cast(),
            ));
            if bitrate >= self.applied_rate.0 {
                statuses.push(api.set_option(
                    ENCODER_OPTION_BITRATE,
                    std::ptr::from_mut(&mut target).cast(),
                ));
            }
            statuses.push(api.set_option(
                ENCODER_OPTION_FRAME_RATE,
                std::ptr::from_mut(&mut frame_rate).cast(),
            ));
            statuses
        };
        if statuses.into_iter().any(|code| code != 0) {
            return Err("无法调整软件视频码率/帧率".into());
        }
        self.applied_rate = self.rate;
        Ok(())
    }
    pub fn encode(&mut self, frame: &Frame, captured: Instant, force: bool) -> Result<bool> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.encode_inner(frame, captured, force);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn encode_inner(&mut self, frame: &Frame, captured: Instant, force: bool) -> Result<bool> {
        if frame.size() != self.size {
            return Err("显示器尺寸变化，请重新建立视频连接".into());
        }
        self.apply_rate()?;
        self.requested_keyframe |= force;
        if self.started && self.requested_keyframe {
            self.codec.force_intra_frame();
        }
        self.yuv.read_rgba8(RgbaSliceU8::new(
            frame.image.as_raw(),
            (self.size.0 as usize, self.size.1 as usize),
        ));
        let stamp = Timestamp::from_millis(
            u64::try_from(self.since.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
        let output = self
            .codec
            .encode_at(&self.yuv, stamp)
            .map_err(codec_error)?;
        self.started = true;
        // A skipped input does not release a pending keyframe request.
        if output.frame_type() == FrameType::Skip {
            return Ok(false);
        }
        let keyframe = output.frame_type() == FrameType::IDR;
        let data = output.to_vec();
        if data.is_empty() {
            return Ok(false);
        }
        let frame = EncodedFrame {
            data,
            keyframe,
            latency: captured.elapsed(),
        };
        validate_profile(&frame, self.profile)?;
        if keyframe {
            self.requested_keyframe = false;
        }
        (self.sink)(frame);
        // Initializing the native encoder uses the constructor's config. A
        // pre-first-frame rate change is applied immediately after that init.
        self.apply_rate()?;
        Ok(true)
    }
}
fn codec_error(error: impl std::fmt::Display) -> String {
    format!("软件 H.264 编码不可用：{error}")
}

/// At most one active source and one retiring OS call. A stuck capture must not
/// make close block forever or permit repeated reconnects to leak OS threads.
static CAPTURES: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn take() -> Result<Self> {
        CAPTURES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "旧视频采集仍在停止，请稍后重试".into())
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        CAPTURES.fetch_sub(1, Ordering::AcqRel);
    }
}
struct CaptureState {
    stopped: AtomicBool,
    fps: AtomicU32,
    wait: Mutex<()>,
    wake: Condvar,
    error: Mutex<Option<String>>,
}
pub(super) struct Capture {
    state: Arc<CaptureState>,
    native: (u32, u32),
}
impl Capture {
    pub fn start(
        config: VideoConfig,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
    ) -> Result<(Self, (u32, u32))> {
        #[cfg(target_os = "linux")]
        if is_wayland(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        ) {
            // Screenshot portals cannot be polled as a video source: that may
            // repeatedly prompt. Only a persistent ScreenCast session is valid.
            return Err("请先完成 Wayland 屏幕共享授权".into());
        }
        let monitor = xcap::Monitor::all()
            .map_err(|_| "无法枚举视频显示器")?
            .into_iter()
            .find(|m| m.id().ok() == Some(config.display))
            .ok_or("视频显示器已断开")?;
        let native = (
            monitor.width().map_err(|_| "无法读取显示器尺寸")?,
            monitor.height().map_err(|_| "无法读取显示器尺寸")?,
        );
        if native.0 == 0
            || native.1 == 0
            || u64::from(native.0) * u64::from(native.1) > 64 * 1024 * 1024
        {
            return Err("显示器尺寸超过软件采集能力".into());
        }
        let display = config.display;
        Self::start_factory(
            config,
            native,
            move || {
                // Windows HMONITOR (and compositor handles) are not Send. Find
                // and retain the native source on the capture thread itself.
                let monitor = xcap::Monitor::all()
                    .map_err(|_| "无法枚举视频显示器")?
                    .into_iter()
                    .find(|m| m.id().ok() == Some(display))
                    .ok_or("视频显示器已断开")?;
                Ok(move || {
                    monitor
                        .capture_image()
                        .map(Arc::new)
                        .map_err(|_| "视频采集失败，请检查显示器和桌面权限".into())
                })
            },
            sink,
            Duration::from_secs(3),
        )
    }

    #[cfg(target_os = "linux")]
    pub fn from_source(
        config: VideoConfig,
        source: Arc<dyn super::ScreenSource>,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
    ) -> Result<(Self, (u32, u32))> {
        let native = source.size()?;
        Self::start_factory(
            config,
            native,
            move || Ok(move || source.frame()),
            sink,
            Duration::from_secs(3),
        )
    }

    #[cfg(test)]
    fn start_source<F>(
        config: VideoConfig,
        native: (u32, u32),
        source: F,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        timeout: Duration,
    ) -> Result<(Self, (u32, u32))>
    where
        F: FnMut() -> Result<RgbaImage> + Send + 'static,
    {
        let mut source = source;
        Self::start_factory(
            config,
            native,
            move || Ok(move || source().map(Arc::new)),
            sink,
            timeout,
        )
    }
    fn start_factory<F, S>(
        config: VideoConfig,
        native: (u32, u32),
        factory: F,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        timeout: Duration,
    ) -> Result<(Self, (u32, u32))>
    where
        F: FnOnce() -> Result<S> + Send + 'static,
        S: FnMut() -> Result<Arc<RgbaImage>> + 'static,
    {
        let permit = Permit::take()?;
        let size = profile_size(native.0, native.1, config.max_width, config.profile)?;
        dimensions(size.0, size.1)?;
        let state = Arc::new(CaptureState {
            stopped: AtomicBool::new(false),
            fps: AtomicU32::new(config.fps.clamp(1, 60)),
            wait: Mutex::new(()),
            wake: Condvar::new(),
            error: Mutex::new(None),
        });
        let capture = Self {
            state: state.clone(),
            native,
        };
        let (ready, wait) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("remote-video-capture".into())
            .spawn(move || {
                let _permit = permit;
                let mut ready = Some(ready);
                let mut source = match factory() {
                    Ok(source) => source,
                    Err(error) => {
                        *state.error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(error.clone());
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(Err(error));
                        }
                        return;
                    }
                };
                let mut previous: Option<Frame> = None;
                while !state.stopped.load(Ordering::Acquire) {
                    let started = Instant::now();
                    let image = match source().and_then(|image| {
                        if image.dimensions() != native {
                            Err("显示器配置已变化，请重新连接".into())
                        } else {
                            Ok(image)
                        }
                    }) {
                        Ok(image) => image,
                        Err(error) => {
                            *state.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error.clone());
                            if let Some(ready) = ready.take() {
                                let _ = ready.send(Err(error));
                            }
                            break;
                        }
                    };
                    if state.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let image = if native == size {
                        image
                    } else {
                        Arc::new(image::imageops::resize(
                            &*image,
                            size.0,
                            size.1,
                            image::imageops::FilterType::Triangle,
                        ))
                    };
                    if let Some(ready) = ready.take()
                        && ready.send(Ok(())).is_err()
                    {
                        break;
                    }
                    let changed = previous
                        .as_ref()
                        .is_none_or(|last| last.image.as_raw() != image.as_raw());
                    if changed && !state.stopped.load(Ordering::Acquire) {
                        let frame = Frame { image };
                        previous = Some(frame.clone());
                        sink(frame);
                    }
                    let interval =
                        Duration::from_secs(1) / state.fps.load(Ordering::Acquire).max(1);
                    let guard = state.wait.lock().unwrap_or_else(|e| e.into_inner());
                    if state.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let _ = state
                        .wake
                        .wait_timeout(guard, interval.saturating_sub(started.elapsed()));
                }
            })
            .map_err(|_| "无法启动软件视频采集线程")?;
        match wait.recv_timeout(timeout) {
            Ok(Ok(())) => Ok((capture, size)),
            Ok(Err(error)) => {
                capture.stop();
                Err(error)
            }
            Err(_) => {
                capture.stop();
                Err("视频采集启动超时，请核实桌面权限".into())
            }
        }
    }
    pub fn native_size(&self) -> (u32, u32) {
        self.native
    }
    pub fn error(&self) -> Option<String> {
        self.state
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn set_fps(&self, fps: u32) {
        let _guard = self.state.wait.lock().unwrap_or_else(|e| e.into_inner());
        self.state.fps.store(fps.clamp(1, 60), Ordering::Release);
        self.state.wake.notify_all();
    }
    pub fn stop(&self) {
        let _guard = self.state.wait.lock().unwrap_or_else(|e| e.into_inner());
        self.state.stopped.store(true, Ordering::Release);
        self.state.wake.notify_all();
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(any(target_os = "linux", test))]
fn is_wayland(session: Option<&str>, display: Option<&str>) -> bool {
    session.is_some_and(|s| s.eq_ignore_ascii_case("wayland"))
        || display.is_some_and(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    static CAPTURE_TEST: Mutex<()> = Mutex::new(());
    fn config() -> VideoConfig {
        VideoConfig {
            display: 0,
            max_width: 1280,
            fps: 30,
            bitrate: 800_000,
            profile: VideoProfile::ConstrainedBaseline31,
        }
    }
    fn wait_for(mut ready: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < until, "condition not reached");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn real_software_encoder_outputs_decodable_cb31_and_repeated_idr_parameters() {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let copy = frames.clone();
        let mut encoder = Encoder::with_profile(
            480,
            320,
            30,
            800_000,
            Arc::new(move |frame| copy.lock().unwrap().push(frame)),
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        for t in 0..20 {
            encoder
                .encode(
                    &Frame::synthetic(480, 320, t).unwrap(),
                    Instant::now(),
                    t == 12,
                )
                .unwrap();
            std::thread::sleep(Duration::from_millis(34));
        }
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 20);
        assert!(frames[0].keyframe && frames[12].keyframe);
        assert!(!frames[1].keyframe);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut decoded = 0;
        for frame in frames.iter() {
            validate_profile(frame, VideoProfile::ConstrainedBaseline31).unwrap();
            if let Some(image) = decoder.decode(&frame.data).unwrap() {
                use openh264::formats::YUVSource;
                assert_eq!(image.dimensions(), (480, 320));
                decoded += 1;
            }
        }
        assert_eq!(decoded, 20);
        if let Ok(path) = std::env::var("DOGCODES_SOFTWARE_VIDEO_FIXTURE") {
            let mut records = Vec::new();
            let mut annex = Vec::new();
            for f in frames.iter() {
                records.extend(u32::try_from(f.data.len()).unwrap().to_be_bytes());
                records.push(u8::from(f.keyframe));
                records.extend(&f.data);
                annex.extend(&f.data);
            }
            std::fs::write(&path, records).unwrap();
            std::fs::write(format!("{path}.h264"), annex).unwrap();
        }
    }
    #[test]
    fn live_rate_changes_reach_the_native_codec_without_recreating_the_stream() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let copy = output.clone();
        let mut encoder = Encoder::with_profile(
            480,
            320,
            30,
            2_000_000,
            Arc::new(move |f| copy.lock().unwrap().push(f)),
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        encoder
            .encode(
                &Frame::synthetic(480, 320, 0).unwrap(),
                Instant::now(),
                true,
            )
            .unwrap();
        for (n, (bitrate, fps)) in [
            (3_000_000, 30),
            (100_000, 12),
            (9_000_000, 30),
            (1_000_000, 20),
            (2_000_000, 30),
        ]
        .into_iter()
        .enumerate()
        {
            encoder.set_rate(bitrate, fps);
            std::thread::sleep(Duration::from_millis(90));
            encoder
                .encode(
                    &Frame::synthetic(480, 320, n as u32 + 1).unwrap(),
                    Instant::now(),
                    false,
                )
                .unwrap();
            let mut rate = openh264_sys2::SBitrateInfo {
                iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
                iBitrate: 0,
            };
            let mut frame_rate = 0f32;
            // SAFETY: typed, writable output values live through synchronous
            // GetOption; these reads cannot alter wrapper state or pointers.
            unsafe {
                let api = encoder.codec.raw_api();
                assert_eq!(
                    api.get_option(
                        openh264_sys2::ENCODER_OPTION_BITRATE,
                        std::ptr::from_mut(&mut rate).cast()
                    ),
                    0
                );
                assert_eq!(
                    api.get_option(
                        openh264_sys2::ENCODER_OPTION_FRAME_RATE,
                        std::ptr::from_mut(&mut frame_rate).cast()
                    ),
                    0
                );
            }
            assert_eq!(rate.iBitrate, bitrate as i32);
            assert_eq!(frame_rate, fps as f32);
            let mut cap = openh264_sys2::SBitrateInfo {
                iLayer: openh264_sys2::SPATIAL_LAYER_0,
                iBitrate: 0,
            };
            // SAFETY: a typed synchronous GetOption writes this live value.
            unsafe {
                assert_eq!(
                    encoder.codec.raw_api().get_option(
                        openh264_sys2::ENCODER_OPTION_MAX_BITRATE,
                        std::ptr::from_mut(&mut cap).cast()
                    ),
                    0
                );
            }
            assert_eq!(cap.iBitrate, (bitrate + bitrate / 2) as i32);
        }
        assert_eq!(
            output.lock().unwrap().iter().filter(|f| f.keyframe).count(),
            1
        );
    }
    #[test]
    fn encoder_rejects_invalid_or_out_of_budget_geometry_and_reports_runtime_failure() {
        let sink: Sink = Arc::new(|_| {});
        assert!(Frame::synthetic(3, 4, 0).is_err());
        assert!(
            Encoder::with_profile(
                1920,
                1080,
                30,
                1_000_000,
                sink.clone(),
                VideoProfile::ConstrainedBaseline31
            )
            .is_err()
        );
        let mut encoder = Encoder::with_profile(
            480,
            320,
            30,
            500_000,
            sink,
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        assert!(
            encoder
                .encode(
                    &Frame::synthetic(640, 480, 0).unwrap(),
                    Instant::now(),
                    true
                )
                .is_err()
        );
        assert!(encoder.error().is_some());
    }
    #[test]
    fn software_budget_is_explicit_and_wayland_is_not_polled_through_screenshot_portals() {
        let c = normalize(VideoConfig {
            profile: VideoProfile::ConstrainedBaselineAuto,
            fps: 60,
            bitrate: 50_000_000,
            ..config()
        });
        assert_eq!(c.profile, VideoProfile::ConstrainedBaseline31);
        assert_eq!((c.bitrate, c.fps), (9_000_000, 30));
        assert!(is_wayland(Some("WAYLAND"), None));
        assert!(is_wayland(None, Some("wayland-0")));
        assert!(!is_wayland(Some("x11"), Some("")));
    }
    #[test]
    fn polling_suppresses_static_frames_and_surfaces_capture_errors() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let count = Arc::new(AtomicUsize::new(0));
        let copy = count.clone();
        let mut calls = 0;
        let (capture, size) = Capture::start_source(
            config(),
            (16, 16),
            move || {
                calls += 1;
                if calls == 4 {
                    return Err("display removed".into());
                }
                Ok(RgbaImage::from_pixel(
                    16,
                    16,
                    image::Rgba([20, 30, 40, 255]),
                ))
            },
            Box::new(move |_| {
                copy.fetch_add(1, Ordering::AcqRel);
            }),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(size, (16, 16));
        wait_for(|| capture.error().is_some());
        assert_eq!(capture.error().unwrap(), "display removed");
        assert_eq!(count.load(Ordering::Acquire), 1);
        capture.stop();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
    }
    #[test]
    fn startup_timeouts_bound_stragglers_and_never_publish_a_late_frame() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let frames = Arc::new(AtomicUsize::new(0));
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (tx, rx) = mpsc::channel();
            releases.push(tx);
            let copy = frames.clone();
            let now = Instant::now();
            let result = Capture::start_source(
                config(),
                (16, 16),
                move || {
                    rx.recv().map_err(|_| "abandoned")?;
                    Ok(RgbaImage::new(16, 16))
                },
                Box::new(move |_| {
                    copy.fetch_add(1, Ordering::AcqRel);
                }),
                Duration::from_millis(40),
            );
            assert!(result.is_err());
            assert!(now.elapsed() < Duration::from_secs(1));
        }
        assert_eq!(CAPTURES.load(Ordering::Acquire), 2);
        assert!(
            Capture::start_source(
                config(),
                (16, 16),
                || panic!("must not spawn a third blocked source"),
                Box::new(|_| {}),
                Duration::from_millis(40)
            )
            .is_err()
        );
        for tx in releases {
            tx.send(()).unwrap();
        }
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        assert_eq!(frames.load(Ordering::Acquire), 0);
    }
    #[test]
    fn closing_does_not_wait_for_a_blocked_os_call_or_deliver_its_frame() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut n = 0;
        let frames = Arc::new(AtomicUsize::new(0));
        let copy = frames.clone();
        let (capture, _) = Capture::start_source(
            config(),
            (16, 16),
            move || {
                n += 1;
                if n > 1 {
                    let _ = entered_tx.send(());
                    release_rx.recv().map_err(|_| "abandoned")?;
                }
                Ok(RgbaImage::from_pixel(16, 16, image::Rgba([n, 0, 0, 255])))
            },
            Box::new(move |_| {
                copy.fetch_add(1, Ordering::AcqRel);
            }),
            Duration::from_secs(1),
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let now = Instant::now();
        capture.stop();
        assert!(now.elapsed() < Duration::from_millis(100));
        release_tx.send(()).unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        assert_eq!(frames.load(Ordering::Acquire), 1);
    }
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the isolated Docker/Xvfb fixture"]
    fn isolated_x11_capture_encodes_real_owned_display_changes() {
        assert_eq!(
            std::env::var("DOGCODES_OWNED_XVFB").as_deref(),
            Ok("video-fallback-fixture-v1")
        );
        assert_eq!(std::env::var("DISPLAY").as_deref(), Ok(":99"));
        assert_eq!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("x11"));
        let _serial = CAPTURE_TEST.lock().unwrap();
        let monitor = xcap::Monitor::all()
            .unwrap()
            .into_iter()
            .next()
            .expect("owned display");
        let native = (monitor.width().unwrap(), monitor.height().unwrap());
        let cfg = normalize(VideoConfig {
            display: monitor.id().unwrap(),
            ..config()
        });
        let size = profile_size(native.0, native.1, cfg.max_width, cfg.profile).unwrap();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let encoder = Arc::new(Mutex::new(
            Encoder::with_profile(
                size.0,
                size.1,
                cfg.fps,
                cfg.bitrate,
                Arc::new(move |f| output.lock().unwrap().push(f)),
                cfg.profile,
            )
            .unwrap(),
        ));
        let (capture, actual) = Capture::start(
            cfg,
            Box::new(move |frame| {
                encoder
                    .lock()
                    .unwrap()
                    .encode(&frame, Instant::now(), false)
                    .unwrap();
            }),
        )
        .unwrap();
        assert_eq!(actual, size);
        wait_for(|| !frames.lock().unwrap().is_empty());
        let count = frames.lock().unwrap().len();
        assert!(
            std::process::Command::new("xsetroot")
                .args(["-solid", "#335588"])
                .status()
                .unwrap()
                .success()
        );
        wait_for(|| frames.lock().unwrap().len() > count);
        capture.stop();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let frames = frames.lock().unwrap();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        for f in frames.iter() {
            validate_profile(f, cfg.profile).unwrap();
            let image = decoder.decode(&f.data).unwrap().unwrap();
            use openh264::formats::YUVSource;
            assert_eq!(image.dimensions(), (size.0 as usize, size.1 as usize));
        }
        println!(
            "owned X11 capture: {native:?} -> CB3.1 {size:?}, {} decoded change frames",
            frames.len()
        );
    }
}
