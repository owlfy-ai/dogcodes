use super::*;
use windows::{
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDataObject,
        },
        UI::Shell::{
            ASSOC_FILTER_RECOMMENDED, ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW,
            BHID_DataObject, IAssocHandler, IShellItem, SHAssocEnumHandlers,
            SHCreateItemFromParsingName,
        },
    },
    core::{HSTRING, PCWSTR, PWSTR},
};

struct Apartment(bool);
impl Apartment {
    fn new() -> Result<Self, String> {
        // SAFETY: Initializes only this calling thread. The guard balances successful calls.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        result.ok().map_err(|_| "无法初始化系统应用查询")?;
        Ok(Self(true))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: Balanced on the same thread, after the local COM objects are dropped.
            unsafe {
                CoUninitialize();
            }
        }
    }
}

fn owned_string(pointer: PWSTR) -> String {
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: GetName/GetUIName return NUL-terminated CoTaskMemAlloc strings.
    unsafe {
        let value = pointer.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pointer.0.cast()));
        value
    }
}

fn handlers(path: &Path) -> Result<Vec<(Candidate, IAssocHandler)>, String> {
    let Some(extension) = path.extension() else {
        return Ok(Vec::new());
    };
    let extension = HSTRING::from(format!(".{}", extension.to_string_lossy()));
    let mut default = [0u16; 32768];
    let mut length = default.len() as u32;
    // SAFETY: The output buffer and length are valid for this call; input is NUL-terminated.
    let default = unsafe {
        if AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_EXECUTABLE,
            &extension,
            PCWSTR::null(),
            Some(PWSTR(default.as_mut_ptr())),
            &mut length,
        )
        .is_ok()
        {
            String::from_utf16_lossy(
                &default[..default
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(default.len())],
            )
        } else {
            String::new()
        }
    };
    // SAFETY: Called with COM initialized and a valid extension string.
    let enumerator = unsafe { SHAssocEnumHandlers(&extension, ASSOC_FILTER_RECOMMENDED) }
        .map_err(|_| "无法查询系统推荐应用")?;
    let mut result = Vec::new();
    loop {
        let mut items = [None];
        let mut count = 0;
        // SAFETY: The one-element output slice and fetched count outlive this call.
        unsafe { enumerator.Next(&mut items, Some(&mut count)) }
            .map_err(|_| "读取系统应用列表失败")?;
        if count == 0 {
            break;
        }
        let Some(handler) = items[0].take() else {
            break;
        };
        // SAFETY: The handler is a live enumerated COM object on this initialized thread.
        let (id, name, recommended) = unsafe {
            (
                owned_string(handler.GetName().map_err(|_| "读取应用标识失败")?),
                owned_string(handler.GetUIName().map_err(|_| "读取应用名称失败")?),
                handler.IsRecommended().0 == 0,
            )
        };
        let location = PathBuf::from(&id);
        // Packaged application identifiers need not be executable paths.
        let visible = !location.is_absolute() || location.is_file();
        result.push((
            Candidate {
                app: Application {
                    is_default: id.eq_ignore_ascii_case(&default),
                    icon: None,
                    id,
                    name,
                },
                location,
                visible,
                recommended,
            },
            handler,
        ));
    }
    Ok(result)
}

pub fn candidates(path: &Path) -> Result<Vec<Candidate>, String> {
    let _apartment = Apartment::new()?;
    Ok(handlers(path)?
        .into_iter()
        .map(|(candidate, _)| candidate)
        .collect())
}

pub fn launch(path: &Path, candidate: &Candidate) -> Result<(), String> {
    let _apartment = Apartment::new()?;
    let (_, handler) = handlers(path)?
        .into_iter()
        .find(|(entry, _)| entry.app.id == candidate.app.id)
        .ok_or("此应用已不可用")?;
    let path = HSTRING::from(path.as_os_str());
    // SAFETY: The canonical file path is NUL-terminated; all objects stay on this
    // COM thread. Let the system handler build the launch request (including UWP).
    unsafe {
        let item: IShellItem =
            SHCreateItemFromParsingName(&path, None).map_err(|_| "无法读取系统文件对象")?;
        let data: IDataObject = item
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|_| "无法准备打开文件")?;
        handler
            .Invoke(&data)
            .map_err(|_| "应用无法打开此文件".into())
    }
}

pub fn load_icons(path: &Path, candidates: &mut [Candidate]) {
    let Ok(_apartment) = Apartment::new() else {
        return;
    };
    let Ok(handlers) = handlers(path) else {
        return;
    };
    for candidate in candidates {
        let Some((_, handler)) = handlers
            .iter()
            .find(|(entry, _)| entry.app.id == candidate.app.id)
        else {
            continue;
        };
        candidate.app.icon = cached_icon(candidate, || handler_icon(handler));
    }
}

fn handler_icon(handler: &IAssocHandler) -> Option<Vec<u8>> {
    use windows::Win32::UI::{
        Shell::SHDefExtractIconW,
        WindowsAndMessaging::{DestroyIcon, HICON},
    };
    let mut path = PWSTR::null();
    let mut index = 0;
    // SAFETY: Live COM handler, initialized apartment and valid output pointers.
    unsafe { handler.GetIconLocation(&mut path, &mut index) }.ok()?;
    let path = owned_string(path);
    // Some packaged applications register a PNG asset rather than an EXE/DLL.
    let file = Path::new(&path);
    if file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        if std::fs::metadata(file).ok()?.len() <= 256 * 1024 {
            return std::fs::read(file).ok();
        }
        return None;
    }
    let mut icon = HICON::default();
    // SAFETY: Valid NUL-terminated path and output handle. The returned owned
    // icon is destroyed after drawing, including when drawing fails.
    unsafe {
        SHDefExtractIconW(&HSTRING::from(path), index, 0, Some(&mut icon), None, 48)
            .ok()
            .ok()?;
        if icon.is_invalid() {
            return None;
        }
        let result = icon_png(icon);
        let _ = DestroyIcon(icon);
        result
    }
}

fn icon_png(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows::Win32::{
        Graphics::Gdi::*,
        UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx},
    };
    const SIZE: usize = 48;
    // SAFETY: The DIB is a top-down 48x48 BGRA buffer owned by this function.
    // Restore its previous selection before deleting it, then release its DC.
    let (black, white) = unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE as i32,
                biHeight: -(SIZE as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut pixels, None, 0) {
            Ok(bitmap) => bitmap,
            Err(_) => {
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let old = SelectObject(dc, HGDIOBJ(bitmap.0));
        let result = (|| {
            if pixels.is_null() {
                return None;
            }
            let buffer = std::slice::from_raw_parts_mut(pixels.cast::<u8>(), SIZE * SIZE * 4);
            buffer.fill(0);
            DrawIconEx(dc, 0, 0, icon, SIZE as i32, SIZE as i32, 0, None, DI_NORMAL).ok()?;
            GdiFlush().ok().ok()?;
            let black = buffer.to_vec();
            buffer.fill(255);
            DrawIconEx(dc, 0, 0, icon, SIZE as i32, SIZE as i32, 0, None, DI_NORMAL).ok()?;
            GdiFlush().ok().ok()?;
            Some((black, buffer.to_vec()))
        })();
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        result?
    };
    // Compositing over black and white recovers alpha for both modern ARGB
    // icons and legacy mask icons, without black fringes on a light theme.
    let mut rgba = Vec::with_capacity(black.len());
    for (black, white) in black.chunks_exact(4).zip(white.chunks_exact(4)) {
        let alpha = 255 - (0..3).map(|i| white[i].saturating_sub(black[i])).max()?;
        for i in [2, 1, 0] {
            rgba.push(if alpha == 0 {
                0
            } else {
                (u32::from(black[i]) * 255 / u32::from(alpha)).min(255) as u8
            });
        }
        rgba.push(alpha);
    }
    let mut result = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut result, SIZE as u32, SIZE as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
    }
    Some(result)
}
