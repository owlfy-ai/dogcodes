//! Bounded read-only helpers, owned before their initialization handshake.
use super::{CodexDriver, Connection, DriverError};
use driver_api::ModelCatalog;
use futures::{FutureExt, future::BoxFuture};
use serde_json::{Value, json};
use std::sync::Arc;
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::sync::{Mutex, oneshot};

pub(super) async fn read(driver: &CodexDriver) -> Result<ModelCatalog, DriverError> {
    query(
        driver,
        driver.catalog_probe.clone(),
        "model catalog",
        |connection| Box::pin(connection.model_catalog()),
    )
    .await
}

pub(super) async fn read_history(driver: &CodexDriver, id: &str) -> Result<Value, DriverError> {
    let id = id.to_owned();
    query(
        driver,
        driver.history_probe.clone(),
        "child history",
        move |connection| {
            Box::pin(async move {
                connection
                    .request("thread/read", json!({"threadId":id,"includeTurns":true}))
                    .await
                    .map(|exchange| exchange.result)
            })
        },
    )
    .await
}

async fn query<T: Send + 'static>(
    driver: &CodexDriver,
    admission: Arc<Mutex<()>>,
    label: &'static str,
    operation: impl for<'a> FnOnce(&'a mut Connection) -> BoxFuture<'a, Result<T, DriverError>>
    + Send
    + 'static,
) -> Result<T, DriverError> {
    // A cancelled queued caller cannot launch a helper later.
    let admission = admission.lock_owned().await;
    let executable = driver.executable.clone();
    let (mut reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _admission = admission;
        if reply.is_closed() {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut connection = match Connection::create(&executable) {
            Ok(connection) => connection,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        // Retain this exact OS scope even if cancellation occurs before the
        // handshake has produced a usable protocol connection.
        let native = connection.native.clone();
        let outcome = tokio::select! {
            biased;
            () = reply.closed() => None,
            result = AssertUnwindSafe(tokio::time::timeout_at(deadline, async {
                connection.initialize().await?;
                operation(&mut connection).await
            })).catch_unwind() => Some(result
                .unwrap_or_else(|_| Ok(Err(DriverError::Disconnected(format!("Codex {label} worker ended unexpectedly")))))
                .map_err(|_| DriverError::Disconnected(format!("Codex {label} timed out")))
                .and_then(std::convert::identity)),
        };
        // Preserve EOF and the original native grace, not just a kill request.
        connection.transport.writer.take();
        native.request_close();
        native.confirmed().await;
        if let Some(outcome) = outcome {
            let _ = reply.send(outcome);
        }
    });
    result.await.map_err(|_| {
        DriverError::Disconnected(format!("Codex {label} worker ended without a result"))
    })?
}
