//! Original question bridges may outlive their logical prompt on a shared CLI.
use super::*;
use futures::FutureExt;

fn answer(cancelled: bool) -> agent_domain::QuestionAnswer {
    agent_domain::QuestionAnswer {
        cancelled,
        values: if cancelled {
            serde_json::Value::Null
        } else {
            serde_json::json!({"q":"Yes"})
        },
    }
}
async fn finish(stream: DriverEventStream) -> Vec<DriverEvent> {
    tokio::time::timeout(Duration::from_secs(4), stream.collect::<Vec<_>>())
        .await
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .collect()
}
async fn wait_for(root: &Path, method: &str, expected: usize) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while count(root, method) != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn has_bridge_error(events: &[DriverEvent]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            DriverEvent::ProtocolWarning { .. } | DriverEvent::QuestionClosed { .. }
        )
    })
}

#[tokio::test]
async fn stopping_codebuddy_does_not_send_a_second_vendor_question_action() {
    let (root, driver) = fixture_as("codebuddy-bridge", "codebuddy");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "first"))
        .await
        .unwrap();
    let id = question_id(&mut stream).await;
    driver.interrupt(&session).await.unwrap();
    let events = finish(stream).await;
    assert!(
        driver
            .respond_to_question(&session, &id, answer(false))
            .await
            .is_err()
    );
    driver.shutdown(session).await.unwrap();
    assert_eq!(count(root.path(), "_codebuddy.ai/resolveInterruption"), 0);
    assert!(!has_bridge_error(&events));
}

#[tokio::test]
async fn codebuddy_delivery_failure_still_reports_the_original_active_question() {
    let (root, driver) = fixture_as("codebuddy-bridge", "codebuddy");
    std::fs::write(root.path().join("resolve-ready"), "no").unwrap();
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "first"))
        .await
        .unwrap();
    let id = question_id(&mut stream).await;
    driver
        .respond_to_question(&session, &id, answer(false))
        .await
        .unwrap();
    let events = finish(stream).await;
    driver.shutdown(session).await.unwrap();
    assert_eq!(count(root.path(), "_codebuddy.ai/resolveInterruption"), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, DriverEvent::ProtocolWarning { .. }))
            .count(),
        1
    );
    assert_eq!(events.iter().filter(|e| matches!(e, DriverEvent::QuestionClosed { native_request_id } if *native_request_id == id)).count(), 1);
}

#[tokio::test]
async fn current_codebuddy_question_keeps_user_answer_and_cancel_semantics() {
    for cancelled in [false, true] {
        let (root, driver) = fixture_as("codebuddy-bridge", "codebuddy");
        std::fs::write(root.path().join("resolve-ready"), "yes").unwrap();
        let session = driver.create_session(request(root.path())).await.unwrap();
        let mut stream = driver
            .prompt(&session, prompt(root.path(), "first"))
            .await
            .unwrap();
        let id = question_id(&mut stream).await;
        driver
            .respond_to_question(&session, &id, answer(cancelled))
            .await
            .unwrap();
        let events = finish(stream).await;
        driver.shutdown(session.clone()).await.unwrap();
        assert!(!has_bridge_error(&events));
        let requests: Vec<serde_json::Value> =
            std::fs::read_to_string(root.path().join("requests.jsonl"))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .filter(|r: &serde_json::Value| r["method"] == "_codebuddy.ai/resolveInterruption")
                .collect();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["params"]["decision"],
            if cancelled { "deny" } else { "allow" }
        );
        assert_eq!(
            requests[0]["params"]["sessionId"],
            session.native_session_id
        );
        if cancelled {
            assert!(requests[0]["params"].get("answers").is_none());
        } else {
            assert_eq!(
                requests[0]["params"]["answers"]["q"],
                serde_json::json!(["Yes"])
            );
        }
    }
}

#[tokio::test]
async fn a_delayed_codebuddy_answer_cannot_be_dispatched_into_the_next_turn() {
    let (root, driver) = fixture_as("codebuddy-bridge", "codebuddy");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "first"))
        .await
        .unwrap();
    let id = question_id(&mut stream).await;
    // Hold the original accepted-answer boundary while the CLI ends its prompt.
    let sender = driver
        .shared
        .questions
        .lock()
        .unwrap()
        .remove(&id)
        .unwrap()
        .reply;
    std::fs::write(root.path().join("finish-first"), "").unwrap();
    finish(stream).await;
    let current = driver
        .resume_session(&session.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let mut next = driver
        .prompt(&current, prompt(root.path(), "next"))
        .await
        .unwrap();
    let next_id = question_id(&mut next).await;
    sender.send(answer(false)).unwrap();
    // The original RPC responder is still acknowledged, but no vendor action
    // can now use the reused session/tool ID to target the current prompt.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let log =
                std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap_or_default();
            if log
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .any(|r| r["id"] == "codebuddy-first" && r.get("method").is_none())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(count(root.path(), "_codebuddy.ai/resolveInterruption"), 0);
    assert!(
        driver
            .shared
            .questions
            .lock()
            .unwrap()
            .contains_key(&next_id)
    );
    std::fs::write(root.path().join("resolve-ready"), "yes").unwrap();
    driver
        .respond_to_question(&current, &next_id, answer(false))
        .await
        .unwrap();
    let events = finish(next).await;
    driver.shutdown(current).await.unwrap();
    assert!(!has_bridge_error(&events));
    assert_eq!(count(root.path(), "_codebuddy.ai/resolveInterruption"), 1);
}

#[tokio::test]
async fn late_codebuddy_delivery_failure_cannot_close_the_next_question() {
    let (root, driver) = fixture_as("codebuddy-bridge", "codebuddy");
    let session = driver.create_session(request(root.path())).await.unwrap();
    let mut stream = driver
        .prompt(&session, prompt(root.path(), "first"))
        .await
        .unwrap();
    let id = question_id(&mut stream).await;
    driver
        .respond_to_question(&session, &id, answer(false))
        .await
        .unwrap();
    wait_for(root.path(), "_codebuddy.ai/resolveInterruption", 1).await;
    std::fs::write(root.path().join("finish-first"), "").unwrap();
    finish(stream).await;
    let current = driver
        .resume_session(&session.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let mut next = driver
        .prompt(&current, prompt(root.path(), "next"))
        .await
        .unwrap();
    let next_id = question_id(&mut next).await;
    std::fs::write(root.path().join("resolve-ready"), "no").unwrap();
    let mut events = Vec::new();
    // The first raw response settles before its original permission response.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let log =
                std::fs::read_to_string(root.path().join("requests.jsonl")).unwrap_or_default();
            if log
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .any(|r| r["id"] == "codebuddy-first" && r.get("method").is_none())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Drain any immediately available events without making timeouts success.
    while let Some(Some(event)) = next.next().now_or_never() {
        events.push(event.unwrap());
    }
    assert!(
        !has_bridge_error(&events),
        "late delivery leaked to the current stream: {events:?}"
    );
    assert!(
        driver
            .shared
            .questions
            .lock()
            .unwrap()
            .contains_key(&next_id)
    );
    std::fs::write(root.path().join("resolve-ready"), "yes").unwrap();
    driver
        .respond_to_question(&current, &next_id, answer(false))
        .await
        .unwrap();
    events.extend(finish(next).await);
    driver.shutdown(current).await.unwrap();
    assert!(!has_bridge_error(&events));
    assert_eq!(count(root.path(), "_codebuddy.ai/resolveInterruption"), 2);
}
