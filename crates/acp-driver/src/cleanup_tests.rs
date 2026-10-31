//! Only private Python ACP runtimes and isolated process scopes.
use super::*;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
const FIXTURE: &str = r"#!/usr/bin/env python3
import json,os,sys,signal,subprocess,time,threading
os.chdir(os.path.dirname(os.path.abspath(__file__)))
mode='__MODE__'
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=None
if mode in ['probe','leader','stubborn','usage','usage-http']:
 child=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])
pid_marker='pids.'+str(os.getpid())+'.tmp'
with open(pid_marker,'w') as f:json.dump([os.getpid(),child.pid if child else None],f)
os.replace(pid_marker,'pids.json')
if mode=='usage':time.sleep(60)
if mode=='usage-http':
 from http.server import BaseHTTPRequestHandler,HTTPServer
 class Usage(BaseHTTPRequestHandler):
  def do_GET(self):
   if self.path!='/api/v1/oauth/usage' or self.headers.get('Authorization')!='Bearer fixture-token':
    self.send_response(400);self.end_headers();return
   body=json.dumps({'data':{'kind':'ok','quota':{'usages':{'limit5h':{'usedRatio':0.25}}}}}).encode()
   self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
  def log_message(self,*args):pass
 server=HTTPServer(('127.0.0.1',0),Usage)
 print('http://127.0.0.1:'+str(server.server_port)+'/#token=fixture-token',flush=True)
 server.serve_forever()
sessions=0;pending=None;output_lock=threading.Lock()
def out(v):
 with output_lock:print(json.dumps(v),flush=True)
def reply(r,v):out({'jsonrpc':'2.0','id':r['id'],'result':v})
def permission(session):out({'jsonrpc':'2.0','id':'permission-fixture','method':'session/request_permission','params':{'sessionId':session,'toolCall':{'toolCallId':'tool-1','title':'Fixture','kind':'execute'},'options':[{'optionId':'allow','name':'Allow once','kind':'allow_once'}]}})
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(dict(r,pid=os.getpid()))+'\n')
 method=r.get('method')
 if method is None:
  if mode=='codebuddy-bridge' and pending and r.get('id')=='codebuddy-'+pending['params']['prompt'][0]['text']:
   reply(pending,{'stopReason':'end_turn'});pending=None
  if r.get('id')=='task-fixture' and pending:reply(pending,{'stopReason':'end_turn'});pending=None
  if r.get('id')=='permission-fixture' and pending:
   selected=r.get('result',{}).get('outcome',{}).get('outcome')=='selected'
   reply(pending,{'stopReason':'end_turn' if selected else 'cancelled'});pending=None
  if r.get('id')=='question-fixture' and pending:
   submitted=r.get('result',{}).get('outcome',{}).get('outcome')=='answered'
   reply(pending,{'stopReason':'end_turn' if submitted else 'cancelled'});pending=None
  continue
 if 'id' not in r:
  if method=='session/cancel' and pending and mode!='stop-stuck' and not (mode.startswith('native-close') and pending['params']['sessionId']=='s-1'):
   if mode=='late-permission':permission(r['params']['sessionId'])
   if mode=='delayed-stop':time.sleep(0.3)
   reply(pending,{'stopReason':'cancelled'});pending=None
  continue
 if method=='initialize':
  if mode=='probe':time.sleep(60)
  if mode=='eof':os.close(1);os.close(2);time.sleep(60)
  if mode=='leader':sys.exit(0)
  if mode=='oversize':sys.stdout.write('x'*(8*1024*1024+1));sys.stdout.flush();time.sleep(60)
  caps={'loadSession':True}
  if mode.startswith('native-close'):caps['sessionCapabilities']={'close':{}}
  reply(r,{'protocolVersion':1,'agentCapabilities':caps,'authMethods':[]})
 elif method=='session/new':
  if (mode=='blocked-meta' and sessions>0) or (mode=='blocked-catalog' and sessions>1):
   while not os.path.exists('metadata-ready'):time.sleep(0.01)
  sessions+=1
  reply(r,{'sessionId':'s-1' if mode=='duplicate-new' else 's-'+str(sessions),'models':{'currentModelId':'fixture','availableModels':[{'modelId':'fixture','name':'Fixture'}]}})
 elif method=='session/load':
  if mode=='blocked-load':
   while not os.path.exists('load-ready'):time.sleep(0.01)
  reply(r,{'models':{'currentModelId':'fixture'}})
 elif method in ['_kiro/account/getUsage','_x.ai/billing']:
  if os.path.exists('quota-failure'):
   with open('quota-failure') as f:message=f.read()
   out({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32000,'message':message}})
  elif mode=='aux-account-blocked':
   def account(request):
    while not os.path.exists('quota-ready'):time.sleep(0.01)
    reply(request,{})
   threading.Thread(target=account,args=(r,),daemon=True).start()
  else:
   out({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'s-1','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'quota-only'}}}})
   reply(r,{'data':{'planName':'Fixture','usageBreakdowns':[{'resourceType':'CREDIT','used':1,'limit':50}]},'config':{'currentPeriod':{'start':'2026-10-01T00:00:00Z','end':'2026-11-01T00:00:00Z'},'creditUsagePercent':25}})
 elif method=='_x.ai/session/usage':
  if pending:reply(pending,{'stopReason':'end_turn'});pending=None
  def cost(request):
   while not os.path.exists('cost-ready'):time.sleep(0.01)
   reply(request,{'totalCost':99})
  threading.Thread(target=cost,args=(r,),daemon=True).start()
 elif method=='_kiro/session/context':
  def context(request):
   while not os.path.exists('context-ready'):time.sleep(0.01)
   reply(request,{'contextUsage':{'usagePercentage':7}})
  threading.Thread(target=context,args=(r,),daemon=True).start()
 elif method=='_codebuddy.ai/resolveInterruption':
  def resolve(request):
   while not os.path.exists('resolve-ready'):time.sleep(0.01)
   with open('resolve-ready') as f:resolved=f.read()=='yes'
   reply(request,{'resolved':resolved})
  threading.Thread(target=resolve,args=(r,),daemon=True).start()
 elif method=='session/prompt':
  if mode=='foreign-update':
   for session,text in [('s-1','stale-other-session'),(r['params']['sessionId'],'current-session')]:
    out({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':session,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
  if mode in ['permission','foreign-permission']:
   pending=r
   permission('not-the-active-session' if mode=='foreign-permission' else r['params']['sessionId'])
  elif mode=='codebuddy-bridge':
   pending=r
   text=r['params']['prompt'][0]['text']
   out({'jsonrpc':'2.0','id':'codebuddy-'+text,'method':'session/request_permission','params':{'sessionId':r['params']['sessionId'],'toolCall':{'toolCallId':'reused-tool','title':'AskUserQuestion','kind':'other','rawInput':{'questions':[{'id':'q','question':'Choose','options':[{'label':'Yes'}]}]},'_meta':{'codebuddy.ai/toolName':'AskUserQuestion'}},'options':[{'optionId':'allow','name':'Allow once','kind':'allow_once'}]}})
   if text=='first':
    def finish(request):
     while not os.path.exists('finish-first'):time.sleep(0.01)
     reply(request,{'stopReason':'end_turn'})
    threading.Thread(target=finish,args=(r,),daemon=True).start()
  elif mode in ['question','unscoped-question']:
   pending=r
   params={'questions':[{'id':'q','prompt':'Choose','options':[{'id':'yes','label':'Yes'}]}]}
   if mode=='question':params['sessionId']=r['params']['sessionId']
   out({'jsonrpc':'2.0','id':'question-fixture','method':'cursor/ask_question','params':params})
  elif r['params']['prompt'][0]['text'] in ['ambiguous','scoped']:
   pending=r
   params={'questions':[{'id':'q','prompt':'Choose','options':[{'id':'yes','label':'Yes'}]}]}
   if r['params']['prompt'][0]['text']=='scoped':params['sessionId']=r['params']['sessionId']
   out({'jsonrpc':'2.0','id':'question-fixture','method':'cursor/ask_question','params':params})
  elif r['params']['prompt'][0]['text']=='grok-cost':
   pending=r
   out({'jsonrpc':'2.0','method':'_x.ai/session_notification','params':{'sessionId':r['params']['sessionId'],'update':{'sessionUpdate':'response_completed','message_id':'cost-fixture','usage':{'input_tokens':80,'cache_read_input_tokens':20,'cache_creation_input_tokens':0,'output_tokens':10,'reasoning_tokens':5}}}})
  elif r['params']['prompt'][0]['text']=='foreign-task':
   pending=r
   out({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':r['params']['sessionId'],'update':{'sessionUpdate':'tool_call','toolCallId':'task-1','title':'Task','kind':'other','status':'completed','rawInput':{'_toolName':'task'},'rawOutput':{'result':'fixture'}}}})
   out({'jsonrpc':'2.0','id':'task-fixture','method':'cursor/task','params':{'sessionId':'s-1','toolCallId':'task-1'}})
  elif r['params']['prompt'][0]['text']=='slow':pending=r
  else:reply(r,{'stopReason':'end_turn'})
 elif method=='session/close':
  if mode=='native-close-error':out({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32000,'message':'fixture rejected close'}})
  elif mode=='native-close-malformed':reply(r,'not-an-object')
  elif mode=='native-close-stuck':pass
  elif mode=='native-close-delayed':
   def delayed(request):
    while not os.path.exists('close-ready'):time.sleep(0.01)
    reply(request,{})
   threading.Thread(target=delayed,args=(r,),daemon=True).start()
  else:reply(r,{})
 elif method=='session/set_model' and mode=='fail-model':reply(r,{'_meta':{'model':{'Err':'fixture selection could not be confirmed'}}})
 else:reply(r,{})
if mode in ['probe','eof','leader','stubborn','usage']:
 while True:time.sleep(60)
";
fn fixture(mode: &str) -> (tempfile::TempDir, Arc<AcpDriver>) {
    fixture_as(mode, "cursor")
}
fn fixture_as(mode: &str, provider: &str) -> (tempfile::TempDir, Arc<AcpDriver>) {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("runtime");
    std::fs::write(&file, FIXTURE.replace("__MODE__", mode)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = AcpDriver::new(
        provider,
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
        app_mcps: vec![],
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
    let closed = driver.shutdown(a).await;
    let preserved = !native.stopping() && ids(root.path()).iter().all(|id| present(*id));
    driver.shutdown(b).await.unwrap();
    drop(stream);
    closed.unwrap();
    assert!(preserved);
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
    let session = driver
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
    let permit = driver.shared.metadata.clone().lock_owned().await;
    let query = tokio::spawn(async move { read_kimi_web_usage(&executable, permit).await });
    wait_for(|| root.path().join("pids.json").exists()).await;
    let ids = ids(root.path());
    query.abort();
    let _ = query.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}

#[tokio::test]
async fn cancelled_kimi_quota_keeps_query_admission_until_its_original_tree_exits() {
    let (root, driver) = fixture_as("usage", "kimi");
    let query = tokio::spawn({
        let driver = driver.clone();
        async move { driver.read_quota_with_refresh(None, true).await }
    });
    wait_for(|| root.path().join("pids.json").exists()).await;
    let ids = ids(root.path());
    let held_during_query = driver.shared.metadata.try_lock().is_err();
    query.abort();
    let _ = query.await;
    let permit = tokio::time::timeout(
        Duration::from_secs(9),
        driver.shared.metadata.clone().lock_owned(),
    )
    .await
    .expect("cancelled quota helper must settle its original query");
    let exited_before_reuse = ids.iter().all(|id| !present(*id));
    // Let the old implementation finish its supervised cleanup before the
    // negative assertions, so this test never strands its private processes.
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
    assert!(driver.shared.connection.lock().await.is_none());
    assert!(
        driver
            .shared
            .quota
            .account_snapshot
            .lock()
            .unwrap()
            .is_none()
    );
    assert!(held_during_query, "quota bypassed helper query admission");
    assert!(
        exited_before_reuse,
        "query slot released before helper exit"
    );
    drop(permit);
}

#[tokio::test]
async fn kimi_quota_success_returns_after_helper_exit_and_preserves_account_cache() {
    let (root, driver) = fixture_as("usage-http", "kimi");
    let snapshot = driver.read_quota_with_refresh(None, true).await.unwrap();
    let original = ids(root.path());
    assert!(original.iter().all(|id| !present(*id)));
    assert!(driver.shared.metadata.try_lock().is_ok());
    assert!(driver.shared.connection.lock().await.is_none());
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].used_percent, Some(25.0));
    let cached = driver.read_quota(None).await.unwrap();
    assert_eq!(cached.collected_at_unix_ms, snapshot.collected_at_unix_ms);
    assert_eq!(ids(root.path()), original, "fresh quota restarted a helper");
}

#[tokio::test]
async fn kimi_quota_cancelled_while_waiting_for_admission_does_not_launch_later() {
    let (root, driver) = fixture_as("usage-http", "kimi");
    let permit = driver.shared.metadata.clone().lock_owned().await;
    let mut query = Box::pin(driver.read_quota_with_refresh(None, true));
    assert!(futures::poll!(query.as_mut()).is_pending());
    drop(query);
    drop(permit);
    let _permit = driver.shared.metadata.lock().await;
    let _refresh = driver.shared.quota.refresh.lock().await;
    assert!(!root.path().join("pids.json").exists());
    assert!(
        driver
            .shared
            .quota
            .account_snapshot
            .lock()
            .unwrap()
            .is_none()
    );
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
async fn cancelled_stop_waiters_join_one_cancel_and_wait_for_the_real_prompt_response() {
    let (root, driver) = fixture("delayed-stop");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let first = tokio::spawn({
        let driver = driver.clone();
        let session = session.clone();
        async move { driver.interrupt(&session).await }
    });
    wait_for(|| count(root.path(), "session/cancel") == 1).await;
    let early = first.is_finished();
    first.abort();
    let _ = first.await;
    let retried = driver.interrupt(&session).await;
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(!early);
    retried.unwrap();
    assert_eq!(count(root.path(), "session/cancel"), 1);
    assert!(
        events
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnInterrupted { .. })))
    );
}

#[tokio::test]
async fn an_ignored_cancel_cannot_report_success_or_a_completed_turn() {
    let (root, mut driver) = fixture("stop-stuck");
    Arc::get_mut(&mut driver).unwrap().stop_timeout = Duration::from_millis(100);
    let session = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let result = driver.interrupt(&session).await;
    driver.shutdown(session).await.unwrap();
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(count(root.path(), "session/cancel"), 1);
    assert!(!events.iter().any(|v| matches!(
        v,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnInterrupted { .. })
    )));
}

#[tokio::test]
async fn a_stop_for_a_different_session_does_not_change_the_running_turn() {
    let (root, driver) = fixture("normal");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let wrong = driver.interrupt(&a).await;
    let affected = driver.shared.interrupt_requested.load(Ordering::Acquire);
    driver.interrupt(&b).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(b).await.unwrap();
    assert!(wrong.is_err());
    assert!(!affected);
    assert_eq!(count(root.path(), "session/cancel"), 1);
}

async fn original_catalog_barrier(
    driver: &AcpDriver,
) -> oneshot::Receiver<Result<driver_api::ModelCatalog, DriverError>> {
    let connection = driver.ensure_connection().await.unwrap();
    let (reply, response) = oneshot::channel();
    connection
        .commands
        .send(Command::Catalog {
            model: None,
            workspace: None,
            reply,
        })
        .unwrap();
    response
}

#[tokio::test]
async fn stopping_a_queued_prompt_prevents_dispatch_even_if_cancel_arrives_before_the_command() {
    let (root, driver) = fixture("blocked-meta");
    let session = driver.create_session(request(root.path())).await.unwrap();
    // Force SDK queue backpressure directly. Public metadata now owns a
    // different process; this retains the low-level queued-cancel invariant.
    let metadata = original_catalog_barrier(&driver).await;
    wait_for(|| count(root.path(), "session/new") == 2).await;
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let stopping = tokio::spawn({
        let driver = driver.clone();
        let session = session.clone();
        async move { driver.interrupt(&session).await }
    });
    wait_for(|| driver.shared.interrupt_requested.load(Ordering::Acquire)).await;
    std::fs::write(root.path().join("metadata-ready"), b"").unwrap();
    let stopped = stopping.await.unwrap();
    let _ = metadata.await.unwrap();
    driver.shutdown(session).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/prompt"), 0);
    stopped.unwrap();
}

async fn approval_id(stream: &mut DriverEventStream) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = stream.next().await {
            if let Ok(DriverEvent::ApprovalRequested {
                native_request_id, ..
            }) = event
            {
                return native_request_id;
            }
        }
        panic!("approval fixture ended before asking");
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn an_approval_cannot_be_answered_with_another_session_handle() {
    let (root, driver) = fixture("permission");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let id = approval_id(&mut stream).await;
    let wrong = driver.respond_to_interaction(&a, &id, "allow").await;
    let retained = driver
        .shared
        .pending_permissions
        .lock()
        .unwrap()
        .contains_key(&id);
    let _ = driver.interrupt(&b).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(b).await.unwrap();
    assert!(wrong.is_err());
    assert!(retained);
}

#[tokio::test]
async fn an_old_connection_approval_id_cannot_answer_a_restored_session_request() {
    let (root, driver) = fixture("permission");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut old = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let old_id = approval_id(&mut old).await;
    driver.shutdown(session.clone()).await.unwrap();
    drop(old);
    let session = driver
        .resume_session(&session.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let current = approval_id(&mut stream).await;
    let stale = driver
        .respond_to_interaction(&session, &old_id, "allow")
        .await;
    let retained = driver
        .shared
        .pending_permissions
        .lock()
        .unwrap()
        .contains_key(&current);
    let _ = driver.interrupt(&session).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(session).await.unwrap();
    assert_ne!(old_id, current);
    assert!(stale.is_err());
    assert!(retained);
}

#[tokio::test]
async fn an_invalid_approval_option_preserves_the_request_and_a_valid_answer_is_delivered_once() {
    let (root, driver) = fixture("permission");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let id = approval_id(&mut stream).await;
    let invalid = driver
        .respond_to_interaction(&session, &id, "not-offered")
        .await;
    let retained = driver
        .shared
        .pending_permissions
        .lock()
        .unwrap()
        .contains_key(&id);
    let answered = driver.respond_to_interaction(&session, &id, "allow").await;
    let duplicate = driver.respond_to_interaction(&session, &id, "allow").await;
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(invalid.is_err());
    assert!(retained);
    answered.unwrap();
    assert!(duplicate.is_err());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let replies = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    assert_eq!(
        replies
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["id"] == "permission-fixture"
                && value["result"]["outcome"]["outcome"] == "selected")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_native_permission_for_an_inactive_session_is_cancelled_before_reaching_the_current_ui() {
    let (root, driver) = fixture("foreign-permission");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(session).await.unwrap();
    assert!(!events.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::ApprovalRequested { .. } | DriverEvent::TurnCompleted { .. })
    )));
    assert!(driver.shared.pending_permissions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_permission_arriving_after_stop_is_cancelled_without_opening_a_new_approval() {
    let (root, driver) = fixture("late-permission");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let stopped = driver.interrupt(&session).await;
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    // The callback may be queued after the terminal response, but shutdown must
    // still cancel/join it; it cannot leave a new pending permission behind.
    driver.shutdown(session).await.unwrap();
    stopped.unwrap();
    assert!(!events.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::ApprovalRequested { .. } | DriverEvent::TurnCompleted { .. })
    )));
    assert!(driver.shared.pending_permissions.lock().unwrap().is_empty());
}

async fn question_id(stream: &mut DriverEventStream) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = stream.next().await {
            if let Ok(DriverEvent::QuestionRequested {
                native_request_id, ..
            }) = event
            {
                return native_request_id;
            }
        }
        panic!("question fixture ended before asking");
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn scoped_and_vendor_unscoped_questions_validate_the_active_caller_without_consuming_bad_answers()
 {
    for mode in ["question", "unscoped-question"] {
        let (root, driver) = fixture(mode);
        let a = driver.create_session(request(root.path())).await.unwrap();
        let b = driver.create_session(request(root.path())).await.unwrap();
        let mut stream = driver
            .prompt(&b, prompt(root.path(), "slow"))
            .await
            .unwrap();
        let id = question_id(&mut stream).await;
        let answer = agent_domain::QuestionAnswer {
            cancelled: false,
            values: serde_json::json!({"q": "yes"}),
        };
        let wrong = driver.respond_to_question(&a, &id, answer.clone()).await;
        let invalid = driver
            .respond_to_question(
                &b,
                &id,
                agent_domain::QuestionAnswer {
                    cancelled: false,
                    values: serde_json::json!({"q":"not-offered"}),
                },
            )
            .await;
        let retained = driver.shared.questions.lock().unwrap().contains_key(&id);
        let answered = driver.respond_to_question(&b, &id, answer.clone()).await;
        let duplicate = driver.respond_to_question(&b, &id, answer).await;
        let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
            .await
            .unwrap();
        driver.shutdown(b).await.unwrap();
        assert!(wrong.is_err(), "{mode}");
        assert!(invalid.is_err(), "{mode}");
        assert!(retained, "{mode}");
        answered.unwrap();
        assert!(duplicate.is_err(), "{mode}");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. }))),
            "{mode}"
        );
    }
}

#[tokio::test]
async fn stopping_a_question_cancels_the_native_bridge_and_rejects_late_answers() {
    let (root, driver) = fixture("question");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let id = question_id(&mut stream).await;
    let stopped = driver.interrupt(&session).await;
    let late = driver
        .respond_to_question(
            &session,
            &id,
            agent_domain::QuestionAnswer {
                cancelled: false,
                values: serde_json::json!({"q":"yes"}),
            },
        )
        .await;
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    driver.shutdown(session).await.unwrap();
    stopped.unwrap();
    assert!(late.is_err());
    assert!(driver.shared.questions.lock().unwrap().is_empty());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
}

#[tokio::test]
async fn shared_retirement_of_an_idle_session_preserves_the_other_session_and_requires_explicit_reload()
 {
    let (root, driver) = fixture("normal");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, finished) = original(&driver).await;
    let closed = driver.shutdown(a.clone()).await;
    let preserved = !native.stopping() && !finished.load(Ordering::Acquire);
    let mut success = false;
    let mut reloaded = false;
    if preserved {
        assert!(
            driver
                .prompt(&a, prompt(root.path(), "closed"))
                .await
                .is_err()
        );
        let stream = driver
            .prompt(&b, prompt(root.path(), "other"))
            .await
            .unwrap();
        success = stream
            .collect::<Vec<_>>()
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })));
        let a = driver
            .resume_session(&a.native_session_id, &root.path().to_string_lossy())
            .await
            .unwrap();
        reloaded = count(root.path(), "session/load") == 1;
        driver.shutdown(a).await.unwrap();
    }
    driver.shutdown(b).await.unwrap();
    closed.unwrap();
    assert!(
        preserved,
        "closing one idle session killed the shared native connection"
    );
    assert!(success);
    assert!(
        reloaded,
        "retired session metadata was incorrectly reused without native load"
    );
    assert!(finished.load(Ordering::Acquire));
    assert_eq!(count(root.path(), "initialize"), 1);
    assert_eq!(
        count(root.path(), "session/close"),
        0,
        "unadvertised native close must never be called"
    );
}

#[tokio::test]
async fn shared_retirement_of_an_idle_session_does_not_stop_another_active_turn() {
    let (root, driver) = fixture("normal");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, _) = original(&driver).await;
    let stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let closed = driver.shutdown(a).await;
    let unaffected =
        !native.stopping() && !driver.shared.interrupt_requested.load(Ordering::Acquire);
    driver.interrupt(&b).await.unwrap();
    let _ = stream.collect::<Vec<_>>().await;
    driver.shutdown(b).await.unwrap();
    closed.unwrap();
    assert!(unaffected);
    assert_eq!(count(root.path(), "session/cancel"), 1);
}

#[tokio::test]
async fn shared_retirement_of_an_active_session_waits_for_its_turn_and_keeps_the_other_owner() {
    let (root, driver) = fixture("delayed-stop");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, _) = original(&driver).await;
    let stream = driver
        .prompt(&a, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let closed = driver.shutdown(a.clone()).await;
    let preserved = !native.stopping();
    let events = stream.collect::<Vec<_>>().await;
    let cancelled = count(root.path(), "session/cancel");
    let mut other_completed = false;
    if preserved {
        let other = driver
            .prompt(&b, prompt(root.path(), "other"))
            .await
            .unwrap();
        other_completed = other
            .collect::<Vec<_>>()
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })));
    }
    driver.shutdown(b).await.unwrap();
    closed.unwrap();
    assert!(preserved);
    assert_eq!(cancelled, 1);
    assert!(other_completed);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
}

#[tokio::test]
async fn cancelled_shared_retirement_still_forgets_the_original_cache_before_explicit_restore() {
    let (root, driver) = fixture("blocked-catalog");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, _) = original(&driver).await;
    // Deliberately occupy the original SDK queue so cancelled retirement
    // still has to await its local ForgetSession proof under backpressure.
    let catalog = original_catalog_barrier(&driver).await;
    wait_for(|| count(root.path(), "session/new") == 3).await;
    let closing = tokio::spawn({
        let driver = driver.clone();
        let a = a.clone();
        async move { driver.shutdown(a).await }
    });
    wait_for(|| {
        driver
            .shared
            .closed_sessions
            .lock()
            .unwrap()
            .contains(&a.native_session_id)
    })
    .await;
    closing.abort();
    let _ = closing.await;
    let preserved = !native.stopping();
    let premature = driver
        .resume_session(&a.native_session_id, &root.path().to_string_lossy())
        .await;
    std::fs::write(root.path().join("metadata-ready"), b"").unwrap();
    catalog.await.unwrap().unwrap();
    driver.shutdown(a.clone()).await.unwrap();
    let a = driver
        .resume_session(&a.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let stream = driver
        .prompt(&a, prompt(root.path(), "restored"))
        .await
        .unwrap();
    let completed = stream.collect::<Vec<_>>().await;
    driver.shutdown(b).await.unwrap();
    driver.shutdown(a).await.unwrap();
    assert!(preserved);
    assert!(premature.is_err());
    assert_eq!(count(root.path(), "session/load"), 1);
    assert_eq!(count(root.path(), "initialize"), 1);
    assert!(
        completed
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
}

#[tokio::test]
async fn an_abandoned_restore_keeps_setup_admission_until_native_settlement_then_retires_itself() {
    let (root, driver) = fixture("blocked-load");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (_, finished) = original(&driver).await;
    driver.shutdown(a.clone()).await.unwrap();
    let restoring = tokio::spawn({
        let driver = driver.clone();
        let a = a.clone();
        let cwd = root.path().to_string_lossy().into_owned();
        async move { driver.resume_session(&a.native_session_id, &cwd).await }
    });
    wait_for(|| count(root.path(), "session/load") == 1).await;
    restoring.abort();
    let _ = restoring.await;
    let closing = tokio::spawn({
        let driver = driver.clone();
        let b = b.clone();
        async move { driver.shutdown(b).await }
    });
    wait_for(|| {
        driver
            .shared
            .closed_sessions
            .lock()
            .unwrap()
            .contains(&b.native_session_id)
    })
    .await;
    let admission_retained = driver.shared.setup_in_progress.load(Ordering::Acquire)
        && !closing.is_finished()
        && !finished.load(Ordering::Acquire);
    std::fs::write(root.path().join("load-ready"), b"").unwrap();
    closing.await.unwrap().unwrap();
    // Native exit wakes independent retirement actors. Closing b only joins
    // b's actor; the abandoned restore must also finish removing its resident.
    wait_for(|| {
        finished.load(Ordering::Acquire) && driver.shared.sessions.lock().unwrap().is_empty()
    })
    .await;
    assert!(
        driver.shutdown(a).await.is_err(),
        "abandoned restore must not reauthorize an old handle"
    );
    assert!(admission_retained);
    assert!(driver.shared.sessions.lock().unwrap().is_empty());
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
    assert_eq!(count(root.path(), "session/load"), 1);
}

#[tokio::test]
async fn a_stop_timeout_preserves_other_owners_until_the_last_owner_explicitly_closes() {
    let (root, mut driver) = fixture("stop-stuck");
    Arc::get_mut(&mut driver).unwrap().stop_timeout = Duration::from_millis(150);
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (native, finished) = original(&driver).await;
    let stream = driver
        .prompt(&a, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let stopped = driver.interrupt(&a).await;
    let retried = driver.interrupt(&a).await;
    let preserved = !native.stopping() && !finished.load(Ordering::Acquire);
    let closing = tokio::spawn({
        let driver = driver.clone();
        let a = a.clone();
        async move { driver.shutdown(a).await }
    });
    wait_for(|| {
        driver
            .shared
            .closed_sessions
            .lock()
            .unwrap()
            .contains(&a.native_session_id)
    })
    .await;
    let pending = !closing.is_finished();
    driver.shutdown(b).await.unwrap();
    closing.await.unwrap().unwrap();
    let _ = stream.collect::<Vec<_>>().await;
    assert!(stopped.is_err());
    assert!(retried.is_err());
    assert!(preserved);
    assert!(pending);
    assert!(finished.load(Ordering::Acquire));
    assert!(driver.shared.sessions.lock().unwrap().is_empty());
    assert_eq!(count(root.path(), "session/cancel"), 1);
}

#[tokio::test]
async fn a_retired_sessions_late_native_text_cannot_enter_the_other_sessions_stream() {
    let (root, driver) = fixture("foreign-update");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    driver.shutdown(a).await.unwrap();
    let stream = driver
        .prompt(&b, prompt(root.path(), "answer"))
        .await
        .unwrap();
    let events = stream.collect::<Vec<_>>().await;
    driver.shutdown(b).await.unwrap();
    let rendered = format!("{events:?}");
    assert!(rendered.contains("current-session"));
    assert!(!rendered.contains("stale-other-session"));
}

#[tokio::test]
async fn a_failed_configuration_cannot_leave_an_old_handle_usable_with_unconfirmed_settings() {
    let (root, driver) = fixture("fail-model");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let configured = driver
        .resume_selected_session(
            &a.native_session_id,
            request(root.path()),
            Some("other"),
            None,
        )
        .await;
    let pending = driver.prompt(&a, prompt(root.path(), "must not run")).await;
    let rejected = pending.is_err();
    if let Ok(stream) = pending {
        let _ = stream.collect::<Vec<_>>().await;
    }
    driver.shutdown(a).await.unwrap();
    let other = driver
        .prompt(&b, prompt(root.path(), "other"))
        .await
        .unwrap();
    let events = other.collect::<Vec<_>>().await;
    driver.shutdown(b).await.unwrap();
    assert!(configured.is_err());
    assert!(
        rejected,
        "failed configuration left the previous handle callable"
    );
    assert_eq!(count(root.path(), "session/prompt"), 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
}

#[tokio::test]
async fn an_unowned_native_handle_cannot_spawn_or_send_without_explicit_restore() {
    let (root, driver) = fixture("normal");
    let handle = SessionHandle::new("not-owned".into());
    let sent = driver
        .prompt(&handle, prompt(root.path(), "must not run"))
        .await;
    assert!(sent.is_err());
    assert!(!root.path().join("pids.json").exists());
    assert_eq!(count(root.path(), "session/prompt"), 0);
}

#[tokio::test]
async fn a_retired_sessions_late_interrupt_cannot_request_rebuild_of_the_remaining_owner() {
    let (root, mut driver) = fixture("normal");
    Arc::get_mut(&mut driver).unwrap().executor.provider_id =
        agent_domain::ProviderId::parse("codebuddy").unwrap();
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&a, prompt(root.path(), "done"))
        .await
        .unwrap();
    let _ = stream.collect::<Vec<_>>().await;
    driver.shutdown(a.clone()).await.unwrap();
    let late = driver.interrupt(&a).await;
    let unaffected = !driver.shared.interrupt_requested.load(Ordering::Acquire)
        && !driver.shared.reconnect_required.load(Ordering::Acquire);
    driver.shutdown(b).await.unwrap();
    assert!(late.is_err());
    assert!(unaffected);
}

#[tokio::test]
async fn an_old_turns_interrupt_cannot_invalidate_the_same_sessions_new_configuration() {
    let (root, mut driver) = fixture("normal");
    Arc::get_mut(&mut driver).unwrap().executor.provider_id =
        agent_domain::ProviderId::parse("codebuddy").unwrap();
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver
        .prompt(&a, prompt(root.path(), "done"))
        .await
        .unwrap();
    let _ = stream.collect::<Vec<_>>().await;
    let current = driver
        .resume_session(&a.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let old = driver.interrupt(&a).await;
    let unaffected = !driver.shared.interrupt_requested.load(Ordering::Acquire)
        && !driver.shared.reconnect_required.load(Ordering::Acquire);
    driver.shutdown(current).await.unwrap();
    driver.shutdown(b).await.unwrap();
    assert!(old.is_err());
    assert!(unaffected);
}

mod native_close;

mod auxiliary;

mod scopes;

mod codebuddy_questions;
