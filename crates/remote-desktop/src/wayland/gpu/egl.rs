//! Optional EGL/GLES, confined to the capture worker and private offscreen surfaces.
use super::{Capability, FORMATS};
use crate::Result;
use image::RgbaImage;
use khronos_egl as egl;
use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    rc::Rc,
    sync::{Mutex, OnceLock},
};
type Egl = egl::DynamicInstance<egl::EGL1_4>;
type Ptr = *mut c_void;
type CreateImage = unsafe extern "system" fn(Ptr, Ptr, u32, Ptr, *const i32) -> Ptr;
type DestroyImage = unsafe extern "system" fn(Ptr, Ptr) -> u32;

macro_rules! function {
    ($api:expr, $name:expr, $ty:ty) => {{
        let address = $api
            .get_proc_address($name)
            .ok_or_else(|| format!("GPU 接口不可用：{}", $name))?;
        // SAFETY: each private call site supplies the exact Khronos ABI for this
        // symbol. Its EGL library remains owned by the context using the pointer.
        unsafe { std::mem::transmute::<extern "system" fn(), $ty>(address) }
    }};
}
macro_rules! gl_api {
    ($($field:ident: $name:literal ($($arg:ty),*) -> $ret:ty;)*) => {
        struct Gl { $( $field: unsafe extern "system" fn($($arg),*) -> $ret, )* }
        impl Gl {
            fn load(api: &Egl) -> Result<Self> {
                Ok(Self { $( $field: function!(api, $name, unsafe extern "system" fn($($arg),*) -> $ret), )* })
            }
        }
    };
}
gl_api! {
    error: "glGetError" () -> u32;
    string: "glGetString" (u32) -> *const u8;
    integer: "glGetIntegerv" (u32, *mut i32) -> ();
    create_shader: "glCreateShader" (u32) -> u32;
    shader_source: "glShaderSource" (u32, i32, *const *const c_char, *const i32) -> ();
    compile: "glCompileShader" (u32) -> ();
    shader_status: "glGetShaderiv" (u32, u32, *mut i32) -> ();
    delete_shader: "glDeleteShader" (u32) -> ();
    create_program: "glCreateProgram" () -> u32;
    attach: "glAttachShader" (u32, u32) -> ();
    bind_attribute: "glBindAttribLocation" (u32, u32, *const c_char) -> ();
    link: "glLinkProgram" (u32) -> ();
    program_status: "glGetProgramiv" (u32, u32, *mut i32) -> ();
    delete_program: "glDeleteProgram" (u32) -> ();
    use_program: "glUseProgram" (u32) -> ();
    gen_buffers: "glGenBuffers" (i32, *mut u32) -> ();
    bind_buffer: "glBindBuffer" (u32, u32) -> ();
    buffer_data: "glBufferData" (u32, isize, *const c_void, u32) -> ();
    delete_buffers: "glDeleteBuffers" (i32, *const u32) -> ();
    attribute_pointer: "glVertexAttribPointer" (u32, i32, u32, u8, i32, *const c_void) -> ();
    enable_attribute: "glEnableVertexAttribArray" (u32) -> ();
    gen_textures: "glGenTextures" (i32, *mut u32) -> ();
    bind_texture: "glBindTexture" (u32, u32) -> ();
    texture_parameter: "glTexParameteri" (u32, u32, i32) -> ();
    texture_image: "glTexImage2D" (u32, i32, i32, i32, i32, i32, u32, u32, *const c_void) -> ();
    delete_textures: "glDeleteTextures" (i32, *const u32) -> ();
    image_texture: "glEGLImageTargetTexture2DOES" (u32, Ptr) -> ();
    gen_framebuffers: "glGenFramebuffers" (i32, *mut u32) -> ();
    bind_framebuffer: "glBindFramebuffer" (u32, u32) -> ();
    framebuffer_texture: "glFramebufferTexture2D" (u32, u32, u32, u32, i32) -> ();
    framebuffer_status: "glCheckFramebufferStatus" (u32) -> u32;
    delete_framebuffers: "glDeleteFramebuffers" (i32, *const u32) -> ();
    viewport: "glViewport" (i32, i32, i32, i32) -> ();
    disable: "glDisable" (u32) -> ();
    draw: "glDrawArrays" (u32, i32, i32) -> ();
    read_pixels: "glReadPixels" (i32, i32, i32, i32, u32, u32, *mut c_void) -> ();
}

fn error(error: impl std::fmt::Display) -> String {
    format!("GPU 画面处理不可用：{error}")
}
fn extension(value: &CStr, name: &str) -> bool {
    value
        .to_bytes()
        .split(|b| *b == b' ')
        .any(|token| token == name.as_bytes())
}

struct Context {
    api: Rc<Egl>,
    display: egl::Display,
    context: Option<egl::Context>,
    surface: Option<egl::Surface>,
    initialized: bool,
}
// EGL caches displays across threads; eglInitialize is not a reference count.
// One stream terminating its display must not invalidate another stream's context.
static DISPLAYS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
impl Context {
    fn open(api: Rc<Egl>, display: Ptr) -> Result<Self> {
        if display.is_null() {
            return Err(error("无法创建离屏显示"));
        }
        // SAFETY: obtained from eglGetPlatformDisplayEXT for a valid enumerated
        // EGL device, or the test-only surfaceless platform. No native window.
        let display = unsafe { egl::Display::from_ptr(display) };
        let mut owner = Self {
            api,
            display,
            context: None,
            surface: None,
            initialized: false,
        };
        {
            let mut displays = DISPLAYS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(count) = displays.get_mut(&(display.as_ptr() as usize)) {
                *count += 1;
            } else {
                owner.api.initialize(display).map_err(error)?;
                displays.insert(display.as_ptr() as usize, 1);
            }
        }
        owner.initialized = true;
        owner.api.bind_api(egl::OPENGL_ES_API).map_err(error)?;
        let config = owner
            .api
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::PBUFFER_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES2_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::NONE,
                ],
            )
            .map_err(error)?
            .ok_or_else(|| error("缺少离屏配置"))?;
        owner.surface = Some(
            owner
                .api
                .create_pbuffer_surface(
                    display,
                    config,
                    &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE],
                )
                .map_err(error)?,
        );
        owner.context = Some(
            owner
                .api
                .create_context(
                    display,
                    config,
                    None,
                    &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
                )
                .map_err(error)?,
        );
        owner.current()?;
        Ok(owner)
    }
    fn current(&self) -> Result<()> {
        self.api
            .make_current(self.display, self.surface, self.surface, self.context)
            .map_err(error)
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }
        let _ = self.api.make_current(self.display, None, None, None);
        if let Some(context) = self.context {
            let _ = self.api.destroy_context(self.display, context);
        }
        if let Some(surface) = self.surface {
            let _ = self.api.destroy_surface(self.display, surface);
        }
        let mut displays = DISPLAYS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(count) = displays.get_mut(&(self.display.as_ptr() as usize)) {
            *count -= 1;
            if *count == 0 {
                displays.remove(&(self.display.as_ptr() as usize));
                let _ = self.api.terminate(self.display);
            }
        }
    }
}

pub(super) struct Device {
    context: Context,
    gl: Gl,
    create: CreateImage,
    destroy: DestroyImage,
    program: u32,
    shaders: Vec<u32>,
    vbo: u32,
    framebuffer: u32,
    target: u32,
    size: (u32, u32),
    maximum: u32,
    capabilities: Vec<Capability>,
}
struct Image<'a> {
    device: &'a Device,
    image: Ptr,
    texture: u32,
}
impl Drop for Image<'_> {
    fn drop(&mut self) {
        // SAFETY: this image is scoped to its current device's synchronous draw;
        // the PipeWire buffer/fds are still borrowed and its EGL library is alive.
        unsafe {
            if self.device.context.current().is_ok() {
                (self.device.gl.delete_textures)(1, &self.texture);
            }
            (self.device.destroy)(self.device.context.display.as_ptr(), self.image);
        }
    }
}
struct Texture<'a> {
    context: &'a Context,
    gl: &'a Gl,
    id: u32,
}
impl Drop for Texture<'_> {
    fn drop(&mut self) {
        // SAFETY: texture's creating context is current and outlives this guard.
        if self.context.current().is_ok() {
            // SAFETY: this texture's owning context is now current.
            unsafe {
                (self.gl.delete_textures)(1, &self.id);
            }
        }
    }
}
impl Device {
    fn open(context: Context) -> Result<Self> {
        let gl = Gl::load(&context.api)?;
        // SAFETY: a GLES2 context is current; GL owns this terminated extension string.
        let extensions = unsafe { (gl.string)(0x1F03) };
        if extensions.is_null() {
            return Err(error("缺少 GLES 扩展"));
        }
        // SAFETY: non-null GL extension string remains live with the current context.
        let extensions = unsafe { CStr::from_ptr(extensions.cast()) };
        if !extension(extensions, "GL_OES_EGL_image_external") {
            return Err(error("不支持外部纹理"));
        }
        let create = function!(context.api, "eglCreateImageKHR", CreateImage);
        let destroy = function!(context.api, "eglDestroyImageKHR", DestroyImage);
        let mut owner = Self {
            context,
            gl,
            create,
            destroy,
            program: 0,
            shaders: Vec::new(),
            vbo: 0,
            framebuffer: 0,
            target: 0,
            size: (0, 0),
            maximum: 0,
            capabilities: Vec::new(),
        };
        owner.setup()?;
        owner.self_test()?;
        Ok(owner)
    }
    fn check(&self) -> Result<()> {
        // SAFETY: only invoked while this private GLES context is current.
        let code = unsafe { (self.gl.error)() };
        if code == 0 {
            Ok(())
        } else {
            Err(error(format!("GL 0x{code:04x}")))
        }
    }
    fn shader(&mut self, kind: u32, source: &str) -> Result<u32> {
        let source = CString::new(source).map_err(error)?;
        // SAFETY: current GLES context, NUL-terminated shader and valid status output.
        unsafe {
            let shader = (self.gl.create_shader)(kind);
            if shader == 0 {
                return Err(error("无法创建着色器"));
            }
            self.shaders.push(shader);
            (self.gl.shader_source)(shader, 1, &source.as_ptr(), ptr::null());
            (self.gl.compile)(shader);
            let mut status = 0;
            (self.gl.shader_status)(shader, 0x8B81, &mut status);
            if status == 0 {
                return Err(error("着色器编译失败"));
            }
            Ok(shader)
        }
    }
    fn setup(&mut self) -> Result<()> {
        let vertex = self.shader(0x8B31, "attribute vec2 position; varying vec2 uv; void main() { uv=position*0.5+0.5; gl_Position=vec4(position,0.0,1.0); }")?;
        let fragment = self.shader(0x8B30, "#extension GL_OES_EGL_image_external : require\nprecision mediump float; varying vec2 uv; uniform samplerExternalOES source; void main() { gl_FragColor=vec4(texture2D(source,uv).rgb,1.0); }")?;
        let vertices: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        // SAFETY: owned shader/program handles and synchronous, correctly sized
        // vertex data; all generated names remain owned by this device's Drop.
        unsafe {
            self.program = (self.gl.create_program)();
            if self.program == 0 {
                return Err(error("无法创建画面处理程序"));
            }
            (self.gl.attach)(self.program, vertex);
            (self.gl.attach)(self.program, fragment);
            (self.gl.bind_attribute)(self.program, 0, c"position".as_ptr());
            (self.gl.link)(self.program);
            let mut status = 0;
            (self.gl.program_status)(self.program, 0x8B82, &mut status);
            if status == 0 {
                return Err(error("画面处理程序链接失败"));
            }
            (self.gl.gen_buffers)(1, &mut self.vbo);
            (self.gl.bind_buffer)(0x8892, self.vbo);
            (self.gl.buffer_data)(
                0x8892,
                std::mem::size_of_val(&vertices) as isize,
                vertices.as_ptr().cast(),
                0x88E4,
            );
            (self.gl.gen_framebuffers)(1, &mut self.framebuffer);
            (self.gl.gen_textures)(1, &mut self.target);
            let mut maximum = 0;
            (self.gl.integer)(0x0D33, &mut maximum);
            let mut viewport = [0; 2];
            (self.gl.integer)(0x0D3A, viewport.as_mut_ptr());
            maximum = maximum.min(viewport[0]).min(viewport[1]);
            if maximum < 2 {
                return Err(error("显卡画面尺寸无效"));
            }
            self.maximum = (maximum as u32).min(16384);
            (self.gl.disable)(0x0BD0); // Disable dithering for deterministic 8-bit readback.
        }
        self.check()
    }
    fn parameters(&self, target: u32) {
        // SAFETY: target is the current context's newly bound 2D/external texture.
        unsafe {
            (self.gl.texture_parameter)(target, 0x2801, 0x2600);
            (self.gl.texture_parameter)(target, 0x2800, 0x2600);
            (self.gl.texture_parameter)(target, 0x2802, 0x812F);
            (self.gl.texture_parameter)(target, 0x2803, 0x812F);
        }
    }
    fn resize(&mut self, size: (u32, u32)) -> Result<()> {
        self.context.current()?;
        if self.size == size {
            return Ok(());
        }
        if size.0 == 0
            || size.1 == 0
            || size.0 > self.maximum
            || size.1 > self.maximum
            || u64::from(size.0) * u64::from(size.1) > 64 * 1024 * 1024
        {
            return Err(error("画面尺寸超限"));
        }
        // SAFETY: owned handles, validated sizes, and a null allocation pointer
        // request storage only. The FBO is checked before any draw/readback.
        unsafe {
            (self.gl.bind_texture)(0x0DE1, self.target);
            self.parameters(0x0DE1);
            (self.gl.texture_image)(
                0x0DE1,
                0,
                0x1908,
                size.0 as i32,
                size.1 as i32,
                0,
                0x1908,
                0x1401,
                ptr::null(),
            );
            (self.gl.bind_framebuffer)(0x8D40, self.framebuffer);
            (self.gl.framebuffer_texture)(0x8D40, 0x8CE0, 0x0DE1, self.target, 0);
            if (self.gl.framebuffer_status)(0x8D40) != 0x8CD5 {
                return Err(error("离屏缓冲区不可用"));
            }
        }
        self.check()?;
        self.size = size;
        Ok(())
    }
    fn image(&self, context: Ptr, target: u32, source: Ptr, attrs: &[i32]) -> Result<Image<'_>> {
        // SAFETY: validated attributes are terminated, source is either borrowed
        // DMA-BUF fds (null context/source) or this device's owned test texture.
        let image = unsafe {
            (self.create)(
                self.context.display.as_ptr(),
                context,
                target,
                source,
                attrs.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(error("无法导入屏幕缓冲区"));
        }
        let mut owner = Image {
            device: self,
            image,
            texture: 0,
        };
        // SAFETY: valid EGLImage/current context; the guard frees both handles
        // on failure, after synchronous readback, and before a buffer is returned.
        unsafe {
            (self.gl.gen_textures)(1, &mut owner.texture);
            (self.gl.bind_texture)(0x8D65, owner.texture);
            self.parameters(0x8D65);
            (self.gl.image_texture)(0x8D65, image);
        }
        self.check()?;
        Ok(owner)
    }
    fn draw(&self) -> Result<RgbaImage> {
        let mut out = RgbaImage::new(self.size.0, self.size.1);
        // SAFETY: complete owned FBO/current context, four valid VBO vertices,
        // imported external texture, and exactly width*height*4 writable bytes.
        // ReadPixels completes GPU reads before the borrowed source is released.
        unsafe {
            (self.gl.bind_framebuffer)(0x8D40, self.framebuffer);
            (self.gl.viewport)(0, 0, self.size.0 as i32, self.size.1 as i32);
            (self.gl.use_program)(self.program);
            (self.gl.bind_buffer)(0x8892, self.vbo);
            (self.gl.enable_attribute)(0);
            (self.gl.attribute_pointer)(0, 2, 0x1406, 0, 8, ptr::null());
            (self.gl.draw)(5, 0, 4);
            (self.gl.read_pixels)(
                0,
                0,
                self.size.0 as i32,
                self.size.1 as i32,
                0x1908,
                0x1401,
                out.as_mut_ptr().cast(),
            );
        }
        self.check()?;
        Ok(out)
    }
    fn self_test(&mut self) -> Result<()> {
        self.resize((2, 2))?;
        let pixels: [u8; 16] = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut source = Texture {
            context: &self.context,
            gl: &self.gl,
            id: 0,
        };
        // SAFETY: current context and exact 2x2 RGBA bytes, copied synchronously.
        unsafe {
            (self.gl.gen_textures)(1, &mut source.id);
            (self.gl.bind_texture)(0x0DE1, source.id);
            self.parameters(0x0DE1);
            (self.gl.texture_image)(
                0x0DE1,
                0,
                0x1908,
                2,
                2,
                0,
                0x1908,
                0x1401,
                pixels.as_ptr().cast(),
            );
        }
        let _image = self.image(
            self.context.context.unwrap().as_ptr(),
            0x30B1,
            source.id as usize as Ptr,
            &[0x30D2, 1, egl::NONE],
        )?;
        let out = self.draw()?;
        if out.as_raw() != &pixels {
            return Err(error("显卡画面通道或方向校验失败"));
        }
        Ok(())
    }
    fn query_capabilities(&mut self) -> Result<()> {
        let extensions = self
            .context
            .api
            .query_string(Some(self.context.display), egl::EXTENSIONS)
            .map_err(error)?;
        if !extension(extensions, "EGL_EXT_image_dma_buf_import")
            || !extension(extensions, "EGL_EXT_image_dma_buf_import_modifiers")
        {
            return Ok(());
        }
        let formats = function!(
            self.context.api,
            "eglQueryDmaBufFormatsEXT",
            unsafe extern "system" fn(Ptr, i32, *mut i32, *mut i32) -> u32
        );
        let modifiers = function!(
            self.context.api,
            "eglQueryDmaBufModifiersEXT",
            unsafe extern "system" fn(Ptr, i32, i32, *mut u64, *mut u32, *mut i32) -> u32
        );
        let mut supported = [0; 512];
        let mut count = 0;
        // SAFETY: fixed output arrays/count capacity match the EGL extension ABI.
        if unsafe {
            formats(
                self.context.display.as_ptr(),
                512,
                supported.as_mut_ptr(),
                &mut count,
            )
        } == 0
            || !(0..=512).contains(&count)
        {
            return Ok(());
        }
        for (format, fourcc) in FORMATS {
            if !supported[..count as usize].contains(&(fourcc as i32)) {
                continue;
            }
            let mut values = [0; 256];
            let mut external = [0; 256];
            let mut count = 0;
            // SAFETY: arrays/count have exactly the advertised capacity. Both
            // ordinary and external-only modifiers use our external sampler.
            let queried = unsafe {
                modifiers(
                    self.context.display.as_ptr(),
                    fourcc as i32,
                    256,
                    values.as_mut_ptr(),
                    external.as_mut_ptr(),
                    &mut count,
                )
            } != 0
                && (0..=256).contains(&count);
            let mut layouts = if queried {
                values[..count as usize].to_vec()
            } else {
                Vec::new()
            };
            // The base import extension supports implicit layouts for queried
            // formats even when no explicit modifiers are reported.
            if !layouts.contains(&super::INVALID_MODIFIER) {
                layouts.push(super::INVALID_MODIFIER);
            }
            self.capabilities.push(Capability {
                format,
                modifiers: layouts,
                max_size: self.maximum,
            });
        }
        Ok(())
    }
    pub fn capabilities(&self) -> Vec<Capability> {
        self.capabilities
            .iter()
            .map(|cap| Capability {
                format: cap.format,
                modifiers: cap.modifiers.clone(),
                max_size: cap.max_size,
            })
            .collect()
    }
    pub fn supports(
        &self,
        format: pipewire::spa::param::video::VideoFormat,
        modifier: u64,
        size: (u32, u32),
    ) -> bool {
        self.capabilities.iter().any(|cap| {
            cap.format == format
                && cap.modifiers.contains(&modifier)
                && size.0 <= cap.max_size
                && size.1 <= cap.max_size
        })
    }
    pub fn read(&mut self, size: (u32, u32), attrs: &[i32]) -> Result<RgbaImage> {
        self.resize(size)?;
        let _image = self.image(ptr::null_mut(), 0x3270, ptr::null_mut(), attrs)?;
        self.draw()
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        if self.context.current().is_err() {
            return;
        }
        // SAFETY: owned generated names (zero is harmless), current context;
        // no image guards or borrowed PipeWire buffers outlive this device.
        unsafe {
            for shader in &self.shaders {
                (self.gl.delete_shader)(*shader);
            }
            (self.gl.delete_program)(self.program);
            (self.gl.delete_buffers)(1, &self.vbo);
            (self.gl.delete_framebuffers)(1, &self.framebuffer);
            (self.gl.delete_textures)(1, &self.target);
        }
    }
}

pub(super) fn devices() -> Result<Vec<Device>> {
    // SAFETY: system EGL loader, retained by every context and function pointer.
    let api = Rc::new(unsafe { Egl::load_required() }.map_err(error)?);
    let extensions = api.query_string(None, egl::EXTENSIONS).map_err(error)?;
    if !extension(extensions, "EGL_EXT_device_enumeration")
        && !extension(extensions, "EGL_EXT_device_base")
    {
        return Ok(Vec::new());
    }
    let query = function!(
        api,
        "eglQueryDevicesEXT",
        unsafe extern "system" fn(i32, *mut Ptr, *mut i32) -> u32
    );
    let platform = function!(
        api,
        "eglGetPlatformDisplayEXT",
        unsafe extern "system" fn(u32, Ptr, *const i32) -> Ptr
    );
    let mut devices = [ptr::null_mut(); 8];
    let mut count = 0;
    // SAFETY: output capacity/count follow EGL_EXT_device_enumeration.
    if unsafe { query(8, devices.as_mut_ptr(), &mut count) } == 0 || !(0..=8).contains(&count) {
        return Ok(Vec::new());
    }
    let mut ready = Vec::new();
    for device in &devices[..count as usize] {
        if device.is_null() {
            continue;
        }
        // SAFETY: enumerated EGLDevice with the EGL_PLATFORM_DEVICE_EXT ABI.
        let display = unsafe { platform(0x313F, *device, ptr::null()) };
        if let Ok(context) = Context::open(api.clone(), display)
            && let Ok(mut backend) = Device::open(context)
            && backend.query_capabilities().is_ok()
            && !backend.capabilities.is_empty()
        {
            ready.push(backend);
        }
    }
    Ok(ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

    struct Export<'a> {
        _image: Image<'a>,
        _source: Texture<'a>,
        fds: Vec<OwnedFd>,
        indices: Vec<usize>,
        strides: Vec<i32>,
        offsets: Vec<i32>,
        fourcc: u32,
        modifier: u64,
    }
    fn export(device: &Device) -> Result<Export<'_>> {
        device.context.current()?;
        let extensions = device
            .context
            .api
            .query_string(Some(device.context.display), egl::EXTENSIONS)
            .map_err(error)?;
        if !extension(extensions, "EGL_MESA_image_dma_buf_export") {
            return Err(error("GPU 合成夹具需要 MESA DMA-BUF export"));
        }
        let query = function!(
            device.context.api,
            "eglExportDMABUFImageQueryMESA",
            unsafe extern "system" fn(Ptr, Ptr, *mut i32, *mut i32, *mut u64) -> u32
        );
        let export = function!(
            device.context.api,
            "eglExportDMABUFImageMESA",
            unsafe extern "system" fn(Ptr, Ptr, *mut i32, *mut i32, *mut i32) -> u32
        );
        let mut source = Texture {
            context: &device.context,
            gl: &device.gl,
            id: 0,
        };
        let pixels: [u8; 16] = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        // SAFETY: this test owns the current device and all sixteen input bytes;
        // its texture/image guards retain the storage through exported-fd reads.
        unsafe {
            (device.gl.gen_textures)(1, &mut source.id);
            (device.gl.bind_texture)(0x0DE1, source.id);
            device.parameters(0x0DE1);
            (device.gl.texture_image)(
                0x0DE1,
                0,
                0x1908,
                2,
                2,
                0,
                0x1908,
                0x1401,
                pixels.as_ptr().cast(),
            );
        }
        let image = device.image(
            device.context.context.unwrap().as_ptr(),
            0x30B1,
            source.id as usize as Ptr,
            &[0x30D2, 1, egl::NONE],
        )?;
        let mut fourcc = 0;
        let mut count = 0;
        let mut modifiers = [0; 4];
        // SAFETY: valid owned image and exact scalar outputs for the MESA ABI.
        if unsafe {
            query(
                device.context.display.as_ptr(),
                image.image,
                &mut fourcc,
                &mut count,
                modifiers.as_mut_ptr(),
            )
        } == 0
            || !(1..=4).contains(&count)
        {
            return Err(error("GPU 合成图无法导出"));
        }
        let mut fds = [-1; 4];
        let mut strides = [0; 4];
        let mut offsets = [0; 4];
        // SAFETY: output arrays have capacity four, at least the queried count;
        // exported descriptors become owned below, including any partial failure.
        let success = unsafe {
            export(
                device.context.display.as_ptr(),
                image.image,
                fds.as_mut_ptr(),
                strides.as_mut_ptr(),
                offsets.as_mut_ptr(),
            )
        };
        let mut owned: Vec<OwnedFd> = Vec::new();
        let mut indices = Vec::new();
        for fd in fds.iter().take(count as usize) {
            if *fd >= 0 {
                if let Some(index) = owned.iter().position(|owned| owned.as_raw_fd() == *fd) {
                    indices.push(Some(index));
                } else {
                    // SAFETY: MESA export transfers each distinct descriptor to
                    // the caller; deduplication prevents closing it twice.
                    owned.push(unsafe { OwnedFd::from_raw_fd(*fd) });
                    indices.push(Some(owned.len() - 1));
                }
            } else {
                indices.push(None);
            }
        }
        if success == 0 || owned.is_empty() {
            return Err(error("GPU 合成图导出 FD 失败"));
        }
        let indices = indices
            .into_iter()
            .map(|index| {
                index
                    .or_else(|| (owned.len() == 1).then_some(0))
                    .ok_or_else(|| error("GPU 夹具无法确定辅助平面的 FD"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Export {
            _image: image,
            _source: source,
            fds: owned,
            indices,
            strides: strides[..count as usize].to_vec(),
            offsets: offsets[..count as usize].to_vec(),
            fourcc: fourcc as u32,
            modifier: modifiers[0],
        })
    }

    #[test]
    #[ignore = "requires a real GPU with MESA DMA-BUF export; synthetic content only"]
    fn owned_egl_dmabuf_export_import_roundtrip_on_a_real_gpu() -> Result<()> {
        assert_eq!(
            std::env::var("DOGCODES_OWNED_EGL").unwrap(),
            "dmabuf-synthetic-only"
        );
        let source = devices()?
            .pop()
            .ok_or("没有可用的 DMA-BUF GPU；不能将跳过算作通过")?;
        let mut consumer = Device::open(Context::open(
            source.context.api.clone(),
            source.context.display.as_ptr(),
        )?)?;
        consumer.query_capabilities()?;
        let exported = export(&source)?;
        let format = FORMATS
            .iter()
            .find(|(_, fourcc)| *fourcc == exported.fourcc)
            .ok_or("GPU 导出格式不受支持")?
            .0;
        assert!(consumer.supports(format, exported.modifier, (2, 2)));
        let frame = super::super::Frame {
            size: (2, 2),
            format,
            modifier: Some(exported.modifier),
            colors: None,
            planes: exported
                .indices
                .iter()
                .enumerate()
                .map(|(i, index)| {
                    Ok(super::super::Plane {
                        fd: exported.fds[*index].as_fd(),
                        pitch: exported.strides[i],
                        offset: exported.offsets[i]
                            .try_into()
                            .map_err(|_| "GPU 导出偏移无效")?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        let attrs = super::super::attributes(&frame)?;
        for size in [(2, 2), (32, 24), (2, 2)] {
            if size != (2, 2) {
                consumer.resize(size)?;
                continue;
            }
            let out = consumer.read(size, &attrs)?;
            assert_eq!(
                out.as_raw(),
                &[
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255
                ]
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires an explicitly owned offscreen EGL software renderer"]
    fn owned_egl_renderer_verifies_external_images_channels_orientation_and_resize() -> Result<()> {
        assert_eq!(
            std::env::var("DOGCODES_OWNED_EGL").unwrap(),
            "offscreen-only"
        );
        // SAFETY: system library and the headless MESA platform, no display connection.
        let api = Rc::new(unsafe { Egl::load_required() }.unwrap());
        let platform = function!(
            api,
            "eglGetPlatformDisplayEXT",
            unsafe extern "system" fn(u32, Ptr, *const i32) -> Ptr
        );
        // SAFETY: EGL_MESA_platform_surfaceless explicitly takes a null native display.
        let display = unsafe { platform(0x31DD, ptr::null_mut(), ptr::null()) };
        let mut device = Device::open(Context::open(api, display).unwrap()).unwrap();
        let mut second = Device::open(Context::open(device.context.api.clone(), display)?)?;
        second.self_test()?;
        drop(second);
        // EGLTerminate from one stream must not invalidate this still-open owner.
        device.self_test()?;
        for _ in 0..5 {
            device.resize((32, 24)).unwrap();
            device.self_test().unwrap();
        }
        Ok::<(), String>(())
    }
}
