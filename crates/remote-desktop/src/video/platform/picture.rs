//! Lazy JPEG source using the same owned SCK stream as video, with BGRA
//! output and the system cursor composited by SCK. One retained latest frame;
//! no encoding or CPU copies on the framework's delivery queue.
use super::{Capture, Frame};
use crate::{
    Result,
    video::{VideoConfig, VideoProfile},
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetDataSize, CVPixelBufferGetPixelFormatType, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Frame>>,
    changed: Condvar,
}
impl Latest {
    fn replace(&self, frame: Option<Frame>) {
        *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = frame;
        self.changed.notify_all();
    }
}
enum State {
    New,
    Running(Capture),
    Unavailable,
}
pub(crate) struct Picture {
    display: u32,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
    state: State,
    latest: Arc<Latest>,
    pending: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
}
impl Picture {
    pub fn new(display: u32, alive: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            display,
            alive,
            state: State::New,
            latest: Arc::new(Latest::default()),
            pending: Arc::new(AtomicBool::new(false)),
            changed: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn idle(&mut self) {
        if matches!(self.state, State::Running(_)) {
            self.state = State::New;
            self.latest.replace(None);
            // Late stop/display callbacks retain the old generation only.
            self.latest = Arc::new(Latest::default());
            self.pending = Arc::new(AtomicBool::new(false));
            self.changed = Arc::new(AtomicBool::new(false));
        }
    }
    pub fn check_owner(&self) -> Result<()> {
        if (self.alive)() {
            Ok(())
        } else {
            Err("远控租约已结束，请重新连接".into())
        }
    }
    pub fn check_current(&self) -> Result<()> {
        self.check_owner()?;
        if self.pending.load(Ordering::Acquire) || self.changed.load(Ordering::Acquire) {
            return Err("显示器配置已改变，请重新读取画面".into());
        }
        match &self.state {
            State::Running(capture) => capture.error().map_or(Ok(()), Err),
            _ => Err("图片采集尚未就绪".into()),
        }
    }
    /// A backend failure leaves the independent xcap fallback available. Do
    /// not repeatedly start streams on every pull after the original failure.
    pub fn frame(&mut self) -> Result<Option<image::RgbaImage>> {
        self.check_owner()?;
        if matches!(self.state, State::New) {
            self.state = State::Unavailable;
            // Recovery and fallback must not trigger a new permission prompt.
            if crate::video::check_recovery_source(&None).is_ok() {
                let received = self.latest.clone();
                let retired = self.latest.clone();
                if let Ok((capture, _)) = Capture::start_with_format(
                    VideoConfig {
                        display: self.display,
                        max_width: 2560,
                        fps: 15,
                        bitrate: 100_000,
                        profile: VideoProfile::ConstrainedBaselineAuto,
                    },
                    Box::new(move |frame| received.replace(Some(frame))),
                    self.alive.clone(),
                    move |_| retired.replace(None),
                    self.pending.clone(),
                    self.changed.clone(),
                    kCVPixelFormatType_32BGRA,
                ) {
                    self.state = State::Running(capture);
                }
            }
        }
        self.check_owner()?;
        let State::Running(capture) = &self.state else {
            return Ok(None);
        };
        // The same worker probes geometry even when its FPS is unchanged.
        capture.set_fps(15);
        if self.check_current().is_err() {
            self.state = State::Unavailable;
            self.latest.replace(None);
            return Ok(None);
        }
        let pixels = {
            let frame = self.latest.frame.lock().unwrap_or_else(|e| e.into_inner());
            let (frame, _) = self
                .latest
                .changed
                .wait_timeout_while(frame, Duration::from_millis(300), |frame| frame.is_none())
                .unwrap_or_else(|e| e.into_inner());
            frame.clone()
        };
        self.check_owner()?;
        let Some(frame) = pixels else {
            return Ok(None);
        };
        let image = rgba(&frame);
        self.check_owner()?;
        if self.check_current().is_err() || image.is_err() {
            self.state = State::Unavailable;
            self.latest.replace(None);
            return Ok(None);
        }
        image.map(Some)
    }
}

struct ReadPixels<'a>(&'a CVPixelBuffer);
impl Drop for ReadPixels<'_> {
    fn drop(&mut self) {
        // SAFETY: paired with the successful read lock that created this guard.
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
        }
    }
}
fn rgba(frame: &Frame) -> Result<image::RgbaImage> {
    let buffer = &frame.0;
    let (width, height) = frame.size();
    if width == 0
        || height == 0
        || width > 3840
        || height > 3840
        || u64::from(width) * u64::from(height) > 3840 * 2160
        || CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA
    {
        return Err("图片采集格式或尺寸无效".into());
    }
    // SAFETY: the retained pixel buffer stays alive through the matching guard.
    let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
    if status != 0 {
        return Err(format!("无法读取图片采集像素（{status}）"));
    }
    let _locked = ReadPixels(buffer);
    let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let row = width as usize * 4;
    let required = stride
        .checked_mul(height as usize - 1)
        .and_then(|n| n.checked_add(row));
    if base.is_null()
        || stride < row
        || required.is_none_or(|n| n > CVPixelBufferGetDataSize(buffer))
    {
        return Err("图片采集像素布局无效".into());
    }
    let mut pixels = image::RgbaImage::new(width, height);
    for (y, output) in pixels.as_mut().chunks_exact_mut(row).enumerate() {
        // SAFETY: the retained BGRA buffer is read-locked, and the full row
        // range, stride and total data size were checked above.
        let input = unsafe { std::slice::from_raw_parts(base.add(y * stride), row) };
        for (input, output) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
            output.copy_from_slice(&[input[2], input[1], input[0], 255]);
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::{capture_worker, display_changes, worker};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFType};
    use objc2_core_video::{CVPixelBufferCreate, kCVPixelBufferBytesPerRowAlignmentKey};
    use std::{ptr::NonNull, sync::mpsc};

    fn pixels(red: u8) -> Frame {
        let alignment = CFNumber::new_isize(64);
        // SAFETY: static CFString keys and number values live across creation.
        let attributes = unsafe {
            CFDictionary::from_slices(
                &[kCVPixelBufferBytesPerRowAlignmentKey],
                &[alignment.as_ref() as &CFType],
            )
        };
        let mut raw = std::ptr::null_mut();
        // SAFETY: CoreVideo returns a +1 buffer retained below. All writes are
        // confined to the checked stride/height while the buffer is locked.
        unsafe {
            assert_eq!(
                CVPixelBufferCreate(
                    None,
                    18,
                    8,
                    kCVPixelFormatType_32BGRA,
                    Some(attributes.as_opaque()),
                    NonNull::from(&mut raw)
                ),
                0
            );
            let buffer = CFRetained::from_raw(NonNull::new(raw).unwrap());
            assert_eq!(
                CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()),
                0
            );
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            assert!(stride > 18 * 4, "fixture must exercise padded rows");
            assert!(CVPixelBufferGetDataSize(&buffer) >= stride * 8);
            for y in 0..8 {
                let row = std::slice::from_raw_parts_mut(base.add(y * stride), stride);
                row.fill(0xee);
                for px in row[..18 * 4].chunks_exact_mut(4) {
                    px.copy_from_slice(&[7, 29, red, 255]);
                }
            }
            std::slice::from_raw_parts_mut(base.add(2 * stride + 3 * 4), 4)
                .copy_from_slice(&[240, 90, 10, 255]);
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
            Frame(buffer)
        }
    }

    fn running() -> (
        Picture,
        Arc<capture_worker::Delivery<Frame>>,
        Arc<AtomicBool>,
    ) {
        let alive = Arc::new(AtomicBool::new(true));
        let watch = alive.clone();
        let mut picture = Picture::new(7, Arc::new(move || watch.load(Ordering::Acquire)));
        let frames = picture.latest.clone();
        let retired = picture.latest.clone();
        let (sent, received) = mpsc::channel();
        let capture = capture_worker::Capture::start_with_hook(
            move |delivery| {
                sent.send(delivery).unwrap();
                Ok((
                    (),
                    capture_worker::Info {
                        size: (18, 8),
                        native: (18, 8),
                    },
                ))
            },
            |_, _| Ok(()),
            move |frame| frames.replace(Some(frame)),
            move |_| retired.replace(None),
            picture.alive.clone(),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        picture.state = State::Running(Capture(
            capture,
            Arc::new(display_changes::Changes::new(
                picture.pending.clone(),
                picture.changed.clone(),
            )),
        ));
        (picture, received.recv().unwrap(), alive)
    }

    #[test]
    fn bgra_rows_are_read_with_stride_and_color_order_and_video_planes_are_rejected() {
        let frame = pixels(131);
        let rgba = rgba(&frame).unwrap();
        assert_eq!(rgba.dimensions(), (18, 8));
        assert_eq!(rgba.get_pixel(3, 2).0, [10, 90, 240, 255]);
        for (x, y) in [(0, 0), (17, 0), (17, 7), (3, 3)] {
            assert_eq!(rgba.get_pixel(x, y).0, [131, 29, 7, 255]);
        }
        assert!(super::rgba(&Frame::synthetic(480, 320, 0).unwrap()).is_err());
    }

    #[test]
    fn latest_picture_replaces_old_pixels_and_retired_owner_cannot_publish_or_restart() {
        let _serial = worker::isolated_capture();
        let (mut picture, delivery, alive) = running();
        delivery.emit(pixels(80));
        delivery.emit(pixels(180));
        let image = picture.frame().unwrap().unwrap();
        assert_eq!(image.get_pixel(17, 7).0, [180, 29, 7, 255]);
        picture.check_current().unwrap();
        alive.store(false, Ordering::Release);
        delivery.emit(pixels(220));
        assert!(picture.latest.frame.lock().unwrap().is_none());
        assert!(picture.frame().is_err());
        assert!(picture.check_current().is_err());
    }

    #[test]
    fn changed_geometry_or_capture_failure_discards_retained_picture_without_restart() {
        let _serial = worker::isolated_capture();
        let (mut picture, delivery, _) = running();
        delivery.emit(pixels(80));
        picture.pending.store(true, Ordering::Release);
        assert!(picture.check_current().is_err());
        assert!(picture.frame().unwrap().is_none());
        assert!(picture.latest.frame.lock().unwrap().is_none());
        assert!(matches!(picture.state, State::Unavailable));
        assert!(picture.frame().unwrap().is_none());
        drop(picture);
        let (mut picture, delivery, _) = running();
        delivery.emit(pixels(90));
        delivery.fail("original capture stopped".into());
        assert!(picture.frame().unwrap().is_none());
        picture.idle();
        assert!(matches!(picture.state, State::Unavailable));
        assert!(picture.frame().unwrap().is_none());
    }

    #[test]
    fn idle_image_stream_releases_original_worker_and_image_configuration_embeds_cursor() {
        let _serial = worker::isolated_capture();
        let (mut picture, delivery, _) = running();
        delivery.emit(pixels(80));
        let configuration = super::super::configuration((480, 320), 5, kCVPixelFormatType_32BGRA);
        // SAFETY: accessors on the owned configuration; this never starts SCK.
        unsafe {
            assert!(configuration.showsCursor());
            assert_eq!(configuration.pixelFormat(), kCVPixelFormatType_32BGRA);
            assert_eq!(configuration.queueDepth(), 3);
            let video = super::super::configuration(
                (480, 320),
                30,
                objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            );
            assert!(video.showsCursor());
            assert_eq!(
                video.pixelFormat(),
                objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            );
            assert_eq!(video.queueDepth(), 6);
        }
        assert_eq!(worker::capture_count(), 1);
        picture.idle();
        assert!(picture.latest.frame.lock().unwrap().is_none());
        assert!(delivery.check_active().is_err());
        assert!(matches!(picture.state, State::New));
        picture.latest.replace(Some(pixels(190)));
        delivery.retire();
        assert!(
            picture.latest.frame.lock().unwrap().is_some(),
            "late retirement cleared a replacement source's cache"
        );
    }
}
