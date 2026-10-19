use super::*;

fn intent(root: &Path, command: Option<&str>) -> LaunchIntent {
    LaunchIntent {
        project_launch: None,
        operation_id: uuid::Uuid::new_v4().to_string(),
        project_id: "prj_terminal".into(),
        directory_index: 0,
        task_id: None,
        command_id: None,
        label: "Terminal test".into(),
        cwd: root.into(),
        command: command.map(String::from),
        cols: 80,
        rows: 24,
    }
}
#[cfg(unix)]
async fn read_until(host: &TerminalHost, id: &str, needle: &str) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut bytes = vec![];
        let mut cursor = 0;
        loop {
            let batch = host.read(id, cursor).await.unwrap();
            cursor = batch.next;
            bytes.extend(batch.bytes);
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return bytes;
            }
            assert!(
                batch.info.status.active(),
                "process exited before marker: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
    })
    .await
    .expect("terminal output timeout")
}
#[cfg(unix)]
async fn finished(host: &TerminalHost, id: &str) -> TerminalInfo {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut cursor = 0;
        loop {
            let batch = host.read(id, cursor).await.unwrap();
            cursor = batch.next;
            if !batch.info.status.active() {
                return batch.info;
            }
        }
    })
    .await
    .expect("terminal did not exit")
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_runs_without_a_view_and_replays_output_before_and_after_attach() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::persistent(root.path().join("operations"));
    let request = intent(
        root.path(),
        Some("printf 'BEFORE_ATTACH\\n'; sleep 0.1; printf 'AFTER_ATTACH\\n'; exit 7"),
    );
    let id = host.start_resolved(request, None).await.unwrap().id;
    // No subscriber owns the reader or the process; all output still reaches the ring.
    let info = finished(&host, &id).await;
    assert_eq!(info.status, TerminalStatus::Exited);
    assert_eq!(info.exit_code, Some(7));
    let replay = host.read(&id, 0).await.unwrap();
    let text = String::from_utf8_lossy(&replay.bytes);
    assert!(text.contains("BEFORE_ATTACH"));
    assert!(text.contains("AFTER_ATTACH"));
    assert!(!replay.gap);
    assert_eq!(host.list("prj_terminal").unwrap().len(), 1);
    drop(host);
    let restored = TerminalHost::persistent(root.path().join("operations"));
    assert_eq!(restored.get(&id).unwrap().exit_code, Some(7));
    let unavailable = restored.read(&id, 0).await.unwrap();
    assert!(unavailable.gap);
    assert!(unavailable.bytes.is_empty());
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_lost_reply_and_concurrent_retry_cannot_spawn_twice() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::persistent(root.path().join("operations"));
    let request = intent(
        root.path(),
        Some("printf x >> count; printf 'RUNNING\\n'; sleep 60"),
    );
    let (a, b) = tokio::join!(
        host.start_resolved(request.clone(), None),
        host.start_resolved(request.clone(), None)
    );
    let a = a.unwrap();
    assert_eq!(a.id, b.unwrap().id);
    read_until(&host, &a.id, "RUNNING").await;
    assert_eq!(std::fs::read(root.path().join("count")).unwrap(), b"x");
    let mut changed = request.clone();
    changed.command = Some("printf y >> count".into());
    assert!(host.start_resolved(changed, None).await.is_err());
    assert!(host.has_active());
    host.stop(&a.id).await.unwrap();
    finished(&host, &a.id).await;
    assert!(!host.has_active());
    // An accepted operation stays accepted even after it ends or the host reopens.
    drop(host);
    let reopened = TerminalHost::persistent(root.path().join("operations"));
    assert_eq!(
        reopened.start_resolved(request, None).await.unwrap().status,
        TerminalStatus::Exited
    );
    assert_eq!(std::fs::read(root.path().join("count")).unwrap(), b"x");
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_reader_disconnection_does_not_stop_process_or_repeat_input() {
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(TerminalHost::default());
    let request = intent(
        root.path(),
        Some("printf 'READY\\n'; read text; printf 'GOT:%s\\n' \"$text\"; sleep 60"),
    );
    let info = host.start_resolved(request, None).await.unwrap();
    let bytes = read_until(&host, &info.id, "READY").await;
    let reader_host = host.clone();
    let reader_id = info.id.clone();
    let reader =
        tokio::spawn(async move { reader_host.read(&reader_id, bytes.len() as u64).await });
    reader.abort();
    let _ = reader.await;
    assert_eq!(host.get(&info.id).unwrap().status, TerminalStatus::Running);
    host.write(&info.id, b"one\n".to_vec()).await.unwrap();
    let text = read_until(&host, &info.id, "GOT:one").await;
    assert_eq!(String::from_utf8_lossy(&text).matches("GOT:one").count(), 1);
    host.stop(&info.id).await.unwrap();
    finished(&host, &info.id).await;
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_stop_is_available_while_input_is_blocked() {
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(TerminalHost::default());
    let info = host
        .start_resolved(
            intent(
                root.path(),
                Some("stty -echo -icanon; printf 'READY\\n'; sleep 60"),
            ),
            None,
        )
        .await
        .unwrap();
    read_until(&host, &info.id, "READY").await;
    let writing = host.clone();
    let id = info.id.clone();
    let writer = tokio::spawn(async move { writing.write(&id, vec![b'x'; READ_LIMIT]).await });
    let session = host.session(&info.id).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if session.input.try_lock().is_err() {
                break;
            }
            assert!(!writer.is_finished(), "fixture must fill the input buffer");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), host.stop(&info.id))
        .await
        .unwrap()
        .unwrap();
    finished(&host, &info.id).await;
    // Stop cancels backpressured input even on Linux, where killing the slave
    // alone does not reliably release a blocking PTY write. Never replay it.
    let _ = tokio::time::timeout(Duration::from_secs(3), writer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_child_exit_releases_backpressured_input() {
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(TerminalHost::default());
    let info = host.start_resolved(intent(root.path(), Some(
        "stty -echo -icanon; printf 'READY\\n'; while [ ! -e exit-request ]; do sleep 0.01; done"
    )), None).await.unwrap();
    read_until(&host, &info.id, "READY").await;
    let writing = host.clone();
    let id = info.id.clone();
    let writer = tokio::spawn(async move { writing.write(&id, vec![b'x'; READ_LIMIT]).await });
    let session = host.session(&info.id).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while session.input.try_lock().is_ok() {
            assert!(!writer.is_finished(), "fixture must fill the input buffer");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(root.path().join("exit-request"), b"exit").unwrap();
    assert_eq!(
        finished(&host, &info.id).await.status,
        TerminalStatus::Exited
    );
    let _ = tokio::time::timeout(Duration::from_secs(3), writer)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn terminal_host_restart_marks_unresolved_intent_unknown_and_never_kills_an_old_pid() {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("operations");
    let original = TerminalHost::persistent(journal.clone());
    let mut request = intent(root.path(), Some("printf 'ALIVE\\n'; sleep 60"));
    request.task_id = Some("task".into());
    request.command_id = Some("dev".into());
    request.project_launch = Some(ProjectLaunchRequest {
        task_id: "task".into(),
        generation: "old-generation".into(),
        directory_index: 0,
        command_id: "dev".into(),
        cols: 80,
        rows: 24,
        operation_id: request.operation_id.clone(),
    });
    let info = original
        .start_resolved(request.clone(), None)
        .await
        .unwrap();
    read_until(&original, &info.id, "ALIVE").await;
    // Loading the journal models another epoch seeing an unresolved record.
    // The test keeps the original host alive to prove no reused/old PID is touched.
    let restarted = TerminalHost::persistent(journal);
    assert_eq!(
        restarted.get(&info.id).unwrap().status,
        TerminalStatus::Unknown
    );
    assert_eq!(
        restarted
            .existing_project_launch(request.project_launch.as_ref().unwrap())
            .unwrap()
            .unwrap()
            .status,
        TerminalStatus::Unknown
    );
    assert_eq!(
        restarted
            .start_resolved(request, None)
            .await
            .unwrap()
            .status,
        TerminalStatus::Unknown
    );
    assert!(restarted.stop(&info.id).await.is_err());
    assert!(restarted.write(&info.id, vec![3]).await.is_err());
    assert_eq!(
        original.get(&info.id).unwrap().status,
        TerminalStatus::Running
    );
    original.stop(&info.id).await.unwrap();
    finished(&original, &info.id).await;
}

fn buffered_session(root: &Path, status: TerminalStatus, created_at: i64) -> Arc<Session> {
    let request = intent(root, None);
    Arc::new(Session {
        intent: request.clone(),
        output: Mutex::new(Output {
            first: 0,
            bytes: VecDeque::new(),
            info: TerminalInfo {
                id: request.operation_id,
                epoch: "epoch".into(),
                project_id: request.project_id,
                directory_index: 0,
                task_id: None,
                command_id: None,
                label: request.label,
                cwd: request.cwd,
                command: None,
                status,
                exit_code: None,
                error: None,
                output_end: 0,
                created_at,
                cols: 80,
                rows: 24,
            },
        }),
        process: Mutex::new(None),
        input: Mutex::new(None),
        changed: tokio::sync::Notify::new(),
        state_changes: Arc::new(crate::state_changes::Changes::new()),
    })
}

#[test]
fn terminal_host_bounds_output_and_marks_missing_ranges() {
    let root = tempfile::tempdir().unwrap();
    let session = buffered_session(root.path(), TerminalStatus::Running, 0);
    session.append(&vec![b'a'; OUTPUT_LIMIT + 123]);
    let batch = session.batch(0).unwrap();
    assert!(batch.gap);
    assert_eq!(batch.from, 123);
    assert_eq!(batch.bytes.len(), READ_LIMIT);
    let next = session.batch(batch.next).unwrap();
    assert!(!next.gap);
    assert_eq!(next.from, batch.next);
    assert_eq!(session.output.lock().unwrap().bytes.len(), OUTPUT_LIMIT);
}

#[test]
fn completed_terminal_eviction_releases_allocations_and_preserves_active_output_and_receipts() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::default();
    let mut completed = Vec::new();
    let mut active = Vec::new();
    {
        let mut sessions = host.sessions.lock().unwrap();
        for index in 0..COMPLETED_OUTPUT_LIMIT + 3 {
            let session = buffered_session(
                root.path(),
                TerminalStatus::Exited,
                i64::try_from(index).unwrap(),
            );
            session.append(&vec![b'x'; 4096]);
            sessions.insert(session.intent.operation_id.clone(), session.clone());
            completed.push(session);
        }
        for status in [
            TerminalStatus::Starting,
            TerminalStatus::Running,
            TerminalStatus::Stopping,
        ] {
            let session = buffered_session(root.path(), status, -1);
            session.append(b"live output");
            sessions.insert(session.intent.operation_id.clone(), session.clone());
            active.push(session);
        }
        release_completed_output(&sessions);
    }

    for (index, session) in completed.iter().enumerate() {
        let output = session.output.lock().unwrap();
        if index < 3 {
            assert_eq!(
                output.bytes.capacity(),
                0,
                "eviction must free the allocation"
            );
            assert_eq!(output.first, output.info.output_end);
        } else {
            assert_eq!(output.bytes.len(), 4096);
        }
        drop(output);
        let replay = session.batch(0).unwrap();
        assert_eq!(replay.gap, index < 3);
        assert_eq!(replay.bytes.len(), if index < 3 { 0 } else { 4096 });
        assert_eq!(
            host.existing(&session.intent).unwrap().unwrap().id,
            session.intent.operation_id
        );
    }
    for session in active {
        let replay = session.batch(0).unwrap();
        assert!(!replay.gap);
        assert_eq!(replay.bytes, b"live output");
    }
    assert_eq!(
        host.list("prj_terminal").unwrap().len(),
        COMPLETED_OUTPUT_LIMIT + 6
    );
}

#[tokio::test]
async fn terminal_host_invalid_identity_size_or_corrupt_journal_never_starts() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::default();
    let mut request = intent(root.path(), None);
    request.operation_id = "../outside".into();
    assert!(host.start_resolved(request, None).await.is_err());
    let mut request = intent(root.path(), None);
    request.cols = 0;
    assert!(host.start_resolved(request, None).await.is_err());
    let journal = root.path().join("operations");
    std::fs::create_dir(&journal).unwrap();
    std::fs::write(journal.join("bad.json"), b"corrupt").unwrap();
    let host = TerminalHost::persistent(journal);
    assert!(
        host.start_resolved(intent(root.path(), None), None)
            .await
            .is_err()
    );
    assert!(host.list("prj_terminal").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_host_preserves_unicode_cwd_and_resize() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("web 服务");
    std::fs::create_dir(&path).unwrap();
    let host = TerminalHost::default();
    let info = host
        .start_resolved(
            intent(
                &path,
                Some("pwd; printf 'READY\\n'; read line; stty size; printf 'DONE\\n'"),
            ),
            None,
        )
        .await
        .unwrap();
    let first = read_until(&host, &info.id, "READY").await;
    assert!(
        String::from_utf8_lossy(&first).contains(path.canonicalize().unwrap().to_str().unwrap())
    );
    assert_eq!(
        (info.cols, info.rows),
        (intent(&path, None).cols, intent(&path, None).rows)
    );
    host.resize(&info.id, 100, 30).await.unwrap();
    // Every attached client sees the size the PTY actually uses.
    let resized = host.get(&info.id).unwrap();
    assert_eq!((resized.cols, resized.rows), (100, 30));
    host.write(&info.id, b"continue\n".to_vec()).await.unwrap();
    let output = read_until(&host, &info.id, "DONE").await;
    assert!(String::from_utf8_lossy(&output).contains("30 100"));
    assert_eq!(finished(&host, &info.id).await.exit_code, Some(0));
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_host_explicit_stop_releases_child_processes() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::default();
    let info = host
        .start_resolved(
            intent(
                root.path(),
                Some("sleep 60 & echo $! > child-pid; printf 'READY\\n'; wait"),
            ),
            None,
        )
        .await
        .unwrap();
    read_until(&host, &info.id, "READY").await;
    let child = std::fs::read_to_string(root.path().join("child-pid")).unwrap();
    host.stop(&info.id).await.unwrap();
    finished(&host, &info.id).await;
    let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", child.trim()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "child still running: {state}"
    );
    assert_eq!(
        host.stop(&info.id).await.unwrap().status,
        TerminalStatus::Exited
    );
}

#[tokio::test]
async fn terminal_host_rejects_inconsistent_recipe_binding_before_recording_or_spawning() {
    let root = tempfile::tempdir().unwrap();
    let host = TerminalHost::persistent(root.path().join("operations"));
    let mut resolved = intent(root.path(), Some("echo never-run"));
    resolved.task_id = Some("task".into());
    resolved.command_id = Some("dev".into());
    resolved.project_launch = Some(ProjectLaunchRequest {
        operation_id: resolved.operation_id.clone(),
        task_id: "task".into(),
        command_id: "dev".into(),
        generation: "g1".into(),
        directory_index: 0,
        cols: 80,
        rows: 24,
    });
    resolved.project_launch.as_mut().unwrap().cols = 100;
    assert!(
        host.start_resolved(resolved.clone(), None)
            .await
            .unwrap_err()
            .contains("不一致")
    );
    assert!(host.list(&resolved.project_id).unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(root.path().join("operations"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_host_unbound_old_record_is_queryable_but_not_replayed_as_a_recipe() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("operations");
    let host = TerminalHost::persistent(directory.clone());
    let mut original = intent(root.path(), Some("printf x >> count"));
    original.task_id = Some("task".into());
    original.command_id = Some("dev".into());
    let info = host.start_resolved(original.clone(), None).await.unwrap();
    finished(&host, &info.id).await;
    drop(host);
    let file = directory.join(format!("{}.json", info.id));
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    stored["intent"]
        .as_object_mut()
        .unwrap()
        .remove("projectLaunch");
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    let reopened = TerminalHost::persistent(directory);
    assert_eq!(
        reopened.get(&info.id).unwrap().status,
        TerminalStatus::Exited
    );
    let request = ProjectLaunchRequest {
        operation_id: original.operation_id,
        task_id: "task".into(),
        generation: "g1".into(),
        command_id: "dev".into(),
        directory_index: 0,
        cols: 80,
        rows: 24,
    };
    assert!(
        reopened
            .existing_project_launch(&request)
            .unwrap_err()
            .contains("未绑定")
    );
    assert_eq!(std::fs::read(root.path().join("count")).unwrap(), b"x");
}
