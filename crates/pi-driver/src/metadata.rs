//! Private ephemeral catalog queries hold one admission through original
//! process retirement; no live session is used for metadata configuration.
use super::{Connection, DriverError, PiDriver, Value, json, model_id};
use driver_api::{ModelCatalog, ModelOption, ThinkingOption};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, time::Duration};

pub(super) async fn read(
    driver: &PiDriver,
    model: Option<&str>,
    workspace: Option<&str>,
) -> Result<ModelCatalog, DriverError> {
    // No detached task exists while queued, so cancelling this future cannot
    // start a helper later when another query finally retires.
    let admission = driver.catalog_probe.clone().lock_owned().await;
    let executable = driver.executable.clone();
    let model = model.map(str::to_owned);
    let workspace = workspace.map_or_else(
        || std::env::temp_dir().to_string_lossy().into_owned(),
        str::to_owned,
    );
    let (observer, owner) = tokio::sync::watch::channel(false);
    let result = tokio::spawn(async move {
        let _admission = admission;
        if owner.has_changed().is_err() {
            return Err(DriverError::Disconnected(
                "Pi catalog observer ended".into(),
            ));
        }
        let connection = Connection::spawn(&executable, &workspace, None, true)?;
        connection.observe_owner(owner);
        let result = AssertUnwindSafe(tokio::time::timeout(
            Duration::from_secs(30),
            catalog(&connection, model.as_deref()),
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Ok(Err(DriverError::Disconnected(
                "Pi catalog worker ended unexpectedly".into(),
            )))
        })
        .map_err(|_| DriverError::Disconnected("Pi model catalog timed out".into()))
        .and_then(std::convert::identity);
        // close() may stop observing before the native process is gone. Keep
        // the actual proof and the same admission instead of returning/retrying.
        connection.request_close();
        connection.confirmed().await;
        result
    })
    .await
    .map_err(|_| DriverError::Disconnected("Pi catalog worker ended without a result".into()))?;
    drop(observer);
    result
}

async fn catalog(
    connection: &Connection,
    model: Option<&str>,
) -> Result<ModelCatalog, DriverError> {
    let initial = connection.call(json!({"type":"get_state"})).await?;
    let state = PiDriver::configure(connection, model, None).await?;
    let models = connection
        .call(json!({"type":"get_available_models"}))
        .await?;
    let levels = connection
        .call(json!({"type":"get_available_thinking_levels"}))
        .await?;
    let selected = model_id(&state["model"]);
    Ok(ModelCatalog {
        default_model: model_id(&initial["model"]),
        models: models["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let id = model_id(row)?;
                let mut option = ModelOption::new(
                    &id,
                    &format!(
                        "{} · {}",
                        row["name"].as_str().unwrap_or(&id),
                        row["provider"].as_str().unwrap_or("")
                    ),
                );
                if Some(&id) == selected.as_ref() {
                    option.thinking_options = levels["levels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|v| ThinkingOption {
                            id: v.into(),
                            label: v.into(),
                        })
                        .collect();
                    option.default_thinking = state["thinkingLevel"].as_str().map(Into::into);
                }
                Some(option)
            })
            .collect(),
    })
}
