//! Human-only Windows emergency Escape. Synthetic agent keys never activate it.
use std::{
    cell::RefCell,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
        PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
        WM_SYSKEYUP,
    },
};

struct State {
    armed: Arc<AtomicBool>,
    held: bool,
    pressed: Box<dyn Fn()>,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

/// Owns a keyboard-hook thread. It observes only Escape and retains no key log.
pub struct EscapeMonitor {
    armed: Arc<AtomicBool>,
    thread_id: u32,
    worker: Option<thread::JoinHandle<()>>,
}
impl EscapeMonitor {
    pub fn new(pressed: impl Fn() + Send + 'static) -> io::Result<Self> {
        let armed = Arc::new(AtomicBool::new(false));
        let shared = armed.clone();
        let (ready, receive) = std::sync::mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("computer-use-escape".into())
            .spawn(move || {
                STATE.with(|state| {
                    *state.borrow_mut() = Some(State {
                        armed: shared,
                        held: false,
                        pressed: Box::new(pressed),
                    })
                });
                // SAFETY: installing a documented low-level callback in this process;
                // this thread pumps its message queue until ownership is dropped.
                let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), None, 0) };
                let Ok(hook) = hook else {
                    let _ = ready.send(Err(io::Error::last_os_error()));
                    STATE.with(|state| state.borrow_mut().take());
                    return;
                };
                let mut message = MSG::default();
                // SAFETY: valid writable MSG; create the thread queue before publishing its id.
                unsafe {
                    let _ = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
                }
                // SAFETY: retrieves this live worker's id, no borrowed native data.
                let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
                // SAFETY: MSG remains live and writable until the pump ends.
                while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0 > 0 {
                    // SAFETY: dispatch the message returned by this thread's message pump.
                    unsafe {
                        let _ = TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                }
                // SAFETY: the hook belongs to this thread and has not been unhooked elsewhere.
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
                STATE.with(|state| state.borrow_mut().take());
            })?;
        match receive.recv().map_err(io::Error::other)? {
            Ok(thread_id) => Ok(Self {
                armed,
                thread_id,
                worker: Some(worker),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
    pub fn arm(&self, enabled: bool) {
        self.armed.store(enabled, Ordering::Release);
    }
}
impl Drop for EscapeMonitor {
    fn drop(&mut self) {
        self.arm(false);
        // SAFETY: this id was published only after the owned worker created its queue.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn consume(
    armed: &AtomicBool,
    held: &mut bool,
    key: u32,
    injected: bool,
    down: bool,
    up: bool,
) -> bool {
    if key != 0x1b || injected {
        return false;
    }
    if down && (*held || armed.swap(false, Ordering::AcqRel)) {
        *held = true;
        return true;
    }
    if up && *held {
        *held = false;
        return true;
    }
    false
}

unsafe extern "system" fn keyboard(code: i32, event: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && data.0 != 0 {
        // SAFETY: WH_KEYBOARD_LL provides a KBDLLHOOKSTRUCT for the callback duration.
        let key = unsafe { &*(data.0 as *const KBDLLHOOKSTRUCT) };
        let consumed = STATE.with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return false;
            };
            let Some(state) = state.as_mut() else {
                return false;
            };
            let was_held = state.held;
            let down = event.0 == WM_KEYDOWN as usize || event.0 == WM_SYSKEYDOWN as usize;
            let up = event.0 == WM_KEYUP as usize || event.0 == WM_SYSKEYUP as usize;
            let handled = consume(
                &state.armed,
                &mut state.held,
                key.vkCode,
                key.flags.contains(LLKHF_INJECTED),
                down,
                up,
            );
            if handled && down && !was_held {
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (state.pressed)()));
            }
            handled
        });
        if consumed {
            return LRESULT(1);
        }
    }
    // SAFETY: preserve the original hook parameters for every unhandled key.
    unsafe { CallNextHookEx(None, code, event, data) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_human_escape_is_consumed_once_while_armed() {
        let armed = AtomicBool::new(true);
        let mut held = false;
        assert!(!consume(&armed, &mut held, 0x1b, true, true, false));
        assert!(armed.load(Ordering::Acquire));
        assert!(!consume(&armed, &mut held, 0x41, false, true, false));
        assert!(consume(&armed, &mut held, 0x1b, false, true, false));
        assert!(!armed.load(Ordering::Acquire));
        assert!(consume(&armed, &mut held, 0x1b, false, true, false));
        assert!(consume(&armed, &mut held, 0x1b, false, false, true));
        assert!(!consume(&armed, &mut held, 0x1b, false, true, false));
    }
    #[test]
    fn hook_can_be_installed_and_retired_without_grabbing_focus() {
        let monitor = EscapeMonitor::new(|| {}).expect("Windows keyboard hook");
        monitor.arm(true);
        drop(monitor);
    }
    #[test]
    fn callback_notifies_once_for_a_physical_escape_press() {
        use std::sync::atomic::AtomicUsize;
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        STATE.with(|state| {
            *state.borrow_mut() = Some(State {
                armed: Arc::new(AtomicBool::new(true)),
                held: false,
                pressed: Box::new(move || {
                    observed.fetch_add(1, Ordering::AcqRel);
                }),
            })
        });
        let key = KBDLLHOOKSTRUCT {
            vkCode: 0x1b,
            ..Default::default()
        };
        let data = LPARAM((&raw const key) as isize);
        // SAFETY: the fixture provides the documented hook payload, live until
        // all calls finish; handled keys do not call any downstream hook.
        unsafe {
            assert_eq!(keyboard(0, WPARAM(WM_KEYDOWN as usize), data).0, 1);
            assert_eq!(keyboard(0, WPARAM(WM_KEYDOWN as usize), data).0, 1);
            assert_eq!(keyboard(0, WPARAM(WM_KEYUP as usize), data).0, 1);
        }
        assert_eq!(count.load(Ordering::Acquire), 1);
        STATE.with(|state| state.borrow_mut().take());
    }
}
