//! Optional read-only AT-SPI metadata for paste selection. Never enables
//! accessibility, reads widget text/titles, guesses by executable, or retries
//! a different shortcut after an uncertain effect.
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const ACTIVE: u32 = 1 << 1;
const FOCUSED: u32 = 1 << 12;
const DEFUNCT: u32 = 1 << 6;
const TERMINAL: u32 = 60;
type Object = (String, OwnedObjectPath);

#[derive(Default)]
pub(super) struct Probe {
    retry_at: Option<Instant>,
    connection: Option<Connection>,
    cached: Option<Focus>,
}
#[derive(Clone)]
pub(super) struct Focus {
    connection: Connection,
    object: Object,
    window: Object,
    role: u32,
}
impl Focus {
    pub(super) fn terminal(&self) -> bool {
        self.role == TERMINAL
    }
    pub(super) async fn current(&self) -> bool {
        tokio::time::timeout(Duration::from_millis(100), async {
            let object = object_proxy(&self.connection, &self.object).await?;
            let flags = states(&object).await?;
            let role: u32 = object.call("GetRole", &()).await.ok()?;
            let window = object_proxy(&self.connection, &self.window).await?;
            let active = states(&window).await?;
            Some(
                role == self.role
                    && flags & FOCUSED != 0
                    && flags & DEFUNCT == 0
                    && active & ACTIVE != 0
                    && active & DEFUNCT == 0,
            )
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    }
}
impl Probe {
    pub(super) async fn resolve(&mut self) -> Option<Focus> {
        tokio::time::timeout(Duration::from_millis(200), self.lookup())
            .await
            .ok()
            .flatten()
    }
    async fn lookup(&mut self) -> Option<Focus> {
        if self.connection.as_ref().is_some_and(Connection::is_closed) {
            self.connection = None;
            self.cached = None;
        }
        if self.connection.is_none() {
            if self.retry_at.is_some_and(|when| Instant::now() < when) {
                return None;
            }
            self.retry_at = Some(Instant::now() + Duration::from_secs(5));
            let session = Connection::session().await.ok()?;
            // GetNameOwner does not activate accessibility. Pin each peer to
            // its unique connection; a replacement cannot inherit its cache.
            let bus = zbus::fdo::DBusProxy::new(&session).await.ok()?;
            let owner = bus
                .get_name_owner("org.a11y.Bus".try_into().ok()?)
                .await
                .ok()?;
            let proxy = Proxy::new(&session, owner, "/org/a11y/bus", "org.a11y.Bus")
                .await
                .ok()?;
            let address: String = proxy.call("GetAddress", &()).await.ok()?;
            if address.len() > 4096 || !address.starts_with("unix:") {
                return None;
            }
            self.connection = zbus::connection::Builder::address(address.as_str())
                .ok()?
                .build()
                .await
                .ok();
        }
        let connection = self.connection.as_ref()?;
        if let Some(cached) = &self.cached
            && cached.current().await
        {
            return Some(cached.clone());
        }
        self.cached = None;
        let bus = zbus::fdo::DBusProxy::new(connection).await.ok()?;
        let registry = bus
            .get_name_owner("org.a11y.atspi.Registry".try_into().ok()?)
            .await
            .ok()?;
        let root = (registry.to_string(), OwnedObjectPath::try_from(ROOT).ok()?);
        let mut queue = VecDeque::from([(root, 0usize, None::<Object>)]);
        let mut visited = HashSet::new();
        while let Some((reference, depth, mut window)) = queue.pop_front() {
            if visited.len() >= 128 {
                return None;
            }
            if !visited.insert(reference.clone()) {
                continue;
            }
            let Some(proxy) = object_proxy(connection, &reference).await else {
                continue;
            };
            let Some(flags) = states(&proxy).await else {
                continue;
            };
            if flags & DEFUNCT != 0 {
                continue;
            }
            let role: u32 = match proxy.call("GetRole", &()).await {
                Ok(role) => role,
                Err(_) => continue,
            };
            if matches!(role, 16 | 23 | 69) && flags & ACTIVE == 0 {
                continue;
            }
            if flags & (ACTIVE | FOCUSED) != 0 {
                if flags & ACTIVE != 0 && matches!(role, 16 | 23 | 69) {
                    window = Some(reference.clone());
                }
                if flags & FOCUSED != 0
                    && reference.0.starts_with(':')
                    && let Some(window) = window
                {
                    let focused = Focus {
                        connection: connection.clone(),
                        object: reference,
                        window,
                        role,
                    };
                    if focused.current().await {
                        self.cached = Some(focused.clone());
                        return Some(focused);
                    }
                    return None;
                }
            }
            if depth >= 12 {
                continue;
            }
            let children: Vec<Object> = match proxy.call("GetChildren", &()).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if children.len() > 128 || queue.len() + children.len() > 128 {
                return None;
            }
            queue.extend(
                children
                    .into_iter()
                    .filter(|(bus, path)| bus.len() <= 255 && path.as_str().len() <= 1024)
                    .map(|child| (child, depth + 1, window.clone())),
            );
        }
        None
    }
}
async fn object_proxy(connection: &Connection, reference: &Object) -> Option<Proxy<'static>> {
    Proxy::new(
        connection,
        reference.0.clone(),
        reference.1.clone(),
        ACCESSIBLE,
    )
    .await
    .ok()
}
async fn states(proxy: &Proxy<'_>) -> Option<u32> {
    let states: Vec<u32> = proxy.call("GetState", &()).await.ok()?;
    (states.len() == 2).then(|| states[0])
}
