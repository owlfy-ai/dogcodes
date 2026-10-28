//! Real EI wire protocol over an owned socket pair. No desktop, portal, socket
//! environment, input devices, or user session is accessed by these tests.
use super::*;
use reis::{
    PendingRequestResult, eis,
    handshake::EisHandshaker,
    request::{self, EisRequest},
};
use std::{
    io::Write,
    os::fd::AsFd,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub(in crate::wayland) enum Record {
    Move(f32, f32),
    Button(u32, bool),
    Key(u32, bool),
    Sym(u32, bool),
    Text(String),
    Scroll(i32, i32),
    Frame(u64),
    Start(u32),
}
pub(in crate::wayland) enum Change {
    Pause,
    Resume,
    Replace,
    Duplicate,
    Disconnect,
    ExternalShift,
}
pub(in crate::wayland) struct Fixture {
    pub changes: mpsc::Sender<Change>,
    pub log: Arc<Mutex<Vec<Record>>>,
    typed: Arc<Mutex<String>>,
    pub on_paste: PasteHook,
    pub paste_shift: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}
type PasteHook = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.join.take().unwrap().join().unwrap();
    }
}
pub(in crate::wayland) fn socket(text: bool) -> (OwnedFd, Fixture) {
    socket_config(text, "us", "", false)
}
fn socket_config(
    text: bool,
    layout: &'static str,
    options: &'static str,
    switch_on_enter: bool,
) -> (OwnedFd, Fixture) {
    let (client, server) = UnixStream::pair().unwrap();
    let (tx, rx) = mpsc::channel();
    let log = Arc::new(Mutex::new(Vec::new()));
    let records = log.clone();
    let typed = Arc::new(Mutex::new(String::new()));
    let on_paste: PasteHook = Arc::new(Mutex::new(None));
    let pasted = on_paste.clone();
    let paste_shift = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let expected_shift = paste_shift.clone();
    let output = typed.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = stop.clone();
    let join = std::thread::spawn(move || {
        let context = eis::Context::new(server).unwrap();
        let mut handshake = Some(EisHandshaker::new(&context, 100));
        let mut converter: Option<request::EisRequestConverter> = None;
        let mut device: Option<request::Device> = None;
        let mut pending_layout: Option<request::Device> = None;
        let mut extras = Vec::new();
        let xkb_context = xkbcommon::xkb::Context::new(0);
        let keymap = xkbcommon::xkb::Keymap::new_from_names(
            &xkb_context,
            "evdev",
            "pc105",
            layout,
            "",
            Some(options.to_owned()),
            0,
        )
        .unwrap();
        let mut keys = xkbcommon::xkb::State::new(&keymap);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done.load(Ordering::Acquire) && Instant::now() < deadline {
            if context.read().is_err() {
                break;
            }
            while let Some(request) = context.pending_request() {
                let PendingRequestResult::Request(request) = request else {
                    panic!("invalid EI request")
                };
                if let Some(handshaker) = handshake.as_mut() {
                    if let Some(response) = handshaker.handle_request(request).unwrap() {
                        assert_eq!(response.context_type, eis::handshake::ContextType::Sender);
                        let c = request::EisRequestConverter::new(&context, response, 100);
                        let _ = c.handle().add_seat(
                            Some("owned"),
                            request::DeviceCapability::PointerAbsolute
                                | request::DeviceCapability::Button
                                | request::DeviceCapability::Scroll
                                | request::DeviceCapability::Keyboard
                                | request::DeviceCapability::Text,
                        );
                        converter = Some(c);
                    }
                    if converter.is_some() {
                        handshake = None;
                    }
                } else {
                    let c = converter.as_mut().unwrap();
                    // A compositor may defer directly triggered feedback until
                    // sync. Publish it before callback.done, as EI guarantees.
                    if matches!(
                        &request,
                        eis::Request::Connection(_, eis::connection::Request::Sync { .. })
                    ) && let Some(d) = pending_layout.take()
                    {
                        keys.update_mask(0, 0, 0, 0, 0, 1);
                        c.handle().with_next_serial(|serial| {
                            d.interface::<eis::Keyboard>()
                                .unwrap()
                                .modifiers(serial, 0, 0, 0, 1)
                        });
                    }
                    c.handle_request(request).unwrap();
                    while let Some(request) = c.next_request() {
                        let record = match request {
                            EisRequest::Bind(e) => {
                                device = Some(add_device(&e.seat, text, false, &keymap));
                                None
                            }
                            EisRequest::Ready(e) => {
                                e.device.resumed();
                                None
                            }
                            EisRequest::DeviceStartEmulating(e) => Some(Record::Start(e.sequence)),
                            EisRequest::PointerMotionAbsolute(e) => {
                                Some(Record::Move(e.dx_absolute, e.dy_absolute))
                            }
                            EisRequest::Button(e) => Some(Record::Button(
                                e.button,
                                e.state == eis::button::ButtonState::Press,
                            )),
                            EisRequest::KeyboardKey(e) => {
                                let down = e.state == eis::keyboard::KeyState::Press;
                                if down
                                    && e.key == 47
                                    && (keys.serialize_mods(xkbcommon::xkb::STATE_MODS_DEPRESSED)
                                        & 1
                                        != 0)
                                        == expected_shift.load(Ordering::Acquire)
                                    && keys.serialize_mods(xkbcommon::xkb::STATE_MODS_DEPRESSED) & 4
                                        != 0
                                {
                                    let hook = pasted.lock().unwrap().clone();
                                    if let Some(hook) = hook {
                                        hook();
                                    }
                                }
                                if down {
                                    output.lock().unwrap().push_str(
                                        &keys.key_get_utf8(xkbcommon::xkb::Keycode::new(e.key + 8)),
                                    );
                                }
                                if switch_on_enter && down && e.key == 28 {
                                    pending_layout = Some(e.device.clone());
                                }
                                if keys.update_key(
                                    xkbcommon::xkb::Keycode::new(e.key + 8),
                                    if down {
                                        xkbcommon::xkb::KeyDirection::Down
                                    } else {
                                        xkbcommon::xkb::KeyDirection::Up
                                    },
                                ) != 0
                                {
                                    c.handle().with_next_serial(|serial| {
                                        e.device.interface::<eis::Keyboard>().unwrap().modifiers(
                                            serial,
                                            keys.serialize_mods(
                                                xkbcommon::xkb::STATE_MODS_DEPRESSED,
                                            ),
                                            keys.serialize_mods(xkbcommon::xkb::STATE_MODS_LOCKED),
                                            keys.serialize_mods(xkbcommon::xkb::STATE_MODS_LATCHED),
                                            keys.serialize_layout(
                                                xkbcommon::xkb::STATE_LAYOUT_EFFECTIVE,
                                            ),
                                        )
                                    });
                                }
                                Some(Record::Key(e.key, down))
                            }
                            EisRequest::TextKeysym(e) => Some(Record::Sym(
                                e.keysym,
                                e.state == eis::keyboard::KeyState::Press,
                            )),
                            EisRequest::TextUtf8(e) => {
                                output.lock().unwrap().push_str(&e.text);
                                Some(Record::Text(e.text))
                            }
                            EisRequest::ScrollDiscrete(e) => {
                                Some(Record::Scroll(e.discrete_dx, e.discrete_dy))
                            }
                            EisRequest::Frame(e) => Some(Record::Frame(e.time)),
                            _ => None,
                        };
                        if let Some(r) = record {
                            records.lock().unwrap().push(r);
                        }
                    }
                }
            }
            while let Ok(change) = rx.try_recv() {
                match change {
                    Change::Pause => {
                        device.as_ref().unwrap().paused();
                        keys = xkbcommon::xkb::State::new(&keymap);
                    }
                    Change::Resume => device.as_ref().unwrap().resumed(),
                    Change::Replace => {
                        let d = device.take().unwrap();
                        d.remove();
                        keys = xkbcommon::xkb::State::new(&keymap);
                        device = Some(add_device(d.seat(), text, true, &keymap));
                    }
                    Change::Duplicate => extras.push(add_device(
                        device.as_ref().unwrap().seat(),
                        text,
                        false,
                        &keymap,
                    )),
                    Change::Disconnect => return,
                    Change::ExternalShift => {
                        let d = device.as_ref().unwrap();
                        converter
                            .as_ref()
                            .unwrap()
                            .handle()
                            .with_next_serial(|serial| {
                                d.interface::<eis::Keyboard>()
                                    .unwrap()
                                    .modifiers(serial, 1, 0, 0, 0)
                            });
                    }
                }
            }
            if context.flush().is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    (
        client.into(),
        Fixture {
            changes: tx,
            log,
            typed,
            on_paste,
            paste_shift,
            stop,
            join: Some(join),
        },
    )
}
fn add_device(
    seat: &request::Seat,
    text: bool,
    changed: bool,
    map: &xkbcommon::xkb::Keymap,
) -> request::Device {
    let mut caps = request::DeviceCapability::PointerAbsolute
        | request::DeviceCapability::Button
        | request::DeviceCapability::Scroll
        | request::DeviceCapability::Keyboard;
    if text {
        caps |= request::DeviceCapability::Text;
    }
    seat.add_device(
        Some("screen-a-input"),
        eis::device::DeviceType::Virtual,
        caps,
        |d| {
            d.device().region_mapping_id("screen-a");
            d.device().region(
                if changed { 3000 } else { 200 },
                100,
                if changed { 640 } else { 1920 },
                if changed { 480 } else { 1080 },
                1.,
            );
            let data = map.get_as_string(xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1) + "\0";
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(data.as_bytes()).unwrap();
            d.interface::<eis::Keyboard>().unwrap().keymap(
                eis::keyboard::KeymapType::Xkb,
                data.len() as u32,
                file.as_fd(),
            );
        },
    )
}
pub(super) fn state() -> State {
    State {
        phase: Mutex::new(super::super::Phase::Pending),
        closed: std::sync::atomic::AtomicBool::new(false),
        input_geometry_changed: std::sync::atomic::AtomicBool::new(false),
        eis_active: std::sync::atomic::AtomicBool::new(true),
        input_generation: std::sync::atomic::AtomicU64::new(0),
        alive: Arc::new(|| true),
    }
}
fn stream() -> StreamInfo {
    StreamInfo {
        node: 7,
        serial: Some(99),
        mapping_id: Some("screen-a".into()),
        display: crate::Display {
            id: 1,
            name: "screen".into(),
            x: 0,
            y: 0,
            width: 960,
            height: 540,
            primary: true,
            scale: 2.,
        },
    }
}
async fn ready(sender: &mut Sender, state: &State) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !sender.devices.iter().any(|d| d.resumed) {
            let e = sender.next().await;
            sender.handle(state, e).unwrap();
        }
    })
    .await
    .unwrap();
}
async fn changed(sender: &mut Sender, state: &State, old: u64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.input_generation.load(Ordering::Acquire) == old {
            let e = sender.next().await;
            sender.handle(state, e).unwrap();
        }
        sender.drain(state).unwrap();
    })
    .await
    .unwrap();
}
async fn settle(fixture: &Fixture, min_frames: usize) -> Vec<Record> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let log = fixture.log.lock().unwrap().clone();
            if log.iter().filter(|r| matches!(r, Record::Frame(_))).count() >= min_frames {
                return log;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
async fn send(sender: &mut Sender, state: &State, raw: Vec<Input>) {
    let generation = state.input_generation.load(Ordering::Acquire);
    let p = sender.prepare(state, &stream(), generation, raw).unwrap();
    sender.apply(state, p).await.unwrap();
}
#[tokio::test]
async fn actual_sender_transmits_utf8_shortcuts_pointer_scroll_and_separate_frames() {
    let (fd, fixture) = socket(true);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    let text = "中🟢é".repeat(100);
    send(
        &mut sender,
        &state,
        vec![
            Input::Move { x: 65535, y: 0 },
            Input::Text { text: text.clone() },
            Input::Key {
                key: "ctrl".into(),
                direction: "press".into(),
            },
            Input::Key {
                key: "c".into(),
                direction: "click".into(),
            },
            Input::Key {
                key: "ctrl".into(),
                direction: "release".into(),
            },
            Input::Button {
                button: "left".into(),
                direction: "click".into(),
            },
            Input::Scroll { x: -2, y: 3 },
        ],
    )
    .await;
    let log = settle(&fixture, text_chunks(&text).len() + 8).await;
    assert!(
        log.contains(&Record::Move(2119., 100.)),
        "logical region ignored: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter_map(|r| if let Record::Text(t) = r {
                Some(t.as_str())
            } else {
                None
            })
            .collect::<String>(),
        text
    );
    assert!(log.contains(&Record::Key(29, true)));
    assert!(log.contains(&Record::Key(46, false)));
    assert!(log.contains(&Record::Scroll(-240, 360)));
    for pair in log.windows(2) {
        if !matches!(pair[0], Record::Start(_) | Record::Frame(_)) {
            assert!(matches!(pair[1], Record::Frame(_)));
        }
    }
    assert!(log.iter().all(|r| !matches!(r, Record::Frame(0))));
    assert!(sender.held.keys.is_empty() && sender.held.buttons.is_empty());
}
#[tokio::test]
async fn old_batches_cannot_cross_pause_resume_or_region_replacement() {
    let (fd, fixture) = socket(true);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    send(
        &mut sender,
        &state,
        vec![Input::Key {
            key: "shift".into(),
            direction: "press".into(),
        }],
    )
    .await;
    let before = state.input_generation.load(Ordering::Acquire);
    let p = sender
        .prepare(
            &state,
            &stream(),
            before,
            vec![Input::Move { x: 65535, y: 65535 }],
        )
        .unwrap();
    fixture.changes.send(Change::Pause).unwrap();
    changed(&mut sender, &state, before).await;
    assert!(sender.held.keys.is_empty());
    assert!(sender.apply(&state, p).await.is_err());
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                before,
                vec![Input::Button {
                    button: "left".into(),
                    direction: "click".into()
                }]
            )
            .is_err()
    );
    let old = state.input_generation.load(Ordering::Acquire);
    fixture.changes.send(Change::Resume).unwrap();
    changed(&mut sender, &state, old).await;
    let old = state.input_generation.load(Ordering::Acquire);
    fixture.changes.send(Change::Replace).unwrap();
    changed(&mut sender, &state, old).await;
    ready(&mut sender, &state).await;
    state.input_geometry_changed.store(true, Ordering::Release);
    send(
        &mut sender,
        &state,
        vec![Input::Move { x: 65535, y: 65535 }],
    )
    .await;
    let log = settle(&fixture, 2).await;
    assert!(log.contains(&Record::Move(3639., 579.)));
    assert!(!log.contains(&Record::Move(2119., 1179.)));
    assert!(
        !log.contains(&Record::Key(42, false)),
        "release went to a replacement device"
    );
}
#[tokio::test]
async fn cancellation_and_eof_never_send_or_reopen_input() {
    let (fd, fixture) = socket(true);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    let p = sender
        .prepare(
            &state,
            &stream(),
            state.input_generation.load(Ordering::Acquire),
            vec![Input::Move { x: 100, y: 100 }],
        )
        .unwrap();
    state.closed.store(true, Ordering::Release);
    assert!(sender.apply(&state, p).await.is_err());
    fixture.changes.send(Change::Disconnect).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), sender.next())
        .await
        .unwrap();
    assert!(sender.handle(&state, event).is_err());
    assert!(
        fixture
            .log
            .lock()
            .unwrap()
            .iter()
            .all(|r| matches!(r, Record::Start(_)))
    );
}
#[tokio::test]
async fn unmappable_text_and_ambiguous_regions_fail_before_a_prefix_is_sent() {
    let (fd, fixture) = socket(false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    let generation = state.input_generation.load(Ordering::Acquire);
    let mut other = stream();
    other.mapping_id = Some("screen-b".into());
    assert!(
        sender
            .prepare(&state, &other, generation, vec![Input::Move { x: 1, y: 1 }])
            .is_err(),
        "a different stream borrowed the first region"
    );
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                generation,
                vec![
                    Input::Key {
                        key: "capsLock".into(),
                        direction: "click".into()
                    },
                    Input::Text { text: "a".into() }
                ]
            )
            .is_ok()
    );
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                generation,
                vec![
                    Input::Move { x: 1, y: 1 },
                    Input::Text {
                        text: "abc中".into()
                    }
                ]
            )
            .is_err()
    );
    send(&mut sender, &state, vec![Input::Text { text: "A".into() }]).await;
    let log = settle(&fixture, 4).await;
    assert_eq!(
        log.iter()
            .filter_map(|r| if let Record::Key(k, down) = r {
                Some((*k, *down))
            } else {
                None
            })
            .collect::<Vec<_>>(),
        vec![(42, true), (30, true), (30, false), (42, false)]
    );
    assert!(!log.iter().any(|r| matches!(r, Record::Move(..))));
    let old = state.input_generation.load(Ordering::Acquire);
    fixture.changes.send(Change::Duplicate).unwrap();
    changed(&mut sender, &state, old).await;
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                state.input_generation.load(Ordering::Acquire),
                vec![Input::Move { x: 1, y: 1 }]
            )
            .is_err()
    );
}
#[test]
fn utf8_chunks_follow_wire_byte_limit_and_regions_reject_overflow() {
    let text = "a中🟢é".repeat(1000);
    let chunks = text_chunks(&text);
    assert_eq!(chunks.concat(), text);
    assert!(chunks.iter().all(|t| !t.is_empty() && t.len() <= 254));
    assert!(text_chunks("").is_empty());
    let r = reis::event::Region {
        x: u32::MAX,
        y: 0,
        width: 1,
        height: 1,
        scale: 1.,
        mapping_id: Some("screen-a".into()),
    };
    assert!(validate_region(&r).is_err());
}
#[tokio::test]
async fn external_modifier_changes_invalidate_a_prepared_batch_without_sending_a_prefix() {
    let (fd, fixture) = socket(false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    let generation = state.input_generation.load(Ordering::Acquire);
    let prepared = sender
        .prepare(
            &state,
            &stream(),
            generation,
            vec![Input::Text { text: "a".into() }],
        )
        .unwrap();
    fixture.changes.send(Change::ExternalShift).unwrap();
    changed(&mut sender, &state, generation).await;
    assert!(sender.apply(&state, prepared).await.is_err());
    assert!(
        fixture
            .log
            .lock()
            .unwrap()
            .iter()
            .all(|r| matches!(r, Record::Start(_)))
    );
}
#[tokio::test]
async fn repeated_presses_and_clicks_do_not_release_an_existing_drag_or_modifier() {
    let (fd, fixture) = socket(true);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    for _ in 0..2 {
        send(
            &mut sender,
            &state,
            vec![
                Input::Key {
                    key: "ctrl".into(),
                    direction: "press".into(),
                },
                Input::Button {
                    button: "left".into(),
                    direction: "press".into(),
                },
            ],
        )
        .await;
    }
    let generation = state.input_generation.load(Ordering::Acquire);
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                generation,
                vec![Input::Key {
                    key: "ctrl".into(),
                    direction: "click".into()
                }]
            )
            .is_err()
    );
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                generation,
                vec![Input::Button {
                    button: "left".into(),
                    direction: "click".into()
                }]
            )
            .is_err()
    );
    let log = settle(&fixture, 2).await;
    assert_eq!(
        log.iter()
            .filter(|r| matches!(r, Record::Key(29, true) | Record::Button(0x110, true)))
            .count(),
        2
    );
    assert!(!log.contains(&Record::Key(29, false)) && !log.contains(&Record::Button(0x110, false)));
    sender.release(&state).await.unwrap();
    let log = settle(&fixture, 4).await;
    assert!(log.contains(&Record::Key(29, false)) && log.contains(&Record::Button(0x110, false)));
    assert!(sender.held.keys.is_empty() && sender.held.buttons.is_empty());
}
fn key(name: &str, direction: &str) -> Input {
    Input::Key {
        key: name.into(),
        direction: direction.into(),
    }
}
fn text(value: &str) -> Input {
    Input::Text { text: value.into() }
}

#[tokio::test]
async fn mixed_text_after_caps_lock_and_released_shortcuts_uses_the_actual_layout() {
    let (fd, fixture) = socket(false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    send(
        &mut sender,
        &state,
        vec![
            key("ctrl", "press"),
            key("c", "click"),
            key("ctrl", "release"),
            key("capsLock", "click"),
            text("aA"),
            key("capsLock", "click"),
            text("zZ"),
        ],
    )
    .await;
    assert_eq!(*fixture.typed.lock().unwrap(), "\u{3}aAzZ");
    assert!(sender.held.keys.is_empty());
}
#[tokio::test]
async fn mixed_release_of_a_previous_modifier_allows_plain_text_without_losing_the_owner() {
    let (fd, fixture) = socket(false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    send(&mut sender, &state, vec![key("ctrl", "press")]).await;
    send(&mut sender, &state, vec![key("ctrl", "release"), text("a")]).await;
    assert_eq!(*fixture.typed.lock().unwrap(), "a");
    assert!(sender.held.keys.is_empty());
}
#[tokio::test]
async fn mixed_layout_toggle_wraps_and_uses_the_new_group_for_each_following_text() {
    let (fd, fixture) = socket_config(false, "us,de", "grp:alt_shift_toggle", false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    send(
        &mut sender,
        &state,
        vec![
            key("alt", "press"),
            key("shift", "click"),
            key("alt", "release"),
            text("yY"),
            key("alt", "press"),
            key("shift", "click"),
            key("alt", "release"),
            text("yY"),
        ],
    )
    .await;
    assert_eq!(*fixture.typed.lock().unwrap(), "yYyY");
    let log = fixture.log.lock().unwrap();
    assert!(log.contains(&Record::Key(44, true)) && log.contains(&Record::Key(21, true)));
}

#[tokio::test]
async fn actual_sender_types_non_latin_layouts_without_text_or_clipboard_capabilities() {
    for (layout, text_value) in [("ru", "Привет Ёё"), ("gr", "Ελλαδα Ωω")] {
        let (fd, fixture) = socket_config(false, layout, "", false);
        let mut sender = Sender::open(fd).await.unwrap();
        let state = state();
        ready(&mut sender, &state).await;
        send(&mut sender, &state, vec![text(text_value)]).await;
        assert_eq!(*fixture.typed.lock().unwrap(), text_value);
        assert!(
            fixture
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, Record::Key(..)))
        );
        assert!(sender.held.keys.is_empty());
    }
}

#[tokio::test]
async fn a_mixed_batch_switches_into_and_out_of_a_non_latin_group() {
    let (fd, fixture) = socket_config(false, "us,ru", "grp:alt_shift_toggle", false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    send(
        &mut sender,
        &state,
        vec![
            text("abc"),
            key("alt", "press"),
            key("shift", "click"),
            key("alt", "release"),
            text("Привет"),
            key("alt", "press"),
            key("shift", "click"),
            key("alt", "release"),
            text("XYZ"),
        ],
    )
    .await;
    assert_eq!(*fixture.typed.lock().unwrap(), "abcПриветXYZ");
    assert!(sender.held.keys.is_empty());
}
#[tokio::test]
async fn mixed_invalid_suffix_has_no_effect_on_the_live_keyboard_or_remote_desktop() {
    let (fd, fixture) = socket(false);
    let mut sender = Sender::open(fd).await.unwrap();
    let state = state();
    ready(&mut sender, &state).await;
    let generation = state.input_generation.load(Ordering::Acquire);
    assert!(
        sender
            .prepare(
                &state,
                &stream(),
                generation,
                vec![key("capsLock", "click"), text("a中")]
            )
            .is_err()
    );
    assert!(
        fixture
            .log
            .lock()
            .unwrap()
            .iter()
            .all(|r| matches!(r, Record::Start(_)))
    );
    send(&mut sender, &state, vec![text("a")]).await;
    assert_eq!(
        *fixture.typed.lock().unwrap(),
        "a",
        "preflight toggled the live caps lock state"
    );
}
#[tokio::test]
async fn mixed_shortcut_feedback_cannot_send_a_suffix_prepared_for_a_different_layout() {
    for native_text in [false, true] {
        let (fd, fixture) = socket_config(native_text, "us,de", "", true);
        let mut sender = Sender::open(fd).await.unwrap();
        let state = state();
        ready(&mut sender, &state).await;
        let prepared = sender
            .prepare(
                &state,
                &stream(),
                state.input_generation.load(Ordering::Acquire),
                vec![key("enter", "click"), text("y")],
            )
            .unwrap();
        assert!(sender.apply(&state, prepared).await.is_err());
        assert_eq!(
            *fixture.typed.lock().unwrap(),
            "\r",
            "text was sent after reported layout change"
        );
        assert!(
            !fixture
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Record::Key(21, true) | Record::Text(_)))
        );
    }
}
