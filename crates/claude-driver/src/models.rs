pub(super) async fn read(
    executable: &std::path::Path,
    workspace: Option<&str>,
) -> Result<ModelCatalog, DriverError> {
    read_with_auto(executable, workspace)
        .await
        .map(|(catalog, _)| catalog)
}
use super::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, ClaudeLaunch, DriverError, PROBE_TIMEOUT, Value,
    spawn,
};
use driver_api::{ModelCatalog, ModelOption, ThinkingOption};
use tokio::io::AsyncReadExt;

pub(super) async fn read_with_auto(
    executable: &std::path::Path,
    workspace: Option<&str>,
) -> Result<(ModelCatalog, bool), DriverError> {
    let mut child = spawn(
        &ClaudeLaunch {
            executable: executable.into(),
        },
        &uuid::Uuid::new_v4().to_string(),
        false,
        workspace,
    )?;
    let result = async {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| DriverError::Disconnected("Claude stdin unavailable".into()))?;
        stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"model-catalog\",\"request\":{\"subtype\":\"initialize\",\"hooks\":{}}}\n").await.map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
        stdin
            .flush()
            .await
            .map_err(|_| DriverError::Disconnected("Claude model request failed".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Disconnected("Claude stdout unavailable".into()))?;
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
            if value["type"] != "control_response"
                || value["response"]["request_id"] != "model-catalog"
            {
                continue;
            }
            let rows = value
                .pointer("/response/response/models")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DriverError::CapabilityUnavailable("Claude 未返回模型目录".into())
                })?;
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
    };
    let outcome = tokio::time::timeout(PROBE_TIMEOUT, result)
        .await
        .map_err(|_| DriverError::Disconnected("Claude 模型目录读取超时".into()));
    if tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    outcome?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "reads the locally installed Claude model catalog, no model turn"]
    async fn local_model_catalog() {
        let path = std::env::var("DOGCODES_CLAUDE_BINARY").unwrap();
        let catalog = read(std::path::Path::new(&path), None).await.unwrap();
        assert!(!catalog.models.is_empty());
        println!("{}", serde_json::to_string(&catalog).unwrap());
    }
}
