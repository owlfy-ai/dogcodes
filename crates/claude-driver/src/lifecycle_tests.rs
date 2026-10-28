//! Protocol fixtures exercise the real reader, control pipe and child process cleanup.
#![cfg(unix)]
use super::*;
use std::{os::unix::fs::PermissionsExt, time::Duration};

const FIXTURE: &str = r"#!/usr/bin/python3
import sys,json,os
with open('starts.jsonl','a') as f: f.write(str(os.getpid())+'\n')
mode=''
def emit(v): print(json.dumps(v),flush=True)
def result(): emit({'type':'result','subtype':'success'})
def stopped(): emit({'type':'system','subtype':'task_notification','task_id':'worker','tool_use_id':'call','status':'stopped'})
for line in sys.stdin:
 r=json.loads(line)
 if r.get('fixture')=='stopped': stopped()
 elif r.get('fixture')=='exit': sys.exit(0)
 elif r.get('fixture')=='withdraw': emit({'type':'control_cancel_request','request_id':'approval'})
 elif r.get('fixture')=='append-bash':
  with open('native-bash.output','a') as f:f.write('middle\n')
 elif r.get('fixture')=='finish-bash':
  with open('native-bash.output','a') as f:f.write('last\n')
  emit({'type':'system','subtype':'task_notification','task_id':'shell-task','status':'completed','summary':'summary'})
 elif r.get('fixture')=='continue':
  emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'continued'}}})
 elif r.get('fixture')=='fail': emit({'type':'result','subtype':'error_during_execution','is_error':True,'errors':['fixture failure']})
 elif r.get('fixture')=='late_approval':
  emit({'type':'control_request','request_id':'late','request':{'subtype':'can_use_tool','agent_id':'worker','tool_name':'Bash','input':{'command':'echo late'}}})
 elif r.get('type')=='control_response':
  with open('responses.jsonl','a') as f:f.write(json.dumps(r)+'\n')
  if r['response']['request_id']=='approval':
   stopped()
   result()
 elif r.get('type')=='control_request':
  request=r['request']
  with open('controls.jsonl','a') as f:f.write(json.dumps(request)+'\n')
  data={'applied':{'model':'fixture','effort':'high'},'effective':{'alwaysThinkingEnabled':True}} if request['subtype']=='get_settings' else {}
  rejected=request['subtype']=='apply_flag_settings' and request['settings'].get('model')=='reject'
  emit({'type':'control_response','response':{'request_id':r['request_id'],'subtype':'error' if rejected else 'success','response':data}})
  if request['subtype']=='interrupt': result()
  elif request['subtype']=='stop_task' and mode=='stop': stopped()
 elif r.get('type')=='user':
  mode=r['message']['content'][0]['text']
  if mode=='simple': result();continue
  if mode=='background':
   emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'shell','name':'Bash','input':{'command':'background fixture'}}]}})
   path=os.path.abspath('native-bash.output')
   with open(path,'w') as f:f.write('first\n')
   emit({'type':'user','tool_use_result':{'backgroundTaskId':'shell-task'},'message':{'content':[{'type':'tool_result','tool_use_id':'shell','content':'Output is being written to: '+path+'.'}]}})
   result();continue
  emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'call','name':'Workflow','input':{}}]}})
  emit({'type':'system','subtype':'task_started','task_id':'worker','tool_use_id':'call','task_type':'local_workflow'})
  if mode=='approval':
   emit({'type':'control_request','request_id':'approval','request':{'subtype':'can_use_tool','agent_id':'worker','tool_name':'Bash','input':{'command':'echo approved'}}})
  result()
";

fn setup() -> (tempfile::TempDir, ClaudeDriver) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("claude-fixture");
    std::fs::write(&executable, FIXTURE).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let driver = ClaudeDriver::new(
        "claude-code",
        &InstallationId::new().to_string(),
        executable,
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    (dir, driver)
}

#[tokio::test]
async fn background_bash_output_streams_while_idle_and_finishes_on_its_original_turn() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let process = driver.processes.lock().await[&session.native_session_id].clone();
    let mut old = driver
        .prompt(&session, prompt(dir.path(), "background"))
        .await
        .unwrap();
    until(&mut old, terminal).await;
    // No native stdout event accompanies this write; the reader must poll while idle.
    process
        .send_line(&json!({"fixture":"append-bash"}))
        .await
        .unwrap();
    until(&mut old, |event|matches!(event,DriverEvent::Presentation{item_id,data} if item_id == "shell" && data["output"] == "first\nmiddle\n")).await;
    let mut next_turn = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    until(&mut next_turn, terminal).await;
    process
        .send_line(&json!({"fixture":"finish-bash"}))
        .await
        .unwrap();
    until(&mut old, |event|matches!(event,DriverEvent::Presentation{item_id,data} if item_id == "shell" && data["status"] == "completed" && data["output"] == "first\nmiddle\nlast\n")).await;
    assert!(old.next().await.is_none());
    assert!(next_turn.next().await.is_none());
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn idle_exit_is_restored_once_under_concurrent_resume_and_next_prompt_completes() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let original = driver.processes.lock().await[&session.native_session_id].clone();
    original
        .send_line(&json!({"fixture":"exit"}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !original.cleanup_done.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let id = &session.native_session_id;
    let (a, b) = tokio::join!(
        driver.resume_selected_session(id, workspace(dir.path()), None, None),
        driver.resume_selected_session(id, workspace(dir.path()), None, None)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    let session = driver.current_session_handle(id).await.unwrap();
    assert!(a.same_scope(&session) || b.same_scope(&session));
    let id = &session.native_session_id;
    let current = driver.processes.lock().await[id].clone();
    assert!(!Arc::ptr_eq(&original, &current));
    await_file(&dir.path().join("starts.jsonl"), "\n").await;
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("starts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn autonomous_failure_is_acknowledged_before_session_reuse() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut autonomous = driver.take_autonomous_events(&session).await.unwrap();
    let mut first = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut first, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    let process = driver.processes.lock().await[&session.native_session_id].clone();
    process
        .send_line(&json!({"fixture":"continue"}))
        .await
        .unwrap();
    until(&mut autonomous, |e| {
        matches!(e, DriverEvent::TurnStarted { .. })
    })
    .await;
    process.send_line(&json!({"fixture":"fail"})).await.unwrap();
    assert!(matches!(
        until(&mut autonomous, terminal).await,
        DriverEvent::TurnFailed { .. }
    ));
    assert!(matches!(
        driver
            .resume_selected_session(
                &session.native_session_id,
                workspace(dir.path()),
                None,
                None
            )
            .await,
        Err(DriverError::AutonomousTurnBusy)
    ));
    driver.acknowledge_autonomous_turn(&session).await;
    let session = driver
        .resume_selected_session(
            &session.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &process,
        &driver.processes.lock().await[&session.native_session_id]
    ));
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    assert!(!driver.has_autonomous_turn(&session.native_session_id).await);
    driver.shutdown(session).await.unwrap();
}

fn workspace(dir: &std::path::Path) -> SessionRequest {
    SessionRequest {
        working_directory: dir.to_string_lossy().into_owned(),
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: None,
    }
}

#[tokio::test]
async fn hot_reuse_rejects_original_handles_and_queued_cancel_without_consuming_new_approval() {
    let (dir, driver) = setup();
    let original = driver.create_session(workspace(dir.path())).await.unwrap();
    let process = driver.processes.lock().await[&original.native_session_id].clone();
    let successor = driver
        .resume_selected_session(
            &original.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!original.same_scope(&successor));
    assert!(Arc::ptr_eq(
        &process,
        &driver.processes.lock().await[&successor.native_session_id]
    ));
    let mut stream = driver
        .prompt(&successor, prompt(dir.path(), "approval"))
        .await
        .unwrap();
    until(&mut stream, |e| {
        matches!(e, DriverEvent::ApprovalRequested { .. })
    })
    .await;

    assert!(matches!(
        driver
            .prompt(&original, prompt(dir.path(), "must not run"))
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver.interrupt(&original).await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver
            .respond_to_interaction(&original, "approval", "allow")
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver
            .respond_to_question(
                &original,
                "approval",
                agent_domain::QuestionAnswer {
                    cancelled: true,
                    values: Value::Null
                }
            )
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver.shutdown(original.clone()).await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(driver.shutdown_proof(&original).await.is_none());
    assert!(driver.take_autonomous_events(&original).await.is_none());
    assert!(
        driver
            .process_for(&SessionHandle::new(original.native_session_id.clone()))
            .await
            .is_err()
    );

    process
        .commands
        .send(lifecycle::Command::Begin(original.clone()))
        .unwrap();
    let (response, received) = oneshot::channel();
    process
        .commands
        .send(lifecycle::Command::Cancel {
            handle: original,
            response,
        })
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), received)
            .await
            .unwrap()
            .unwrap(),
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(!process.interrupt_requested.load(Ordering::Acquire));
    assert!(!process.native.stopping());
    assert!(
        process
            .pending_approvals
            .lock()
            .unwrap()
            .contains_key("approval")
    );
    assert!(!dir.path().join("responses.jsonl").exists());
    driver
        .respond_to_interaction(&successor, "approval", "allow")
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn an_operation_waiting_for_admission_rechecks_scope_after_hot_reuse() {
    let (dir, driver) = setup();
    let original = driver.create_session(workspace(dir.path())).await.unwrap();
    let process = driver.processes.lock().await[&original.native_session_id].clone();
    let admission = process.admission.lock().await;
    let request = workspace(dir.path());
    let mut reuse =
        Box::pin(driver.resume_selected_session(&original.native_session_id, request, None, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut reuse)
            .await
            .is_err()
    );
    let mut late = Box::pin(driver.respond_to_interaction(&original, "approval", "allow"));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut late)
            .await
            .is_err()
    );
    process
        .pending_approvals
        .lock()
        .unwrap()
        .insert("approval".into(), json!({"command":"echo protected"}));
    drop(admission);
    let successor = reuse.await.unwrap();
    assert!(matches!(
        late.await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(
        process
            .pending_approvals
            .lock()
            .unwrap()
            .contains_key("approval")
    );
    assert!(!dir.path().join("responses.jsonl").exists());
    assert!(!process.native.stopping());
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn passive_proof_and_close_receipts_cannot_touch_a_same_id_replacement() {
    let (dir, driver) = setup();
    let original = driver.create_session(workspace(dir.path())).await.unwrap();
    let process = driver.processes.lock().await[&original.native_session_id].clone();
    let mut proof = driver.shutdown_proof(&original).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut proof)
            .await
            .is_err()
    );
    assert!(!process.native.stopping());
    assert!(!dir.path().join("controls.jsonl").exists());
    driver.shutdown(original.clone()).await.unwrap();
    driver.shutdown(original.clone()).await.unwrap();
    assert!(
        driver
            .shutdown(SessionHandle::new(original.native_session_id.clone()))
            .await
            .is_err()
    );
    let successor = driver
        .resume_selected_session(
            &original.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    let replacement = driver.processes.lock().await[&successor.native_session_id].clone();
    assert!(!Arc::ptr_eq(&process, &replacement));
    tokio::time::timeout(Duration::from_secs(2), proof)
        .await
        .unwrap();
    assert!(driver.shutdown(original.clone()).await.is_err());
    assert!(driver.shutdown_proof(&original).await.is_none());
    assert!(!replacement.native.stopping());
    let mut stream = driver
        .prompt(&successor, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn hot_reuse_hands_new_continuations_to_the_new_handle_and_keeps_old_shell_output() {
    let (dir, driver) = setup();
    let original = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut old_autonomous = driver.take_autonomous_events(&original).await.unwrap();
    let process = driver.process_for(&original).await.unwrap();
    let mut old_turn = driver
        .prompt(&original, prompt(dir.path(), "background"))
        .await
        .unwrap();
    until(&mut old_turn, terminal).await;
    let successor = driver
        .resume_selected_session(
            &original.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    // Continuation can arrive before Core takes the new receiver. It must be
    // queued on the new scope, not sent to the old observer or dropped.
    process
        .send_line(&json!({"fixture":"continue"}))
        .await
        .unwrap();
    wait_for_active(&process).await;
    let mut current = driver.take_autonomous_events(&successor).await.unwrap();
    assert!(matches!(
        until(&mut current, |e| matches!(
            e,
            DriverEvent::TurnStarted { .. }
        ))
        .await,
        DriverEvent::TurnStarted { .. }
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), old_autonomous.next())
            .await
            .unwrap()
            .is_none()
    );
    process
        .send_line(&json!({"fixture":"append-bash"}))
        .await
        .unwrap();
    until(&mut old_turn, |e| matches!(e, DriverEvent::Presentation {item_id,data} if item_id == "shell" && data["output"] == "first\nmiddle\n")).await;
    process
        .send_line(&json!({"fixture":"finish-bash"}))
        .await
        .unwrap();
    until(&mut old_turn, |e| matches!(e, DriverEvent::Presentation {item_id,data} if item_id == "shell" && data["status"] == "completed")).await;
    assert!(driver.interrupt(&original).await.is_err());
    let pending = process.autonomous_pending.load(Ordering::Acquire);
    assert!(pending > 0);
    driver.acknowledge_autonomous_turn(&original).await;
    assert_eq!(
        process.autonomous_pending.load(Ordering::Acquire),
        pending,
        "an old scope acknowledged the successor's continuation"
    );
    driver.interrupt(&successor).await.unwrap();
    assert!(matches!(
        until(&mut current, terminal).await,
        DriverEvent::TurnInterrupted { .. }
    ));
    driver.acknowledge_autonomous_turn(&successor).await;
    let next = driver
        .resume_selected_session(
            &successor.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    let mut stream = driver
        .prompt(&next, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    driver.shutdown(next).await.unwrap();
}

#[tokio::test]
async fn rejected_configuration_keeps_the_original_handle_and_continuation_observer() {
    let (dir, driver) = setup();
    let original = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut events = driver.take_autonomous_events(&original).await.unwrap();
    let process = driver.process_for(&original).await.unwrap();
    assert_eq!(
        driver
            .reconfigure_session(
                &original.native_session_id,
                workspace(dir.path()),
                Some("reject"),
                None
            )
            .await
            .unwrap_err(),
        DriverError::Protocol("selection_rejected".into())
    );
    assert!(process.handle().same_scope(&original));
    assert!(!process.native.stopping());
    process
        .send_line(&json!({"fixture":"continue"}))
        .await
        .unwrap();
    until(&mut events, |e| {
        matches!(e, DriverEvent::TurnStarted { .. })
    })
    .await;
    driver.interrupt(&original).await.unwrap();
    assert!(matches!(
        until(&mut events, terminal).await,
        DriverEvent::TurnInterrupted { .. }
    ));
    driver.acknowledge_autonomous_turn(&original).await;
    driver.shutdown(original).await.unwrap();
}

async fn wait_for_active(process: &SessionProcess) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while process.autonomous_pending.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn prompt(dir: &std::path::Path, text: &str) -> HubPromptRequest {
    HubPromptRequest {
        working_directory: dir.to_string_lossy().into_owned(),
        additional_directories: vec![],
        permission_mode: None,
        prompt: text.into(),
        context: vec![],
        attachments: vec![],
    }
}
async fn next(stream: &mut DriverEventStream) -> DriverEvent {
    tokio::time::timeout(Duration::from_secs(8), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
fn terminal(event: &DriverEvent) -> bool {
    matches!(
        event,
        DriverEvent::TurnCompleted { .. }
            | DriverEvent::TurnFailed { .. }
            | DriverEvent::TurnInterrupted { .. }
    )
}
async fn until(
    stream: &mut DriverEventStream,
    matches: impl Fn(&DriverEvent) -> bool,
) -> DriverEvent {
    loop {
        let event = next(stream).await;
        if matches(&event) {
            return event;
        }
    }
}
async fn await_file(path: &std::path::Path, needle: &str) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let content = std::fs::read_to_string(path).unwrap_or_default();
            if content.contains(needle) {
                return content;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn agent_approval_survives_root_result_and_remains_answerable() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "approval"))
        .await
        .unwrap();
    until(&mut stream, |e| {
        matches!(e, DriverEvent::ApprovalRequested { .. })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let process = driver.processes.lock().await[&session.native_session_id].clone();
    assert!(process.turn_active.load(Ordering::Acquire));
    let mut changed = workspace(dir.path());
    changed.permission_mode = Some("full-access".into());
    assert!(matches!(
        driver
            .resume_selected_session(&session.native_session_id, changed, None, None)
            .await,
        Err(DriverError::AutonomousTurnBusy)
    ));
    assert!(Arc::ptr_eq(
        &process,
        &driver.processes.lock().await[&session.native_session_id]
    ));
    assert!(
        process
            .pending_approvals
            .lock()
            .unwrap()
            .contains_key("approval")
    );
    assert!(
        driver
            .prompt(&session, prompt(dir.path(), "simple"))
            .await
            .is_err()
    );
    driver
        .respond_to_interaction(&session, "approval", "allow")
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    let response = await_file(&dir.path().join("responses.jsonl"), "allow").await;
    assert!(response.contains("echo approved"));
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn cancellation_waits_for_task_terminal_and_interrupts_continuation() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "manual"))
        .await
        .unwrap();
    until(
        &mut stream,
        |e| matches!(e, DriverEvent::Presentation { data, .. } if data["status"] == "running"),
    )
    .await;
    driver.interrupt(&session).await.unwrap();
    await_file(&dir.path().join("controls.jsonl"), "stop_task").await;
    let process = driver.processes.lock().await[&session.native_session_id].clone();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        process.turn_active.load(Ordering::Acquire),
        "control ack is not task completion"
    );
    process
        .send_line(&json!({"fixture":"continue"}))
        .await
        .unwrap();
    await_file(&dir.path().join("controls.jsonl"), "interrupt").await;
    process
        .send_line(&json!({"fixture":"late_approval"}))
        .await
        .unwrap();
    await_file(&dir.path().join("responses.jsonl"), "deny").await;
    process
        .send_line(&json!({"fixture":"stopped"}))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnInterrupted { .. }
    ));
    assert!(!process.finished.load(Ordering::Acquire));
    let mut next_turn = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut next_turn, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn missing_task_terminal_forces_confirmed_shutdown_and_resume() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "manual"))
        .await
        .unwrap();
    until(
        &mut stream,
        |e| matches!(e, DriverEvent::Presentation { data, .. } if data["status"] == "running"),
    )
    .await;
    let original = driver.processes.lock().await[&session.native_session_id].clone();
    driver.interrupt(&session).await.unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnInterrupted { .. }
    ));
    assert!(original.finished.load(Ordering::Acquire));
    assert!(
        driver
            .prompt(&session, prompt(dir.path(), "simple"))
            .await
            .is_err()
    );
    let session = driver
        .resume_selected_session(
            &session.native_session_id,
            workspace(dir.path()),
            None,
            None,
        )
        .await
        .unwrap();
    let resumed = driver.processes.lock().await[&session.native_session_id].clone();
    assert!(!Arc::ptr_eq(&original, &resumed));
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "simple"))
        .await
        .unwrap();
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnCompleted { .. }
    ));
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn cancellation_expires_agent_approval_and_rejects_late_answer() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "approval"))
        .await
        .unwrap();
    until(&mut stream, |e| {
        matches!(e, DriverEvent::ApprovalRequested { .. })
    })
    .await;
    driver.interrupt(&session).await.unwrap();
    until(&mut stream, |e| matches!(e, DriverEvent::ApprovalClosed { native_request_id } if native_request_id == "approval")).await;
    assert!(
        driver
            .respond_to_interaction(&session, "approval", "allow")
            .await
            .is_err()
    );
    assert!(matches!(
        until(&mut stream, terminal).await,
        DriverEvent::TurnInterrupted { .. }
    ));
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn native_withdrawal_expires_approval_without_ending_running_workflow() {
    let (dir, driver) = setup();
    let session = driver.create_session(workspace(dir.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(dir.path(), "approval"))
        .await
        .unwrap();
    until(&mut stream, |e| {
        matches!(e, DriverEvent::ApprovalRequested { .. })
    })
    .await;
    let process = driver.processes.lock().await[&session.native_session_id].clone();
    process
        .send_line(&json!({"fixture":"withdraw"}))
        .await
        .unwrap();
    until(&mut stream, |e| {
        matches!(e, DriverEvent::ApprovalClosed { .. })
    })
    .await;
    assert!(
        driver
            .respond_to_interaction(&session, "approval", "allow")
            .await
            .is_err()
    );
    assert!(process.turn_active.load(Ordering::Acquire));
    process
        .send_line(&json!({"fixture":"stopped"}))
        .await
        .unwrap();
    until(&mut stream, terminal).await;
    driver.shutdown(session).await.unwrap();
}
