//! D3D11 desktop duplication. COM/GPU objects are created, used and released
//! only by the bounded capture worker; owned pixels alone cross that boundary.
use super::{
    Result,
    capture_source::{Cancellation, Driver, Fault, PointerMetadata, bgra},
    scrgb::Converter,
    worker::Worker,
};
use image::RgbaImage;
use std::{
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
mod white_level;
use windows::{
    Win32::{
        Foundation::{E_ACCESSDENIED, E_INVALIDARG, HMODULE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    },
    core::Interface,
};
type Picture = std::result::Result<Option<Arc<RgbaImage>>, Fault>;
/// One pull controller shared by JPEG and continuous video. The controller's
/// workers retain their original native slots until actual destruction.
pub(super) fn capture(
    display: u32,
    name: String,
    cancelled: Cancellation,
    paused: Arc<AtomicBool>,
) -> impl FnMut() -> Result<super::software::Frame> {
    let fallback_name = name.clone();
    let native_cancel = cancelled.clone();
    let gdi_cancel = cancelled.clone();
    let mut gdi: Option<Gdi> = None;
    let mut source = super::capture_source::Source::new(
        move |budget| Backend::open(display, name.clone(), budget, native_cancel.clone()),
        move |budget| {
            let until = Instant::now() + budget;
            if gdi.is_none() {
                gdi = Some(Gdi::open(
                    display,
                    fallback_name.clone(),
                    budget,
                    gdi_cancel.clone(),
                )?);
            }
            gdi.as_mut()
                .ok_or("回退视频来源已结束")?
                .frame(until.saturating_duration_since(Instant::now()))
        },
        paused,
        super::capture_source::WallClock,
    )
    .controlled(cancelled);
    move || {
        let image = source.frame()?;
        Ok(super::software::Frame {
            image,
            cursor_embedded: source.cursor_embedded(),
        })
    }
}
pub(super) struct Backend {
    worker: Worker<(), (Picture, Option<bool>)>,
    cursor: Option<bool>,
}
pub(super) struct Gdi(Worker<(), Arc<RgbaImage>>);
impl Gdi {
    pub fn open(
        display: u32,
        name: String,
        budget: Duration,
        cancelled: Cancellation,
    ) -> Result<Self> {
        let starting = cancelled.clone();
        Worker::start_capture(
            move || {
                active(&starting)?;
                xcap::Monitor::all()
                    .map_err(|_| "无法枚举回退显示器")?
                    .into_iter()
                    .find(|m| {
                        m.id().ok() == Some(display)
                            && m.name().ok().as_deref() == Some(name.as_str())
                    })
                    .ok_or_else(|| "回退视频显示器已断开".into())
            },
            move |monitor, ()| {
                active(&cancelled)?;
                monitor
                    .capture_image()
                    .map(Arc::new)
                    .map_err(|_| "回退视频采集失败，请检查桌面权限".into())
            },
            budget.min(Duration::from_secs(2)),
            Duration::from_secs(2),
        )
        .map(Self)
    }
    pub fn frame(&mut self, budget: Duration) -> Result<Arc<RgbaImage>> {
        self.0.run_with_timeout((), budget)
    }
}
impl Backend {
    pub fn open(
        display: u32,
        name: String,
        budget: Duration,
        cancelled: Cancellation,
    ) -> Result<Self> {
        let starting = cancelled.clone();
        Worker::start_capture(
            move || {
                Ok(if starting.is_cancelled() {
                    Err(Fault::Denied)
                } else {
                    Native::open(display, &name)
                })
            },
            move |native, ()| {
                if cancelled.is_cancelled() {
                    return Ok((Err(Fault::Denied), None));
                }
                Ok(match native {
                    Ok(native) => {
                        let picture = native.poll();
                        (picture, native.cursor.embedded())
                    }
                    Err(fault) => (Err(*fault), None),
                })
            },
            budget.min(Duration::from_secs(2)),
            Duration::from_secs(2),
        )
        .map(|worker| Self {
            worker,
            cursor: None,
        })
    }
}
fn active(cancelled: &Cancellation) -> Result<()> {
    if cancelled.is_cancelled() {
        Err("视频采集请求已结束".into())
    } else {
        Ok(())
    }
}
impl Driver for Backend {
    fn poll(&mut self, budget: Duration) -> Picture {
        match self.worker.run_with_timeout((), budget) {
            Ok((result, cursor)) => {
                if result.is_ok() {
                    self.cursor = cursor;
                }
                result
            }
            Err(error) => {
                eprintln!("remote capture: {error}");
                Err(Fault::Unavailable)
            }
        }
    }
    fn cursor_embedded(&self) -> Option<bool> {
        self.cursor
    }
}
fn fault(error: windows::core::Error) -> Fault {
    let code = error.code();
    eprintln!("remote capture: DXGI/D3D11 failure {code:?}");
    if code == DXGI_ERROR_ACCESS_LOST
        || code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
        || code == E_INVALIDARG
    {
        Fault::Lost
    } else if code == E_ACCESSDENIED || code == DXGI_ERROR_SESSION_DISCONNECTED {
        Fault::Denied
    } else {
        Fault::Unavailable
    }
}
struct Com;
impl Com {
    fn start() -> std::result::Result<Self, Fault> {
        // SAFETY: this dedicated native thread owns its balanced COM apartment.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(fault)?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: one successful CoInitializeEx on this same native thread.
        unsafe {
            CoUninitialize();
        }
    }
}
struct Native {
    cursor: PointerMetadata,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory1,
    _device: ID3D11Device,
    format: DXGI_FORMAT,
    converter: Option<Converter>,
    white: Option<white_level::Target>,
    color_checked: Instant,
    size: (u32, u32),
    rotation: u32,
    _thread: Rc<()>,
    _com: Com,
}
impl Native {
    fn open(display: u32, name: &str) -> std::result::Result<Self, Fault> {
        let com = Com::start()?;
        // SAFETY: initialized output interfaces own COM references; enumeration
        // is bounded, and the selected output must match BOTH handle and name.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(fault)?;
            for a in 0..32 {
                let adapter = match factory.EnumAdapters1(a) {
                    Ok(adapter) => adapter,
                    Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(error) => return Err(fault(error)),
                };
                for o in 0..16 {
                    let output = match adapter.EnumOutputs(o) {
                        Ok(output) => output,
                        Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(error) => return Err(fault(error)),
                    };
                    let description = output.GetDesc().map_err(fault)?;
                    let end = description
                        .DeviceName
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(description.DeviceName.len());
                    if description.Monitor.0 as u32 != display
                        || String::from_utf16_lossy(&description.DeviceName[..end]) != name
                    {
                        continue;
                    }
                    if !description.AttachedToDesktop.as_bool() {
                        return Err(Fault::Denied);
                    }
                    let advanced = output.cast::<IDXGIOutput6>().ok();
                    let hdr = if let Some(output) = &advanced {
                        match output.GetDesc1().map_err(fault)?.ColorSpace {
                            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 => false,
                            DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 => true,
                            _ => return Err(Fault::Unavailable),
                        }
                    } else {
                        false
                    };
                    let white = if hdr {
                        Some(white_level::Target::find(
                            adapter.GetDesc1().map_err(fault)?.AdapterLuid,
                            &description.DeviceName,
                        )?)
                    } else {
                        None
                    };
                    let reference_white = match &white {
                        Some(target) => target.read()?,
                        None => 1000,
                    };
                    let adapter: IDXGIAdapter = adapter.cast().map_err(fault)?;
                    let (mut device, mut context) = (None, None);
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )
                    .map_err(fault)?;
                    let device = device.ok_or(Fault::Unavailable)?;
                    let context = context.ok_or(Fault::Unavailable)?;
                    let duplication = if let Some(output) = advanced {
                        match output.DuplicateOutput1(
                            &device,
                            0,
                            &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM],
                        ) {
                            Ok(duplication) => duplication,
                            Err(error)
                                if !hdr
                                    && matches!(
                                        error.code(),
                                        DXGI_ERROR_UNSUPPORTED | E_INVALIDARG
                                    ) =>
                            {
                                output.DuplicateOutput(&device).map_err(fault)?
                            }
                            Err(error) => return Err(fault(error)),
                        }
                    } else {
                        let output: IDXGIOutput1 = output.cast().map_err(fault)?;
                        output.DuplicateOutput(&device).map_err(fault)?
                    };
                    let desc = duplication.GetDesc();
                    let size = (desc.ModeDesc.Width, desc.ModeDesc.Height);
                    if size.0 == 0
                        || size.1 == 0
                        || u64::from(size.0) * u64::from(size.1) > 64 * 1024 * 1024
                        || !matches!(
                            desc.ModeDesc.Format,
                            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R16G16B16A16_FLOAT
                        )
                        || hdr && desc.ModeDesc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT
                    {
                        return Err(Fault::Unavailable);
                    }
                    let rotation = match desc.Rotation {
                        DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED => 0,
                        DXGI_MODE_ROTATION_ROTATE90 => 1,
                        DXGI_MODE_ROTATION_ROTATE180 => 2,
                        DXGI_MODE_ROTATION_ROTATE270 => 3,
                        _ => return Err(Fault::Unavailable),
                    };
                    let texture = D3D11_TEXTURE2D_DESC {
                        Width: size.0,
                        Height: size.1,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: desc.ModeDesc.Format,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut staging = None;
                    device
                        .CreateTexture2D(&texture, None, Some(&mut staging))
                        .map_err(fault)?;
                    return Ok(Self {
                        cursor: PointerMetadata::default(),
                        duplication,
                        staging: staging.ok_or(Fault::Unavailable)?,
                        context,
                        factory,
                        _device: device,
                        format: desc.ModeDesc.Format,
                        converter: (desc.ModeDesc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT)
                            .then(|| Converter::new(reference_white))
                            .transpose()
                            .map_err(|_| Fault::Unavailable)?,
                        white,
                        color_checked: Instant::now(),
                        size,
                        rotation,
                        _thread: Rc::new(()),
                        _com: com,
                    });
                }
            }
        }
        Err(Fault::Unavailable)
    }
    fn poll(&mut self) -> Picture {
        // SAFETY: this factory belongs to this same capture thread. A stale
        // factory's output description cannot identify the current HDR state.
        if !unsafe { self.factory.IsCurrent() }.as_bool() {
            return Err(Fault::Lost);
        }
        if self.color_checked.elapsed() >= Duration::from_secs(1) {
            if let (Some(target), Some(converter)) = (&self.white, &mut self.converter) {
                converter
                    .update(target.read()?)
                    .map_err(|_| Fault::Unavailable)?;
            }
            self.color_checked = Instant::now();
        }
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        // SAFETY: valid live duplication and initialized outputs. A finite wait
        // returns no-new-frame normally; its owner also enforces a call deadline.
        match unsafe {
            self.duplication
                .AcquireNextFrame(20, &mut info, &mut resource)
        } {
            Ok(()) => (),
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(error) => return Err(fault(error)),
        }
        let frame = Acquired(Some(&self.duplication));
        self.cursor.observe(
            info.LastMouseUpdateTime != 0,
            info.PointerPosition.Visible.as_bool(),
        );
        if info.LastPresentTime == 0 {
            frame.release()?;
            return Ok(None); // cursor-only update: no GPU copy/readback
        }
        let texture: ID3D11Texture2D = resource.ok_or(Fault::Unavailable)?.cast().map_err(fault)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: live queried texture, fully initialized descriptor output.
        unsafe {
            texture.GetDesc(&mut desc);
        }
        if (desc.Width, desc.Height) != self.size
            || desc.Format != self.format
            || desc.SampleDesc.Count != 1
        {
            return Err(Fault::Lost);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: matching staging texture and source. Mapping is bounded by
        // the actor deadline; the staging texture stays alive through Unmap.
        unsafe {
            self.context.CopyResource(&self.staging, &texture);
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(fault)?;
        }
        let mapping = Mapped(&self.context, &self.staging);
        let pixel_bytes = if self.converter.is_some() { 8 } else { 4 };
        let row = self.size.0 as usize * pixel_bytes;
        let pitch = mapped.RowPitch as usize;
        let length = pitch
            .checked_mul(self.size.1.saturating_sub(1) as usize)
            .and_then(|n| n.checked_add(row))
            .filter(|n| *n <= 512 * 1024 * 1024)
            .ok_or(Fault::Unavailable)?;
        if mapped.pData.is_null()
            || pitch < row
            || mapped.DepthPitch != 0 && (mapped.DepthPitch as usize) < length
        {
            return Err(Fault::Unavailable);
        }
        // SAFETY: successful Map guarantees the texture's rows/RowPitch remain
        // readable until Unmap. Dimensions/stride/length were bounded above;
        // normalization copies to owned pixels before either native release.
        let bytes = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), length) };
        let image = match &self.converter {
            Some(converter) => {
                converter.frame(bytes, self.size.0, self.size.1, pitch, self.rotation)
            }
            None => bgra(bytes, self.size.0, self.size.1, pitch, self.rotation),
        }
        .map_err(|_| Fault::Unavailable)?;
        drop(mapping);
        frame.release()?;
        // SAFETY: the same live factory; a mode/color transition during the
        // readback must not publish pixels normalized with its stale state.
        if !unsafe { self.factory.IsCurrent() }.as_bool() {
            return Err(Fault::Lost);
        }
        Ok(Some(Arc::new(image)))
    }
}
struct Acquired<'a>(Option<&'a IDXGIOutputDuplication>);
impl Acquired<'_> {
    fn release(mut self) -> std::result::Result<(), Fault> {
        // SAFETY: exactly one successful AcquireNextFrame; take prevents a
        // second ReleaseFrame from Drop even when native release reports lost.
        unsafe {
            self.0
                .take()
                .expect("acquired frame")
                .ReleaseFrame()
                .map_err(fault)
        }
    }
}
impl Drop for Acquired<'_> {
    fn drop(&mut self) {
        if let Some(duplication) = self.0.take() {
            // SAFETY: cleanup of this still-acquired frame on every early return.
            let _ = unsafe { duplication.ReleaseFrame() };
        }
    }
}
struct Mapped<'a>(&'a ID3D11DeviceContext, &'a ID3D11Texture2D);
impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        // SAFETY: one successful Map of subresource zero on this same native thread.
        unsafe {
            self.0.Unmap(self.1, 0);
        }
    }
}
