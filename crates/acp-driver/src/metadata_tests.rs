//! Private Python ACP scopes; no installed Agent, credentials, or model calls.
use super::*;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
const SCRIPT: &str = r"#!/usr/bin/env python3
import os,sys,json,time
os.chdir(os.path.dirname(os.path.abspath(__file__)))
mode='__MODE__'
open('pid-'+str(os.getpid()),'w').close()
count=0;pending=None
def out(v):print(json.dumps(v),flush=True)
def reply(r,v):out({'jsonrpc':'2.0','id':r['id'],'result':v})
def config(model='base'):
 return {'configOptions':[{'id':'model','category':'model','type':'select','name':'Model','currentValue':model,'options':[{'value':'base','name':'Base'},{'value':'other','name':'Other'}]}],'modes':{'currentModeId':'default','availableModes':[{'id':'default','name':'Ask'}]}}
for line in sys.stdin:
 r=json.loads(line);log=dict(r,pid=os.getpid())
 fd=os.open('metadata-requests.jsonl',os.O_WRONLY|os.O_CREAT|os.O_APPEND,0o600)
 os.write(fd,(json.dumps(log)+'\n').encode());os.close(fd)
 m=r.get('method')
 if m=='session/cancel' and pending:reply(pending,{'stopReason':'cancelled'});pending=None
 if 'id' not in r:continue
 if m=='initialize':reply(r,{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'authMethods':[]})
 elif m=='session/new':
  if mode=='blocked-new' and r['params']['cwd'].endswith('/probe'):
   while not os.path.exists('new-ready'):time.sleep(0.01)
  count+=1;reply(r,dict(config(),sessionId='s-'+str(count)))
 elif m=='session/load':reply(r,config())
 elif m=='session/set_config_option':
  if mode=='blocked-config':
   while not os.path.exists('config-ready'):time.sleep(0.01)
  out({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':r['params']['sessionId'],'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'metadata-only'}}}})
  reply(r,config('other'))
 elif m=='session/prompt':
  if r['params']['prompt'][0]['text']=='slow':pending=r
  else:reply(r,{'stopReason':'end_turn'})
 else:reply(r,{})
if mode=='delayed-exit':time.sleep(0.4)
";
fn fixture(mode: &str) -> (tempfile::TempDir, Arc<AcpDriver>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("probe")).unwrap();
    let file = root.path().join("runtime");
    std::fs::write(&file, SCRIPT.replace("__MODE__", mode)).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = AcpDriver::new(
        "kimi",
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
fn records(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("metadata-requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn native_pid(record: &serde_json::Value) -> u32 {
    u32::try_from(record["pid"].as_u64().unwrap()).unwrap()
}

#[tokio::test]
async fn cancelled_before_dispatch_does_not_start_any_native_process() {
    let (root, driver) = fixture("normal");
    let mut query = Box::pin(driver.model_catalog());
    assert!(futures::poll!(query.as_mut()).is_pending());
    drop(query);
    // Joining this admission proves the spawned owner has settled, rather
    // than racing a timing assertion against an unscheduled child process.
    let gate = tokio::time::timeout(Duration::from_secs(9), driver.shared.metadata.lock())
        .await
        .unwrap();
    assert!(records(root.path()).is_empty());
    assert!(driver.shared.connection.lock().await.is_none());
    drop(gate);
}

#[tokio::test]
async fn catalogs_during_a_running_turn_do_not_route_helper_updates_into_the_business_stream() {
    let (root, driver) = fixture("normal");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let original = native_pid(&records(root.path())[0]);
    let stream = driver
        .prompt(
            &session,
            HubPromptRequest {
                working_directory: root.path().to_string_lossy().into_owned(),
                additional_directories: vec![],
                context: vec![],
                attachments: vec![],
                permission_mode: None,
                prompt: "slow".into(),
            },
        )
        .await
        .unwrap();
    wait(|| {
        records(root.path())
            .iter()
            .any(|r| r["method"] == "session/prompt")
    })
    .await;
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let models = driver
        .model_catalog_in_workspace(Some("other"), Some(&workspace))
        .await;
    let permissions = driver.permission_catalog(Some(&workspace)).await;
    let active = driver.shared.turn_active.load(Ordering::Acquire) && present(original);
    driver.interrupt(&session).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;
    let original_probes = records(root.path())
        .iter()
        .filter(|r| native_pid(r) == original && r["method"] == "session/new")
        .count();
    clean(root.path(), &driver, session).await;
    assert!(models.is_ok());
    assert!(permissions.is_ok());
    assert!(active);
    assert_eq!(original_probes, 1);
    assert!(!events.iter().any(|event|matches!(event,Ok(DriverEvent::TextDelta{text,..}) if text.contains("metadata-only"))));
}

#[tokio::test]
async fn probe_deadline_owns_cleanup_even_without_a_core_or_caller_timeout() {
    let (root, mut driver) = fixture("blocked-config");
    Arc::get_mut(&mut driver).unwrap().metadata_timeout = Duration::from_secs(1);
    let session = driver.create_session(request(root.path())).await.unwrap();
    let original = native_pid(&records(root.path())[0]);
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let result = driver
        .model_catalog_in_workspace(Some("other"), Some(&workspace))
        .await;
    let log = records(root.path());
    let helper = native_pid(
        log.iter()
            .find(|r| r["method"] == "session/set_config_option")
            .unwrap(),
    );
    let exited = !present(helper);
    let preserved = present(original);
    clean(root.path(), &driver, session).await;
    assert!(
        matches!(result,Err(DriverError::Disconnected(message)) if message.contains("查询超时"))
    );
    assert_ne!(helper, original);
    assert!(exited);
    assert!(preserved);
}

#[tokio::test]
async fn cancelled_waiter_cannot_leave_a_queued_probe_to_execute_later() {
    let (root, driver) = fixture("blocked-config");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let first = tokio::spawn({
        let driver = driver.clone();
        let workspace = workspace.clone();
        async move {
            driver
                .model_catalog_in_workspace(Some("other"), Some(&workspace))
                .await
        }
    });
    wait(|| {
        records(root.path())
            .iter()
            .any(|r| r["method"] == "session/set_config_option")
    })
    .await;
    let mut queued = Box::pin(driver.permission_catalog(Some(&workspace)));
    assert!(futures::poll!(queued.as_mut()).is_pending());
    drop(queued);
    std::fs::write(root.path().join("config-ready"), b"").unwrap();
    first.await.unwrap().unwrap();
    let gate = driver.shared.metadata.lock().await;
    let starts = records(root.path())
        .iter()
        .filter(|r| r["method"] == "initialize")
        .count();
    drop(gate);
    clean(root.path(), &driver, session).await;
    assert_eq!(starts, 2, "a cancelled queued query started a native scope");
}

#[tokio::test]
async fn cancelled_blocked_probe_is_terminated_without_stopping_the_business_process() {
    let (root, driver) = fixture("blocked-config");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let original = native_pid(&records(root.path())[0]);
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let query = tokio::spawn({
        let driver = driver.clone();
        async move {
            driver
                .model_catalog_in_workspace(Some("other"), Some(&workspace))
                .await
        }
    });
    wait(|| {
        records(root.path())
            .iter()
            .any(|r| r["method"] == "session/set_config_option")
    })
    .await;
    let log = records(root.path());
    let helper = native_pid(
        log.iter()
            .find(|r| r["method"] == "session/set_config_option")
            .unwrap(),
    );
    query.abort();
    let _ = query.await;
    let exited = tokio::time::timeout(Duration::from_secs(9), async {
        while present(helper) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let preserved = present(original);
    clean(root.path(), &driver, session).await;
    assert_ne!(helper, original);
    assert!(exited, "cancelled probe kept its native scope");
    assert!(preserved);
}
fn present(pid: u32) -> bool {
    let result = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(result.stdout).unwrap().trim() == pid.to_string()
}
async fn wait(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(9), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn clean(root: &Path, driver: &AcpDriver, session: SessionHandle) {
    std::fs::write(root.join("new-ready"), b"").unwrap();
    std::fs::write(root.join("config-ready"), b"").unwrap();
    driver.shutdown(session).await.unwrap();
    let pids: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("pid-")?
                .parse::<u32>()
                .ok()
        })
        .collect();
    wait(|| pids.iter().all(|pid| !present(*pid))).await;
}
#[tokio::test]
async fn catalog_scope_exits_before_result_without_reusing_the_business_process() {
    let (root, driver) = fixture("delayed-exit");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let original = native_pid(&records(root.path())[0]);
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let catalog = driver
        .model_catalog_in_workspace(Some("other"), Some(&workspace))
        .await
        .unwrap();
    let log = records(root.path());
    let probe = log
        .iter()
        .find(|r| r["method"] == "session/new" && r["params"]["cwd"] == workspace)
        .unwrap();
    let pid = native_pid(probe);
    let exited = !present(pid);
    let preserved = present(original);
    clean(root.path(), &driver, session).await;
    assert_eq!(catalog.default_model.as_deref(), Some("base"));
    assert_ne!(pid, original, "metadata used the business process");
    assert!(exited, "catalog returned before its native scope exited");
    assert!(preserved);
}
#[tokio::test]
async fn blocked_catalog_does_not_delay_business_prompt_or_cancel() {
    let (root, driver) = fixture("blocked-new");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let catalog = tokio::spawn({
        let driver = driver.clone();
        let workspace = workspace.clone();
        async move {
            driver
                .model_catalog_in_workspace(None, Some(&workspace))
                .await
        }
    });
    wait(|| {
        records(root.path())
            .iter()
            .any(|r| r["method"] == "session/new" && r["params"]["cwd"] == workspace)
    })
    .await;
    let stream = driver
        .prompt(
            &session,
            HubPromptRequest {
                working_directory: root.path().to_string_lossy().into_owned(),
                additional_directories: vec![],
                context: vec![],
                attachments: vec![],
                permission_mode: None,
                prompt: "slow".into(),
            },
        )
        .await
        .unwrap();
    let dispatched = tokio::time::timeout(Duration::from_millis(500), async {
        wait(|| {
            records(root.path())
                .iter()
                .any(|r| r["method"] == "session/prompt")
        })
        .await;
    })
    .await
    .is_ok();
    let stop = if dispatched {
        Some(tokio::time::timeout(Duration::from_secs(2), driver.interrupt(&session)).await)
    } else {
        None
    };
    std::fs::write(root.path().join("new-ready"), b"").unwrap();
    catalog.await.unwrap().unwrap();
    if !dispatched {
        wait(|| {
            records(root.path())
                .iter()
                .any(|r| r["method"] == "session/prompt")
        })
        .await;
        driver.interrupt(&session).await.unwrap();
    }
    let _ = stream.collect::<Vec<_>>().await;
    clean(root.path(), &driver, session).await;
    assert!(
        dispatched,
        "business prompt was queued behind catalog metadata"
    );
    assert!(
        matches!(stop, Some(Ok(Ok(())))),
        "cancel was delayed by metadata"
    );
}
#[tokio::test]
async fn cancelled_catalog_retains_admission_until_only_its_native_scope_exits() {
    let (root, driver) = fixture("blocked-config");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let original = native_pid(&records(root.path())[0]);
    let workspace = root.path().join("probe").to_string_lossy().into_owned();
    let catalog = tokio::spawn({
        let driver = driver.clone();
        let workspace = workspace.clone();
        async move {
            driver
                .model_catalog_in_workspace(Some("other"), Some(&workspace))
                .await
        }
    });
    wait(|| {
        records(root.path())
            .iter()
            .any(|r| r["method"] == "session/set_config_option")
    })
    .await;
    let log = records(root.path());
    let old = native_pid(
        log.iter()
            .find(|r| r["method"] == "session/set_config_option")
            .unwrap(),
    );
    catalog.abort();
    let _ = catalog.await;
    let admission_held = driver.shared.metadata.try_lock().is_err();
    let queued = tokio::spawn({
        let driver = driver.clone();
        let workspace = workspace.clone();
        async move { driver.permission_catalog(Some(&workspace)).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let still_one = records(root.path())
        .iter()
        .filter(|r| r["method"] == "initialize")
        .count()
        == 2;
    std::fs::write(root.path().join("config-ready"), b"").unwrap();
    queued.await.unwrap().unwrap();
    let retired = !present(old);
    let preserved = present(original);
    clean(root.path(), &driver, session).await;
    assert_ne!(old, original);
    assert!(
        admission_held,
        "cancel released admission before physical exit"
    );
    assert!(
        still_one,
        "cancel released the scope slot before physical exit"
    );
    assert!(retired);
    assert!(preserved);
}
