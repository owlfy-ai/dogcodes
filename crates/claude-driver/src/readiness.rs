//! Retained initialize channels and ephemeral probes with supervised retirement.
use super::{
    Classified, ClaudeDriver, ClaudeLaunch, DriverError, PROBE_TIMEOUT, native::NativeProcess,
    read_until, spawn,
};
use agent_runtime::ProcessInput;
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream},
    sync::watch,
};

pub(super) struct ReadyConnection {
    pub native: std::sync::Arc<NativeProcess>,
    pub stdin: ProcessInput,
    pub stdout: BufReader<DuplexStream>,
}

impl Drop for ReadyConnection {
    fn drop(&mut self) {
        self.native.request_close();
    }
}

impl ReadyConnection {
    pub async fn retire(self) {
        let native = self.native.clone();
        drop(self);
        native.request_close();
        native.confirmed().await;
    }
}

pub(super) async fn connect(driver: &ClaudeDriver) -> Result<ReadyConnection, DriverError> {
    let child = spawn(
        &ClaudeLaunch {
            executable: driver.executable.clone(),
        },
        &uuid::Uuid::new_v4().to_string(),
        false,
        Some(&driver.executable.target.temp_directory().to_string_lossy()),
    )?;
    let (native, stdin, stdout) = NativeProcess::start_with_grace(child, Duration::from_secs(5))?;
    native.observe_owner(driver.stopping.subscribe());
    let mut connection = ReadyConnection {
        native,
        stdin,
        stdout: BufReader::new(stdout),
    };
    let result = tokio::time::timeout(PROBE_TIMEOUT, async {
        connection.stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"dogcodes-probe\",\"request\":{\"subtype\":\"initialize\",\"hooks\":{}}}\n").await
            .map_err(|_| DriverError::Disconnected("claude stdin rejected the probe".into()))?;
        connection.stdin.flush().await
            .map_err(|_| DriverError::Disconnected("claude stdin rejected the probe".into()))?;
        let mut reader = (&mut connection.stdout).take(super::FRAME_LIMIT as u64);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await
                .map_err(|_| DriverError::Protocol("Claude readiness response unavailable".into()))? == 0 {
                return Err(DriverError::Protocol("Claude readiness ended before initialize".into()));
            }
            if let Some(Classified::ControlResponse { request_id, success, data }) = super::classify(&line)
                && request_id == "dogcodes-probe" {
                if success && let Ok((models, auto)) = super::models::catalog(&data) {
                    *driver.startup_catalogs.lock().unwrap() = driver_api::models::StartupCatalogs {
                        models: Some(models), permissions: Some(driver_api::permissions::claude_with_auto(auto)),
                    };
                }
                return if success { Ok(()) } else { Err(DriverError::Protocol("Claude rejected initialize".into())) };
            }
        }
    }).await.map_err(|_| DriverError::Disconnected("Claude readiness timed out".into()))
        .and_then(std::convert::identity);
    if let Err(error) = result {
        connection.retire().await;
        return Err(error);
    }
    Ok(connection)
}

pub(super) async fn probe(executable: agent_runtime::TargetExecutable) -> Result<(), DriverError> {
    let (observer, owner) = watch::channel(false);
    let result = tokio::spawn(async move {
        if owner.has_changed().is_err() {
            return Err(DriverError::Disconnected("Claude probe observer ended".into()));
        }
        let temp_directory = executable.target.temp_directory();
        let child = spawn(
            &ClaudeLaunch { executable },
            &uuid::Uuid::new_v4().to_string(),
            false,
            Some(&temp_directory.to_string_lossy()),
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
