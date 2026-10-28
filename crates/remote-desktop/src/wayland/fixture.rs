//! Opt-in fixture: isolated D-Bus portal; optional private PipeWire test source.
//! Never runs against the user's portal, desktop or input devices.
#[path = "fixture_clipboard.rs"]
mod clipboard;
#[path = "fixture_focus.rs"]
mod focus;
#[path = "fixture_source.rs"]
mod source;
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
    eis_mode: AtomicUsize,
    eis: Mutex<Option<super::eis::tests::Fixture>>,
    clipboard: Arc<clipboard::Fixture>,
    focus: Arc<focus::Fixture>,
    start_mode: AtomicUsize,
    fail_input: AtomicBool,
    stall_input: AtomicBool,
    input_started: AtomicUsize,
    logind_locked: AtomicBool,
    logind_user_queries: AtomicUsize,
    logind_queries: AtomicUsize,
    logind_lock_after: AtomicUsize,
    logind_unavailable: AtomicBool,
    revoke_after: AtomicUsize,
    revoke: Mutex<Option<Session>>,
}
struct MockLogindManager(Arc<Fixture>);
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl MockLogindManager {
    fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if id != "dogcodes-fixture" {
            return Err(zbus::fdo::Error::UnknownObject(
                "expired fixture session".into(),
            ));
        }
        Ok(OwnedObjectPath::try_from("/org/freedesktop/login1/session/fixture").unwrap())
    }
    fn get_session_by_pid(&self, _pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        Err(zbus::fdo::Error::UnknownObject(
            "owned Core user-service has no login session".into(),
        ))
    }
    fn get_user(&self, _uid: u32) -> OwnedObjectPath {
        self.0.logind_user_queries.fetch_add(1, Ordering::SeqCst);
        OwnedObjectPath::try_from("/org/freedesktop/login1/user/fixture").unwrap()
    }
}
struct MockLogindUser;
#[zbus::interface(name = "org.freedesktop.login1.User")]
impl MockLogindUser {
    #[zbus(property)]
    fn display(&self) -> (String, OwnedObjectPath) {
        (
            "dogcodes-fixture".into(),
            OwnedObjectPath::try_from("/org/freedesktop/login1/session/fixture").unwrap(),
        )
    }
}
struct MockLogindSession(Arc<Fixture>);
#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl MockLogindSession {
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::fdo::Result<bool> {
        if self.0.logind_unavailable.load(Ordering::Acquire) {
            return Err(zbus::fdo::Error::Failed(
                "owned lock state unavailable".into(),
            ));
        }
        let locked = self.0.logind_locked.load(Ordering::Acquire);
        let count = self.0.logind_queries.fetch_add(1, Ordering::SeqCst) + 1;
        if count == self.0.logind_lock_after.load(Ordering::SeqCst) {
            self.0.logind_locked.store(true, Ordering::Release);
        }
        if count == self.0.revoke_after.load(Ordering::SeqCst)
            && let Some(session) = self.0.revoke.lock().unwrap().take()
        {
            session.close();
        }
        Ok(locked)
    }
    #[zbus(property)]
    fn active(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn remote(&self) -> bool {
        false
    }
    #[zbus(property, name = "Type")]
    fn kind(&self) -> &str {
        "wayland"
    }
    #[zbus(property)]
    fn user(&self) -> (u32, OwnedObjectPath) {
        // SAFETY: geteuid takes no pointers and returns this fixture's UID.
        (
            unsafe { libc::geteuid() },
            OwnedObjectPath::try_from("/org/freedesktop/login1/user/fixture").unwrap(),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "only the owned desktop codec launcher starts this private login1 service"]
async fn serve_logind_fixture() {
    assert_eq!(
        std::env::var("DOGCODES_OWNED_SESSION_STATE").as_deref(),
        Ok("login1-fixture-v1")
    );
    let root = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
    let marker = std::env::var("DOGCODES_SESSION_STATE_MARKER").unwrap();
    assert!(!marker.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join(".fixture-owned")).unwrap(),
        marker
    );
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    assert!(!bus.is_empty());
    assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap(), bus);
    let shared = Arc::new(Fixture::default());
    let _service = zbus::connection::Builder::session()
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at("/org/freedesktop/login1", MockLogindManager(shared.clone()))
        .unwrap()
        .serve_at("/org/freedesktop/login1/user/fixture", MockLogindUser)
        .unwrap()
        .serve_at(
            "/org/freedesktop/login1/session/fixture",
            MockLogindSession(shared),
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    std::fs::write(root.join(".login1-ready"), marker).unwrap();
    std::future::pending::<()>().await;
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
    if shared.eis_mode.load(Ordering::Acquire) != 0 {
        props.insert("mapping_id", Value::from("screen-a"));
    }
    if let Ok(serial) = std::env::var("DOGCODES_OWNED_PIPEWIRE_SERIAL") {
        props.insert(
            "pipewire-serial",
            Value::from(serial.parse::<u64>().unwrap()),
        );
    }
    let streams = vec![(node, props)];
    let values = HashMap::from([
        ("clipboard_enabled", Value::from(shared.clipboard.granted())),
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
    #[zbus(name = "ConnectToEIS")]
    fn connect_to_eis(
        &self,
        session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        assert!(
            self.0
                .log
                .lock()
                .unwrap()
                .sessions
                .contains(&session.to_string())
        );
        self.0.record("ConnectToEIS", &options);
        let (fd, fixture) = super::eis::tests::socket(self.0.eis_mode.load(Ordering::Acquire) != 3);
        if self.0.eis_mode.load(Ordering::Acquire) == 2 {
            drop(fixture);
        } else {
            *self.0.eis.lock().unwrap() = Some(fixture);
        }
        Ok(fd.into())
    }
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
        if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("rgb-cursor") {
            5
        } else {
            3
        }
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
        let expected =
            if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("rgb-cursor") {
                4
            } else {
                2
            };
        assert_eq!(
            u32::try_from(options.get("cursor_mode").unwrap()).unwrap(),
            expected
        );
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated dbus-run-session, never the user's desktop portal"]
async fn owned_portal_fixture_verifies_rotation_grants_input_and_disconnect() {
    let system = std::env::var("DBUS_SYSTEM_BUS_ADDRESS").expect("owned login1 bus");
    assert!(!system.is_empty());
    assert_eq!(
        system,
        std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("owned portal bus")
    );
    assert_eq!(
        std::env::var("DOGCODES_OWNED_PORTAL").as_deref(),
        Ok("portal-fixture-v1")
    );
    let shared = Arc::new(Fixture::default());
    let service = zbus::connection::Builder::session()
        .unwrap()
        .name(DEST)
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at("/org/freedesktop/login1", MockLogindManager(shared.clone()))
        .unwrap()
        .serve_at("/org/freedesktop/login1/user/fixture", MockLogindUser)
        .unwrap()
        .serve_at(
            "/org/freedesktop/login1/session/fixture",
            MockLogindSession(shared.clone()),
        )
        .unwrap()
        .serve_at(ROOT, MockRemote(shared.clone()))
        .unwrap()
        .serve_at(ROOT, clipboard::Mock(shared.clipboard.clone()))
        .unwrap()
        .serve_at(ROOT, MockCast(shared.clone()))
        .unwrap()
        .build()
        .await
        .unwrap();
    // Verify the actual D-Bus selection logic, including a background Core
    // without a login session and refusal to switch an expired explicit ID.
    // SAFETY: geteuid takes no pointers and reads only this fixture's UID.
    let uid = unsafe { libc::geteuid() };
    let pid = std::process::id();
    assert!(
        !crate::session::fixture_login1_query(&service, Some("dogcodes-fixture"), pid, uid)
            .await
            .unwrap()
    );
    assert!(
        !crate::session::fixture_login1_query(&service, None, pid, uid)
            .await
            .unwrap()
    );
    assert_eq!(shared.logind_user_queries.load(Ordering::SeqCst), 1);
    shared.logind_unavailable.store(true, Ordering::Release);
    assert!(
        tokio::task::spawn_blocking(crate::require_unlocked)
            .await
            .unwrap()
            .is_err()
    );
    shared.logind_unavailable.store(false, Ordering::Release);
    tokio::task::spawn_blocking(crate::require_unlocked)
        .await
        .unwrap()
        .unwrap();
    assert!(
        crate::session::fixture_login1_query(&service, Some("expired"), pid, uid)
            .await
            .is_err()
    );
    assert_eq!(shared.logind_user_queries.load(Ordering::SeqCst), 1);
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
        let nv12 = std::env::var("DOGCODES_OWNED_VIDEO_FORMAT")
            .is_ok_and(|format| format.starts_with("nv12-"));
        let geometry = std::env::var("DOGCODES_OWNED_VIDEO_FORMAT")
            .is_ok_and(|format| format.starts_with("rgb-crop-"));
        if geometry {
            let bytes = image::DynamicImage::ImageRgba8((*image).clone())
                .into_rgb8()
                .into_raw();
            check_corners(&bytes, 320, 240);
            let node = std::env::var("DOGCODES_OWNED_PIPEWIRE_NODE").unwrap();
            let moving = session.clone();
            tokio::task::spawn_blocking(move || {
                moving.send(
                    1,
                    vec![
                        Input::Move { x: 0, y: 0 },
                        Input::Move { x: 65535, y: 0 },
                        Input::Move { x: 0, y: 65535 },
                        Input::Move { x: 65535, y: 65535 },
                    ],
                )
            })
            .await
            .unwrap()
            .unwrap();
            for (x, y) in [(0, 0), (1727, 0), (0, 1116), (1727, 1116)] {
                assert!(
                    shared
                        .log
                        .lock()
                        .unwrap()
                        .input
                        .iter()
                        .any(|line| line == &format!("move:{node}:{x}:{y}"))
                );
            }
            assert!(
                shared
                    .log
                    .lock()
                    .unwrap()
                    .input
                    .iter()
                    .any(|line| line == &format!("move:{node}:1727:0"))
            );
        }
        if nv12 {
            let pixel = image.get_pixel(10, 10);
            assert!(
                pixel[0] >= 240 && pixel[1] <= 16 && pixel[2] <= 16,
                "NV12 red: {pixel:?}"
            );
            println!("owned NV12 color conversion: red pixel {pixel:?}");
        }
        assert!(source.cursor_embedded());
        if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("rgb-cursor") {
            assert_eq!(image.get_pixel(20, 30).0, [0, 0, 255, 255]);
            let root = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
            std::fs::write(root.join(".force-empty"), b"metadata-only").unwrap();
            std::fs::write(root.join(".cursor-moved"), b"new-position").unwrap();
            wait_for(|| {
                source
                    .frame()
                    .is_ok_and(|image| image.get_pixel(60, 30).0 == [0, 0, 255, 255])
            })
            .await;
            let moved = source.frame().unwrap();
            assert_eq!(moved.get_pixel(20, 30).0, [255, 0, 0, 255]);
            std::fs::write(root.join(".cursor-hidden"), b"hide").unwrap();
            wait_for(|| {
                source
                    .frame()
                    .is_ok_and(|image| image.get_pixel(60, 30).0 == [255, 0, 0, 255])
            })
            .await;
            assert_eq!(
                source.frame().unwrap().get_pixel(20, 30).0,
                [255, 0, 0, 255]
            );
            for name in [".force-empty", ".cursor-moved", ".cursor-hidden"] {
                std::fs::remove_file(root.join(name)).unwrap();
            }
            wait_for(|| {
                source
                    .frame()
                    .is_ok_and(|image| image.get_pixel(20, 30).0 == [0, 0, 255, 255])
            })
            .await;
        }
        // Recreate differently sized encoders on the same authorized source;
        // each begins with an independently decodable SPS/PPS/IDR.
        for (position, width) in [320, 160, 320].into_iter().enumerate() {
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
            if nv12 || geometry {
                let mut rgb = vec![0; decoded.dimensions().0 * decoded.dimensions().1 * 3];
                decoded.write_rgb8(&mut rgb);
                if geometry {
                    check_corners(&rgb, decoded.dimensions().0, decoded.dimensions().1);
                }
                if nv12 {
                    let offset = (10 * width as usize + 10) * 3;
                    let pixel = &rgb[offset..offset + 3];
                    assert!(
                        pixel[0] >= 240 && pixel[1] <= 16 && pixel[2] <= 16,
                        "NV12 -> H.264 -> RGB at {width}px: {pixel:?}"
                    );
                }
            }
            if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("rgb-cursor") {
                let mut rgb = vec![0; decoded.dimensions().0 * decoded.dimensions().1 * 3];
                decoded.write_rgb8(&mut rgb);
                let (x, y) = (22 * width as usize / 320, 32 * width as usize / 320);
                let offset = (y * decoded.dimensions().0 + x) * 3;
                assert!(
                    rgb[offset + 2] > 180 && rgb[offset] < 80,
                    "cursor lost during H264: {:?}",
                    &rgb[offset..offset + 3]
                );
            }
            assert_eq!(stream.size_for_width(3840).unwrap(), (320, 240));
            assert_eq!(stream.size_for_width(160).unwrap(), (160, 120));
            if position == 2 {
                // A real post-encode native guard runs against only this private
                // login1 service. No host lock state or display is consulted.
                shared.logind_locked.store(true, Ordering::Release);
                stream.request_keyframe();
                wait_for(|| stream.error().is_some()).await;
                assert!(stream.error().unwrap().contains("锁定"));
                let count = encoded.lock().unwrap().len();
                shared.logind_locked.store(false, Ordering::Release);
                stream.request_keyframe();
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert_eq!(encoded.lock().unwrap().len(), count);
            }
            drop(stream);
        }
        let mut jpeg = crate::Capturer::from_portal(&session, 1).unwrap();
        let (returned, preview) = tokio::task::spawn_blocking(move || {
            let preview = jpeg.frame(320, 70, None);
            (jpeg, preview)
        })
        .await
        .unwrap();
        jpeg = returned;
        let preview = preview.unwrap();
        assert_eq!(preview["cursorEmbedded"], true);
        let known = preview["frameId"].as_str().unwrap().to_owned();
        let (returned, unchanged) = tokio::task::spawn_blocking(move || {
            let unchanged = jpeg.frame(320, 70, Some(&known));
            (jpeg, unchanged)
        })
        .await
        .unwrap();
        jpeg = returned;
        let unchanged = unchanged.unwrap();
        assert_eq!(unchanged["unchanged"], true);
        assert_eq!(unchanged["cursorEmbedded"], true);
        assert!(unchanged.get("bytes").is_none());
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            preview["bytes"].as_str().unwrap(),
        )
        .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (320, 240));
        if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("rgb-cursor") {
            let pixel = decoded.get_pixel(22, 32);
            assert!(
                pixel[2] > 180 && pixel[0] < 80,
                "cursor lost during JPEG: {pixel:?}"
            );
        }
        if geometry {
            check_corners(decoded.as_raw(), 320, 240);
        }
        let checks = shared.logind_queries.load(Ordering::SeqCst);
        // Start and pre-encode checks both see unlocked. Lock immediately after
        // the latter's answer; only a final post-JPEG check can reject the bytes.
        shared.logind_lock_after.store(checks + 2, Ordering::SeqCst);
        let (mut jpeg, result) = tokio::task::spawn_blocking(move || {
            let result = jpeg.frame(320, 70, None);
            (jpeg, result)
        })
        .await
        .unwrap();
        assert!(
            result.is_err(),
            "JPEG pixels were returned after the lock transition"
        );
        assert!(result.unwrap_err().contains("锁定"));
        shared.logind_lock_after.store(0, Ordering::SeqCst);
        shared.logind_locked.store(false, Ordering::Release);
        *shared.revoke.lock().unwrap() = Some(session.clone());
        shared.revoke_after.store(
            shared.logind_queries.load(Ordering::SeqCst) + 3,
            Ordering::SeqCst,
        );
        let result = tokio::task::spawn_blocking(move || jpeg.frame(320, 70, None))
            .await
            .unwrap();
        assert!(
            result.is_err(),
            "JPEG pixels were returned after the original portal grant ended"
        );
        assert!(!shared.logind_locked.load(Ordering::Acquire));
        shared.revoke_after.store(0, Ordering::SeqCst);
        drop(source);
        println!("restricted FD -> actual PipeWire pixels: 320x240");
    }
    session.close();
    drop(session);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    if std::env::var_os("DOGCODES_OWNED_PIPEWIRE_NODE").is_some() {
        wait_for(|| capture::worker_count() == 0).await;
        let authorization = Session::start(store.clone(), true).unwrap();
        ready(&authorization).await;
        let capturing = authorization.clone();
        let source = tokio::task::spawn_blocking(move || capturing.capture(1))
            .await
            .unwrap()
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let output = count.clone();
        let leaked = Arc::new(AtomicBool::new(false));
        let leak = leaked.clone();
        let scope = authorization.clone();
        let stream = tokio::task::spawn_blocking(move || {
            crate::video::VideoStream::start_from(
                crate::video::VideoConfig {
                    display: 1,
                    max_width: 320,
                    fps: 20,
                    bitrate: 800_000,
                    profile: crate::video::VideoProfile::ConstrainedBaseline31,
                },
                Arc::new(move |_| {
                    if scope.status().is_err() {
                        leak.store(true, Ordering::Release);
                    }
                    output.fetch_add(1, Ordering::SeqCst);
                }),
                Some(source),
            )
        })
        .await
        .unwrap()
        .unwrap();
        wait_for(|| count.load(Ordering::SeqCst) > 0).await;
        *shared.revoke.lock().unwrap() = Some(authorization.clone());
        shared.revoke_after.store(
            shared.logind_queries.load(Ordering::SeqCst) + 1,
            Ordering::SeqCst,
        );
        stream.request_keyframe();
        wait_for(|| authorization.status().is_err()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !leaked.load(Ordering::Acquire),
            "encoded pixels escaped a retired portal grant"
        );
        assert!(stream.error().is_some());
        shared.revoke_after.store(0, Ordering::SeqCst);
        drop(stream);
        drop(authorization);
        wait_for(|| ACTORS.load(Ordering::Acquire) == 0 && capture::worker_count() == 0).await;
    }
    if std::env::var("DOGCODES_OWNED_VIDEO_FORMAT").as_deref() == Ok("nv12-bt709-planes") {
        let root = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
        let force = root.join(".force-corrupt");
        let corrupt = root.join(".corrupt-written");
        let healthy = root.join(".healthy-written");
        let checking = Session::start(store.clone(), true).unwrap();
        ready(&checking).await;
        let capturing = checking.clone();
        let source = tokio::task::spawn_blocking(move || capturing.capture(1))
            .await
            .unwrap()
            .unwrap();
        std::fs::write(&force, b"owned-fixture").unwrap();
        wait_for(|| corrupt.exists()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        if healthy.exists() {
            std::fs::remove_file(&healthy).unwrap();
        }
        std::fs::remove_file(&force).unwrap();
        wait_for(|| healthy.exists()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let empty = root.join(".force-empty");
        std::fs::write(&empty, b"owned-fixture").unwrap();
        wait_for(|| root.join(".empty-written").exists()).await;
        // A good frame must cancel the first deadline rather than merely reset
        // the displayed picture. Empty metadata-only updates thereafter must
        // not start a new deadline on a legitimately static desktop.
        tokio::time::sleep(Duration::from_millis(3200)).await;
        assert!(source.check_active().is_ok());
        std::fs::remove_file(&empty).unwrap();
        std::fs::remove_file(&corrupt).unwrap();
        std::fs::write(&force, b"owned-fixture").unwrap();
        wait_for(|| corrupt.exists()).await;
        wait_for(|| source.check_active().is_err()).await;
        assert!(source.check_active().unwrap_err().contains("恢复超时"));
        assert!(checking.status().is_err());
        std::fs::remove_file(&force).unwrap();
        drop(source);
        drop(checking);
        wait_for(|| ACTORS.load(Ordering::Acquire) == 0 && capture::worker_count() == 0).await;

        // Chunk-level corruption on only the UV plane must have the same
        // bounded recovery as a bad Header, without reading either payload.
        let checking = Session::start(store.clone(), true).unwrap();
        ready(&checking).await;
        let capturing = checking.clone();
        let source = tokio::task::spawn_blocking(move || capturing.capture(1))
            .await
            .unwrap()
            .unwrap();
        let force = root.join(".force-corrupt-chunk");
        std::fs::write(&force, b"owned-fixture").unwrap();
        wait_for(|| root.join(".corrupt-chunk-written").exists()).await;
        wait_for(|| source.check_active().is_err()).await;
        assert!(source.check_active().unwrap_err().contains("恢复超时"));
        assert!(checking.status().is_err());
        std::fs::remove_file(&force).unwrap();
        drop(source);
        drop(checking);
        wait_for(|| ACTORS.load(Ordering::Acquire) == 0 && capture::worker_count() == 0).await;
    }
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
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    shared.stall_input.store(false, Ordering::Release);
    shared.eis_mode.store(1, Ordering::Release);
    let eis_store = Arc::new(TokenStore::new(directory.path().join("eis-portal")));
    let eis = Session::start(eis_store.clone(), true).unwrap();
    ready(&eis).await;
    let notify_before = shared.log.lock().unwrap().input.len();
    let sending = eis.clone();
    tokio::task::spawn_blocking(move || {
        sending.send(
            1,
            vec![
                Input::Move { x: 65535, y: 0 },
                Input::Text {
                    text: "完整链路🟢".into(),
                },
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
    eis.release_held();
    wait_for(|| {
        shared
            .eis
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .log
            .lock()
            .unwrap()
            .contains(&super::eis::tests::Record::Key(29, false))
    })
    .await;
    assert!(
        eis.status().unwrap(),
        "own modifier feedback retired a healthy grant"
    );
    {
        let fixture = shared.eis.lock().unwrap();
        let log = fixture.as_ref().unwrap().log.lock().unwrap();
        assert!(log.contains(&super::eis::tests::Record::Move(2119., 100.)));
        assert!(log.contains(&super::eis::tests::Record::Text("完整链路🟢".into())));
    }
    assert_eq!(
        shared.log.lock().unwrap().input.len(),
        notify_before,
        "EIS mixed with Notify"
    );
    assert!(
        shared
            .log
            .lock()
            .unwrap()
            .options
            .iter()
            .any(|(m, _, _)| m == "ConnectToEIS")
    );
    let starts = shared.log.lock().unwrap().sessions.len();
    shared
        .eis
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .changes
        .send(super::eis::tests::Change::Disconnect)
        .unwrap();
    wait_for(|| eis.status().is_err()).await;
    drop(eis);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    assert_eq!(shared.log.lock().unwrap().sessions.len(), starts);
    shared.eis.lock().unwrap().take();
    shared.eis_mode.store(2, Ordering::Release);
    let broken = Session::start(eis_store.clone(), true).unwrap();
    wait_for(|| broken.status().is_err()).await;
    assert_eq!(shared.log.lock().unwrap().input.len(), notify_before);
    drop(broken);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    clipboard_input_fixture(&shared, &service, eis_store).await;
}

async fn send_text(session: &Session, text: &str) -> Result<()> {
    let session = session.clone();
    let text = text.to_string();
    tokio::task::spawn_blocking(move || session.send(1, vec![Input::Text { text }]))
        .await
        .unwrap()
}
async fn clipboard_session(
    shared: &Arc<Fixture>,
    service: &Connection,
    store: Arc<TokenStore>,
    deliver: bool,
) -> Session {
    let session = Session::start(store, true).unwrap();
    ready(&session).await;
    if deliver {
        let runtime = tokio::runtime::Handle::current();
        let clip = shared.clipboard.clone();
        let service = service.clone();
        *shared
            .eis
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .on_paste
            .lock()
            .unwrap() = Some(Arc::new(move || {
            let clip = clip.clone();
            let service = service.clone();
            runtime.spawn(async move {
                let _ = clip.request(&service, true).await;
            });
        }));
    }
    session
}
async fn clipboard_input_fixture(
    shared: &Arc<Fixture>,
    service: &Connection,
    store: Arc<TokenStore>,
) {
    focus::register(service, shared.focus.clone()).await;
    let clip = &shared.clipboard;
    // A granted capability never reads/writes clipboard for native EI text.
    clip.reset();
    shared.eis_mode.store(1, Ordering::Release);
    let native = clipboard_session(shared, service, store.clone(), true).await;
    send_text(&native, "native中🟢").await.unwrap();
    assert_eq!(clip.sets.load(Ordering::Acquire), 0);
    assert_eq!(shared.focus.queries.load(Ordering::Acquire), 0);
    assert_eq!(*clip.value.lock().unwrap(), b"private original clipboard");
    drop(native);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // No ei_text: actual EI Ctrl+V triggers FD data transfer. Prefetch alone
    // must not complete an input; the fake focused client requests after keys.
    clip.reset();
    clip.prefetch.store(true, Ordering::Release);
    shared.eis_mode.store(3, Ordering::Release);
    let session = clipboard_session(shared, service, store.clone(), true).await;
    send_text(&session, "粘贴🟢").await.unwrap();
    assert_eq!(
        *clip.pasted.lock().unwrap(),
        vec!["粘贴🟢".as_bytes().to_vec()]
    );
    assert_eq!(clip.sets.load(Ordering::Acquire), 1);
    assert_eq!(
        clip.request(service, false).await.unwrap(),
        "粘贴🟢".as_bytes()
    );
    send_text(&session, "下一段中文").await.unwrap();
    assert_eq!(
        *clip.pasted.lock().unwrap(),
        vec![
            "粘贴🟢".as_bytes().to_vec(),
            "下一段中文".as_bytes().to_vec()
        ]
    );
    assert_eq!(
        clip.sets.load(Ordering::Acquire),
        2,
        "unexpected restoration write"
    );
    assert_eq!(
        clip.request(service, false).await.unwrap(),
        "下一段中文".as_bytes(),
        "paste guard restored the old clipboard"
    );
    clip.copy_new(service, b"newer clipboard").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(*clip.value.lock().unwrap(), b"newer clipboard");
    assert_eq!(clip.sets.load(Ordering::Acquire), 2);
    assert!(session.status().unwrap());
    drop(session);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // Denial affects only the unsupported input, not the viewing grant.
    clip.reset();
    clip.granted.store(false, Ordering::Release);
    let denied = clipboard_session(shared, service, store.clone(), true).await;
    assert!(send_text(&denied, "中文").await.is_err());
    assert_eq!(clip.sets.load(Ordering::Acquire), 0);
    assert!(denied.status().unwrap());
    drop(denied);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // Prefetch before arming is not evidence of data requested after paste.
    clip.reset();
    clip.prefetch.store(true, Ordering::Release);
    let no_consumer = clipboard_session(shared, service, store.clone(), false).await;
    assert!(send_text(&no_consumer, "未读取中文").await.is_err());
    assert!(clip.pasted.lock().unwrap().is_empty());
    assert_eq!(clip.sets.load(Ordering::Acquire), 1);
    assert_eq!(
        *clip.value.lock().unwrap(),
        "未读取中文".as_bytes(),
        "timeout restored clipboard"
    );
    drop(no_consumer);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // External copying while delivery is acknowledged is preserved, including
    // when the caller cannot establish whether the target application used it.
    clip.reset();
    clip.replace_on_delivery.store(true, Ordering::Release);
    let replaced = clipboard_session(shared, service, store.clone(), true).await;
    assert!(send_text(&replaced, "被替换中文").await.is_err());
    assert_eq!(clip.pasted.lock().unwrap().len(), 1);
    assert_eq!(clip.sets.load(Ordering::Acquire), 1);
    assert_eq!(*clip.value.lock().unwrap(), b"newer user clipboard");
    drop(replaced);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // A terminal and normal entry expose different paste bindings. Reusing
    // one session and object path must not keep the old terminal classification.
    clip.reset();
    shared.focus.mode.store(1, Ordering::Release);
    let adaptive = clipboard_session(shared, service, store.clone(), true).await;
    let expected_shift = shared
        .eis
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .paste_shift
        .clone();
    expected_shift.store(true, Ordering::Release);
    send_text(&adaptive, "终端粘贴🟢").await.unwrap();
    shared.focus.mode.store(3, Ordering::Release);
    expected_shift.store(false, Ordering::Release);
    send_text(&adaptive, "忽略失焦终端🟢").await.unwrap();
    shared.focus.mode.store(1, Ordering::Release);
    expected_shift.store(true, Ordering::Release);
    send_text(&adaptive, "再次识别终端🟢").await.unwrap();
    shared.focus.mode.store(2, Ordering::Release);
    expected_shift.store(false, Ordering::Release);
    send_text(&adaptive, "普通输入框🟢").await.unwrap();
    assert_eq!(clip.pasted.lock().unwrap().len(), 4);
    assert_eq!(clip.sets.load(Ordering::Acquire), 4);
    assert_eq!(
        clip.request(service, false).await.unwrap(),
        "普通输入框🟢".as_bytes()
    );
    drop(adaptive);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // Focus disappears after claiming the clipboard: never emit either paste.
    clip.reset();
    shared.focus.mode.store(1, Ordering::Release);
    let changed = shared.focus.clone();
    *clip.on_set.lock().unwrap() = Some(Arc::new(move || {
        changed.mode.store(4, Ordering::Release);
    }));
    let changed = clipboard_session(shared, service, store.clone(), true).await;
    shared
        .eis
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .paste_shift
        .store(true, Ordering::Release);
    let error = send_text(&changed, "焦点切换🟢").await.unwrap_err();
    assert!(error.contains("焦点已变化"), "{error}");
    assert!(clip.pasted.lock().unwrap().is_empty());
    assert!(
        !shared
            .eis
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|r| matches!(r, super::eis::tests::Record::Key(..)))
    );
    assert_eq!(clip.sets.load(Ordering::Acquire), 1);
    drop(changed);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    // Optional metadata may stall. One bounded attempt still permits the
    // normal shortcut, with no second shortcut or duplicated paste.
    clip.reset();
    shared.focus.mode.store(5, Ordering::Release);
    let stalled = clipboard_session(shared, service, store, true).await;
    send_text(&stalled, "元数据超时🟢").await.unwrap();
    assert_eq!(clip.pasted.lock().unwrap().len(), 1);
    shared.focus.mode.store(1, Ordering::Release);
    shared
        .eis
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .paste_shift
        .store(true, Ordering::Release);
    send_text(&stalled, "恢复终端识别🟢").await.unwrap();
    assert_eq!(clip.pasted.lock().unwrap().len(), 2);
    drop(stalled);
    wait_for(|| ACTORS.load(Ordering::Acquire) == 0).await;
    assert_eq!(
        clip.reads.load(Ordering::Acquire),
        0,
        "original clipboard was read or backed up"
    );
}

fn check_corners(rgb: &[u8], width: usize, height: usize) {
    for (x, y, expected) in [
        (10, 10, [255u8, 0, 0]),
        (width - 11, 10, [0, 255, 0]),
        (10, height - 11, [0, 0, 255]),
        (width - 11, height - 11, [255, 255, 255]),
    ] {
        let pixel = &rgb[(y * width + x) * 3..(y * width + x) * 3 + 3];
        assert!(
            pixel
                .iter()
                .zip(expected)
                .all(|(actual, want)| actual.abs_diff(want) < 32),
            "oriented corner ({x},{y}): {pixel:?}, expected {expected:?}"
        );
    }
}
