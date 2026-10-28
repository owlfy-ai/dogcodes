//! A single authorized PipeWire cursor and clean background, never OS input APIs.
use super::{geometry::Presentation, metadata};
use crate::Result;
use image::RgbaImage;
use pipewire::spa::{param::video::VideoFormat, sys};

const MAX_SIDE: u32 = 512;
pub(super) const MAX_BYTES: usize = size_of::<sys::spa_meta_cursor>()
    + size_of::<sys::spa_meta_bitmap>()
    + MAX_SIDE as usize * MAX_SIDE as usize * 4;

#[derive(Default)]
pub(super) struct Cursor {
    id: u32,
    position: (i32, i32),
    hot: (i32, i32),
    shape: Option<RgbaImage>,
    background: Option<(RgbaImage, Presentation)>,
    changed: bool,
}
impl Cursor {
    /// # Safety
    /// The exclusive buffer loan must own all descriptors and payloads until return.
    pub unsafe fn update(&mut self, buffer: &sys::spa_buffer) -> Result<()> {
        // SAFETY: same live loan; validates descriptors/header before any cursor data.
        if unsafe { metadata::read(buffer)? }.is_none() || buffer.n_metas == 0 {
            return Ok(());
        }
        // SAFETY: metadata::read checked the descriptor count and pointer above.
        let metas = unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
        let mut found = metas.iter().filter(|m| m.type_ == sys::SPA_META_Cursor);
        let Some(meta) = found.next() else {
            return Ok(());
        };
        let length = meta.size as usize;
        if found.next().is_some()
            || meta.data.is_null()
            || length < size_of::<sys::spa_meta_cursor>()
            || length > MAX_BYTES
        {
            return Err("Wayland 光标元数据范围无效".into());
        }
        // SAFETY: the owned payload was bounded above; copy integer POD once,
        // so offsets cannot change between their validation and use.
        let bytes = unsafe { std::slice::from_raw_parts(meta.data.cast::<u8>(), length) };
        self.read(bytes)
    }
    fn read(&mut self, bytes: &[u8]) -> Result<()> {
        let cursor: sys::spa_meta_cursor = pod(bytes, 0)?;
        if cursor.id == 0 {
            return Ok(());
        } // SPA: no new cursor data, not hidden.
        let position = (cursor.position.x, cursor.position.y);
        let mut shape = None;
        if cursor.bitmap_offset != 0 {
            let start = cursor.bitmap_offset as usize;
            if start < size_of::<sys::spa_meta_cursor>() {
                return Err("Wayland 光标位图偏移无效".into());
            }
            let bitmap: sys::spa_meta_bitmap = pod(bytes, start)?;
            if bitmap.format != 0 {
                shape = Some(if bitmap.offset == 0 {
                    None // Explicitly invisible; discard the old image.
                } else {
                    let (w, h) = (bitmap.size.width, bitmap.size.height);
                    let pitch =
                        usize::try_from(bitmap.stride).map_err(|_| "Wayland 光标行跨度无效")?;
                    let row = w as usize * 4;
                    if w == 0
                        || h == 0
                        || w > MAX_SIDE
                        || h > MAX_SIDE
                        || pitch < row
                        || pitch > MAX_SIDE as usize * 4
                        || cursor.hotspot.x < 0
                        || cursor.hotspot.y < 0
                        || cursor.hotspot.x as u32 >= w
                        || cursor.hotspot.y as u32 >= h
                        || (bitmap.offset as usize) < size_of::<sys::spa_meta_bitmap>()
                    {
                        return Err("Wayland 光标位图尺寸或热点无效".into());
                    }
                    let offset = start
                        .checked_add(bitmap.offset as usize)
                        .ok_or("Wayland 光标偏移溢出")?;
                    let size = pitch
                        .checked_mul(h as usize - 1)
                        .and_then(|v| v.checked_add(row))
                        .ok_or("Wayland 光标长度溢出")?;
                    let pixels = bytes
                        .get(offset..offset.checked_add(size).ok_or("Wayland 光标长度溢出")?)
                        .ok_or("Wayland 光标位图不完整")?;
                    let format = VideoFormat::from_raw(bitmap.format);
                    if !matches!(format, VideoFormat::RGBA | VideoFormat::BGRA) {
                        return Err("Wayland 光标像素格式不支持".into());
                    }
                    Some(RgbaImage::from_fn(w, h, |x, y| {
                        let i = y as usize * pitch + x as usize * 4;
                        let p = &pixels[i..i + 4];
                        image::Rgba(if format == VideoFormat::BGRA {
                            [p[2], p[1], p[0], p[3]]
                        } else {
                            [p[0], p[1], p[2], p[3]]
                        })
                    }))
                });
            }
        }
        if self.id != cursor.id {
            self.shape = None;
        }
        self.id = cursor.id;
        self.position = position;
        if let Some(shape) = shape {
            self.shape = shape;
            self.hot = (cursor.hotspot.x, cursor.hotspot.y);
        }
        self.changed = true;
        Ok(())
    }
    pub fn render(
        &mut self,
        image: Option<RgbaImage>,
        presentation: Presentation,
    ) -> Result<Option<RgbaImage>> {
        if let Some(image) = image {
            self.background = Some((image, presentation));
        } else if !self.changed {
            return Ok(None);
        }
        let Some((background, presentation)) = &self.background else {
            return Ok(None);
        };
        let mut image = background.clone();
        if let Some(shape) = &self.shape {
            let left = i64::from(self.position.0) - i64::from(self.hot.0);
            let top = i64::from(self.position.1) - i64::from(self.hot.1);
            for (x, y, pixel) in shape.enumerate_pixels() {
                let (dx, dy) = (left + i64::from(x), top + i64::from(y));
                if dx < 0
                    || dy < 0
                    || dx >= i64::from(image.width())
                    || dy >= i64::from(image.height())
                {
                    continue;
                }
                let target = image.get_pixel_mut(dx as u32, dy as u32);
                // Portal cursor bitmaps use premultiplied alpha. Blend once
                // onto a clean base, never onto the previously drawn cursor.
                for c in 0..3 {
                    target[c] = (u32::from(pixel[c])
                        + (u32::from(target[c]) * (255 - u32::from(pixel[3])) + 127) / 255)
                        .min(255) as u8;
                }
                target[3] = 255;
            }
        }
        let image = presentation.apply(image)?;
        self.changed = false;
        Ok(Some(image))
    }
}

// Only used with integer-only SPA cursor/bitmap PODs, not arbitrary Rust values.
fn pod<T: Copy>(bytes: &[u8], offset: usize) -> Result<T> {
    let extent = offset
        .checked_add(size_of::<T>())
        .ok_or("Wayland 光标范围溢出")?;
    let raw = bytes
        .get(offset..extent)
        .ok_or("Wayland 光标元数据不完整")?;
    // SAFETY: callers request integer-only SPA PODs; bounded read may be unaligned.
    Ok(unsafe { std::ptr::read_unaligned(raw.as_ptr().cast::<T>()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write<T: Copy>(bytes: &mut [u8], offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= bytes.len());
        // SAFETY: test owns the checked writable extent; integer-only SPA POD.
        unsafe { std::ptr::write_unaligned(bytes.as_mut_ptr().add(offset).cast::<T>(), value) }
    }
    pub(super) fn fixture(position: (i32, i32), bitmap: bool) -> Vec<u8> {
        let header = size_of::<sys::spa_meta_cursor>();
        let shape = size_of::<sys::spa_meta_bitmap>();
        let mut bytes = vec![0; header + shape + 32];
        write(
            &mut bytes,
            0,
            sys::spa_meta_cursor {
                id: 1,
                flags: 0,
                position: sys::spa_point {
                    x: position.0,
                    y: position.1,
                },
                hotspot: sys::spa_point { x: 1, y: 1 },
                bitmap_offset: if bitmap { header as u32 } else { 0 },
            },
        );
        write(
            &mut bytes,
            header,
            sys::spa_meta_bitmap {
                format: VideoFormat::BGRA.as_raw(),
                size: sys::spa_rectangle {
                    width: 2,
                    height: 2,
                },
                stride: 16,
                offset: shape as u32,
            },
        );
        for y in 0..2 {
            for x in 0..2 {
                let i = header + shape + y * 16 + x * 4;
                bytes[i..i + 4].copy_from_slice(&[0, 128, 0, 128]);
            }
        }
        bytes
    }
    #[test]
    fn metadata_only_move_hide_and_reused_image_do_not_leave_a_trail() {
        let base = RgbaImage::from_pixel(5, 4, image::Rgba([200, 0, 0, 255]));
        let mut cursor = Cursor::default();
        cursor.read(&fixture((2, 2), true)).unwrap();
        let first = cursor
            .render(Some(base), Presentation::default())
            .unwrap()
            .unwrap();
        assert_eq!(first.get_pixel(1, 1).0, [100, 128, 0, 255]);
        cursor.read(&fixture((4, 2), false)).unwrap();
        let second = cursor
            .render(None, Presentation::default())
            .unwrap()
            .unwrap();
        assert_eq!(second.get_pixel(1, 1).0, [200, 0, 0, 255]);
        assert_eq!(second.get_pixel(3, 1).0, [100, 128, 0, 255]);
        let mut absent = fixture((0, 0), false);
        write(&mut absent, 0, 0u32); // ID 0 means unchanged, not hidden.
        cursor.read(&absent).unwrap();
        assert!(
            cursor
                .render(None, Presentation::default())
                .unwrap()
                .is_none()
        );
        let mut hide = fixture((4, 2), true);
        write(&mut hide, size_of::<sys::spa_meta_cursor>() + 16, 0u32); // bitmap.offset
        cursor.read(&hide).unwrap();
        let hidden = cursor
            .render(None, Presentation::default())
            .unwrap()
            .unwrap();
        assert!(hidden.pixels().all(|p| p.0 == [200, 0, 0, 255]));
    }
    #[test]
    fn pointer_is_clipped_then_cropped_and_oriented_with_the_original_frame() {
        let mut cursor = Cursor::default();
        cursor.read(&fixture((1, 1), true)).unwrap();
        let image = cursor
            .render(
                Some(RgbaImage::new(4, 3)),
                Presentation {
                    crop: Some((1, 0, 3, 2)),
                    transform: 1,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(image.dimensions(), (2, 3));
        assert_eq!(image.get_pixel(0, 0).0, [0, 128, 0, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 128, 0, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [0, 0, 0, 0]);
        cursor.read(&fixture((i32::MIN, i32::MAX), false)).unwrap();
        assert!(
            cursor
                .render(None, Presentation::default())
                .unwrap()
                .unwrap()
                .pixels()
                .all(|p| p.0 == [0, 0, 0, 0])
        );
    }
    #[test]
    fn malformed_offsets_sizes_formats_and_hotspots_are_rejected_before_pixels() {
        let valid = fixture((2, 2), true);
        let header = size_of::<sys::spa_meta_cursor>();
        for (offset, value) in [
            (24, 1),
            (24, u32::MAX),
            (header, VideoFormat::NV12.as_raw()),
            (header + 4, 513),
            (header + 12, u32::MAX),
            (header + 16, 1),
            (16, 2),
        ] {
            let mut bytes = valid.clone();
            write(&mut bytes, offset, value);
            assert!(
                Cursor::default().read(&bytes).is_err(),
                "accepted offset {offset}"
            );
        }
        assert!(Cursor::default().read(&valid[..valid.len() - 9]).is_err());
        let mut unaligned = vec![0];
        unaligned.extend_from_slice(&valid);
        assert!(Cursor::default().read(&unaligned[1..]).is_ok());
    }
    #[test]
    fn native_descriptors_are_bounded_and_corrupted_headers_skip_cursor_payload() {
        let mut bytes = fixture((2, 2), true);
        let good = sys::spa_meta {
            type_: sys::SPA_META_Cursor,
            size: bytes.len() as u32,
            data: bytes.as_mut_ptr().cast(),
        };
        for mut metas in [
            vec![good, good],
            vec![sys::spa_meta {
                size: MAX_BYTES as u32 + 1,
                ..good
            }],
            vec![sys::spa_meta {
                data: std::ptr::null_mut(),
                ..good
            }],
        ] {
            let buffer = sys::spa_buffer {
                n_metas: metas.len() as u32,
                metas: metas.as_mut_ptr(),
                n_datas: 0,
                datas: std::ptr::null_mut(),
            };
            // SAFETY: descriptor array is live; invalid fields must be rejected first.
            assert!(unsafe { Cursor::default().update(&buffer) }.is_err());
        }
        // SAFETY: SPA header is an integer-only POD.
        let mut header: sys::spa_meta_header = unsafe { std::mem::zeroed() };
        header.flags = sys::SPA_META_HEADER_FLAG_CORRUPTED;
        let mut metas = [
            sys::spa_meta {
                type_: sys::SPA_META_Header,
                size: size_of::<sys::spa_meta_header>() as u32,
                data: std::ptr::from_mut(&mut header).cast(),
            },
            sys::spa_meta {
                data: std::ptr::null_mut(),
                ..good
            },
        ];
        let buffer = sys::spa_buffer {
            n_metas: 2,
            metas: metas.as_mut_ptr(),
            n_datas: 0,
            datas: std::ptr::null_mut(),
        };
        // SAFETY: owned descriptor/header array lives throughout this call.
        assert!(unsafe { Cursor::default().update(&buffer) }.is_ok());
    }
}
