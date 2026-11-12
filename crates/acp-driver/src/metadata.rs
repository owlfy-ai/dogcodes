//! Catalog probes create native sessions. Not every deployed Agent advertises
//! optional session/close, so each probe owns a physical scope through cleanup.
use super::{AcpDriver, Command, DriverError};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::sync::oneshot;

impl AcpDriver {
    pub(super) async fn metadata<T: Send + 'static>(
        &self,
        workspace: Option<String>,
        command: impl FnOnce(oneshot::Sender<Result<T, DriverError>>) -> Command + Send + 'static,
    ) -> Result<T, DriverError> {
        self.isolated_query("backendText.mec834e6f", workspace, command)
            .await
    }
    pub(super) async fn isolated_query<T: Send + 'static>(
        &self,
        label: &'static str,
        workspace: Option<String>,
        command: impl FnOnce(oneshot::Sender<Result<T, DriverError>>) -> Command + Send + 'static,
    ) -> Result<T, DriverError> {
        // One original scope remains admitted through proven physical exit.
        // A cancelled waiter cannot free the slot and spawn over stale work.
        let permit = tokio::time::timeout(
            Duration::from_secs(45),
            self.shared.metadata.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            DriverError::CapabilityUnavailable(format!(
                "backendText.m32dd7a58\u{1e}{}",
                serde_json::to_string(&[label.to_string()]).unwrap_or_else(|_| "[]".into())
            ))
        })?;
        let driver = Self::new(
            self.executor.provider_id.as_str(),
            self.executor.installation_id.as_deref().unwrap_or(""),
            self.shared.launch.clone(),
            self.native_version.clone(),
            self.resume_capability,
        )
        .map_err(|_| DriverError::Protocol("invalid ACP metadata installation".into()))?;
        let (mut reply, result) = oneshot::channel();
        #[cfg(test)]
        let budget = self.metadata_timeout;
        #[cfg(not(test))]
        let budget = Duration::from_secs(30);
        tokio::spawn(async move {
            let _permit = permit;
            let query = async {
                let connection = driver
                    .ensure_workspace_connection(None, workspace.as_deref())
                    .await?;
                let (reply, response) = oneshot::channel();
                connection
                    .commands
                    .send(command(reply))
                    .map_err(|_| super::driver_error_from_channel())?;
                response
                    .await
                    .map_err(|_| super::driver_error_from_channel())?
            };
            let outcome = tokio::select! {
                biased;
                () = reply.closed() => None,
                result = tokio::time::timeout(budget, query) => Some(result.unwrap_or_else(|_| Err(
                    DriverError::Disconnected(format!("backendText.m10d1691f\u{1e}{}", serde_json::to_string(&[label.to_string()]).unwrap_or_else(|_| "[]".into())))
                ))),
            };
            // Drop/timeout stops this helper only. The original driver and all
            // business residents keep their native connection and config.
            driver.stopping.send_replace(true);
            let finished = {
                let connection = driver.shared.connection.lock().await;
                connection.as_ref().map(|connection| {
                    if let Some(native) = &connection.native {
                        native.request_close();
                    }
                    connection.finished.clone()
                })
            };
            drop(driver);
            if let Some(finished) = finished {
                let wait = async {
                    while !finished.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                };
                if tokio::time::timeout(Duration::from_secs(10), wait)
                    .await
                    .is_err()
                {
                    let _ = reply.send(Err(DriverError::Disconnected(format!(
                        "backendText.m179437ef\u{1e}{}",
                        serde_json::to_string(&[label.to_string()]).unwrap_or_else(|_| "[]".into())
                    ))));
                    // Cleanup is still owned. Do not release admission merely
                    // because our caller has received an unconfirmed result.
                    while !finished.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    return;
                }
            }
            if let Some(outcome) = outcome {
                let _ = reply.send(outcome);
            }
        });
        result
            .await
            .map_err(|_| super::driver_error_from_channel())?
    }
}
