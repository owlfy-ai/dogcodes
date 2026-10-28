//! Metadata helpers are serialized within one driver and retain admission until
//! their original process tree exits, even when the requesting UI disappears.
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
    let executable = driver.executable.clone();
    let workspace = workspace.map(str::to_owned);
    let (observer, owner) = tokio::sync::watch::channel(false);
    let result = tokio::spawn(async move {
        let _admission = admission;
        if owner.has_changed().is_err() {
            return Err(DriverError::Disconnected(
                "Claude catalog observer ended".into(),
            ));
        }
        let child = spawn(
            &ClaudeLaunch { executable },
            &uuid::Uuid::new_v4().to_string(),
            false,
            workspace.as_deref(),
        )?;
        // Reuse the business-session supervisor: drain both pipes and retain
        // the original OS scope independently of this observer's lifetime.
        let (native, stdin, stdout) = NativeProcess::start(child)?;
        native.observe_owner(owner);
        let result = AssertUnwindSafe(tokio::time::timeout(
            PROBE_TIMEOUT,
            read_catalog(stdin, stdout),
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Ok(Err(DriverError::Disconnected(
                "Claude catalog worker ended unexpectedly".into(),
            )))
        })
        .map_err(|_| DriverError::Disconnected("Claude 模型目录读取超时".into()))
        .and_then(std::convert::identity);
        native.request_close();
        native.confirmed().await;
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
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::io::DuplexStream,
) -> Result<(ModelCatalog, bool), DriverError> {
    stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"model-catalog\",\"request\":{\"subtype\":\"initialize\",\"hooks\":{}}}\n").await.map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
    stdin
        .flush()
        .await
        .map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
    let mut reader = BufReader::new(stdout.take(1024 * 1024));
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
        let rows = value
            .pointer("/response/response/models")
            .and_then(Value::as_array)
            .ok_or_else(|| DriverError::CapabilityUnavailable("Claude 未返回模型目录".into()))?;
        let mut catalog = ModelCatalog {
            default_model: Some("default".into()),
            ..Default::default()
        };
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
                "Claude 未返回可用模型".into(),
            ));
        }
        if !catalog.models.iter().any(|m| m.id == "default") {
            let mut model = ModelOption::new("default", "默认（推荐）");
            if let Some(first) = catalog.models.first() {
                model.thinking_options.clone_from(&first.thinking_options);
                model.default_thinking.clone_from(&first.default_thinking);
            }
            catalog.models.insert(0, model);
        }
        // Third-party aliases can resolve to the same display name.
        let labels: Vec<_> = catalog.models.iter().map(|m| m.label.clone()).collect();
        for model in &mut catalog.models {
            if labels.iter().filter(|label| **label == model.label).count() > 1 {
                model.label = format!("{} ({})", model.label, model.id);
            }
        }
        return Ok((catalog, rows.iter().any(|r| r["supportsAutoMode"] == true)));
    }
    Err(DriverError::Protocol(
        "Claude model response ended early".into(),
    ))
}

#[cfg(test)]
mod installed_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "reads the locally installed Claude model catalog, no model turn"]
    async fn local_model_catalog() {
        let path = std::env::var("DOGCODES_CLAUDE_BINARY").unwrap();
        let driver = ClaudeDriver::new(
            "claude-code",
            &agent_domain::InstallationId::new().to_string(),
            path.into(),
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
