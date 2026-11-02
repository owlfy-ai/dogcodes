//! Default catalogs can reuse readiness. Selected models and other workspaces
//! use private scopes retired before release; business sessions stay isolated.
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
    let prepared = driver.ready_connection.lock().await.take();
    let ready_slot = driver.ready_connection.clone();
    let executable = driver.executable.clone();
    let model = model.map(str::to_owned);
    let workspace = workspace.map_or_else(
        || std::env::temp_dir().to_string_lossy().into_owned(),
        str::to_owned,
    );
    let (observer, mut owner) = tokio::sync::watch::channel(false);
    let result = tokio::spawn(async move {
        let _admission = admission;
        if owner.has_changed().is_err() {
            if let Some(connection) = prepared {
                connection.request_close();
                connection.confirmed().await;
            }
            return Err(DriverError::Disconnected(
                "Pi catalog observer ended".into(),
            ));
        }
        let prepared = if let Some(connection) = prepared {
            let reusable = std::path::Path::new(&workspace) == std::env::temp_dir()
                && !connection.state.lock().expect("Pi state").disconnected;
            if reusable {
                Some(connection)
            } else {
                connection.request_close();
                connection.confirmed().await;
                None
            }
        } else {
            None
        };
        if owner.has_changed().is_err() {
            if let Some(connection) = prepared {
                connection.request_close();
                connection.confirmed().await;
            }
            return Err(DriverError::Disconnected(
                "Pi catalog observer ended".into(),
            ));
        }
        let keep_ready = prepared.is_some() && model.is_none();
        let connection = match prepared {
            Some(connection) => connection,
            None => Connection::spawn(&executable, &workspace, None, true)?,
        };
        let result = tokio::select! {
            biased;
            _ = owner.changed() => Err(DriverError::Disconnected("Pi catalog observer ended".into())),
            result = async { AssertUnwindSafe(tokio::time::timeout(
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
        .and_then(std::convert::identity) } => result,
        };
        if keep_ready && result.is_ok() && owner.has_changed().is_ok()
            && !connection.state.lock().expect("Pi state").disconnected
        {
            // Only default reads preserve the initialized model. A selected
            // model mutates the private scope, so that query still retires it.
            *ready_slot.lock().await = Some(connection);
            return result;
        }
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
