//! Private deterministic runtimes, no installed ZCode/account/model calls.
use super::cleanup_tests::{count, driver, events, fixture, native, prompt, session, wait_for};
use super::*;
use std::{path::Path, time::Duration};

async fn finish(driver: &ZcodeDriver, handle: &SessionHandle, root: &Path) {
    assert!(
        events(driver.prompt(handle, prompt(root, "hello")).await.unwrap())
            .await
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
}
fn edit(launch: &ZcodeLaunch, change: impl FnOnce(String) -> String) {
    let source = std::fs::read_to_string(&launch.app_binary).unwrap();
    std::fs::write(&launch.app_binary, change(source)).unwrap();
}
async fn stop(driver: Arc<ZcodeDriver>) {
    let worker = native(&driver).await;
    drop(driver);
    worker.closed().await.unwrap();
}

#[tokio::test]
async fn warm_handoff_rejects_old_raw_and_previously_captured_authority() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    finish(&driver, &old, root.path()).await;
    let state = driver.sessions.lock().unwrap()[&old.native_session_id].clone();
    let current = driver
        .resume_session(&old.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(!old.same_scope(&current));
    assert!(
        driver
            .prompt(&old, prompt(root.path(), "must not send"))
            .await
            .is_err()
    );
    assert!(driver.interrupt(&old).await.is_err());
    assert!(driver.shutdown(old.clone()).await.is_err());
    assert!(
        driver
            .respond_to_interaction(&old, "request", "allow_once")
            .await
            .is_err()
    );
    assert!(
        driver
            .respond_to_question(
                &old,
                "question",
                agent_domain::QuestionAnswer {
                    cancelled: true,
                    values: Value::Null
                }
            )
            .await
            .is_err()
    );
    assert!(
        driver
            .shutdown(SessionHandle::new(current.native_session_id.clone()))
            .await
            .is_err()
    );
    assert!(driver.shutdown_proof(&old).await.is_none());
    assert!(
        lifecycle::close_session(
            driver.connection.clone(),
            driver.sessions.clone(),
            old,
            state.clone(),
            driver.retired.clone()
        )
        .is_err()
    );
    assert!(!state.closing.load(Ordering::Acquire));
    assert_eq!(count(root.path(), "session/resume"), 0);
    assert_eq!(count(root.path(), "session/stop"), 0);
    assert_eq!(count(root.path(), "session/close"), 0);
    finish(&driver, &current, root.path()).await;
    driver.shutdown(current).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn passive_proof_keeps_original_logical_session_without_stopping_its_shared_worker() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let other = session(&driver, root.path()).await;
    let worker = native(&driver).await;
    let mut proof = driver.shutdown_proof(&old).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "session/close"), 0);
    driver.shutdown(old.clone()).await.unwrap();
    let replacement = driver
        .resume_session(&old.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), &mut proof)
        .await
        .unwrap();
    assert!(driver.shutdown(old).await.is_err());
    assert!(!worker.stopping());
    finish(&driver, &replacement, root.path()).await;
    finish(&driver, &other, root.path()).await;
    driver.shutdown(replacement).await.unwrap();
    driver.shutdown(other).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn busy_restore_does_not_rebind_or_stop_a_running_turn() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/send") == 1).await;
    let result = driver
        .resume_session(&handle.native_session_id, root.path().to_str().unwrap())
        .await;
    assert!(matches!(result, Err(DriverError::AutonomousTurnBusy)));
    assert!(
        driver
            .current_session_handle(&handle.native_session_id)
            .await
            .unwrap()
            .same_scope(&handle)
    );
    assert_eq!(count(root.path(), "session/close"), 0);
    driver.interrupt(&handle).await.unwrap();
    events(stream).await;
    driver.shutdown(handle).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn a_stale_prompt_waiting_for_admission_cannot_start_a_new_turn() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let state = driver.sessions.lock().unwrap()[&old.native_session_id].clone();
    let gate = state.admission.lock().await;
    let queued = tokio::spawn({
        let driver = driver.clone();
        let old = old.clone();
        let request = prompt(root.path(), "must not send");
        async move { driver.prompt(&old, request).await }
    });
    tokio::task::yield_now().await;
    let current = SessionHandle::scoped(old.native_session_id.clone());
    {
        let _gate = state.close_result.lock().unwrap();
        *state.scope.lock().unwrap() = current.clone();
    }
    drop(gate);
    assert!(queued.await.unwrap().is_err());
    assert_eq!(count(root.path(), "session/send"), 0);
    assert!(!state.turn_active.load(Ordering::Acquire));
    driver.shutdown(current).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn duplicate_native_create_identity_does_not_replace_or_close_an_owner() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| s.replace("next_session+=1", "next_session=1"));
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let state = driver.sessions.lock().unwrap()[&old.native_session_id].clone();
    let result = driver
        .create_session(SessionRequest {
            working_directory: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            history_mcp: None,
            permission_mode: None,
        })
        .await;
    assert!(matches!(result, Err(DriverError::SessionIncompatible(_))));
    assert!(Arc::ptr_eq(
        &state,
        &driver.sessions.lock().unwrap()[&old.native_session_id]
    ));
    assert_eq!(count(root.path(), "session/close"), 0);
    assert!(!native(&driver).await.stopping());
    finish(&driver, &old, root.path()).await;
    driver.shutdown(old).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn cancelling_known_candidate_subscription_closes_only_that_logical_session() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace(" elif method=='session/read':", " elif method=='session/subscribe' and id=='session-2':\n  while not os.path.exists('allow-subscribe'):time.sleep(0.01)\n  reply(r)\n elif method=='session/read':")
    });
    let driver = driver(launch);
    let other = session(&driver, root.path()).await;
    let worker = native(&driver).await;
    let opening = tokio::spawn({
        let driver = driver.clone();
        let path = root.path().to_path_buf();
        async move { session(&driver, &path).await }
    });
    wait_for(|| count(root.path(), "session/subscribe") == 2).await;
    let state = driver.sessions.lock().unwrap()["session-2"].clone();
    let candidate = state.handle();
    let mut proof = driver.shutdown_proof(&candidate).await.unwrap();
    opening.abort();
    assert!(opening.await.unwrap_err().is_cancelled());
    assert!(state.closing.load(Ordering::Acquire));
    assert!(!worker.stopping());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    std::fs::write(root.path().join("allow-subscribe"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(5), &mut proof)
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/close"), 1);
    finish(&driver, &other, root.path()).await;
    driver.shutdown(other).await.unwrap();
    stop(driver).await;
}

fn selection_fixture(broken: bool, blocked: bool) -> (tempfile::TempDir, ZcodeLaunch) {
    let (root, launch) = fixture("normal");
    edit(&launch, |mut s| {
        s=s.replace("sessions={};next_session=0;close_count=0",&format!("sessions={{}};next_session=0;close_count=0\nchosen={{'providerId':'test','modelId':'old'}};thought='medium'\nbroken={};blocked={}",if broken{"True"}else{"False"},if blocked{"True"}else{"False"}));
        let before = s
            .lines()
            .find(|l| l.starts_with("def snap("))
            .unwrap()
            .to_owned();
        s=s.replace(&before,"def snap(id):return {'sessionId':id,'session':{'sessionId':id,'workspace':{'workspacePath':os.getcwd()},'status':'idle'},'projection':{'currentTurnId':None},'settings':{'model':{'current':chosen,'available':[{'ref':{'providerId':'test','modelId':'old'}},{'ref':{'providerId':'test','modelId':'new'}}]},'thoughtLevel':{'enabled':True,'current':thought}}}");
        s=s.replace(" elif method=='session/read':",r" elif method=='session/setModel':
  if blocked and r['params']['model']['modelId']=='new':
   while not os.path.exists('allow-model'):time.sleep(0.01)
  if not broken or r['params']['model']['modelId']!='old':chosen=r['params']['model']
  reply(r,snap(id))
 elif method=='session/setThoughtLevel':
  if r['params']['thoughtLevel']=='high':out({'id':r['id'],'error':{'message':'selection rejected'}})
  else:thought=r['params']['thoughtLevel'];reply(r,snap(id))
 elif method=='session/read':");
        s
    });
    (root, launch)
}
fn selected_request(root: &Path) -> SessionRequest {
    SessionRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        history_mcp: None,
        permission_mode: None,
    }
}
const NEW: &str = r#"{"providerId":"test","modelId":"new"}"#;
#[tokio::test]
async fn failed_partial_selection_rolls_back_before_scope_handoff() {
    let (root, launch) = selection_fixture(false, false);
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let result = driver
        .resume_selected_session(
            &old.native_session_id,
            selected_request(root.path()),
            Some(NEW),
            Some("high"),
        )
        .await;
    assert_eq!(
        driver_api::failure::kind(&result.unwrap_err()),
        "selection_rejected"
    );
    assert!(
        driver
            .current_session_handle(&old.native_session_id)
            .await
            .unwrap()
            .same_scope(&old)
    );
    assert_eq!(count(root.path(), "session/close"), 0);
    finish(&driver, &old, root.path()).await;
    let current = driver
        .resume_selected_session(
            &old.native_session_id,
            selected_request(root.path()),
            Some(NEW),
            Some("low"),
        )
        .await
        .unwrap();
    assert!(!old.same_scope(&current));
    let requests = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    for request in requests
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .filter(|r| {
            matches!(
                r["method"].as_str(),
                Some("session/setModel" | "session/setThoughtLevel")
            )
        })
    {
        assert_eq!(request["params"]["persistAsWorkspaceLastUsed"], false);
    }
    driver.shutdown(current).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn unverified_rollback_retires_only_the_original_session_before_reporting_failure() {
    let (root, launch) = selection_fixture(true, false);
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let other = session(&driver, root.path()).await;
    let mut proof = driver.shutdown_proof(&old).await.unwrap();
    assert!(
        driver
            .resume_selected_session(
                &old.native_session_id,
                selected_request(root.path()),
                Some(NEW),
                Some("high")
            )
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), &mut proof)
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/close"), 1);
    assert!(!native(&driver).await.stopping());
    finish(&driver, &other, root.path()).await;
    driver.shutdown(other).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn abandoned_selection_keeps_the_original_scope_retiring_until_confirmed() {
    let (root, launch) = selection_fixture(false, true);
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let other = session(&driver, root.path()).await;
    let mut proof = driver.shutdown_proof(&old).await.unwrap();
    let selecting = tokio::spawn({
        let driver = driver.clone();
        let old = old.clone();
        let request = selected_request(root.path());
        async move {
            driver
                .resume_selected_session(&old.native_session_id, request, Some(NEW), Some("low"))
                .await
        }
    });
    wait_for(|| count(root.path(), "session/setModel") == 1).await;
    selecting.abort();
    assert!(selecting.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    std::fs::write(root.path().join("allow-model"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(5), &mut proof)
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/close"), 1);
    assert!(!native(&driver).await.stopping());
    finish(&driver, &other, root.path()).await;
    driver.shutdown(other).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn foreign_session_approval_and_late_labelled_terminal_do_not_affect_the_current_turn() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace("  if r['params']['content']!='slow':",r"  if r['params']['content']=='labels':
   turn='turn-'+str(r['id'])
   out({'method':'turn.completed','params':{'sessionId':id,'turnId':'old-turn','resultType':'success'}})
   out({'method':'stream.delta','params':{'sessionId':'foreign','turnId':turn,'kind':'text_delta','delta':'FOREIGN'}})
   out({'id':'foreign-permission','method':'interaction/requestPermission','params':{'sessionId':'foreign','turnId':turn,'requestId':'permission','toolName':'bash','options':[{'kind':'allow_once','name':'Allow','optionId':'allow_once','response':{'decision':'allow'}}]}})
   out({'method':'stream.delta','params':{'sessionId':id,'turnId':turn,'kind':'text_delta','delta':'CURRENT'}})
   out({'method':'turn.completed','params':{'sessionId':id,'turnId':turn,'resultType':'success'}})
  if r['params']['content'] not in ['slow','labels']:")
    });
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let observed = events(
        driver
            .prompt(&handle, prompt(root.path(), "labels"))
            .await
            .unwrap(),
    )
    .await;
    let text: Vec<_> = observed
        .iter()
        .filter_map(|e| match e {
            Ok(DriverEvent::TextDelta { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, vec!["CURRENT"]);
    assert!(
        !observed
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::ApprovalRequested { .. })))
    );
    assert_eq!(
        observed
            .iter()
            .filter(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
            .count(),
        1
    );
    let requests = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    assert!(
        requests
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .any(|r| r["id"] == "foreign-permission" && r["result"]["decision"] == "deny")
    );
    driver.shutdown(handle).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn abandoned_startup_before_identity_keeps_original_worker_supervision() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace("  sessions[id]=True;reply(r,snap(id))", "  sessions[id]=True\n  if id=='session-2':\n   while not os.path.exists('allow-create'):time.sleep(0.01)\n  reply(r,snap(id))")
    });
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    let original = native(&driver).await;
    let opening = tokio::spawn({
        let driver = driver.clone();
        let root = root.path().to_path_buf();
        async move { session(&driver, &root).await }
    });
    wait_for(|| count(root.path(), "session/create") == 2).await;
    opening.abort();
    assert!(opening.await.unwrap_err().is_cancelled());
    assert!(original.stopping());
    // No native session ID was delivered, so cleanup must retain the exact OS worker.
    tokio::time::timeout(Duration::from_secs(7), original.confirmed())
        .await
        .unwrap();
    let restored = driver
        .resume_session(&old.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(!old.same_scope(&restored));
    assert!(driver.interrupt(&old).await.is_err());
    finish(&driver, &restored, root.path()).await;
    driver.shutdown(restored).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn embedded_reasoning_selection_does_not_use_a_disabled_thought_level_api() {
    let (root, launch) = selection_fixture(false, false);
    edit(&launch, |s| {
        s.replace(
            "'thoughtLevel':{'enabled':True",
            "'thoughtLevel':{'enabled':False",
        )
    });
    let driver = driver(launch);
    let selected = driver
        .create_selected_session(
            selected_request(root.path()),
            Some(r#"{"providerId":"test","modelId":"new","options":{"reasoningLevel":"medium"}}"#),
            Some("high"),
        )
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/setModel"), 1);
    assert_eq!(count(root.path(), "session/setThoughtLevel"), 0);
    {
        let mut connection = driver.connection.lock().await;
        let value = connection
            .as_mut()
            .unwrap()
            .request(
                "session/read",
                json!({"sessionId":selected.native_session_id}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            value["settings"]["model"]["current"]["options"]["reasoningLevel"],
            "high"
        );
    }
    driver.shutdown(selected).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn repeated_native_permission_ids_cannot_reuse_an_old_ui_answer_in_the_same_handle() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace("sessions={};next_session=0;close_count=0","sessions={};next_session=0;close_count=0;pending=None")
        .replace(" if method=='session/list':",r" if method is None:
  if pending:
   out({'method':'turn.completed','params':{'sessionId':pending,'resultType':'success'}});pending=None
  continue
 if method=='session/list':")
        .replace("  if r['params']['content']!='slow':",r"  if r['params']['content']=='approval':
   pending=id
   out({'id':'reused-server-id','method':'interaction/requestPermission','params':{'sessionId':id,'requestId':'reused-request-id','toolName':'bash','input':'echo private','options':[{'kind':'allow_once','name':'Allow once','optionId':'allow_once','response':{'decision':'allow'}}]}})
  if r['params']['content'] not in ['slow','approval']:")
    });
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let mut previous: Option<String> = None;
    for _ in 0..2 {
        let mut stream = driver
            .prompt(&handle, prompt(root.path(), "approval"))
            .await
            .unwrap();
        let id = tokio::time::timeout(Duration::from_secs(4), async {
            while let Some(event) = stream.next().await {
                if let Ok(DriverEvent::ApprovalRequested {
                    native_request_id, ..
                }) = event
                {
                    return native_request_id;
                }
            }
            panic!("approval stream ended");
        })
        .await
        .unwrap();
        if let Some(previous) = previous.as_ref() {
            assert_ne!(&id, previous);
            assert!(
                driver
                    .respond_to_interaction(&handle, previous, "allow_once")
                    .await
                    .is_err()
            );
        }
        driver
            .respond_to_interaction(&handle, &id, "allow_once")
            .await
            .unwrap();
        assert!(
            events(stream)
                .await
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        previous = Some(id);
    }
    driver.shutdown(handle).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn explicit_close_refusal_does_not_satisfy_the_original_passive_proof() {
    let (root, launch) = fixture("false");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let mut proof = driver.shutdown_proof(&handle).await.unwrap();
    assert!(driver.shutdown(handle.clone()).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    assert!(
        driver
            .sessions
            .lock()
            .unwrap()
            .contains_key(&handle.native_session_id)
    );
    driver.shutdown(handle).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), &mut proof)
        .await
        .unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn concurrent_cold_restores_share_one_native_installation_and_return_distinct_authority() {
    let (root, launch) = fixture("normal");
    let driver = driver(launch);
    let workspace = root.path().to_str().unwrap();
    let (first, second) = tokio::join!(
        driver.resume_session("cold-native", workspace),
        driver.resume_session("cold-native", workspace)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(!first.same_scope(&second));
    assert_eq!(count(root.path(), "session/resume"), 1);
    let current = driver.current_session_handle("cold-native").await.unwrap();
    let old = if current.same_scope(&first) {
        second
    } else {
        first
    };
    assert!(driver.shutdown(old).await.is_err());
    finish(&driver, &current, root.path()).await;
    driver.shutdown(current).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn foreign_workspace_snapshot_is_rejected_without_configuring_or_closing_it() {
    let (root, launch) = fixture("normal");
    let other = tempfile::tempdir().unwrap();
    let replacement = format!(
        "'workspacePath':{}",
        serde_json::to_string(other.path().to_str().unwrap()).unwrap()
    );
    edit(&launch, |s| {
        s.replace("'workspacePath':os.getcwd()", &replacement)
    });
    let driver = driver(launch);
    let request = selected_request(root.path());
    assert!(
        driver
            .create_selected_session(request, Some(NEW), None)
            .await
            .is_err()
    );
    assert!(driver.sessions.lock().unwrap().is_empty());
    assert_eq!(count(root.path(), "session/setModel"), 0);
    assert_eq!(count(root.path(), "session/subscribe"), 0);
    assert_eq!(count(root.path(), "session/close"), 0);
    assert!(!native(&driver).await.stopping());
    stop(driver).await;
}

#[tokio::test]
async fn known_old_turn_labels_remain_fenced_after_cold_restore_on_the_same_worker() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace("sessions={};next_session=0;close_count=0","sessions={};next_session=0;close_count=0;previous_turn=None")
        .replace("  reply(r);out({'method':'turn.started','params':{'sessionId':id,'turnId':'turn-'+str(r['id'])}})",r"  reply(r)
  if previous_turn:
   out({'method':'turn.started','params':{'sessionId':id,'turnId':previous_turn}})
   out({'method':'turn.completed','params':{'sessionId':id,'turnId':previous_turn,'resultType':'success'}})
  previous_turn='turn-'+str(r['id'])
  out({'method':'turn.started','params':{'sessionId':id,'turnId':previous_turn}})
  out({'method':'stream.delta','params':{'sessionId':id,'turnId':previous_turn,'kind':'text_delta','delta':'CURRENT'}})")
    });
    let driver = driver(launch);
    let old = session(&driver, root.path()).await;
    finish(&driver, &old, root.path()).await;
    driver.shutdown(old.clone()).await.unwrap();
    let current = driver
        .resume_session(&old.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    let observed = events(
        driver
            .prompt(&current, prompt(root.path(), "hello"))
            .await
            .unwrap(),
    )
    .await;
    let started: Vec<_> = observed
        .iter()
        .filter_map(|e| {
            if let Ok(DriverEvent::TurnStarted {
                native_turn_id: Some(id),
            }) = e
            {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();
    let requests = std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap();
    let sent: Vec<Value> = requests
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .filter(|r: &Value| r["method"] == "session/send")
        .collect();
    assert_eq!(
        started,
        vec![format!(
            "turn-{}",
            sent.last().unwrap()["id"].as_u64().unwrap()
        )]
    );
    assert!(
        observed
            .iter()
            .any(|e| matches!(e,Ok(DriverEvent::TextDelta {text,..}) if text=="CURRENT"))
    );
    driver.shutdown(current).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn explicit_missing_restore_preserves_other_sessions_on_the_original_worker() {
    let (root, launch) = fixture("normal");
    edit(&launch, |s| {
        s.replace("  sessions[id]=True;reply(r,snap(id))","  if method=='session/resume' and id=='missing':out({'id':r['id'],'error':{'code':-32000,'message':'Session not found: private','data':{'code':'session_not_found'}}});continue\n  sessions[id]=True;reply(r,snap(id))")
    });
    let driver = driver(launch);
    let existing = session(&driver, root.path()).await;
    let worker = native(&driver).await;
    assert!(matches!(
        driver
            .resume_session("missing", root.path().to_str().unwrap())
            .await,
        Err(DriverError::SessionMissing(_))
    ));
    assert!(!worker.stopping());
    assert!(Arc::ptr_eq(&worker, &native(&driver).await));
    finish(&driver, &existing, root.path()).await;
    driver.shutdown(existing).await.unwrap();
    stop(driver).await;
}

#[tokio::test]
async fn original_worker_exit_alone_satisfies_its_captured_passive_session_proof() {
    let (root, launch) = fixture("stubborn");
    let driver = driver(launch);
    let handle = session(&driver, root.path()).await;
    let original = native(&driver).await;
    let mut proof = driver.shutdown_proof(&handle).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    assert!(!original.stopping());
    assert_eq!(count(root.path(), "session/close"), 0);
    drop(driver);
    tokio::time::timeout(Duration::from_secs(7), &mut proof)
        .await
        .unwrap();
    original.closed().await.unwrap();
    assert_eq!(count(root.path(), "session/close"), 0);
}
