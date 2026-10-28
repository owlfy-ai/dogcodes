use super::{MAX_SIDE, Shape, Snapshot, straight};
use x11rb::{
    connection::Connection,
    protocol::{xfixes::ConnectionExt as _, xproto::ConnectionExt as _},
    rust_connection::RustConnection,
};
#[derive(Default)]
pub(super) struct Native(Option<(RustConnection, u32)>);
impl Native {
    pub fn read(&mut self) -> Option<Snapshot> {
        if crate::wayland::available() {
            return None;
        }
        if self.0.is_none() {
            let (connection, screen) = x11rb::connect(None).ok()?;
            let root = connection.setup().roots[screen].root;
            let _ = connection
                .xfixes_query_version(5, 0)
                .ok()
                .and_then(|reply| reply.reply().ok());
            self.0 = Some((connection, root));
        }
        let (connection, root) = self.0.as_ref()?;
        let pointer = connection.query_pointer(*root).ok()?.reply().ok()?;
        if !pointer.same_screen {
            return None;
        }
        let shape = (|| {
            let image = connection.xfixes_get_cursor_image().ok()?.reply().ok()?;
            let (w, h) = (u32::from(image.width), u32::from(image.height));
            if w == 0
                || h == 0
                || w > MAX_SIDE
                || h > MAX_SIDE
                || image.cursor_image.len() != (w * h) as usize
            {
                return None;
            }
            let pixels = image
                .cursor_image
                .into_iter()
                .flat_map(|p| {
                    let a = (p >> 24) as u8;
                    [
                        straight((p >> 16) as u8, a),
                        straight((p >> 8) as u8, a),
                        straight(p as u8, a),
                        a,
                    ]
                })
                .collect();
            Some(Shape {
                image: image::RgbaImage::from_raw(w, h, pixels)?,
                hot: (u32::from(image.xhot), u32::from(image.yhot)),
            })
        })();
        Some(Snapshot {
            x: i32::from(pointer.root_x),
            y: i32::from(pointer.root_y),
            shape,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, CreateGCAux, Rectangle};
    #[test]
    #[ignore = "requires an explicitly isolated Xvfb; never a real desktop"]
    fn owned_xvfb_cursor_has_native_pixels_hotspot_and_recovers_after_shape_cache_loss() {
        assert_eq!(
            std::env::var("DOGCODES_CURSOR_X11_FIXTURE").as_deref(),
            Ok("owned-xvfb")
        );
        assert_eq!(std::env::var("DISPLAY").as_deref(), Ok(":97"));
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let source = connection.generate_id().unwrap();
        let mask = connection.generate_id().unwrap();
        let gc = connection.generate_id().unwrap();
        let cursor = connection.generate_id().unwrap();
        for pixmap in [source, mask] {
            connection
                .create_pixmap(1, pixmap, root, 8, 8)
                .unwrap()
                .check()
                .unwrap();
        }
        connection
            .create_gc(gc, source, &CreateGCAux::new().foreground(1))
            .unwrap()
            .check()
            .unwrap();
        for pixmap in [source, mask] {
            connection
                .poly_fill_rectangle(
                    pixmap,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: 8,
                        height: 8,
                    }],
                )
                .unwrap()
                .check()
                .unwrap();
        }
        let mut native = Native::default();
        let mut cache = crate::cursor::Cache::default();
        let mut known = None;
        for (red, blue) in [(65535, 0), (0, 65535)] {
            connection
                .create_cursor(cursor, source, mask, red, 0, blue, 0, 0, 0, 2, 3)
                .unwrap()
                .check()
                .unwrap();
            connection
                .change_window_attributes(root, &ChangeWindowAttributesAux::new().cursor(cursor))
                .unwrap()
                .check()
                .unwrap();
            connection
                .warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, 120, 80)
                .unwrap()
                .check()
                .unwrap();
            let snapshot = native.read().unwrap();
            assert_eq!((snapshot.x, snapshot.y), (120, 80));
            let shape = snapshot.shape.as_ref().unwrap();
            assert_eq!(shape.image.dimensions(), (8, 8));
            assert_eq!(shape.hot, (2, 3));
            assert_eq!(
                shape.image.get_pixel(0, 0).0,
                [(red / 257) as u8, 0, (blue / 257) as u8, 255]
            );
            let (id, full) = cache.describe(Some(shape), known.as_deref()).unwrap();
            assert!(
                full.is_some(),
                "changed pixels on a reused cursor ID must be sent"
            );
            assert!(cache.describe(Some(shape), Some(&id)).unwrap().1.is_none());
            assert!(cache.describe(Some(shape), None).unwrap().1.is_some());
            known = Some(id);
            connection.free_cursor(cursor).unwrap().check().unwrap();
        }
    }
}
