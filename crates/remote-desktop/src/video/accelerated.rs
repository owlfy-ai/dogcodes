//! Shared hardware-first selection; expired native output never owns the sink.
pub(super) use super::software::Frame;
use super::{EncodedFrame, Result, Sink, VideoProfile, worker::Worker};
#[cfg(all(test, not(target_os = "windows")))]
use std::time::Duration;
use std::time::Instant;
pub(super) type Input = (Frame, Instant, bool, (u32, u32));
pub(super) struct Encoder {
    hardware: Option<Worker<Input, EncodedFrame>>,
    software: Option<super::software::Encoder>,
    size: (u32, u32),
    rate: (u32, u32),
    profile: VideoProfile,
    sink: Sink,
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
        if width < 2
            || height < 2
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || width.max(height) > 3840
            || width.min(height) > 2160
        {
            return Err("系统视频尺寸无效".into());
        }
        let size = (width, height);
        let rate = profile.rate(bitrate, fps);
        #[cfg(target_os = "windows")]
        let hardware = super::windows::hardware_attempt(size, rate, profile);
        #[cfg(target_os = "linux")]
        let hardware = super::linux::hardware_attempt(size, rate, profile);
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let hardware = Err("当前平台未使用此系统编码器".into());
        Self::from_attempt(size, rate, sink, profile, hardware)
    }
    pub(super) fn from_attempt(
        size: (u32, u32),
        rate: (u32, u32),
        sink: Sink,
        profile: VideoProfile,
        hardware: Result<Worker<Input, EncodedFrame>>,
    ) -> Result<Self> {
        let mut encoder = Self {
            hardware: None,
            software: None,
            size,
            rate,
            profile,
            sink,
        };
        match hardware {
            Ok(hardware) => encoder.hardware = Some(hardware),
            Err(error) => {
                eprintln!("remote video: {error}; using source-built software encoder");
                encoder.use_software()?;
            }
        }
        Ok(encoder)
    }
    fn use_software(&mut self) -> Result<()> {
        self.hardware.take();
        self.software = Some(if self.profile == VideoProfile::ConstrainedBaselineAuto {
            super::software::Encoder::new(
                self.size.0,
                self.size.1,
                self.rate.1,
                self.rate.0,
                self.sink.clone(),
            )?
        } else {
            super::software::Encoder::with_profile(
                self.size.0,
                self.size.1,
                self.rate.1,
                self.rate.0,
                self.sink.clone(),
                self.profile,
            )?
        });
        Ok(())
    }
    pub fn size(&self) -> (u32, u32) {
        self.software
            .as_ref()
            .map_or(self.size, super::software::Encoder::size)
    }
    pub fn can_encode(&self) -> bool {
        // Native worker completion is synchronous; keep fallback credit tied
        // to its own backend rather than bypassing its admission contract.
        self.software
            .as_ref()
            .is_none_or(super::software::Encoder::can_encode)
    }
    pub fn error(&self) -> Option<String> {
        self.software
            .as_ref()
            .and_then(super::software::Encoder::error)
    }
    pub fn set_rate(&mut self, bitrate: u32, fps: u32) {
        self.rate = self.profile.rate(bitrate, fps);
        if let Some(software) = &mut self.software {
            software.set_rate(self.rate.0, self.rate.1);
        }
    }
    pub fn encode(&mut self, frame: &Frame, captured: Instant, force: bool) -> Result<bool> {
        if frame.size() != self.size {
            return Err("显示器尺寸变化，请重新建立视频连接".into());
        }
        if let Some(hardware) = &mut self.hardware {
            match hardware.run((frame.clone(), captured, force, self.rate)) {
                Ok(mut output) => {
                    output.cursor_embedded = frame.cursor_embedded;
                    (self.sink)(output);
                    return Ok(true);
                }
                Err(error) => {
                    eprintln!("remote video: {error}; switching to software IDR");
                    self.use_software()?;
                }
            }
        }
        // New fallback has a real codec warmup and always begins with its own
        // SPS/PPS/IDR. No timed-out worker owns this sink or can send old frames.
        self.software
            .as_mut()
            .ok_or("视频编码器未就绪")?
            .encode(frame, captured, force)
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod portable_tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    fn target() -> (Sink, Arc<Mutex<Vec<EncodedFrame>>>) {
        let frames = Arc::new(Mutex::new(vec![]));
        let observed = frames.clone();
        (
            Arc::new(move |frame| observed.lock().unwrap().push(frame)),
            frames,
        )
    }
    fn decoded(frames: &[EncodedFrame]) {
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        for frame in frames {
            super::super::validate_profile(frame, VideoProfile::ConstrainedBaseline31).unwrap();
            use openh264::formats::YUVSource;
            assert_eq!(
                decoder.decode(&frame.data).unwrap().unwrap().dimensions(),
                (32, 32)
            );
        }
    }
    #[test]
    fn unavailable_native_initialization_starts_a_real_decodable_software_stream() {
        let (sink, frames) = target();
        let mut encoder = Encoder::from_attempt(
            (32, 32),
            (1_000_000, 30),
            sink,
            VideoProfile::ConstrainedBaseline31,
            Err("system DLL absent".into()),
        )
        .unwrap();
        encoder
            .encode(&Frame::synthetic(32, 32, 0).unwrap(), Instant::now(), false)
            .unwrap();
        assert!(encoder.hardware.is_none());
        assert!(encoder.software.is_some());
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].keyframe);
        decoded(&frames);
    }
    #[test]
    fn hardware_failure_switches_once_to_software_sps_pps_idr_and_keeps_live_rate_control() {
        let _serial = super::super::worker::isolated();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let worker = Worker::start(
            || {
                let (sink, frames) = target();
                let codec = super::super::software::Encoder::with_profile(
                    32,
                    32,
                    30,
                    1_000_000,
                    sink,
                    VideoProfile::ConstrainedBaseline31,
                )?;
                Ok((codec, frames))
            },
            move |(codec, frames), (frame, captured, force, _): Input| {
                if observed.fetch_add(1, Ordering::AcqRel) > 0 {
                    return Err("device removed".into());
                }
                codec.encode(&frame, captured, force)?;
                frames
                    .lock()
                    .unwrap()
                    .pop()
                    .ok_or("mock native frame missing".into())
            },
            Duration::from_secs(1),
        )
        .unwrap();
        let (sink, frames) = target();
        let mut encoder = Encoder::from_attempt(
            (32, 32),
            (1_000_000, 30),
            sink,
            VideoProfile::ConstrainedBaseline31,
            Ok(worker),
        )
        .unwrap();
        encoder
            .encode(&Frame::synthetic(32, 32, 0).unwrap(), Instant::now(), true)
            .unwrap();
        encoder
            .encode(&Frame::synthetic(32, 32, 1).unwrap(), Instant::now(), false)
            .unwrap();
        encoder.set_rate(2_000_000, 20);
        encoder
            .encode(&Frame::synthetic(32, 32, 2).unwrap(), Instant::now(), false)
            .unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 2);
        assert!(encoder.hardware.is_none());
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames[0].keyframe);
        assert!(frames[1].keyframe);
        decoded(&frames);
    }
    #[test]
    fn a_timed_out_hardware_frame_cannot_publish_after_software_takes_over() {
        let _serial = super::super::worker::isolated();
        struct Closed(Arc<AtomicBool>);
        impl Drop for Closed {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let ended = Arc::new(AtomicBool::new(false));
        let observed = ended.clone();
        let (release, blocked) = mpsc::channel();
        let worker = Worker::start(
            move || Ok(Closed(observed)),
            move |_, _: Input| {
                blocked.recv().unwrap();
                Ok(EncodedFrame {
                    cursor_embedded: None,
                    captured: std::time::Instant::now(),
                    data: vec![99],
                    keyframe: false,
                    latency: Duration::ZERO,
                })
            },
            Duration::from_secs(1),
        )
        .unwrap();
        let (sink, frames) = target();
        let mut encoder = Encoder::from_attempt(
            (32, 32),
            (1_000_000, 30),
            sink,
            VideoProfile::ConstrainedBaseline31,
            Ok(worker),
        )
        .unwrap();
        encoder
            .encode(&Frame::synthetic(32, 32, 0).unwrap(), Instant::now(), false)
            .unwrap();
        assert_eq!(frames.lock().unwrap().len(), 1);
        release.send(()).unwrap();
        for _ in 0..1000 {
            if ended.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(ended.load(Ordering::Acquire));
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].keyframe);
        decoded(&frames);
    }
}
