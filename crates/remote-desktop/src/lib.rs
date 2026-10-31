//! Native capture/input/clipboard boundary. Protocol authorization lives in the host.
use base64::{Engine, engine::general_purpose::STANDARD};
use enigo::{Enigo, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, io::Cursor};

pub mod cursor;
#[cfg(windows)]
pub mod escape;

pub type Result<T> = std::result::Result<T, String>;
fn native(error: impl std::fmt::Display) -> String {
    format!("系统远控接口不可用，请检查屏幕录制、辅助功能及桌面权限：{error}")
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}
pub fn displays() -> Result<Vec<Display>> {
    xcap::Monitor::all()
        .map_err(native)?
        .into_iter()
        .map(|m| {
            Ok(Display {
                id: m.id().map_err(native)?,
                name: m.name().map_err(native)?,
                x: m.x().map_err(native)?,
                y: m.y().map_err(native)?,
                width: m.width().map_err(native)?,
                height: m.height().map_err(native)?,
                scale: m.scale_factor().map_err(native)?,
                primary: m.is_primary().map_err(native)?,
            })
        })
        .collect()
}
/// Normalised wire coordinate (0..=65535 across the display) → global OS
/// coordinates. Display bounds are logical points on macOS, so a Retina
/// frame of any pixel size maps to the same place.
pub fn to_global(display: &Display, x: u16, y: u16) -> (i32, i32) {
    let along = |origin: i32, extent: u32, value: u16| {
        let offset = (i64::from(value) * i64::from(extent.saturating_sub(1)) + 32767) / 65535;
        i32::try_from(i64::from(origin) + offset).unwrap_or(origin)
    };
    (
        along(display.x, display.width, x),
        along(display.y, display.height, y),
    )
}
/// Global OS coordinates → normalised display coordinates, `None` off-display.
pub fn to_normalized(display: &Display, x: i32, y: i32) -> Option<(u16, u16)> {
    let along = |origin: i32, extent: u32, value: i32| {
        let offset = i64::from(value) - i64::from(origin);
        let span = i64::from(extent.saturating_sub(1)).max(1);
        (0..=span)
            .contains(&offset)
            .then(|| u16::try_from((offset * 65535 + span / 2) / span).unwrap_or(u16::MAX))
    };
    Some((
        along(display.x, display.width, x)?,
        along(display.y, display.height, y)?,
    ))
}

/// One display's capture source. Keeping the monitor handle avoids
/// re-enumerating displays and re-arming capture on every frame (~55 ms on a
/// Retina Mac), and the last frame's digest lets an unchanged screen skip
/// scaling output and JPEG encoding entirely.
pub struct Capturer {
    pub display: Display,
    source: CaptureBackend,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    picture: video::Picture,
}
enum CaptureBackend {
    Native(xcap::Monitor),
    #[cfg(target_os = "linux")]
    Portal(std::sync::Arc<dyn video::ScreenSource>),
}
impl Capturer {
    pub fn new(id: u32) -> Result<Self> {
        let monitor = xcap::Monitor::all()
            .map_err(native)?
            .into_iter()
            .find(|m| m.id().is_ok_and(|value| value == id))
            .ok_or("显示器已断开，请刷新屏幕列表")?;
        let display = displays()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or("显示器已断开，请刷新屏幕列表")?;
        #[cfg(target_os = "macos")]
        let picture = video::Picture::new(id, std::sync::Arc::new(|| true));
        #[cfg(target_os = "windows")]
        let picture = video::Picture::new(id, display.name.clone(), std::sync::Arc::new(|| true));
        Ok(Self {
            display,
            source: CaptureBackend::Native(monitor),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            picture,
        })
    }
    /// Stop a no-longer-used image stream while video supplies the screen.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn release_idle_picture(&mut self) {
        self.picture.idle();
    }
    /// The original desktop lease, also checked after native preparation.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn new_owned(
        id: u32,
        alive: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        let mut capturer = Self::new(id)?;
        #[cfg(target_os = "macos")]
        {
            capturer.picture = video::Picture::new(id, alive);
        }
        #[cfg(target_os = "windows")]
        {
            capturer.picture = video::Picture::new(id, capturer.display.name.clone(), alive);
        }
        Ok(capturer)
    }
    #[cfg(target_os = "linux")]
    pub fn from_portal(session: &wayland::Session, id: u32) -> Result<Self> {
        let mut display = session
            .displays()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or("屏幕不在 Wayland 授权范围内")?;
        let source = session.capture(id)?;
        display.scale = source.size()?.0 as f32 / display.width as f32;
        Ok(Self {
            display,
            source: CaptureBackend::Portal(source),
        })
    }
    pub fn video_source(&self) -> video::Source {
        match &self.source {
            CaptureBackend::Native(_) => None,
            #[cfg(target_os = "linux")]
            CaptureBackend::Portal(source) => Some(source.clone()),
        }
    }
    fn check_current(&self) -> Result<()> {
        require_unlocked()?;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.picture.check_owner()?;
        #[cfg(target_os = "linux")]
        if let CaptureBackend::Portal(source) = &self.source {
            source.check_active()?;
        }
        Ok(())
    }
    fn check_picture(&self, cursor_embedded: Option<bool>) -> Result<()> {
        self.check_current()?;
        #[cfg(target_os = "macos")]
        if cursor_embedded == Some(true) {
            self.picture.check_current()?;
        }
        #[cfg(target_os = "windows")]
        self.picture.check_current()?;
        #[cfg(not(target_os = "macos"))]
        let _ = cursor_embedded;
        Ok(())
    }
    /// `known` is the frame id the client is still showing; a matching
    /// capture answers `unchanged` without image bytes.
    pub fn frame(&mut self, max_width: u32, quality: u8, known: Option<&str>) -> Result<Value> {
        self.check_current()?;
        if !(320..=2560).contains(&max_width) || !(30..=90).contains(&quality) {
            return Err("画面尺寸或质量无效".into());
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let current = displays()?
                .into_iter()
                .find(|d| d.id == self.display.id)
                .ok_or("显示器已断开，请刷新屏幕列表")?;
            if (
                current.x,
                current.y,
                current.width,
                current.height,
                current.scale.to_bits(),
            ) != (
                self.display.x,
                self.display.y,
                self.display.width,
                self.display.height,
                self.display.scale.to_bits(),
            ) {
                self.picture.idle();
            }
            self.display = current;
        }
        let (pixels, cursor_embedded) = match &self.source {
            CaptureBackend::Native(monitor) => {
                #[cfg(target_os = "macos")]
                if let Some(pixels) = self.picture.frame()? {
                    (std::sync::Arc::new(pixels), Some(true))
                } else {
                    // SCK is optional for JPEG; the screenshot fallback keeps
                    // an explicit separate pointer overlay mode.
                    self.check_current()?;
                    (
                        std::sync::Arc::new(monitor.capture_image().map_err(native)?),
                        Some(false),
                    )
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = monitor;
                    self.picture.frame()?
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    (
                        std::sync::Arc::new(monitor.capture_image().map_err(native)?),
                        Some(false),
                    )
                }
            }
            #[cfg(target_os = "linux")]
            CaptureBackend::Portal(source) => (source.frame()?, Some(source.cursor_embedded())),
        };
        let image = picture_thumbnail(pixels.as_ref(), max_width)?;
        let id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (image.width(), image.height(), quality, image.as_raw()).hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        // Never publish a frame captured during a transition to the lock screen.
        self.check_picture(cursor_embedded)?;
        if known == Some(id.as_str()) {
            return Ok(
                json!({"monitorId":self.display.id,"frameId":id,"unchanged":true,
                "cursorEmbedded":cursor_embedded}),
            );
        }
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&image)
            .map_err(native)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("画面过大，请降低画质".into());
        }
        let bytes = STANDARD.encode(bytes);
        // JPEG/base64 work may outlive the previous check just like a native
        // video encode. Revalidate immediately before returning pixels.
        self.check_picture(cursor_embedded)?;
        Ok(
            json!({"monitorId":self.display.id,"frameId":id,"width":image.width(),"height":image.height(),
            "pointWidth":self.display.width,"pointHeight":self.display.height,"scale":self.display.scale,
            "cursorEmbedded":cursor_embedded,
            "mimeType":"image/jpeg","bytes":bytes}),
        )
    }
}
/// Borrow the retained source rather than cloning a full native-size picture.
fn picture_thumbnail(pixels: &image::RgbaImage, max_width: u32) -> Result<image::RgbImage> {
    let (width, height) = pixels.dimensions();
    if width == 0 || height == 0 {
        return Err("屏幕图像尺寸无效".into());
    }
    let ratio = (f64::from(max_width) / f64::from(width)).min(1440.0 / f64::from(height));
    let size = (
        (f64::from(width) * ratio).round().max(1.0) as u32,
        (f64::from(height) * ratio).round().max(1.0) as u32,
    );
    Ok(
        image::DynamicImage::ImageRgba8(image::imageops::thumbnail(pixels, size.0, size.1))
            .to_rgb8(),
    )
}
pub fn frame(id: u32, max_width: u32, quality: u8) -> Result<Value> {
    Capturer::new(id)?.frame(max_width, quality, None)
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Input {
    Move { x: u16, y: u16 },
    Button { button: String, direction: String },
    Key { key: String, direction: String },
    Text { text: String },
    Scroll { x: i16, y: i16 },
}
mod input_geometry;
struct NativeController {
    geometry: input_geometry::Resolver,
    pointer: input_geometry::Pointer,
    input: Enigo,
    buttons: HashSet<enigo::Button>,
}
impl NativeController {
    pub fn new() -> Result<Self> {
        require_unlocked()?;
        Ok(Self {
            geometry: input_geometry::Resolver::new(),
            pointer: input_geometry::Pointer::default(),
            input: Enigo::new(&Settings {
                open_prompt_to_get_permissions: false,
                release_keys_when_dropped: true,
                ..Settings::default()
            })
            .map_err(native)?,
            buttons: HashSet::new(),
        })
    }
    /// Pointer position on `display`, normalised; `None` when it is elsewhere.
    pub fn cursor(&self, display: &Display) -> Option<(u16, u16)> {
        let _context = input_geometry::Context::enter().ok()?;
        let display = self.geometry.current(display).ok()?;
        let (x, y) = self.input.location().ok()?;
        to_normalized(&display, x, y)
    }
    /// Revalidate lock state after a positional query. One current geometry
    /// snapshot covers the batch; owner retirement stops subsequent actions.
    pub fn send_batch(
        &mut self,
        display: &Display,
        events: Vec<Input>,
        active: &dyn Fn() -> bool,
    ) -> Result<()> {
        require_unlocked()?;
        let _context = input_geometry::Context::enter()?;
        // Resolve before mutable injection; one snapshot covers the ordered batch.
        let current = if input_geometry::needs_bounds(&events) {
            Some(self.geometry.current(display))
        } else {
            None
        };
        if current.is_some() {
            require_unlocked()?;
        }
        input_geometry::batch_guarded(
            display,
            events,
            |_| current.unwrap_or_else(|| Err("缺少当前屏幕坐标".into())),
            active,
            |display, event| self.apply(display, event, active),
        )
    }
    fn reproject(&mut self, display: &Display) -> Result<()> {
        if let Some((x, y)) = self.pointer.changed(display) {
            let (global_x, global_y) = to_global(display, x, y);
            self.geometry.move_to(&mut self.input, global_x, global_y)?;
            self.pointer.remember(display, x, y);
        }
        Ok(())
    }
    fn apply(&mut self, display: &Display, event: Input, active: &dyn Fn() -> bool) -> Result<()> {
        let direction = |raw: &str| match raw {
            "press" => Ok(enigo::Direction::Press),
            "release" => Ok(enigo::Direction::Release),
            "click" => Ok(enigo::Direction::Click),
            _ => Err("输入方向无效".to_string()),
        };
        match event {
            Input::Move { x, y } => {
                let (normalized_x, normalized_y) = (x, y);
                let (x, y) = to_global(display, x, y);
                self.geometry.move_to(&mut self.input, x, y)?;
                self.pointer.remember(display, normalized_x, normalized_y);
                Ok(())
            }
            Input::Button {
                button,
                direction: raw,
            } => {
                let button = match button.as_str() {
                    "left" => enigo::Button::Left,
                    "right" => enigo::Button::Right,
                    "middle" => enigo::Button::Middle,
                    _ => return Err("鼠标按钮无效".into()),
                };
                let dir = direction(&raw)?;
                if dir != enigo::Direction::Release {
                    self.reproject(display)?;
                }
                if !active() {
                    return Err("输入请求已结束".into());
                }
                self.input.button(button, dir).map_err(native)?;
                if dir == enigo::Direction::Press {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
                Ok(())
            }
            Input::Key {
                key,
                direction: raw,
            } => {
                let key = match key.as_str() {
                    "ctrl" => enigo::Key::Control,
                    "alt" => enigo::Key::Alt,
                    "shift" => enigo::Key::Shift,
                    "meta" => enigo::Key::Meta,
                    "enter" => enigo::Key::Return,
                    "tab" => enigo::Key::Tab,
                    "escape" => enigo::Key::Escape,
                    "backspace" => enigo::Key::Backspace,
                    "delete" => enigo::Key::Delete,
                    "left" => enigo::Key::LeftArrow,
                    "right" => enigo::Key::RightArrow,
                    "up" => enigo::Key::UpArrow,
                    "down" => enigo::Key::DownArrow,
                    "home" => enigo::Key::Home,
                    "end" => enigo::Key::End,
                    "pageUp" => enigo::Key::PageUp,
                    "pageDown" => enigo::Key::PageDown,
                    "space" => enigo::Key::Space,
                    "capsLock" => enigo::Key::CapsLock,
                    "f1" => enigo::Key::F1,
                    "f2" => enigo::Key::F2,
                    "f3" => enigo::Key::F3,
                    "f4" => enigo::Key::F4,
                    "f5" => enigo::Key::F5,
                    "f6" => enigo::Key::F6,
                    "f7" => enigo::Key::F7,
                    "f8" => enigo::Key::F8,
                    "f9" => enigo::Key::F9,
                    "f10" => enigo::Key::F10,
                    "f11" => enigo::Key::F11,
                    "f12" => enigo::Key::F12,
                    raw if raw.chars().count() == 1 => {
                        enigo::Key::Unicode(raw.chars().next().ok_or("按键无效")?)
                    }
                    _ => return Err("按键无效".into()),
                };
                self.input.key(key, direction(&raw)?).map_err(native)
            }
            Input::Text { text } => {
                if text.len() > 16384 {
                    return Err("输入文本过长".into());
                }
                self.input.text(&text).map_err(native)
            }
            Input::Scroll { x, y } => {
                if x.unsigned_abs() > 100 || y.unsigned_abs() > 100 {
                    return Err("滚动距离过大".into());
                }
                self.reproject(display)?;
                if !active() {
                    return Err("输入请求已结束".into());
                }
                self.input
                    .scroll(i32::from(x), enigo::Axis::Horizontal)
                    .map_err(native)?;
                if !active() {
                    return Err("输入请求已结束".into());
                }
                self.input
                    .scroll(i32::from(y), enigo::Axis::Vertical)
                    .map_err(native)
            }
        }
    }
}
impl Drop for NativeController {
    fn drop(&mut self) {
        for button in self.buttons.drain() {
            let _ = self.input.button(button, enigo::Direction::Release);
        }
    }
}

pub struct Controller {
    backend: InputBackend,
}
enum InputBackend {
    Native(Box<NativeController>),
    #[cfg(target_os = "linux")]
    Portal(wayland::Session),
}
impl Controller {
    pub fn new() -> Result<Self> {
        Ok(Self {
            backend: InputBackend::Native(Box::new(NativeController::new()?)),
        })
    }
    #[cfg(target_os = "linux")]
    pub fn from_portal(session: wayland::Session) -> Self {
        Self {
            backend: InputBackend::Portal(session),
        }
    }
    pub fn cursor(&self, display: &Display) -> Option<(u16, u16)> {
        match &self.backend {
            InputBackend::Native(input) => input.cursor(display),
            #[cfg(target_os = "linux")]
            InputBackend::Portal(_) => None,
        }
    }
    pub fn send(&mut self, display: &Display, event: Input) -> Result<()> {
        self.send_batch(display, vec![event])
    }
    pub fn send_batch(&mut self, display: &Display, events: Vec<Input>) -> Result<()> {
        self.send_batch_guarded(display, events, || true)
    }
    pub fn send_batch_guarded(
        &mut self,
        display: &Display,
        events: Vec<Input>,
        active: impl Fn() -> bool,
    ) -> Result<()> {
        if !active() {
            return Err("输入请求已结束".into());
        }
        match &mut self.backend {
            InputBackend::Native(input) => input.send_batch(display, events, &active),
            #[cfg(target_os = "linux")]
            InputBackend::Portal(session) => {
                require_unlocked()?;
                session.send(display.id, events)
            }
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let InputBackend::Portal(session) = &self.backend {
            session.release_held();
        }
    }
}

/// Resource limits for one manually requested image; never a connection traffic cap.
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CLIPBOARD_IMAGE_PIXELS: u64 = 8 * 1024 * 1024;

pub fn clipboard_image_get_png() -> Result<Vec<u8>> {
    require_unlocked()?;
    let image = arboard::Clipboard::new()
        .map_err(|_| "无法访问电脑剪贴板，请检查桌面会话和剪贴板权限")?
        .get_image()
        .map_err(|_| "无法读取电脑剪贴板图片，请重新复制图片并允许剪贴板访问")?;
    let width = u32::try_from(image.width).map_err(native)?;
    let height = u32::try_from(image.height).map_err(native)?;
    image_dimensions(width, height)?;
    let image = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .ok_or("剪贴板图片格式无效")?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(native)?;
    if out.get_ref().len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Err("剪贴板图片超过 16 MB，请缩小图片后重试".into());
    }
    Ok(out.into_inner())
}
fn image_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > MAX_CLIPBOARD_IMAGE_PIXELS
    {
        return Err("剪贴板图片超过 800 万像素，请缩小图片后重试".into());
    }
    Ok(())
}
/// Validate the encoded size and dimensions before allocating decoded pixels.
pub fn decode_clipboard_png(bytes: &[u8]) -> Result<image::RgbaImage> {
    if bytes.len() > MAX_CLIPBOARD_IMAGE_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("剪贴板图片格式或大小无效".into());
    }
    let (width, height) =
        image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(native)?;
    image_dimensions(width, height)?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode().map_err(native)?.to_rgba8())
}
pub fn clipboard_image_set_png(bytes: &[u8]) -> Result<arboard::Clipboard> {
    clipboard_image_set_png_if_current(bytes, &|| true)
}

/// Recheck the owner after expensive decoding, immediately before publishing.
/// Cancellation cannot undo a platform write that already entered the OS.
pub fn clipboard_image_set_png_if_current(
    bytes: &[u8],
    current: &dyn Fn() -> bool,
) -> Result<arboard::Clipboard> {
    if !current() {
        return Err("剪贴板传输已取消，未写入图片".into());
    }
    require_unlocked()?;
    let image = decode_clipboard_png(bytes)?;
    let mut clipboard = arboard::Clipboard::new().map_err(native)?;
    require_unlocked()?;
    if !current() {
        return Err("剪贴板传输已取消，未写入图片".into());
    }
    clipboard
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Owned(image.into_raw()),
        })
        .map_err(native)?;
    Ok(clipboard)
}

pub fn clipboard_get(kind: &str) -> Result<Value> {
    require_unlocked()?;
    let mut clipboard = arboard::Clipboard::new().map_err(native)?;
    match kind {
        "text" => {
            let text = clipboard.get_text().map_err(native)?;
            if text.len() > 1024 * 1024 {
                return Err("剪贴板文本超过 1 MB".into());
            }
            Ok(json!({"kind":"text","text":text}))
        }
        "image" => {
            let image = clipboard.get_image().map_err(native)?;
            if image.bytes.len() > 32 * 1024 * 1024 {
                return Err("剪贴板图片过大".into());
            }
            let image = image::RgbaImage::from_raw(
                u32::try_from(image.width).map_err(native)?,
                u32::try_from(image.height).map_err(native)?,
                image.bytes.into_owned(),
            )
            .ok_or("剪贴板图片格式无效")?;
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .map_err(native)?;
            if bytes.get_ref().len() > 4 * 1024 * 1024 {
                return Err("剪贴板图片超过传输限制".into());
            }
            Ok(
                json!({"kind":"image","mimeType":"image/png","bytes":STANDARD.encode(bytes.into_inner())}),
            )
        }
        _ => Err("剪贴板类型无效".into()),
    }
}
pub fn clipboard_set(value: Value) -> Result<arboard::Clipboard> {
    require_unlocked()?;
    let mut clipboard = arboard::Clipboard::new().map_err(native)?;
    match value["kind"].as_str() {
        Some("text") => {
            let text = value["text"].as_str().ok_or("缺少剪贴板文本")?;
            if text.len() > 1024 * 1024 {
                return Err("剪贴板文本超过 1 MB".into());
            }
            clipboard.set_text(text).map_err(native)
        }
        Some("image") => {
            let encoded = value["bytes"].as_str().ok_or("缺少剪贴板图片")?;
            if encoded.len() > 6 * 1024 * 1024 {
                return Err("剪贴板图片过大".into());
            }
            let bytes = STANDARD.decode(encoded).map_err(native)?;
            let mut reader = image::ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .map_err(native)?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(32 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().map_err(native)?.to_rgba8();
            clipboard
                .set_image(arboard::ImageData {
                    width: image.width() as usize,
                    height: image.height() as usize,
                    bytes: std::borrow::Cow::Owned(image.into_raw()),
                })
                .map_err(native)
        }
        _ => Err("剪贴板类型无效".into()),
    }?;
    Ok(clipboard)
}
pub use arboard::Clipboard;
pub use keepawake::KeepAwake as Awake;
pub fn keep_awake() -> Result<keepawake::KeepAwake> {
    keepawake::Builder::default()
        .idle(true)
        .display(false)
        .sleep(false)
        .app_name("DogCodes")
        .app_reverse_domain("ai.owlfy.dogcodes")
        .reason("Agent task or remote desktop in progress")
        .create()
        .map_err(native)
}

mod session;
pub mod video;
pub use session::locked;
pub fn require_unlocked() -> Result<()> {
    if locked()? {
        Err("电脑已锁定，Agent 操作仍可用".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn borrowed_picture_thumbnail_preserves_the_existing_pixels_and_aspect() {
        for (width, height, limit) in [
            (640, 360, 320),
            (360, 640, 480),
            (1, 999, 320),
            (999, 1, 2560),
            (10, 10, 320),
        ] {
            let image = image::RgbaImage::from_fn(width, height, |x, y| {
                image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 128])
            });
            let original = image::DynamicImage::ImageRgba8(image.clone())
                .thumbnail(limit, 1440)
                .to_rgb8();
            assert_eq!(picture_thumbnail(&image, limit).unwrap(), original);
        }
        assert!(picture_thumbnail(&image::RgbaImage::new(0, 1), 320).is_err());
    }
    fn retina() -> Display {
        // A 2x panel: frames are 3456 px wide, OS coordinates are 1728 points.
        Display {
            id: 2,
            name: "built-in".into(),
            x: -1728,
            y: 120,
            width: 1728,
            height: 1117,
            scale: 2.0,
            primary: false,
        }
    }
    #[test]
    fn wire_coordinates_cover_the_display_in_points() {
        let d = retina();
        assert_eq!(to_global(&d, 0, 0), (-1728, 120));
        assert_eq!(to_global(&d, 65535, 65535), (-1, 120 + 1116));
        assert_eq!(to_global(&d, 32768, 32768), (-1728 + 864, 120 + 558));
    }
    #[test]
    fn cursor_round_trips_and_is_hidden_off_display() {
        let d = retina();
        for (x, y) in [(-1728, 120), (-900, 600), (-1, 1236)] {
            let (nx, ny) = to_normalized(&d, x, y).expect("on display");
            assert_eq!(to_global(&d, nx, ny), (x, y));
        }
        assert_eq!(to_normalized(&d, 0, 600), None);
        assert_eq!(to_normalized(&d, -900, 119), None);
    }
}

#[cfg(test)]
mod clipboard_image_tests {
    use super::*;
    #[test]
    fn cancelled_image_write_does_not_open_the_os_clipboard() {
        assert!(
            clipboard_image_set_png_if_current(b"not decoded", &|| false)
                .err()
                .unwrap()
                .contains("已取消")
        );
    }
    #[test]
    fn png_validation_rejects_invalid_format_dimensions_and_size_without_touching_os() {
        assert!(decode_clipboard_png(b"not png").is_err());
        assert!(decode_clipboard_png(&vec![0; MAX_CLIPBOARD_IMAGE_BYTES + 1]).is_err());
        assert!(image_dimensions(0, 1).is_err());
        assert!(image_dimensions(8192, 8192).is_err());
        assert!(image_dimensions(3840, 2160).is_ok());
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_clipboard_png(bytes.get_ref()).unwrap(), image);
        assert!(decode_clipboard_png(&bytes.get_ref()[..20]).is_err());
    }
}

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(all(test, not(target_os = "linux")))]
#[path = "wayland/geometry.rs"]
mod wayland_geometry;
#[cfg(all(test, not(target_os = "linux")))]
#[path = "wayland/pixels.rs"]
mod wayland_pixels;
