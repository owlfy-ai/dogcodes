//! Optional hardware Media Foundation encoder. All COM/MFT state stays on the
//! bounded codec worker. MF DLLs are loaded from System32 at runtime so Windows
//! N/missing Media Features can still start the app and use OpenH264.
use super::{EncodedFrame, Result, VideoProfile, h264::Headers, profile_size, software::Frame};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};
use std::{
    ffi::c_void,
    mem::ManuallyDrop,
    rc::Rc,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, E_POINTER, FreeLibrary, HMODULE},
        Media::MediaFoundation::*,
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
            LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
            Variant::VARIANT,
        },
    },
    core::{GUID, HRESULT, Interface, PCSTR, w},
};
type Create = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type Allocate = unsafe extern "system" fn(u32, u32, *mut *mut c_void) -> HRESULT;
type Enumerate = unsafe extern "system" fn(
    GUID,
    u32,
    *const MFT_REGISTER_TYPE_INFO,
    *const MFT_REGISTER_TYPE_INFO,
    *mut *mut Option<IMFActivate>,
    *mut u32,
) -> HRESULT;
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: this owns a successful LoadLibrary reference, after all MF objects have dropped.
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}
struct Api {
    startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    shutdown: unsafe extern "system" fn() -> HRESULT,
    enumerate: Enumerate,
    media_type: Create,
    sample: Create,
    allocate: Allocate,
    _library: Library,
}
fn error(e: impl std::fmt::Display) -> String {
    format!("Windows 系统视频编码不可用：{e}")
}
impl Api {
    fn load() -> Result<Self> {
        // SAFETY: only a fixed System32 DLL and fixed documented exports are
        // resolved. Each signature matches mfapi.h; the library outlives them.
        unsafe {
            let library = Library(
                LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                    .map_err(error)?,
            );
            macro_rules! load {
                ($name:literal, $signature:ty) => {{
                    let proc = GetProcAddress(library.0, PCSTR(concat!($name, "\0").as_ptr()))
                        .ok_or_else(|| error(concat!("缺少 ", $name)))?;
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, $signature>(proc)
                }};
            }
            Ok(Self {
                startup: load!("MFStartup", unsafe extern "system" fn(u32, u32) -> HRESULT),
                shutdown: load!("MFShutdown", unsafe extern "system" fn() -> HRESULT),
                enumerate: load!("MFTEnumEx", Enumerate),
                media_type: load!("MFCreateMediaType", Create),
                sample: load!("MFCreateSample", Create),
                allocate: load!("MFCreateAlignedMemoryBuffer", Allocate),
                _library: library,
            })
        }
    }
    fn create<T: Interface>(&self, create: Create) -> Result<T> {
        // SAFETY: the concrete export and interface match at every caller. COM
        // transfers one owned reference; null success is rejected.
        unsafe {
            let mut p = std::ptr::null_mut();
            create(&mut p).ok().map_err(error)?;
            if p.is_null() {
                return Err(error(E_POINTER));
            }
            Ok(T::from_raw(p))
        }
    }
    fn buffer(&self, length: usize, alignment: u32) -> Result<IMFMediaBuffer> {
        if length == 0
            || length > 8 * 1024 * 1024
            || alignment > 65536
            || (alignment > 0 && !alignment.is_power_of_two())
        {
            return Err(error("编码缓冲区预算无效"));
        }
        // SAFETY: bounded allocation, documented alignment mask, out pointer
        // initialized null. One returned reference is owned by this wrapper.
        unsafe {
            let mut p = std::ptr::null_mut();
            (self.allocate)(length as u32, alignment.saturating_sub(1), &mut p)
                .ok()
                .map_err(error)?;
            if p.is_null() {
                return Err(error(E_POINTER));
            }
            Ok(IMFMediaBuffer::from_raw(p))
        }
    }
}
struct Runtime {
    api: Api,
}
struct Activated(IMFActivate);
impl Drop for Activated {
    fn drop(&mut self) {
        // SAFETY: this owns the activation; cleanup also runs when an
        // interface cast or format setup fails before Native is constructed.
        unsafe {
            let _ = self.0.ShutdownObject();
        }
    }
}
impl Runtime {
    fn start() -> Result<Rc<Self>> {
        // SAFETY: initialized and balanced on the dedicated codec thread; no
        // caller apartment is changed. S_FALSE also owns an initialization.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(error)?;
        }
        let result = Api::load().and_then(|api| {
            // SAFETY: export/library live, fixed SDK version/full startup flags.
            unsafe {
                (api.startup)(MF_VERSION, MFSTARTUP_FULL)
                    .ok()
                    .map_err(error)?;
            }
            Ok(Rc::new(Self { api }))
        });
        if result.is_err() {
            // SAFETY: balances only this thread's successful initialization.
            unsafe {
                CoUninitialize();
            }
        }
        result
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: all MF COM references drop before the final Rc; API/library remain alive during shutdown.
        unsafe {
            let _ = (self.api.shutdown)();
            CoUninitialize();
        }
    }
}
struct Activations {
    array: *mut Option<IMFActivate>,
    count: u32,
}
impl Drop for Activations {
    fn drop(&mut self) {
        if !self.array.is_null() {
            // SAFETY: MFTEnumEx owns this count-sized COM pointer array and uses CoTaskMem allocation.
            unsafe {
                for activation in std::slice::from_raw_parts_mut(self.array, self.count as usize) {
                    activation.take();
                }
                CoTaskMemFree(Some(self.array.cast()));
            }
        }
    }
}
struct Output(MFT_OUTPUT_DATA_BUFFER);
impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: these are owned Option COM references, each released exactly once even on ProcessOutput failure.
        unsafe {
            ManuallyDrop::drop(&mut self.0.pSample);
            ManuallyDrop::drop(&mut self.0.pEvents);
        }
    }
}
struct Locked<'a> {
    buffer: &'a IMFMediaBuffer,
    data: *mut u8,
    capacity: u32,
    length: u32,
}
impl<'a> Locked<'a> {
    fn new(buffer: &'a IMFMediaBuffer) -> Result<Self> {
        let (mut data, mut capacity, mut length) = (std::ptr::null_mut(), 0, 0);
        // SAFETY: valid COM buffer; mapping lives only until this guard drops.
        unsafe {
            buffer
                .Lock(&mut data, Some(&mut capacity), Some(&mut length))
                .map_err(error)?;
        }
        let locked = Self {
            buffer,
            data,
            capacity,
            length,
        };
        if data.is_null() || length > capacity {
            return Err(error("编码映射无效"));
        }
        Ok(locked)
    }
    fn write_nv12(&mut self, source: &YUVBuffer, length: usize) -> Result<()> {
        if length > self.capacity as usize {
            return Err(error("编码输入缓冲区不足"));
        }
        // SAFETY: this newly allocated input buffer is private to its codec
        // worker and not yet attached to a sample. Lock keeps the non-null
        // pointer valid; checked capacity bounds this exclusive mutable slice.
        let destination = unsafe { std::slice::from_raw_parts_mut(self.data, length) };
        super::nv12::write(source, destination).map_err(error)
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        if self.length == 0 || self.length > 8 * 1024 * 1024 {
            return Err(error("编码输出长度无效"));
        }
        // SAFETY: length <= mapped capacity, still locked, copied before unlock.
        Ok(unsafe { std::slice::from_raw_parts(self.data, self.length as usize) }.to_vec())
    }
}
impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: exactly balances the successful IMFMediaBuffer::Lock.
        unsafe {
            let _ = self.buffer.Unlock();
        }
    }
}

pub(super) struct Native {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    codec: ICodecAPI,
    _activate: Activated,
    input_id: u32,
    output_id: u32,
    input_bytes: usize,
    input_alignment: u32,
    headers: Headers,
    yuv: YUVBuffer,
    nv12_len: usize,
    size: (u32, u32),
    rate: (u32, u32),
    profile: VideoProfile,
    input_credits: u32,
    output_credits: u32,
    timestamp: i64,
    first: bool,
    // Last: MFT interfaces release before runtime/DLL/apartment teardown.
    runtime: Rc<Runtime>,
}
impl Native {
    pub fn new(size: (u32, u32), rate: (u32, u32), profile: VideoProfile) -> Result<Self> {
        if rate.1 > 30 || rate.0 > 9_000_000 {
            return Err(error("系统编码速率超过当前等级预算"));
        }
        if profile_size(size.0, size.1, size.0, VideoProfile::ConstrainedBaseline31)? != size {
            return Err(error("尺寸超过系统编码等级预算"));
        }
        let runtime = Runtime::start()?;
        let mut activations = Activations {
            array: std::ptr::null_mut(),
            count: 0,
        };
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        // SAFETY: fixed hardware-only category and correctly typed, stack-owned filters/out pointers.
        unsafe {
            (runtime.api.enumerate)(
                MFT_CATEGORY_VIDEO_ENCODER,
                (MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER).0 as u32,
                &input,
                &output,
                &mut activations.array,
                &mut activations.count,
            )
            .ok()
            .map_err(error)?;
        }
        if activations.array.is_null() || activations.count == 0 {
            return Err(error("未找到可用的硬件 H.264 编码器"));
        }
        let mut failure = error("硬件编码器未通过首帧验证");
        // SAFETY: successful MFTEnumEx returned this count-sized array; the
        // allocation remains owned until all candidate references are released.
        let candidates =
            unsafe { std::slice::from_raw_parts(activations.array, activations.count as usize) };
        for activation in candidates.iter().flatten().take(8) {
            let candidate = Self::open(runtime.clone(), activation.clone(), size, rate, profile);
            match candidate {
                Ok(mut candidate) => {
                    let frame = Frame::synthetic(size.0, size.1, 0)?;
                    match candidate.encode(&frame, Instant::now(), true, rate) {
                        Ok(probe) if probe.keyframe => {
                            let mut decoder = openh264::decoder::Decoder::new().map_err(error)?;
                            match decoder.decode(&probe.data) {
                                Ok(Some(decoded))
                                    if decoded.dimensions()
                                        == (size.0 as usize, size.1 as usize) =>
                                {
                                    // The warmup IDR never reaches the phone.
                                    // The first real input needs its own IDR.
                                    candidate.first = true;
                                    return Ok(candidate);
                                }
                                _ => failure = error("硬件编码首帧未通过实际解码验证"),
                            }
                        }
                        Ok(_) => failure = error("硬件编码首帧不是 IDR"),
                        Err(e) => failure = e,
                    }
                }
                Err(e) => failure = e,
            }
        }
        Err(failure)
    }
    fn open(
        runtime: Rc<Runtime>,
        activate: IMFActivate,
        size: (u32, u32),
        rate: (u32, u32),
        profile: VideoProfile,
    ) -> Result<Self> {
        // SAFETY: all objects and calls stay in the initialized MTA worker.
        // COM methods validate support/types; owned wrappers clean every error.
        unsafe {
            let activate = Activated(activate);
            let transform: IMFTransform = activate.0.ActivateObject().map_err(error)?;
            let events = transform.cast().map_err(error)?;
            let codec: ICodecAPI = transform.cast().map_err(error)?;
            let mut native = Self {
                transform,
                events,
                codec,
                _activate: activate,
                input_id: 0,
                output_id: 0,
                input_bytes: 0,
                input_alignment: 16,
                headers: Headers::default(),
                yuv: YUVBuffer::new(size.0 as usize, size.1 as usize),
                nv12_len: size.0 as usize * size.1 as usize * 3 / 2,
                size,
                rate,
                profile,
                input_credits: 0,
                output_credits: 0,
                timestamp: 0,
                first: true,
                runtime,
            };
            let attributes = native.transform.GetAttributes().map_err(error)?;
            if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).map_err(error)? != 1 {
                return Err(error("系统编码器不是异步硬件实现"));
            }
            attributes
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(error)?;
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            // Optional properties: the real one-input/one-output deadline,
            // matching timestamps, baseline SPS and decode probe enforce the
            // usable low-latency contract even when a driver omits these.
            let _ = native
                .codec
                .SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
            let _ = native
                .codec
                .SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(0u32));
            let _ = native
                .codec
                .SetValue(&CODECAPI_AVEncCommonRateControlMode, &VARIANT::from(0u32));
            let (mut inputs, mut outputs) = (0, 0);
            native
                .transform
                .GetStreamCount(&mut inputs, &mut outputs)
                .map_err(error)?;
            if (inputs, outputs) != (1, 1) {
                return Err(error("系统编码流布局不支持"));
            }
            let mut input_ids = [0];
            let mut output_ids = [0];
            match native
                .transform
                .GetStreamIDs(&mut input_ids, &mut output_ids)
            {
                Ok(()) => {
                    native.input_id = input_ids[0];
                    native.output_id = output_ids[0];
                }
                Err(e) if e.code() == E_NOTIMPL => {}
                Err(e) => return Err(error(e)),
            }
            let out = native.media_type(MFVideoFormat_H264, true)?;
            native
                .transform
                .SetOutputType(native.output_id, &out, 0)
                .map_err(error)?;
            let input = native.media_type(MFVideoFormat_NV12, false)?;
            native
                .transform
                .SetInputType(native.input_id, &input, 0)
                .map_err(error)?;
            let mut input_info = MFT_INPUT_STREAM_INFO::default();
            native
                .transform
                .GetInputStreamInfo(native.input_id, &mut input_info)
                .map_err(error)?;
            native.input_bytes = (input_info.cbSize as usize).max(native.nv12_len);
            native.input_alignment = input_info.cbAlignment.max(16);
            native
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(error)?;
            native
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(error)?;
            Ok(native)
        }
    }
    fn media_type(&self, subtype: GUID, output: bool) -> Result<IMFMediaType> {
        let media: IMFMediaType = self.runtime.api.create(self.runtime.api.media_type)?;
        // SAFETY: owned attribute store, SDK GUIDs and bounded scalar values.
        unsafe {
            media
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(error)?;
            media.SetGUID(&MF_MT_SUBTYPE, &subtype).map_err(error)?;
            media
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(self.size.0) << 32) | u64::from(self.size.1),
                )
                .map_err(error)?;
            media
                .SetUINT64(&MF_MT_FRAME_RATE, (u64::from(self.rate.1) << 32) | 1)
                .map_err(error)?;
            media
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
                .map_err(error)?;
            media
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(error)?;
            if output {
                media
                    .SetUINT32(&MF_MT_AVG_BITRATE, self.rate.0)
                    .map_err(error)?;
                media
                    .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                    .map_err(error)?;
                media.SetUINT32(&MF_MT_MPEG2_LEVEL, 31).map_err(error)?;
            } else {
                media
                    .SetUINT32(&MF_MT_DEFAULT_STRIDE, self.size.0)
                    .map_err(error)?;
                media
                    .SetUINT32(&MF_MT_SAMPLE_SIZE, self.nv12_len as u32)
                    .map_err(error)?;
                media
                    .SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)
                    .map_err(error)?;
                media
                    .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)
                    .map_err(error)?;
            }
        }
        Ok(media)
    }
    fn credit(&mut self, input: bool, deadline: Instant) -> Result<()> {
        loop {
            let credits = if input {
                &mut self.input_credits
            } else {
                &mut self.output_credits
            };
            if *credits > 0 {
                *credits -= 1;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error("硬件编码事件等待超时"));
            }
            // SAFETY: own worker's event source, nonblocking GetEvent only.
            let event = unsafe { self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) };
            match event {
                Ok(event) => {
                    // SAFETY: owned immutable event, no native memory escapes.
                    unsafe {
                        event.GetStatus().map_err(error)?.ok().map_err(error)?;
                        let kind = event.GetType().map_err(error)? as i32;
                        if kind == METransformNeedInput.0 {
                            self.input_credits = self
                                .input_credits
                                .checked_add(1)
                                .filter(|n| *n <= 64)
                                .ok_or_else(|| error("编码输入事件超量"))?;
                        } else if kind == METransformHaveOutput.0 {
                            self.output_credits = self
                                .output_credits
                                .checked_add(1)
                                .filter(|n| *n <= 64)
                                .ok_or_else(|| error("编码输出事件超量"))?;
                        }
                    }
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(e) => return Err(error(e)),
            }
        }
    }
    pub fn encode(
        &mut self,
        frame: &Frame,
        captured: Instant,
        force: bool,
        rate: (u32, u32),
    ) -> Result<EncodedFrame> {
        if frame.size() != self.size {
            return Err(error("画面尺寸已变化"));
        }
        if rate.1 != self.rate.1 {
            *self = Self::new(self.size, rate, self.profile)?;
        }
        if rate.0 != self.rate.0 {
            // SAFETY: correctly typed scalar property, owning initialized COM worker.
            unsafe {
                self.codec
                    .SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(rate.0))
                    .map_err(error)?;
            }
            self.rate.0 = rate.0;
        }
        let force = force || self.first;
        let deadline = Instant::now() + Duration::from_millis(800);
        self.credit(true, deadline)?;
        if force {
            // SAFETY: VT_UI4 force-IDR applies to the next ProcessInput on this worker.
            unsafe {
                self.codec
                    .SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32))
                    .map_err(error)?;
            }
        }
        self.yuv.read_rgba8(RgbaSliceU8::new(
            frame.image.as_raw(),
            (self.size.0 as usize, self.size.1 as usize),
        ));
        let buffer = self
            .runtime
            .api
            .buffer(self.input_bytes, self.input_alignment)?;
        {
            let mut locked = Locked::new(&buffer)?;
            locked.write_nv12(&self.yuv, self.nv12_len)?;
        }
        let sample: IMFSample = self.runtime.api.create(self.runtime.api.sample)?;
        self.timestamp = self
            .timestamp
            .checked_add(10_000_000 / i64::from(rate.1))
            .ok_or_else(|| error("视频时间戳溢出"))?;
        // SAFETY: owned, unlocked NV12 memory of exact sample length; sample and
        // input timestamps stay valid through asynchronous MFT ownership.
        unsafe {
            buffer
                .SetCurrentLength(self.nv12_len as u32)
                .map_err(error)?;
            sample.AddBuffer(&buffer).map_err(error)?;
            sample.SetSampleTime(self.timestamp).map_err(error)?;
            sample
                .SetSampleDuration(10_000_000 / i64::from(rate.1))
                .map_err(error)?;
            self.transform
                .ProcessInput(self.input_id, &sample, 0)
                .map_err(error)?;
        }
        self.credit(false, deadline)?;
        for _ in 0..4 {
            match self.output() {
                Ok((bytes, stamp)) => {
                    if stamp != self.timestamp {
                        return Err(error("编码输出与输入帧身份不符"));
                    }
                    self.sequence_header()?;
                    let encoded = self.headers.frame(
                        &bytes,
                        VideoProfile::ConstrainedBaseline31,
                        captured,
                    )?;
                    if force && !encoded.keyframe {
                        return Err(error("硬件编码器未确认关键帧"));
                    }
                    self.first = false;
                    return Ok(encoded);
                }
                Err(e) if e == "stream-change" => {
                    let out = self.media_type(MFVideoFormat_H264, true)?;
                    // SAFETY: same negotiated output geometry/profile; no input is replayed on a stream change.
                    unsafe {
                        self.transform
                            .SetOutputType(self.output_id, &out, 0)
                            .map_err(error)?;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(error("系统编码输出格式反复变化"))
    }
    fn sequence_header(&mut self) -> Result<()> {
        // SAFETY: current media type/attribute is COM-owned; blob is copied
        // into a bounded Vec, never trusted for allocation beyond 128 KiB.
        unsafe {
            let output = self
                .transform
                .GetOutputCurrentType(self.output_id)
                .map_err(error)?;
            if let Ok(size) = output.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                if size > 131072 {
                    return Err(error("编码参数超过预算"));
                }
                if size > 0 {
                    let mut bytes = vec![0; size as usize];
                    output
                        .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut bytes, None)
                        .map_err(error)?;
                    self.headers.observe(&bytes)?;
                }
            }
        }
        Ok(())
    }
    fn output(&self) -> Result<(Vec<u8>, i64)> {
        // SAFETY: every output sample/event is owned and released by Output,
        // even on HRESULT failure/format change. Buffers are size-bounded.
        unsafe {
            let info = self
                .transform
                .GetOutputStreamInfo(self.output_id)
                .map_err(error)?;
            let mut output = Output(MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.output_id,
                ..Default::default()
            });
            if info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                == 0
            {
                let sample: IMFSample = self.runtime.api.create(self.runtime.api.sample)?;
                sample
                    .AddBuffer(
                        &self
                            .runtime
                            .api
                            .buffer((info.cbSize as usize).max(self.nv12_len), info.cbAlignment)?,
                    )
                    .map_err(error)?;
                output.0.pSample = ManuallyDrop::new(Some(sample));
            }
            let mut status = 0;
            match self
                .transform
                .ProcessOutput(0, std::slice::from_mut(&mut output.0), &mut status)
            {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    return Err("stream-change".into());
                }
                Err(e) => return Err(error(e)),
            }
            let sample = output
                .0
                .pSample
                .as_ref()
                .ok_or_else(|| error("编码输出没有样本"))?;
            let stamp = sample.GetSampleTime().map_err(error)?;
            let length = sample.GetTotalLength().map_err(error)?;
            if length == 0 || length > 8 * 1024 * 1024 {
                return Err(error("编码输出超过预算"));
            }
            let buffer = sample.ConvertToContiguousBuffer().map_err(error)?;
            Ok((Locked::new(&buffer)?.bytes()?, stamp))
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: worker-only teardown, no sink/callback can escape this codec; interfaces/runtime stay alive during shutdown.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}
