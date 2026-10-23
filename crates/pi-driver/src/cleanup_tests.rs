//! Only private Python RPC fixtures; no installed Agent, credentials or model use.
use super::tests::{FAKE, events, fixture_with, prompt};
use super::*;
use driver_api::DriverEvent;
use std::{path::Path, time::Duration};

fn stubborn(descendant: bool) -> String {
    let setup = if descendant {
        "descendant=subprocess.Popen([sys.executable,'-c','import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])\n"
    } else {
        "descendant=None\n"
    };
    let head = format!(
        "import json,sys,os,uuid,time,signal,subprocess\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\n{setup}with open('process.json','w') as f:json.dump([os.getpid(),descendant.pid if descendant else None],f)"
    );
    format!(
        "{}\nwhile True:time.sleep(60)\n",
        FAKE.replace("import json,sys,os,uuid", &head)
    )
}
async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(9), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn process_ids(dir: &Path) -> Vec<u32> {
    serde_json::from_slice::<Vec<Option<u32>>>(&std::fs::read(dir.join("process.json")).unwrap())
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}
fn present(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim() == pid.to_string()
}
async fn stopped(connection: &Connection) {
    let mut cleanup = connection.cleanup();
    tokio::time::timeout(Duration::from_secs(9), async {
        loop {
            if let Some(result) = cleanup.borrow_and_update().clone() {
                result.unwrap();
                break;
            }
            cleanup.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
fn count(dir: &Path, kind: &str) -> usize {
    std::fs::read_to_string(dir.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| row["type"] == kind)
        .count()
}
#[tokio::test]
async fn cancelled_shutdown_retains_the_original_process_until_the_tree_is_gone() {
    let (dir, driver) = fixture_with(&stubborn(true));
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let original = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let closing = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        async move { driver.shutdown(handle).await }
    });
    wait_for(|| original.state.lock().unwrap().disconnected).await;
    closing.abort();
    let _ = closing.await;
    assert!(Arc::ptr_eq(
        &original,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    assert!(ids.iter().all(|id| present(*id)));
    driver.shutdown(handle).await.unwrap();
    stopped(&original).await;
    assert!(ids.iter().all(|id| !present(*id)));
    assert!(driver.sessions.lock().await.is_empty());
}
#[tokio::test]
async fn eof_does_not_confirm_that_the_pi_process_stopped() {
    let script = stubborn(false).replace(
        "if r['message']=='disconnect':sys.exit(0)",
        "if r['message']=='disconnect':os.close(1);os.close(2);time.sleep(60)",
    );
    let (dir, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let ended = events(
        driver
            .prompt(&handle, prompt(dir.path(), "disconnect"))
            .await
            .unwrap(),
    )
    .await;
    assert!(ended.iter().any(Result::is_err));
    assert!(connection.cleanup().borrow().is_none());
    assert!(ids.iter().all(|id| present(*id)));
    driver.shutdown(handle).await.unwrap();
    assert!(ids.iter().all(|id| !present(*id)));
}
#[tokio::test]
async fn native_exit_is_observed_when_a_descendant_keeps_stdout_open() {
    let (dir, driver) = fixture_with(&stubborn(true));
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "disconnect"))
        .await
        .unwrap();
    stopped(&connection).await;
    let ended = events(stream).await;
    assert!(ended.iter().any(Result::is_err));
    assert!(!ended.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::TurnCompleted { .. } | DriverEvent::TurnInterrupted { .. })
    )));
    assert!(ids.iter().all(|id| !present(*id)));
    driver.shutdown(handle).await.unwrap();
}
#[tokio::test]
async fn driver_drop_closes_a_session_with_an_unacknowledged_prompt() {
    let (dir, driver) = fixture_with(&stubborn(true));
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "question"))
        .await
        .unwrap();
    wait_for(|| count(dir.path(), "prompt") == 1).await;
    drop(driver);
    stopped(&connection).await;
    assert!(ids.iter().all(|id| !present(*id)));
    {
        let state = connection.state.lock().unwrap();
        assert!(state.pending.is_empty());
        assert!(state.turn.is_none());
    }
    assert!(events(stream).await.iter().any(Result::is_err));
}
#[tokio::test]
async fn cancelled_probe_still_cleans_its_original_process_tree() {
    let script = stubborn(true).replace(
        "elif t=='get_available_models':reply(r,",
        "elif t=='get_available_models':time.sleep(60);reply(r,",
    );
    let (dir, driver) = fixture_with(&script);
    // probe uses the system temp directory; keep the script's own directory as
    // cwd so metadata remains isolated from other tests and the developer.
    let script=script.replace("import json,sys,os,uuid,time,signal,subprocess","import json,sys,os,uuid,time,signal,subprocess\nos.chdir(os.path.dirname(os.path.abspath(__file__)))");
    std::fs::write(&driver.executable, script).unwrap();
    let probe = tokio::spawn({
        let executable = driver.executable.clone();
        async move { PiDriver::probe_ready(&executable).await }
    });
    wait_for(|| count(dir.path(), "get_available_models") == 1).await;
    let ids = process_ids(dir.path());
    assert!(ids.iter().all(|id| present(*id)));
    probe.abort();
    let _ = probe.await;
    wait_for(|| ids.iter().all(|id| !present(*id))).await;
}
#[tokio::test]
async fn retry_after_a_cancelled_abort_waiter_joins_one_native_abort() {
    let script=FAKE.replace("elif t=='abort':out({'type':'agent_settled'});reply(r)","elif t=='abort':\n  import time\n  time.sleep(0.2);reply(r);time.sleep(0.1);out({'type':'agent_settled'})");
    let (dir, driver) = fixture_with(&script);
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let original = driver.connection(&handle.native_session_id).await.unwrap();
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "slow"))
        .await
        .unwrap();
    let first = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        async move { driver.interrupt(&handle).await }
    });
    wait_for(|| count(dir.path(), "abort") == 1).await;
    first.abort();
    let _ = first.await;
    driver.interrupt(&handle).await.unwrap();
    assert_eq!(count(dir.path(), "abort"), 1);
    let ended = events(stream).await;
    assert_eq!(
        ended
            .iter()
            .filter(|event| matches!(event, Ok(DriverEvent::TurnInterrupted { .. })))
            .count(),
        1
    );
    assert!(Arc::ptr_eq(
        &original,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    assert!(
        events(
            driver
                .prompt(&handle, prompt(dir.path(), "continue"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}
#[tokio::test]
async fn cancelled_abort_without_settlement_still_retires_and_can_cold_resume() {
    let script = FAKE.replace(
        "elif t=='abort':out({'type':'agent_settled'});reply(r)",
        "elif t=='abort':reply(r)",
    );
    let (dir, driver) = fixture_with(&script);
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let original = driver.connection(&handle.native_session_id).await.unwrap();
    let stream = driver
        .prompt(&handle, prompt(dir.path(), "slow"))
        .await
        .unwrap();
    let first = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        async move {
            driver
                .interrupt_with_timeout(&handle, Duration::from_millis(200))
                .await
        }
    });
    wait_for(|| count(dir.path(), "abort") == 1).await;
    first.abort();
    let _ = first.await;
    stopped(&original).await;
    assert_eq!(count(dir.path(), "abort"), 1);
    let ended = events(stream).await;
    assert!(ended.iter().any(Result::is_err));
    assert!(!ended.iter().any(|event| matches!(
        event,
        Ok(DriverEvent::TurnInterrupted { .. } | DriverEvent::TurnCompleted { .. })
    )));
    driver
        .resume_session(&handle.native_session_id, &dir.path().to_string_lossy())
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(
        &original,
        &driver.connection(&handle.native_session_id).await.unwrap()
    ));
    assert!(
        events(
            driver
                .prompt(&handle, prompt(dir.path(), "continue"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn an_old_shutdown_waiter_cannot_remove_a_cold_replacement() {
    let (dir, driver) = fixture_with(&stubborn(true));
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let old = driver.connection(&handle.native_session_id).await.unwrap();
    assert!(
        events(
            driver
                .prompt(&handle, prompt(dir.path(), "hello"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let closing = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        async move { driver.shutdown(handle).await }
    });
    wait_for(|| old.state.lock().unwrap().disconnected).await;
    driver
        .resume_session(&handle.native_session_id, &dir.path().to_string_lossy())
        .await
        .unwrap();
    closing.await.unwrap().unwrap();
    let replacement = driver.connection(&handle.native_session_id).await.unwrap();
    assert!(!Arc::ptr_eq(&old, &replacement));
    assert!(
        events(
            driver
                .prompt(&handle, prompt(dir.path(), "continue"))
                .await
                .unwrap()
        )
        .await
        .iter()
        .any(|event| matches!(event, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn blocked_stdin_writer_does_not_hold_native_shutdown_hostage() {
    let script = stubborn(false).replace(
        "'isStreaming':False})",
        "'isStreaming':False});time.sleep(60)",
    );
    let (dir, driver) = fixture_with(&script);
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let writer = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        let request = prompt(dir.path(), &"x".repeat(2 * 1024 * 1024));
        async move { driver.prompt(&handle, request).await }
    });
    wait_for(|| connection.input_busy()).await;
    tokio::time::timeout(Duration::from_secs(8), driver.shutdown(handle))
        .await
        .expect("shutdown must not wait for the 10-second stdin write timeout")
        .unwrap();
    assert!(writer.await.unwrap().is_err());
    assert!(ids.iter().all(|id| !present(*id)));
    assert!(connection.state.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn cancelling_a_partial_prompt_retires_the_pipe_and_its_pending_requests() {
    let script = stubborn(false).replace(
        "'isStreaming':False})",
        "'isStreaming':False});time.sleep(60)",
    );
    let (dir, driver) = fixture_with(&script);
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let writer = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        let request = prompt(dir.path(), &"x".repeat(2 * 1024 * 1024));
        async move { driver.prompt(&handle, request).await }
    });
    wait_for(|| connection.input_busy()).await;
    writer.abort();
    assert!(matches!(writer.await, Err(error) if error.is_cancelled()));
    let retired_by_cancellation = connection.state.lock().unwrap().disconnected;
    if retired_by_cancellation {
        stopped(&connection).await;
    } else {
        connection.close().await.unwrap();
    }
    assert!(retired_by_cancellation);
    assert!(
        connection
            .write(&json!({"type":"get_state"}))
            .await
            .is_err()
    );
    assert!(ids.iter().all(|id| !present(*id)));
    assert!(connection.state.lock().unwrap().pending.is_empty());
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn cancelling_an_unconfirmed_model_change_retires_the_original_native_state() {
    let script = stubborn(false).replace(
        "reply(r,{'model':model})",
        "time.sleep(60);reply(r,{'model':model})",
    );
    let (dir, driver) = fixture_with(&script);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let changing = tokio::spawn({
        let connection = connection.clone();
        async move {
            connection
                .call(json!({"type":"set_model","provider":"test","modelId":"other"}))
                .await
        }
    });
    wait_for(|| count(dir.path(), "set_model") == 1).await;
    changing.abort();
    assert!(matches!(changing.await, Err(error) if error.is_cancelled()));
    let retired_by_cancellation = connection.state.lock().unwrap().disconnected;
    if retired_by_cancellation {
        stopped(&connection).await;
    } else {
        connection.close().await.unwrap();
    }
    assert!(retired_by_cancellation);
    assert!(connection.state.lock().unwrap().pending.is_empty());
    assert!(ids.iter().all(|id| !present(*id)));
    driver.shutdown(handle).await.unwrap();
}

#[tokio::test]
async fn cancelling_an_admitted_prompt_waiting_for_input_cannot_leave_a_busy_turn() {
    let script = stubborn(false).replace(
        "'isStreaming':False})",
        "'isStreaming':False});time.sleep(60)",
    );
    let (dir, driver) = fixture_with(&script);
    let driver = Arc::new(driver);
    let handle = driver
        .create_session(session_request(&dir.path().to_string_lossy()))
        .await
        .unwrap();
    let connection = driver.connection(&handle.native_session_id).await.unwrap();
    let ids = process_ids(dir.path());
    let writer = tokio::spawn({
        let connection = connection.clone();
        async move {
            connection
                .write(&json!({"type":"blocked","blob":"x".repeat(2*1024*1024)}))
                .await
        }
    });
    wait_for(|| connection.input_busy()).await;
    let sending = tokio::spawn({
        let driver = driver.clone();
        let handle = handle.clone();
        let request = prompt(dir.path(), "queued behind input");
        async move { driver.prompt(&handle, request).await }
    });
    wait_for(|| {
        let state = connection.state.lock().unwrap();
        state.turn.is_some() && !state.pending.is_empty()
    })
    .await;
    sending.abort();
    assert!(matches!(sending.await, Err(error) if error.is_cancelled()));
    let retired_by_cancellation = connection.state.lock().unwrap().disconnected;
    if retired_by_cancellation {
        stopped(&connection).await;
    } else {
        connection.close().await.unwrap();
    }
    assert!(writer.await.unwrap().is_err());
    assert!(retired_by_cancellation);
    {
        let state = connection.state.lock().unwrap();
        assert!(state.pending.is_empty());
        assert!(state.turn.is_none());
    }
    assert!(ids.iter().all(|id| !present(*id)));
    driver.shutdown(handle).await.unwrap();
}
