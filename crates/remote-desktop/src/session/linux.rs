//! Read login1 directly; no per-frame child processes or cached unlocked grant.
use super::query::Probe;
use crate::Result;
use std::{collections::HashMap, sync::OnceLock, time::Duration};
use zbus::{
    Connection,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const DEST: &str = "org.freedesktop.login1";
const ROOT: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const USER: &str = "org.freedesktop.login1.User";
const BUDGET: Duration = Duration::from_millis(250);

pub fn locked() -> Result<bool> {
    static PROBE: OnceLock<Result<Probe>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            // Construct the runtime lazily on the worker, so sync callers inside
            // an existing Tokio runtime cannot enter another runtime recursively.
            let mut native = None;
            Probe::start(
                move || {
                    if native.is_none() {
                        native = Some(Native::new()?);
                    }
                    native.as_mut().unwrap().read()
                },
                BUDGET,
            )
        })
        .as_ref()
        .map_err(Clone::clone)?
        .read()
}

struct Native {
    connection: Option<Connection>,
    runtime: tokio::runtime::Runtime,
}
impl Native {
    fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "backendText.m65c2f267")?;
        Ok(Self {
            runtime,
            connection: None,
        })
    }
    fn read(&mut self) -> Result<bool> {
        let result = self.runtime.block_on(async {
            tokio::time::timeout(BUDGET, async {
                if self.connection.is_none() {
                    self.connection = Some(
                        Connection::system()
                            .await
                            .map_err(|_| "backendText.mb922de66")?,
                    );
                }
                // SAFETY: geteuid takes no pointers and returns this process UID.
                let uid = unsafe { libc::geteuid() };
                query(
                    self.connection.as_ref().unwrap(),
                    std::env::var("XDG_SESSION_ID").ok().as_deref(),
                    std::process::id(),
                    uid,
                )
                .await
            })
            .await
            .map_err(|_| "backendText.m2ea94fde")?
        });
        if result.is_err() {
            self.connection = None;
        }
        result
    }
}
async fn properties(
    connection: &Connection,
    path: &str,
    interface: &str,
) -> Result<HashMap<String, OwnedValue>> {
    connection
        .call_method(
            Some(DEST),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &(interface,),
        )
        .await
        .map_err(|_| "backendText.ma13a97a7")?
        .body()
        .deserialize()
        .map_err(|_| "backendText.m45b42acb".into())
}
async fn object<B>(connection: &Connection, method: &str, body: &B) -> Result<OwnedObjectPath>
where
    B: serde::ser::Serialize + zbus::zvariant::DynamicType,
{
    connection
        .call_method(Some(DEST), ROOT, Some(MANAGER), method, body)
        .await
        .map_err(|_| "backendText.mbe750ef1")?
        .body()
        .deserialize()
        .map_err(|_| "backendText.m1bbcb4cd".into())
}
pub(super) async fn query(
    connection: &Connection,
    session: Option<&str>,
    pid: u32,
    uid: u32,
) -> Result<bool> {
    let path = if let Some(session) = session.filter(|s| !s.is_empty()) {
        if session.len() > 256 {
            return Err("backendText.m1bbcb4cd".into());
        }
        // An explicit but expired session never falls back to another desktop.
        object(connection, "GetSession", &(session,)).await?
    } else {
        match object(connection, "GetSessionByPID", &(pid,)).await {
            Ok(path) => path,
            Err(_) => {
                // A user-service Core need not belong to a login session. Only
                // its own UID's display session may be selected, then validated.
                let user = object(connection, "GetUser", &(uid,)).await?;
                let mut props = properties(connection, user.as_str(), USER).await?;
                let (id, path): (String, OwnedObjectPath) = props
                    .remove("Display")
                    .ok_or("backendText.md338a450")?
                    .try_into()
                    .map_err(|_| "backendText.m1bbcb4cd")?;
                if id.is_empty() || path.as_str() == "/" {
                    return Err("backendText.md338a450".into());
                }
                path
            }
        }
    };
    status(properties(connection, path.as_str(), SESSION).await?, uid)
}
fn status(mut props: HashMap<String, OwnedValue>, uid: u32) -> Result<bool> {
    let locked = props
        .remove("LockedHint")
        .and_then(|v| bool::try_from(v).ok())
        .ok_or("backendText.ma77f32d6")?;
    let active = props
        .remove("Active")
        .and_then(|v| bool::try_from(v).ok())
        .ok_or("backendText.m253e97f6")?;
    let remote = props
        .remove("Remote")
        .and_then(|v| bool::try_from(v).ok())
        .ok_or("backendText.m26af25f9")?;
    let kind = props
        .remove("Type")
        .and_then(|v| String::try_from(v).ok())
        .ok_or("backendText.mb3d01d7d")?;
    let (owner, _): (u32, OwnedObjectPath) = props
        .remove("User")
        .ok_or("backendText.m2466afa3")?
        .try_into()
        .map_err(|_| "backendText.m2466afa3")?;
    if owner != uid || !active || remote || !matches!(kind.as_str(), "x11" | "wayland") {
        return Err("backendText.mc55b5de6".into());
    }
    Ok(locked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;
    fn props(
        locked: bool,
        active: bool,
        remote: bool,
        kind: &str,
        uid: u32,
    ) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("LockedHint".into(), OwnedValue::from(locked)),
            ("Active".into(), OwnedValue::from(active)),
            ("Remote".into(), OwnedValue::from(remote)),
            (
                "Type".into(),
                OwnedValue::try_from(Value::from(kind)).unwrap(),
            ),
            (
                "User".into(),
                OwnedValue::try_from(Value::from((
                    uid,
                    OwnedObjectPath::try_from("/user/test").unwrap(),
                )))
                .unwrap(),
            ),
        ])
    }
    #[test]
    fn only_this_users_active_local_graphical_session_can_report_unlocked() {
        assert!(!status(props(false, true, false, "wayland", 42), 42).unwrap());
        assert!(status(props(true, true, false, "x11", 42), 42).unwrap());
        for input in [
            props(false, false, false, "wayland", 42),
            props(false, true, true, "wayland", 42),
            props(false, true, false, "tty", 42),
            props(false, true, false, "wayland", 1),
        ] {
            assert!(status(input, 42).is_err());
        }
    }
    #[test]
    fn missing_or_wrongly_typed_login_properties_never_mean_unlocked() {
        for field in ["LockedHint", "Active", "Remote", "Type", "User"] {
            let mut incomplete = props(false, true, false, "wayland", 42);
            incomplete.remove(field);
            assert!(status(incomplete, 42).is_err());
            let mut wrong = props(false, true, false, "wayland", 42);
            wrong.insert(field.into(), OwnedValue::from(17u32));
            assert!(status(wrong, 42).is_err());
        }
    }
}
