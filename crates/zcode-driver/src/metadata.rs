//! Catalog probes own a private process; they never lock the active turn's
//! connection. The admission guard outlives the observer until actual cleanup.
use super::{Connection, DriverError, ZcodeDriver, models, read_native_catalog};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, path::PathBuf};

pub(super) async fn read(
    driver: &ZcodeDriver,
    workspace: PathBuf,
) -> Result<driver_api::ModelCatalog, DriverError> {
    // Cancellation while queued must not start a helper later.
    let admission = driver.catalog_probe.clone().lock_owned().await;
    let launch = driver.launch.clone();
    let (observer, owner) = tokio::sync::watch::channel(false);
    let result = tokio::spawn(async move {
        let _admission = admission;
        if owner.has_changed().is_err() {
            return Err(DriverError::Disconnected(
                "ZCode catalog observer ended".into(),
            ));
        }
        let mut connection = Connection::spawn(&launch)?;
        connection.observe_owner(owner);
        let result = AssertUnwindSafe(async {
            connection.refresh_account(true).await?;
            read_native_catalog(&mut connection, &workspace).await
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(DriverError::Disconnected(
                "ZCode catalog worker ended unexpectedly".into(),
            ))
        });
        // Cancelling/closing a private helper cannot interrupt a user turn.
        // Keep its admission through unknown stop, without spawning or replaying.
        connection.native.request_close();
        connection.native.confirmed().await;
        result.map(|state| models::catalog(&state))
    })
    .await
    .map_err(|_| DriverError::Disconnected("ZCode catalog worker ended without a result".into()))?;
    drop(observer);
    result
}

#[cfg(all(test, unix))]
pub(super) mod tests;
