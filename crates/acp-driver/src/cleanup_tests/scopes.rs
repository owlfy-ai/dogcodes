use super::*;

async fn finish(stream: DriverEventStream, interrupted: bool) {
    let events = tokio::time::timeout(Duration::from_secs(3), stream.collect::<Vec<_>>())
        .await
        .expect("private prompt did not settle");
    assert!(
        events.iter().any(|e| match e {
            Ok(DriverEvent::TurnCompleted { .. }) => !interrupted,
            Ok(DriverEvent::TurnInterrupted { .. }) => interrupted,
            _ => false,
        }),
        "unexpected terminal events: {events:?}"
    );
}

#[tokio::test]
async fn a_reloaded_session_rejects_old_raw_and_deserialized_handles_without_native_effects() {
    let (root, driver) = fixture("question");
    let old = driver.create_session(request(root.path())).await.unwrap();
    let peer = driver.create_session(request(root.path())).await.unwrap();
    let (_, physical) = original(&driver).await;
    driver.shutdown(old.clone()).await.unwrap();
    driver.shutdown(old.clone()).await.unwrap(); // Same original receipt is retryable.
    let current = driver
        .resume_session(&old.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    assert!(!current.same_scope(&old));
    let decoded: SessionHandle =
        serde_json::from_value(serde_json::to_value(&current).unwrap()).unwrap();
    let invalid = [
        old,
        SessionHandle::new(current.native_session_id.clone()),
        decoded,
        SessionHandle::scoped(current.native_session_id.clone()),
    ];
    for handle in &invalid {
        assert!(
            driver
                .prompt(handle, prompt(root.path(), "must-not-send"))
                .await
                .is_err()
        );
        assert!(driver.shutdown(handle.clone()).await.is_err());
    }
    assert_eq!(count(root.path(), "session/prompt"), 0);
    assert!(!physical.load(Ordering::Acquire));
    let mut stream = driver
        .prompt(&current, prompt(root.path(), "question"))
        .await
        .unwrap();
    let question = question_id(&mut stream).await;
    for handle in &invalid {
        assert!(driver.interrupt(handle).await.is_err());
        assert!(
            driver
                .respond_to_question(
                    handle,
                    &question,
                    agent_domain::QuestionAnswer {
                        cancelled: true,
                        values: serde_json::Value::Null,
                    }
                )
                .await
                .is_err()
        );
        assert!(
            driver
                .shared
                .questions
                .lock()
                .unwrap()
                .contains_key(&question)
        );
    }
    assert_eq!(count(root.path(), "session/cancel"), 0);
    driver
        .respond_to_question(
            &current,
            &question,
            agent_domain::QuestionAnswer {
                cancelled: true,
                values: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();
    finish(stream, true).await;
    driver.shutdown(current).await.unwrap();
    driver.shutdown(peer).await.unwrap();
    assert!(physical.load(Ordering::Acquire));
}

#[tokio::test]
async fn an_old_handle_cannot_approve_a_current_request_with_the_same_native_session_id() {
    let (root, driver) = fixture("permission");
    let old = driver.create_session(request(root.path())).await.unwrap();
    driver.shutdown(old.clone()).await.unwrap();
    let current = driver
        .resume_session(&old.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    let mut stream = driver
        .prompt(&current, prompt(root.path(), "permission"))
        .await
        .unwrap();
    let approval = approval_id(&mut stream).await;
    assert!(
        driver
            .respond_to_interaction(&old, &approval, "allow")
            .await
            .is_err()
    );
    assert!(
        driver
            .shared
            .pending_permissions
            .lock()
            .unwrap()
            .contains_key(&approval)
    );
    driver
        .respond_to_interaction(&current, &approval, "allow")
        .await
        .unwrap();
    finish(stream, false).await;
    driver.shutdown(current).await.unwrap();
}

#[tokio::test]
async fn identical_ids_on_a_new_physical_connection_do_not_revalidate_old_handles() {
    let (root, driver) = fixture("normal");
    let old = driver.create_session(request(root.path())).await.unwrap();
    let (_, first_exit) = original(&driver).await;
    driver.shutdown(old.clone()).await.unwrap();
    assert!(first_exit.load(Ordering::Acquire));
    let current = driver.create_session(request(root.path())).await.unwrap();
    let (_, second_exit) = original(&driver).await;
    assert_eq!(old.native_session_id, current.native_session_id);
    assert!(!old.same_scope(&current));
    assert!(
        driver
            .prompt(&old, prompt(root.path(), "stale"))
            .await
            .is_err()
    );
    assert!(driver.shutdown(old.clone()).await.is_err());
    let stream = driver
        .prompt(&current, prompt(root.path(), "slow"))
        .await
        .unwrap();
    wait_for(|| count(root.path(), "session/prompt") == 1).await;
    assert!(driver.interrupt(&old).await.is_err());
    assert_eq!(count(root.path(), "session/cancel"), 0);
    assert!(!second_exit.load(Ordering::Acquire));
    driver.interrupt(&current).await.unwrap();
    finish(stream, true).await;
    driver.shutdown(current.clone()).await.unwrap();
    driver.shutdown(current).await.unwrap();
    assert!(second_exit.load(Ordering::Acquire));
    assert_eq!(count(root.path(), "initialize"), 2);
    assert_eq!(count(root.path(), "session/prompt"), 1);
}

#[tokio::test]
async fn a_dead_native_epoch_cannot_be_restarted_by_prompting_its_old_handle() {
    let (root, driver) = fixture("normal");
    let old = driver.create_session(request(root.path())).await.unwrap();
    let (native, exited) = original(&driver).await;
    native.request_close();
    wait_for(|| exited.load(Ordering::Acquire)).await;
    assert!(
        driver
            .current_session_handle(&old.native_session_id)
            .await
            .is_none()
    );
    assert!(
        driver
            .prompt(&old, prompt(root.path(), "must-not-respawn"))
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "initialize"), 1);
    assert_eq!(count(root.path(), "session/prompt"), 0);
    let current = driver
        .resume_session(&old.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    assert!(!current.same_scope(&old));
    let stream = driver
        .prompt(&current, prompt(root.path(), "restored"))
        .await
        .unwrap();
    finish(stream, false).await;
    assert!(driver.shutdown(old).await.is_err());
    driver.shutdown(current).await.unwrap();
    assert_eq!(count(root.path(), "initialize"), 2);
}

#[tokio::test]
async fn a_duplicate_new_id_never_replaces_or_closes_an_existing_resident() {
    let (root, driver) = fixture("duplicate-new");
    let original_handle = driver.create_session(request(root.path())).await.unwrap();
    let (_, first_exit) = original(&driver).await;
    assert!(driver.create_session(request(root.path())).await.is_err());
    assert!(
        driver
            .shared
            .sessions
            .lock()
            .unwrap()
            .get(&original_handle.native_session_id)
            .unwrap()
            .handle
            .same_scope(&original_handle)
    );
    assert!(!first_exit.load(Ordering::Acquire));
    assert_eq!(count(root.path(), "session/close"), 0);
    assert!(
        driver
            .prompt(&original_handle, prompt(root.path(), "uncertain"))
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "session/prompt"), 0);
    let current = driver
        .resume_session(
            &original_handle.native_session_id,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
    assert!(first_exit.load(Ordering::Acquire));
    assert!(!current.same_scope(&original_handle));
    assert!(driver.shutdown(original_handle).await.is_err());
    finish(
        driver
            .prompt(&current, prompt(root.path(), "restored"))
            .await
            .unwrap(),
        false,
    )
    .await;
    driver.shutdown(current).await.unwrap();
    assert_eq!(count(root.path(), "initialize"), 2);
    assert_eq!(count(root.path(), "session/load"), 1);
}
