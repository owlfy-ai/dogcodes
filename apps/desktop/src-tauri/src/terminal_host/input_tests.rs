use super::super::{LaunchIntent, Output, TerminalInfo};
use super::*;
use std::{
    collections::VecDeque,
    io::Write,
    sync::{Mutex, atomic::AtomicBool},
};
struct Writer {
    bytes: Arc<Mutex<Vec<u8>>>,
    block: Arc<AtomicBool>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut out = self.bytes.lock().unwrap();
        if self.block.load(Ordering::Acquire) {
            if out.is_empty() {
                out.push(bytes[0]);
                return Ok(1);
            }
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        out.extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct Fixture {
    host: Arc<TerminalHost>,
    id: String,
    epoch: String,
    bytes: Arc<Mutex<Vec<u8>>>,
    block: Arc<AtomicBool>,
}
impl Fixture {
    fn new() -> Self {
        let host = Arc::new(TerminalHost::default());
        let id = uuid::Uuid::new_v4().to_string();
        let epoch = host.epoch.clone();
        let bytes = Arc::new(Mutex::new(vec![]));
        let block = Arc::new(AtomicBool::new(false));
        let intent = LaunchIntent {
            project_launch: None,
            operation_id: id.clone(),
            project_id: "fixture".into(),
            directory_index: 0,
            task_id: None,
            command_id: None,
            label: "test".into(),
            cwd: ".".into(),
            command: None,
            cols: 80,
            rows: 24,
        };
        let info:TerminalInfo=serde_json::from_value(json!({"id":id,"epoch":epoch,"projectId":"fixture","directoryIndex":0,"taskId":null,"commandId":null,"label":"test","cwd":".","command":null,"status":"running","exitCode":null,"error":null,"outputEnd":0,"createdAt":0})).unwrap();
        let session = Arc::new(Session {
            intent,
            output: Mutex::new(Output {
                info,
                first: 0,
                bytes: VecDeque::new(),
            }),
            process: Mutex::new(None),
            input: Mutex::new(Some(Box::new(Writer {
                bytes: bytes.clone(),
                block: block.clone(),
            }))),
            input_stream: Mutex::default(),
            changed: tokio::sync::Notify::new(),
            state_changes: host.changes.clone(),
        });
        host.sessions.lock().unwrap().insert(id.clone(), session);
        Self {
            host,
            id,
            epoch,
            bytes,
            block,
        }
    }
    fn open(&self, revision: &str, ack: bool) -> Value {
        json!({"type":"open","terminalId":self.id,"expectedEpoch":self.epoch,"expectedRevision":revision,"streamId":uuid::Uuid::new_v4().to_string(),"acknowledgeUncertain":ack})
    }
    fn write(&self, lease: &Value, sequence: &str, bytes: &[u8]) -> Value {
        json!({"type":"write","terminalId":self.id,"expectedEpoch":self.epoch,"streamId":lease["streamId"],"revision":lease["revision"],"sequence":sequence,"data":STANDARD.encode(bytes)})
    }
    async fn inspect(&self) -> Value {
        self.host
            .input_operation(
                "a",
                json!({"type":"inspect","terminalId":self.id,"expectedEpoch":self.epoch}),
            )
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn terminal_input_sequence_and_digest_do_not_replay_duplicates_or_reorder() {
    let f = Fixture::new();
    let open = f.open("0", false);
    let lease = f.host.input_operation("a", open.clone()).await.unwrap();
    let first = f.write(&lease, "1", b"one");
    assert_eq!(
        f.host.input_operation("a", first.clone()).await.unwrap()["state"],
        "applied"
    );
    assert_eq!(
        f.host.input_operation("a", first).await.unwrap()["state"],
        "applied"
    );
    assert!(
        f.host
            .input_operation("a", f.write(&lease, "1", b"changed"))
            .await
            .is_err()
    );
    assert!(
        f.host
            .input_operation("a", f.write(&lease, "3", b"three"))
            .await
            .is_err()
    );
    assert_eq!(
        f.host.input_operation("a", open).await.unwrap()["sequence"],
        "1"
    );
    f.host
        .input_operation("a", f.write(&lease, "2", b"two"))
        .await
        .unwrap();
    assert_eq!(*f.bytes.lock().unwrap(), b"onetwo");
}
#[tokio::test]
async fn terminal_input_binds_actor_epoch_and_generation_even_if_a_stream_id_is_reused() {
    let f = Fixture::new();
    let open = f.open("0", false);
    let lease = f.host.input_operation("a", open.clone()).await.unwrap();
    let old = f.write(&lease, "1", b"old");
    assert!(f.host.input_operation("b", old.clone()).await.is_err());
    let mut next = f.open("1", false);
    next["streamId"] = lease["streamId"].clone();
    let renewed = f.host.input_operation("b", next).await.unwrap();
    assert!(f.host.input_operation("a", old.clone()).await.is_err());
    assert!(f.host.input_operation("b", old).await.is_err());
    assert!(f.host.input_operation("a", open).await.is_err());
    let mut wrong = f.write(&renewed, "1", b"wrong");
    wrong["expectedEpoch"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(f.host.input_operation("b", wrong).await.is_err());
    assert!(f.bytes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn terminal_input_rotation_drains_partial_old_input_and_requires_explicit_acknowledgement() {
    let f = Fixture::new();
    f.block.store(true, Ordering::Release);
    let lease = f
        .host
        .input_operation("a", f.open("0", false))
        .await
        .unwrap();
    let host = f.host.clone();
    let request = f.write(&lease, "1", b"abandoned-tail");
    let duplicate = request.clone();
    let writing = tokio::spawn(async move { host.input_operation("a", request).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.bytes.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.host
            .input_operation("a", duplicate.clone())
            .await
            .unwrap()["state"],
        "writing"
    );
    assert!(
        f.host
            .input_operation("a", f.write(&lease, "2", b"out-of-order"))
            .await
            .is_err()
    );
    let blocked = f
        .host
        .input_operation("a", f.open("1", false))
        .await
        .unwrap();
    assert_eq!(blocked["state"], "blocked");
    assert_eq!(blocked["uncertain"], true);
    assert!(writing.await.unwrap().is_err());
    assert!(f.host.input_operation("a", duplicate).await.is_err());
    assert!(
        f.host
            .input_operation("a", f.write(&blocked, "1", b"not-yet"))
            .await
            .is_err()
    );
    f.block.store(false, Ordering::Release);
    let ready = f
        .host
        .input_operation("a", f.open("2", true))
        .await
        .unwrap();
    f.host
        .input_operation("a", f.write(&ready, "1", b"NEW"))
        .await
        .unwrap();
    assert_eq!(*f.bytes.lock().unwrap(), b"aNEW");
}
#[tokio::test]
async fn terminal_input_invalid_size_and_metadata_do_not_consume_a_sequence() {
    let f = Fixture::new();
    let lease = f
        .host
        .input_operation("a", f.open("0", false))
        .await
        .unwrap();
    assert!(
        f.host
            .input_operation("a", f.write(&lease, "1", &vec![0; LIMIT + 1]))
            .await
            .is_err()
    );
    let mut forged = f.write(&lease, "1", b"x");
    forged["owner"] = json!("a");
    assert!(f.host.input_operation("b", forged).await.is_err());
    assert_eq!(f.inspect().await["sequence"], "0");
    assert!(f.bytes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn terminal_input_late_drain_does_not_activate_an_obsolete_open() {
    let f = Fixture::new();
    let session = f.host.session(&f.id).unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let holding = tokio::task::spawn_blocking(move || {
        let _input = session.input.lock().unwrap();
        entered.send(()).unwrap();
        let _ = wait.recv();
    });
    started.await.unwrap();
    let first = f.open("0", false);
    assert_eq!(
        f.host.input_operation("a", first.clone()).await.unwrap()["state"],
        "opening"
    );
    let second = tokio::time::timeout(Duration::from_secs(3), async {
        let mut latest = Value::Null;
        for revision in 1..=20 {
            latest = f.open(&revision.to_string(), false);
            assert_eq!(
                f.host.input_operation("a", latest.clone()).await.unwrap()["state"],
                "opening"
            );
        }
        latest
    })
    .await
    .expect("Pending switches must coalesce instead of adding blocked workers");
    drop(release);
    holding.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if f.inspect().await["state"] == "ready" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ready = f.inspect().await;
    assert_eq!(ready["streamId"], second["streamId"]);
    assert_eq!(ready["revision"], "21");
    assert!(f.host.input_operation("a", first).await.is_err());
}
