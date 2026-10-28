//! Pull-only images reuse the video capture controller and native worker pool.
//! The facade is synchronous; only its source uses the existing native pool.
//! It adds no continuous capture, timer, or coordinator thread.
use super::{Result, capture_source::Cancellation, software::Frame};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

type Producer = Box<dyn FnMut() -> Result<Frame>>;
pub(crate) struct Picture {
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
    factory: Box<dyn Fn(Cancellation) -> Producer>,
    source: Option<Producer>,
    retired: Arc<AtomicBool>,
}
impl Picture {
    #[cfg(target_os = "windows")]
    pub fn new(display: u32, name: String, alive: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self::with_factory(alive, move |cancelled| {
            Box::new(super::dxgi::capture(
                display,
                name.clone(),
                cancelled,
                Arc::new(AtomicBool::new(false)),
            ))
        })
    }
    fn with_factory(
        alive: Arc<dyn Fn() -> bool + Send + Sync>,
        factory: impl Fn(Cancellation) -> Producer + 'static,
    ) -> Self {
        Self {
            alive,
            factory: Box::new(factory),
            source: None,
            retired: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn check_owner(&self) -> Result<()> {
        if !(self.alive)() || self.retired.load(Ordering::Acquire) {
            self.retired.store(true, Ordering::Release);
            return Err("远控租约已结束，请重新连接".into());
        }
        Ok(())
    }
    pub fn check_current(&self) -> Result<()> {
        self.check_owner()?;
        if self.source.is_none() {
            return Err("图片采集尚未就绪".into());
        }
        Ok(())
    }
    pub fn frame(&mut self) -> Result<(Arc<image::RgbaImage>, Option<bool>)> {
        self.check_owner()?;
        if self.source.is_none() {
            self.source = Some((self.factory)(
                Cancellation::from(self.retired.clone()).with_owner(self.alive.clone()),
            ));
        }
        self.check_owner()?;
        let result = self.source.as_mut().ok_or("图片采集尚未就绪")?();
        self.check_owner()?;
        result.map(|frame| (frame.image, frame.cursor_embedded))
    }
    pub fn idle(&mut self) {
        let can_resume = self.check_owner().is_ok();
        if let Some(source) = self.source.take() {
            self.retired.store(true, Ordering::Release);
            drop(source);
            if can_resume {
                self.retired = Arc::new(AtomicBool::new(false));
            }
        }
    }
}
impl Drop for Picture {
    fn drop(&mut self) {
        self.retired.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    fn frame(value: u8) -> Frame {
        Frame {
            image: Arc::new(image::RgbaImage::from_pixel(
                2,
                2,
                image::Rgba([value, 0, 0, 255]),
            )),
            cursor_embedded: None,
        }
    }
    #[test]
    fn lazy_reuse_idle_and_drop_keep_the_original_cancellation() {
        let starts = Rc::new(Cell::new(0u8));
        let cancellations = Rc::new(RefCell::new(Vec::new()));
        let count = starts.clone();
        let cancel = cancellations.clone();
        let mut picture = Picture::with_factory(Arc::new(|| true), move |cancelled| {
            count.set(count.get() + 1);
            let value = count.get();
            cancel.borrow_mut().push(cancelled);
            Box::new(move || Ok(frame(value)))
        });
        assert_eq!(starts.get(), 0);
        assert!(picture.check_current().is_err());
        assert_eq!(picture.frame().unwrap().0.get_pixel(0, 0)[0], 1);
        assert!(picture.frame().unwrap().1.is_none());
        assert_eq!(starts.get(), 1);
        picture.idle();
        assert!(cancellations.borrow()[0].is_cancelled());
        assert!(picture.check_current().is_err());
        assert_eq!(picture.frame().unwrap().0.get_pixel(0, 0)[0], 2);
        assert!(!cancellations.borrow()[1].is_cancelled());
        drop(picture);
        assert!(
            cancellations
                .borrow()
                .iter()
                .all(Cancellation::is_cancelled)
        );
    }
    #[test]
    fn revoked_owner_cannot_start_or_deliver_after_prepare() {
        let alive = Arc::new(AtomicBool::new(false));
        let owner = alive.clone();
        let revoke = alive.clone();
        let calls = Rc::new(Cell::new(0));
        let called = calls.clone();
        let mut picture =
            Picture::with_factory(Arc::new(move || owner.load(Ordering::Acquire)), move |_| {
                revoke.store(false, Ordering::Release);
                let called = called.clone();
                Box::new(move || {
                    called.set(called.get() + 1);
                    Ok(frame(20))
                })
            });
        assert!(picture.frame().is_err());
        assert_eq!(calls.get(), 0);
        alive.store(true, Ordering::Release);
        // An already revoked facade never resumes under a replacement owner.
        picture.idle();
        assert!(picture.frame().is_err());
        assert_eq!(calls.get(), 0);

        let owner = alive.clone();
        let revoke = alive.clone();
        let mut picture =
            Picture::with_factory(Arc::new(move || owner.load(Ordering::Acquire)), move |_| {
                revoke.store(false, Ordering::Release);
                Box::new(|| panic!("source ran after prepare revoked its original owner"))
            });
        assert!(picture.frame().is_err());
    }
    #[test]
    fn owner_revoked_during_frame_cancels_native_work_and_rejects_pixels() {
        let alive = Arc::new(AtomicBool::new(true));
        let owner = alive.clone();
        let revoke = alive.clone();
        let mut picture = Picture::with_factory(
            Arc::new(move || owner.load(Ordering::Acquire)),
            move |cancelled| {
                let revoke = revoke.clone();
                Box::new(move || {
                    assert!(!cancelled.is_cancelled());
                    revoke.store(false, Ordering::Release);
                    assert!(cancelled.is_cancelled());
                    Ok(frame(240))
                })
            },
        );
        assert!(picture.frame().is_err());
        assert!(picture.check_current().is_err());
        picture.idle();
        alive.store(true, Ordering::Release);
        assert!(picture.frame().is_err());
    }
    #[test]
    fn normalized_hdr_picture_survives_the_actual_borrowed_jpeg_path() {
        let pixels: Vec<_> = (0..64 * 64)
            .flat_map(|_| [0x3c00u16; 4].into_iter().flat_map(u16::to_le_bytes))
            .collect();
        let normalized = super::super::scrgb::Converter::new(3000)
            .unwrap()
            .frame(&pixels, 64, 64, 64 * 8, 0)
            .unwrap();
        let image = Arc::new(normalized);
        let shared = image.clone();
        let mut picture = Picture::with_factory(Arc::new(|| true), move |_| {
            let image = shared.clone();
            Box::new(move || {
                Ok(Frame {
                    image: image.clone(),
                    cursor_embedded: Some(false),
                })
            })
        });
        let frame = picture.frame().unwrap();
        assert!(Arc::ptr_eq(&frame.0, &image));
        let thumbnail = crate::picture_thumbnail(frame.0.as_ref(), 320).unwrap();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 70)
            .encode_image(&thumbnail)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (320, 320));
        assert!(
            decoded
                .get_pixel(160, 160)
                .0
                .iter()
                .all(|value| value.abs_diff(156) <= 8)
        );
        assert!(Arc::ptr_eq(&frame.0, &image));
        assert!(picture.check_current().is_ok());
    }
}
