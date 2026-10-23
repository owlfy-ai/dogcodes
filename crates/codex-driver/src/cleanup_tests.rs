//! Only private Python app-server fixtures; no installed Agent or model calls.
use super::*;
use futures::StreamExt;
use std::os::unix::fs::PermissionsExt;

const FIXTURE: &str = r"#!/usr/bin/env python3
import sys,os,json,signal,time,subprocess
os.chdir(os.path.dirname(os.path.abspath(__file__)))
mode='__MODE__'
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=None
if mode in ['probe','leader','stubborn']:
 child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])
with open('pids.json','w') as f:json.dump([os.getpid(),child.pid if child else None],f)
def out(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 method=r.get('method')
 if 'id' not in r:continue
 if method=='initialize':
  if mode=='probe':time.sleep(60)
  if mode=='eof':os.close(1);os.close(2);time.sleep(60)
  if mode=='leader':sys.exit(0)
  result={}
 elif method in ['thread/start','thread/resume']:result={'thread':{'id':'s','model':'fixture'}}
 elif method=='turn/start':result={'turn':{'id':'t'}}
 elif method=='account/rateLimits/read':result={'rateLimits':{'primary':{'usedPercent':1,'windowDurationMins':300,'resetsAt':1800000000}}}
 else:result={}
 out({'id':r['id'],'result':result})
 if mode=='writer' and method=='thread/start':time.sleep(60)
 if method=='turn/start' and r['params']['input'][0]['text']!='slow':
  out({'method':'turn/completed','params':{'threadId':'s','turn':{'id':'t','status':'completed'}}})
 if method=='turn/interrupt' and mode!='stop-stuck':
  time.sleep(0.3)
  out({'method':'turn/completed','params':{'threadId':'s','turn':{'id':'t','status':'interrupted'}}})
if mode in ['probe','eof','leader','stubborn','writer']:
 while True:time.sleep(60)
";
fn fixture(mode: &str) -> (tempfile::TempDir, Arc<CodexDriver>) {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("runtime");
    std::fs::write(&executable, FIXTURE.replace("__MODE__", mode)).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver =
        Arc::new(CodexDriver::new(&executable, "fixture".into(), "fixture".into()).unwrap());
    (root, driver)
}
fn workspace(root: &Path) -> SessionRequest {
    SessionRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: None,
    }
}
fn prompt(root: &Path, text: &str) -> PromptRequest {
    PromptRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        attachments: vec![],
        permission_mode: None,
        context: vec![],
        prompt: text.into(),
    }
}
fn count(root: &Path, method: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v["method"] == method)
        .count()
}
fn ids(root: &Path) -> Vec<u32> {
    serde_json::from_slice::<Vec<Option<u32>>>(&std::fs::read(root.join("pids.json")).unwrap())
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}
fn present(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim() == pid.to_string()
}
async fn wait_for(mut predicate: impl FnMut() -> bool) {
    timeout(Duration::from_secs(8), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_cancelled_initialization_probe_still_cleans_its_original_scope() {
    let (root, driver) = fixture("probe");
    let executable = driver.executable.clone();
    let probe = tokio::spawn(async move { CodexDriver::probe_ready(&executable).await });
    wait_for(|| count(root.path(), "initialize") == 1).await;
    let ids = ids(root.path());
    probe.abort();
    let _ = probe.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}

#[tokio::test]
async fn eof_and_an_exited_leader_with_inherited_pipes_do_not_leave_a_native_scope_alive() {
    for mode in ["eof", "leader"] {
        let (root, driver) = fixture(mode);
        assert!(Connection::spawn(&driver.executable).await.is_err());
        wait_for(|| ids(root.path()).iter().all(|id| !present(*id))).await;
    }
}

#[tokio::test]
async fn dropping_the_driver_retires_a_worker_even_while_its_prompt_write_is_blocked() {
    let (root, driver) = fixture("writer");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let native = driver.sessions.lock().await["s"].native.clone().unwrap();
    let sender = driver.command_sender("s").await.unwrap();
    let (response, received) = oneshot::channel();
    sender
        .send(SessionCommand::Prompt {
            request: prompt(root.path(), &"x".repeat(2 * 1024 * 1024)),
            response,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(driver);
    native.closed().await.unwrap();
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
    assert!(matches!(received.await, Ok(Err(_)) | Err(_)));
    drop(session);
}

#[tokio::test]
async fn cancelled_shutdown_waiting_keeps_the_original_scope_until_exit_is_confirmed() {
    let (root, driver) = fixture("stubborn");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let native = driver.sessions.lock().await["s"].native.clone().unwrap();
    let shutting = tokio::spawn({
        let driver = driver.clone();
        let session = session.clone();
        async move { driver.shutdown(session).await }
    });
    wait_for(|| native.stopping()).await;
    shutting.abort();
    let _ = shutting.await;
    assert!(driver.sessions.lock().await.contains_key("s"));
    native.closed().await.unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(!driver.sessions.lock().await.contains_key("s"));
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}

#[tokio::test]
async fn a_cancelled_interrupt_waiter_joins_one_request_until_the_real_terminal() {
    let (root, driver) = fixture("normal");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let stopping = tokio::spawn({
        let driver = driver.clone();
        let session = session.clone();
        async move { driver.interrupt(&session).await }
    });
    wait_for(|| count(root.path(), "turn/interrupt") == 1).await;
    let returned_before_terminal = stopping.is_finished();
    stopping.abort();
    let _ = stopping.await;
    let retried = driver.interrupt(&session).await;
    let events = timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    let native = driver.sessions.lock().await["s"].native.clone().unwrap();
    driver.shutdown(session).await.unwrap();
    native.closed().await.unwrap();
    assert!(!returned_before_terminal);
    retried.unwrap();
    assert_eq!(count(root.path(), "turn/interrupt"), 1);
    assert!(
        events
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnInterrupted { .. })))
    );
}

#[tokio::test]
async fn a_stop_ack_without_a_real_terminal_does_not_report_success() {
    let (root, driver) = fixture("stop-stuck");
    let mut connection = Connection::spawn(&driver.executable).await.unwrap();
    connection.thread_id = "s".into();
    connection.stop_timeout = Duration::from_millis(100);
    let native = connection.native.clone();
    driver
        .install_session("s".into(), connection)
        .await
        .unwrap();
    let session = SessionHandle {
        native_session_id: "s".into(),
    };
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let stopped = driver.interrupt(&session).await;
    // Both success and failure paths clean up this private fixture.
    native.request_close();
    native.closed().await.unwrap();
    driver.shutdown(session).await.unwrap();
    let events = timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(stopped.is_err());
    assert!(!events.iter().any(|v| matches!(
        v,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnInterrupted { .. })
    )));
}

#[tokio::test]
async fn dropping_an_active_core_event_stream_retires_its_unobserved_native_turn() {
    let (root, driver) = fixture("stubborn");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let native = driver.sessions.lock().await["s"].native.clone().unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    drop(stream);
    wait_for(|| native.stopping()).await;
    native.closed().await.unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}

#[tokio::test]
async fn a_cancelled_queued_prompt_is_not_dispatched_after_admission() {
    let (root, driver) = fixture("normal");
    let mut connection = Connection::spawn(&driver.executable).await.unwrap();
    connection.thread_id = "s".into();
    let native = connection.native.clone();
    let (sender, commands) = mpsc::channel(4);
    let (response, received) = oneshot::channel();
    sender
        .send(SessionCommand::Prompt {
            request: prompt(root.path(), "must not run"),
            response,
        })
        .await
        .unwrap();
    drop(received);
    let (response, ended) = oneshot::channel();
    sender
        .send(SessionCommand::Shutdown { response })
        .await
        .unwrap();
    session_actor(connection, commands).await;
    ended.await.unwrap();
    native.closed().await.unwrap();
    assert_eq!(count(root.path(), "turn/start"), 0);
}

#[tokio::test]
async fn a_closed_actor_channel_is_not_a_substitute_for_native_exit_proof() {
    let (root, driver) = fixture("stubborn");
    let connection = Connection::spawn(&driver.executable).await.unwrap();
    let native = connection.native.clone();
    let (commands, receiver) = mpsc::channel(4);
    drop(receiver);
    driver.sessions.lock().await.insert(
        "s".into(),
        ResidentSession {
            commands,
            native: Some(native.clone()),
        },
    );
    driver
        .shutdown(SessionHandle {
            native_session_id: "s".into(),
        })
        .await
        .unwrap();
    native.closed().await.unwrap();
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
    assert!(driver.sessions.lock().await.is_empty());
    drop(connection);
}

#[tokio::test]
async fn restoring_a_closed_actor_waits_for_the_original_scope_before_spawning_a_replacement() {
    let (root, driver) = fixture("stubborn");
    let connection = Connection::spawn(&driver.executable).await.unwrap();
    let old_ids = ids(root.path());
    let native = connection.native.clone();
    let (commands, receiver) = mpsc::channel(4);
    drop(receiver);
    driver.sessions.lock().await.insert(
        "s".into(),
        ResidentSession {
            commands,
            native: Some(native.clone()),
        },
    );
    let restored = driver
        .resume_selected_session("s", workspace(root.path()), None, None)
        .await
        .unwrap();
    assert!(old_ids.iter().all(|id| !present(*id)));
    let replacement = driver.sessions.lock().await["s"].native.clone().unwrap();
    assert!(!Arc::ptr_eq(&replacement, &native));
    drop(connection);
    assert!(!replacement.stopping());
    driver.shutdown(restored).await.unwrap();
    replacement.closed().await.unwrap();
}

#[tokio::test]
async fn dropping_a_driver_with_a_cached_quota_connection_does_not_keep_a_reference_cycle_alive() {
    let (root, driver) = fixture("stubborn");
    driver.read_quota(None).await.unwrap();
    let native = driver
        .quota
        .connection
        .lock()
        .await
        .as_ref()
        .unwrap()
        .native
        .clone();
    let quota = Arc::downgrade(&driver.quota);
    drop(driver);
    let released = quota.upgrade().is_none();
    // Always clean up our fixture even when checking the pre-fix cycle.
    if !released {
        if let Some(state) = quota.upgrade() {
            state.connection.lock().await.take();
        }
        native.request_close();
    }
    native.closed().await.unwrap();
    assert!(released);
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
