use super::*;
use ::windows::Win32::{
    Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW},
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::SetCursorPos,
    },
};
#[derive(Default)]
pub(crate) struct Resolver;
/// Only this worker's thread changes units; never alter process/UI DPI mode.
pub(crate) struct Context(DPI_AWARENESS_CONTEXT);
impl Context {
    pub fn enter() -> Result<Self> {
        // SAFETY: a predefined context, applied only on the current worker.
        let old =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if old.0.is_null() {
            Err("无法确认输入线程的屏幕坐标单位".into())
        } else {
            Ok(Self(old))
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: restore exactly the valid context returned on this thread.
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}
impl Resolver {
    pub fn new() -> Self {
        Self
    }
    pub fn current(&self, display: &Display) -> Result<Display> {
        let monitor = xcap::Monitor::all()
            .map_err(crate::native)?
            .into_iter()
            .find(|m| m.id().is_ok_and(|id| id == display.id))
            .ok_or("显示器已断开")?;
        let name = monitor.name().map_err(crate::native)?;
        if name != display.name {
            return Err("显示器身份已变化，请重新选择屏幕".into());
        }
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..DEVMODEW::default()
        };
        // SAFETY: NUL-terminated device name and correctly sized output object.
        if !unsafe {
            EnumDisplaySettingsW(
                ::windows::core::PCWSTR(name.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            )
        }
        .as_bool()
        {
            return Err("无法读取当前显示器配置".into());
        }
        // SAFETY: ENUM_CURRENT_SETTINGS supplies the display-position union.
        let p = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
        Bounds {
            x: p.x,
            y: p.y,
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
        }
        .apply(display)
    }
    pub fn move_to(&self, _input: &mut enigo::Enigo, x: i32, y: i32) -> Result<()> {
        // SAFETY: coordinates were validated in physical desktop pixels under the
        // current thread's DPI context; SetCursorPos handles negative/multi-screen
        // origins instead of mapping everything through Enigo's primary display.
        unsafe { SetCursorPos(x, y) }.map_err(crate::native)
    }
}
