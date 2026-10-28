//! Normalize the visible buffer region to its intended display orientation.
use crate::Result;
use image::RgbaImage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Presentation {
    pub crop: Option<(i32, i32, u32, u32)>,
    /// SPA describes the counter-clockwise transformation already applied to
    /// storage. Undo it for rendering; mirrored transformations are involutions.
    pub transform: u32,
}
impl Presentation {
    pub fn apply(self, image: RgbaImage) -> Result<RgbaImage> {
        if self.transform > 7 {
            return Err("Wayland 画面方向元数据无效".into());
        }
        let (x, y, w, h) = self.crop.unwrap_or((0, 0, image.width(), image.height()));
        if x < 0
            || y < 0
            || w == 0
            || h == 0
            || (x as u32)
                .checked_add(w)
                .is_none_or(|end| end > image.width())
            || (y as u32)
                .checked_add(h)
                .is_none_or(|end| end > image.height())
        {
            return Err("Wayland 画面裁剪区域超出缓冲区".into());
        }
        if self.transform == 0 && (x, y, w, h) == (0, 0, image.width(), image.height()) {
            return Ok(image);
        }
        let size = if self.transform & 1 == 1 {
            (h, w)
        } else {
            (w, h)
        };
        let mut output = RgbaImage::new(size.0, size.1);
        if self.transform == 0 {
            let row = w as usize * 4;
            for (line, target) in output.as_mut().chunks_exact_mut(row).enumerate() {
                let offset = ((y as usize + line) * image.width() as usize + x as usize) * 4;
                target.copy_from_slice(&image.as_raw()[offset..offset + row]);
            }
            return Ok(output);
        }
        for (dx, dy, target) in output.enumerate_pixels_mut() {
            let (sx, sy) = match self.transform {
                0 => (dx, dy),
                1 => (dy, h - 1 - dx),
                2 => (w - 1 - dx, h - 1 - dy),
                3 => (w - 1 - dy, dx),
                4 => (w - 1 - dx, dy),
                5 => (dy, dx),
                6 => (dx, h - 1 - dy),
                7 => (w - 1 - dy, h - 1 - dx),
                _ => unreachable!(),
            };
            *target = *image.get_pixel(x as u32 + sx, y as u32 + sy);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn numbered() -> RgbaImage {
        RgbaImage::from_fn(3, 2, |x, y| image::Rgba([(y * 3 + x + 1) as u8, 0, 0, 255]))
    }
    #[test]
    fn all_spa_orientations_are_undone_including_mirror_order() {
        let expected = [
            vec![1, 2, 3, 4, 5, 6],
            vec![4, 1, 5, 2, 6, 3],
            vec![6, 5, 4, 3, 2, 1],
            vec![3, 6, 2, 5, 1, 4],
            vec![3, 2, 1, 6, 5, 4],
            vec![1, 4, 2, 5, 3, 6],
            vec![4, 5, 6, 1, 2, 3],
            vec![6, 3, 5, 2, 4, 1],
        ];
        for (transform, expected) in expected.into_iter().enumerate() {
            let image = Presentation {
                transform: transform as u32,
                ..Presentation::default()
            }
            .apply(numbered())
            .unwrap();
            assert_eq!(
                image.dimensions(),
                if transform & 1 == 1 { (2, 3) } else { (3, 2) }
            );
            assert_eq!(image.pixels().map(|p| p[0]).collect::<Vec<_>>(), expected);
        }
    }
    #[test]
    fn crop_coordinates_belong_to_storage_before_orientation_is_undone() {
        let image = RgbaImage::from_fn(5, 4, |x, y| image::Rgba([(y * 5 + x) as u8, 0, 0, 255]));
        let output = Presentation {
            crop: Some((1, 1, 3, 2)),
            transform: 1,
        }
        .apply(image)
        .unwrap();
        assert_eq!(output.dimensions(), (2, 3));
        assert_eq!(
            output.pixels().map(|p| p[0]).collect::<Vec<_>>(),
            [11, 6, 12, 7, 13, 8]
        );
    }
    #[test]
    fn malformed_crop_or_transform_is_rejected_instead_of_showing_padding() {
        for crop in [
            (-1, 0, 2, 2),
            (0, -1, 2, 2),
            (0, 0, 0, 2),
            (0, 0, 2, 0),
            (2, 0, 2, 2),
            (0, 1, 3, 2),
            (i32::MAX, 0, u32::MAX, 2),
        ] {
            assert!(
                Presentation {
                    crop: Some(crop),
                    ..Presentation::default()
                }
                .apply(numbered())
                .is_err()
            );
        }
        assert!(
            Presentation {
                transform: 8,
                ..Presentation::default()
            }
            .apply(numbered())
            .is_err()
        );
    }
}
