//! Normalize Windows' linear FP16 desktop to the existing 8-bit SDR input.
//! Preserve SDR reference white; values beyond it clip, rather than claiming
//! that the current SDR codec/renderer can preserve HDR highlights or gamut.
use super::Result;
use image::{Rgba, RgbaImage};

pub(super) struct Converter {
    white: u32,
    channels: Vec<u8>,
}
impl Converter {
    pub fn new(white: u32) -> Result<Self> {
        let mut converter = Self {
            white: 0,
            channels: Vec::new(),
        };
        converter.update(white)?;
        Ok(converter)
    }
    pub fn update(&mut self, white: u32) -> Result<()> {
        if white == 0 {
            return Err("系统 SDR 白电平无效".into());
        }
        if white == self.white {
            return Ok(());
        }
        // One 64 KiB lookup per capture worker, only rebuilt on a white change.
        // No half conversion, gamma pow or intermediate full-frame copy per pixel.
        self.channels = (0..=u16::MAX).map(|bits| channel(bits, white)).collect();
        self.white = white;
        Ok(())
    }
    pub fn frame(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        pitch: usize,
        rotation: u32,
    ) -> Result<RgbaImage> {
        let row = (width as usize).checked_mul(8).ok_or("视频行尺寸溢出")?;
        let length = pitch
            .checked_mul(height.saturating_sub(1) as usize)
            .and_then(|n| n.checked_add(row))
            .ok_or("视频缓冲区溢出")?;
        if width == 0
            || height == 0
            || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
            || pitch < row
            || length > bytes.len()
            || rotation > 3
        {
            return Err("系统浮点视频缓冲区无效".into());
        }
        let size = if rotation % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        };
        let mut image = RgbaImage::new(size.0, size.1);
        for y in 0..height {
            for x in 0..width {
                let offset = y as usize * pitch + x as usize * 8;
                let pixel = &bytes[offset..offset + 8];
                let component =
                    |n| self.channels[usize::from(u16::from_le_bytes([pixel[n], pixel[n + 1]]))];
                let (dx, dy) = match rotation {
                    0 => (x, y),
                    1 => (height - 1 - y, x),
                    2 => (width - 1 - x, height - 1 - y),
                    _ => (y, width - 1 - x),
                };
                image.put_pixel(
                    dx,
                    dy,
                    Rgba([component(0), component(2), component(4), 255]),
                );
            }
        }
        Ok(image)
    }
}

fn channel(bits: u16, white: u32) -> u8 {
    if bits & 0x8000 != 0 {
        return 0;
    }
    let exponent = (bits >> 10) & 31;
    let mantissa = bits & 1023;
    if exponent == 31 {
        return if mantissa == 0 { 255 } else { 0 }; // +inf / NaN
    }
    let linear = if exponent == 0 {
        f32::from(mantissa) / 16_777_216.0 // half subnormal: mantissa * 2^-24
    } else {
        f32::from_bits((u32::from(exponent) + 112) << 23 | u32::from(mantissa) << 13)
    };
    // DISPLAYCONFIG_SDR_WHITE_LEVEL uses 1000 units for 80 nits. Non-HDR
    // Advanced Color desktops instead use reference white 1.0 (white = 1000).
    let normalized = (f64::from(linear) * 1000.0 / f64::from(white)).clamp(0.0, 1.0);
    let srgb = if normalized <= 0.003_130_8 {
        normalized * 12.92
    } else {
        1.055 * normalized.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(values: &[[u16; 4]], padding: usize) -> Vec<u8> {
        let mut bytes: Vec<_> = values
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        bytes.resize(bytes.len() + padding, 0xff);
        bytes
    }

    #[test]
    fn reference_white_and_srgb_transfer_preserve_desktop_colors() {
        let source = pixels(&[[0x3800, 0x3400, 0x3c00, 0]], 0); // 0.5, 0.25, 1
        let image = Converter::new(1000)
            .unwrap()
            .frame(&source, 1, 1, 8, 0)
            .unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [188, 137, 255, 255]);
        let hdr = pixels(&[[0x3c00, 0x4000, 0x4200, 0x7e00]], 0); // 1, 2, 3
        let image = Converter::new(3000)
            .unwrap()
            .frame(&hdr, 1, 1, 8, 0)
            .unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [156, 213, 255, 255]);
        assert_eq!(channel(0x1400, 1000), 3); // 2^-10, linear segment
    }

    #[test]
    fn negative_nonfinite_and_extended_range_cannot_wrap() {
        let c = Converter::new(1000).unwrap();
        for bits in [0u16, 1, 0x03ff, 0x8000, 0xbc00, 0xfc00, 0x7e00, 0x7fff] {
            assert_eq!(c.channels[usize::from(bits)], 0);
        }
        for bits in [0x3c00u16, 0x4a40, 0x7bff, 0x7c00] {
            assert_eq!(c.channels[usize::from(bits)], 255);
        }
        assert!(c.channels[..0x7c00].windows(2).all(|v| v[0] <= v[1]));
    }

    #[test]
    fn white_changes_rebuild_only_when_needed_and_zero_is_rejected() {
        assert!(Converter::new(0).is_err());
        let mut c = Converter::new(1000).unwrap();
        let allocation = c.channels.as_ptr();
        c.update(1000).unwrap();
        assert_eq!(allocation, c.channels.as_ptr());
        c.update(2000).unwrap();
        assert_eq!(c.channels[0x3c00], 188);
        assert_eq!(c.channels[0x4000], 255);
        assert!(c.update(0).is_err());
        assert_eq!(c.channels[0x3c00], 188);
    }

    #[test]
    fn mapped_rows_rotation_alpha_and_invalid_layouts() {
        let colors = [
            [0x3c00, 0, 0, 0],
            [0, 0x3c00, 0, 0],
            [0, 0, 0x3c00, 0],
            [0x3c00; 4],
            [0x3c00, 0x3c00, 0, 0],
            [0, 0x3c00, 0x3c00, 0],
        ];
        let mut source = pixels(&colors[..2], 8);
        source.extend(pixels(&colors[2..4], 8));
        source.extend(pixels(&colors[4..], 0));
        let c = Converter::new(1000).unwrap();
        let expected = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255; 4],
            [255, 255, 0, 255],
            [0, 255, 255, 255],
        ];
        for (rotation, order) in [
            (0, [0, 1, 2, 3, 4, 5]),
            (1, [4, 2, 0, 5, 3, 1]),
            (2, [5, 4, 3, 2, 1, 0]),
            (3, [1, 3, 5, 0, 2, 4]),
        ] {
            let image = c.frame(&source, 2, 3, 24, rotation).unwrap();
            assert_eq!(
                image.dimensions(),
                if rotation % 2 == 0 { (2, 3) } else { (3, 2) }
            );
            assert_eq!(
                image.pixels().map(|p| p.0).collect::<Vec<_>>(),
                order.map(|i| expected[i])
            );
        }
        for (width, height, pitch, rotation) in [
            (0, 2, 24, 0),
            (2, 0, 24, 0),
            (2, 2, 8, 0),
            (2, 2, 24, 4),
            (2, 2, usize::MAX, 0),
        ] {
            assert!(c.frame(&source, width, height, pitch, rotation).is_err());
        }
        assert!(c.frame(&source[..39], 2, 2, 24, 0).is_err());
        assert!(c.frame(&source, u32::MAX, u32::MAX, usize::MAX, 0).is_err());
    }

    #[test]
    fn hdr_desktop_reference_colors_survive_the_actual_sdr_codec() {
        use crate::video::{
            VideoProfile,
            software::{Encoder, Frame},
        };
        use openh264::formats::YUVSource;
        use std::{
            sync::{Arc, Mutex},
            time::Instant,
        };
        let colors = [
            [0x4200, 0, 0, 0],
            [0, 0x4200, 0, 0],
            [0, 0, 0x4200, 0],
            [0x3c00; 4],
        ];
        let pitch = 64 * 8 + 16;
        let mut source = vec![0xff; pitch * 64];
        for y in 0..64 {
            for x in 0..64 {
                let color = colors[usize::from(x >= 32) + 2 * usize::from(y >= 32)];
                source[y * pitch + x * 8..y * pitch + x * 8 + 8]
                    .copy_from_slice(&pixels(&[color], 0));
            }
        }
        let image = Converter::new(3000)
            .unwrap()
            .frame(&source, 64, 64, pitch, 0)
            .unwrap();
        let encoded = Arc::new(Mutex::new(Vec::new()));
        let sink = encoded.clone();
        let mut encoder = Encoder::with_profile(
            64,
            64,
            30,
            2_000_000,
            Arc::new(move |frame| sink.lock().unwrap().push(frame)),
            VideoProfile::ConstrainedBaseline31,
        )
        .unwrap();
        assert!(
            encoder
                .encode(
                    &Frame {
                        cursor_embedded: Some(false),
                        image: Arc::new(image)
                    },
                    Instant::now(),
                    true
                )
                .unwrap()
        );
        let output = encoded.lock().unwrap();
        assert_eq!(output.len(), 1);
        assert!(output[0].keyframe);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let decoded = decoder.decode(&output[0].data).unwrap().unwrap();
        assert_eq!(decoded.dimensions(), (64, 64));
        let mut rgb = vec![0; 64 * 64 * 3];
        decoded.write_rgb8(&mut rgb);
        for (x, y, expected) in [
            (16, 16, [255u8, 0, 0]),
            (48, 16, [0, 255, 0]),
            (16, 48, [0, 0, 255]),
            (48, 48, [156, 156, 156]),
        ] {
            let pixel = &rgb[(y * 64 + x) * 3..(y * 64 + x) * 3 + 3];
            assert!(
                pixel
                    .iter()
                    .zip(expected)
                    .all(|(actual, wanted)| actual.abs_diff(wanted) <= 16),
                "{pixel:?} != {expected:?}"
            );
        }
    }
}
