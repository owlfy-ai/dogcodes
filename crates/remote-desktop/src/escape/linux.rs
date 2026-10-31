//! XInput2 raw keys retain their physical source; XTEST injection is excluded.
use std::{
    collections::HashSet,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xinput::{ConnectionExt as _, Device, DeviceType, EventMask, XIEventMask},
        xproto::ConnectionExt as _,
    },
};

pub struct EscapeMonitor {
    armed: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl EscapeMonitor {
    pub fn new(pressed: impl Fn() + Send + 'static) -> io::Result<Self> {
        // XWayland cannot observe the physical keys belonging to other native
        // Wayland clients. Never advertise an emergency key through that route.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Global physical Escape requires an X11 session; background computer use remains available",
            ));
        }
        let (connection, screen) = x11rb::connect(None).map_err(io::Error::other)?;
        connection
            .xinput_xi_query_version(2, 0)
            .map_err(io::Error::other)?
            .reply()
            .map_err(io::Error::other)?;
        let root = connection.setup().roots[screen].root;
        connection
            .xinput_xi_select_events(
                root,
                &[EventMask {
                    deviceid: Device::ALL_MASTER.into(),
                    mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE],
                }],
            )
            .map_err(io::Error::other)?
            .check()
            .map_err(io::Error::other)?;
        let setup = connection.setup();
        let mapping = connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(io::Error::other)?
            .reply()
            .map_err(io::Error::other)?;
        if mapping.keysyms_per_keycode == 0 {
            return Err(io::Error::other("X11 keyboard mapping is empty"));
        }
        let escape: HashSet<u32> = mapping
            .keysyms
            .chunks(usize::from(mapping.keysyms_per_keycode))
            .enumerate()
            .filter(|(_, keys)| keys.contains(&0xff1b))
            .map(|(index, _)| u32::from(setup.min_keycode) + u32::try_from(index).unwrap_or(0))
            .collect();
        if escape.is_empty() {
            return Err(io::Error::other(
                "Escape key is absent from the X11 keyboard mapping",
            ));
        }
        connection.flush().map_err(io::Error::other)?;
        let armed = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let shared = armed.clone();
        let stop = stopping.clone();
        let worker = thread::Builder::new()
            .name("computer-use-escape".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match connection.poll_for_event() {
                        Ok(Some(Event::XinputRawKeyPress(event)))
                            if escape.contains(&event.detail) =>
                        {
                            let physical = connection
                                .xinput_xi_query_device(event.sourceid)
                                .ok()
                                .and_then(|cookie| cookie.reply().ok())
                                .is_some_and(|reply| {
                                    reply.infos.iter().any(|device| {
                                        let name = String::from_utf8_lossy(&device.name)
                                            .to_ascii_lowercase();
                                        device.enabled
                                            && device.type_ == DeviceType::SLAVE_KEYBOARD
                                            && !["xtest", "cua", "virtual"]
                                                .iter()
                                                .any(|word| name.contains(word))
                                    })
                                });
                            if physical && shared.swap(false, Ordering::AcqRel) {
                                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                    &pressed,
                                ));
                            }
                        }
                        Ok(Some(_)) => (),
                        Ok(None) => thread::sleep(Duration::from_millis(10)),
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            armed,
            stopping,
            worker: Some(worker),
        })
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
