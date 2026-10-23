//! Opt-in fixture: isolated D-Bus portal; optional private PipeWire test source.
//! Never runs against the user's portal, desktop or input devices.
use super::portal::{DEST, ROOT, SESSION};
use super::*;
use std::collections::HashMap;
use zbus::{
    Connection,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
#[derive(Default)]
struct Log {
    clients: Vec<String>,
    sessions: Vec<String>,
    options: Vec<(String, Vec<String>, Option<String>)>,
    input: Vec<String>,
    closes: usize,
}
#[derive(Default)]
struct Fixture {
    log: Mutex<Log>,
    start_mode: AtomicUsize,
    fail_input: AtomicBool,
    stall_input: AtomicBool,
    input_started: AtomicUsize,
}
impl Fixture {
    fn record(&self, method: &str, options: &HashMap<String, OwnedValue>) {
        let restore = options
            .get("restore_token")
            .and_then(|v| <&str>::try_from(v).ok())
            .map(str::to_owned);
        self.log.lock().unwrap().options.push((
            method.into(),
            options.keys().cloned().collect(),
            restore,
        ));
    }
}
fn request_path(header: &Header<'_>, options: &HashMap<String, OwnedValue>) -> OwnedObjectPath {
    let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
    let unique = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    OwnedObjectPath::try_from(format!("{ROOT}/request/{unique}/{token}")).unwrap()
}
async fn respond(
    connection: &Connection,
    header: &Header<'_>,
    path: OwnedObjectPath,
    code: u32,
    values: HashMap<&str, Value<'_>>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    connection
        .emit_signal(
            header.sender().map(|s| s.as_str()),
            path.clone(),
            "org.freedesktop.portal.Request",
            "Response",
            &(code, values),
        )
        .await
        .unwrap();
    Ok(path)
}
async fn create(
    shared: &Arc<Fixture>,
    connection: &Connection,
    header: &Header<'_>,
    options: HashMap<String, OwnedValue>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    shared.record("CreateSession", &options);
    let unique = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let token = <&str>::try_from(options.get("session_handle_token").unwrap()).unwrap();
    let session = format!("{ROOT}/session/{unique}/{token}");
    connection
        .object_server()
        .at(session.clone(), MockSession(shared.clone()))
        .await
        .unwrap();
    shared.log.lock().unwrap().sessions.push(session.clone());
    shared
        .log
        .lock()
        .unwrap()
        .clients
        .push(header.sender().unwrap().as_str().into());
    respond(
        connection,
        header,
        request_path(header, &options),
        0,
        HashMap::from([("session_handle", Value::from(session))]),
    )
    .await
}
async fn start(
    shared: &Fixture,
    connection: &Connection,
    header: &Header<'_>,
    options: HashMap<String, OwnedValue>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    shared.record("Start", &options);
    while shared.start_mode.load(Ordering::Acquire) == 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let count = shared
        .log
        .lock()
        .unwrap()
        .options
        .iter()
        .filter(|(method, _, _)| method == "Start")
        .count();
    let node = std::env::var("DOGCODES_OWNED_PIPEWIRE_NODE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(47u32);
    let mut props = HashMap::from([
        ("id", Value::from("fixture-screen")),
        ("size", Value::from((1728i32, 1117i32))),
        ("position", Value::from((-1728i32, 100i32))),
        ("source_type", Value::from(1u32)),
    ]);
    if let Ok(serial) = std::env::var("DOGCODES_OWNED_PIPEWIRE_SERIAL") {
        props.insert(
            "pipewire-serial",
            Value::from(serial.parse::<u64>().unwrap()),
        );
    }
    let streams = vec![(node, props)];
    let values = HashMap::from([
        ("devices", Value::from(3u32)),
        ("streams", Value::new(streams)),
        (
            "restore_token",
            Value::from(format!("fixture-token-{count}")),
        ),
    ]);
    if shared.start_mode.load(Ordering::Acquire) == 3 {
        let session = shared.log.lock().unwrap().sessions.last().unwrap().clone();
        connection
            .emit_signal(
                None::<&str>,
                session,
                SESSION,
                "Closed",
                &(HashMap::<String, Value<'_>>::new(),),
            )
            .await
            .unwrap();
    }
    respond(
        connection,
        header,
        request_path(header, &options),
        u32::from(shared.start_mode.load(Ordering::Acquire) == 1),
        values,
    )
    .await
}
struct MockSession(Arc<Fixture>);
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl MockSession {
    async fn close(
        &self,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) {
        self.0.log.lock().unwrap().closes += 1;
        connection
            .emit_signal(
                None::<&str>,
                header.path().unwrap(),
                SESSION,
                "Closed",
                &(HashMap::<String, Value<'_>>::new(),),
            )
            .await
            .unwrap();
    }
}
struct MockRemote(Arc<Fixture>);
#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl MockRemote {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        create(&self.0, connection, &header, options).await
    }
    async fn select_devices(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.record("SelectDevices", &options);
        respond(
            connection,
            &header,
            request_path(&header, &options),
            0,
            HashMap::new(),
        )
        .await
    }
    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        start(&self.0, connection, &header, options).await
    }
    fn notify_pointer_motion_absolute(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        stream: u32,
        x: f64,
        y: f64,
    ) {
        self.0
            .log
            .lock()
            .unwrap()
            .input
            .push(format!("move:{stream}:{x}:{y}"));
    }
    fn notify_pointer_button(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        button: i32,
        state: u32,
    ) {
        self.0
            .log
            .lock()
            .unwrap()
            .input
            .push(format!("button:{button}:{state}"));
    }
    fn notify_pointer_axis_discrete(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        axis: u32,
        steps: i32,
    ) {
        self.0
            .log
            .lock()
            .unwrap()
            .input
            .push(format!("scroll:{axis}:{steps}"));
    }
    async fn notify_keyboard_keysym(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        keysym: i32,
        state: u32,
    ) -> zbus::fdo::Result<()> {
        self.0.input_started.fetch_add(1, Ordering::AcqRel);
        if self.0.stall_input.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
        self.0
            .log
            .lock()
            .unwrap()
            .input
            .push(format!("key:{keysym}:{state}"));
        if self.0.fail_input.load(Ordering::Acquire) {
            return Err(zbus::fdo::Error::Failed("fixture input failure".into()));
        }
        Ok(())
    }
}
struct MockCast(Arc<Fixture>);
#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl MockCast {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        6
    }
    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        3
    }
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        create(&self.0, connection, &header, options).await
    }
    async fn select_sources(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.record("SelectSources", &options);
        respond(
            connection,
            &header,
            request_path(&header, &options),
            0,
            HashMap::new(),
        )
        .await
    }
    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        start(&self.0, connection, &header, options).await
    }
    fn open_pipe_wire_remote(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        let root = std::env::var("XDG_RUNTIME_DIR").unwrap();
        std::os::unix::net::UnixStream::connect(format!("{root}/pipewire-0"))
            .map(std::os::fd::OwnedFd::from)
            .map(zbus::zvariant::OwnedFd::from)
            .map_err(|_| zbus::fdo::Error::Failed("fixture has no PipeWire".into()))
    }
}
async fn wait_for(mut predicate: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(6);
    while !predicate() {
        assert!(std::time::Instant::now() < until, "fixture deadline");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn ready(session: &Session) {
    wait_for(|| session.status() != Ok(false)).await;
    assert_eq!(session.status(), Ok(true));
}
#[tokio::test]
#[ignore = "requires an isolated dbus-run-session, never the user's desktop portal"]
async fn owned_portal_fixture_verifies_rotation_grants_input_and_disconnect() {
    assert_eq!(
        std::env::var("DOGCODES_OWNED_PORTAL").as_deref(),
        Ok("portal-fixture-v1")
    );
    let shared = Arc::new(Fixture::default());
    let service = zbus::connection::Builder::session()
        .unwrap()
        .name(DEST)
        .unwrap()
        .serve_at(ROOT, MockRemote(shared.clone()))
        .unwrap()
        .serve_at(ROOT, MockCast(shared.clone()))
        .unwrap()
        .build()
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(TokenStore::new(directory.path().join("portal")));
    let session = Session::start(store.clone(), true).unwrap();
    ready(&session).await;
    assert!(session.cursor_embedded());
    let displays = session.displays().unwrap();
    assert_eq!((displays[0].x, displays[0].width), (-1728, 1728));
    {
        let log = shared.log.lock().unwrap();
        let sources = log
            .options
            .iter()
            .find(|(method, _, _)| method == "SelectSources")
            .unwrap();
        assert!(
            !sources.1.contains(&"persist_mode".into())
                && !sources.1.contains(&"restore_token".into())
        );
        assert!(
            log.options
                .iter()
                .any(|(method, keys, _)| method == "SelectDevices"
                    && keys.contains(&"persist_mode".into()))
        );
    }
    let sending = session.clone();
    tokio::task::spawn_blocking(move || {
        sending.send(
            1,
            vec![
                Input::Move { x: 65535, y: 0 },
                Input::Key {
                    key: "ctrl".into(),
                    direction: "press".into(),
                },
            ],
        )
    })
    .await
    .unwrap()
    .unwrap();
    session.release_held();
    wait_for(|| {
        shared
            .log
            .lock()
            .unwrap()
            .input
            .iter()
            .any(|line| line == "key:65507:0")
    })
    .await;
    if std::env::var_os("DOGCODES_OWNED_PIPEWIRE_NODE").is_some() {
        let capturing = session.clone();
        let source = tokio::task::spawn_blocking(move || capturing.capture(1))
            .await
            .unwrap()
            .unwrap();
        let image = source.frame().unwrap();
        assert_eq!(image.dimensions(), (320, 240));
        assert!(source.cursor_embedded());
        // Recreate differently sized encoders on the same authorized source;
        // each begins with an independently decodable SPS/PPS/IDR.
        for width in [320, 160, 320] {
            let encoded = Arc::new(Mutex::new(Vec::new()));
            let output = encoded.clone();
            let video_source = source.clone();
            let stream = tokio::task::spawn_blocking(move || {
                crate::video::VideoStream::start_from(
                    crate::video::VideoConfig {
                        display: 1,
                        max_width: width,
                        fps: 20,
                        bitrate: 800_000,
                        profile: crate::video::VideoProfile::ConstrainedBaseline31,
                    },
                    Arc::new(move |frame| output.lock().unwrap().push(frame)),
                    Some(video_source),
                )
            })
            .await
            .unwrap()
            .unwrap();
            wait_for(|| !encoded.lock().unwrap().is_empty()).await;
            let first = encoded.lock().unwrap()[0].clone();
            assert!(first.keyframe);
            let mut decoder = openh264::decoder::Decoder::new().unwrap();
            let decoded = decoder.decode(&first.data).unwrap().unwrap();
            use openh264::formats::YUVSource;
            assert_eq!(
                decoded.dimensions(),
                (width as usize, (width * 3 / 4) as usize)
            );
            assert_eq!(stream.size_for_width(3840).unwrap(), (320, 240));
            assert_eq!(stream.size_for_width(160).unwrap(), (160, 120));
            drop(stream);
        }
        drop(source);
        println!("restricted FD -> actual PipeWire pixels: 320x240");
    }
    session.close();
    drop(session);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    let next = Session::start(store.clone(), true).unwrap();
    ready(&next).await;
    assert!(
        shared
            .log
            .lock()
            .unwrap()
            .options
            .iter()
            .any(|(method, _, restore)| method == "SelectDevices"
                && restore.as_deref() == Some("fixture-token-1"))
    );
    shared.fail_input.store(true, Ordering::Release);
    let sending = next.clone();
    assert!(
        tokio::task::spawn_blocking(move || sending.send(
            1,
            vec![
                Input::Key {
                    key: "a".into(),
                    direction: "press".into()
                },
                Input::Key {
                    key: "b".into(),
                    direction: "press".into()
                }
            ]
        ))
        .await
        .unwrap()
        .is_err()
    );
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    assert!(
        !shared
            .log
            .lock()
            .unwrap()
            .input
            .iter()
            .any(|s| s == "key:98:1")
    );
    drop(next);
    // A fresh source choice consumes the old restore token but never uses it.
    let fresh = Session::start_owned(store.clone(), true, true, Arc::new(|| true)).unwrap();
    ready(&fresh).await;
    let device_options = shared
        .log
        .lock()
        .unwrap()
        .options
        .iter()
        .rev()
        .find(|(method, _, _)| method == "SelectDevices")
        .cloned()
        .unwrap();
    assert!(device_options.2.is_none());
    fresh.close();
    drop(fresh);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // Cancelling a pending picker disconnects its private bus; late grant dies with it.
    shared.start_mode.store(2, Ordering::Release);
    let before = shared
        .log
        .lock()
        .unwrap()
        .options
        .iter()
        .filter(|(m, _, _)| m == "Start")
        .count();
    let pending = Session::start(store.clone(), true).unwrap();
    wait_for(|| {
        shared
            .log
            .lock()
            .unwrap()
            .options
            .iter()
            .filter(|(m, _, _)| m == "Start")
            .count()
            > before
    })
    .await;
    let client = shared.log.lock().unwrap().clients.last().unwrap().clone();
    drop(pending);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    let bus = zbus::fdo::DBusProxy::new(&service).await.unwrap();
    assert!(
        !bus.name_has_owner(zbus::names::BusName::try_from(client.as_str()).unwrap())
            .await
            .unwrap()
    );
    shared.start_mode.store(1, Ordering::Release);
    let denied = Session::start(store.clone(), true).unwrap();
    wait_for(|| denied.status() != Ok(false)).await;
    assert!(denied.status().unwrap_err().contains("取消"));
    drop(denied);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    shared.start_mode.store(3, Ordering::Release);
    let revoked = Session::start(store.clone(), true).unwrap();
    wait_for(|| revoked.status() != Ok(false)).await;
    assert!(
        revoked.status().is_err(),
        "a grant immediately revoked must never become ready"
    );
    drop(revoked);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    shared.start_mode.store(0, Ordering::Release);
    shared.fail_input.store(false, Ordering::Release);
    let before_view = shared.log.lock().unwrap().options.len();
    let view = Session::start(store.clone(), false).unwrap();
    ready(&view).await;
    {
        let log = shared.log.lock().unwrap();
        let options = &log.options[before_view..];
        assert!(
            !options
                .iter()
                .any(|(method, _, _)| method == "SelectDevices")
        );
        let sources = options
            .iter()
            .find(|(method, _, _)| method == "SelectSources")
            .unwrap();
        assert!(sources.1.contains(&"persist_mode".into()));
        assert!(
            sources.2.is_none(),
            "view-only must not reuse a control token"
        );
    }
    let sending = view.clone();
    assert!(
        tokio::task::spawn_blocking(move || sending.send(1, vec![Input::Move { x: 0, y: 0 }]))
            .await
            .unwrap()
            .is_err()
    );
    drop(view);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // The transport owner disappearing cancels in-flight input promptly,
    // without waiting for the 3s D-Bus deadline or sending the next key.
    shared.stall_input.store(true, Ordering::Release);
    let alive = Arc::new(AtomicBool::new(true));
    let owner = alive.clone();
    let owned = Session::start_owned(
        store,
        true,
        false,
        Arc::new(move || owner.load(Ordering::Acquire)),
    )
    .unwrap();
    ready(&owned).await;
    let before_input = shared.input_started.load(Ordering::Acquire);
    let sending = owned.clone();
    let job = tokio::task::spawn_blocking(move || {
        sending.send(
            1,
            vec![
                Input::Key {
                    key: "c".into(),
                    direction: "press".into(),
                },
                Input::Key {
                    key: "d".into(),
                    direction: "press".into(),
                },
            ],
        )
    });
    wait_for(|| shared.input_started.load(Ordering::Acquire) > before_input).await;
    let now = std::time::Instant::now();
    alive.store(false, Ordering::Release);
    assert!(job.await.unwrap().is_err());
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    assert!(
        now.elapsed() < Duration::from_secs(1),
        "old transport must cancel an in-flight batch"
    );
    assert!(owned.status().is_err());
    drop(owned);
    assert!(
        !shared
            .log
            .lock()
            .unwrap()
            .input
            .iter()
            .any(|s| s == "key:100:1")
    );
    assert!(shared.log.lock().unwrap().closes >= 3);
}
