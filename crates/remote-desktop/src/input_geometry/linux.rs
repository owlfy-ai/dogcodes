use super::*;
use std::sync::OnceLock;
use x11rb::{
    connection::Connection,
    protocol::randr::{Connection as OutputConnection, ConnectionExt, SetConfig},
    protocol::xproto::ConnectionExt as _,
    rust_connection::RustConnection,
};
#[derive(Default)]
pub(crate) struct Resolver(OnceLock<Result<Native>>);
struct Native {
    connection: RustConnection,
    root: u32,
}
pub(crate) struct Context;
impl Context {
    pub fn enter() -> Result<Self> {
        Ok(Self)
    }
}
impl Resolver {
    pub fn new() -> Self {
        Self::default()
    }
    fn native(&self) -> Result<&Native> {
        self.0
            .get_or_init(|| {
                x11rb::connect(None)
                    .map(|(connection, screen)| {
                        let root = connection.setup().roots[screen].root;
                        Native { connection, root }
                    })
                    .map_err(crate::native)
            })
            .as_ref()
            .map_err(Clone::clone)
    }
    pub fn move_to(&self, _input: &mut enigo::Enigo, x: i32, y: i32) -> Result<()> {
        self.move_root(x, y)
    }
    fn move_root(&self, x: i32, y: i32) -> Result<()> {
        if x < 0 || y < 0 {
            return Err("X11 坐标超出根窗口范围".into());
        }
        let native = self.native()?;
        let x = i16::try_from(x).map_err(|_| "X11 横坐标超出协议范围")?;
        let y = i16::try_from(y).map_err(|_| "X11 纵坐标超出协议范围")?;
        // Specify this DISPLAY's root instead of the pointer's previous root.
        // Root-window pixels match the CRTC snapshot and ignore Xft DPI scaling.
        native
            .connection
            .warp_pointer(x11rb::NONE, native.root, 0, 0, 0, 0, x, y)
            .map_err(crate::native)?
            .check()
            .map_err(crate::native)
    }
    pub fn current(&self, display: &Display) -> Result<Display> {
        if crate::wayland::available() {
            return Err("Wayland 输入必须使用原授权会话".into());
        }
        self.read(display)
    }
    fn read(&self, display: &Display) -> Result<Display> {
        let connection = &self.native()?.connection;
        let output = connection
            .randr_get_output_info(display.id, x11rb::CURRENT_TIME)
            .map_err(crate::native)?
            .reply()
            .map_err(crate::native)?;
        if output.status != SetConfig::SUCCESS
            || output.connection != OutputConnection::CONNECTED
            || output.crtc == 0
        {
            return Err("显示器已断开或正在重新配置".into());
        }
        if String::from_utf8_lossy(&output.name) != display.name {
            return Err("显示器身份已变化，请重新选择屏幕".into());
        }
        let crtc = connection
            .randr_get_crtc_info(output.crtc, x11rb::CURRENT_TIME)
            .map_err(crate::native)?
            .reply()
            .map_err(crate::native)?;
        if crtc.status != SetConfig::SUCCESS || !crtc.outputs.contains(&display.id) {
            return Err("显示器配置已变化，请重试".into());
        }
        // XTest and cursor locations use root-window pixels, never Xft logical
        // coordinates. CRTC bounds already incorporate the active rotation.
        Bounds {
            x: i32::from(crtc.x),
            y: i32::from(crtc.y),
            width: u32::from(crtc.width),
            height: u32::from(crtc.height),
        }
        .apply(display)
    }
}

#[cfg(test)]
mod tests;
