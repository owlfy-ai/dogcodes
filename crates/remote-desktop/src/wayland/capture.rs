//! A restricted PipeWire FD, one replaceable latest frame and explicit teardown.
//! No connection to the unrestricted default PipeWire server is allowed here.
use super::{
    Session, StreamInfo, binding, gpu,
    loan::Loan,
    metadata,
    pixels::{self, Layout, Matrix, Plane, Range},
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
    cell::RefCell,
    io::Cursor,
    os::fd::OwnedFd,
    rc::Rc,
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
    recovering_until: Mutex<Option<Instant>>,
}
impl Frames {
    fn fail(&self, error: String) {
        self.error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(error);
        self.changed.notify_all();
    }
    fn unavailable(&self) {
        self.recovering_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
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
        self.check_active()?;
        self.frames
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "等待 Wayland 屏幕首帧".into())
    }
    fn check_active(&self) -> Result<()> {
        if self.frames.stopped.load(Ordering::Acquire) {
            return Err("Wayland 屏幕来源已结束".into());
        }
        if self
            .frames
            .recovering_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| Instant::now() >= until)
        {
            self.frames
                .fail("Wayland 屏幕画面恢复超时，请重新连接".into());
            self.session.close();
        }
        if let Some(error) = self
            .frames
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(error);
        }
        if !self.session.status()? {
            return Err("Wayland 屏幕授权尚未就绪".into());
        }
        Ok(())
    }
    fn size(&self) -> Result<(u32, u32)> {
        Ok(self.frame()?.dimensions())
    }
    fn cursor_embedded(&self) -> bool {
        self.session.cursor_embedded()
    }
}
static WORKERS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(super) fn worker_count() -> usize {
    WORKERS.load(Ordering::Acquire)
}
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
        recovering_until: Mutex::new(None),
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
        if let Err(error) = source.session.status() {
            return Err(frames
                .error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or(error));
        }
    }
    drop(image);
    Ok(source)
}
struct Format {
    cursor: super::cursor::Cursor,
    info: Option<VideoInfoRaw>,
    gpu: gpu::Importer,
    retries: u8,
}

fn layout(info: VideoInfoRaw) -> Result<Layout> {
    match info.format() {
        VideoFormat::RGBA => Ok(Layout::Rgba),
        VideoFormat::RGBx => Ok(Layout::Rgbx),
        VideoFormat::BGRA => Ok(Layout::Bgra),
        VideoFormat::BGRx => Ok(Layout::Bgrx),
        VideoFormat::RGB => Ok(Layout::Rgb),
        VideoFormat::BGR => Ok(Layout::Bgr),
        VideoFormat::ARGB | VideoFormat::xRGB => Ok(Layout::Argb),
        VideoFormat::ABGR | VideoFormat::xBGR => Ok(Layout::Abgr),
        _ => Err("Wayland 屏幕像素格式不支持".into()),
    }
}

fn colors(info: VideoInfoRaw) -> Result<(Matrix, Range)> {
    use pw::spa::sys;
    let matrix = match info.color_matrix() {
        // Unspecified NV12 uses the conventional SDR BT.601 limited default;
        // explicit metadata always wins. Do not infer a matrix from resolution.
        sys::SPA_VIDEO_COLOR_MATRIX_UNKNOWN | sys::SPA_VIDEO_COLOR_MATRIX_BT601 => Matrix::Bt601,
        sys::SPA_VIDEO_COLOR_MATRIX_BT709 => Matrix::Bt709,
        _ => return Err("Wayland NV12 来源未提供受支持的颜色矩阵".into()),
    };
    let range = match info.color_range() {
        sys::SPA_VIDEO_COLOR_RANGE_0_255 => Range::Full,
        sys::SPA_VIDEO_COLOR_RANGE_UNKNOWN | sys::SPA_VIDEO_COLOR_RANGE_16_235 => Range::Limited,
        _ => return Err("Wayland NV12 来源未提供受支持的颜色范围".into()),
    };
    Ok((matrix, range))
}

fn set_format(format: &mut Format, param: Option<&Pod>) -> Result<()> {
    // Parsing may fail halfway through. Invalidate first and parse into a fresh
    // value so a rejected/unset format cannot interpret buffers with old fields.
    let was_ready = format.info.take().is_some();
    format.cursor = super::cursor::Cursor::default();
    let Some(param) = param else {
        return if was_ready {
            Err("Wayland 屏幕格式已撤销，请重新连接".into())
        } else {
            Ok(())
        };
    };
    let mut info = VideoInfoRaw::default();
    info.parse(param).map_err(|_| "Wayland 视频格式协商失败")?;
    let size = info.size();
    if size.width == 0
        || size.height == 0
        || size.width > 16384
        || size.height > 16384
        || u64::from(size.width) * u64::from(size.height) > 64 * 1024 * 1024
    {
        return Err("Wayland 画面尺寸超出支持范围".into());
    }
    if info.as_raw().flags & pw::spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0
        && !format
            .gpu
            .supports(info.format(), info.modifier(), (size.width, size.height))
    {
        return Err("当前显卡不支持所选屏幕共享格式".into());
    }
    if info.format() == VideoFormat::NV12 {
        if !size.width.is_multiple_of(2) || !size.height.is_multiple_of(2) {
            return Err("Wayland NV12 画面尺寸无效".into());
        }
        colors(info)?;
    } else {
        layout(info)?;
    }
    format.info = Some(info);
    Ok(())
}

fn chunks_corrupted(data: &[pw::spa::buffer::Data]) -> Result<bool> {
    for data in data {
        let chunk = data.as_raw().chunk;
        if chunk.is_null() {
            return Err("Wayland 画面分块信息缺失".into());
        }
        // SAFETY: the exclusive native buffer loan owns every chunk throughout
        // this callback. Check flags before accessing CPU memory or GPU FDs.
        if unsafe { (*chunk).flags } & pw::spa::buffer::ChunkFlags::CORRUPTED.bits() != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_frame(info: VideoInfoRaw, data: &[pw::spa::buffer::Data]) -> Result<Option<RgbaImage>> {
    use pw::spa::buffer::{ChunkFlags, DataFlags, DataType};
    if data.is_empty() {
        return Ok(None);
    }
    if (info.format() == VideoFormat::NV12 && data.len() > 2)
        || (info.format() != VideoFormat::NV12 && data.len() != 1)
    {
        return Err("Wayland 画面数据平面数量无效".into());
    }
    let mut mappings = Vec::with_capacity(data.len());
    for data in data {
        let raw = data.as_raw();
        if raw.chunk.is_null() {
            return Err("Wayland 画面分块信息缺失".into());
        }
        // SAFETY: The dequeued PipeWire buffer owns its chunk and mapped data until the
        // process callback returns. Only immutable slices escape this block.
        let chunk = unsafe { &*raw.chunk };
        if chunk.size == 0 || chunk.flags & ChunkFlags::CORRUPTED.bits() != 0 {
            return Ok(None);
        }
        if !matches!(data.type_(), DataType::MemPtr | DataType::MemFd)
            || !data.flags().contains(DataFlags::READABLE)
            || raw.maxsize == 0
            || raw.maxsize > 512 * 1024 * 1024
        {
            return Err("Wayland 未提供可读取的线性屏幕缓冲区".into());
        }
        mappings.push(if data.type_() == DataType::MemFd {
            Some(super::memory::Mapping::new(raw, false)?)
        } else {
            if raw.data.is_null() || raw.data as usize == usize::MAX {
                return Err("Wayland 屏幕映射地址无效".into());
            }
            None
        });
    }
    let mut planes = Vec::with_capacity(data.len());
    for (data, mapping) in data.iter().zip(&mappings) {
        let raw = data.as_raw();
        // SAFETY: chunks belong to the still-dequeued PipeWire buffer and were
        // checked above. MemPtr belongs to the producer's buffer loan; MemFd is
        // independently mapped and bounded, including on older PipeWire runtimes.
        let chunk = unsafe { &*raw.chunk };
        let bytes = if let Some(mapping) = mapping {
            mapping.bytes()
        } else {
            // SAFETY: a readable, non-null MemPtr with bounded length was checked
            // above; its buffer is not returned until conversion finishes.
            unsafe { std::slice::from_raw_parts(raw.data.cast::<u8>(), raw.maxsize as usize) }
        };
        planes.push(Plane {
            bytes,
            offset: chunk.offset,
            length: chunk.size,
            stride: chunk.stride,
        });
    }
    let size = info.size();
    let size = (size.width, size.height);
    if info.format() == VideoFormat::NV12 {
        let (matrix, range) = colors(info)?;
        pixels::unpack_nv12(size, &planes, matrix, range).map(Some)
    } else {
        let plane = &planes[0];
        let layout = layout(info)?;
        let bytes = plane.rows(size.0 as usize * layout.components(), size.1 as usize)?;
        pixels::unpack(&bytes, size, 0, bytes.len(), plane.stride, layout).map(Some)
    }
}

fn buffer_params(gpu: bool) -> Result<Vec<u8>> {
    use pw::spa::{buffer::DataType, sys};
    let mask = if gpu {
        1u32 << DataType::DmaBuf.as_raw()
    } else {
        (1u32 << DataType::MemPtr.as_raw()) | (1u32 << DataType::MemFd.as_raw())
    } as i32;
    let buffers = pod::object!(
        SpaTypes::ObjectParamBuffers,
        ParamType::Buffers,
        pod::Property {
            key: sys::SPA_PARAM_BUFFERS_dataType,
            flags: pod::PropertyFlags::empty(),
            value: pod::Value::Choice(pod::ChoiceValue::Int(pw::spa::utils::Choice(
                pw::spa::utils::ChoiceFlags::empty(),
                pw::spa::utils::ChoiceEnum::Flags {
                    default: mask,
                    flags: vec![mask]
                }
            )))
        }
    );
    PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(buffers))
        .map(|serialized| serialized.0.into_inner())
        .map_err(|_| "Wayland 缓冲区协商失败".into())
}
fn format_parameters(caps: Vec<gpu::Capability>) -> Result<Vec<Vec<u8>>> {
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
            VideoFormat::BGR,
            VideoFormat::ARGB,
            VideoFormat::ABGR,
            VideoFormat::xRGB,
            VideoFormat::xBGR,
            VideoFormat::NV12
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
    let mut objects = Vec::new();
    for cap in caps {
        if cap.modifiers.is_empty() {
            continue;
        }
        let mut object = format.clone();
        object.properties.retain(|p| {
            p.key != FormatProperties::VideoFormat.as_raw()
                && p.key != FormatProperties::VideoSize.as_raw()
        });
        object.properties.push(pod::property!(
            FormatProperties::VideoFormat,
            Id,
            cap.format
        ));
        object.properties.push(pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1280.min(cap.max_size),
                height: 720.min(cap.max_size)
            },
            Rectangle {
                width: 2,
                height: 2
            },
            Rectangle {
                width: cap.max_size,
                height: cap.max_size
            }
        ));
        object.properties.push(pod::Property {
            key: FormatProperties::VideoModifier.as_raw(),
            flags: pod::PropertyFlags::MANDATORY,
            value: pod::Value::Choice(pod::ChoiceValue::Long(pw::spa::utils::Choice(
                pw::spa::utils::ChoiceFlags::empty(),
                pw::spa::utils::ChoiceEnum::Enum {
                    default: cap.modifiers[0] as i64,
                    alternatives: cap.modifiers.iter().map(|v| *v as i64).collect(),
                },
            ))),
        });
        objects.push(object);
    }
    objects.push(format); // SHM remains available even when GPU negotiation fails.
    objects
        .into_iter()
        .map(|object| {
            PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(object))
                .map(|v| v.0.into_inner())
                .map_err(|_| "Wayland 视频格式协商失败".into())
        })
        .collect()
}
fn apply_parameters(stream: &pw::stream::Stream, bytes: Vec<Vec<u8>>) -> Result<()> {
    let mut params = bytes
        .iter()
        .map(|b| Pod::from_bytes(b).ok_or("Wayland 视频格式无效"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    stream
        .update_params(&mut params)
        .map_err(|_| "Wayland 视频格式重新协商失败".into())
}

fn fixate_modifier(param: &Pod, caps: &[gpu::Capability]) -> Result<Option<Vec<u8>>> {
    use pw::spa::{
        pod::{ChoiceValue, Value, deserialize::PodDeserializer},
        utils::ChoiceEnum,
    };
    if param.as_bytes().len() > 64 * 1024 {
        return Err("Wayland 视频格式描述过大".into());
    }
    let (_, value) = PodDeserializer::deserialize_any_from(param.as_bytes())
        .map_err(|_| "Wayland 视频格式描述无效")?;
    let Value::Object(mut object) = value else {
        return Ok(None);
    };
    let mut info = VideoInfoRaw::default();
    info.parse(param).map_err(|_| "Wayland 视频格式协商失败")?;
    let Some(property) = object
        .properties
        .iter_mut()
        .find(|p| p.key == FormatProperties::VideoModifier.as_raw())
    else {
        return Ok(None);
    };
    let candidates = match &property.value {
        Value::Choice(ChoiceValue::Long(choice)) => match &choice.1 {
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => std::iter::once(*default)
                .chain(alternatives.iter().copied())
                .collect::<Vec<_>>(),
            ChoiceEnum::None(value) => vec![*value],
            _ => return Err("Wayland GPU modifier 选择无效".into()),
        },
        Value::Long(value)
            if property.flags.bits() & pw::spa::sys::SPA_POD_PROP_FLAG_DONT_FIXATE != 0 =>
        {
            vec![*value]
        }
        Value::Long(_) => return Ok(None),
        _ => return Err("Wayland GPU modifier 类型无效".into()),
    };
    let size = info.size();
    let modifier = candidates
        .into_iter()
        .find(|value| {
            caps.iter().any(|cap| {
                cap.format == info.format()
                    && cap.max_size >= size.width
                    && cap.max_size >= size.height
                    && cap.modifiers.contains(&(*value as u64))
            })
        })
        .ok_or("显卡没有可用的屏幕共享 modifier")?;
    property.value = Value::Long(modifier);
    property.flags = pod::PropertyFlags::MANDATORY;
    object.id = ParamType::Format.as_raw();
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|v| Some(v.0.into_inner()))
        .map_err(|_| "Wayland GPU 格式固定失败".into())
}

fn process_frame(
    stream: &pw::stream::Stream,
    format: &mut Format,
    on_frame: &Arc<Frames>,
    portal: &Session,
    reconfigure: &channel::Sender<Vec<Vec<u8>>>,
    binding: &binding::State,
) {
    if !binding.ready() {
        return;
    }
    if on_frame.stopped.load(Ordering::Acquire) || portal.status().is_err() {
        return;
    }
    let Some(mut buffer) = Loan::dequeue(stream) else {
        return;
    };
    // Prefer the newest queued frame after a slow GPU readback. Bound
    // draining even if a producer keeps filling buffers concurrently.
    for _ in 0..31 {
        let Some(newer) = Loan::dequeue(stream) else {
            break;
        };
        if portal.cursor_mode() == 4 {
            // Preserve shape-only updates even in buffers skipped for latency.
            // SAFETY: the exclusive Loan still owns all metadata here.
            let cursor = (|| unsafe { format.cursor.update(buffer.raw()?) })();
            if let Err(error) = cursor {
                on_frame.fail(error);
                portal.close();
                return;
            }
        }
        buffer = newer;
    }
    if portal.cursor_mode() == 4 {
        // SAFETY: the final buffer is still exclusively held by this Loan.
        if let Err(error) = (|| unsafe { format.cursor.update(buffer.raw()?) })() {
            on_frame.fail(error);
            portal.close();
            return;
        }
    }
    let Some(info) = format.info else {
        return;
    };
    let presentation = (|| {
        // SAFETY: Loan exclusively retains the native buffer and all metadata
        // while read validates descriptors and copies only known SPA PODs.
        unsafe { metadata::read(buffer.raw()?) }
    })();
    let presentation = match presentation {
        Ok(Some(presentation)) => presentation,
        Ok(None) => {
            on_frame.unavailable();
            return;
        }
        Err(error) => {
            on_frame.fail(error);
            portal.close();
            return;
        }
    };
    let data = match buffer.datas_mut() {
        Ok(data) => data,
        Err(error) => {
            on_frame.fail(error);
            portal.close();
            return;
        }
    };
    match chunks_corrupted(data) {
        Ok(true) => {
            on_frame.unavailable();
            return;
        }
        Ok(false) => (),
        Err(error) => {
            on_frame.fail(error);
            portal.close();
            return;
        }
    }
    let is_gpu = info.as_raw().flags & pw::spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0;
    let result = if is_gpu {
        gpu::planes(data).and_then(|planes| {
            let Some(planes) = planes else {
                return Ok(None);
            };
            let size = info.size();
            let frame = gpu::Frame {
                size: (size.width, size.height),
                format: info.format(),
                modifier: Some(info.modifier()),
                planes,
                colors: if info.format() == VideoFormat::NV12 {
                    Some(colors(info)?)
                } else {
                    None
                },
            };
            format.gpu.read(&frame).map(Some)
        })
    } else {
        read_frame(info, data)
    };
    // Invalid presentation is a source-metadata error, not a failed GPU import;
    // never blacklist a working modifier or renegotiate it for a bad crop.
    let result = match result {
        Ok(image) => match if portal.cursor_mode() == 4 {
            format.cursor.render(image, presentation)
        } else {
            image.map(|image| presentation.apply(image)).transpose()
        } {
            Ok(image) => Ok(image),
            Err(error) => {
                on_frame.fail(error);
                portal.close();
                return;
            }
        },
        other => other,
    };
    match result {
        // Empty chunks can be metadata-only updates on a static desktop. Only
        // explicit corruption above starts recovery; no new pixels is normal.
        Ok(None) => (),
        Ok(Some(image)) => {
            if on_frame.stopped.load(Ordering::Acquire) || portal.status().is_err() {
                return;
            }
            *on_frame
                .recovering_until
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            format.retries = 0;
            let mut latest = on_frame.value.lock().unwrap_or_else(|e| e.into_inner());
            // Pixel geometry belongs to the video renegotiation path. It does
            // not revoke this grant or redefine the portal's logical input size.
            if latest
                .as_ref()
                .is_some_and(|previous| previous.dimensions() != image.dimensions())
            {
                portal.invalidate_input_geometry();
            }
            *latest = Some(Arc::new(image));
            on_frame.changed.notify_all();
        }
        Err(error) => {
            if is_gpu && format.retries < 16 {
                format.retries += 1;
                format.gpu.reject(info.format(), info.modifier());
                format.info = None;
                on_frame
                    .recovering_until
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
                drop(buffer); // Release the original loan before asking for new buffers.
                if let Ok(params) = format_parameters(format.gpu.capabilities())
                    && reconfigure.send(params).is_ok()
                {
                    return;
                }
            }
            on_frame.fail(error);
            portal.close();
        }
    }
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
    properties.insert("node.target", stream_info.node.to_string());
    properties.insert("node.dont-reconnect", "true");
    if let Some(serial) = stream_info.serial {
        properties.insert("target.object", serial.to_string());
    }
    let stream = StreamRc::new(core.clone(), "DogCodes remote screen", properties)
        .map_err(|_| "无法创建 Wayland 屏幕订阅")?;
    let gpu = gpu::Importer::discover();
    let gpu_caps = gpu.capabilities();
    let (reconfigure, changes) = channel::channel::<Vec<Vec<u8>>>();
    let fixating = reconfigure.clone();
    let poll_reconfigure = reconfigure.clone();
    let failed_frames = frames.clone();
    let failed_portal = session.clone();
    let failed_loop = main_loop.clone();
    let binding = binding::Guard::new(
        &core,
        stream_info.node,
        stream_info.serial,
        Rc::new(move |error| {
            failed_frames.fail(error);
            failed_portal.close();
            failed_loop.quit();
        }),
    )?;
    let state_binding = binding.state.clone();
    let frame_binding = binding.state.clone();
    let format = Rc::new(RefCell::new(Format {
        cursor: super::cursor::Cursor::default(),
        info: None,
        gpu,
        retries: 0,
    }));
    let on_state = frames.clone();
    let state_portal = session.clone();
    let on_frame = frames.clone();
    let portal = session.clone();
    let quitting = main_loop.clone();
    let format_quit = main_loop.clone();
    let format_frames = frames.clone();
    let format_portal = session.clone();
    let _listener = stream
        .add_local_listener_with_user_data(format.clone())
        .state_changed(move |_, _, old, new| {
            state_binding.linked(matches!(new, StreamState::Streaming));
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
        .param_changed(move |_, data, id, param| {
            let mut borrowed = data.borrow_mut();
            let format = &mut *borrowed;
            if id != ParamType::Format.as_raw() {
                return;
            }
            let result = (|| {
                if let Some(param) = param
                    && let Some(fixed) = fixate_modifier(param, &format.gpu.capabilities())?
                {
                    if format.retries >= 16 {
                        return Err("Wayland GPU 格式协商次数超限".into());
                    }
                    format.retries += 1;
                    format.info = None;
                    format_frames
                        .recovering_until
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
                    return fixating
                        .send(vec![fixed])
                        .map_err(|_| "Wayland GPU 格式固定失败".into());
                }
                set_format(format, param)?;
                let Some(info) = format.info else {
                    return Ok(());
                };
                let bytes = buffer_params(
                    info.as_raw().flags & pw::spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0,
                )?;
                let mut params = vec![bytes];
                params.extend(metadata::parameters_for_cursor(
                    format_portal.cursor_mode() == 4,
                )?);
                fixating
                    .send(params)
                    .map_err(|_| "Wayland 缓冲区协商失败".into())
            })();
            if let Err(error) = result {
                format.info = None;
                format_frames.fail(error);
                format_portal.close();
                format_quit.quit();
            }
        })
        .process(move |stream, data| {
            process_frame(
                stream,
                &mut data.borrow_mut(),
                &on_frame,
                &portal,
                &reconfigure,
                &frame_binding,
            );
        })
        .register()
        .map_err(|_| "无法监听 Wayland 屏幕流")?;
    let mut bytes = format_parameters(gpu_caps)?;
    bytes.extend(metadata::parameters_for_cursor(session.cursor_mode() == 4)?);
    let mut params = bytes
        .iter()
        .map(|b| Pod::from_bytes(b).ok_or("Wayland 视频格式无效"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Identity is checked before publication; disable automatic routing to a
    // replacement node. The numeric target also works with early WirePlumber 0.4.
    stream
        .connect(
            Direction::Input,
            Some(stream_info.node),
            StreamFlags::AUTOCONNECT | StreamFlags::DONT_RECONNECT,
            &mut params,
        )
        .map_err(|_| "无法订阅已授权的 Wayland 屏幕")?;
    let changing = stream.clone();
    let change_frames = frames.clone();
    let change_portal = session.clone();
    let change_quit = main_loop.clone();
    // Format changes run outside process/param callbacks, after the original
    // PipeWire buffer loan is returned and mutable listener data is released.
    let _changes = changes.attach(main_loop.loop_(), move |params| {
        if change_frames.stopped.load(Ordering::Acquire) || change_portal.status().is_err() {
            return;
        }
        if let Err(error) = apply_parameters(&changing, params) {
            change_frames.fail(error);
            change_portal.close();
            change_quit.quit();
        }
    });
    let timeout_frames = frames.clone();
    let polling = stream.clone();
    let poll_format = format.clone();
    let poll_binding = binding.state.clone();
    let timeout_portal = session.clone();
    let timeout_quit = main_loop.clone();
    let timer = main_loop.loop_().add_timer(move |_| {
        let expired = timeout_frames
            .recovering_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| Instant::now() >= until);
        if expired {
            timeout_frames.fail("Wayland 屏幕画面恢复超时，请重新连接".into());
            timeout_portal.close();
        }
        if expired
            || timeout_frames.stopped.load(Ordering::Acquire)
            || timeout_portal.status().is_err()
        {
            timeout_quit.quit();
        } else {
            // A static source may have queued its only frame before the identity
            // barrier. Poll its existing queue without losing that first frame.
            process_frame(
                &polling,
                &mut poll_format.borrow_mut(),
                &timeout_frames,
                &timeout_portal,
                &poll_reconfigure,
                &poll_binding,
            );
        }
    });
    timer
        .update_timer(
            Some(Duration::from_millis(100)),
            Some(Duration::from_millis(100)),
        )
        .into_result()
        .map_err(|_| "Wayland 视频超时检测不可用")?;
    let quitting = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quitting.quit());
    main_loop.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw::spa::sys;

    #[test]
    fn gpu_modifier_choice_is_fixed_without_dropping_the_shm_alternative() {
        let caps = [gpu::Capability {
            format: VideoFormat::BGRx,
            modifiers: vec![9, 3],
            max_size: 2048,
        }];
        let bytes = format_parameters(vec![gpu::Capability {
            format: VideoFormat::BGRx,
            modifiers: vec![9, 3],
            max_size: 2048,
        }])
        .unwrap();
        assert_eq!(bytes.len(), 2);
        let offer = Pod::from_bytes(&bytes[0]).unwrap();
        let fixed = fixate_modifier(offer, &caps).unwrap().unwrap();
        let fixed = Pod::from_bytes(&fixed).unwrap();
        let mut info = VideoInfoRaw::default();
        info.parse(fixed).unwrap();
        assert_eq!(info.modifier(), 9);
        assert_ne!(info.as_raw().flags & sys::SPA_VIDEO_FLAG_MODIFIER, 0);
        assert!(fixate_modifier(fixed, &caps).unwrap().is_none());
        assert!(fixate_modifier(offer, &[]).is_err());
        let shm = Pod::from_bytes(&bytes[1]).unwrap();
        info.parse(shm).unwrap();
        assert_eq!(info.as_raw().flags & sys::SPA_VIDEO_FLAG_MODIFIER, 0);
        assert!(fixate_modifier(shm, &[]).unwrap().is_none());
    }

    fn format_bytes(format: VideoFormat, matrix: u32, range: u32) -> Vec<u8> {
        let object = pod::object!(
            SpaTypes::ObjectParamFormat,
            ParamType::Format,
            pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            pod::property!(FormatProperties::VideoFormat, Id, format),
            pod::property!(
                FormatProperties::VideoSize,
                Rectangle,
                Rectangle {
                    width: 2,
                    height: 2
                }
            ),
            pod::property!(
                FormatProperties::VideoColorMatrix,
                pod::Value::Id(pw::spa::utils::Id(matrix))
            ),
            pod::property!(
                FormatProperties::VideoColorRange,
                pod::Value::Id(pw::spa::utils::Id(range))
            )
        );
        PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Object(object))
            .unwrap()
            .0
            .into_inner()
    }

    #[test]
    fn rejected_or_unset_format_cannot_leave_previous_pixel_description_active() {
        let mut format = Format {
            cursor: super::super::cursor::Cursor::default(),
            info: None,
            gpu: gpu::Importer::empty(),
            retries: 0,
        };
        let rgb = format_bytes(VideoFormat::BGRx, 0, 0);
        let missing = format_bytes(VideoFormat::NV12, sys::SPA_VIDEO_COLOR_MATRIX_RGB, 0);
        set_format(&mut format, Pod::from_bytes(&rgb)).unwrap();
        assert_eq!(format.info.unwrap().format(), VideoFormat::BGRx);
        assert!(set_format(&mut format, Pod::from_bytes(&missing)).is_err());
        assert!(format.info.is_none());
        let valid = format_bytes(
            VideoFormat::NV12,
            sys::SPA_VIDEO_COLOR_MATRIX_BT709,
            sys::SPA_VIDEO_COLOR_RANGE_16_235,
        );
        set_format(&mut format, Pod::from_bytes(&valid)).unwrap();
        assert_eq!(format.info.unwrap().format(), VideoFormat::NV12);
        assert!(set_format(&mut format, None).is_err());
        assert!(format.info.is_none());
        set_format(&mut format, None).unwrap();
        set_format(&mut format, Pod::from_bytes(&rgb)).unwrap();
        let malformed = PodSerializer::serialize(Cursor::new(Vec::new()), &pod::Value::Int(123))
            .unwrap()
            .0
            .into_inner();
        assert!(set_format(&mut format, Pod::from_bytes(&malformed)).is_err());
        assert!(format.info.is_none());
    }

    fn info() -> VideoInfoRaw {
        let bytes = format_bytes(
            VideoFormat::NV12,
            sys::SPA_VIDEO_COLOR_MATRIX_BT601,
            sys::SPA_VIDEO_COLOR_RANGE_16_235,
        );
        let mut format = Format {
            cursor: super::super::cursor::Cursor::default(),
            info: None,
            gpu: gpu::Importer::empty(),
            retries: 0,
        };
        set_format(&mut format, Pod::from_bytes(&bytes)).unwrap();
        format.info.unwrap()
    }

    fn data(bytes: &[u8], chunk: *mut sys::spa_chunk) -> sys::spa_data {
        sys::spa_data {
            type_: sys::SPA_DATA_MemPtr,
            flags: pw::spa::buffer::DataFlags::READABLE.bits(),
            fd: -1,
            mapoffset: 0,
            maxsize: bytes.len() as u32,
            data: bytes.as_ptr().cast_mut().cast(),
            chunk,
        }
    }

    fn read(data: &[sys::spa_data]) -> Result<Option<RgbaImage>> {
        // SAFETY: libspa Data is repr(transparent) over spa_data. These test
        // descriptors, chunks and immutable bytes live through the synchronous read.
        let mapped = unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), data.len()) };
        read_frame(info(), mapped)
    }

    #[test]
    fn read_only_planes_are_combined_and_a_corrupted_second_plane_drops_the_frame() {
        let y = [16, 16, 235, 235];
        let uv = [128, 128];
        let mut y_chunk = sys::spa_chunk {
            offset: 0,
            size: 4,
            stride: 2,
            flags: 0,
        };
        let mut uv_chunk = sys::spa_chunk {
            offset: 0,
            size: 2,
            stride: 2,
            flags: 0,
        };
        let planes = [data(&y, &mut y_chunk), data(&uv, &mut uv_chunk)];
        let image = read(&planes).unwrap().unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [255, 255, 255, 255]);
        uv_chunk.flags = pw::spa::buffer::ChunkFlags::CORRUPTED.bits();
        let planes = [data(&y, &mut y_chunk), data(&uv, &mut uv_chunk)];
        assert!(read(&planes).unwrap().is_none());
    }

    #[test]
    fn empty_updates_are_normal_but_corrupted_planes_are_detected_before_payload_access() {
        let mut chunk = sys::spa_chunk {
            offset: 0,
            size: 0,
            stride: 0,
            flags: 0,
        };
        let mut descriptor = data(&[], &mut chunk);
        // A DMA-BUF may have a zero chunk size. This check must neither import
        // its FD nor dereference the pixel address, even for corrupted input.
        descriptor.type_ = sys::SPA_DATA_DmaBuf;
        descriptor.data = std::ptr::dangling_mut::<u8>().cast();
        let inspect = |descriptor: sys::spa_data| {
            // SAFETY: Data is transparent; the chunk is alive for this call.
            let data = unsafe { &*(&descriptor as *const sys::spa_data).cast() };
            chunks_corrupted(std::slice::from_ref(data))
        };
        assert!(!chunks_corrupted(&[]).unwrap());
        assert!(!inspect(descriptor).unwrap());
        chunk.flags = pw::spa::buffer::ChunkFlags::CORRUPTED.bits();
        descriptor.chunk = &mut chunk;
        assert!(inspect(descriptor).unwrap());
        descriptor.chunk = std::ptr::null_mut();
        assert!(inspect(descriptor).is_err());
    }

    #[test]
    fn missing_metadata_and_non_cpu_or_unreadable_memory_fail_without_dereferencing_data() {
        let mut chunk = sys::spa_chunk {
            offset: 0,
            size: 6,
            stride: 2,
            flags: 0,
        };
        let mut descriptor = data(&[], &mut chunk);
        descriptor.data = std::ptr::dangling_mut::<u8>().cast();
        descriptor.maxsize = 6;
        descriptor.type_ = sys::SPA_DATA_DmaBuf;
        assert!(read(&[descriptor]).is_err());
        descriptor.type_ = sys::SPA_DATA_MemFd;
        descriptor.flags = 0;
        assert!(read(&[descriptor]).is_err());
        descriptor.flags = pw::spa::buffer::DataFlags::READABLE.bits();
        descriptor.chunk = std::ptr::null_mut();
        assert!(read(&[descriptor]).is_err());
    }
}
