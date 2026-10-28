//! Checked mapped PipeWire pixels; no frame queues or native pointers here.
use crate::Result;
use image::RgbaImage;
use std::borrow::Cow;
#[derive(Clone, Copy)]
pub(super) enum Layout {
    Rgba,
    Rgbx,
    Bgra,
    Bgrx,
    Rgb,
    Bgr,
    Argb,
    Abgr,
}
impl Layout {
    pub fn components(self) -> usize {
        if matches!(self, Self::Rgb | Self::Bgr) {
            3
        } else {
            4
        }
    }
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
    let components = layout.components();
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
            let bgr = matches!(
                layout,
                Layout::Bgra | Layout::Bgrx | Layout::Bgr | Layout::Abgr
            );
            let start = usize::from(matches!(layout, Layout::Argb | Layout::Abgr));
            dst.copy_from_slice(&[
                src[start + if bgr { 2 } else { 0 }],
                src[start + 1],
                src[start + if bgr { 0 } else { 2 }],
                255,
            ]);
        }
    }
    Ok(out)
}

pub(super) struct Plane<'a> {
    pub bytes: &'a [u8],
    pub offset: u32,
    pub length: u32,
    pub stride: i32,
}
impl Plane<'_> {
    /// SPA chunks wrap within maxsize: offset is modulo maxsize and size is
    /// clamped to it. Only wrapped chunks require a copy; ordinary frames borrow.
    pub fn chunk(&self, length: usize) -> Result<Cow<'_, [u8]>> {
        if self.bytes.is_empty() || self.length == 0 {
            return Err("Wayland 画面缓冲区为空".into());
        }
        let offset = self.offset as usize % self.bytes.len();
        if length > (self.length as usize).min(self.bytes.len()) {
            return Err("Wayland 画面缓冲区不完整".into());
        }
        if length <= self.bytes.len() - offset {
            return Ok(Cow::Borrowed(&self.bytes[offset..offset + length]));
        }
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(&self.bytes[offset..]);
        bytes.extend_from_slice(&self.bytes[..length - (self.bytes.len() - offset)]);
        Ok(Cow::Owned(bytes))
    }

    pub fn rows(&self, row_bytes: usize, height: usize) -> Result<Cow<'_, [u8]>> {
        let pitch = self.stride.unsigned_abs() as usize;
        let needed = height
            .checked_sub(1)
            .and_then(|rows| pitch.checked_mul(rows))
            .and_then(|n| n.checked_add(row_bytes))
            .filter(|_| pitch >= row_bytes)
            .ok_or("Wayland 画面行跨度无效")?;
        self.chunk(needed)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Matrix {
    Bt601,
    Bt709,
}
#[derive(Clone, Copy)]
pub(super) enum Range {
    Full,
    Limited,
}

/// NV12 can have one allocation (Y followed by UV) or two independently padded
/// planes. Chunks/strides belong to each plane, not just the first spa_data.
pub(super) fn unpack_nv12(
    size: (u32, u32),
    planes: &[Plane<'_>],
    matrix: Matrix,
    range: Range,
) -> Result<RgbaImage> {
    let (width, height) = size;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
    {
        return Err("Wayland NV12 画面尺寸无效".into());
    }
    let convert = |y: &[u8], y_stride: i32, uv: &[u8], uv_stride: i32| {
        let y_pitch = y_stride.unsigned_abs() as usize;
        let uv_pitch = uv_stride.unsigned_abs() as usize;
        let needed = |pitch: usize, rows: u32| {
            pitch
                .checked_mul(rows as usize - 1)
                .and_then(|n| n.checked_add(width as usize))
        };
        if y_pitch < width as usize
            || uv_pitch < width as usize
            || needed(y_pitch, height).is_none_or(|n| n > y.len())
            || needed(uv_pitch, height / 2).is_none_or(|n| n > uv.len())
        {
            return Err("Wayland NV12 画面缓冲区不完整".into());
        }
        let (gain, bias, rv, gu, gv, bu) = match (matrix, range) {
            (Matrix::Bt601, Range::Limited) => (298, 16, 409, -100, -208, 516),
            (Matrix::Bt709, Range::Limited) => (298, 16, 459, -55, -136, 541),
            (Matrix::Bt601, Range::Full) => (256, 0, 359, -88, -183, 454),
            (Matrix::Bt709, Range::Full) => (256, 0, 403, -48, -120, 475),
        };
        let mut out = RgbaImage::new(width, height);
        for (row, target) in out
            .as_mut()
            .chunks_exact_mut(width as usize * 4)
            .enumerate()
        {
            let source_y = if y_stride < 0 {
                height as usize - 1 - row
            } else {
                row
            };
            let source_uv = if uv_stride < 0 {
                height as usize / 2 - 1 - row / 2
            } else {
                row / 2
            };
            let luma = &y[source_y * y_pitch..][..width as usize];
            let chroma = &uv[source_uv * uv_pitch..][..width as usize];
            for (x, pixel) in target.chunks_exact_mut(4).enumerate() {
                let light = gain * (i32::from(luma[x]) - bias);
                let u = i32::from(chroma[x & !1]) - 128;
                let v = i32::from(chroma[(x & !1) + 1]) - 128;
                let component = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
                pixel.copy_from_slice(&[
                    component(light + rv * v),
                    component(light + gu * u + gv * v),
                    component(light + bu * u),
                    255,
                ]);
            }
        }
        Ok(out)
    };
    match planes {
        [plane] => {
            let split = (plane.stride.unsigned_abs() as usize)
                .checked_mul(height as usize)
                .ok_or("Wayland NV12 色度平面缺失")?;
            let needed = (plane.stride.unsigned_abs() as usize)
                .checked_mul(height as usize / 2 - 1)
                .and_then(|n| n.checked_add(width as usize))
                .and_then(|n| n.checked_add(split))
                .filter(|_| plane.stride.unsigned_abs() >= width)
                .ok_or("Wayland NV12 画面行跨度无效")?;
            let bytes = plane.chunk(needed)?;
            convert(&bytes[..split], plane.stride, &bytes[split..], plane.stride)
        }
        [y, uv] => convert(
            &y.rows(width as usize, height as usize)?,
            y.stride,
            &uv.rows(width as usize, height as usize / 2)?,
            uv.stride,
        ),
        _ => Err("Wayland NV12 数据平面数量无效".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leading_alpha_and_unused_components_do_not_shift_rgb_channels() {
        for (layout, bytes) in [(Layout::Argb, [99, 1, 2, 3]), (Layout::Abgr, [99, 3, 2, 1])] {
            assert_eq!(
                unpack(&bytes, (1, 1), 0, 4, 4, layout).unwrap().as_raw(),
                &[1, 2, 3, 255]
            );
        }
    }
    #[test]
    fn nv12_uses_the_negotiated_matrix_and_full_or_limited_range() {
        for (matrix, range, values) in [
            (Matrix::Bt601, Range::Limited, [81, 90, 240]),
            (Matrix::Bt709, Range::Limited, [63, 102, 240]),
            (Matrix::Bt601, Range::Full, [76, 85, 255]),
            (Matrix::Bt709, Range::Full, [54, 99, 255]),
        ] {
            let bytes = [
                values[0], values[0], values[0], values[0], values[1], values[2],
            ];
            let plane = Plane {
                bytes: &bytes,
                offset: 0,
                length: 6,
                stride: 2,
            };
            let image = unpack_nv12((2, 2), &[plane], matrix, range).unwrap();
            for pixel in image.pixels() {
                assert!(
                    pixel[0] >= 253 && pixel[1] <= 2 && pixel[2] <= 2,
                    "{pixel:?}"
                );
                assert_eq!(pixel[3], 255);
            }
        }
        for (range, black, white) in [(Range::Full, 0, 255), (Range::Limited, 16, 235)] {
            let bytes = [black, white, black, white, 128, 128];
            let plane = Plane {
                bytes: &bytes,
                offset: 0,
                length: 6,
                stride: 2,
            };
            let image = unpack_nv12((2, 2), &[plane], Matrix::Bt709, range).unwrap();
            assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
            assert_eq!(image.get_pixel(1, 0).0, [255, 255, 255, 255]);
        }
    }
    #[test]
    fn nv12_separate_planes_have_independent_offsets_padding_and_orientation() {
        let y = [
            99, 235, 235, 88, 88, 235, 235, 88, 88, 16, 16, 88, 88, 16, 16,
        ];
        let uv = [99, 99, 128, 128, 88, 88, 88, 88, 128, 128];
        let planes = [
            Plane {
                bytes: &y,
                offset: 1,
                length: 14,
                stride: -4,
            },
            Plane {
                bytes: &uv,
                offset: 2,
                length: 8,
                stride: -6,
            },
        ];
        let image = unpack_nv12((2, 4), &planes, Matrix::Bt601, Range::Limited).unwrap();
        for row in 0..4 {
            assert_eq!(
                image.get_pixel(0, row).0,
                if row < 2 {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                }
            );
        }
        // A single plane's UV begins after all padded Y rows, not width*height.
        let contiguous = [16, 16, 99, 99, 16, 16, 99, 99, 128, 128];
        let image = unpack_nv12(
            (2, 2),
            &[Plane {
                bytes: &contiguous,
                offset: 0,
                length: 10,
                stride: 4,
            }],
            Matrix::Bt709,
            Range::Limited,
        )
        .unwrap();
        assert!(image.pixels().all(|p| p.0 == [0, 0, 0, 255]));
    }
    #[test]
    fn spa_chunk_wraps_offset_and_clamps_length_without_reading_outside_memory() {
        let bytes = [16, 16, 128, 128, 16, 16];
        let plane = Plane {
            bytes: &bytes,
            offset: 10,
            length: u32::MAX,
            stride: 2,
        };
        let image = unpack_nv12((2, 2), &[plane], Matrix::Bt601, Range::Limited).unwrap();
        assert!(image.pixels().all(|p| p.0 == [0, 0, 0, 255]));
        assert!(
            Plane {
                bytes: &[],
                offset: 0,
                length: 1,
                stride: 2
            }
            .chunk(1)
            .is_err()
        );
    }
    #[test]
    fn truncated_or_invalid_nv12_is_rejected_before_allocation_or_indexing() {
        let bytes = [16, 16, 16, 16, 128];
        for size in [(2, 2), (1, 2), (2, 1), (0, 2), (16384, 16384)] {
            for stride in [0, 1, 2, i32::MIN] {
                let plane = Plane {
                    bytes: &bytes,
                    offset: 0,
                    length: 5,
                    stride,
                };
                assert!(unpack_nv12(size, &[plane], Matrix::Bt601, Range::Limited).is_err());
            }
        }
        assert!(unpack_nv12((2, 2), &[], Matrix::Bt709, Range::Full).is_err());
    }
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
