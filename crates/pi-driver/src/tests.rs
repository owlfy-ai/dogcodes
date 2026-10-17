use super::*;
use driver_api::DriverEvent;
use futures::StreamExt;
use std::{path::Path, time::Duration};
const FAKE: &str = r"#!/usr/bin/env python3
import json,sys,os,uuid
args=sys.argv
path=args[args.index('--session')+1] if '--session' in args else os.path.join(os.getcwd(),str(uuid.uuid4())+'.jsonl')
model={'provider':'test','id':'model','name':'Test','reasoning':True}
level='medium'
pending=None
current_dialog=False
def out(v): print(json.dumps(v),flush=True)
def reply(r,data=None): out({'type':'response','id':r['id'],'success':True,'data':data})
def finish():
 out({'type':'message_start','message':{'role':'assistant'}})
 out({'type':'message_update','assistantMessageEvent':{'type':'text_delta','contentIndex':0,'delta':'Hello\u2028'}})
 out({'type':'message_end','message':{'role':'assistant','content':[{'type':'text','text':'Hello\u2028world'}],'stopReason':'stop'}})
 out({'type':'agent_end','willRetry':False});out({'type':'agent_settled'})
for line in sys.stdin:
 r=json.loads(line)
 with open('requests.jsonl','a') as f:f.write(json.dumps(r)+'\n')
 t=r['type']
 if t=='get_state':reply(r,{'sessionId':'native-id','sessionFile':path,'model':model,'thinkingLevel':level,'isStreaming':False})
 elif t=='get_available_models':reply(r,{'models':[model,{'provider':'test','id':'other','name':'Other'}]})
 elif t=='get_available_thinking_levels':reply(r,{'levels':['off','medium','high']})
 elif t=='set_model':
  if r['modelId']!='unconfirmed':model['id']=r['modelId']
  reply(r,{'model':model})
 elif t=='set_thinking_level':level=r['level'];reply(r)
 elif t=='prompt':
  with open(path,'w') as f:f.write('{}')
  if r['message']=='disconnect':sys.exit(0)
  if r['message']=='malformed':print('{bad',flush=True);continue
  if r['message']=='question':
   pending=r;current_dialog=True;out({'type':'extension_ui_request','id':'question-1','method':'select','title':'Choose','options':['Allow','Block']})
  else:
   reply(r)
   if r['message']!='slow':finish()
 elif t=='extension_ui_response':
  reply(pending);pending=None
  if r.get('cancelled'):out({'type':'agent_settled'})
  else:finish()
 elif t=='abort':out({'type':'agent_settled'});reply(r)
 else:reply(r)
";
fn fixture() -> (tempfile::TempDir, PiDriver) {
    fixture_with(FAKE)
}
fn fixture_with(script: &str) -> (tempfile::TempDir, PiDriver) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pi");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = PiDriver::new(
        &agent_domain::InstallationId::new().to_string(),
        path,
        Some("0.87.1".into()),
    )
    .unwrap();
    (dir, driver)
}
fn prompt(dir: &Path, text: &str) -> PromptRequest {
    PromptRequest {
        attachments: vec![],
        permission_mode: None,
        additional_directories: vec![],
        prompt: text.into(),
        working_directory: dir.to_string_lossy().into_owned(),
        context: vec![],
    }
}
async fn events(mut stream: DriverEventStream) -> Vec<Result<DriverEvent, DriverError>> {
    tokio::time::timeout(Duration::from_secs(5), async move {
        let mut events = vec![];
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        events
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn slow_abort_acknowledgement_preserves_session_and_waits_for_idle_confirmation() {
    let script = FAKE.replace("elif t=='abort':out({'type':'agent_settled'});reply(r)",
        "elif t=='abort':\n  import time\n  time.sleep(5.1);reply(r);time.sleep(0.1);out({'type':'agent_settled'})");
    let (dir, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "slow"))
        .await
        .unwrap();
    driver.interrupt(&handle).await.unwrap();
    assert!(
        events(stream)
            .await
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(Arc::ptr_eq(
        &connection,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    let next = events(
        driver
            .prompt(&handle, prompt(dir.path(), "continue"))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        next.iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn acknowledged_abort_without_settlement_retires_session_without_fake_success() {
    let script = FAKE.replace(
        "elif t=='abort':out({'type':'agent_settled'});reply(r)",
        "elif t=='abort':reply(r)",
    );
    let (dir, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "slow"))
        .await
        .unwrap();
    assert!(
        driver
            .interrupt_with_timeout(&handle, Duration::from_millis(200))
            .await
            .is_err()
    );
    let ended = events(stream).await;
    assert!(ended.iter().any(Result::is_err));
    assert!(!ended.iter().any(|e| matches!(
        e,
        Ok(DriverEvent::TurnInterrupted { .. } | DriverEvent::TurnCompleted { .. })
    )));
    assert!(driver.connection(&handle.native_session_id).await.is_err());
}

#[tokio::test]
async fn settlement_with_still_streaming_state_is_not_reused() {
    let script = FAKE.replace("'isStreaming':False", "'isStreaming':True");
    let (dir, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "slow"))
        .await
        .unwrap();
    assert!(driver.interrupt(&handle).await.is_err());
    assert!(events(stream).await.iter().any(Result::is_err));
    assert!(driver.connection(&handle.native_session_id).await.is_err());
}
#[tokio::test]
async fn native_rpc_catalog_streaming_warm_reuse_and_cross_process_resume() {
    let (dir, driver) = fixture();
    let cwd = dir.path().to_string_lossy();
    let catalog = driver
        .model_catalog_in_workspace(Some("test/other"), Some(&cwd))
        .await
        .unwrap();
    assert_eq!(catalog.default_model.as_deref(), Some("test/model"));
    assert_eq!(
        catalog
            .models
            .iter()
            .find(|m| m.id == "test/other")
            .unwrap()
            .thinking_options
            .len(),
        3
    );
    let handle = driver
        .create_selected_session(session_request(&cwd), Some("test/model"), Some("high"))
        .await
        .unwrap();
    let first = events(
        driver
            .prompt(&handle, prompt(dir.path(), "hello"))
            .await
            .unwrap(),
    )
    .await;
    let text = first
        .iter()
        .filter_map(|v| match v {
            Ok(DriverEvent::TextDelta { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "Hello\u{2028}world");
    assert_eq!(
        first
            .iter()
            .filter(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
            .count(),
        1
    );
    driver
        .resume_session(&handle.native_session_id, &cwd)
        .await
        .unwrap();
    let conn = driver.connection(&handle.native_session_id).await.unwrap();
    driver
        .resume_session(&handle.native_session_id, &cwd)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &conn,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    driver.shutdown(handle.clone()).await.unwrap();
    driver
        .resume_session(&handle.native_session_id, &cwd)
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(
        &conn,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    driver.shutdown(handle).await.unwrap();
}
#[tokio::test]
async fn preflight_question_is_visible_before_acknowledgement_and_can_be_cancelled() {
    let (dir, driver) = fixture();
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let mut stream = driver
        .prompt(&handle, prompt(dir.path(), "question"))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(event, DriverEvent::QuestionRequested { .. }));
    assert!(
        driver
            .respond_to_question(
                &handle,
                "question-1",
                QuestionAnswer {
                    cancelled: false,
                    values: json!({"answer":"invented"})
                }
            )
            .await
            .is_err()
    );
    driver.interrupt(&handle).await.unwrap();
    let ended = events(stream).await;
    assert!(
        ended
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    let next = events(
        driver
            .prompt(&handle, prompt(dir.path(), "hello again"))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        next.iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}
#[tokio::test]
async fn contexts_are_ordered_and_unconfirmed_selections_and_lost_sessions_fail() {
    let (dir, driver) = fixture();
    let cwd = dir.path().to_string_lossy();
    assert!(
        driver
            .create_selected_session(session_request(&cwd), Some("test/unconfirmed"), None)
            .await
            .is_err()
    );
    assert!(matches!(
        driver.resume_session("/missing/pi.jsonl", &cwd).await,
        Err(DriverError::SessionMissing(_))
    ));
    let handle = driver.create_session(session_request(&cwd)).await.unwrap();
    let mut request = prompt(dir.path(), "USER_PROMPT");
    request.context.push(driver_api::ContextDelivery {
        artifact_id: agent_domain::ArtifactId::new(),
        kind: driver_api::ContextDeliveryKind::Handoff,
        payload_sha256: "hash".into(),
        content: b"REVIEWED_CONTEXT".to_vec(),
    });
    events(driver.prompt(&handle, request).await.unwrap()).await;
    let records = std::fs::read_to_string(dir.path().join("requests.jsonl")).unwrap();
    let record = records
        .lines()
        .filter_map(|v| serde_json::from_str::<Value>(v).ok())
        .find(|v| v["type"] == "prompt")
        .unwrap();
    let message = record["message"].as_str().unwrap();
    assert!(message.find("REVIEWED_CONTEXT").unwrap() < message.find("USER_PROMPT").unwrap());
    driver.shutdown(handle).await.unwrap();
}
#[tokio::test]
async fn process_failure_and_malformed_json_terminate_the_stream() {
    for message in ["disconnect", "malformed"] {
        let (dir, driver) = fixture();
        let handle = driver
            .create_session(session_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let failed = events(
            driver
                .prompt(&handle, prompt(dir.path(), message))
                .await
                .unwrap(),
        )
        .await;
        assert!(failed.iter().any(Result::is_err));
        driver
            .resume_session(&handle.native_session_id, &dir.path().to_string_lossy())
            .await
            .unwrap();
        let recovered = events(
            driver
                .prompt(&handle, prompt(dir.path(), "after failure"))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            recovered
                .iter()
                .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
        );
        driver.shutdown(handle).await.unwrap();
    }
}
#[tokio::test]
#[ignore = "metadata-only probe of the locally installed Pi"]
async fn real_pi_metadata() {
    let binary = std::env::var("DOGCODES_PI_BINARY").unwrap();
    PiDriver::probe_ready(Path::new(&binary)).await.unwrap();
    let driver = PiDriver::new(
        &agent_domain::InstallationId::new().to_string(),
        binary.into(),
        None,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy();
    let catalog = driver
        .model_catalog_in_workspace(None, Some(&cwd))
        .await
        .unwrap();
    assert!(!catalog.models.is_empty());
    let handle = driver.create_session(session_request(&cwd)).await.unwrap();
    println!(
        "Pi native models={} persistent_session=true",
        catalog.models.len()
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
#[ignore = "runs short Pi model turns in a temporary workspace"]
async fn real_pi_turn_resume_and_cancel() {
    let driver = PiDriver::new(
        &agent_domain::InstallationId::new().to_string(),
        std::env::var("DOGCODES_PI_BINARY").unwrap().into(),
        None,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy();
    let handle = driver.create_session(session_request(&cwd)).await.unwrap();
    let nonce = format!("PI_PROBE_{}", uuid::Uuid::new_v4().simple());
    let collect = async {
        let mut text = String::new();
        let mut stream=driver.prompt(&handle,prompt(dir.path(),&format!("Remember this marker: {nonce}. Reply exactly with that marker. Do not run tools."))).await.unwrap();
        let started = std::time::Instant::now();
        let mut count = 0;
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                DriverEvent::TextDelta { text: delta, .. } => {
                    if count == 0 {
                        println!("pi first_delta_ms={}", started.elapsed().as_millis());
                    }
                    count += 1;
                    text.push_str(&delta);
                }
                DriverEvent::TurnCompleted { .. } => break,
                DriverEvent::TurnFailed { reason } => panic!("Pi turn failed: {reason}"),
                _ => {}
            }
        }
        assert!(text.contains(&nonce));
        assert!(count > 0);
    };
    tokio::time::timeout(Duration::from_secs(90), collect)
        .await
        .unwrap();
    driver.shutdown(handle.clone()).await.unwrap();
    driver
        .resume_session(&handle.native_session_id, &cwd)
        .await
        .unwrap();
    let mut stream=driver.prompt(&handle,prompt(dir.path(),"What marker did I ask you to remember? Reply with only that marker. Do not run tools.")).await.unwrap();
    let restored = tokio::time::timeout(Duration::from_secs(90), async {
        let mut text = String::new();
        while let Some(v) = stream.next().await {
            if let DriverEvent::TextDelta { text: delta, .. } = v.unwrap() {
                text.push_str(&delta);
            }
        }
        text
    })
    .await
    .unwrap();
    assert!(restored.contains(&nonce));
    let mut stream = driver
        .prompt(
            &handle,
            prompt(
                dir.path(),
                "List all integers from 1 to 10000, each on a separate line, without using tools.",
            ),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(v) = stream.next().await {
            if matches!(v.unwrap(), DriverEvent::TextDelta { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    driver.interrupt(&handle).await.unwrap();
    let remaining = events(stream).await;
    assert!(
        remaining
            .iter()
            .any(|v| matches!(v, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
    println!("pi native stream/resume/cancel passed");
}
