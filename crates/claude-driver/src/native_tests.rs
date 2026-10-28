//! Private process fixtures only; installed CLI checks remain opt-in.
use super::*;
use std::time::Duration;

pub(super) fn fixture(mode: &str) -> (tempfile::TempDir, ClaudeDriver) {
    let root = tempfile::tempdir().unwrap();
    // A just-written executable may transiently remain open for writing in
    // another test's forked child (ETXTBSY on Linux). Keep the executable fixed
    // and put only per-test data in the private workspace.
    let executable =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-process.py");
    std::fs::write(root.path().join("fixture-mode"), mode).unwrap();
    let driver = ClaudeDriver::new(
        "claude-code",
        &InstallationId::new().to_string(),
        executable,
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    (root, driver)
}

pub(super) fn workspace(root: &std::path::Path) -> SessionRequest {
    SessionRequest {
        working_directory: root.to_string_lossy().into_owned(),
        additional_directories: vec![],
        permission_mode: None,
        history_mcp: None,
    }
}

pub(super) async fn wait_for(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

pub(super) async fn ids(root: &std::path::Path) -> Vec<i32> {
    wait_for(|| root.join("pids.json").exists()).await;
    serde_json::from_slice(&std::fs::read(root.join("pids.json")).unwrap()).unwrap()
}

pub(super) fn present(pid: i32) -> bool {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
    match kill(Pid::from_raw(pid), None) {
        Err(Errno::ESRCH) => false,
        Ok(()) | Err(Errno::EPERM) => true,
        result => panic!("private process presence could not be verified: {result:?}"),
    }
}

pub(super) fn requests(root: &std::path::Path, request: &str) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|r| r.pointer("/request/subtype").and_then(Value::as_str) == Some(request))
        .count()
}

#[tokio::test]
async fn owner_drop_retires_a_blocked_writer_and_descendants_without_touching_a_peer() {
    let (root, driver) = fixture("writer");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let process = driver.process_for(&session).await.unwrap();
    let owned = ids(root.path()).await;
    let (peer_root, peer) = fixture("normal");
    let peer_handle = peer
        .create_session(workspace(peer_root.path()))
        .await
        .unwrap();
    let peer_process = peer.process_for(&peer_handle).await.unwrap();
    let peer_ids = ids(peer_root.path()).await;
    let writer_process = process.clone();
    let writer = tokio::spawn(async move {
        writer_process
            .send_line(&json!({"message":"x".repeat(2 * 1024 * 1024)}))
            .await
    });
    wait_for(|| process.stdin.try_lock().is_err()).await;
    assert!(!writer.is_finished());
    drop(driver);
    tokio::time::timeout(Duration::from_secs(8), process.retired())
        .await
        .unwrap();
    assert!(writer.await.unwrap().is_err());
    assert!(owned.iter().all(|pid| !present(*pid)));
    assert!(peer_ids.iter().all(|pid| present(*pid)));
    assert!(!peer_process.native.stopping());
    peer.shutdown(peer_handle).await.unwrap();
}

#[tokio::test]
async fn cancelling_a_partial_write_fences_and_retires_its_original_scope() {
    let (root, driver) = fixture("writer");
    let session = driver.create_session(workspace(root.path())).await.unwrap();
    let process = driver.process_for(&session).await.unwrap();
    let owned = ids(root.path()).await;
    let writer_process = process.clone();
    let writer = tokio::spawn(async move {
        writer_process
            .send_line(&json!({"message":"x".repeat(2 * 1024 * 1024)}))
            .await
    });
    wait_for(|| process.stdin.try_lock().is_err()).await;
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    assert!(process.closing.load(Ordering::Acquire));
    tokio::time::timeout(Duration::from_secs(8), process.retired())
        .await
        .unwrap();
    assert!(owned.iter().all(|pid| !present(*pid)));
    driver.shutdown(session).await.unwrap();
}

#[tokio::test]
async fn public_shutdown_and_interrupt_are_not_blocked_by_the_prompt_they_must_stop() {
    for action in ["shutdown", "interrupt"] {
        let (root, driver) = fixture("writer");
        let driver = Arc::new(driver);
        let session = driver.create_session(workspace(root.path())).await.unwrap();
        let process = driver.process_for(&session).await.unwrap();
        let owned = ids(root.path()).await;
        let prompt_driver = driver.clone();
        let prompt_session = session.clone();
        let request = HubPromptRequest {
            working_directory: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            attachments: vec![],
            context: vec![],
            permission_mode: None,
            prompt: "x".repeat(2 * 1024 * 1024),
        };
        let sending =
            tokio::spawn(async move { prompt_driver.prompt(&prompt_session, request).await });
        wait_for(|| process.stdin.try_lock().is_err()).await;
        assert!(process.admission.try_lock().is_err());
        assert!(!sending.is_finished());
        if action == "interrupt" {
            tokio::time::timeout(Duration::from_secs(2), driver.interrupt(&session))
                .await
                .unwrap()
                .unwrap();
        } else {
            tokio::time::timeout(Duration::from_secs(4), driver.shutdown(session.clone()))
                .await
                .unwrap()
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(6), process.retired())
            .await
            .unwrap();
        assert!(sending.await.unwrap().is_err());
        assert!(owned.iter().all(|pid| !present(*pid)), "{action}");
        driver.shutdown(session).await.unwrap();
    }
}

#[tokio::test]
async fn abandoning_a_native_configuration_retires_it_without_replaying_the_control() {
    let (root, driver) = fixture("configuration");
    let original = driver.create_session(workspace(root.path())).await.unwrap();
    let process = driver.process_for(&original).await.unwrap();
    let owned = ids(root.path()).await;
    let mut configuring = Box::pin(driver.reconfigure_session(
        &original.native_session_id,
        workspace(root.path()),
        Some("sonnet"),
        None,
    ));
    tokio::select! {
        result = &mut configuring => panic!("configuration should still be awaiting its native reply: {result:?}"),
        () = wait_for(|| requests(root.path(), "get_settings") == 1) => {},
    }
    assert!(!process.native.stopping());
    drop(configuring);
    assert!(process.closing.load(Ordering::Acquire));
    tokio::time::timeout(Duration::from_secs(8), process.retired())
        .await
        .unwrap();
    assert!(owned.iter().all(|pid| !present(*pid)));
    assert_eq!(requests(root.path(), "get_settings"), 1);
    driver.shutdown(process.handle()).await.unwrap();
}

#[tokio::test]
async fn eof_and_exited_leader_do_not_leave_an_inherited_pipe_descendant_alive() {
    for mode in ["eof", "leader"] {
        let (root, driver) = fixture(mode);
        let session = driver.create_session(workspace(root.path())).await.unwrap();
        let process = driver.process_for(&session).await.unwrap();
        let owned = ids(root.path()).await;
        let observed = tokio::time::timeout(Duration::from_secs(8), process.retired()).await;
        if observed.is_err() {
            process.request_close();
            tokio::time::timeout(Duration::from_secs(8), process.retired())
                .await
                .unwrap();
        }
        assert!(
            observed.is_ok(),
            "{mode} did not retire after actual EOF/leader exit"
        );
        assert!(owned.iter().all(|pid| !present(*pid)), "{mode}");
        driver.shutdown(session).await.unwrap();
    }
}

#[tokio::test]
async fn cancelling_initialization_before_install_still_retires_the_owned_process_tree() {
    let (root, driver) = fixture("history");
    let child = spawn(
        &ClaudeLaunch {
            executable: driver.executable.clone(),
        },
        "private-initialization",
        false,
        Some(root.path().to_str().unwrap()),
    )
    .unwrap();
    let mut installing = Box::pin(driver.install_session(
        "private-initialization",
        child,
        root.path().into(),
        None,
        true,
    ));
    tokio::select! {
        result = &mut installing => panic!("initialization should await its native response: {}", result.is_ok()),
        () = wait_for(|| requests(root.path(), "initialize") == 1) => {},
    }
    let owned = ids(root.path()).await;
    drop(installing);
    assert!(driver.processes.lock().await.is_empty());
    wait_for(|| owned.iter().all(|pid| !present(*pid))).await;
}

#[tokio::test]
async fn original_shutdown_waits_for_local_cleanup_and_cannot_remove_a_replacement() {
    let (root, driver) = fixture("normal");
    let driver = Arc::new(driver);
    let original = driver.create_session(workspace(root.path())).await.unwrap();
    let process = driver.process_for(&original).await.unwrap();
    let _owned = ids(root.path()).await;
    let blocker = process.stdin.lock().await;
    let mut proof = driver.shutdown_proof(&original).await.unwrap();
    let closing_driver = driver.clone();
    let closing_handle = original.clone();
    let closing = tokio::spawn(async move { closing_driver.shutdown(closing_handle).await });
    tokio::time::timeout(Duration::from_secs(6), process.native.confirmed())
        .await
        .unwrap();
    assert!(!process.cleanup_done.load(Ordering::Acquire));
    assert!(!closing.is_finished());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut proof)
            .await
            .is_err()
    );
    let (peer_root, peer) = fixture("normal");
    let replacement_handle = peer
        .resume_selected_session(
            &original.native_session_id,
            workspace(peer_root.path()),
            None,
            None,
        )
        .await
        .unwrap();
    let replacement = peer.process_for(&replacement_handle).await.unwrap();
    let peer_ids = ids(peer_root.path()).await;
    driver
        .processes
        .lock()
        .await
        .insert(original.native_session_id.clone(), replacement.clone());
    drop(blocker);
    closing.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2), proof)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &driver.processes.lock().await[&original.native_session_id],
        &replacement
    ));
    assert!(driver.shutdown(original).await.is_err());
    assert!(peer_ids.iter().all(|pid| present(*pid)));
    assert!(!replacement.native.stopping());
    peer.shutdown(replacement_handle).await.unwrap();
}
#[tokio::test]
#[ignore = "requires the local Claude CLI; control requests only, no model quota"]
async fn native_idle_shutdown_is_confirmed_without_model_requests() {
    use driver_api::{AgentDriver, SessionRequest, SessionResumeCapability};
    let executable =
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/bin/claude");
    let driver = crate::ClaudeDriver::new(
        "claude-code",
        &agent_domain::InstallationId::new().to_string(),
        executable,
        None,
        SessionResumeCapability::Supported,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        let session = driver
            .create_selected_session(
                SessionRequest {
                    working_directory: directory.path().to_string_lossy().into_owned(),
                    additional_directories: vec![],
                    history_mcp: None,
                    permission_mode: None,
                },
                Some("sonnet"),
                None,
            )
            .await
            .unwrap();
        driver.shutdown(session).await.unwrap();
    }
}
