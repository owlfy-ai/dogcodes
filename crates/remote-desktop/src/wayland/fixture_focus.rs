//! Read-only accessibility tree on the launcher's private D-Bus, never a desktop.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zbus::{Connection, zvariant::OwnedObjectPath};
pub(super) const ROOT: &str = "/org/a11y/atspi/accessible/root";
const WINDOW: &str = "/org/a11y/atspi/accessible/window";
const WIDGET: &str = "/org/a11y/atspi/accessible/widget";
#[derive(Default)]
pub(super) struct Fixture {
    // 0 unavailable, 1 focused terminal, 2 focused entry, 3 inactive window,
    // 4 missing focus, 5 stalled metadata.
    pub mode: AtomicUsize,
    pub queries: AtomicUsize,
}
pub(super) struct Bus(pub Arc<Fixture>);
#[zbus::interface(name = "org.a11y.Bus")]
impl Bus {
    fn get_address(&self) -> zbus::fdo::Result<String> {
        self.0.queries.fetch_add(1, Ordering::AcqRel);
        if self.0.mode.load(Ordering::Acquire) == 0 {
            return Err(zbus::fdo::Error::NotSupported(
                "owned fixture disabled".into(),
            ));
        }
        Ok(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap())
    }
}
pub(super) struct Accessible(pub Arc<Fixture>, pub u8);
#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_state(&self) -> Vec<u32> {
        self.0.queries.fetch_add(1, Ordering::AcqRel);
        let mode = self.0.mode.load(Ordering::Acquire);
        if mode == 5 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        vec![
            match self.1 {
                1 if mode != 3 => 1 << 1,
                2 if mode != 4 => 1 << 12,
                _ => 0,
            },
            0,
        ]
    }
    fn get_role(&self) -> u32 {
        match self.1 {
            0 => 14,
            1 => 23,
            _ if self.0.mode.load(Ordering::Acquire) == 2 => 79,
            _ => 60,
        }
    }
    fn get_children(
        &self,
        #[zbus(connection)] connection: &Connection,
    ) -> Vec<(String, OwnedObjectPath)> {
        let path = match self.1 {
            0 => WINDOW,
            1 => WIDGET,
            _ => return vec![],
        };
        vec![(
            connection.unique_name().unwrap().to_string(),
            OwnedObjectPath::try_from(path).unwrap(),
        )]
    }
}
pub(super) async fn register(connection: &Connection, shared: Arc<Fixture>) {
    connection.request_name("org.a11y.Bus").await.unwrap();
    connection
        .request_name("org.a11y.atspi.Registry")
        .await
        .unwrap();
    connection
        .object_server()
        .at("/org/a11y/bus", Bus(shared.clone()))
        .await
        .unwrap();
    for (path, kind) in [(ROOT, 0), (WINDOW, 1), (WIDGET, 2)] {
        connection
            .object_server()
            .at(path, Accessible(shared.clone(), kind))
            .await
            .unwrap();
    }
}
