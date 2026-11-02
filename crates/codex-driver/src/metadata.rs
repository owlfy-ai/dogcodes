//! Bounded read-only helpers and a thread-free retained readiness connection.
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
        true,
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
        false,
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
    reuse_ready: bool,
    operation: impl for<'a> FnOnce(&'a mut Connection) -> BoxFuture<'a, Result<T, DriverError>>
    + Send
    + 'static,
) -> Result<T, DriverError> {
    // A cancelled queued caller cannot launch a helper later.
    let admission = admission.lock_owned().await;
    let prepared = if reuse_ready {
        driver.ready_connection.lock().await.take()
    } else {
        None
    };
    let executable = driver.executable.clone();
    let ready_slot = driver.ready_connection.clone();
    let (mut reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _admission = admission;
        if reply.is_closed() {
            if let Some(mut connection) = prepared {
                connection.transport.writer.take();
                connection.native.request_close();
                connection.native.confirmed().await;
            }
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let prepared = if let Some(connection) = prepared {
            if connection.native.stopping() {
                connection.native.confirmed().await;
                None
            } else {
                Some(connection)
            }
        } else {
            None
        };
        if reply.is_closed() {
            if let Some(mut connection) = prepared {
                connection.transport.writer.take();
                connection.native.request_close();
                connection.native.confirmed().await;
            }
            return;
        }
        let initialized = prepared.is_some();
        let mut connection = match prepared.map_or_else(|| Connection::create(&executable), Ok) {
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
                if !initialized { connection.initialize().await?; }
                operation(&mut connection).await
            })).catch_unwind() => Some(result
                .unwrap_or_else(|_| Ok(Err(DriverError::Disconnected(format!("Codex {label} worker ended unexpectedly")))))
                .map_err(|_| DriverError::Disconnected(format!("Codex {label} timed out")))
                .and_then(std::convert::identity)),
        };
        if initialized
            && outcome.as_ref().is_some_and(Result::is_ok)
            && !reply.is_closed()
            && !native.stopping()
        {
            // This thread-free connection belongs to readiness, not a business
            // session. Fully completed reads can return it to the same owner.
            *ready_slot.lock().await = Some(connection);
            if let Some(outcome) = outcome {
                let _ = reply.send(outcome);
            }
            return;
        }
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
