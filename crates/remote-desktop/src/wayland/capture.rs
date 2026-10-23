//! A restricted PipeWire FD, one replaceable latest frame and explicit teardown.
//! No connection to the unrestricted default PipeWire server is allowed here.
use super::{
    Session, StreamInfo,
    pixels::{self, Layout},
};
use crate::{Result, video::ScreenSource};
use image::RgbaImage;
use pipewire as pw;
use pw::{
    channel,
    context::ContextRc,
    main_loop::MainLoopRc,
    properties,
    spa::{
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
            video::{VideoFormat, VideoInfoRaw},
        },
        pod::{self, Pod, serialize::PodSerializer},
        utils::{Direction, Fraction, Rectangle, SpaTypes},
    },
    stream::{StreamFlags, StreamRc, StreamState},
};
use std::{
    io::Cursor,
    os::fd::OwnedFd,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
struct Frames {
    value: Mutex<Option<Arc<RgbaImage>>>,
    changed: Condvar,
    error: Mutex<Option<String>>,
    stopped: AtomicBool,
}
impl Frames {
    fn fail(&self, error: String) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        self.changed.notify_all();
    }
}
struct Source {
    session: Session,
    frames: Arc<Frames>,
    stop: channel::Sender<()>,
}
impl Drop for Source {
    fn drop(&mut self) {
        self.frames.stopped.store(true, Ordering::Release);
        let _ = self.stop.send(());
    }
}
impl ScreenSource for Source {
    fn frame(&self) -> Result<Arc<RgbaImage>> {
        self.session.status()?;
        if let Some(error) = self
            .frames
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(error);
        }
        self.frames
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "等待 Wayland 屏幕首帧".into())
    }
    fn size(&self) -> Result<(u32, u32)> {
        Ok(self.frame()?.dimensions())
    }
    fn cursor_embedded(&self) -> bool {
        self.session.cursor_embedded()
    }
}
static WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(super) fn start(
    session: Session,
    stream: StreamInfo,
    fd: OwnedFd,
) -> Result<Arc<dyn ScreenSource>> {
    WORKERS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < 2).then_some(n + 1)
        })
        .map_err(|_| "旧 Wayland 屏幕流仍在退出")?;
    let slot = Slot;
    let frames = Arc::new(Frames {
        value: Mutex::new(None),
        changed: Condvar::new(),
        error: Mutex::new(None),
        stopped: AtomicBool::new(false),
    });
    let (stop, receiver) = channel::channel();
    let worker = frames.clone();
    let portal = session.clone();
    std::thread::Builder::new()
        .name("wayland-frames".into())
        .spawn(move || {
            let _slot = slot;
            if let Err(error) = run(&worker, &portal, stream, fd, receiver) {
                worker.fail(error);
            }
        })
        .map_err(|_| "无法启动 Wayland 屏幕流")?;
    let source = Arc::new(Source {
        session,
        frames: frames.clone(),
        stop,
    });
    let until = Instant::now() + Duration::from_secs(3);
    let mut image = frames.value.lock().unwrap_or_else(|e| e.into_inner());
    while image.is_none() {
        if let Some(error) = frames
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(error);
        }
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Wayland 屏幕首帧超时，请重新连接".into());
        }
        image = frames
            .changed
            .wait_timeout(image, remaining.min(Duration::from_millis(100)))
            .unwrap_or_else(|e| e.into_inner())
            .0;
        source.session.status()?;
    }
    drop(image);
    Ok(source)
}
struct Format {
    info: VideoInfoRaw,
}
fn run(
    frames: &Arc<Frames>,
    session: &Session,
    stream_info: StreamInfo,
    fd: OwnedFd,
    stop: channel::Receiver<()>,
) -> Result<()> {
    pw::init();
    let main_loop = MainLoopRc::new(None).map_err(|_| "无法创建 Wayland 视频循环")?;
    let context = ContextRc::new(&main_loop, None).map_err(|_| "无法创建 Wayland 视频上下文")?;
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|_| "无法连接已授权的 Wayland 屏幕流")?;
    let mut properties = properties::properties! {*pw::keys::MEDIA_TYPE=>"Video",*pw::keys::MEDIA_CATEGORY=>"Capture",*pw::keys::MEDIA_ROLE=>"Screen"};
    if let Some(serial) = stream_info.serial {
        properties.insert("target.object", serial.to_string());
    }
    let stream = StreamRc::new(core.clone(), "DogCodes remote screen", properties)
        .map_err(|_| "无法创建 Wayland 屏幕订阅")?;
    let on_state = frames.clone();
    let state_portal = session.clone();
    let on_frame = frames.clone();
    let portal = session.clone();
    let quitting = main_loop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(Format {
            info: VideoInfoRaw::default(),
        })
        .state_changed(move |_, _, old, new| {
            if !on_state.stopped.load(Ordering::Acquire)
                && (matches!(new, StreamState::Error(_))
                    || (matches!(new, StreamState::Unconnected)
                        && !matches!(old, StreamState::Unconnected)))
            {
                on_state.fail("Wayland 屏幕流已断开，请重新连接".into());
                state_portal.close();
                quitting.quit();
            }
        })
        .param_changed(|_, format, id, param| {
            if id == ParamType::Format.as_raw()
                && let Some(param) = param
            {
                let _ = format.info.parse(param);
            }
        })
        .process(move |stream, format| {
            if on_frame.stopped.load(Ordering::Acquire) || portal.status().is_err() {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let chunk = data.chunk();
            if chunk.size() == 0
                || chunk
                    .flags()
                    .contains(pw::spa::buffer::ChunkFlags::CORRUPTED)
            {
                return;
            }
            let (offset, length, stride) = (
                chunk.offset() as usize,
                chunk.size() as usize,
                chunk.stride(),
            );
            let layout = match format.info.format() {
                VideoFormat::RGBA => Layout::Rgba,
                VideoFormat::RGBx => Layout::Rgbx,
                VideoFormat::BGRA => Layout::Bgra,
                VideoFormat::BGRx => Layout::Bgrx,
                VideoFormat::RGB => Layout::Rgb,
                VideoFormat::BGR => Layout::Bgr,
                _ => {
                    on_frame.fail("Wayland 屏幕像素格式不支持".into());
                    portal.close();
                    return;
                }
            };
            let Some(bytes) = data.data() else {
                on_frame.fail("Wayland 未提供可读取的屏幕缓冲区".into());
                portal.close();
                return;
            };
            let size = format.info.size();
            match pixels::unpack(
                bytes,
                (size.width, size.height),
                offset,
                length,
                stride,
                layout,
            ) {
                Ok(image) => {
                    let mut latest = on_frame.value.lock().unwrap_or_else(|e| e.into_inner());
                    if latest
                        .as_ref()
                        .is_some_and(|previous| previous.dimensions() != image.dimensions())
                    {
                        drop(latest);
                        on_frame.fail("Wayland 屏幕尺寸已变化，请重新连接".into());
                        portal.close();
                        return;
                    }
                    *latest = Some(Arc::new(image));
                    on_frame.changed.notify_all();
                }
                Err(error) => {
                    on_frame.fail(error);
                    portal.close();
                }
            }
        })
        .register()
        .map_err(|_| "无法监听 Wayland 屏幕流")?;
    let format = pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGRA,
            VideoFormat::RGBA,
            VideoFormat::RGB,
            VideoFormat::BGR
        ),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1280,
                height: 720
            },
            Rectangle {
                width: 2,
                height: 2
            },
            Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 60, denom: 1 }
        )
    );
    let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(format))
        .map_err(|_| "Wayland 视频格式协商失败")?
        .0
        .into_inner();
    let mut params = [Pod::from_bytes(&bytes).ok_or("Wayland 视频格式无效")?];
    // A serial binds capture to the authorized object even if a node id is reused.
    stream
        .connect(
            Direction::Input,
            if stream_info.serial.is_some() {
                None
            } else {
                Some(stream_info.node)
            },
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|_| "无法订阅已授权的 Wayland 屏幕")?;
    let quitting = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quitting.quit());
    main_loop.run();
    Ok(())
}
