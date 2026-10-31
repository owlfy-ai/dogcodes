//! Session event tap; process-generated keys never trigger the human stop.
use std::{
    ffi::c_void,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user: *mut c_void,
    ) -> *mut c_void;
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventTapEnable(tap: *mut c_void, enabled: bool);
    fn CGEventTapIsEnabled(tap: *mut c_void) -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFMachPortInvalidate(port: *mut c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(runloop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(runloop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, once: bool) -> i32;
    fn CFRelease(value: *const c_void);
}

struct State {
    armed: Arc<AtomicBool>,
    held: bool,
    pressed: Box<dyn Fn()>,
}
pub struct EscapeMonitor {
    armed: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl EscapeMonitor {
    pub fn new(pressed: impl Fn() + Send + 'static) -> io::Result<Self> {
        let armed = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let shared = armed.clone();
        let stop = stopping.clone();
        let (ready, received) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("computer-use-escape".into())
            .spawn(move || {
                let mut state = Box::new(State {
                    armed: shared,
                    held: false,
                    pressed: Box::new(pressed),
                });
                // SAFETY: the boxed callback state stays live until the tap is invalidated;
                // the session tap receives only keyboard down/up on this worker's runloop.
                let tap = unsafe {
                    CGEventTapCreate(
                        1,
                        0,
                        0,
                        (1 << 10) | (1 << 11),
                        keyboard,
                        (&raw mut *state).cast(),
                    )
                };
                if tap.is_null() {
                    let _ = ready.send(false);
                    return;
                }
                // SAFETY: tap is a retained live CFMachPort. The source and runloop
                // are created, used, detached and released on this same worker.
                unsafe {
                    let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                    if source.is_null() {
                        CFMachPortInvalidate(tap);
                        CFRelease(tap);
                        let _ = ready.send(false);
                        return;
                    }
                    let runloop = CFRunLoopGetCurrent();
                    CFRunLoopAddSource(runloop, source, kCFRunLoopDefaultMode);
                    let _ = ready.send(true);
                    while !stop.load(Ordering::Acquire) {
                        if !CGEventTapIsEnabled(tap) {
                            CGEventTapEnable(tap, true);
                        }
                        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false);
                    }
                    CFMachPortInvalidate(tap);
                    CFRunLoopRemoveSource(runloop, source, kCFRunLoopDefaultMode);
                    CFRelease(source);
                    CFRelease(tap);
                }
            })?;
        if received.recv().unwrap_or(false) {
            Ok(Self {
                armed,
                stopping,
                worker: Some(worker),
            })
        } else {
            let _ = worker.join();
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Grant dog.codes Accessibility and Input Monitoring, then retry computer use",
            ))
        }
    }
    pub fn arm(&self, active: bool) {
        self.armed.store(active, Ordering::Release);
    }
}
impl Drop for EscapeMonitor {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
unsafe extern "C" fn keyboard(
    _: *mut c_void,
    kind: u32,
    event: *mut c_void,
    user: *mut c_void,
) -> *mut c_void {
    if event.is_null() || user.is_null() || !matches!(kind, 10 | 11) {
        return event;
    }
    // SAFETY: CoreGraphics owns the event for this callback; user points to the
    // live State exclusively dispatched by its owning worker runloop.
    let (key, source, state) = unsafe {
        (
            CGEventGetIntegerValueField(event, 9),
            CGEventGetIntegerValueField(event, 41),
            &mut *user.cast::<State>(),
        )
    };
    if key != 53 || source != 0 {
        return event;
    }
    if kind == 11 && state.held {
        state.held = false;
        return std::ptr::null_mut();
    }
    if kind == 10 && (state.held || state.armed.swap(false, Ordering::AcqRel)) {
        if !state.held {
            state.held = true;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (state.pressed)()));
        }
        return std::ptr::null_mut();
    }
    event
}
