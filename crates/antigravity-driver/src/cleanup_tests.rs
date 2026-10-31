//! No installed Agent/model calls: private shell runtimes and native hook fixture only.
use super::*;
use std::{path::Path, time::Duration};
pub(super) fn script(directory: &Path, body: &str) -> PathBuf {
    let path = tests::write_fake_agy(directory);
    let source = std::fs::read_to_string(&path).unwrap();
    let marker = "printf '%s\\n' \"$@\" > \"${0}.args\"";
    assert!(source.contains(marker));
    std::fs::write(
        &path,
        source.replace(
            marker,
            &format!("printf '%s\\n' \"$$\" > \"${{0}}.pid\"\n{body}\n{marker}"),
        ),
    )
    .unwrap();
    path
}
pub(super) fn request(path: &Path) -> SessionRequest {
    SessionRequest {
        working_directory: path.to_string_lossy().into_owned(),
        additional_directories: vec![],
        app_mcps: vec![],
        permission_mode: Some("desktop-approvals".into()),
    }
}
pub(super) async fn pid(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path.with_extension("pid"))
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
pub(super) fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

async fn wait_cleanup(process: &SessionProcess) {
    let mut cleanup = process.cleanup.clone();
    while cleanup.borrow_and_update().as_ref() != Some(&Ok(())) {
        cleanup.changed().await.unwrap();
    }
}

#[tokio::test]
async fn cancelling_a_partial_frame_fences_input_and_reports_failure() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(temp.path(), "trap '' TERM; while :; do sleep 60; done");
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let process = driver.processes.lock().await[&handle.native_session_id].clone();
    let (events, mut received) = mpsc::unbounded_channel();
    *process.updates.lock().unwrap() = Some(events);
    process.turn_active.store(true, Ordering::Release);
    let writer = tokio::spawn({
        let process = process.clone();
        async move {
            process
                .send_line(&json!({"blob":"x".repeat(2*1024*1024)}))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while process.stdin.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    let retired_by_cancellation = *process.closing.borrow();
    if retired_by_cancellation {
        tokio::time::timeout(Duration::from_secs(7), wait_cleanup(&process))
            .await
            .unwrap();
    } else {
        process.close().await.unwrap();
    }
    assert!(retired_by_cancellation);
    assert!(!alive(native));
    assert!(process.send_line(&json!({"event":"next"})).await.is_err());
    assert!(matches!(
        received.recv().await,
        Some(DriverEvent::TurnFailed { .. })
    ));
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn unterminated_oversized_output_is_bounded_before_newline_and_closes_the_scope() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(
        temp.path(),
        "exec python3 -c 'import sys,signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);sys.stdin.readline();sys.stdout.write(\"x\"*(2*1024*1024+1));sys.stdout.flush();time.sleep(60)'",
    );
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let process = driver.processes.lock().await[&handle.native_session_id].clone();
    let (events, mut received) = mpsc::unbounded_channel();
    *process.updates.lock().unwrap() = Some(events);
    process.turn_active.store(true, Ordering::Release);
    process.send_line(&json!({"event":"user"})).await.unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(7), wait_cleanup(&process)).await;
    if completed.is_err() {
        process.close().await.unwrap();
    }
    assert!(completed.is_ok());
    assert!(!alive(native));
    assert!(matches!(
        received.recv().await,
        Some(DriverEvent::TurnFailed { .. })
    ));
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn interrupt_waits_for_a_scope_that_ignores_stdin_eof_instead_of_reporting_early_success() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(temp.path(), "trap '' TERM; while :; do sleep 60; done");
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let process = driver.processes.lock().await[&handle.native_session_id].clone();
    let stream = driver
        .prompt(
            &handle,
            HubPromptRequest {
                attachments: vec![],
                permission_mode: Some("desktop-approvals".into()),
                additional_directories: vec![],
                prompt: "private stop fixture".into(),
                working_directory: temp.path().to_string_lossy().into_owned(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    driver.interrupt(&handle).await.unwrap();
    let confirmed = process.finished.load(Ordering::Acquire) && !alive(native);
    driver.shutdown(handle).await.unwrap();
    assert!(confirmed);
    let events = tokio::time::timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnFailed { .. })
    )));
}

#[tokio::test]
async fn eof_is_not_exit_and_completed_old_references_do_not_hold_the_spawn_slot() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(
        temp.path(),
        "exec 1>&-; exec 2>&-; trap '' TERM; while :; do sleep 60; done",
    );
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let old = driver
        .processes
        .lock()
        .await
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    assert!(alive(native));
    assert!(!old.finished.load(Ordering::Acquire));
    tokio::time::timeout(Duration::from_secs(7), driver.shutdown(handle))
        .await
        .unwrap()
        .unwrap();
    assert!(!alive(native));
    assert!(old.finished.load(Ordering::Acquire));
    assert!(driver.processes.lock().await.is_empty());
    let next = tempfile::tempdir().unwrap();
    let next_driver = tests::test_driver(tests::write_fake_agy(next.path()));
    let handle = tokio::time::timeout(
        Duration::from_secs(3),
        next_driver.create_session(request(next.path())),
    )
    .await
    .expect("old references must not own the exited process slot")
    .unwrap();
    next_driver.shutdown(handle).await.unwrap();
    drop(old);
}
#[tokio::test]
async fn cancelling_a_shutdown_waiter_keeps_the_original_actor_for_retry() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(temp.path(), "trap '' TERM; while :; do sleep 60; done");
    let driver = Arc::new(tests::test_driver(executable.clone()));
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let process = driver
        .processes
        .lock()
        .await
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    let closing = {
        let driver = driver.clone();
        let handle = handle.clone();
        tokio::spawn(async move { driver.shutdown(handle).await })
    };
    while !*process.closing.borrow() {
        tokio::task::yield_now().await;
    }
    closing.abort();
    let _ = closing.await;
    assert!(
        driver
            .processes
            .lock()
            .await
            .get(&handle.native_session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &process))
    );
    assert!(!process.finished.load(Ordering::Acquire));
    driver.shutdown(handle).await.unwrap();
    assert!(!alive(native));
    assert!(driver.processes.lock().await.is_empty());
}

#[tokio::test]
async fn idle_retirement_cannot_close_a_new_active_turn() {
    use futures::StreamExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = script(temp.path(), "trap '' TERM; while :; do sleep 60; done");
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let stream = driver
        .prompt(
            &handle,
            HubPromptRequest {
                attachments: vec![],
                permission_mode: Some("desktop-approvals".into()),
                additional_directories: vec![],
                prompt: "private fixture only".into(),
                working_directory: temp.path().to_string_lossy().into_owned(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        driver.retire_idle_sessions().await,
        Err(DriverError::CapabilityUnavailable(_))
    ));
    let process = driver
        .processes
        .lock()
        .await
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    assert!(process.turn_active.load(Ordering::Acquire));
    assert!(!*process.closing.borrow());
    assert!(alive(native));
    // Recheck the per-session idle gate as well: retirement's initial map
    // snapshot may have been taken before this turn started.
    assert!(matches!(
        process.close_if(true).await,
        Err(DriverError::CapabilityUnavailable(_))
    ));
    assert!(!*process.closing.borrow());
    driver.shutdown(handle).await.unwrap();
    let events = tokio::time::timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    assert!(!alive(native));
}

#[tokio::test]
async fn dropping_the_driver_still_waits_for_native_and_bridge_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(temp.path(), "trap '' TERM; while :; do sleep 60; done");
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let native = pid(&executable).await;
    let process = driver
        .processes
        .lock()
        .await
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    let mut cleanup = process.cleanup.clone();
    drop(driver);
    tokio::time::timeout(Duration::from_secs(7), async {
        while cleanup.borrow_and_update().as_ref() != Some(&Ok(())) {
            cleanup.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(process.finished.load(Ordering::Acquire));
    assert!(!alive(native));
    assert!(process.background.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_readiness_probe_keeps_auth_exclusion_until_its_scope_exits() {
    let temp = tempfile::tempdir().unwrap();
    let executable = script(
        temp.path(),
        "exec 1>&-; exec 2>&-; trap '' TERM; while :; do sleep 60; done",
    );
    let lock = SpawnLock::default();
    let probe = tokio::spawn({
        let (executable, lock) = (executable.clone(), lock.clone());
        async move { AntigravityDriver::probe_ready_with(&executable, &lock).await }
    });
    let native = pid(&executable).await;
    probe.abort();
    let _ = probe.await;
    assert!(alive(native));
    let next = tempfile::tempdir().unwrap();
    let driver = tests::test_driver_with(tests::write_fake_agy(next.path()), lock);
    let handle = tokio::time::timeout(
        Duration::from_secs(7),
        driver.create_session(request(next.path())),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        !alive(native),
        "another CLI cannot start before the original probe exits"
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn leader_exit_is_observed_even_when_a_descendant_keeps_stdout_open() {
    use futures::StreamExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = script(
        temp.path(),
        "trap '' TERM; IFS= read -r line; sleep 60 & printf '%s\\n' \"$!\" > \"${0}.descendant\"; exit 0",
    );
    let driver = tests::test_driver(executable.clone());
    let handle = driver.create_session(request(temp.path())).await.unwrap();
    let process = driver
        .processes
        .lock()
        .await
        .get(&handle.native_session_id)
        .cloned()
        .unwrap();
    let stream = driver
        .prompt(
            &handle,
            HubPromptRequest {
                attachments: vec![],
                permission_mode: Some("desktop-approvals".into()),
                additional_directories: vec![],
                prompt: "private exit fixture".into(),
                working_directory: temp.path().to_string_lossy().into_owned(),
                context: vec![],
            },
        )
        .await
        .unwrap();
    let mut cleanup = process.cleanup.clone();
    tokio::time::timeout(Duration::from_secs(7), async {
        while cleanup.borrow_and_update().as_ref() != Some(&Ok(())) {
            cleanup.changed().await.unwrap();
        }
    })
    .await
    .expect("native leader exit must not wait for an inherited stdout EOF");
    let descendant = std::fs::read_to_string(executable.with_extension("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!alive(descendant));
    assert!(process.finished.load(Ordering::Acquire));
    assert!(!process.turn_active.load(Ordering::Acquire));
    let events = tokio::time::timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(DriverEvent::TurnFailed { .. })))
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnInterrupted { .. })
    )));
    driver.shutdown(handle).await.unwrap();
}
