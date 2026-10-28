//! Private RPC fixtures; no installed Agent or real model requests.
use super::{
    tests::{FAKE, events, fixture_with, prompt},
    *,
};
use driver_api::{DriverEvent, QuotaProvider};
use futures::StreamExt;
use std::{path::Path, time::Duration};

async fn wait_for(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn count(root: &Path, kind: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|request| request["type"] == kind)
        .count()
}

#[tokio::test]
async fn hot_reuse_rejects_old_and_captured_handles_without_answering_the_new_dialog() {
    let (root, driver) = fixture_with(FAKE);
    let original = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let captured = driver.session_for(&original).await.unwrap().connection;
    let successor = driver
        .resume_session(&original.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(!original.same_scope(&successor));
    let mut stream = driver
        .prompt(&successor, prompt(root.path(), "question"))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(event, DriverEvent::QuestionRequested { .. }));
    assert!(matches!(
        driver
            .prompt(&original, prompt(root.path(), "must not run"))
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver.interrupt(&original).await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        captured.abort_turn(&original, Duration::from_secs(1)).await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        captured.begin_turn(&original),
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(matches!(
        driver
            .respond_to_question(
                &original,
                "question-1",
                QuestionAnswer {
                    cancelled: true,
                    values: Value::Null
                }
            )
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(driver.shutdown(original.clone()).await.is_err());
    assert!(driver.shutdown_proof(&original).await.is_none());
    assert!(
        driver
            .session_for(&SessionHandle::new(original.native_session_id))
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "abort"), 0);
    assert_eq!(count(root.path(), "extension_ui_response"), 0);
    assert!(
        captured
            .state
            .lock()
            .unwrap()
            .turn
            .as_ref()
            .unwrap()
            .dialogs
            .contains_key("question-1")
    );
    driver
        .respond_to_question(
            &successor,
            "question-1",
            QuestionAnswer {
                cancelled: false,
                values: json!({"answer":"Allow"}),
            },
        )
        .await
        .unwrap();
    assert!(
        events(stream)
            .await
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn waiting_prompt_rechecks_its_captured_scope_after_admission_changes() {
    let (root, driver) = fixture_with(FAKE);
    let original = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let connection = driver.session_for(&original).await.unwrap().connection;
    let held = connection.admission.lock().await;
    let mut resuming =
        Box::pin(driver.resume_session(&original.native_session_id, root.path().to_str().unwrap()));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut resuming)
            .await
            .is_err()
    );
    let mut sending = Box::pin(driver.prompt(&original, prompt(root.path(), "stale")));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut sending)
            .await
            .is_err()
    );
    drop(held);
    let successor = resuming.await.unwrap();
    assert!(matches!(
        sending.await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert_eq!(count(root.path(), "prompt"), 0);
    assert!(!connection.state.lock().unwrap().disconnected);
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn passive_exit_proof_and_scoped_receipts_do_not_target_a_cold_replacement() {
    let (root, driver) = fixture_with(FAKE);
    let original = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let connection = driver.session_for(&original).await.unwrap().connection;
    events(
        driver
            .prompt(&original, prompt(root.path(), "first"))
            .await
            .unwrap(),
    )
    .await;
    let mut proof = driver.shutdown_proof(&original).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut proof)
            .await
            .is_err()
    );
    assert!(!connection.state.lock().unwrap().disconnected);
    driver.shutdown(original.clone()).await.unwrap();
    driver.shutdown(original.clone()).await.unwrap();
    assert!(
        driver
            .shutdown(SessionHandle::new(original.native_session_id.clone()))
            .await
            .is_err()
    );
    let successor = driver
        .resume_session(&original.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    let replacement = driver.session_for(&successor).await.unwrap().connection;
    assert!(!Arc::ptr_eq(&connection, &replacement));
    tokio::time::timeout(Duration::from_secs(1), proof)
        .await
        .unwrap();
    assert!(driver.shutdown(original).await.is_err());
    assert!(!replacement.state.lock().unwrap().disconnected);
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn blocked_reconfiguration_does_not_hold_the_global_map_or_another_session() {
    let script = FAKE.replace(
        "elif t=='set_model':",
        "elif t=='set_model':\n  if r['modelId']=='blocked':\n   import time\n   time.sleep(60)",
    );
    let (root, driver) = fixture_with(&script);
    let first = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let peer = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let mut changing = Box::pin(driver.resume_selected_session(
        &first.native_session_id,
        session_request(root.path().to_str().unwrap()),
        Some("test/blocked"),
        None,
    ));
    tokio::select! {
        result = &mut changing => panic!("native configuration should still be pending: {result:?}"),
        () = wait_for(|| count(root.path(), "set_model") == 1) => {},
    }
    let stream = tokio::time::timeout(
        Duration::from_secs(2),
        driver.prompt(&peer, prompt(root.path(), "peer")),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        events(stream)
            .await
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    tokio::time::timeout(Duration::from_secs(2), driver.shutdown(peer))
        .await
        .unwrap()
        .unwrap();
    drop(changing);
    assert!(
        driver
            .connection(&first.native_session_id)
            .await
            .unwrap()
            .state
            .lock()
            .unwrap()
            .disconnected
    );
    driver.shutdown(first).await.unwrap();
}

#[tokio::test]
async fn partial_selection_is_rolled_back_before_a_failed_reuse_returns() {
    let (root, driver) = fixture_with(FAKE);
    let original = driver
        .create_selected_session(
            session_request(root.path().to_str().unwrap()),
            Some("test/model"),
            Some("high"),
        )
        .await
        .unwrap();
    assert_eq!(
        driver
            .resume_selected_session(
                &original.native_session_id,
                session_request(root.path().to_str().unwrap()),
                Some("test/other"),
                Some("unsupported")
            )
            .await
            .unwrap_err(),
        DriverError::Protocol("selection_rejected".into())
    );
    assert!(
        driver
            .current_session_handle(&original.native_session_id)
            .await
            .unwrap()
            .same_scope(&original)
    );
    let connection = driver.session_for(&original).await.unwrap().connection;
    let state = connection.call(json!({"type":"get_state"})).await.unwrap();
    assert_eq!(model_id(&state["model"]).as_deref(), Some("test/model"));
    assert_eq!(state["thinkingLevel"], "high");
    assert!(!connection.state.lock().unwrap().disconnected);
    assert!(
        events(
            driver
                .prompt(&original, prompt(root.path(), "continue"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(original).await.unwrap();
}

#[tokio::test]
async fn a_late_prompt_rejection_does_not_consume_the_next_turn_on_the_same_pipe() {
    let script = FAKE.replace("if r['message']=='question':", "if r['message']=='old':\n   delayed=r;finish();continue\n  if r['message']=='new':\n   reply(r);out({'type':'response','id':delayed['id'],'success':False});continue\n  if r['message']=='question':");
    let (root, driver) = fixture_with(&script);
    let original = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let baseline = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    assert!(
        events(
            driver
                .prompt(&original, prompt(root.path(), "old"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let successor = driver
        .resume_session(&original.native_session_id, root.path().to_str().unwrap())
        .await
        .unwrap();
    let stream = driver
        .prompt(&successor, prompt(root.path(), "new"))
        .await
        .unwrap();
    let connection = driver.session_for(&successor).await.unwrap().connection;
    connection.read_stats().await.unwrap();
    // Both receipt observers must have actually consumed their replies before
    // testing the new turn; a sleep alone would leave a scheduling loophole.
    wait_for(|| {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
            <= baseline
    })
    .await;
    assert!(connection.state.lock().unwrap().turn.is_some());
    assert!(!connection.state.lock().unwrap().disconnected);
    driver.interrupt(&successor).await.unwrap();
    let output = events(stream).await;
    assert!(
        output
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(!output.iter().any(Result::is_err));
    driver.shutdown(successor).await.unwrap();
}

#[tokio::test]
async fn cancelling_statistics_discards_only_that_query_and_keeps_the_running_turn() {
    let script = FAKE.replace(
        "elif t=='abort':",
        "elif t=='get_session_stats':continue\n elif t=='abort':",
    );
    let (root, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let stream = driver
        .prompt(&handle, prompt(root.path(), "slow"))
        .await
        .unwrap();
    let connection = driver.session_for(&handle).await.unwrap().connection;
    let mut query = Box::pin(driver.read_session_usage(&handle.native_session_id));
    tokio::select! {
        _ = &mut query => panic!("statistics must await the native reply"),
        () = wait_for(|| count(root.path(), "get_session_stats") == 1) => {},
    }
    let query_id = std::fs::read_to_string(root.path().join("requests.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|r| r["type"] == "get_session_stats")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(query);
    assert!(
        !connection
            .state
            .lock()
            .unwrap()
            .pending
            .contains_key(&query_id)
    );
    assert!(!connection.state.lock().unwrap().disconnected);
    assert!(connection.state.lock().unwrap().turn.is_some());
    driver.interrupt(&handle).await.unwrap();
    assert!(
        events(stream)
            .await
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn a_duplicate_native_file_is_rejected_without_replacing_the_original_owner() {
    let script = FAKE.replace("str(uuid.uuid4())+'.jsonl'", "'same-session.jsonl'");
    let (root, driver) = fixture_with(&script);
    let original = driver
        .create_session(session_request(root.path().to_str().unwrap()))
        .await
        .unwrap();
    let connection = driver.session_for(&original).await.unwrap().connection;
    assert!(matches!(
        driver
            .create_session(session_request(root.path().to_str().unwrap()))
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(Arc::ptr_eq(
        &connection,
        &driver.session_for(&original).await.unwrap().connection
    ));
    assert!(!connection.state.lock().unwrap().disconnected);
    assert!(
        events(
            driver
                .prompt(&original, prompt(root.path(), "owner"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(original).await.unwrap();
}

#[tokio::test]
async fn an_initially_streaming_native_session_is_not_admitted_as_idle() {
    let script = FAKE.replace("'isStreaming':False", "'isStreaming':True");
    let (root, driver) = fixture_with(&script);
    assert!(matches!(
        driver
            .create_session(session_request(root.path().to_str().unwrap()))
            .await,
        Err(DriverError::SessionIncompatible(_))
    ));
    assert!(driver.sessions.lock().await.is_empty());
    assert_eq!(count(root.path(), "prompt"), 0);
}
