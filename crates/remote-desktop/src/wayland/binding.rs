//! Bind the authorized source identity before publishing, including old managers
//! which route by node.target instead of target.object.
use crate::Result;
use pipewire as pw;
use pw::spa::utils::result::AsyncSeq;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct DoneCallbacks {
    done: Box<dyn Fn(u32, AsyncSeq)>,
    failed: Rc<dyn Fn(String)>,
}
struct CoreDone {
    _core: pw::core::CoreRc,
    hook: Box<pw::spa::sys::spa_hook>,
    _events: Box<pw::sys::pw_core_events>,
    _callbacks: Box<DoneCallbacks>,
    registered: bool,
}
impl CoreDone {
    fn new(
        core: &pw::core::CoreRc,
        done: impl Fn(u32, AsyncSeq) + 'static,
        failed: Rc<dyn Fn(String)>,
    ) -> Result<Self> {
        unsafe extern "C" fn callback(data: *mut std::ffi::c_void, id: u32, sequence: i32) {
            // SAFETY: the hook retains this boxed callback object through dispatch;
            // Drop removes the hook before freeing its data on this same loop.
            let callbacks = unsafe { &*data.cast::<DoneCallbacks>() };
            // Older servers omit SPA_ASYNC_BIT in done events. Normalizing the
            // sequence avoids the high-level binding's from_raw debug assertion.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (callbacks.done)(id, AsyncSeq::from_seq(sequence));
            }));
            if result.is_err() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (callbacks.failed)("Wayland 来源身份回调失败，请重新连接".into());
                }));
            }
        }
        // SAFETY: zero initializes optional callbacks and an unregistered hook;
        // both stay at stable boxed addresses for the full registration lifetime.
        let mut owner = unsafe {
            Self {
                _core: core.clone(),
                hook: Box::new(std::mem::zeroed()),
                _events: Box::new(std::mem::zeroed()),
                _callbacks: Box::new(DoneCallbacks {
                    done: Box::new(done),
                    failed,
                }),
                registered: false,
            }
        };
        owner._events.version = pw::sys::PW_VERSION_CORE_EVENTS;
        owner._events.done = Some(callback);
        // SAFETY: live core and correctly typed, stable hook/events/data pointers;
        // registration and removal occur on the owning PipeWire loop thread.
        let result = unsafe {
            pw::spa::spa_interface_call_method!(
                core.as_raw_ptr(),
                pw::sys::pw_core_methods,
                add_listener,
                std::ptr::from_mut(owner.hook.as_mut()).cast(),
                owner._events.as_ref(),
                std::ptr::from_mut(owner._callbacks.as_mut()).cast()
            )
        };
        if result < 0 {
            return Err("无法监听 Wayland 身份同步".into());
        }
        owner.registered = true;
        Ok(owner)
    }
}
impl Drop for CoreDone {
    fn drop(&mut self) {
        if self.registered {
            remove_hook(self.hook.as_mut());
        }
    }
}
fn remove_hook(hook: &mut pw::spa::sys::spa_hook) {
    // Keep the original registration address. The SDK convenience helper takes
    // a copy, which is unsuitable for a native removed callback using its hook.
    pw::spa::utils::list::remove(&hook.link);
    if let Some(removed) = hook.removed {
        // SAFETY: the live hook is unlinked, its registration storage is still
        // valid, and this is the matching callback set by the native owner.
        unsafe {
            removed(hook);
        }
    }
}

#[derive(Default)]
struct Identity {
    expected: Option<u64>,
    current: Option<u64>,
    frozen: Option<u64>,
    initial: bool,
    linked: bool,
    ready: bool,
    invalid: bool,
}
impl Identity {
    fn observe(&mut self, serial: u64) -> Result<()> {
        if self.invalid
            || self.expected.is_some_and(|value| value != serial)
            || self.frozen.is_some_and(|value| value != serial)
        {
            self.invalidate();
            return Err("Wayland 授权来源身份已变化，请重新连接".into());
        }
        self.current = Some(serial);
        Ok(())
    }
    fn initialize(&mut self) -> Result<()> {
        let serial = self
            .current
            .ok_or("已授权的 Wayland 来源不可见，请重新连接")?;
        self.observe(serial)?;
        self.frozen = Some(serial);
        self.initial = true;
        Ok(())
    }
    fn confirm(&mut self) -> Result<()> {
        if self.invalid || !self.initial || self.current != self.frozen || self.current.is_none() {
            self.invalidate();
            return Err("Wayland 屏幕连接身份无法确认，请重新连接".into());
        }
        self.ready = self.linked;
        Ok(())
    }
    fn invalidate(&mut self) {
        self.invalid = true;
        self.ready = false;
    }
}

pub(super) struct Guard {
    _core_listener: CoreDone,
    _registry_listener: pw::registry::Listener,
    _registry: pw::registry::RegistryRc,
    pub state: Rc<State>,
}
pub(super) struct State {
    identity: RefCell<Identity>,
    core: pw::core::CoreRc,
    pending: Cell<Option<AsyncSeq>>,
    sequence: Cell<i32>,
    fail: Rc<dyn Fn(String)>,
}
impl State {
    fn error(&self, error: String) {
        self.identity.borrow_mut().invalidate();
        (self.fail)(error);
    }
    fn synchronize(&self) {
        let identity = self.identity.borrow();
        let needed = identity.initial && identity.linked && !identity.ready && !identity.invalid;
        drop(identity);
        if !needed || self.pending.get().is_some() {
            return;
        }
        let sequence = self.sequence.get();
        if sequence >= 0x3fff_fffe {
            self.error("Wayland 身份同步次数超限".into());
            return;
        }
        self.sequence.set(sequence + 1);
        match self.core.sync(sequence) {
            Ok(pending) => self.pending.set(Some(pending)),
            Err(_) => self.error("Wayland 身份同步失败".into()),
        }
    }
    pub fn linked(&self, linked: bool) {
        let mut identity = self.identity.borrow_mut();
        identity.linked = linked;
        identity.ready = false;
        drop(identity);
        if linked {
            self.synchronize();
        }
    }
    pub fn ready(&self) -> bool {
        self.identity.borrow().ready
    }
}
impl Guard {
    pub fn new(
        core: &pw::core::CoreRc,
        node: u32,
        expected: Option<u64>,
        fail: Rc<dyn Fn(String)>,
    ) -> Result<Self> {
        let registry = core
            .get_registry_rc()
            .map_err(|_| "无法核对 Wayland 授权来源")?;
        let state = Rc::new(State {
            identity: RefCell::new(Identity {
                expected,
                ..Identity::default()
            }),
            core: core.clone(),
            pending: Cell::new(None),
            sequence: Cell::new(1),
            fail,
        });
        let seen = state.clone();
        let removed = state.clone();
        let registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                if global.id != node || global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let serial = global
                    .props
                    .and_then(|props| props.get("object.serial"))
                    .and_then(|value| value.parse().ok());
                let result = match serial {
                    Some(serial) => seen.identity.borrow_mut().observe(serial),
                    None => Err("Wayland 来源缺少稳定身份".into()),
                };
                if let Err(error) = result {
                    seen.error(error);
                }
            })
            .global_remove(move |id| {
                if id == node {
                    removed.error("Wayland 授权来源已移除，请重新连接".into());
                }
            })
            .register();
        let initial = core.sync(0).map_err(|_| "无法核对 Wayland 来源列表")?;
        let done = state.clone();
        let core_listener = CoreDone::new(
            core,
            move |id, sequence| {
                if id != pw::core::PW_ID_CORE {
                    return;
                }
                let result = if sequence == initial {
                    done.identity.borrow_mut().initialize()
                } else if done.pending.get() == Some(sequence) {
                    done.pending.set(None);
                    done.identity.borrow_mut().confirm()
                } else {
                    return;
                };
                if let Err(error) = result {
                    done.error(error);
                } else {
                    done.synchronize();
                }
            },
            state.fail.clone(),
        )?;
        Ok(Self {
            _core_listener: core_listener,
            _registry_listener: registry_listener,
            _registry: registry,
            state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_removed_callback_receives_the_original_registered_hook_address() {
        thread_local! {
            static EXPECTED:Cell<usize>=const { Cell::new(0) };
            static MATCHED:Cell<bool>=const { Cell::new(false) };
        }
        unsafe extern "C" fn removed(hook: *mut pw::spa::sys::spa_hook) {
            MATCHED.with(|matched| {
                EXPECTED.with(|expected| matched.set(expected.get() == hook as usize))
            });
        }
        // SAFETY: optional function pointers and raw pointers admit zero; this
        // synthetic hook is initialized as a one-item ring before removal.
        let mut hook: Box<pw::spa::sys::spa_hook> = Box::new(unsafe { std::mem::zeroed() });
        let link = std::ptr::from_mut(&mut hook.link);
        hook.link.next = link;
        hook.link.prev = link;
        hook.removed = Some(removed);
        EXPECTED.with(|expected| expected.set(std::ptr::from_ref(hook.as_ref()) as usize));
        remove_hook(hook.as_mut());
        MATCHED.with(|matched| assert!(matched.get()));
    }
    #[test]
    fn frames_need_the_original_serial_and_a_post_link_roundtrip() {
        let mut state = Identity {
            expected: Some(100),
            ..Identity::default()
        };
        state.observe(100).unwrap();
        state.initialize().unwrap();
        assert!(!state.ready);
        state.linked = true;
        assert!(!state.ready);
        state.confirm().unwrap();
        assert!(state.ready);
        assert!(state.observe(101).is_err());
        assert!(!state.ready);
        assert!(state.observe(100).is_err());
    }
    #[test]
    fn removal_and_id_reuse_cannot_reactivate_a_stream() {
        let mut state = Identity::default();
        state.observe(100).unwrap();
        state.initialize().unwrap();
        state.linked = true;
        state.confirm().unwrap();
        state.invalidate();
        assert!(state.observe(101).is_err());
        assert!(state.confirm().is_err());
        assert!(!state.ready);
    }
}
