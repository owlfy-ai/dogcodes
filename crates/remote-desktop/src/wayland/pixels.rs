//! Checked packed PipeWire pixels; no frame queues or native pointers here.
use crate::Result;
use image::RgbaImage;
#[derive(Clone, Copy)]
pub(super) enum Layout {
    Rgba,
    Rgbx,
    Bgra,
    Bgrx,
    Rgb,
    Bgr,
}
/// PipeWire chunks can have padding, a nonzero offset and negative stride.
/// Never assume the mapped allocation is exactly width*height*4 bytes.
pub(super) fn unpack(
    bytes: &[u8],
    size: (u32, u32),
    offset: usize,
    length: usize,
    stride: i32,
    layout: Layout,
) -> Result<RgbaImage> {
    let (width, height) = size;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
    {
        return Err("Wayland 画面尺寸超出支持范围".into());
    }
    let components = if matches!(layout, Layout::Rgb | Layout::Bgr) {
        3
    } else {
        4
    };
    let row = usize::try_from(width)
        .map_err(|_| "画面宽度无效")?
        .checked_mul(components)
        .ok_or("画面过大")?;
    let pitch = usize::try_from(stride.unsigned_abs()).map_err(|_| "画面行跨度无效")?;
    let needed = pitch
        .checked_mul(height as usize - 1)
        .and_then(|v| v.checked_add(row))
        .ok_or("画面过大")?;
    let end = offset.checked_add(length).ok_or("画面偏移无效")?;
    if pitch < row || needed > length || end > bytes.len() {
        return Err("Wayland 画面缓冲区不完整".into());
    }
    let data = &bytes[offset..end];
    let mut out = RgbaImage::new(width, height);
    for (y, target) in out
        .as_mut()
        .chunks_exact_mut(width as usize * 4)
        .enumerate()
    {
        let source_y = if stride < 0 {
            height as usize - 1 - y
        } else {
            y
        };
        let source = &data[source_y * pitch..source_y * pitch + row];
        for (src, dst) in source
            .chunks_exact(components)
            .zip(target.chunks_exact_mut(4))
        {
            let bgr = matches!(layout, Layout::Bgra | Layout::Bgrx | Layout::Bgr);
            dst.copy_from_slice(&[
                src[if bgr { 2 } else { 0 }],
                src[1],
                src[if bgr { 0 } else { 2 }],
                255,
            ]);
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_rgb24_rows_expand_to_opaque_rgba_without_reading_padding() {
        let image = unpack(&[99, 1, 2, 3, 88, 4, 5, 6], (1, 2), 1, 7, 4, Layout::Rgb).unwrap();
        assert_eq!(image.as_raw(), &[1, 2, 3, 255, 4, 5, 6, 255]);
        let bgr = unpack(&[3, 2, 1, 99], (1, 1), 0, 3, 3, Layout::Bgr).unwrap();
        assert_eq!(bgr.as_raw(), &[1, 2, 3, 255]);
    }
    #[test]
    fn offset_padding_and_negative_stride_are_checked_and_preserved() {
        let bytes = [
            99, 99, 1, 2, 3, 0, 4, 5, 6, 0, 88, 88, 88, 88, 7, 8, 9, 0, 10, 11, 12, 0,
        ];
        let frame = unpack(&bytes, (2, 2), 2, 20, 12, Layout::Bgrx).unwrap();
        assert_eq!(
            frame.as_raw(),
            &[3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
        let flipped = unpack(&bytes, (2, 2), 2, 20, -12, Layout::Rgba).unwrap();
        assert_eq!(&flipped.as_raw()[..8], &[7, 8, 9, 255, 10, 11, 12, 255]);
        for (offset, length, stride) in [
            (2, 19, 12),
            (3, 20, 12),
            (2, 20, 7),
            (usize::MAX, 20, 12),
            (2, 20, i32::MIN),
        ] {
            assert!(unpack(&bytes, (2, 2), offset, length, stride, Layout::Bgra).is_err());
        }
        assert!(unpack(&bytes, (16384, 16384), 0, 22, 65536, Layout::Rgbx).is_err());
    }
}
