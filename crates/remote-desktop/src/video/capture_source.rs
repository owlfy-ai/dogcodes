//! Desktop duplication recovery, independent of native objects and pixels.
//! A static desktop is healthy. Only a replacement waits for its first new frame.
use super::Result;
use image::RgbaImage;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
pub(super) enum Fault {
    Lost,
    Denied,
    Unavailable,
}
#[derive(Clone)]
pub(super) struct Cancellation {
    parent: Arc<AtomicBool>,
    local: Arc<AtomicBool>,
    owner: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl Cancellation {
    pub fn new(parent: Arc<AtomicBool>, local: Arc<AtomicBool>) -> Self {
        Self {
            parent,
            local,
            owner: None,
        }
    }
    pub fn with_owner(mut self, alive: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.owner = Some(alive);
        self
    }
    pub fn is_cancelled(&self) -> bool {
        self.parent.load(Ordering::Acquire)
            || self.local.load(Ordering::Acquire)
            || self.owner.as_ref().is_some_and(|alive| !alive())
    }
}
impl From<Arc<AtomicBool>> for Cancellation {
    fn from(parent: Arc<AtomicBool>) -> Self {
        Self::new(parent, Arc::new(AtomicBool::new(false)))
    }
}
pub(super) trait Driver {
    fn poll(&mut self, budget: Duration) -> std::result::Result<Option<Arc<RgbaImage>>, Fault>;
    fn cursor_embedded(&self) -> Option<bool> {
        Some(false)
    }
}

/// PointerPosition is valid only with a non-zero LastMouseUpdateTime.
/// An invisible separate pointer requires no client overlay (hidden/off-output
/// or already part of the desktop). Unknown stays explicit until valid input.
#[derive(Default)]
pub(super) struct PointerMetadata(Option<bool>);
impl PointerMetadata {
    pub fn observe(&mut self, updated: bool, separate_visible: bool) {
        if updated {
            self.0 = Some(!separate_visible);
        }
    }
    pub fn embedded(&self) -> Option<bool> {
        self.0
    }
}
pub(super) trait Clock {
    fn now(&self) -> Instant;
    fn wait(&self, duration: Duration);
}
pub(super) struct WallClock;
impl Clock for WallClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
/// Copy a mapped BGRA surface, respecting driver row padding and DXGI rotation.
pub(super) fn bgra(
    bytes: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
    rotation: u32,
) -> Result<RgbaImage> {
    let row = (width as usize).checked_mul(4).ok_or("视频行尺寸溢出")?;
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
        return Err("系统视频缓冲区无效".into());
    }
    let size = if rotation % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let mut image = RgbaImage::new(size.0, size.1);
    for y in 0..height {
        for x in 0..width {
            let offset = y as usize * pitch + x as usize * 4;
            let pixel = &bytes[offset..offset + 4];
            let (dx, dy) = match rotation {
                0 => (x, y),
                1 => (height - 1 - y, x),
                2 => (width - 1 - x, height - 1 - y),
                _ => (y, width - 1 - x),
            };
            image.put_pixel(dx, dy, image::Rgba([pixel[2], pixel[1], pixel[0], 255]));
        }
    }
    Ok(image)
}
pub(super) struct Source<D, F, G, C = WallClock> {
    driver: Option<D>,
    factory: F,
    fallback: G,
    clock: C,
    paused: Arc<AtomicBool>,
    cancelled: Cancellation,
    latest: Option<Arc<RgbaImage>>,
    warming: bool,
    software: bool,
    attempts: usize,
    window: Instant,
    cursor_embedded: Option<bool>,
}
impl<
    D: Driver,
    F: FnMut(Duration) -> Result<D>,
    G: FnMut(Duration) -> Result<Arc<RgbaImage>>,
    C: Clock,
> Source<D, F, G, C>
{
    pub fn new(factory: F, fallback: G, paused: Arc<AtomicBool>, clock: C) -> Self {
        let now = clock.now();
        Self {
            driver: None,
            factory,
            fallback,
            clock,
            paused,
            cancelled: Arc::new(AtomicBool::new(false)).into(),
            latest: None,
            warming: true,
            software: false,
            attempts: 0,
            window: now,
            cursor_embedded: None,
        }
    }
    pub fn controlled(mut self, cancelled: impl Into<Cancellation>) -> Self {
        self.cancelled = cancelled.into();
        self
    }
    pub fn cursor_embedded(&self) -> Option<bool> {
        self.cursor_embedded
    }
    fn check_active(&mut self) -> Result<()> {
        if self.cancelled.is_cancelled() {
            self.paused.store(true, Ordering::Release);
            self.driver.take();
            return Err("视频采集请求已结束".into());
        }
        Ok(())
    }
    fn fallback(&mut self) -> Result<Arc<RgbaImage>> {
        self.check_active()?;
        eprintln!(
            "remote capture: desktop duplication unavailable or recovery exhausted; using GDI"
        );
        self.paused.store(true, Ordering::Release);
        self.driver.take();
        self.software = true;
        self.latest = None;
        let result = (self.fallback)(Duration::from_secs(2));
        self.check_active()?;
        if result.is_ok() {
            self.cursor_embedded = Some(false);
        }
        result
    }
    pub fn frame(&mut self) -> Result<Arc<RgbaImage>> {
        self.check_active()?;
        if self.software {
            let result = (self.fallback)(Duration::from_secs(2));
            self.check_active()?;
            if result.is_err() {
                self.paused.store(true, Ordering::Release);
            } else {
                self.cursor_embedded = Some(false);
            }
            return result;
        }
        let until = self.clock.now() + Duration::from_secs(2);
        loop {
            self.check_active()?;
            let remaining = until.saturating_duration_since(self.clock.now());
            if remaining.is_zero() {
                return self.fallback();
            }
            if self.driver.is_none() {
                self.paused.store(true, Ordering::Release);
                self.driver = match (self.factory)(remaining) {
                    Ok(driver) => Some(driver),
                    Err(error) => {
                        eprintln!("remote capture: {error}");
                        return self.fallback();
                    }
                };
                self.warming = true;
            }
            self.check_active()?;
            let remaining = until.saturating_duration_since(self.clock.now());
            if remaining.is_zero() {
                return self.fallback();
            }
            let result = self
                .driver
                .as_mut()
                .expect("created driver")
                .poll(remaining);
            self.check_active()?;
            if result.is_ok() {
                self.cursor_embedded = self.driver.as_ref().and_then(Driver::cursor_embedded);
            }
            match result {
                Ok(Some(image)) => {
                    self.latest = Some(image.clone());
                    self.warming = false;
                    // Publication resumes only AFTER capture delivers this
                    // input to Shared. Clearing here would race its old ticker.
                    return Ok(image);
                }
                Ok(None) if !self.warming => {
                    return self
                        .latest
                        .clone()
                        .ok_or_else(|| "视频来源缺少已就绪画面".into());
                }
                Ok(None) => self.clock.wait(
                    Duration::from_millis(10)
                        .min(until.saturating_duration_since(self.clock.now())),
                ),
                Err(Fault::Denied) => {
                    self.paused.store(true, Ordering::Release);
                    self.driver.take();
                    return Err("无法访问当前桌面，请检查锁屏、登录状态和屏幕权限".into());
                }
                Err(Fault::Unavailable) => return self.fallback(),
                Err(Fault::Lost) => {
                    self.paused.store(true, Ordering::Release);
                    self.driver.take();
                    let now = self.clock.now();
                    if now.saturating_duration_since(self.window) >= Duration::from_secs(10) {
                        self.attempts = 0;
                        self.window = now;
                    }
                    if self.attempts >= 3 {
                        return self.fallback();
                    }
                    self.attempts += 1;
                    self.clock.wait(
                        Duration::from_millis(50)
                            .min(until.saturating_duration_since(self.clock.now())),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        rc::Rc,
    };
    #[derive(Clone)]
    struct Time(Rc<Cell<Instant>>);
    impl Clock for Time {
        fn now(&self) -> Instant {
            self.0.get()
        }
        fn wait(&self, duration: Duration) {
            self.0.set(self.0.get() + duration);
        }
    }
    struct Native {
        frames: VecDeque<std::result::Result<Option<Arc<RgbaImage>>, Fault>>,
    }
    impl Driver for Native {
        fn poll(&mut self, _: Duration) -> std::result::Result<Option<Arc<RgbaImage>>, Fault> {
            self.frames.pop_front().unwrap_or(Ok(None))
        }
    }
    fn picture(gray: u8) -> Arc<RgbaImage> {
        Arc::new(RgbaImage::from_pixel(
            16,
            8,
            image::Rgba([gray, gray, gray, 255]),
        ))
    }
    #[test]
    fn absent_mouse_updates_do_not_replace_the_last_valid_pointer_determination() {
        let mut cursor = PointerMetadata::default();
        cursor.observe(false, false);
        assert_eq!(cursor.embedded(), None);
        cursor.observe(true, true);
        assert_eq!(cursor.embedded(), Some(false));
        cursor.observe(false, false);
        assert_eq!(cursor.embedded(), Some(false));
        cursor.observe(true, false);
        assert_eq!(cursor.embedded(), Some(true));
        cursor.observe(false, true);
        assert_eq!(cursor.embedded(), Some(true));
    }
    #[test]
    fn pointer_only_update_reuses_pixels_and_recovery_cannot_reuse_old_metadata() {
        type Picture = std::result::Result<Option<Arc<RgbaImage>>, Fault>;
        struct PointerDriver {
            steps: VecDeque<(Picture, Option<bool>)>,
            mode: Option<bool>,
        }
        impl Driver for PointerDriver {
            fn poll(&mut self, _: Duration) -> std::result::Result<Option<Arc<RgbaImage>>, Fault> {
                let (result, mode) = self.steps.pop_front().unwrap();
                self.mode = mode;
                result
            }
            fn cursor_embedded(&self) -> Option<bool> {
                self.mode
            }
        }
        let mut generation = 0;
        let mut source = Source::new(
            move |_| {
                generation += 1;
                let steps = if generation == 1 {
                    vec![
                        (Ok(Some(picture(7))), Some(true)),
                        (Ok(None), Some(false)),
                        (Err(Fault::Lost), Some(false)),
                    ]
                } else {
                    vec![
                        (Ok(Some(picture(9))), None),
                        (Err(Fault::Unavailable), None),
                    ]
                };
                Ok(PointerDriver {
                    steps: steps.into(),
                    mode: None,
                })
            },
            |_| Ok(picture(11)),
            Arc::new(AtomicBool::new(false)),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        let first = source.frame().unwrap();
        assert_eq!(source.cursor_embedded(), Some(true));
        assert!(Arc::ptr_eq(&first, &source.frame().unwrap()));
        assert_eq!(source.cursor_embedded(), Some(false));
        assert_eq!(source.frame().unwrap().get_pixel(0, 0)[0], 9);
        assert_eq!(source.cursor_embedded(), None);
        assert_eq!(source.frame().unwrap().get_pixel(0, 0)[0], 11);
        assert_eq!(source.cursor_embedded(), Some(false));
    }
    #[test]
    fn padded_bgra_surfaces_keep_channels_and_all_four_orientations() {
        let bytes: Vec<u8> = (0..3)
            .flat_map(|y| [0, 100, y * 2, 0, 0, 100, y * 2 + 1, 0, 99, 99, 99, 99])
            .collect();
        for (rotation, values) in [
            (0, vec![0, 1, 2, 3, 4, 5]),
            (1, vec![4, 2, 0, 5, 3, 1]),
            (2, vec![5, 4, 3, 2, 1, 0]),
            (3, vec![1, 3, 5, 0, 2, 4]),
        ] {
            let image = bgra(&bytes, 2, 3, 12, rotation).unwrap();
            assert_eq!(
                image.dimensions(),
                if rotation % 2 == 1 { (3, 2) } else { (2, 3) }
            );
            assert_eq!(image.pixels().map(|p| p[0]).collect::<Vec<_>>(), values);
            assert!(
                image
                    .pixels()
                    .all(|p| p[1] == 100 && p[2] == 0 && p[3] == 255)
            );
        }
        assert!(bgra(&bytes[..20], 2, 3, 12, 0).is_err());
        assert!(bgra(&bytes, 2, 3, 4, 0).is_err());
        assert!(bgra(&bytes, 2, 3, 12, 4).is_err());
    }
    #[test]
    fn a_static_ready_desktop_does_not_rebuild_or_fall_back_and_recovery_requires_current_pixels() {
        let time = Time(Rc::new(Cell::new(Instant::now())));
        let paused = Arc::new(AtomicBool::new(false));
        let attempts = Rc::new(Cell::new(0));
        let count = attempts.clone();
        let mut source = Source::new(
            move |_| {
                count.set(count.get() + 1);
                Ok(Native {
                    frames: if count.get() == 1 {
                        [Ok(Some(picture(30))), Ok(None), Err(Fault::Lost)].into()
                    } else {
                        [Ok(None), Ok(Some(picture(235)))].into()
                    },
                })
            },
            |_| panic!("healthy source fell back"),
            paused.clone(),
            time.clone(),
        );
        assert_eq!(source.frame().unwrap().as_raw()[0], 30);
        paused.store(false, Ordering::Release); // capture delivered the input
        time.wait(Duration::from_secs(30));
        assert_eq!(source.frame().unwrap().as_raw()[0], 30);
        assert_eq!(attempts.get(), 1);
        assert!(!paused.load(Ordering::Acquire));
        assert_eq!(source.frame().unwrap().as_raw()[0], 235);
        assert_eq!(attempts.get(), 2);
        assert!(
            paused.load(Ordering::Acquire),
            "resumed before current input reached Shared"
        );
    }
    #[test]
    fn repeated_losses_use_a_finite_window_then_stay_on_software() {
        let attempts = Rc::new(Cell::new(0));
        let count = attempts.clone();
        let mut source = Source::new(
            move |_| {
                count.set(count.get() + 1);
                Ok(Native {
                    frames: [Err(Fault::Lost)].into(),
                })
            },
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        assert_eq!(source.frame().unwrap().as_raw()[0], 200);
        assert_eq!(attempts.get(), 4); // initial + three replacements
        assert_eq!(source.frame().unwrap().as_raw()[0], 200);
        assert_eq!(attempts.get(), 4);
    }
    #[test]
    fn separated_device_losses_get_a_new_window_instead_of_a_lifetime_retry_limit() {
        let time = Time(Rc::new(Cell::new(Instant::now())));
        let made = Rc::new(Cell::new(0));
        let count = made.clone();
        let mut source = Source::new(
            move |_| {
                count.set(count.get() + 1);
                Ok(Native {
                    frames: [Ok(Some(picture(100))), Err(Fault::Lost)].into(),
                })
            },
            |_| panic!("stable intervals still consumed lifetime retries"),
            Arc::new(AtomicBool::new(false)),
            time.clone(),
        );
        assert!(source.frame().is_ok());
        for _ in 0..8 {
            time.wait(Duration::from_secs(11));
            assert!(source.frame().is_ok());
        }
        assert_eq!(made.get(), 9);
    }
    #[test]
    fn warmup_and_all_replacements_share_the_original_frame_deadline() {
        let time = Time(Rc::new(Cell::new(Instant::now())));
        let start = time.now();
        let slow = time.clone();
        let budgets = Rc::new(RefCell::new(Vec::new()));
        let observed = budgets.clone();
        let mut source = Source::new(
            move |budget| {
                observed.borrow_mut().push(budget);
                slow.wait(Duration::from_millis(600));
                Ok(Native {
                    frames: [Err(Fault::Lost)].into(),
                })
            },
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            time.clone(),
        );
        assert!(source.frame().is_ok());
        assert!(time.now().duration_since(start) < Duration::from_secs(3));
        let budgets = budgets.borrow();
        assert!(budgets.windows(2).all(|pair| pair[1] < pair[0]));
    }
    #[test]
    fn no_first_frame_times_out_but_permission_loss_never_uses_a_fallback() {
        let time = Time(Rc::new(Cell::new(Instant::now())));
        let start = time.now();
        let mut source = Source::new(
            |_| {
                Ok(Native {
                    frames: VecDeque::new(),
                })
            },
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            time.clone(),
        );
        assert!(source.frame().is_ok());
        assert_eq!(time.now().duration_since(start), Duration::from_secs(2));
        let mut denied = Source::new(
            |_| {
                Ok(Native {
                    frames: [Err(Fault::Denied)].into(),
                })
            },
            |_| panic!("permission failure fell back"),
            Arc::new(AtomicBool::new(false)),
            time,
        );
        assert!(denied.frame().is_err());
    }
    #[test]
    fn unavailable_native_and_initialization_errors_select_software_once() {
        let mut source = Source::new(
            |_| {
                Ok(Native {
                    frames: [Err(Fault::Unavailable)].into(),
                })
            },
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        assert!(source.frame().is_ok());
        assert!(source.frame().is_ok());
        let mut failed = Source::<Native, _, _, _>::new(
            |_| Err("init timeout".into()),
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        assert!(failed.frame().is_ok());
        let mut retired = Source::new(
            |_| {
                Ok(Native {
                    frames: [Ok(Some(picture(30))), Err(Fault::Unavailable)].into(),
                })
            },
            |_| Ok(picture(200)),
            Arc::new(AtomicBool::new(false)),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        let original = retired.frame().unwrap();
        let old_pixels = Arc::downgrade(&original);
        drop(original);
        assert_eq!(retired.frame().unwrap().as_raw()[0], 200);
        assert!(
            old_pixels.upgrade().is_none(),
            "fallback retained an obsolete full-resolution picture"
        );
    }
    #[test]
    fn fallback_requests_keep_their_budget_and_failure_fences_the_previously_ready_picture() {
        let paused = Arc::new(AtomicBool::new(false));
        let mut calls = 0;
        let mut source = Source::<Native, _, _, _>::new(
            |_| Err("native unavailable".into()),
            |budget| {
                assert_eq!(budget, Duration::from_secs(2));
                calls += 1;
                if calls == 1 {
                    Ok(picture(100))
                } else {
                    Err("fallback timeout".into())
                }
            },
            paused.clone(),
            Time(Rc::new(Cell::new(Instant::now()))),
        );
        assert!(source.frame().is_ok());
        paused.store(false, Ordering::Release);
        assert_eq!(source.frame().unwrap_err(), "fallback timeout");
        assert!(paused.load(Ordering::Acquire));
    }
    #[test]
    fn cancelled_owners_cannot_start_recovery_or_accept_late_native_or_fallback_pixels() {
        let clock = || Time(Rc::new(Cell::new(Instant::now())));
        let cancelled = Arc::new(AtomicBool::new(true));
        let mut source = Source::<Native, _, _, _>::new(
            |_| panic!("cancelled native start"),
            |_| panic!("cancelled fallback"),
            Arc::new(AtomicBool::new(false)),
            clock(),
        )
        .controlled(cancelled);
        assert!(source.frame().is_err());
        let parent = Arc::new(AtomicBool::new(false));
        let retired = Arc::new(AtomicBool::new(true));
        let mut local = Source::<Native, _, _, _>::new(
            |_| panic!("retired stream restarted capture"),
            |_| panic!("retired stream started fallback"),
            Arc::new(AtomicBool::new(false)),
            clock(),
        )
        .controlled(Cancellation::new(parent.clone(), retired));
        assert!(local.frame().is_err());
        assert!(
            !parent.load(Ordering::Acquire),
            "local rebuild cancelled the parent lease"
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        let closing = cancelled.clone();
        let mut late = Source::new(
            move |_| {
                closing.store(true, Ordering::Release);
                Ok(Native {
                    frames: [Ok(Some(picture(200)))].into(),
                })
            },
            |_| panic!("cancelled native fell back"),
            Arc::new(AtomicBool::new(false)),
            clock(),
        )
        .controlled(cancelled);
        assert!(late.frame().is_err());
        assert!(late.latest.is_none());
        let cancelled = Arc::new(AtomicBool::new(false));
        let closing = cancelled.clone();
        let mut fallback = Source::<Native, _, _, _>::new(
            |_| Err("native unavailable".into()),
            move |_| {
                closing.store(true, Ordering::Release);
                Ok(picture(200))
            },
            Arc::new(AtomicBool::new(false)),
            clock(),
        )
        .controlled(cancelled);
        assert!(fallback.frame().is_err());
    }
    #[test]
    fn a_hung_native_actor_keeps_its_slot_while_a_bounded_fallback_serves_current_pixels() {
        use super::super::worker::{self, Worker};
        use std::sync::mpsc;
        let _serial = worker::isolated_capture();
        struct Actor(Worker<(), Arc<RgbaImage>>);
        impl Driver for Actor {
            fn poll(
                &mut self,
                budget: Duration,
            ) -> std::result::Result<Option<Arc<RgbaImage>>, Fault> {
                self.0
                    .run_with_timeout((), budget)
                    .map(Some)
                    .map_err(|_| Fault::Unavailable)
            }
        }
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sent) = self.0.take() {
                    let _ = sent.send(());
                }
            }
        }
        let (release, blocked) = mpsc::channel();
        let mut release = Release(Some(release));
        let mut blocked = Some(blocked);
        let mut gdi: Option<Worker<(), Arc<RgbaImage>>> = None;
        let mut source = Source::new(
            move |budget| {
                let blocked = blocked.take().expect("native retried after a call timeout");
                Worker::start_capture(
                    move || Ok(blocked),
                    |blocked, ()| {
                        blocked.recv().map_err(|_| "test native release closed")?;
                        Ok(picture(30))
                    },
                    budget,
                    Duration::from_millis(200),
                )
                .map(Actor)
            },
            move |budget| {
                if gdi.is_none() {
                    gdi = Some(Worker::start_capture(
                        || Ok(()),
                        |_, ()| Ok(picture(235)),
                        budget,
                        budget,
                    )?);
                }
                gdi.as_mut().unwrap().run_with_timeout((), budget)
            },
            Arc::new(AtomicBool::new(false)),
            WallClock,
        );
        assert_eq!(source.frame().unwrap().as_raw()[0], 235);
        assert_eq!(
            worker::capture_count(),
            2,
            "hung native actor returned its slot early"
        );
        assert!(
            Worker::start_capture(
                || panic!("third native capture started"),
                |_: &mut (), ()| Ok(()),
                Duration::from_millis(20),
                Duration::from_secs(1)
            )
            .is_err()
        );
        let _ = release.0.take().unwrap().send(());
        let until = Instant::now() + Duration::from_secs(1);
        while worker::capture_count() == 2 && Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(worker::capture_count(), 1);
        assert_eq!(
            source.frame().unwrap().as_raw()[0],
            235,
            "late native picture replaced the fallback"
        );
        drop(source);
        let until = Instant::now() + Duration::from_secs(1);
        while worker::capture_count() != 0 && Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(worker::capture_count(), 0);
    }
}
