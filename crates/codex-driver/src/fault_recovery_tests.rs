#![cfg(unix)]
use super::*;
use futures::StreamExt;
use std::os::unix::fs::PermissionsExt;

const FIXTURE: &str = r"#!/usr/bin/python3
import json,os,sys
root=os.path.dirname(__file__)
with open(os.path.join(root,'starts.jsonl'),'a') as f: f.write(str(os.getpid())+'\n')
def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 method=r['method']
 if method in ('thread/start','thread/resume'):
  result={'thread':{'id':'s','path':os.path.join(root,'rollout.jsonl'),'model':'actual'}}
 elif method=='turn/start': result={'turn':{'id':'t'}}
 else: result={}
 emit({'id':r['id'],'result':result})
 if method=='turn/start':
  if r['params']['input'][0]['text']=='hold': continue
  emit({'method':'item/agentMessage/delta','params':{'threadId':'s','turnId':'t','delta':'continued'}})
  emit({'method':'turn/completed','params':{'threadId':'s','turn':{'id':'t','status':'completed'}}})
";
fn setup() -> (tempfile::TempDir, CodexDriver) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("codex-fixture");
    std::fs::write(&file, FIXTURE).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = CodexDriver::new(&file, "fixture".into(), "fixture".into()).unwrap();
    (dir, driver)
}
fn workspace(path: &Path) -> SessionRequest {
    SessionRequest {
        working_directory: path.to_string_lossy().into_owned(),
        additional_directories: vec![],
        app_mcps: vec![],
        permission_mode: None,
    }
}
fn prompt(path: &Path, text: &str) -> PromptRequest {
    PromptRequest {
        prompt: text.into(),
        working_directory: path.to_string_lossy().into_owned(),
        context: vec![],
        additional_directories: vec![],
        attachments: vec![],
        permission_mode: None,
    }
}
#[tokio::test]
async fn idle_process_death_and_closed_registration_restore_once_and_continue() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let original = driver.command_sender(&session).await.unwrap();
    let pid = std::fs::read_to_string(dir.path().join("starts.jsonl")).unwrap();
    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-TERM", pid.trim()])
            .status()
            .unwrap()
            .success()
    );
    let native = driver.sessions.lock().await["s"].native.clone().unwrap();
    native.closed().await.unwrap();
    let (a, b) = tokio::join!(
        driver.resume_selected_session("s", workspace(dir.path()), None, None),
        driver.resume_selected_session("s", workspace(dir.path()), None, None)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    let session = driver.current_session_handle("s").await.unwrap();
    assert!(a.same_scope(&session) || b.same_scope(&session));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let current = driver.command_sender(&session).await.unwrap();
    assert!(!current.same_channel(&original));
    let mut events = driver
        .prompt(&session, prompt(dir.path(), "continue"))
        .await
        .unwrap();
    let mut completed = false;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(3), events.next())
        .await
        .unwrap()
    {
        completed |= matches!(event.unwrap(), DriverEvent::TurnCompleted { .. });
    }
    assert!(completed);
    let closed_registration = driver.sessions.lock().await["s"].clone();
    driver.shutdown(session.clone()).await.unwrap();
    // A dead actor can still have a persisted registration; presence alone is not liveness.
    driver
        .sessions
        .lock()
        .await
        .insert("s".into(), closed_registration);
    let session = driver
        .resume_selected_session("s", workspace(dir.path()), None, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    driver.shutdown(session).await.unwrap();
}
#[tokio::test]
async fn refresh_during_execution_does_not_replace_the_active_native_process() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let original = driver.command_sender(&session).await.unwrap();
    let _events = driver
        .prompt(&session, prompt(dir.path(), "hold"))
        .await
        .unwrap();
    assert!(matches!(
        driver
            .resume_selected_session("s", workspace(dir.path()), None, None)
            .await,
        Err(DriverError::CapabilityUnavailable(_))
    ));
    assert!(original.same_channel(&driver.command_sender(&session).await.unwrap()));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    driver.shutdown(session).await.unwrap();
}
