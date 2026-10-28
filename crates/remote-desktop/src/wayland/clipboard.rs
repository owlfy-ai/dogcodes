//! Clipboard capability on the original RemoteDesktop grant. No user data is
//! read or backed up. Explicit paste input replaces it and never restores it.
mod io;
use super::State;
use crate::Result;
use futures::StreamExt;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::Notify,
    task::{JoinHandle, JoinSet},
};
use zbus::{
    Connection, Proxy,
    zvariant::{DeserializeDict, OwnedObjectPath, Type, Value},
};

const INTERFACE: &str = "org.freedesktop.portal.Clipboard";
const TEXT: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

#[derive(DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct OwnerReply {
    mime_types: Option<Vec<String>>,
    session_is_owner: Option<bool>,
}
#[derive(Clone)]
struct Owner {
    formats: Vec<String>,
    ours: bool,
}
enum Source {
    Temporary {
        id: String,
        text: Arc<Vec<u8>>,
        in_use: bool,
        claimed: bool,
        armed: bool,
        delivered: Option<Result<()>>,
    },
}
#[derive(Default)]
struct Data {
    owner: Option<Owner>,
    source: Option<Source>,
    error: Option<String>,
}
struct Shared {
    proxy: Proxy<'static>,
    path: OwnedObjectPath,
    state: Arc<State>,
    data: Mutex<Data>,
    changed: Notify,
}
pub(super) struct Clipboard {
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}
pub(super) struct Temporary {
    shared: Arc<Shared>,
    id: String,
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let mut data = self.shared.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Source::Temporary { id, in_use, .. }) = &mut data.source
            && id == &self.id
        {
            // End the transaction, preserving the offered text. No clipboard
            // read, backup, content switch, SetSelection or delayed restoration.
            *in_use = false;
        }
    }
}
impl Clipboard {
    pub(super) async fn request(
        connection: &Connection,
        path: &OwnedObjectPath,
        state: Arc<State>,
    ) -> Option<Self> {
        let proxy = Proxy::new(
            connection,
            super::portal::DEST,
            super::portal::ROOT,
            INTERFACE,
        )
        .await
        .ok()?;
        let version: u32 =
            tokio::time::timeout(Duration::from_secs(1), proxy.get_property("version"))
                .await
                .ok()?
                .ok()?;
        if version < 1 {
            return None;
        }
        let mut events = proxy.receive_all_signals().await.ok()?;
        tokio::time::timeout(
            Duration::from_secs(1),
            proxy.call::<_, _, ()>(
                "RequestClipboard",
                &(path, BTreeMap::<&str, Value<'_>>::new()),
            ),
        )
        .await
        .ok()?
        .ok()?;
        let shared = Arc::new(Shared {
            proxy,
            path: path.clone(),
            state,
            data: Mutex::new(Data::default()),
            changed: Notify::new(),
        });
        let worker = shared.clone();
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            let mut serials = HashSet::new();
            let mut active = HashSet::new();
            let mut recent = VecDeque::new();
            loop {
                tokio::select! {
                    biased;
                    event = events.next() => {
                        let Some(event) = event else { break; };
                        // Preserve ordering across owner and transfer messages.
                        let member = event.header().member().map(ToString::to_string);
                        if member.as_deref() == Some("SelectionOwnerChanged") {
                            let Ok((path, reply)) = event.body().deserialize::<(OwnedObjectPath, OwnerReply)>() else { break; };
                            if path != worker.path { continue; }
                            let formats = reply.mime_types.unwrap_or_default();
                            if formats.len() > 64 || formats.iter().any(|s| s.is_empty() || s.len() > 255 || s.chars().any(char::is_control)) { break; }
                            let mut data = worker.data.lock().unwrap_or_else(|e| e.into_inner());
                            let ours = reply.session_is_owner.unwrap_or(false);
                            if let Some(Source::Temporary { id, claimed, .. }) = &mut data.source {
                                if ours && formats.contains(id) { *claimed = true; }
                                else if !ours && *claimed { data.source = None; }
                            } else if !ours { data.source = None; }
                            data.owner = Some(Owner { formats, ours });
                            worker.changed.notify_waiters();
                            continue;
                        }
                        if member.as_deref() != Some("SelectionTransfer") { continue; }
                        let Ok((path, mime, serial)) = event.body().deserialize::<(OwnedObjectPath, String, u32)>() else { break; };
                        if path != worker.path || active.contains(&serial) || !serials.insert(serial) { continue; }
                        recent.push_back(serial);
                        if recent.len() > 1024 && let Some(old) = recent.pop_front() { serials.remove(&old); }
                        if jobs.len() >= 4 {
                            let _ = tokio::time::timeout(Duration::from_millis(100), worker.done(serial, false)).await;
                            continue;
                        }
                        let data = worker.transfer(&mime);
                        let owner = worker.clone();
                        active.insert(serial);
                        jobs.spawn(async move { owner.write(serial, data).await; serial });
                    },
                    result = jobs.join_next(), if !jobs.is_empty() => {
                        match result { Some(Ok(serial)) => { active.remove(&serial); }, _ => break }
                    },
                }
            }
            worker.data.lock().unwrap_or_else(|e| e.into_inner()).error =
                Some("剪贴板连接已结束".into());
            worker.changed.notify_waiters();
        });
        Some(Self { shared, task })
    }
    pub(super) async fn begin(&self, text: String) -> Result<Temporary> {
        if text.is_empty() || text.len() > 64 * 1024 || text.contains('\0') {
            return Err("临时粘贴文字无效或过长".into());
        }
        let shared = &self.shared;
        if !shared.state.active() {
            return Err("远控已结束，未执行粘贴".into());
        }
        {
            let data = shared.data.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(error) = &data.error {
                return Err(error.clone());
            }
            if matches!(&data.source, Some(Source::Temporary { in_use: true, .. })) {
                return Err("上一项粘贴尚未结束".into());
            }
        }
        let id = format!(
            "application/x-dogcodes-paste-{}",
            uuid::Uuid::new_v4().simple()
        );
        let temporary = Temporary {
            shared: shared.clone(),
            id: id.clone(),
        };
        let mut formats: Vec<String> = TEXT.iter().map(|s| (*s).to_owned()).collect();
        formats.push(id.clone());
        {
            let mut data = shared.data.lock().unwrap_or_else(|e| e.into_inner());
            data.source = Some(Source::Temporary {
                id,
                text: Arc::new(text.into_bytes()),
                in_use: true,
                claimed: false,
                armed: false,
                delivered: None,
            });
        }
        let opts = BTreeMap::from([("mime_types", Value::from(formats))]);
        shared
            .proxy
            .call::<_, _, ()>("SetSelection", &(&shared.path, opts))
            .await
            .map_err(|_| "临时剪贴板设置结果未知，禁止重放输入")?;
        temporary.wait_owned().await?;
        Ok(temporary)
    }
}
impl Temporary {
    async fn wait_owned(&self) -> Result<()> {
        loop {
            let notified = self.shared.changed.notified();
            {
                let data = self.shared.data.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(error) = &data.error {
                    return Err(error.clone());
                }
                if data
                    .owner
                    .as_ref()
                    .is_some_and(|o| o.ours && o.formats.contains(&self.id))
                {
                    return Ok(());
                }
                if !matches!(&data.source, Some(Source::Temporary { id, .. }) if id == &self.id) {
                    return Err("剪贴板已被其他操作替换，未执行粘贴".into());
                }
            }
            notified.await;
        }
    }
    pub(super) fn arm(&self) -> Result<()> {
        let mut data = self.shared.data.lock().unwrap_or_else(|e| e.into_inner());
        if !data
            .owner
            .as_ref()
            .is_some_and(|o| o.ours && o.formats.contains(&self.id))
        {
            return Err("剪贴板已变化，未执行粘贴".into());
        }
        match &mut data.source {
            Some(Source::Temporary { id, armed, .. }) if id == &self.id => {
                *armed = true;
                Ok(())
            }
            _ => Err("临时粘贴已失效".into()),
        }
    }
    pub(super) async fn delivered(&self) -> Result<()> {
        loop {
            let notified = self.shared.changed.notified();
            {
                let data = self.shared.data.lock().unwrap_or_else(|e| e.into_inner());
                match &data.source {
                    Some(Source::Temporary { id, delivered, .. }) if id == &self.id => {
                        if let Some(result) = delivered {
                            return result.clone();
                        }
                    }
                    _ => return Err("粘贴期间剪贴板已变化，输入结果未知，禁止重放".into()),
                }
                if let Some(error) = &data.error {
                    return Err(error.clone());
                }
            }
            notified.await;
        }
    }
}
impl Shared {
    fn transfer(&self, mime: &str) -> Option<(Arc<Vec<u8>>, Option<String>)> {
        if !self.state.active() {
            return None;
        }
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if !data.owner.as_ref().is_some_and(|o| o.ours) {
            return None;
        }
        match &data.source {
            Some(Source::Temporary {
                id, text, armed, ..
            }) => {
                if !data.owner.as_ref().is_some_and(|o| o.formats.contains(id)) {
                    return None;
                }
                if TEXT.contains(&mime) {
                    Some((text.clone(), armed.then(|| id.clone())))
                } else {
                    None
                }
            }
            None => None,
        }
    }
    async fn done(&self, serial: u32, success: bool) -> Result<()> {
        self.proxy
            .call::<_, _, ()>("SelectionWriteDone", &(&self.path, serial, success))
            .await
            .map_err(|_| "剪贴板交付确认失败".into())
    }
    async fn write(&self, serial: u32, transfer: Option<(Arc<Vec<u8>>, Option<String>)>) {
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let (bytes, _) = transfer.as_ref().ok_or("剪贴板格式已失效")?;
            if !self.state.active() {
                return Err("远控授权已结束".into());
            }
            let fd: zbus::zvariant::OwnedFd = self
                .proxy
                .call("SelectionWrite", &(&self.path, serial))
                .await
                .map_err(|_| "无法交付剪贴板文字")?;
            let channel = io::Channel::new(fd.into())?;
            if !self.state.active() {
                return Err("远控授权已结束".into());
            }
            channel.write(bytes, || self.state.active()).await
        })
        .await
        .unwrap_or_else(|_| Err("剪贴板数据交付超时".into()));
        let confirmed =
            tokio::time::timeout(Duration::from_secs(1), self.done(serial, result.is_ok())).await;
        if let Some((_, Some(token))) = transfer {
            let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(Source::Temporary { id, delivered, .. }) = &mut data.source
                && *id == token
            {
                *delivered = Some(match confirmed {
                    Ok(Ok(())) => result,
                    _ => Err("剪贴板交付结果未知，禁止重放".into()),
                });
                self.changed.notify_waiters();
            }
        }
    }
}
