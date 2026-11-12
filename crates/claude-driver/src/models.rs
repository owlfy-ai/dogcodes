//! Unscoped metadata can reuse the readiness control channel. Other helpers are
//! serialized through physical retirement, including when their caller leaves.
use super::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, ClaudeDriver, ClaudeLaunch, DriverError,
    PROBE_TIMEOUT, Value, native::NativeProcess, spawn,
};
use driver_api::{ModelCatalog, ModelOption, ThinkingOption};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use tokio::io::AsyncReadExt;

pub(super) async fn read(
    driver: &ClaudeDriver,
    workspace: Option<&str>,
) -> Result<ModelCatalog, DriverError> {
    read_with_auto(driver, workspace)
        .await
        .map(|(catalog, _)| catalog)
}

pub(super) async fn read_with_auto(
    driver: &ClaudeDriver,
    workspace: Option<&str>,
) -> Result<(ModelCatalog, bool), DriverError> {
    // A cancelled waiter never starts a helper. This gate is separate from
    // live sessions, so reading a menu cannot wait behind a running prompt.
    let admission = driver.catalog_probe.clone().lock_owned().await;
    let prepared = driver.ready_connection.lock().await.take();
    let ready_slot = driver.ready_connection.clone();
    let executable = driver.executable.clone();
    let workspace = workspace.map(str::to_owned);
    let (observer, mut owner) = tokio::sync::watch::channel(false);
    let result = tokio::spawn(async move {
        let _admission = admission;
        if owner.has_changed().is_err() {
            if let Some(connection) = prepared {
                connection.retire().await;
            }
            return Err(DriverError::Disconnected(
                "Claude catalog observer ended".into(),
            ));
        }
        let prepared = if let Some(connection) = prepared {
            if workspace.is_none() && !connection.native.stopping() {
                Some(connection)
            } else {
                connection.retire().await;
                None
            }
        } else {
            None
        };
        if owner.has_changed().is_err() {
            if let Some(connection) = prepared {
                connection.retire().await;
            }
            return Err(DriverError::Disconnected(
                "Claude catalog observer ended".into(),
            ));
        }
        let keep_ready = prepared.is_some();
        let mut connection = if let Some(connection) = prepared {
            connection
        } else {
            let temp_directory = executable.target.temp_directory();
            let child = spawn(
                &ClaudeLaunch { executable },
                &uuid::Uuid::new_v4().to_string(),
                false,
                Some(workspace.as_deref().unwrap_or(&temp_directory.to_string_lossy())),
            )?;
            // Reuse the business-session supervisor: drain both pipes and retain
            // the original OS scope independently of this observer's lifetime.
            let (native, stdin, stdout) = NativeProcess::start(child)?;
            super::readiness::ReadyConnection {
                native,
                stdin,
                stdout: BufReader::new(stdout),
            }
        };
        let result = tokio::select! {
            biased;
            _ = owner.changed() => Err(DriverError::Disconnected("Claude catalog observer ended".into())),
            result = async { AssertUnwindSafe(tokio::time::timeout(
            PROBE_TIMEOUT,
            read_catalog(&mut connection.stdin, &mut connection.stdout),
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Ok(Err(DriverError::Disconnected(
                "Claude catalog worker ended unexpectedly".into(),
            )))
        })
        .map_err(|_| DriverError::Disconnected("backendText.m5b7ea04d".into()))
        .and_then(std::convert::identity) } => result,
        };
        if keep_ready && result.is_ok() && owner.has_changed().is_ok() && !connection.native.stopping() {
            *ready_slot.lock().await = Some(connection);
            return result;
        }
        connection.retire().await;
        result
    })
    .await
    .map_err(|_| {
        DriverError::Disconnected("Claude catalog worker ended without a result".into())
    })?;
    drop(observer);
    result
}

async fn read_catalog(
    stdin: &mut agent_runtime::ProcessInput,
    stdout: &mut BufReader<tokio::io::DuplexStream>,
) -> Result<(ModelCatalog, bool), DriverError> {
    stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"model-catalog\",\"request\":{\"subtype\":\"initialize\",\"hooks\":{}}}\n").await.map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
    stdin
        .flush()
        .await
        .map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
    let mut reader = stdout.take(1024 * 1024);
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .await
            .map_err(|_| DriverError::Protocol("Claude model response unavailable".into()))?
            == 0
        {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["type"] != "control_response" || value["response"]["request_id"] != "model-catalog"
        {
            continue;
        }
        return catalog(&value["response"]["response"]);
    }
    Err(DriverError::Protocol(
        "Claude model response ended early".into(),
    ))
}

pub(super) fn catalog(value: &Value) -> Result<(ModelCatalog, bool), DriverError> {
    let rows = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| DriverError::CapabilityUnavailable("backendText.mb1a5d447".into()))?;
    let mut catalog = ModelCatalog::default();
    for row in rows {
        let Some(id) = row["value"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        if catalog.models.iter().any(|m| m.id == id) {
            continue;
        }
        let mut model = ModelOption::new(id, row["displayName"].as_str().unwrap_or(id));
        model.thinking_options = ["off", "auto", "low", "medium", "high", "xhigh", "max"]
            .iter()
            .map(|id| ThinkingOption {
                id: (*id).into(),
                label: (*id).into(),
            })
            .collect();
        model.default_thinking = Some("auto".into());
        catalog.models.push(model);
    }
    if catalog.models.is_empty() {
        return Err(DriverError::CapabilityUnavailable(
            "backendText.mb4bec615".into(),
        ));
    }
    catalog.default_model = catalog.models.first().map(|model| model.id.clone());
    // Third-party aliases can resolve to the same display name.
    let labels: Vec<_> = catalog.models.iter().map(|m| m.label.clone()).collect();
    for model in &mut catalog.models {
        if labels.iter().filter(|label| **label == model.label).count() > 1 {
            model.label = format!("{} ({})", model.label, model.id);
        }
    }
    Ok((catalog, rows.iter().any(|r| r["supportsAutoMode"] == true)))
}

#[cfg(test)]
mod installed_tests {
    use super::*;
    #[test]
    fn catalog_preserves_native_order_without_inserting_a_default_model() {
        let (catalog, _) = catalog(&serde_json::json!({"models":[
            {"value":"first","displayName":"First"},
            {"value":"second","displayName":"Second"}
        ]}))
        .unwrap();
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(catalog.default_model.as_deref(), Some("first"));
    }
    #[tokio::test]
    #[ignore = "reads the locally installed Claude model catalog, no model turn"]
    async fn local_model_catalog() {
        let path = std::env::var("DOGCODES_CLAUDE_BINARY").unwrap();
        let driver = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            std::path::PathBuf::from(path),
            None,
            driver_api::SessionResumeCapability::Supported,
        )
        .unwrap();
        let catalog = read(&driver, None).await.unwrap();
        assert!(!catalog.models.is_empty());
        println!("{}", serde_json::to_string(&catalog).unwrap());
    }
}

#[cfg(all(test, unix))]
mod scope_tests;
