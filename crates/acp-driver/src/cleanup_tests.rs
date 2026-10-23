//! Only private Python ACP runtimes and isolated process scopes.
use super::*;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
const FIXTURE: &str = r"#!/usr/bin/env python3
import json,os,sys,signal,subprocess,time
os.chdir(os.path.dirname(os.path.abspath(__file__)))
mode='__MODE__'
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=None
if mode in ['probe','leader','stubborn','usage']:
 child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])
with open('pids.json','w') as f:json.dump([os.getpid(),child.pid if child else None],f)
if mode=='usage':time.sleep(60)
sessions=0;pending=None
def out(v):print(json.dumps(v),flush=True)
def reply(r,v):out({'jsonrpc':'2.0','id':r['id'],'result':v})
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 method=r.get('method')
 if 'id' not in r:
  if method=='session/cancel' and pending and mode!='stop-stuck':
   if mode=='delayed-stop':time.sleep(0.3)
   reply(pending,{'stopReason':'cancelled'});pending=None
  continue
 if method=='initialize':
  if mode=='probe':time.sleep(60)
  if mode=='eof':os.close(1);os.close(2);time.sleep(60)
  if mode=='leader':sys.exit(0)
  if mode=='oversize':sys.stdout.write('x'*(8*1024*1024+1));sys.stdout.flush();time.sleep(60)
  reply(r,{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'authMethods':[]})
 elif method=='session/new':
  if mode=='blocked-meta' and sessions>0:
   while not os.path.exists('metadata-ready'):time.sleep(0.01)
  sessions+=1
  reply(r,{'sessionId':'s-'+str(sessions),'models':{'currentModelId':'fixture','availableModels':[{'modelId':'fixture','name':'Fixture'}]}})
 elif method=='session/load':reply(r,{'models':{'currentModelId':'fixture'}})
 elif method=='session/prompt':
  if mode=='permission':
   pending=r
   out({'jsonrpc':'2.0','id':'permission-fixture','method':'session/request_permission','params':{'sessionId':r['params']['sessionId'],'toolCall':{'toolCallId':'tool-1','title':'Fixture','kind':'execute'},'options':[{'optionId':'allow','name':'Allow once','kind':'allow_once'}]}})
  elif r['params']['prompt'][0]['text']=='slow':pending=r
  else:reply(r,{'stopReason':'end_turn'})
 else:reply(r,{})
if mode in ['probe','eof','leader','stubborn','usage']:
 while True:time.sleep(60)
";
fn fixture(mode: &str) -> (tempfile::TempDir, Arc<AcpDriver>) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("runtime");
    std::fs::write(&file, FIXTURE.replace("__MODE__", mode)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = AcpDriver::new(
        "cursor",
        &agent_domain::InstallationId::new().to_string(),
        AcpLaunch {
            executable: file,
            args: vec![],
        },
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    (root, Arc::new(driver))
}
fn request(root: &Path) -> SessionRequest {
    SessionRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: None,
    }
}
fn prompt(root: &Path, text: &str) -> HubPromptRequest {
    HubPromptRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        context: vec![],
        attachments: vec![],
        permission_mode: None,
        prompt: text.into(),
    }
}
fn count(root: &Path, method: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
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
async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(9), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn original(driver: &AcpDriver) -> (Arc<lifecycle::Control>, Arc<AtomicBool>) {
    let guard = driver.shared.connection.lock().await;
    let h = guard.as_ref().unwrap();
    (h.native.clone().unwrap(), h.finished.clone())
}

#[tokio::test]
async fn cancelled_probe_still_reaps_its_original_process_and_descendant() {
    let (root, driver) = fixture("probe");
    let launch = driver.shared.launch.clone();
    let probing = tokio::spawn(async move { AcpDriver::probe_ready(&launch).await });
    wait_for(|| count(root.path(), "initialize") == 1).await;
    let ids = ids(root.path());
    probing.abort();
    let _ = probing.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}
#[tokio::test]
async fn eof_and_a_dead_leader_with_inherited_pipes_cannot_leave_the_scope_alive() {
    for mode in ["eof", "leader"] {
        let (root, driver) = fixture(mode);
        assert!(AcpDriver::probe_ready(&driver.shared.launch).await.is_err());
        assert!(ids(root.path()).iter().all(|id| !present(*id)));
    }
}
#[tokio::test]
async fn cancelled_shutdown_waiter_retains_a_marker_until_the_original_scope_is_gone() {
    let (root, driver) = fixture("stubborn");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let (native, finished) = original(&driver).await;
    let closing = tokio::spawn({
        let driver = driver.clone();
        let session = session.clone();
        async move { driver.shutdown(session).await }
    });
    wait_for(|| native.stopping()).await;
    closing.abort();
    let _ = closing.await;
    assert!(driver.shared.connection.lock().await.is_some());
    assert!(!finished.load(Ordering::Acquire));
    driver.shutdown(session).await.unwrap();
    assert!(finished.load(Ordering::Acquire));
    assert!(driver.shared.connection.lock().await.is_none());
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
#[tokio::test]
async fn driver_drop_cleans_a_live_prompt_even_when_the_terminal_waiter_holds_shared_state() {
    let (root, driver) = fixture("stubborn");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let (native, finished) = original(&driver).await;
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    drop(driver);
    native.closed().await.unwrap();
    wait_for(|| finished.load(Ordering::Acquire)).await;
    let events = tokio::time::timeout(Duration::from_secs(2), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnFailed { .. })))
    );
    assert!(
        !events
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnCompleted { .. })))
    );
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
#[tokio::test]
async fn closing_a_different_handle_cannot_kill_an_active_session_on_the_shared_connection() {
    let (root, driver) = fixture("normal");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, _) = original(&driver).await;
    let stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    assert!(matches!(
        driver.shutdown(a).await,
        Err(DriverError::CapabilityUnavailable(_))
    ));
    assert!(!native.stopping());
    assert!(ids(root.path()).iter().all(|id| present(*id)));
    driver.shutdown(b).await.unwrap();
    drop(stream);
}
#[tokio::test]
async fn close_waits_for_permission_and_sdk_tasks_to_release_the_original_connection() {
    let (root, driver) = fixture("permission");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let (_, finished) = original(&driver).await;
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = stream.next().await {
            if matches!(event, Ok(DriverEvent::ApprovalRequested { .. })) {
                return;
            }
        }
        panic!("permission stream ended before the fixture request");
    })
    .await;
    driver.shutdown(session).await.unwrap();
    approval.unwrap();
    assert!(finished.load(Ordering::Acquire));
    assert!(driver.shared.pending_permissions.lock().unwrap().is_empty());
    assert!(driver.shared.questions.lock().unwrap().is_empty());
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
#[tokio::test]
async fn a_closed_handle_cannot_spawn_and_dispatch_until_explicit_native_restore() {
    let (root, driver) = fixture("normal");
    let session = driver.create_session(request(root.path())).await.unwrap();
    driver.shutdown(session.clone()).await.unwrap();
    assert!(
        driver
            .prompt(&session, prompt(root.path(), "must not dispatch"))
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "initialize"), 1);
    assert_eq!(count(root.path(), "session/prompt"), 0);
    driver
        .resume_session(&session.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "resumed"))
        .await
        .unwrap();
    let events = stream.collect::<Vec<_>>().await;
    assert!(
        events
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(session).await.unwrap();
}
#[tokio::test]
async fn an_oversized_unterminated_frame_is_stopped_before_the_sdk_accumulates_it_forever() {
    let (root, driver) = fixture("oversize");
    assert!(AcpDriver::probe_ready(&driver.shared.launch).await.is_err());
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
#[tokio::test]
async fn cancelled_kimi_usage_startup_keeps_cleanup_of_its_private_helper_scope() {
    let (root, driver) = fixture("usage");
    let executable = driver.shared.launch.executable.clone();
    let query = tokio::spawn(async move { read_kimi_web_usage(&executable).await });
    wait_for(|| root.path().join("pids.json").exists()).await;
    let ids = ids(root.path());
    query.abort();
    let _ = query.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}

#[tokio::test]
async fn callback_jobs_are_joined_on_shutdown_and_late_jobs_cannot_restart_the_group() {
    struct Released(Arc<AtomicBool>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let group = lifecycle::Tasks::new();
    let released = Arc::new(AtomicBool::new(false));
    let owned = Released(released.clone());
    let (started, running) = oneshot::channel();
    group.spawn(async move {
        let _owned = owned;
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    group.shutdown().await;
    assert!(released.load(Ordering::Acquire));
    let late = Arc::new(AtomicBool::new(false));
    let owned = Released(late.clone());
    group.spawn(async move {
        let _owned = owned;
        panic!("a retired group must not execute late jobs");
    });
    assert!(late.load(Ordering::Acquire));
}

#[tokio::test]
async fn cancelled_stop_waiters_join_one_cancel_and_wait_for_the_real_prompt_response(){
    let(root,driver)=fixture("delayed-stop");let session=driver.create_session(request(root.path())).await.unwrap();
    let stream=driver.prompt(&session,prompt(root.path(),"slow")).await.unwrap();
    wait_for(||count(root.path(),"session/prompt")==1).await;
    let first=tokio::spawn({let driver=driver.clone();let session=session.clone();async move{driver.interrupt(&session).await}});
    wait_for(||count(root.path(),"session/cancel")==1).await;
    let early=first.is_finished();first.abort();let _=first.await;
    let retried=driver.interrupt(&session).await;
    let events=tokio::time::timeout(Duration::from_secs(3),stream.collect::<Vec<_>>()).await.unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(!early);retried.unwrap();assert_eq!(count(root.path(),"session/cancel"),1);
    assert!(events.iter().any(|v|matches!(v,Ok(DriverEvent::TurnInterrupted{..}))));
}

#[tokio::test]
async fn an_ignored_cancel_cannot_report_success_or_a_completed_turn(){
    let(root,mut driver)=fixture("stop-stuck");Arc::get_mut(&mut driver).unwrap().stop_timeout=Duration::from_millis(100);
    let session=driver.create_session(request(root.path())).await.unwrap();
    let stream=driver.prompt(&session,prompt(root.path(),"slow")).await.unwrap();
    wait_for(||count(root.path(),"session/prompt")==1).await;
    let result=driver.interrupt(&session).await;
    driver.shutdown(session).await.unwrap();
    let events=tokio::time::timeout(Duration::from_secs(3),stream.collect::<Vec<_>>()).await.unwrap();
    assert!(result.is_err());assert_eq!(count(root.path(),"session/cancel"),1);
    assert!(!events.iter().any(|v|matches!(v,Ok(DriverEvent::TurnCompleted{..}|DriverEvent::TurnInterrupted{..}))));
}

#[tokio::test]
async fn a_stop_for_a_different_session_does_not_change_the_running_turn(){
    let(root,driver)=fixture("normal");let a=driver.create_session(request(root.path())).await.unwrap();let b=driver.create_session(request(root.path())).await.unwrap();
    let stream=driver.prompt(&b,prompt(root.path(),"slow")).await.unwrap();
    wait_for(||count(root.path(),"session/prompt")==1).await;
    let wrong=driver.interrupt(&a).await;
    let affected=driver.shared.interrupt_requested.load(Ordering::Acquire);
    driver.interrupt(&b).await.unwrap();
    let _=tokio::time::timeout(Duration::from_secs(3),stream.collect::<Vec<_>>()).await.unwrap();
    driver.shutdown(b).await.unwrap();
    assert!(wrong.is_err());assert!(!affected);assert_eq!(count(root.path(),"session/cancel"),1);
}

#[tokio::test]
async fn stopping_a_queued_prompt_prevents_dispatch_even_if_cancel_arrives_before_the_command(){
    let(root,driver)=fixture("blocked-meta");let session=driver.create_session(request(root.path())).await.unwrap();
    let metadata=tokio::spawn({let driver=driver.clone();async move{driver.model_catalog().await}});
    wait_for(||count(root.path(),"session/new")==2).await;
    let stream=driver.prompt(&session,prompt(root.path(),"slow")).await.unwrap();
    let stopping=tokio::spawn({let driver=driver.clone();let session=session.clone();async move{driver.interrupt(&session).await}});
    wait_for(||driver.shared.interrupt_requested.load(Ordering::Acquire)).await;
    std::fs::write(root.path().join("metadata-ready"),b"").unwrap();
    let stopped=stopping.await.unwrap();let _=metadata.await.unwrap();
    driver.shutdown(session).await.unwrap();
    let _=tokio::time::timeout(Duration::from_secs(3),stream.collect::<Vec<_>>()).await.unwrap();
    assert_eq!(count(root.path(),"session/prompt"),0);stopped.unwrap();
}
