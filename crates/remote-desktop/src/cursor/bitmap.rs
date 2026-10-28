//! Bounded Windows mask conversion, also exercised without Windows/GDI.
use super::{MAX_SIDE, Shape, straight};
use image::{Rgba, RgbaImage};
pub(super) fn convert(
    w: u32,
    h: u32,
    hot: (u32, u32),
    mask: &[u8],
    color: Option<&[u8]>,
) -> Option<Shape> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || hot.0 >= w || hot.1 >= h {
        return None;
    }
    let len = (w * h * 4) as usize;
    if mask.len() != len * if color.is_some() { 1 } else { 2 }
        || color.is_some_and(|b| b.len() != len)
    {
        return None;
    }
    let alpha = color.is_some_and(|b| b.chunks_exact(4).any(|p| p[3] != 0));
    let mut image = RgbaImage::new(w, h);
    let mut contrast = false;
    for (i, pixel) in image.pixels_mut().enumerate() {
        let offset = i * 4;
        let and = mask[offset] != 0;
        *pixel = if let Some(color) = color {
            let p = &color[offset..offset + 4];
            if alpha {
                Rgba([
                    straight(p[2], p[3]),
                    straight(p[1], p[3]),
                    straight(p[0], p[3]),
                    p[3],
                ])
            } else if and && p[..3] == [0, 0, 0] {
                Rgba([0, 0, 0, 0])
            } else {
                contrast |= and;
                Rgba([p[2], p[1], p[0], 255])
            }
        } else {
            let xor = mask[len + offset] != 0;
            if and && !xor {
                Rgba([0, 0, 0, 0])
            } else {
                contrast |= and && xor;
                let c = if xor { 255 } else { 0 };
                Rgba([c, c, c, 255])
            }
        };
    }
    // Destination-dependent XOR pixels cannot be represented by PNG alpha.
    // A contrasting outline keeps those native I-beams visible on either
    // background, without a framebuffer readback or guessing the target color.
    if contrast {
        let mut outlined = RgbaImage::new(w + 2, h + 2);
        for (x, y, _) in image.enumerate_pixels().filter(|(_, _, p)| p[3] != 0) {
            for dy in 0..3 {
                for dx in 0..3 {
                    outlined.put_pixel(x + dx, y + dy, Rgba([0, 0, 0, 255]));
                }
            }
        }
        for (x, y, p) in image.enumerate_pixels().filter(|(_, _, p)| p[3] != 0) {
            outlined.put_pixel(x + 1, y + 1, *p);
        }
        Some(Shape {
            image: outlined,
            hot: (hot.0 + 1, hot.1 + 1),
        })
    } else {
        Some(Shape { image, hot })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monochrome_and_xor_masks_keep_transparency_and_visible_inversion() {
        let mut mask = vec![0; 4 * 4 * 2];
        // All four AND/XOR combinations, one row.
        mask[8] = 255;
        mask[12] = 255;
        mask[16 + 4] = 255;
        mask[16 + 12] = 255;
        let shape = convert(4, 1, (1, 0), &mask, None).unwrap();
        assert_eq!(shape.image.dimensions(), (6, 3));
        assert_eq!(shape.hot, (2, 1));
        assert_eq!(shape.image.get_pixel(1, 1).0, [0, 0, 0, 255]);
        assert_eq!(shape.image.get_pixel(2, 1).0, [255, 255, 255, 255]);
        assert_eq!(shape.image.get_pixel(4, 1).0, [255, 255, 255, 255]);
    }
    #[test]
    fn alpha_and_legacy_color_masks_have_distinct_transparency_rules() {
        let mask = [255, 255, 255, 0];
        let shape = convert(1, 1, (0, 0), &mask, Some(&[16, 32, 64, 128])).unwrap();
        assert_eq!(shape.image.get_pixel(0, 0).0, [128, 64, 32, 128]);
        let shape = convert(1, 1, (0, 0), &mask, Some(&[0, 0, 0, 0])).unwrap();
        assert_eq!(shape.image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert!(convert(1, 1, (0, 0), &mask[..3], Some(&[0, 0, 0, 0])).is_none());
        assert!(convert(1, 1, (0, 0), &mask, Some(&[0, 0, 0])).is_none());
    }
}
