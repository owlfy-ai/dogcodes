//! An owned, single native close request. A disappearing caller or an elapsed
//! deadline never releases the original session/connection ownership.
use super::{
    Arc, Command, ConnectionHandle, DriverError, Duration, Ordering, Resident, Shared, finish,
    oneshot, turn, watch,
};

pub(super) async fn retire_native(
    shared: Arc<Shared>,
    id: String,
    resident: Arc<Resident>,
    completed: watch::Sender<turn::Outcome>,
    original: Option<ConnectionHandle>,
    last_owner: bool,
    timeout: Duration,
) {
    let response = original.as_ref().and_then(|original| {
        let (reply, response) = oneshot::channel();
        original
            .commands
            .send(Command::CloseSession {
                session_id: id.clone(),
                reply,
            })
            .ok()
            .map(|()| response)
    });
    let pending = async {
        match response {
            Some(response) => response
                .await
                .unwrap_or_else(|_| Err(super::super::driver_error_from_channel())),
            None => Err(super::super::driver_error_from_channel()),
        }
    };
    tokio::pin!(pending);
    let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(3));
    let outcome = tokio::select! {
        biased;
        () = exited(&resident) => { complete(&shared, &id, &resident, &completed).await; return; }
        result = &mut pending => Some(result),
        () = tokio::time::sleep_until(deadline) => None,
    };
    if last_owner {
        if matches!(outcome, Some(Ok(()))) {
            // Let the old prompt settle before graceful connection retirement.
            // The deadline still bounds a broken local completion path.
            tokio::select! {
                () = confirm_turn(&resident) => {},
                () = tokio::time::sleep_until(deadline) => {},
            }
        }
        if let Some(native) = &resident.native {
            native.request_close();
        }
        // Process scope + SDK/background-task exit is the final proof, even
        // after an error or an absent native close response.
        exited(&resident).await;
        complete(&shared, &id, &resident, &completed).await;
        return;
    }
    let outcome = if let Some(result) = outcome {
        result
    } else {
        completed.send_replace(Some(Err(DriverError::Disconnected(
            "ACP session/close has no confirmed response".into(),
        ))));
        // Keep the very same request, not a replay. A late ACK may still
        // finish it; peer retirement may instead prove physical exit.
        tokio::select! {
            biased;
            () = exited(&resident) => { complete(&shared, &id, &resident, &completed).await; return; }
            result = &mut pending => result,
        }
    };

    if let Err(error) = outcome {
        completed.send_replace(Some(Err(error)));
        exited(&resident).await;
    } else {
        confirm_turn(&resident).await;
        if !resident.epoch.load(Ordering::Acquire) {
            // Native IDs cannot be reused by new/load until this original
            // retirement clears its cached state and publishes completion.
            let forgotten = original.as_ref().and_then(|original| {
                let (reply, response) = oneshot::channel();
                original
                    .commands
                    .send(Command::ForgetSession {
                        session_id: id.clone(),
                        reply,
                    })
                    .ok()
                    .map(|()| response)
            });
            let cleared = match forgotten {
                Some(response) => tokio::select! {
                    result = response => result.is_ok(),
                    () = exited(&resident) => true,
                },
                None => false,
            };
            if !cleared {
                completed.send_replace(Some(Err(super::super::driver_error_from_channel())));
                exited(&resident).await;
            }
        }
    }
    complete(&shared, &id, &resident, &completed).await;
}

async fn complete(
    shared: &Shared,
    id: &str,
    resident: &Arc<Resident>,
    completed: &watch::Sender<turn::Outcome>,
) {
    // Both native ACK and physical exit prove the old work has stopped. Wait
    // for our original prompt owner to publish its terminal before admitting
    // a replacement session; a PID-only proof cannot reset UI/task ownership.
    confirm_turn(resident).await;
    finish(shared, id, resident, completed).await;
}

async fn confirm_turn(resident: &Resident) {
    resident.native_closed.store(true, Ordering::Release);
    let turn = resident.turn.lock().expect("ACP session turn").clone();
    if let Some(turn) = turn {
        turn.native_closed.send_replace(true);
        let mut done = turn.done.clone();
        loop {
            if done.borrow_and_update().is_some() {
                return;
            }
            tokio::select! {
                changed = done.changed() => {
                    if changed.is_err() { tokio::time::sleep(Duration::from_millis(20)).await; }
                },
                () = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
        }
    }
}
async fn exited(resident: &Resident) {
    while !resident.epoch.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
