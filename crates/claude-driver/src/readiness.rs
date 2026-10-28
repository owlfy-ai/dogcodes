//! Read-only initialize probe supervised through natural exit and observer loss.
use super::{
    Classified, ClaudeLaunch, DriverError, PROBE_TIMEOUT, native::NativeProcess, read_until, spawn,
};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, path::Path, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::watch,
};

pub(super) async fn probe(executable: &Path) -> Result<(), DriverError> {
    let executable = executable.to_path_buf();
    let (observer, owner) = watch::channel(false);
    let result = tokio::spawn(async move {
        if owner.has_changed().is_err() {
            return Err(DriverError::Disconnected("Claude probe observer ended".into()));
        }
        let child = spawn(
            &ClaudeLaunch { executable },
            &uuid::Uuid::new_v4().to_string(),
            false,
            None,
        )?;
        // The existing supervisor drains stderr without retaining diagnostics,
        // bounds the stdout pipe, and owns physical cleanup independently of UI.
        let (native, mut stdin, stdout) = NativeProcess::start_with_grace(child, Duration::from_secs(5))?;
        native.observe_owner(owner);
        let outcome = AssertUnwindSafe(async {
            stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"dogcodes-probe\",\"request\":{\"subtype\":\"initialize\",\"hooks\":{}}}\n").await
                .map_err(|_| DriverError::Disconnected("claude stdin rejected the probe".into()))?;
            stdin.flush().await
                .map_err(|_| DriverError::Disconnected("claude stdin rejected the probe".into()))?;
            // EOF asks Claude to finish normally, preserving its credential
            // persistence opportunity instead of killing a successful handshake.
            drop(stdin);
            read_until(BufReader::new(stdout), PROBE_TIMEOUT, |classified| {
                matches!(classified, Classified::ControlResponse { request_id, success: true, .. } if request_id == "dogcodes-probe")
            }).await
        }).catch_unwind().await.unwrap_or_else(|_| {
            Err(DriverError::Disconnected("Claude probe worker ended unexpectedly".into()))
        });
        native.request_close();
        native.confirmed().await;
        match outcome {
            Ok(true) => Ok(()),
            Ok(false) => Err(DriverError::Protocol(
                "claude stream-json probe ended before the control response".into(),
            )),
            Err(_) => Err(DriverError::Protocol(
                "claude did not answer the stream-json control initialize handshake".into(),
            )),
        }
    }).await.map_err(|_| DriverError::Disconnected("Claude probe worker ended without a result".into()))?;
    drop(observer);
    result
}
