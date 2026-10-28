//! Bounded Linux hardware encoding through the distribution's GStreamer ABI.
//! No runtime downloads, shell pipelines, display capture or hardware guessing.
use super::{
    EncodedFrame, Result, VideoProfile, accelerated::Input, h264::Headers, software::Frame,
    worker::Worker,
};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSrc};
use std::time::{Duration, Instant};

const MAX_FRAME: usize = 8 * 1024 * 1024;
const CANDIDATES: &[Spec] = &[
    Spec {
        factory: "nvh264enc",
        bitrate_unit: 1000,
        raw_format: "NV12",
        settings: "rc-mode=cbr bframes=0 zerolatency=true cabac=false aud=true gop-size=600",
    },
    Spec {
        factory: "vah264enc",
        bitrate_unit: 1000,
        raw_format: "NV12",
        settings: "rate-control=cbr b-frames=0 cabac=false dct8x8=false ref-frames=1 aud=true key-int-max=600",
    },
    Spec {
        factory: "vaapih264enc",
        bitrate_unit: 1000,
        raw_format: "NV12",
        settings: "rate-control=cbr max-bframes=0 cabac=false dct8x8=false",
    },
];
#[derive(Clone, Copy)]
struct Spec {
    factory: &'static str,
    settings: &'static str,
    bitrate_unit: u32,
    raw_format: &'static str,
}

pub(super) fn hardware_attempt(
    size: (u32, u32),
    rate: (u32, u32),
    profile: VideoProfile,
) -> Result<Worker<Input, EncodedFrame>> {
    // Default tests never enumerate the development machine's GPU. The opt-in
    // tests below use only an explicitly selected synthetic software element.
    #[cfg(test)]
    if std::env::var("DOGCODES_LINUX_ENCODER_FIXTURE").as_deref() != Ok("hardware-synthetic-only") {
        return Err("硬件编码实机验证尚未启用".into());
    }
    Worker::start_with_deadlines(
        move || Native::new(size, rate, profile),
        |native, (frame, captured, force, rate): Input| {
            native.encode(&frame, captured, force, rate)
        },
        Duration::from_secs(2),
        Duration::from_millis(500),
    )
}

struct Native {
    stream: Stream,
    spec: Spec,
    size: (u32, u32),
    rate: (u32, u32),
    profile: VideoProfile,
    first: bool,
}
impl Native {
    fn new(size: (u32, u32), rate: (u32, u32), profile: VideoProfile) -> Result<Self> {
        gst::init().map_err(|e| format!("Linux 编码组件无法初始化：{e}"))?;
        let mut failures = Vec::new();
        for spec in CANDIDATES {
            let Some(factory) = gst::ElementFactory::find(spec.factory) else {
                continue;
            };
            if !factory
                .metadata("klass")
                .is_some_and(|v| v.contains("Hardware"))
            {
                continue;
            }
            match Self::open(*spec, size, rate, profile) {
                Ok(native) => return Ok(native),
                Err(error) => failures.push(format!("{}: {error}", spec.factory)),
            }
        }
        Err(if failures.is_empty() {
            "没有可用的 Linux H.264 硬件插件".into()
        } else {
            format!("Linux 硬件试编码失败：{}", failures.join("; "))
        })
    }
    fn open(spec: Spec, size: (u32, u32), rate: (u32, u32), profile: VideoProfile) -> Result<Self> {
        let rate = profile.rate(rate.0, rate.1);
        let stream = Stream::open(spec, size, rate, profile)?;
        let mut native = Self {
            stream,
            spec,
            size,
            rate,
            profile,
            first: true,
        };
        let probe = Frame::synthetic(size.0, size.1, 0)?;
        let frame =
            native
                .stream
                .encode(&probe, Instant::now(), false, Duration::from_millis(500))?;
        if !frame.keyframe {
            return Err("硬件试编码没有返回 IDR".into());
        }
        verify_probe(&frame, size)?;
        // Warmup proves the real negotiated bitstream and is never published.
        Ok(native)
    }
    fn encode(
        &mut self,
        frame: &Frame,
        captured: Instant,
        force: bool,
        rate: (u32, u32),
    ) -> Result<EncodedFrame> {
        if frame.size() != self.size {
            return Err("Linux 编码画面尺寸已变化".into());
        }
        let rate = self.profile.rate(rate.0, rate.1);
        if rate != self.rate {
            // Drivers differ in which properties are mutable while PLAYING.
            // Rebuild the same backend on its owned worker, validate a private
            // warmup and begin the new configuration with headers + an IDR.
            self.stream
                .pipeline
                .set_state(gst::State::Null)
                .map_err(|_| "旧 Linux 编码器未停止")?;
            *self = Self::open(self.spec, self.size, rate, self.profile)?;
        }
        let output = self.stream.encode(
            frame,
            captured,
            force || self.first,
            Duration::from_millis(200),
        )?;
        self.first = false;
        Ok(output)
    }
}
fn verify_probe(frame: &EncodedFrame, size: (u32, u32)) -> Result<()> {
    use openh264::formats::YUVSource;
    if !frame.keyframe {
        return Err("试编码不是关键帧".into());
    }
    let mut decoder = openh264::decoder::Decoder::new().map_err(|_| "无法验证硬件画面")?;
    let decoded = decoder
        .decode(&frame.data)
        .map_err(|_| "硬件试编码无法解码")?
        .ok_or("硬件试编码未产生可显示画面")?;
    if decoded.dimensions() != (size.0 as usize, size.1 as usize) {
        return Err("硬件试编码尺寸不匹配".into());
    }
    Ok(())
}
// GStreamer owns this immutable capture reference until its last memory ref
// is released, including asynchronous conversion and native teardown.
struct InputPixels(std::sync::Arc<image::RgbaImage>);
impl AsRef<[u8]> for InputPixels {
    fn as_ref(&self) -> &[u8] {
        self.0.as_raw()
    }
}
fn input_buffer(frame: &Frame) -> gst::Buffer {
    gst::Buffer::from_slice(InputPixels(frame.image.clone()))
}

struct Stream {
    pipeline: gst::Pipeline,
    source: AppSrc,
    target: AppSink,
    encoder: gst::Element,
    bus: gst::Bus,
    headers: Headers,
    next_pts: u64,
    duration: u64,
    size: (u32, u32),
    profile: VideoProfile,
}
impl Stream {
    fn open(spec: Spec, size: (u32, u32), rate: (u32, u32), profile: VideoProfile) -> Result<Self> {
        if super::profile_size(size.0, size.1, size.0, profile)? != size {
            return Err("Linux 硬编尺寸超出协商范围".into());
        }
        // All interpolated text is from our literal candidate table or bounded
        // integers; neither remote input nor environment supplies a pipeline.
        let level = if profile == VideoProfile::ConstrainedBaseline31 {
            ",level=(string)3.1"
        } else {
            ""
        };
        let description = format!(
            "appsrc name=source is-live=true format=time block=false max-buffers=1 max-bytes={} \
             caps=video/x-raw,format=RGBA,width={},height={},framerate={}/1 ! \
             videoconvert ! video/x-raw,format={},colorimetry=bt601 ! {} name=encoder {} bitrate={} ! \
             h264parse config-interval=-1 ! video/x-h264,profile=constrained-baseline,stream-format=byte-stream,alignment=au{} ! \
             appsink name=target sync=false max-buffers=1 drop=false enable-last-sample=false",
            u64::from(size.0) * u64::from(size.1) * 4,
            size.0,
            size.1,
            rate.1,
            spec.raw_format,
            spec.factory,
            spec.settings,
            rate.0.div_ceil(spec.bitrate_unit),
            level,
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|e| format!("Linux 编码管线不可用：{e}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "Linux 编码对象类型无效")?;
        let source = pipeline
            .by_name("source")
            .and_then(|v| v.downcast::<AppSrc>().ok())
            .ok_or("缺少视频输入")?;
        let target = pipeline
            .by_name("target")
            .and_then(|v| v.downcast::<AppSink>().ok())
            .ok_or("缺少视频输出")?;
        let encoder = pipeline.by_name("encoder").ok_or("缺少硬件编码器")?;
        let bus = pipeline.bus().ok_or("缺少编码状态通道")?;
        let stream = Self {
            pipeline,
            source,
            target,
            encoder,
            bus,
            headers: Headers::default(),
            next_pts: 0,
            duration: 1_000_000_000 / u64::from(rate.1),
            size,
            profile,
        };
        stream
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "Linux 编码管线无法启动")?;
        Ok(stream)
    }
    fn encode(
        &mut self,
        frame: &Frame,
        captured: Instant,
        force: bool,
        timeout: Duration,
    ) -> Result<EncodedFrame> {
        let pts = gst::ClockTime::from_nseconds(self.next_pts);
        self.next_pts = self
            .next_pts
            .checked_add(self.duration)
            .ok_or("Linux 视频时间戳耗尽")?;
        if frame.size() != self.size {
            return Err("Linux 视频尺寸不匹配".into());
        }
        // At most one input is in flight. A surplus sample or queued buffer
        // must not be attached to this request's capture timestamp.
        if self.source.current_level_buffers() != 0
            || self.target.try_pull_sample(gst::ClockTime::ZERO).is_some()
        {
            return Err("Linux 编码残留了旧画面，停止使用原编码器".into());
        }
        if force {
            let event = gstreamer_video::DownstreamForceKeyUnitEvent::builder()
                .running_time(pts)
                .all_headers(true)
                .build();
            if !self
                .encoder
                .static_pad("sink")
                .is_some_and(|pad| pad.send_event(event))
            {
                return Err("Linux 编码器拒绝关键帧请求".into());
            }
        }
        let mut buffer = input_buffer(frame);
        let buffer_mut = buffer.get_mut().ok_or("Linux 视频缓冲区不可写")?;
        buffer_mut.set_pts(pts);
        buffer_mut.set_duration(gst::ClockTime::from_nseconds(self.duration));
        self.source
            .push_buffer(buffer)
            .map_err(|_| "Linux 编码输入已关闭")?;
        let sample = self
            .target
            .try_pull_sample(gst::ClockTime::from_nseconds(timeout.as_nanos() as u64))
            .ok_or_else(|| self.failure())?;
        self.sample_frame(&sample, pts, captured, force)
    }
    fn sample_frame(
        &mut self,
        sample: &gst::Sample,
        pts: gst::ClockTime,
        captured: Instant,
        force: bool,
    ) -> Result<EncodedFrame> {
        let buffer = sample.buffer().ok_or("Linux 编码没有画面数据")?;
        if buffer.pts() != Some(pts) || buffer.size() == 0 || buffer.size() > MAX_FRAME {
            return Err("Linux 编码时间戳或画面大小无效".into());
        }
        let mapped = buffer
            .map_readable()
            .map_err(|_| "Linux 编码画面无法读取")?;
        let output = self
            .headers
            .frame(mapped.as_slice(), self.profile, captured)?;
        if force && !output.keyframe {
            return Err("Linux 编码器未返回请求的 IDR".into());
        }
        Ok(output)
    }
    fn failure(&self) -> String {
        if let Some(message) = self.bus.pop_filtered(&[gst::MessageType::Error])
            && let gst::MessageView::Error(error) = message.view()
        {
            return format!("Linux 编码失败：{}", error.error());
        }
        "Linux 编码未按时返回画面".into()
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // The worker retains its bounded permit until native destruction ends;
        // callers never join a driver stuck in this transition.
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOFTWARE: Spec = Spec {
        factory: "openh264enc",
        bitrate_unit: 1,
        raw_format: "I420",
        settings: "rate-control=bitrate complexity=low usage-type=screen gop-size=600",
    };
    #[test]
    fn input_pixels_are_shared_read_only_and_outlive_the_capture_owner() {
        gst::init().unwrap();
        let frame = Frame::synthetic(64, 32, 0).unwrap();
        let pointer = frame.image.as_raw().as_ptr();
        let owned = std::sync::Arc::downgrade(&frame.image);
        let mut buffer = input_buffer(&frame);
        buffer
            .get_mut()
            .unwrap()
            .set_pts(gst::ClockTime::from_mseconds(40));
        drop(frame);
        assert!(
            owned.upgrade().is_some(),
            "native input did not retain original pixels"
        );
        assert_eq!(
            buffer.map_readable().unwrap().as_ptr(),
            pointer,
            "native input copied the capture instead of sharing it"
        );
        assert!(
            buffer
                .peek_memory(0)
                .flags()
                .contains(gst::MemoryFlags::READONLY)
        );
        assert_eq!(buffer.pts(), Some(gst::ClockTime::from_mseconds(40)));
        let retained = buffer.clone();
        drop(buffer);
        assert!(owned.upgrade().is_some());
        drop(retained);
        assert!(
            owned.upgrade().is_none(),
            "finished native input leaked its pixels"
        );
    }

    #[test]
    fn a_writable_native_map_copies_instead_of_modifying_the_shared_capture() {
        gst::init().unwrap();
        let frame = Frame::synthetic(64, 32, 0).unwrap();
        let original = frame.image.as_raw()[0];
        let mut buffer = input_buffer(&frame);
        assert!(
            buffer
                .peek_memory(0)
                .flags()
                .contains(gst::MemoryFlags::READONLY)
        );
        {
            let mut mapped = buffer.get_mut().unwrap().map_writable().unwrap();
            mapped[0] = original.wrapping_add(1);
            assert_ne!(mapped.as_ptr(), frame.image.as_raw().as_ptr());
        }
        assert_eq!(frame.image.as_raw()[0], original);
        assert_eq!(buffer.map_readable().unwrap()[0], original.wrapping_add(1));
    }

    #[test]
    #[ignore = "explicit CPU input-buffer benchmark; no display or GPU"]
    fn shared_input_buffer_benchmark() {
        gst::init().unwrap();
        let frame = Frame::synthetic(1280, 720, 0).unwrap();
        let iterations = 300;
        let measure = |build: &dyn Fn() -> gst::Buffer| {
            for _ in 0..20 {
                std::hint::black_box(build());
            }
            let started = Instant::now();
            for _ in 0..iterations {
                let buffer = build();
                std::hint::black_box(buffer.map_readable().unwrap()[0]);
            }
            started.elapsed() / iterations
        };
        let copied = measure(&|| gst::Buffer::from_mut_slice(frame.image.as_raw().clone()));
        let shared = measure(&|| input_buffer(&frame));
        eprintln!(
            "1280x720 input wrap: copied={copied:?}; shared={shared:?}; avoided={} bytes/frame; samples={iterations}",
            frame.image.as_raw().len()
        );
        // Timing is evidence, not a flaky speed threshold. Ownership, pointer
        // identity and copy-on-write are covered by the ordinary tests above.
    }

    #[test]
    fn warmup_requires_a_decodable_correct_size_idr() {
        let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = frames.clone();
        let mut encoder = super::super::software::Encoder::new(
            32,
            32,
            30,
            1_000_000,
            std::sync::Arc::new(move |frame| observed.lock().unwrap().push(frame)),
        )
        .unwrap();
        encoder
            .encode(&Frame::synthetic(32, 32, 0).unwrap(), Instant::now(), true)
            .unwrap();
        let mut frame = frames.lock().unwrap()[0].clone();
        verify_probe(&frame, (32, 32)).unwrap();
        assert!(verify_probe(&frame, (64, 32)).is_err());
        frame.keyframe = false;
        assert!(verify_probe(&frame, (32, 32)).is_err());
        frame.keyframe = true;
        frame.data = vec![0, 0, 0, 1, 0x65, 1];
        assert!(verify_probe(&frame, (32, 32)).is_err());
    }
    #[test]
    #[ignore = "owned container; GStreamer software elements, synthetic frames only"]
    fn synthetic_pipeline_verifies_headers_forced_idr_pts_rate_and_rejection() {
        assert_eq!(
            std::env::var("DOGCODES_LINUX_ENCODER_FIXTURE").as_deref(),
            Ok("synthetic-only")
        );
        gst::init().unwrap();
        let size = (1280, 720);
        let profile = VideoProfile::ConstrainedBaseline31;
        let mut native = Native::open(SOFTWARE, size, (2_500_000, 30), profile).unwrap();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut last = None;
        use openh264::formats::YUVSource;
        for i in 0..5 {
            let colors = [
                [235, 30, 30, 255],
                [30, 220, 40, 255],
                [30, 40, 230, 255],
                [235, 235, 235, 255],
            ];
            let frame = Frame {
                image: std::sync::Arc::new(image::RgbaImage::from_fn(size.0, size.1, |x, y| {
                    image::Rgba(colors[(x >= size.0 / 2) as usize + 2 * (y >= size.1 / 2) as usize])
                })),
                cursor_embedded: Some(false),
            };
            let captured = Instant::now();
            let output = native
                .encode(
                    &frame,
                    captured,
                    i == 2,
                    if i < 3 {
                        (2_500_000, 30)
                    } else {
                        (1_000_000, 20)
                    },
                )
                .unwrap();
            assert_eq!(output.captured, captured);
            assert!(i != 0 && i != 2 && i != 3 || output.keyframe);
            if i == 1 || i == 4 {
                assert!(
                    !output.keyframe,
                    "fixture unexpectedly encoded every frame as an IDR"
                );
            }
            let decoded = decoder.decode(&output.data).unwrap().unwrap();
            assert_eq!(decoded.dimensions(), (1280, 720));
            let mut rgb = vec![0; 1280 * 720 * 3];
            decoded.write_rgb8(&mut rgb);
            for (x, y, index) in [(320, 180, 0), (960, 180, 1), (320, 540, 2), (960, 540, 3)] {
                let pixel = &rgb[(y * 1280 + x) * 3..(y * 1280 + x) * 3 + 3];
                assert!(
                    pixel
                        .iter()
                        .zip(colors[index])
                        .all(|(actual, want)| actual.abs_diff(want) < 20),
                    "{pixel:?}"
                );
            }
            last = Some(output);
        }
        let expected_pts = gst::ClockTime::from_nseconds(native.stream.next_pts);
        let mut stale = gst::Buffer::from_mut_slice(last.unwrap().data);
        stale
            .get_mut()
            .unwrap()
            .set_pts(gst::ClockTime::from_nseconds(42));
        let stale = gst::Sample::builder().buffer(&stale).build();
        assert!(
            native
                .stream
                .sample_frame(&stale, expected_pts, Instant::now(), false)
                .is_err()
        );
        assert!(
            native
                .encode(
                    &Frame::synthetic(32, 32, 0).unwrap(),
                    Instant::now(),
                    false,
                    (1_000_000, 20)
                )
                .is_err()
        );
        assert!(
            Native::open(
                Spec {
                    factory: "dogcodes_nonexistent_encoder",
                    bitrate_unit: 1000,
                    raw_format: "NV12",
                    settings: ""
                },
                size,
                (1_000_000, 30),
                profile
            )
            .is_err()
        );
        // Simulate a mismatched/stale AU before it can be published.
        let mut stale = gst::Buffer::from_mut_slice(
            Frame::synthetic(size.0, size.1, 0)
                .unwrap()
                .image
                .as_raw()
                .clone(),
        );
        stale
            .get_mut()
            .unwrap()
            .set_pts(gst::ClockTime::from_nseconds(42));
        native.stream.source.push_buffer(stale).unwrap();
        assert!(
            native
                .encode(
                    &Frame::synthetic(size.0, size.1, 0).unwrap(),
                    Instant::now(),
                    false,
                    (1_000_000, 20)
                )
                .is_err()
        );
    }
    #[test]
    #[ignore = "requires an actual Linux hardware plugin/driver; synthetic frames only"]
    fn hardware_synthetic_idrs_and_live_rate_changes() {
        assert_eq!(
            std::env::var("DOGCODES_LINUX_ENCODER_FIXTURE").as_deref(),
            Ok("hardware-synthetic-only")
        );
        let mut worker = hardware_attempt(
            (1280, 720),
            (2_500_000, 30),
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        use openh264::formats::YUVSource;
        for i in 0..6 {
            let captured = Instant::now();
            let output = worker
                .run((
                    Frame::synthetic(1280, 720, i).unwrap(),
                    captured,
                    i == 2,
                    if i < 3 {
                        (2_500_000, 30)
                    } else {
                        (1_000_000, 20)
                    },
                ))
                .unwrap();
            assert_eq!(output.captured, captured);
            assert!(i != 0 && i != 2 && i != 3 || output.keyframe);
            assert_eq!(
                decoder.decode(&output.data).unwrap().unwrap().dimensions(),
                (1280, 720)
            );
        }
    }
}
