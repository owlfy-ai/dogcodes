//! Owned Clipboard portal and fake focused client. Never reads OS clipboard.
use std::{
    collections::HashMap,
    io::Read,
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use zbus::{
    Connection,
    zvariant::{DeserializeDict, OwnedObjectPath, Type, Value},
};
const IFACE: &str = "org.freedesktop.portal.Clipboard";
const MIME: &str = "text/plain;charset=utf-8";
type Reply = std::result::Result<Vec<u8>, String>;
type Hook = Mutex<Option<Arc<dyn Fn() + Send + Sync>>>;
struct Pending {
    writer: Option<OwnedFd>,
    reader: UnixStream,
    reply: tokio::sync::oneshot::Sender<Reply>,
    paste: bool,
    epoch: usize,
}
#[derive(Default)]
pub(super) struct Fixture {
    pub enabled: AtomicBool,
    pub granted: AtomicBool,
    pub prefetch: AtomicBool,
    pub replace_on_delivery: AtomicBool,
    pub sets: AtomicUsize,
    pub reads: AtomicUsize,
    pub on_set: Hook,
    pub pasted: Mutex<Vec<Vec<u8>>>,
    pub value: Mutex<Vec<u8>>,
    session: Mutex<Option<OwnedObjectPath>>,
    ours: AtomicBool,
    epoch: AtomicUsize,
    serial: AtomicUsize,
    pending: Mutex<HashMap<u32, Pending>>,
}
impl Fixture {
    pub fn reset(&self) {
        self.enabled.store(true, Ordering::Release);
        self.granted.store(true, Ordering::Release);
        self.prefetch.store(false, Ordering::Release);
        self.replace_on_delivery.store(false, Ordering::Release);
        self.ours.store(false, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.sets.store(0, Ordering::Release);
        self.pasted.lock().unwrap().clear();
        *self.on_set.lock().unwrap() = None;
        *self.value.lock().unwrap() = b"private original clipboard".to_vec();
    }
    pub fn granted(&self) -> bool {
        self.enabled.load(Ordering::Acquire) && self.granted.load(Ordering::Acquire)
    }
    pub async fn request(&self, connection: &Connection, paste: bool) -> Reply {
        if !self.ours.load(Ordering::Acquire) {
            return Ok(self.value.lock().unwrap().clone());
        }
        let path = self.session.lock().unwrap().clone().unwrap();
        let (writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let serial = self.serial.fetch_add(1, Ordering::AcqRel) as u32 + 1;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(
            serial,
            Pending {
                writer: Some(writer.into()),
                reader,
                reply: tx,
                paste,
                epoch: self.epoch.load(Ordering::Acquire),
            },
        );
        connection
            .emit_signal(
                None::<&str>,
                super::ROOT,
                IFACE,
                "SelectionTransfer",
                &(&path, MIME, serial),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| "fixture timeout")?
            .map_err(|_| "fixture lost reply")?
    }
    pub async fn copy_new(&self, connection: &Connection, bytes: &[u8]) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.ours.store(false, Ordering::Release);
        *self.value.lock().unwrap() = bytes.to_vec();
        self.owner(connection, vec![MIME.to_string()], false).await;
    }
    async fn owner(&self, connection: &Connection, formats: Vec<String>, ours: bool) {
        let path = self.session.lock().unwrap().clone().unwrap();
        let opts = HashMap::from([
            ("mime_types", Value::from(formats)),
            ("session_is_owner", Value::from(ours)),
        ]);
        connection
            .emit_signal(
                None::<&str>,
                super::ROOT,
                IFACE,
                "SelectionOwnerChanged",
                &(&path, opts),
            )
            .await
            .unwrap();
    }
}
pub(super) struct Mock(pub Arc<Fixture>);
#[derive(DeserializeDict, Type)]
#[zvariant(signature = "dict")]
pub(super) struct Formats {
    mime_types: Vec<String>,
}
#[zbus::interface(name = "org.freedesktop.portal.Clipboard")]
impl Mock {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
    fn request_clipboard(
        &self,
        session: OwnedObjectPath,
        _options: HashMap<String, zbus::zvariant::OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        if !self.0.enabled.load(Ordering::Acquire) {
            return Err(zbus::fdo::Error::NotSupported("fixture disabled".into()));
        }
        *self.0.session.lock().unwrap() = Some(session);
        Ok(())
    }
    async fn set_selection(
        &self,
        session: OwnedObjectPath,
        options: Formats,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<()> {
        assert!(self.0.granted());
        assert_eq!(self.0.session.lock().unwrap().as_ref(), Some(&session));
        self.0.sets.fetch_add(1, Ordering::AcqRel);
        self.0.epoch.fetch_add(1, Ordering::AcqRel);
        self.0.ours.store(true, Ordering::Release);
        self.0.owner(connection, options.mime_types, true).await;
        if let Some(hook) = self.0.on_set.lock().unwrap().clone() {
            hook();
        }
        if self.0.prefetch.load(Ordering::Acquire) {
            self.0.request(connection, false).await.unwrap();
        }
        Ok(())
    }
    fn selection_read(
        &self,
        _session: OwnedObjectPath,
        _mime: String,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        self.0.reads.fetch_add(1, Ordering::AcqRel);
        Err(zbus::fdo::Error::AccessDenied(
            "original clipboard must never be read".into(),
        ))
    }
    fn selection_write(
        &self,
        session: OwnedObjectPath,
        serial: u32,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        assert_eq!(self.0.session.lock().unwrap().as_ref(), Some(&session));
        self.0
            .pending
            .lock()
            .unwrap()
            .get_mut(&serial)
            .and_then(|p| p.writer.take())
            .map(Into::into)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("stale transfer".into()))
    }
    async fn selection_write_done(
        &self,
        _session: OwnedObjectPath,
        serial: u32,
        success: bool,
        #[zbus(connection)] connection: &Connection,
    ) {
        let pending = self.0.pending.lock().unwrap().remove(&serial).unwrap();
        let result = if success {
            let reader = pending.reader;
            tokio::task::spawn_blocking(move || {
                let mut bytes = Vec::new();
                reader
                    .take(64 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 64 * 1024 {
                    return Err("fixture size exceeded".into());
                }
                Ok(bytes)
            })
            .await
            .unwrap()
        } else {
            Err("transfer rejected".into())
        };
        if let Ok(bytes) = &result {
            if self.0.ours.load(Ordering::Acquire)
                && self.0.epoch.load(Ordering::Acquire) == pending.epoch
            {
                *self.0.value.lock().unwrap() = bytes.clone();
            }
            if pending.paste {
                self.0.pasted.lock().unwrap().push(bytes.clone());
                if self.0.replace_on_delivery.load(Ordering::Acquire) {
                    self.0.copy_new(connection, b"newer user clipboard").await;
                }
            }
        }
        let _ = pending.reply.send(result);
    }
}
