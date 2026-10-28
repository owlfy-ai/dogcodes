//! Only synthetic ACP processes; no installed agents or model requests.
use super::*;

async fn receipt_done(mut receipt: turn::Receipt) -> Result<(), DriverError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(result) = receipt.borrow_and_update().clone() {
                return result;
            }
            receipt.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn native_idle_close_releases_one_session_and_preserves_its_peer() {
    let (root, driver) = fixture("native-close");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (scope, finished) = original(&driver).await;
    driver.shutdown(a.clone()).await.unwrap();
    assert_eq!(count(root.path(), "session/close"), 1);
    assert!(!scope.stopping());
    assert!(!finished.load(Ordering::Acquire));
    assert!(
        driver
            .prompt(&a, prompt(root.path(), "closed"))
            .await
            .is_err()
    );
    let events = driver
        .prompt(&b, prompt(root.path(), "peer"))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let a = driver
        .resume_session(&a.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/load"), 1);
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
    assert_eq!(count(root.path(), "session/close"), 3);
    assert!(finished.load(Ordering::Acquire));
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}

#[tokio::test]
async fn native_close_ack_settles_a_missing_prompt_response_without_rebuilding_peers() {
    for provider in ["cursor", "codebuddy"] {
        let (root, driver) = fixture_as("native-close", provider);
        let a = driver.create_session(request(root.path())).await.unwrap();
        let b = driver.create_session(request(root.path())).await.unwrap();
        let (scope, finished) = original(&driver).await;
        let stream = driver
            .prompt(&a, prompt(root.path(), "slow"))
            .await
            .unwrap();
        wait_for(|| count(root.path(), "session/prompt") == 1).await;
        tokio::time::timeout(Duration::from_secs(3), driver.shutdown(a))
            .await
            .unwrap()
            .unwrap();
        let events = tokio::time::timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnInterrupted { .. })))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        assert!(
            !driver.shared.reconnect_required.load(Ordering::Acquire),
            "{provider}"
        );
        assert!(!scope.stopping());
        let events = driver
            .prompt(&b, prompt(root.path(), "peer"))
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        assert_eq!(count(root.path(), "initialize"), 1);
        driver.shutdown(b).await.unwrap();
        assert!(finished.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn cancelled_idle_close_waiter_keeps_one_request_and_does_not_block_peer_cancellation() {
    let (root, driver) = fixture("native-close-delayed");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let waiting = tokio::spawn({
        let driver = driver.clone();
        let a = a.clone();
        async move { driver.shutdown(a).await }
    });
    wait_for(|| count(root.path(), "session/close") == 1).await;
    waiting.abort();
    let _ = waiting.await;
    let receipt = sessions::begin_close(
        driver.shared.clone(),
        &a.native_session_id,
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    assert!(receipt.borrow().is_none());
    assert!(driver.create_session(request(root.path())).await.is_err());
    assert!(
        driver
            .resume_session(&a.native_session_id, &root.path().to_string_lossy())
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "session/new"), 2);
    assert_eq!(count(root.path(), "session/load"), 0);
    let stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    driver.interrupt(&b).await.unwrap();
    let _ = stream.collect::<Vec<_>>().await;
    assert_eq!(count(root.path(), "session/close"), 1);
    assert!(receipt.borrow().is_none());
    std::fs::write(root.path().join("close-ready"), b"").unwrap();
    receipt_done(receipt).await.unwrap();
    driver.shutdown(a).await.unwrap();
    assert_eq!(
        count(root.path(), "session/close"),
        1,
        "retry must reuse the same receipt"
    );
    driver.shutdown(b).await.unwrap();
}

#[tokio::test]
async fn failed_or_malformed_native_close_stays_owned_until_last_peer_exits() {
    for mode in [
        "native-close-error",
        "native-close-malformed",
        "native-close-stuck",
    ] {
        let (root, driver) = fixture(mode);
        let a = driver.create_session(request(root.path())).await.unwrap();
        let b = driver.create_session(request(root.path())).await.unwrap();
        let (scope, finished) = original(&driver).await;
        let receipt = sessions::begin_close(
            driver.shared.clone(),
            &a.native_session_id,
            Duration::from_millis(80),
        )
        .await
        .unwrap();
        assert!(receipt_done(receipt.clone()).await.is_err(), "{mode}");
        assert!(driver.shutdown(a.clone()).await.is_err());
        assert_eq!(count(root.path(), "session/close"), 1);
        assert!(!scope.stopping());
        assert!(driver.create_session(request(root.path())).await.is_err());
        let events = driver
            .prompt(&b, prompt(root.path(), "peer"))
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
        );
        let final_receipt = sessions::begin_close(
            driver.shared.clone(),
            &b.native_session_id,
            Duration::from_millis(80),
        )
        .await
        .unwrap();
        receipt_done(final_receipt).await.unwrap();
        assert!(finished.load(Ordering::Acquire));
        wait_for(|| matches!(*receipt.borrow(), Some(Ok(())))).await;
        driver.shutdown(a).await.unwrap();
        assert_eq!(count(root.path(), "session/close"), 2, "{mode}");
        assert!(ids(root.path()).iter().all(|id| !present(*id)));
    }
}

#[tokio::test]
async fn passive_shutdown_proof_keeps_the_original_resident_after_an_error_without_native_retries()
{
    let (root, driver) = fixture("native-close-error");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let mut proof = driver
        .shutdown_proof(&a)
        .await
        .expect("original passive proof");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut proof)
            .await
            .is_err()
    );
    assert_eq!(
        count(root.path(), "session/close"),
        0,
        "observation sent native work"
    );
    assert!(driver.shutdown(a.clone()).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut proof)
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "session/close"), 1);
    driver.shutdown(b).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), proof)
        .await
        .unwrap();
    assert_eq!(count(root.path(), "session/close"), 2);
    assert!(
        driver
            .shutdown_proof(&SessionHandle::new(a.native_session_id))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn active_close_timeout_blocks_new_work_until_the_same_late_ack_arrives() {
    let (root, driver) = fixture("native-close-delayed");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (scope, _) = original(&driver).await;
    let stream = driver
        .prompt(&a, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let receipt = sessions::begin_close(
        driver.shared.clone(),
        &a.native_session_id,
        Duration::from_millis(80),
    )
    .await
    .unwrap();
    assert!(receipt_done(receipt.clone()).await.is_err());
    assert!(
        driver
            .prompt(&b, prompt(root.path(), "must not run"))
            .await
            .is_err()
    );
    assert!(driver.create_session(request(root.path())).await.is_err());
    assert_eq!(count(root.path(), "session/prompt"), 1);
    assert_eq!(count(root.path(), "session/close"), 1);
    assert!(!scope.stopping());
    std::fs::write(root.path().join("close-ready"), b"").unwrap();
    wait_for(|| matches!(*receipt.borrow(), Some(Ok(())))).await;
    let _ = stream.collect::<Vec<_>>().await;
    let events = driver
        .prompt(&b, prompt(root.path(), "peer"))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(b).await.unwrap();
}

#[tokio::test]
async fn unresponsive_last_native_owner_is_forced_closed_before_reporting_success() {
    let (root, driver) = fixture("native-close-stuck");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let (_, finished) = original(&driver).await;
    let original_pids = ids(root.path());
    let receipt = sessions::begin_close(
        driver.shared.clone(),
        &a.native_session_id,
        Duration::from_millis(80),
    )
    .await
    .unwrap();
    assert!(receipt.borrow().is_none());
    assert!(
        driver
            .shared
            .sessions
            .lock()
            .unwrap()
            .contains_key(&a.native_session_id)
    );
    receipt_done(receipt).await.unwrap();
    assert!(finished.load(Ordering::Acquire));
    assert!(original_pids.iter().all(|id| !present(*id)));
    assert_eq!(count(root.path(), "session/close"), 1);
    driver.shutdown(a).await.unwrap();
}

#[tokio::test]
async fn closing_a_previously_used_idle_session_cannot_settle_an_active_peer_turn() {
    let (root, driver) = fixture("native-close");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let _ = driver
        .prompt(&a, prompt(root.path(), "completed"))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let stream = driver
        .prompt(&b, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 2).await;
    let turn_b = driver
        .shared
        .latest_turn
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    driver.shutdown(a).await.unwrap();
    assert!(driver.shared.turn_active.load(Ordering::Acquire));
    assert!(turn_b.done.borrow().is_none());
    assert!(Arc::ptr_eq(
        driver.shared.latest_turn.lock().unwrap().as_ref().unwrap(),
        &turn_b
    ));
    assert!(!*turn_b.native_closed.borrow());
    assert!(!driver.shared.interrupt_requested.load(Ordering::Acquire));
    assert_eq!(count(root.path(), "session/cancel"), 0);
    driver.interrupt(&b).await.unwrap();
    let events = stream.collect::<Vec<_>>().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    driver.shutdown(b).await.unwrap();
}

#[tokio::test]
async fn forced_last_active_close_settles_local_ownership_before_a_replacement_can_start() {
    let (root, driver) = fixture("native-close-stuck");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let (_, old_epoch) = original(&driver).await;
    let stream = driver
        .prompt(&a, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    let receipt = sessions::begin_close(
        driver.shared.clone(),
        &a.native_session_id,
        Duration::from_millis(80),
    )
    .await
    .unwrap();
    receipt_done(receipt).await.unwrap();
    assert!(old_epoch.load(Ordering::Acquire));
    assert!(!driver.shared.turn_active.load(Ordering::Acquire));
    assert!(driver.shared.active_session.lock().unwrap().is_none());
    assert!(driver.shared.latest_turn.lock().unwrap().is_none());
    let events = stream.collect::<Vec<_>>().await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (_, new_epoch) = original(&driver).await;
    assert!(!Arc::ptr_eq(&old_epoch, &new_epoch));
    let events = driver
        .prompt(&b, prompt(root.path(), "replacement"))
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let receipt = sessions::begin_close(
        driver.shared.clone(),
        &b.native_session_id,
        Duration::from_millis(80),
    )
    .await
    .unwrap();
    receipt_done(receipt).await.unwrap();
}
