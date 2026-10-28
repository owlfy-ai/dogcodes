//! Read-only pointer snapshots; no input controller or extra permission prompt.
use crate::{Display, input_geometry, to_normalized};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageEncoder, RgbaImage};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
use linux::Native;
#[cfg(target_os = "windows")]
use windows::Native;
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Native {}
#[cfg(target_os = "macos")]
impl Native {
    fn read(&mut self) -> Option<Snapshot> {
        use objc2_core_graphics::CGEvent;
        let event = CGEvent::new(None)?;
        let p = CGEvent::location(Some(&event));
        if !p.x.is_finite()
            || !p.y.is_finite()
            || p.x.abs() > f64::from(i32::MAX)
            || p.y.abs() > f64::from(i32::MAX)
        {
            return None;
        }
        Some(Snapshot {
            x: p.x as i32,
            y: p.y as i32,
            shape: None,
        })
    }
}
const MAX_SIDE: u32 = 512;
const MAX_PNG: usize = 128 * 1024;
#[derive(Clone)]
struct Shape {
    image: RgbaImage,
    hot: (u32, u32),
}
struct Snapshot {
    x: i32,
    y: i32,
    shape: Option<Shape>,
}

#[derive(Default)]
struct Cache(Option<(String, Value)>);
impl Cache {
    fn describe(
        &mut self,
        shape: Option<&Shape>,
        known: Option<&str>,
    ) -> Option<(String, Option<Value>)> {
        let Some(shape) = shape else {
            self.0 = None;
            return None;
        };
        let (w, h) = shape.image.dimensions();
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || shape.hot.0 >= w || shape.hot.1 >= h
        {
            self.0 = None;
            return None;
        }
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (w, h, shape.hot, shape.image.as_raw()).hash(&mut hash);
        let id = format!("{:016x}", hash.finish());
        if self.0.as_ref().is_none_or(|(cached, _)| *cached != id) {
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(shape.image.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .ok()?;
            if png.len() > MAX_PNG {
                self.0 = None;
                return None;
            }
            self.0 = Some((
                id.clone(),
                json!({"id":id,"width":w,"height":h,"hotX":shape.hot.0,"hotY":shape.hot.1,"bytes":STANDARD.encode(png)}),
            ));
        }
        Some((
            id.clone(),
            (known != Some(id.as_str()))
                .then(|| self.0.as_ref().expect("encoded cursor").1.clone()),
        ))
    }
}
pub struct Reader {
    native: Native,
    geometry: input_geometry::Resolver,
    cache: Cache,
}
impl Default for Reader {
    fn default() -> Self {
        Self {
            native: Native::default(),
            geometry: input_geometry::Resolver::new(),
            cache: Cache::default(),
        }
    }
}
impl Reader {
    /// Only call for a native source; Wayland must stay on its original Portal.
    pub fn snapshot(&mut self, display: &Display, known: Option<&str>) -> Option<Value> {
        crate::require_unlocked().ok()?;
        let _context = input_geometry::Context::enter().ok()?;
        let current = self.geometry.current(display).ok()?;
        let snapshot = self.native.read()?;
        let (x, y) = to_normalized(&current, snapshot.x, snapshot.y)?;
        let mut value = json!({"x":x,"y":y,"displayWidth":current.width,"displayHeight":current.height,"shapeId":null});
        if let Some((id, shape)) = self.cache.describe(snapshot.shape.as_ref(), known) {
            value["shapeId"] = json!(id);
            if let Some(shape) = shape {
                value["shape"] = shape;
            }
        }
        crate::require_unlocked().ok()?;
        Some(value)
    }
}

// Native image formats are premultiplied; PNG carries straight alpha.
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn straight(color: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        ((u32::from(color) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
    }
}

#[cfg(any(target_os = "windows", test))]
mod bitmap;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_ack_avoids_pixels_but_a_lost_cache_can_recover() {
        let shape = Shape {
            image: RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255])),
            hot: (1, 0),
        };
        let mut cache = Cache::default();
        let (id, full) = cache.describe(Some(&shape), None).unwrap();
        let full = full.unwrap();
        let bytes = STANDARD.decode(full["bytes"].as_str().unwrap()).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().to_rgba8(),
            shape.image
        );
        assert!(cache.describe(Some(&shape), Some(&id)).unwrap().1.is_none());
        assert_eq!(cache.describe(Some(&shape), None).unwrap().1.unwrap(), full);
        let changed = Shape {
            hot: (0, 0),
            ..shape
        };
        assert_ne!(cache.describe(Some(&changed), Some(&id)).unwrap().0, id);
        assert!(cache.describe(None, None).is_none());
    }
    #[test]
    fn invalid_hotspot_and_extent_never_become_wire_images() {
        let mut cache = Cache::default();
        for (w, h, hot) in [
            (0, 1, (0, 0)),
            (513, 1, (0, 0)),
            (2, 2, (2, 0)),
            (2, 2, (0, 2)),
        ] {
            assert!(
                cache
                    .describe(
                        Some(&Shape {
                            image: RgbaImage::new(w, h),
                            hot
                        }),
                        None
                    )
                    .is_none()
            );
        }
        assert_eq!(straight(64, 128), 128);
        assert_eq!(straight(200, 128), 255);
        assert_eq!(straight(200, 0), 0);
    }
}
