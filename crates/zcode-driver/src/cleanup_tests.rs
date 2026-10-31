//! Private Python `ZCode` protocol fixtures only; never starts an installed client.
use super::*;
use std::{path::Path, time::Duration};
const FIXTURE: &str = r"#!/usr/bin/env python3
import json,sys,os,time,threading,signal,subprocess
os.chdir(os.path.dirname(os.path.abspath(__file__)))
mode='__MODE__'
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=None
if mode in ['stubborn','probe','leader']:
 child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])
with open('process.json','w') as f:json.dump([os.getpid(),child.pid if child else None],f)
sessions={};next_session=0;close_count=0
lock=threading.Lock()
def out(v):
 with lock:print(json.dumps(v),flush=True)
def reply(r,v=None):out({'id':r['id'],'result':v if v is not None else {}})
def missing(r):out({'id':r['id'],'error':{'code':-32004,'message':'Session is not active: '+r['params']['sessionId']}})
def snap(id):return {'sessionId':id,'session':{'sessionId':id,'workspace':{'workspacePath':os.getcwd()}},'settings':{'model':{'available':[{'ref':{'providerId':'test','modelId':'test'}}]}}}
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 method=r.get('method');id=r.get('params',{}).get('sessionId')
 if method=='session/list':
  if mode=='probe':time.sleep(60)
  if mode=='eof':os.close(1);os.close(2);time.sleep(60)
  if mode=='leader':sys.exit(0)
  if mode=='oversize':sys.stdout.write('x'*(4*1024*1024+1));sys.stdout.flush();time.sleep(60)
  reply(r,{'sessions':[] if mode=='unknown' else [snap(i) for i in sessions]})
  if mode=='writer':time.sleep(60)
 elif method in ['session/create','session/resume']:
  if method=='session/create':next_session+=1;id='session-'+str(next_session)
  sessions[id]=True;reply(r,snap(id))
 elif method=='session/read':
  if id in sessions:reply(r,snap(id))
  else:missing(r)
 elif method=='session/close':
  close_count+=1
  if mode=='stop-stuck':
   while not os.path.exists('allow-close'):time.sleep(0.01)
  if mode=='false' and close_count==1:reply(r,{'closed':False});continue
  if mode in ['unknown','silent']:
   threading.Thread(target=lambda:(time.sleep(0.4),sessions.pop(id,None)),daemon=True).start()
   if mode=='unknown':reply(r)
   continue
  if mode=='delayed':time.sleep(0.2)
  sessions.pop(id,None);reply(r,{'closed':True})
 elif method=='session/send':
  reply(r);out({'method':'turn.started','params':{'sessionId':id,'turnId':'turn-'+str(r['id'])}})
  if mode=='long':time.sleep(0.2)
  if r['params']['content']!='slow':out({'method':'turn.completed','params':{'sessionId':id,'resultType':'success'}})
 elif method=='session/stop':
  time.sleep(0.2);reply(r);time.sleep(0.2)
  if mode!='stop-stuck':out({'method':'turn.completed','params':{'sessionId':id,'resultType':'cancelled'}})
 else:reply(r)
if mode in ['stubborn','probe','writer','eof','leader']:
 while True:time.sleep(60)
";
pub(super) fn fixture(mode: &str) -> (tempfile::TempDir, ZcodeLaunch) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("runtime");
    std::fs::write(&executable, FIXTURE.replace("__MODE__", mode)).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launch = ZcodeLaunch {
        app_binary: executable,
        cli_script: root.path().join("unused.cjs"),
    };
    (root, launch)
}
pub(super) fn driver(launch: ZcodeLaunch) -> Arc<ZcodeDriver> {
    Arc::new(
        ZcodeDriver::new(
            "zcode",
            &InstallationId::new().to_string(),
            launch,
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap(),
    )
}
pub(super) async fn session(driver: &ZcodeDriver, root: &Path) -> SessionHandle {
    driver
        .create_session(SessionRequest {
            working_directory: root.to_string_lossy().into_owned(),
            additional_directories: vec![],
            app_mcps: vec![],
            permission_mode: None,
        })
        .await
        .unwrap()
}
pub(super) fn prompt(root: &Path, text: &str) -> HubPromptRequest {
    HubPromptRequest {
        attachments: vec![],
        permission_mode: None,
        additional_directories: vec![],
        prompt: text.into(),
        working_directory: root.to_string_lossy().into_owned(),
        context: vec![],
    }
}
pub(super) fn count(root: &Path, method: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["method"] == method)
        .count()
}
pub(super) async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(9), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
pub(super) async fn events(stream: DriverEventStream) -> Vec<Result<DriverEvent, DriverError>> {
    tokio::time::timeout(Duration::from_secs(8), stream.collect())
        .await
        .unwrap()
}
pub(super) async fn native(driver: &ZcodeDriver) -> Arc<lifecycle::NativeProcess> {
    driver
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone()
}
pub(super) fn ids(root: &Path) -> Vec<u32> {
    serde_json::from_slice::<Vec<Option<u32>>>(&std::fs::read(root.join("process.json")).unwrap())
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}
pub(super) fn present(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim() == pid.to_string()
}

#[tokio::test]
async fn active_shutdown_waits_for_native_close_and_cancelled_waiter_does_not_affect_another_session()
 {
    let (root, launch) = fixture("delayed");
    let driver = driver(launch);
    let first = session(&driver, root.path()).await;
    let second = session(&driver, root.path()).await;
    let native = native(&driver).await;
    let pid = native.id();
    let stream = driver
        .prompt(&first, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    let close = tokio::spawn({
        let driver = driver.clone();
        let first = first.clone();
        async move { driver.shutdown(first).await }
    });
    wait_for(|| count(root.path(), "session/stop") == 1).await;
    assert!(!close.is_finished());
    assert!(
        driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&first.native_session_id)
    );
    close.abort();
    let _ = close.await;
    assert!(
        driver
            .prompt(&first, prompt(root.path(), "must not send"))
            .await
            .is_err()
    );
    driver.shutdown(first.clone()).await.unwrap();
    assert!(
        !driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&first.native_session_id)
    );
    assert_eq!(count(root.path(), "session/close"), 1);
    assert_eq!(count(root.path(), "session/stop"), 1);
    assert!(
        events(stream)
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(!native.stopping());
    assert_eq!(native.id(), pid);
    assert!(
        events(
            driver
                .prompt(&second, prompt(root.path(), "hello"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(second).await.unwrap();
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn unknown_close_uses_native_read_proof_without_repeating_or_trusting_empty_list() {
    let (root, launch) = fixture("unknown");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    assert!(driver.shutdown(handle.clone()).await.is_err());
    assert!(
        driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&handle.native_session_id)
    );
    assert!(driver.shutdown(handle.clone()).await.is_err());
    assert!(
        driver
            .prompt(&handle, prompt(root.path(), "must not send"))
            .await
            .is_err()
    );
    let state = driver
        .sessions
        .lock()
        .expect("sessions")
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    let receipt = state.close_result.lock().unwrap().clone().unwrap();
    wait_for(|| matches!(*receipt.borrow(), Some(Ok(())))).await;
    assert!(
        !driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&handle.native_session_id)
    );
    assert_eq!(count(root.path(), "session/close"), 1);
    assert_eq!(count(root.path(), "session/list"), 0);
    assert_eq!(count(root.path(), "session/setMode"), 0);
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn explicit_native_close_refusal_retains_the_session_and_allows_a_fresh_close() {
    let (root, launch) = fixture("false");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    assert!(driver.shutdown(handle.clone()).await.is_err());
    assert!(
        driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&handle.native_session_id)
    );
    driver.shutdown(handle).await.unwrap();
    assert!(driver.sessions.lock().expect("sessions").is_empty());
    assert_eq!(count(root.path(), "session/close"), 2);
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn cancelled_interrupt_waiter_joins_the_same_native_stop_and_waits_for_confirmation() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    let stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    let stop = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        async move { driver.interrupt(&handle).await }
    });
    wait_for(|| count(root.path(), "session/stop") == 1).await;
    assert!(!stop.is_finished());
    stop.abort();
    let _ = stop.await;
    driver.interrupt(&handle).await.unwrap();
    assert_eq!(count(root.path(), "session/stop"), 1);
    assert_eq!(
        events(stream)
            .await
            .iter()
            .filter(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
            .count(),
        1
    );
    assert!(
        events(
            driver
                .prompt(&handle, prompt(root.path(), "continue"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn queued_turn_closes_before_dispatch_and_does_not_change_native_mode() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let first = session(&driver, root.path()).await;
    let second = session(&driver, root.path()).await;
    let native = native(&driver).await;
    let first_stream = driver
        .prompt(&first, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    let second_stream = driver
        .prompt(&second, prompt(root.path(), "must not send"))
        .await
        .unwrap();
    let close = tokio::spawn({
        let driver = driver.clone();
        let second = second.clone();
        async move { driver.shutdown(second).await }
    });
    let state = driver
        .sessions
        .lock()
        .expect("sessions")
        .get(&second.native_session_id)
        .cloned()
        .unwrap();
    wait_for(|| state.closing.load(Ordering::Acquire)).await;
    driver.interrupt(&first).await.unwrap();
    close.await.unwrap().unwrap();
    assert!(
        events(first_stream)
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(
        events(second_stream)
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert_eq!(count(root.path(), "session/send"), 1);
    assert_eq!(count(root.path(), "session/setMode"), 1);
    driver.shutdown(first).await.unwrap();
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn dropping_driver_cleans_a_stubborn_shared_worker_while_the_turn_owns_its_mutex() {
    let (root, launch) = fixture("stubborn");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    let ids = ids(root.path());
    let stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    drop(driver);
    native.closed().await.unwrap();
    assert!(ids.iter().all(|id| !present(*id)));
    assert!(
        events(stream)
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnFailed { .. }) | Err(_)))
    );
}
#[tokio::test]
async fn cancelled_probe_cleans_the_original_private_scope() {
    let (root, launch) = fixture("probe");
    let probe = tokio::spawn(async move { ZcodeDriver::probe_ready(&launch).await });
    wait_for(|| count(root.path(), "session/list") == 1).await;
    let ids = ids(root.path());
    probe.abort();
    let _ = probe.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}
#[tokio::test]
async fn eof_is_not_native_exit_and_leader_exit_is_not_hidden_by_an_inherited_pipe() {
    for mode in ["eof", "leader"] {
        let (root, launch) = fixture(mode);
        let mut connection = Connection::spawn(&launch).unwrap();
        let native = connection.native.clone();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(5),
                connection.request("session/list", json!({}), None)
            )
            .await
            .unwrap()
            .is_err()
        );
        native.closed().await.unwrap();
        assert!(ids(root.path()).iter().all(|id| !present(*id)));
    }
}
#[tokio::test]
async fn blocked_input_does_not_prevent_native_scope_cleanup() {
    let (root, launch) = fixture("writer");
    let mut connection = Connection::spawn(&launch).unwrap();
    connection
        .request("session/list", json!({}), None)
        .await
        .unwrap();
    let native = connection.native.clone();
    let ids = ids(root.path());
    let writer = tokio::spawn({
        let native = native.clone();
        async move {
            native
                .send(&json!({"method":"blocked","blob":"x".repeat(2*1024*1024)}))
                .await
        }
    });
    wait_for(|| native.input_busy()).await;
    native.close().await.unwrap();
    assert!(writer.await.unwrap().is_err());
    assert!(ids.iter().all(|id| !present(*id)));
}

#[tokio::test]
async fn cancelling_a_partial_input_frame_retires_the_worker_before_any_next_write() {
    let (root, launch) = fixture("writer");
    let mut connection = Connection::spawn(&launch).unwrap();
    connection
        .request("session/list", json!({}), None)
        .await
        .unwrap();
    let native = connection.native.clone();
    let ids = ids(root.path());
    let writer = tokio::spawn({
        let native = native.clone();
        async move {
            native
                .send(&json!({"method":"partial","blob":"x".repeat(2*1024*1024)}))
                .await
        }
    });
    wait_for(|| native.input_busy()).await;
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    let retired_by_cancellation = native.stopping();
    if retired_by_cancellation {
        native.closed().await.unwrap();
    } else {
        // Keep the regression failure from leaving our private fixture alive.
        native.close().await.unwrap();
    }
    assert!(retired_by_cancellation);
    assert!(native.send(&json!({"method":"next"})).await.is_err());
    assert!(ids.iter().all(|id| !present(*id)));
}

#[tokio::test]
async fn lost_native_close_reply_is_verified_without_replaying_the_mutation() {
    let (root, launch) = fixture("silent");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    driver
        .connection
        .lock()
        .await
        .as_mut()
        .unwrap()
        .request_timeout = Duration::from_millis(80);
    assert!(driver.shutdown(handle.clone()).await.is_err());
    assert!(
        driver
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(&handle.native_session_id)
    );
    assert!(driver.shutdown(handle.clone()).await.is_err());
    let state = driver
        .sessions
        .lock()
        .expect("sessions")
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    let receipt = state.close_result.lock().unwrap().clone().unwrap();
    wait_for(|| matches!(*receipt.borrow(), Some(Ok(())))).await;
    assert_eq!(count(root.path(), "session/close"), 1);
    let requests = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    let reads: Vec<Value> = requests
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["method"] == "session/read")
        .collect();
    assert!(reads.len() >= 2);
    assert!(reads.iter().all(
        |row| row["params"]["contentProfile"] == "index" && row["params"]["messageLimit"] == 1
    ));
    drop(driver);
    native.closed().await.unwrap();
}

#[tokio::test]
async fn stop_ack_without_terminal_does_not_report_interrupted_or_reuse_the_session() {
    let (root, launch) = fixture("stop-stuck");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    driver
        .connection
        .lock()
        .await
        .as_mut()
        .unwrap()
        .request_timeout = Duration::from_millis(350);
    let stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    assert!(driver.interrupt(&handle).await.is_err());
    let ended = events(stream).await;
    assert!(
        ended
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnFailed { .. })))
    );
    assert!(!ended.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnInterrupted { .. })
    )));
    assert_eq!(count(root.path(), "session/stop"), 1);
    // The native close has not completed, so another stop attempt must retain
    // the failed termination receipt rather than treating turn_active=false as proof.
    let retried = driver.interrupt(&handle).await;
    std::fs::write(root.path().join("allow-close"), b"").unwrap();
    driver.shutdown(handle).await.unwrap();
    drop(driver);
    native.closed().await.unwrap();
    assert!(retried.is_err());
}
#[tokio::test]
async fn a_started_turn_can_outlive_its_start_handshake_deadline() {
    let (root, launch) = fixture("long");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let native = native(&driver).await;
    driver
        .connection
        .lock()
        .await
        .as_mut()
        .unwrap()
        .request_timeout = Duration::from_millis(80);
    let ended = events(
        driver
            .prompt(&handle, prompt(root.path(), "hello"))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        ended
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    assert!(
        !ended
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnFailed { .. })))
    );
    driver.shutdown(handle).await.unwrap();
    drop(driver);
    native.closed().await.unwrap();
}
#[tokio::test]
async fn an_oversized_unterminated_frame_is_rejected_without_waiting_for_a_newline() {
    let (root, launch) = fixture("oversize");
    let mut connection = Connection::spawn(&launch).unwrap();
    let native = connection.native.clone();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            connection.request("session/list", json!({}), None)
        )
        .await
        .unwrap()
        .is_err()
    );
    native.closed().await.unwrap();
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
