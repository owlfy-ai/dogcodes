//! Private CLI fixtures: no installed Agent, credentials, or model requests.
use super::cleanup_tests::{alive, pid, request, script};
use super::*;
use std::time::Duration;

fn prompt(root: &std::path::Path) -> HubPromptRequest {
    HubPromptRequest {
        prompt: "private fixture".into(),
        working_directory: root.to_string_lossy().into_owned(),
        attachments: vec![],
        additional_directories: vec![],
        context: vec![],
        permission_mode: Some("desktop-approvals".into()),
    }
}
async fn turn(
    driver: &AntigravityDriver,
    handle: &SessionHandle,
    root: &std::path::Path,
) -> Vec<DriverEvent> {
    tokio::time::timeout(Duration::from_secs(5), async {
        driver
            .prompt(handle, prompt(root))
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn hot_handoff_rekeys_native_alias_and_rejects_all_old_authority() {
    let root = tempfile::tempdir().unwrap();
    let driver = tests::test_driver(tests::write_fake_agy(root.path()));
    let old = driver.create_session(request(root.path())).await.unwrap();
    let events = turn(&driver, &old, root.path()).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnCompleted { .. }))
    );
    let process = driver.process_for(&old).await.unwrap();
    let mut proof = driver.shutdown_proof(&old).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut proof)
            .await
            .is_err()
    );
    assert!(!*process.closing.borrow()); // Merely capturing/observing proof is passive.
    let current = driver
        .resume_workspace_session("conv-fixture", request(root.path()), None)
        .await
        .unwrap();
    assert_eq!(current.native_session_id, "conv-fixture");
    assert!(!old.same_scope(&current));
    assert!(driver.prompt(&old, prompt(root.path())).await.is_err());
    assert!(driver.interrupt(&old).await.is_err());
    assert!(driver.shutdown(old.clone()).await.is_err());
    assert!(
        driver
            .respond_to_interaction(&old, "any", "allow_once")
            .await
            .is_err()
    );
    assert!(
        driver
            .respond_to_question(
                &old,
                "any",
                agent_domain::QuestionAnswer {
                    cancelled: true,
                    values: json!({})
                }
            )
            .await
            .is_err()
    );
    assert!(
        driver
            .shutdown(SessionHandle::new("conv-fixture".into()))
            .await
            .is_err()
    );
    // A previously captured idle retirement candidate cannot close the new owner.
    assert!(process.request_close(&old, true, false).is_err());
    assert!(!*process.closing.borrow());
    assert!(
        driver
            .current_session_handle("conv-fixture")
            .await
            .unwrap()
            .same_scope(&current)
    );
    let events = turn(&driver, &current, root.path()).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnCompleted { .. }))
    );
    driver.shutdown(current).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), &mut proof)
        .await
        .unwrap();
    // Positive retired receipts are also scope-specific.
    assert!(driver.shutdown(old).await.is_err());
}

#[tokio::test]
async fn a_single_native_init_still_starts_every_warm_turn() {
    let root = tempfile::tempdir().unwrap();
    let exe = tests::write_fake_agy(root.path());
    let source = std::fs::read_to_string(&exe).unwrap();
    let mut output = String::new();
    for line in source.lines() {
        if line.contains("conversation_id") && line.contains("init") {
            output.push_str("      if [ ! -e \"${0}.init\" ]; then\n");
            output.push_str(line);
            output.push_str("\n      touch \"${0}.init\"; fi\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    std::fs::write(&exe, output).unwrap();
    let driver = tests::test_driver(exe);
    let mut handle = driver.create_session(request(root.path())).await.unwrap();
    for _ in 0..3 {
        let events = turn(&driver, &handle, root.path()).await;
        let started = events
            .iter()
            .position(|e| matches!(e, DriverEvent::TurnStarted { .. }))
            .unwrap();
        let done = events
            .iter()
            .position(|e| matches!(e, DriverEvent::TurnCompleted { .. }))
            .unwrap();
        assert!(started < done);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, DriverEvent::TurnStarted { .. }))
                .count(),
            1
        );
        handle = driver
            .resume_workspace_session("conv-fixture", request(root.path()), None)
            .await
            .unwrap();
    }
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn busy_reconfiguration_does_not_close_the_current_scope() {
    let root = tempfile::tempdir().unwrap();
    let driver = tests::test_driver(script(
        root.path(),
        "trap '' TERM; while :; do sleep 60; done",
    ));
    let handle = driver.create_session(request(root.path())).await.unwrap();
    let stream = driver.prompt(&handle, prompt(root.path())).await.unwrap();
    let process = driver.process_for(&handle).await.unwrap();
    assert!(matches!(
        driver
            .resume_workspace_session(&handle.native_session_id, request(root.path()), None)
            .await,
        Err(DriverError::AutonomousTurnBusy)
    ));
    assert!(!*process.closing.borrow());
    assert!(process.handle().same_scope(&handle));
    assert!(driver.spawn.try_lock().is_err());
    driver.shutdown(handle).await.unwrap();
    drop(stream);
}

async fn until_file(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn blocked_verification(root: &std::path::Path) -> PathBuf {
    let exe = script(root, "true");
    let source = std::fs::read_to_string(&exe).unwrap().replace(
        "python3 - \"$@\" <<'PYHOOK'",
        "printf '%s\\n' \"$$\" > \"${0}.verify-pid\"\nwhile [ ! -e \"${0}.release\" ]; do sleep 0.01; done\npython3 - \"$@\" <<'PYHOOK'",
    );
    std::fs::write(&exe, source).unwrap();
    exe
}

#[tokio::test]
async fn cancelled_hook_probe_retains_auth_exclusion_until_original_exit() {
    let root = tempfile::tempdir().unwrap();
    let exe = blocked_verification(root.path());
    let driver = Arc::new(tests::test_driver(exe.clone()));
    let creating = tokio::spawn({
        let driver = driver.clone();
        let req = request(root.path());
        async move { driver.create_session(req).await }
    });
    let marker = exe.with_extension("verify-pid");
    until_file(&marker).await;
    let original: u32 = std::fs::read_to_string(marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    assert!(alive(original));
    assert!(driver.spawn.try_lock().is_err());
    std::fs::write(exe.with_extension("release"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if driver.spawn.try_lock().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!alive(original));
    assert!(driver.processes.lock().await.is_empty());
}

#[tokio::test]
async fn abandoned_candidate_is_supervised_before_its_map_installation() {
    let root = tempfile::tempdir().unwrap();
    let exe = blocked_verification(root.path());
    let driver = Arc::new(tests::test_driver(exe.clone()));
    let creating = tokio::spawn({
        let driver = driver.clone();
        let req = request(root.path());
        async move { driver.create_session(req).await }
    });
    until_file(&exe.with_extension("verify-pid")).await;
    let map = driver.processes.lock().await;
    std::fs::write(exe.with_extension("release"), "release").unwrap();
    let original = pid(&exe).await;
    creating.abort();
    assert!(creating.await.unwrap_err().is_cancelled());
    // Do not release the map: the original scope must clean up independently.
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            if driver.spawn.try_lock().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!alive(original));
    assert!(map.is_empty());
    drop(map);
}

#[tokio::test]
async fn a_foreign_native_identity_fails_the_original_turn() {
    let root = tempfile::tempdir().unwrap();
    let exe = tests::write_fake_agy(root.path());
    let source = std::fs::read_to_string(&exe).unwrap().replace(
        "\"result\":{\"conversation_id\":\"conv-fixture\"",
        "\"result\":{\"conversation_id\":\"foreign-conversation\"",
    );
    std::fs::write(&exe, source).unwrap();
    let driver = tests::test_driver(exe);
    let handle = driver.create_session(request(root.path())).await.unwrap();
    let events = turn(&driver, &handle, root.path()).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnFailed { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnCompleted { .. }))
    );
    let p = driver.process_for(&handle).await.unwrap();
    assert_eq!(p.native_id.lock().unwrap().as_deref(), Some("conv-fixture"));
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn cancelled_model_query_retains_its_original_auth_and_process_scope() {
    let root = tempfile::tempdir().unwrap();
    let exe = tests::write_fake_agy(root.path());
    let source = std::fs::read_to_string(&exe).unwrap().replace(
        "case \"$*\" in",
        r#"if [ "$1" = models ]; then
printf '%s\n' "$$" > "${0}.models-pid"
while [ ! -e "${0}.release" ]; do sleep 0.01; done
printf 'private-model\tPrivate Model\n'
exit 0
fi
case "$*" in"#,
    );
    std::fs::write(&exe, source).unwrap();
    let driver = Arc::new(tests::test_driver(exe.clone()));
    let read = tokio::spawn({
        let driver = driver.clone();
        async move { driver.available_models().await }
    });
    until_file(&exe.with_extension("models-pid")).await;
    let original: u32 = std::fs::read_to_string(exe.with_extension("models-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());
    assert!(alive(original));
    assert!(driver.spawn.try_lock().is_err());
    std::fs::write(exe.with_extension("release"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if driver.spawn.try_lock().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!alive(original));
    assert_eq!(
        driver.available_models().await.unwrap(),
        vec!["private-model"]
    );
}

#[tokio::test]
async fn proof_of_an_old_process_does_not_follow_a_cold_replacement_with_the_same_id() {
    let root = tempfile::tempdir().unwrap();
    let mut driver = tests::test_driver(tests::write_fake_agy(root.path()));
    driver.resume_capability = SessionResumeCapability::Supported;
    let initial = driver.create_session(request(root.path())).await.unwrap();
    turn(&driver, &initial, root.path()).await;
    let old = driver
        .resume_workspace_session("conv-fixture", request(root.path()), None)
        .await
        .unwrap();
    let mut proof = driver.shutdown_proof(&old).await.unwrap();
    driver.shutdown(old.clone()).await.unwrap();
    let replacement = driver
        .resume_workspace_session("conv-fixture", request(root.path()), None)
        .await
        .unwrap();
    assert!(!old.same_scope(&replacement));
    tokio::time::timeout(Duration::from_secs(1), &mut proof)
        .await
        .unwrap();
    assert!(driver.shutdown(old).await.is_err());
    assert!(
        !*driver
            .process_for(&replacement)
            .await
            .unwrap()
            .closing
            .borrow()
    );
    let events = turn(&driver, &replacement, root.path()).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DriverEvent::TurnCompleted { .. }))
    );
    driver.shutdown(replacement).await.unwrap();
}

#[tokio::test]
async fn cancelled_quota_query_does_not_release_auth_while_the_helper_is_alive() {
    use driver_api::QuotaProvider;
    let root = tempfile::tempdir().unwrap();
    let exe = tests::write_fake_agy(root.path());
    let source = std::fs::read_to_string(&exe).unwrap().replace(
        "case \"$*\" in",
        r#"case "$*" in *--print=/usage*)
printf '%s\n' "$$" > "${0}.quota-pid"
while [ ! -e "${0}.release" ]; do sleep 0.01; done
exit 0;; esac
case "$*" in"#,
    );
    std::fs::write(&exe, source).unwrap();
    let driver = Arc::new(tests::test_driver(exe.clone()));
    let read = tokio::spawn({
        let driver = driver.clone();
        async move { driver.read_quota_with_refresh(None, true).await }
    });
    until_file(&exe.with_extension("quota-pid")).await;
    let original: u32 = std::fs::read_to_string(exe.with_extension("quota-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());
    assert!(alive(original));
    assert!(driver.spawn.try_lock().is_err());
    std::fs::write(exe.with_extension("release"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if driver.spawn.try_lock().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!alive(original));
}
