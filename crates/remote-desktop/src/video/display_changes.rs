//! Native display notifications carry no pixels and perform no native work.
//! A process-owned registry keeps callback context valid even if unregister fails.
use super::Result;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct Changes {
    pending: Arc<AtomicBool>,
    changed: Arc<AtomicBool>,
    began: Mutex<Option<Instant>>,
}
impl Changes {
    pub fn suspended(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
    pub fn new(pending: Arc<AtomicBool>, changed: Arc<AtomicBool>) -> Self {
        Self {
            pending,
            changed,
            began: Mutex::new(None),
        }
    }
    fn observe(&self, begin: bool, now: Instant) {
        // Never reopen an old generation after commit. Only a new capture
        // registration gets fresh flags and authoritative display geometry.
        self.pending.store(true, Ordering::Release);
        if begin {
            self.began
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert(now);
        } else {
            self.changed.store(true, Ordering::Release);
        }
    }
    pub fn error(&self, now: Instant) -> Option<String> {
        if self.changed.load(Ordering::Acquire) {
            return None;
        }
        self.began
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .filter(|at| now.saturating_duration_since(*at) >= Duration::from_secs(10))
            .map(|_| "显示器配置更新超时，请稳定显示设置后重新连接".into())
    }
}

/// Callback delivery may depend on an application event loop. A bounded native
/// worker also checks authoritative geometry, without repeatedly reconfiguring
/// ScreenCaptureKit when only this liveness check was due.
pub(super) struct Probe<S> {
    initial: S,
    checked: Instant,
}
impl<S: PartialEq> Probe<S> {
    pub fn new(initial: S, now: Instant) -> Self {
        Self {
            initial,
            checked: now,
        }
    }
    pub fn check(
        &mut self,
        now: Instant,
        read: impl FnOnce() -> Result<S>,
        changes: &Changes,
    ) -> Result<bool> {
        // A delivered begin is authoritative: properties are not guaranteed
        // final until commit. Never mistake an intermediate mode for a commit.
        if changes.suspended() && !changes.changed.load(Ordering::Acquire) {
            return Ok(false);
        }
        if now.saturating_duration_since(self.checked) < Duration::from_secs(1) {
            return Ok(false);
        }
        self.checked = now;
        let changed = read()? != self.initial;
        if changed {
            changes.observe(false, now);
        }
        Ok(changed)
    }
}

trait Hooks: Send + Sync {
    fn register(&self) -> Result<()>;
    fn unregister(&self) -> Result<()>;
}
struct Watch {
    display: u32,
    changes: Weak<Changes>,
}
struct Registry {
    hooks: Arc<dyn Hooks>,
    registered: Mutex<bool>,
    watchers: Mutex<HashMap<u64, Watch>>,
    next: AtomicU64,
}
impl Registry {
    fn new(hooks: Arc<dyn Hooks>) -> Arc<Self> {
        Arc::new(Self {
            hooks,
            registered: Mutex::new(false),
            watchers: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        })
    }
    fn subscribe(self: &Arc<Self>, display: u32, changes: Arc<Changes>) -> Result<Subscription> {
        let id = self
            .next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| "显示器观察身份已耗尽")?;
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id,
                Watch {
                    display,
                    changes: Arc::downgrade(&changes),
                },
            );
        // The OS may synchronously deliver an event here. No watchers lock is
        // held across register/remove, and callbacks never take registered.
        if !*registered {
            if let Err(error) = self.hooks.register() {
                self.watchers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                return Err(error);
            }
            *registered = true;
        }
        Ok(Subscription {
            registry: self.clone(),
            id,
            _changes: changes,
        })
    }
    fn observe(&self, display: u32, begin: bool, now: Instant) {
        for watch in self
            .watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            if watch.display == display
                && let Some(changes) = watch.changes.upgrade()
            {
                changes.observe(begin, now);
            }
        }
    }
    fn remove(&self, id: u64) {
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        let empty = {
            let mut watchers = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
            watchers.remove(&id);
            watchers.is_empty()
        };
        if empty && *registered && self.hooks.unregister().is_ok() {
            *registered = false;
        }
        // Failure retains ONE process-owned registration with no user pointer.
        // Empty callbacks are safe; the next capture reuses it without another
        // registration. Native capture/encoder quotas are unrelated and intact.
    }
}
pub(super) struct Subscription {
    registry: Arc<Registry>,
    id: u64,
    _changes: Arc<Changes>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2_core_graphics::{
        CGDisplayChangeSummaryFlags, CGDisplayRegisterReconfigurationCallback,
        CGDisplayRemoveReconfigurationCallback, CGError,
    };
    static REGISTRY: std::sync::OnceLock<Arc<Registry>> = std::sync::OnceLock::new();
    unsafe extern "C-unwind" fn changed(
        display: u32,
        flags: CGDisplayChangeSummaryFlags,
        _context: *mut std::ffi::c_void,
    ) {
        if let Some(registry) = REGISTRY.get() {
            registry.observe(
                display,
                flags.contains(CGDisplayChangeSummaryFlags::BeginConfigurationFlag),
                Instant::now(),
            );
        }
    }
    struct Native;
    impl Hooks for Native {
        fn register(&self) -> Result<()> {
            // SAFETY: process-lifetime callback; NULL context is never dereferenced.
            let status = unsafe {
                CGDisplayRegisterReconfigurationCallback(Some(changed), std::ptr::null_mut())
            };
            if status == CGError::Success {
                Ok(())
            } else {
                Err("无法监听显示器配置变化".into())
            }
        }
        fn unregister(&self) -> Result<()> {
            // SAFETY: exact same callback and NULL context as registration.
            let status = unsafe {
                CGDisplayRemoveReconfigurationCallback(Some(changed), std::ptr::null_mut())
            };
            if status == CGError::Success {
                Ok(())
            } else {
                Err("显示器配置监听仍在退出".into())
            }
        }
    }
    pub fn subscribe(display: u32, changes: Arc<Changes>) -> Result<Subscription> {
        REGISTRY
            .get_or_init(|| Registry::new(Arc::new(Native)))
            .subscribe(display, changes)
    }
}
#[cfg(target_os = "macos")]
pub(super) use native::subscribe;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[derive(Default)]
    struct Fake {
        adds: AtomicUsize,
        removes: AtomicUsize,
        fail_add: AtomicBool,
        fail_remove: AtomicBool,
        reentrant: Mutex<Weak<Registry>>,
    }
    impl Hooks for Fake {
        fn register(&self) -> Result<()> {
            self.adds.fetch_add(1, Ordering::AcqRel);
            if let Some(registry) = self.reentrant.lock().unwrap().upgrade() {
                registry.observe(7, true, Instant::now());
            }
            if self.fail_add.load(Ordering::Acquire) {
                Err("register failed".into())
            } else {
                Ok(())
            }
        }
        fn unregister(&self) -> Result<()> {
            self.removes.fetch_add(1, Ordering::AcqRel);
            if let Some(registry) = self.reentrant.lock().unwrap().upgrade() {
                registry.observe(7, false, Instant::now());
            }
            if self.fail_remove.load(Ordering::Acquire) {
                Err("remove failed".into())
            } else {
                Ok(())
            }
        }
    }
    fn flags() -> Arc<Changes> {
        Arc::new(Changes::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ))
    }

    #[test]
    fn begin_fences_then_commit_requests_rebuild_only_for_the_selected_display() {
        let registry = Registry::new(Arc::new(Fake::default()));
        let changes = flags();
        let _watch = registry.subscribe(7, changes.clone()).unwrap();
        let now = Instant::now();
        registry.observe(9, true, now);
        assert!(!changes.pending.load(Ordering::Acquire));
        registry.observe(7, true, now);
        assert!(changes.pending.load(Ordering::Acquire));
        assert!(!changes.changed.load(Ordering::Acquire));
        registry.observe(7, false, now);
        assert!(changes.pending.load(Ordering::Acquire));
        assert!(changes.changed.load(Ordering::Acquire));
        assert!(changes.error(now + Duration::from_secs(20)).is_none());
    }
    #[test]
    fn repeated_begin_does_not_extend_the_configuration_deadline() {
        let changes = flags();
        let now = Instant::now();
        changes.observe(true, now);
        changes.observe(true, now + Duration::from_secs(9));
        assert!(changes.error(now + Duration::from_secs(10)).is_some());
        assert!(changes.pending.load(Ordering::Acquire));
    }
    #[test]
    fn a_worker_probe_detects_missed_callbacks_without_guessing_pixels_or_reopening_old_output() {
        let changes = flags();
        let now = Instant::now();
        let mut probe = Probe::new((1440, 900, 2), now);
        assert!(
            !probe
                .check(now, || panic!("probe ran too early"), &changes)
                .unwrap()
        );
        assert!(
            !probe
                .check(
                    now + Duration::from_secs(1),
                    || Ok((1440, 900, 2)),
                    &changes
                )
                .unwrap()
        );
        assert!(
            probe
                .check(
                    now + Duration::from_secs(2),
                    || Ok((900, 1440, 2)),
                    &changes
                )
                .unwrap()
        );
        assert!(changes.pending.load(Ordering::Acquire));
        assert!(changes.changed.load(Ordering::Acquire));
        assert!(
            !probe
                .check(
                    now + Duration::from_secs(3),
                    || Ok((1440, 900, 2)),
                    &changes
                )
                .unwrap()
        );
        assert!(changes.changed.load(Ordering::Acquire));
        assert!(
            probe
                .check(
                    now + Duration::from_secs(4),
                    || Err("display disconnected".into()),
                    &changes
                )
                .is_err()
        );
        let configuring = flags();
        let mut during = Probe::new((1440, 900, 2), now);
        configuring.observe(true, now);
        assert!(
            !during
                .check(
                    now + Duration::from_secs(2),
                    || panic!("queried an uncommitted configuration"),
                    &configuring
                )
                .unwrap()
        );
        assert!(configuring.error(now + Duration::from_secs(10)).is_some());
        configuring.observe(false, now + Duration::from_secs(11));
        assert!(
            during
                .check(
                    now + Duration::from_secs(11),
                    || Ok((900, 1440, 2)),
                    &configuring
                )
                .unwrap()
        );
    }
    #[test]
    fn a_begin_notification_during_final_authorization_drops_the_late_native_frame() {
        let registry = Registry::new(Arc::new(Fake::default()));
        let changes = flags();
        let watch = registry.subscribe(7, changes.clone()).unwrap();
        let gate = changes.clone();
        let observed = registry.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let sink = super::super::publication::guard_when(
            Arc::new(|_| panic!("native output escaped during configuration")),
            stopped.clone(),
            error.clone(),
            move || !gate.pending.load(Ordering::Acquire),
            move || {
                observed.observe(7, true, Instant::now());
                Ok(())
            },
        );
        sink(super::super::EncodedFrame {
            cursor_embedded: None,
            data: vec![1],
            keyframe: true,
            captured: Instant::now(),
            latency: Duration::ZERO,
        });
        assert!(changes.pending.load(Ordering::Acquire));
        assert!(!stopped.load(Ordering::Acquire));
        assert!(error.lock().unwrap().is_none());
        registry.observe(7, false, Instant::now());
        assert!(changes.changed.load(Ordering::Acquire));
        drop(watch);
        let fresh = flags();
        let _replacement = registry.subscribe(7, fresh.clone()).unwrap();
        assert!(!fresh.pending.load(Ordering::Acquire));
        assert!(!fresh.changed.load(Ordering::Acquire));
    }
    #[test]
    fn registrations_are_shared_and_failed_removal_never_duplicates_or_revives_old_owners() {
        let hooks = Arc::new(Fake::default());
        let registry = Registry::new(hooks.clone());
        let old = flags();
        let first = registry.subscribe(7, old.clone()).unwrap();
        let second = registry.subscribe(9, flags()).unwrap();
        assert_eq!(hooks.adds.load(Ordering::Acquire), 1);
        drop(first);
        assert_eq!(hooks.removes.load(Ordering::Acquire), 0);
        hooks.fail_remove.store(true, Ordering::Release);
        drop(second);
        assert!(registry.watchers.lock().unwrap().is_empty());
        let current = flags();
        let third = registry.subscribe(7, current.clone()).unwrap();
        assert_eq!(hooks.adds.load(Ordering::Acquire), 1);
        registry.observe(7, false, Instant::now());
        assert!(!old.changed.load(Ordering::Acquire));
        assert!(current.changed.load(Ordering::Acquire));
        hooks.fail_remove.store(false, Ordering::Release);
        drop(third);
        assert!(!*registry.registered.lock().unwrap());
        let _fourth = registry.subscribe(7, flags()).unwrap();
        assert_eq!(hooks.adds.load(Ordering::Acquire), 2);
    }
    #[test]
    fn failed_registration_rolls_back_and_native_hooks_can_deliver_synchronous_events() {
        let hooks = Arc::new(Fake::default());
        let registry = Registry::new(hooks.clone());
        *hooks.reentrant.lock().unwrap() = Arc::downgrade(&registry);
        hooks.fail_add.store(true, Ordering::Release);
        assert!(registry.subscribe(7, flags()).is_err());
        assert!(registry.watchers.lock().unwrap().is_empty());
        hooks.fail_add.store(false, Ordering::Release);
        let changes = flags();
        let watch = registry.subscribe(7, changes.clone()).unwrap();
        assert!(changes.pending.load(Ordering::Acquire));
        drop(watch);
        assert!(!changes.changed.load(Ordering::Acquire));
    }
}
