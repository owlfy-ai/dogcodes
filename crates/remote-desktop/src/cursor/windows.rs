use super::{MAX_SIDE, Shape, Snapshot, bitmap};
use windows::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
#[derive(Default)]
pub(super) struct Native {}
impl Native {
    pub fn read(&mut self) -> Option<Snapshot> {
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: correctly sized writable output. This only reads the cursor.
        unsafe { GetCursorInfo(&raw mut info) }.ok()?;
        if info.flags.0 & CURSOR_SHOWING.0 == 0 || info.flags.0 & CURSOR_SUPPRESSED.0 != 0 {
            return None;
        }
        Some(Snapshot {
            x: info.ptScreenPos.x,
            y: info.ptScreenPos.y,
            shape: shape(HICON(info.hCursor.0)),
        })
    }
}
struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: a private CopyIcon result, never the system-owned cursor handle.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}
struct Info(ICONINFO);
impl Drop for Info {
    fn drop(&mut self) {
        // SAFETY: GetIconInfo transfers ownership of these two bitmaps to us.
        unsafe {
            if !self.0.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.0.hbmMask.0));
            }
            if !self.0.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.0.hbmColor.0));
            }
        }
    }
}
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        // SAFETY: paired with this GetDC(None), on the same native worker thread.
        unsafe {
            ReleaseDC(None, self.0);
        }
    }
}
fn shape(cursor: HICON) -> Option<Shape> {
    // SAFETY: copy the currently reported handle; failures return no bitmap.
    let icon = Icon(unsafe { CopyIcon(cursor) }.ok()?);
    let mut info = ICONINFO::default();
    // SAFETY: owned valid icon and initialized writable ICONINFO.
    unsafe { GetIconInfo(icon.0, &raw mut info) }.ok()?;
    let info = Info(info);
    let mut mask = BITMAP::default();
    // SAFETY: initialized bitmap descriptor, exact buffer size and owned HBITMAP.
    let count = unsafe {
        GetObjectW(
            HGDIOBJ(info.0.hbmMask.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&raw mut mask).cast()),
        )
    };
    if count != std::mem::size_of::<BITMAP>() as i32 {
        return None;
    }
    if mask.bmPlanes != 1 || mask.bmBitsPixel != 1 {
        return None;
    }
    let w = u32::try_from(mask.bmWidth).ok()?;
    let color = !info.0.hbmColor.is_invalid();
    let raw_h = u32::try_from(mask.bmHeight).ok()?;
    let h = if color {
        raw_h
    } else {
        if raw_h % 2 != 0 {
            return None;
        }
        raw_h / 2
    };
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    // SAFETY: read-only compatible bitmap conversion; no image is selected into
    // the borrowed screen DC and every DC/bitmap/icon is released on this thread.
    let dc = Dc(unsafe { GetDC(None) });
    if dc.0.is_invalid() {
        return None;
    }
    let mask = pixels(&dc, info.0.hbmMask, w, raw_h)?;
    let color = if color {
        Some(pixels(&dc, info.0.hbmColor, w, h)?)
    } else {
        None
    };
    bitmap::convert(
        w,
        h,
        (info.0.xHotspot, info.0.yHotspot),
        &mask,
        color.as_deref(),
    )
}
fn pixels(dc: &Dc, bitmap: HBITMAP, w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE * 2 {
        return None;
    }
    let mut source = BITMAP::default();
    // SAFETY: exact descriptor size and initialized writable output. Reject
    // mismatched color/mask extents before providing any pixel buffer to GDI.
    let count = unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&raw mut source).cast()),
        )
    };
    if count != std::mem::size_of::<BITMAP>() as i32
        || source.bmWidth != w as i32
        || source.bmHeight != h as i32
        || source.bmPlanes != 1
    {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bytes = vec![0; (w * h * 4) as usize];
    // SAFETY: width <=512, height <=1024 and exact 32bpp top-down destination;
    // caller owns an unselected bitmap. BI_RGB 32bpp needs no color table.
    let rows = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            h,
            Some(bytes.as_mut_ptr().cast()),
            &raw mut info,
            DIB_RGB_COLORS,
        )
    };
    (rows == h as i32).then_some(bytes)
}
