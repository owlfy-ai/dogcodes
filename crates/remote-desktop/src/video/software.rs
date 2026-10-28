//! Bounded CPU H.264 fallback for Windows and Linux. Native screenshots or an
//! authorized continuous Wayland source feed the same bounded capture loop.
//! Windows and Linux select their hardware backends above this fallback; capture
//! and the authorized source remain shared.
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

/// Windows native/fallback and Linux share the negotiated CB3.1 budget on all transports.
/// Higher-resolution accelerated backends must negotiate their own budget.
pub(super) fn normalize(mut config: VideoConfig) -> VideoConfig {
    config.profile = VideoProfile::ConstrainedBaseline31;
    (config.bitrate, config.fps) = config.profile.rate(config.bitrate, config.fps);
    config
}
#[derive(Clone)]
pub(super) struct Frame {
    pub(super) cursor_embedded: Option<bool>,
    pub(super) image: Arc<RgbaImage>,
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
            cursor_embedded: Some(false),
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
            cursor_embedded: None,
            captured: std::time::Instant::now(),
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
    pub fn can_encode(&self) -> bool {
        true // This codec completes synchronously under Shared's encoder lock.
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
            cursor_embedded: frame.cursor_embedded,
            captured,
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
    #[cfg(all(test, target_os = "linux"))]
    pub fn start(
        config: VideoConfig,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
    ) -> Result<(Self, (u32, u32))> {
        Self::start_with_geometry(config, sink, Arc::new(AtomicBool::new(false)))
    }
    #[cfg(not(target_os = "windows"))]
    pub fn start_with_geometry(
        config: VideoConfig,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        geometry_changed: Arc<AtomicBool>,
    ) -> Result<(Self, (u32, u32))> {
        Self::start_monitor(
            config,
            sink,
            geometry_changed,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }
    #[cfg(target_os = "windows")]
    pub fn start_windows(
        config: VideoConfig,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        geometry_changed: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        retired: Arc<AtomicBool>,
    ) -> Result<(Self, (u32, u32))> {
        Self::start_monitor(config, sink, geometry_changed, paused, cancelled, retired)
    }
    fn start_monitor(
        config: VideoConfig,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        geometry_changed: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        #[cfg(target_os = "windows")] retired: Arc<AtomicBool>,
    ) -> Result<(Self, (u32, u32))> {
        #[cfg(not(target_os = "windows"))]
        let _ = cancelled;
        #[cfg(target_os = "windows")]
        let cancelled = super::capture_source::Cancellation::new(cancelled, retired);
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
        #[cfg(target_os = "windows")]
        let pause_source = paused.clone();
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
                #[cfg(not(target_os = "windows"))]
                {
                    Ok(move || {
                        monitor
                            .capture_image()
                            .map(|image| Frame {
                                image: Arc::new(image),
                                cursor_embedded: Some(false),
                            })
                            .map_err(|_| "视频采集失败，请检查显示器和桌面权限".into())
                    })
                }
                #[cfg(target_os = "windows")]
                {
                    let name = monitor.name().map_err(|_| "无法核实视频显示器身份")?;
                    Ok(super::dxgi::capture(display, name, cancelled, pause_source))
                }
            },
            sink,
            if cfg!(target_os = "windows") {
                Duration::from_secs(6)
            } else {
                Duration::from_secs(3)
            },
            geometry_changed,
            paused,
        )
    }

    #[cfg(target_os = "linux")]
    pub fn from_source(
        config: VideoConfig,
        source: Arc<dyn super::ScreenSource>,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        geometry_changed: Arc<AtomicBool>,
    ) -> Result<(Self, (u32, u32))> {
        let native = source.size()?;
        Self::start_factory(
            config,
            native,
            move || {
                Ok(move || {
                    let image = source.frame()?;
                    Ok(Frame {
                        image,
                        cursor_embedded: Some(source.cursor_embedded()),
                    })
                })
            },
            sink,
            Duration::from_secs(3),
            geometry_changed,
            Arc::new(AtomicBool::new(false)),
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
            move || {
                Ok(move || {
                    source().map(|image| Frame {
                        image: Arc::new(image),
                        cursor_embedded: Some(false),
                    })
                })
            },
            sink,
            timeout,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }
    fn start_factory<F, S>(
        config: VideoConfig,
        native: (u32, u32),
        factory: F,
        sink: Box<dyn Fn(Frame) + Send + Sync>,
        timeout: Duration,
        geometry_changed: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
    ) -> Result<(Self, (u32, u32))>
    where
        F: FnOnce() -> Result<S> + Send + 'static,
        S: FnMut() -> Result<Frame> + 'static,
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
                let mut previous_source: Option<Arc<RgbaImage>> = None;
                while !state.stopped.load(Ordering::Acquire) {
                    let started = Instant::now();
                    let frame = match source() {
                        Ok(frame) => frame,
                        Err(error) => {
                            *state.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error.clone());
                            if let Some(ready) = ready.take() {
                                let _ = ready.send(Err(error));
                            }
                            break;
                        }
                    };
                    let Frame {
                        image,
                        cursor_embedded,
                    } = frame;
                    if state.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if image.dimensions() != native {
                        // Keep the original source grant. No new-size pixels
                        // reach the old encoder, including its refresh ticker.
                        geometry_changed.store(true, Ordering::Release);
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(Ok(()));
                        }
                        break;
                    }
                    // Immutable shared pixels have not changed. Skip resize and
                    // byte comparison, while still fetching/checking the source.
                    if previous_source
                        .as_ref()
                        .is_none_or(|last| !Arc::ptr_eq(last, &image))
                        || previous
                            .as_ref()
                            .is_some_and(|last| last.cursor_embedded != cursor_embedded)
                        || paused.load(Ordering::Acquire)
                    {
                        previous_source = Some(image.clone());
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
                        let changed = previous.as_ref().is_none_or(|last| {
                            last.image.as_raw() != image.as_raw()
                                || last.cursor_embedded != cursor_embedded
                        }) || paused.load(Ordering::Acquire);
                        if changed && !state.stopped.load(Ordering::Acquire) {
                            let frame = Frame {
                                image,
                                cursor_embedded,
                            };
                            previous = Some(frame.clone());
                            sink(frame);
                        }
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
    #[test]
    fn unchanged_pixels_still_deliver_pointer_mode_changes_through_scaling() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let image = Arc::new(RgbaImage::new(16, 8));
        let (capture, size) = Capture::start_factory(
            VideoConfig {
                max_width: 8,
                ..config()
            },
            (16, 8),
            move || {
                let mut modes = [None, Some(true), Some(false)].into_iter();
                Ok(move || {
                    modes
                        .next()
                        .map(|cursor_embedded| Frame {
                            image: image.clone(),
                            cursor_embedded,
                        })
                        .ok_or_else(|| "fixture done".into())
                })
            },
            Box::new(move |frame| {
                output
                    .lock()
                    .unwrap()
                    .push((frame.size(), frame.cursor_embedded))
            }),
            Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(size, (8, 4));
        wait_for(|| capture.error().is_some());
        capture.stop();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        assert_eq!(
            *frames.lock().unwrap(),
            vec![((8, 4), None), ((8, 4), Some(true)), ((8, 4), Some(false))]
        );
    }
    #[test]
    fn encoded_pointer_metadata_belongs_to_the_original_input_frame() {
        use openh264::formats::YUVSource;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = frames.clone();
        let mut encoder = Encoder::new(
            64,
            32,
            30,
            800_000,
            Arc::new(move |frame| output.lock().unwrap().push(frame)),
        )
        .unwrap();
        for cursor_embedded in [None, Some(true), Some(false)] {
            let frame = Frame {
                cursor_embedded,
                ..Frame::synthetic(64, 32, 0).unwrap()
            };
            assert!(encoder.encode(&frame, Instant::now(), true).unwrap());
        }
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let frames = frames.lock().unwrap();
        assert_eq!(
            frames.iter().map(|f| f.cursor_embedded).collect::<Vec<_>>(),
            [None, Some(true), Some(false)]
        );
        for frame in frames.iter() {
            assert_eq!(
                decoder.decode(&frame.data).unwrap().unwrap().dimensions(),
                (64, 32)
            );
        }
    }
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
    #[ignore = "paced synthetic static-text quality measurement, no desktop capture"]
    fn measure_static_text_refinement_with_real_encoder_and_decoder() {
        use openh264::formats::YUVSource;
        for (bitrate, refresh_idr, bounded) in [
            (400_000, false, false),
            (400_000, true, false),
            (400_000, false, true),
            (1_000_000, false, false),
            (1_000_000, true, false),
            (1_000_000, false, true),
        ] {
            let (send, receive) = mpsc::channel();
            let mut encoder = Encoder::with_profile(
                1280,
                720,
                30,
                bitrate,
                Arc::new(move |frame| send.send(frame).unwrap()),
                VideoProfile::ConstrainedBaseline31,
            )
            .unwrap();
            let mut decoder = openh264::decoder::Decoder::new().unwrap();
            for t in 0..30 {
                let tick = Instant::now();
                if encoder
                    .encode(&Frame::synthetic(1280, 720, t).unwrap(), tick, false)
                    .unwrap()
                {
                    decoder.decode(&receive.recv().unwrap().data).unwrap();
                }
                std::thread::sleep(Duration::from_millis(34).saturating_sub(tick.elapsed()));
            }
            let frame = Frame::synthetic(1280, 720, 30).unwrap();
            let mut original = YUVBuffer::new(1280, 720);
            original.read_rgba8(RgbaSliceU8::new(frame.image.as_raw(), (1280, 720)));
            let mut policy = super::super::refresh::Refresh::new();
            let mut progress = super::super::refresh::Output::default();
            for repeat in 0..=24 {
                let tick = Instant::now();
                if repeat == 0 {
                    policy.changed(tick);
                } else if bounded && policy.tick(tick, progress, false, 30).unwrap().is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                policy.submitting(tick, progress, repeat == 1 && refresh_idr);
                if encoder
                    .encode(&frame, tick, repeat == 1 && refresh_idr)
                    .unwrap()
                {
                    let encoded = receive.recv().unwrap();
                    progress.emitted(encoded.data.len(), encoded.keyframe);
                    let image = decoder.decode(&encoded.data).unwrap().unwrap();
                    let stride = image.strides().0;
                    let squared: u64 = image
                        .y()
                        .chunks(stride)
                        .take(720)
                        .zip(original.y().chunks(1280))
                        .map(|(actual, expected)| {
                            actual[..1280]
                                .iter()
                                .zip(expected)
                                .map(|(a, b)| (i64::from(*a) - i64::from(*b)).pow(2) as u64)
                                .sum::<u64>()
                        })
                        .sum();
                    let mse = squared as f64 / (1280.0 * 720.0);
                    let psnr = 10.0 * (255.0 * 255.0 / mse.max(0.0001)).log10();
                    println!(
                        "static bounded={bounded} idr={refresh_idr} bitrate={bitrate} repeat={repeat} psnr={psnr:.2} bytes={} encode_us={}",
                        encoded.data.len(),
                        encoded.latency.as_micros()
                    );
                } else {
                    println!(
                        "static bounded={bounded} idr={refresh_idr} bitrate={bitrate} repeat={repeat} skipped"
                    );
                }
                let interval = if repeat == 0 { 400 } else { 100 };
                std::thread::sleep(Duration::from_millis(interval).saturating_sub(tick.elapsed()));
            }
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
    fn changed_geometry_retires_capture_without_delivering_new_size_or_a_terminal_error() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let changed = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let sent = frames.clone();
        let (release, released) = mpsc::channel();
        let (capture, size) = Capture::start_factory(
            config(),
            (16, 8),
            move || {
                let mut first = true;
                Ok(move || {
                    if first {
                        first = false;
                        Ok(Frame {
                            image: Arc::new(RgbaImage::new(16, 8)),
                            cursor_embedded: Some(false),
                        })
                    } else {
                        released.recv().map_err(|_| "test release closed")?;
                        Ok(Frame {
                            image: Arc::new(RgbaImage::new(8, 16)),
                            cursor_embedded: Some(false),
                        })
                    }
                })
            },
            Box::new(move |frame| sent.lock().unwrap().push(frame.size())),
            Duration::from_secs(1),
            changed.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(size, (16, 8));
        release.send(()).unwrap();
        wait_for(|| changed.load(Ordering::Acquire));
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        assert_eq!(*frames.lock().unwrap(), vec![(16, 8)]);
        assert!(capture.error().is_none());
    }
    #[test]
    fn recovery_delivers_an_identical_cached_picture_before_resuming_publication() {
        let _serial = CAPTURE_TEST.lock().unwrap();
        wait_for(|| CAPTURES.load(Ordering::Acquire) == 0);
        let paused = Arc::new(AtomicBool::new(false));
        let source_pause = paused.clone();
        let delivery_pause = paused.clone();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let delivered = frames.clone();
        let (capture, _) = Capture::start_factory(
            config(),
            (16, 8),
            move || {
                let image = Arc::new(RgbaImage::new(16, 8));
                let mut n = 0;
                Ok(move || {
                    n += 1;
                    if n == 3 {
                        return Err("owned source ended".into());
                    }
                    if n == 2 {
                        source_pause.store(true, Ordering::Release);
                    }
                    Ok(Frame {
                        image: image.clone(),
                        cursor_embedded: Some(false),
                    })
                })
            },
            Box::new(move |frame| {
                delivered.lock().unwrap().push((
                    frame.image.as_raw().clone(),
                    delivery_pause.load(Ordering::Acquire),
                ));
                delivery_pause.store(false, Ordering::Release);
            }),
            Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
            paused.clone(),
        )
        .unwrap();
        wait_for(|| capture.error().is_some());
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, frames[1].0);
        assert!(!frames[0].1);
        assert!(
            frames[1].1,
            "resumed before delivery installed the fallback image"
        );
        assert!(!paused.load(Ordering::Acquire));
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
