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
        self.isolated_query("模型或权限", workspace, command).await
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
            DriverError::CapabilityUnavailable(format!("上一项{label}查询仍在结束，请稍后重试"))
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
                    DriverError::Disconnected(format!("{label}查询超时"))
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
                        "临时{label}查询的进程退出仍待确认"
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
