//! Short-lived endpoint buffers for manual clipboard transfers. No server storage,
//! automatic resume or replay of clipboard writes. Chunks fit a phone RPC frame.
use super::{Value, json, native};
use base64::{Engine, engine::general_purpose::STANDARD};
use dogcodes_remote_desktop as os;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

pub(super) const CHUNK: usize = 32 * 1024;
const MAX_TRANSFERS: usize = 4;
const PER_CONNECTION: usize = 2;
const IDLE_TTL: Duration = Duration::from_mins(2);

pub(super) trait Clipboard {
    fn get(&mut self, kind: &str) -> Result<Vec<u8>, String>;
    fn set(&mut self, kind: &str, bytes: &[u8]) -> Result<(), String>;
    fn set_current(
        &mut self,
        kind: &str,
        bytes: &[u8],
        current: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if !current() {
            return Err("剪贴板连接已关闭".into());
        }
        self.set(kind, bytes)
    }
}
struct SystemClipboard(Option<os::Clipboard>);
impl Clipboard for SystemClipboard {
    fn get(&mut self, kind: &str) -> Result<Vec<u8>, String> {
        if kind == "image" {
            os::clipboard_image_get_png()
        } else {
            Ok(os::clipboard_get("text")?["text"]
                .as_str()
                .ok_or("剪贴板没有文字")?
                .as_bytes()
                .to_vec())
        }
    }
    fn set(&mut self, kind: &str, bytes: &[u8]) -> Result<(), String> {
        self.set_current(kind, bytes, &|| true)
    }
    fn set_current(
        &mut self,
        kind: &str,
        bytes: &[u8],
        current: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if !current() {
            return Err("剪贴板连接已关闭".into());
        }
        self.0 = Some(if kind == "image" {
            os::clipboard_image_set_png_if_current(bytes, current)?
        } else {
            os::clipboard_set(
                json!({"kind":"text","text":std::str::from_utf8(bytes).map_err(|_|"剪贴板文字编码无效")?}),
            )?
        });
        Ok(())
    }
}

pub(super) const METHODS: &[&str] = &["clipboard_transfer"];
type Factory = Box<dyn FnOnce() -> Box<dyn Clipboard> + Send>;
struct Request {
    owner: Arc<native::Connection>,
    args: Value,
    reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
}
/// Clipboard decoding, encoding and platform I/O have a separate bounded worker
/// from capture/input. A slow image or stalled clipboard provider cannot hold
/// keyboard release, video ACK, or remote-desktop keepalive in its queue.
pub(super) struct Host {
    sender: OnceLock<Result<SyncSender<Request>, String>>,
    factory: Mutex<Option<Factory>>,
    unlocked: fn() -> Result<(), String>,
}
impl Default for Host {
    fn default() -> Self {
        Self {
            sender: OnceLock::new(),
            factory: Mutex::new(Some(Box::new(|| Box::new(SystemClipboard(None))))),
            unlocked: os::require_unlocked,
        }
    }
}
impl Host {
    #[cfg(test)]
    fn fixture(factory: Factory) -> Self {
        Self {
            factory: Mutex::new(Some(factory)),
            unlocked: || Ok(()),
            ..Self::default()
        }
    }
    pub(super) async fn call(
        &self,
        owner: Arc<native::Connection>,
        method: &str,
        args: Value,
    ) -> Result<Value, String> {
        if method != "clipboard_transfer" {
            return Err("剪贴板接口不可用".into());
        }
        let sender = self
            .sender
            .get_or_init(|| {
                let factory = self
                    .factory
                    .lock()
                    .map_err(|_| "剪贴板线程不可用")?
                    .take()
                    .ok_or("剪贴板线程已启动")?;
                let unlocked = self.unlocked;
                let (tx, rx) = sync_channel::<Request>(8);
                std::thread::Builder::new()
                    .name("remote-clipboard".into())
                    .spawn(move || {
                        let mut clipboard = factory();
                        let mut transfers = Transfers::default();
                        loop {
                            transfers.expire();
                            match rx.recv_timeout(Duration::from_millis(100)) {
                                Ok(r) => {
                                    if r.reply.is_closed() || !r.owner.alive() {
                                        continue;
                                    }
                                    let result = (|| {
                                        if r.args["operation"] != "close" {
                                            unlocked()?;
                                        }
                                        transfers.call(&r.owner, r.args, clipboard.as_mut())
                                    })();
                                    let _ = r.reply.send(result);
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    })
                    .map_err(|_| "无法启动剪贴板线程")?;
                Ok(tx)
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let (reply, result) = tokio::sync::oneshot::channel();
        sender
            .try_send(Request { owner, args, reply })
            .map_err(|_| "电脑剪贴板繁忙，请稍后重试")?;
        tokio::time::timeout(Duration::from_secs(15), result)
            .await
            .map_err(|_| "电脑剪贴板响应超时，请先核实复制结果；不会自动重发")?
            .map_err(|_| "电脑剪贴板线程已结束")?
    }
}
struct Transfer {
    owner: Weak<native::Connection>,
    expires: Instant,
    bytes: Vec<u8>,
    size: usize,
    kind: String,
    sha256: String,
    download: bool,
    applied: bool,
}
impl Transfer {
    fn description(&self, id: &str) -> Value {
        json!({"transferId":id,"size":self.size,"sha256":self.sha256,
        "kind":self.kind,"mimeType":mime(&self.kind),"chunkBytes":CHUNK,"offset":if self.applied {self.size} else {self.bytes.len()},
        "direction":if self.download {"download"} else {"upload"},
        "state":if self.applied {"applied"} else if self.download {"ready"} else {"staging"}})
    }
}
#[derive(Default)]
pub(super) struct Transfers {
    values: HashMap<String, Transfer>,
}
impl Transfers {
    pub(super) fn expire(&mut self) {
        let now = Instant::now();
        self.values
            .retain(|_, t| t.expires > now && t.owner.upgrade().is_some_and(|c| c.alive()));
    }
    pub(super) fn call(
        &mut self,
        owner: &Arc<native::Connection>,
        args: Value,
        clipboard: &mut dyn Clipboard,
    ) -> Result<Value, String> {
        self.expire();
        if !owner.alive() {
            return Err("剪贴板连接已关闭，不会重发复制操作".into());
        }
        let op = args["operation"].as_str().ok_or("剪贴板操作无效")?;
        if op == "begin_upload" || op == "begin_download" {
            if self.values.len() >= MAX_TRANSFERS
                || self
                    .values
                    .values()
                    .filter(|t| t.owner.ptr_eq(&Arc::downgrade(owner)))
                    .count()
                    >= PER_CONNECTION
            {
                return Err("剪贴板传输繁忙，请结束当前传输后重试".into());
            }
            let kind = args["kind"]
                .as_str()
                .filter(|k| matches!(*k, "text" | "image"))
                .ok_or("剪贴板类型无效")?;
            let download = op == "begin_download";
            let (bytes, size, sha256) = if download {
                let bytes = clipboard.get(kind)?;
                let size = bytes.len();
                let hash = format!("{:x}", Sha256::digest(&bytes));
                (bytes, size, hash)
            } else {
                let size = number(&args, "size")?;
                let hash = args["sha256"].as_str().ok_or("缺少图片校验信息")?;
                if args["mimeType"] != mime(kind)
                    || hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err("图片校验信息无效".into());
                }
                (Vec::new(), size, hash.to_string())
            };
            if size == 0
                || size
                    > if kind == "image" {
                        os::MAX_CLIPBOARD_IMAGE_BYTES
                    } else {
                        1024 * 1024
                    }
            {
                return Err("剪贴板内容为空或过大（图片限 16 MB，文字限 1 MB）".into());
            }
            let transfer = Transfer {
                owner: Arc::downgrade(owner),
                expires: Instant::now() + IDLE_TTL,
                bytes,
                size,
                kind: kind.to_string(),
                sha256,
                download,
                applied: false,
            };
            let id = uuid::Uuid::new_v4().to_string();
            let result = transfer.description(&id);
            self.values.insert(id, transfer);
            return Ok(result);
        }
        let id = args["transferId"].as_str().ok_or("缺少剪贴板传输标识")?;
        let Some(transfer) = self.values.get_mut(id) else {
            return Err("剪贴板传输已结束或过期，请重新选择操作；不会自动重发".into());
        };
        if !transfer.owner.ptr_eq(&Arc::downgrade(owner)) {
            return Err("剪贴板传输不属于当前连接".into());
        }
        transfer.expires = Instant::now() + IDLE_TTL;
        match op {
            "status" => Ok(transfer.description(id)),
            "close" => {
                self.values.remove(id);
                Ok(Value::Null)
            }
            "read" => {
                if !transfer.download {
                    return Err("剪贴板传输方向无效".into());
                }
                let offset = number(&args, "offset")?;
                if offset >= transfer.bytes.len() {
                    return Err("图片读取位置无效".into());
                }
                let end = offset.saturating_add(CHUNK).min(transfer.bytes.len());
                Ok(json!({"transferId":id,"offset":offset,"nextOffset":end,
                    "bytes":STANDARD.encode(&transfer.bytes[offset..end])}))
            }
            "append" => {
                if transfer.download || transfer.applied {
                    return Err("图片传输已结束或方向无效".into());
                }
                let offset = number(&args, "offset")?;
                let encoded = args["bytes"].as_str().ok_or("缺少图片数据")?;
                if encoded.len() > CHUNK.div_ceil(3) * 4 {
                    return Err("图片数据块过大".into());
                }
                let bytes = STANDARD.decode(encoded).map_err(|_| "图片数据块编码无效")?;
                let end = offset.checked_add(bytes.len()).ok_or("图片位置无效")?;
                if bytes.is_empty() || bytes.len() > CHUNK || end > transfer.size {
                    return Err("图片数据块大小无效".into());
                }
                if offset == transfer.bytes.len() {
                    transfer.bytes.extend_from_slice(&bytes);
                } else if end > transfer.bytes.len()
                    || transfer.bytes.get(offset..end) != Some(bytes.as_slice())
                {
                    return Err("图片数据块顺序或内容不匹配".into());
                }
                Ok(json!({"transferId":id,"offset":transfer.bytes.len()}))
            }
            "commit" => {
                if transfer.download {
                    return Err("剪贴板传输方向无效".into());
                }
                if !transfer.applied {
                    if transfer.bytes.len() != transfer.size
                        || format!("{:x}", Sha256::digest(&transfer.bytes)) != transfer.sha256
                    {
                        return Err("图片不完整或校验失败".into());
                    }
                    if !owner.alive() {
                        return Err("剪贴板连接已关闭，不会重发复制操作".into());
                    }
                    clipboard.set_current(&transfer.kind, &transfer.bytes, &|| owner.alive())?;
                    transfer.applied = true;
                    // Retain the immutable manifest/receipt, not the image, after applying.
                    transfer.bytes.clear();
                    transfer.bytes.shrink_to_fit();
                }
                Ok(transfer.description(id))
            }
            _ => Err("剪贴板操作无效".into()),
        }
    }
}
fn number(args: &Value, key: &str) -> Result<usize, String> {
    args[key]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| "图片大小或位置无效".into())
}

fn mime(kind: &str) -> &str {
    if kind == "image" {
        "image/png"
    } else {
        "text/plain;charset=utf-8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        bytes: Vec<u8>,
        writes: usize,
        fail: bool,
    }
    impl Clipboard for Fake {
        fn get(&mut self, _: &str) -> Result<Vec<u8>, String> {
            Ok(self.bytes.clone())
        }
        fn set(&mut self, _: &str, bytes: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("fixture refused".into());
            }
            self.writes += 1;
            self.bytes = bytes.to_vec();
            Ok(())
        }
    }
    fn owner() -> native::Disconnect {
        let (tx, _) = tokio::sync::mpsc::channel(8);
        native::Disconnect::new(None, tx)
    }
    #[tokio::test]
    async fn removed_clipboard_rpcs_cannot_start_a_native_worker() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let started = Arc::new(AtomicBool::new(false));
        let observed = started.clone();
        let host = Host::fixture(Box::new(move || {
            observed.store(true, Ordering::Release);
            Box::new(Fake::default())
        }));
        let connection = owner();
        for method in ["clipboard_get", "clipboard_set"] {
            assert!(
                host.call(
                    connection.0.clone(),
                    method,
                    json!({"kind":"text","text":"fixture"})
                )
                .await
                .is_err()
            );
        }
        assert!(!started.load(Ordering::Acquire));
        assert!(host.sender.get().is_none());
    }
    fn begin(t: &mut Transfers, c: &native::Disconnect, f: &mut Fake, bytes: &[u8]) -> String {
        t.call(&c.0, json!({"operation":"begin_upload","kind":"image","mimeType":"image/png","size":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}), f).unwrap()["transferId"].as_str().unwrap().into()
    }
    fn append(
        t: &mut Transfers,
        c: &native::Disconnect,
        f: &mut Fake,
        id: &str,
        offset: usize,
        bytes: &[u8],
    ) -> Result<Value, String> {
        t.call(&c.0, json!({"operation":"append","transferId":id,"offset":offset,"bytes":STANDARD.encode(bytes)}), f)
    }
    #[test]
    fn upload_retries_cannot_repeat_a_clipboard_write_or_overwrite_a_later_copy() {
        let mut t = Transfers::default();
        let c = owner();
        let mut f = Fake::default();
        let bytes = vec![42; CHUNK + 17];
        let id = begin(&mut t, &c, &mut f, &bytes);
        let commit = json!({"operation":"commit","transferId":id});
        assert!(t.call(&c.0, commit.clone(), &mut f).is_err());
        assert_eq!(f.writes, 0);
        append(&mut t, &c, &mut f, &id, 0, &bytes[..CHUNK]).unwrap();
        append(&mut t, &c, &mut f, &id, 0, &bytes[..CHUNK]).unwrap();
        assert!(append(&mut t, &c, &mut f, &id, 1, b"wrong").is_err());
        append(&mut t, &c, &mut f, &id, CHUNK, &bytes[CHUNK..]).unwrap();
        let applied = t.call(&c.0, commit.clone(), &mut f).unwrap();
        assert_eq!(applied["state"], "applied");
        assert_eq!(applied["offset"], bytes.len());
        f.bytes = b"user copied something else".to_vec();
        t.call(&c.0, commit, &mut f).unwrap();
        assert_eq!(f.writes, 1);
        assert_eq!(f.bytes, b"user copied something else");
        assert!(t.values[&id].bytes.is_empty());
    }
    #[test]
    fn a_download_is_an_immutable_chunked_snapshot() {
        let mut transfers = Transfers::default();
        let connection = owner();
        let original = vec![27; CHUNK * 2 + 3];
        let mut clipboard = Fake {
            bytes: original.clone(),
            ..Fake::default()
        };
        let snapshot = transfers
            .call(
                &connection.0,
                json!({"operation":"begin_download","kind":"image"}),
                &mut clipboard,
            )
            .unwrap();
        clipboard.bytes = vec![3; 200];
        let id = snapshot["transferId"].as_str().unwrap();
        let mut received = Vec::new();
        while received.len() < original.len() {
            let chunk = transfers
                .call(
                    &connection.0,
                    json!({"operation":"read","transferId":id,"offset":received.len()}),
                    &mut clipboard,
                )
                .unwrap();
            let bytes = STANDARD.decode(chunk["bytes"].as_str().unwrap()).unwrap();
            assert!(bytes.len() <= CHUNK);
            received.extend_from_slice(&bytes);
        }
        assert_eq!(received, original);
        assert_eq!(clipboard.writes, 0);
        assert!(
            transfers
                .call(
                    &connection.0,
                    json!({"operation":"commit","transferId":id}),
                    &mut clipboard
                )
                .is_err()
        );
    }
    #[test]
    fn connection_binding_expiration_disconnect_and_close_release_resources() {
        let mut t = Transfers::default();
        let c = owner();
        let other = owner();
        let mut f = Fake::default();
        let id = begin(&mut t, &c, &mut f, b"image");
        for operation in ["status", "commit", "close", "read", "append"] {
            assert!(
                t.call(
                    &other.0,
                    json!({"operation":operation,"transferId":id}),
                    &mut f
                )
                .is_err()
            );
        }
        t.values.get_mut(&id).unwrap().expires =
            Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        t.expire();
        assert!(t.values.is_empty());
        let id = begin(&mut t, &c, &mut f, b"image");
        drop(c);
        t.expire();
        assert!(!t.values.contains_key(&id));
        let id = begin(&mut t, &other, &mut f, b"image");
        t.call(
            &other.0,
            json!({"operation":"close","transferId":id}),
            &mut f,
        )
        .unwrap();
        assert!(t.values.is_empty());
    }
    #[test]
    fn invalid_data_and_failed_system_writes_have_no_acceptance_receipt() {
        let mut t = Transfers::default();
        let c = owner();
        let mut f = Fake::default();
        let id = begin(&mut t, &c, &mut f, b"expected");
        assert!(append(&mut t, &c, &mut f, &id, 0, &vec![0; CHUNK + 1]).is_err());
        assert!(append(&mut t, &c, &mut f, &id, 8, b"x").is_err());
        append(&mut t, &c, &mut f, &id, 0, b"tampered").unwrap();
        assert!(
            t.call(&c.0, json!({"operation":"commit","transferId":id}), &mut f)
                .is_err()
        );
        assert_eq!(f.writes, 0);
        let id = begin(&mut t, &c, &mut f, b"valid");
        append(&mut t, &c, &mut f, &id, 0, b"valid").unwrap();
        f.fail = true;
        assert!(
            t.call(&c.0, json!({"operation":"commit","transferId":id}), &mut f)
                .is_err()
        );
        assert_eq!(
            t.call(&c.0, json!({"operation":"status","transferId":id}), &mut f)
                .unwrap()["state"],
            "staging"
        );
    }
    #[test]
    fn per_connection_and_global_limits_bound_retained_memory() {
        let mut t = Transfers::default();
        let cs = [owner(), owner(), owner()];
        let mut f = Fake::default();
        let args = json!({"operation":"begin_upload","kind":"image","mimeType":"image/png","size":os::MAX_CLIPBOARD_IMAGE_BYTES+1,"sha256":"a".repeat(64)});
        assert!(t.call(&cs[0].0, args, &mut f).is_err());
        for c in &cs[..2] {
            for _ in 0..2 {
                begin(&mut t, c, &mut f, b"x");
            }
        }
        assert!(
            t.call(
                &cs[2].0,
                json!({"operation":"begin_download","kind":"image"}),
                &mut f
            )
            .is_err()
        );
        assert_eq!(t.values.len(), MAX_TRANSFERS);
    }
    #[test]
    fn utf8_text_uses_the_same_bounded_protocol_and_its_own_limit() {
        let mut t = Transfers::default();
        let c = owner();
        let mut f = Fake {
            bytes: "手机文本🟢".repeat(12000).into_bytes(),
            ..Fake::default()
        };
        let d = t
            .call(
                &c.0,
                json!({"operation":"begin_download","kind":"text"}),
                &mut f,
            )
            .unwrap();
        assert_eq!(d["kind"], "text");
        assert_eq!(d["mimeType"], "text/plain;charset=utf-8");
        f.bytes = vec![0; 1024 * 1024 + 1];
        assert!(
            t.call(
                &c.0,
                json!({"operation":"begin_download","kind":"text"}),
                &mut f
            )
            .is_err()
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_clipboard_io_does_not_hold_desktop_control_or_heartbeat() {
        struct Blocked {
            entered: Option<std::sync::mpsc::Sender<()>>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Clipboard for Blocked {
            fn get(&mut self, _: &str) -> Result<Vec<u8>, String> {
                self.entered.take().unwrap().send(()).unwrap();
                self.release
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "fixture timed out")?;
                Ok(b"synthetic image, never a real OS clipboard".to_vec())
            }
            fn set(&mut self, _: &str, _: &[u8]) -> Result<(), String> {
                panic!("no OS writes in this test")
            }
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let clipboard = Host::fixture(Box::new(move || {
            Box::new(Blocked {
                entered: Some(entered_tx),
                release: release_rx,
            })
        }));
        let directory = tempfile::tempdir().unwrap();
        let power = Arc::new(super::super::power::Host::new(
            directory.path().join("power.json"),
        ));
        let native = Arc::new(native::Host::with_clipboard(power, clipboard));
        let c = owner();
        let link = c.0.clone();
        let worker = native.clone();
        let pending = tokio::spawn(async move {
            worker
                .call(
                    link,
                    "clipboard_transfer",
                    json!({"operation":"begin_download","kind":"image"}),
                )
                .await
        });
        tokio::task::spawn_blocking(move || {
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        })
        .await
        .unwrap();
        // A real heartbeat/status request uses the same public native Host, but
        // cannot queue behind the blocked clipboard backend. No screen capture,
        // key injection, system clipboard or paid Agent is touched.
        let status = tokio::time::timeout(
            Duration::from_secs(2),
            native.call(c.0.clone(), "device_status", Value::Null),
        )
        .await;
        release_tx.send(()).unwrap();
        assert_eq!(status.unwrap().unwrap()["agentAvailable"], true);
        assert_eq!(pending.await.unwrap().unwrap()["state"], "ready");
    }
}
