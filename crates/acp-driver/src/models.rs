use super::{Agent, ConnectionTo, DriverError, SessionId, StderrTail, map_session_error};
use driver_api::{ModelCatalog, ModelOption};
use serde_json::{Value, json};

pub(super) fn has_kiro_template(value: &Value) -> bool {
    value
        .pointer("/agentCapabilities/_meta/kiro/extensionMethods")
        .and_then(Value::as_array)
        .is_some_and(|methods| {
            methods
                .iter()
                .any(|method| method == "_kiro/config/template")
        })
}
pub(super) async fn kiro_template(connection: &ConnectionTo<Agent>) -> Result<Value, DriverError> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let template = raw(connection, "_kiro/config/template", json!({})).await?;
            if !catalog(&template).models.is_empty() {
                return Ok(template);
            }
            // Native catalog refresh starts asynchronously after authentication.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| DriverError::Disconnected("Kiro model catalog refresh timed out".into()))?
}

pub(super) async fn raw(
    connection: &ConnectionTo<Agent>,
    method: &str,
    params: Value,
) -> Result<Value, DriverError> {
    let message = agent_client_protocol::UntypedMessage::new(method, params)
        .map_err(|_| DriverError::Protocol("invalid model request".into()))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        connection.send_request(message).block_task(),
    )
    .await
    .map_err(|_| DriverError::Disconnected("模型请求超时".into()))?
    .map_err(|error| map_session_error(&error, "Agent 未提供可用的模型配置接口", None))
}
fn flatten_options(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|r| {
            r.get("options")
                .and_then(Value::as_array)
                .map_or_else(|| vec![r], |items| items.iter().collect())
        })
        .collect()
}
fn config<'a>(value: &'a Value, category: &str) -> Option<&'a Value> {
    value["configOptions"].as_array()?.iter().find(|c| {
        c["category"] == category
            || c["id"] == category
            || (category == "thought_level" && c["category"] == "thought_level")
    })
}
pub(super) fn model_config(value: &Value) -> Option<&Value> {
    config(value, "model")
}
pub(super) fn catalog(value: &Value) -> ModelCatalog {
    let mut catalog = driver_api::models::acp(value);
    if let Some(model_config) = config(value, "model") {
        catalog.default_model = model_config["currentValue"].as_str().map(Into::into);
        catalog.models = flatten_options(&model_config["options"])
            .into_iter()
            .filter_map(|row| {
                let id = row["value"].as_str()?;
                Some(ModelOption::new(id, row["name"].as_str().unwrap_or(id)))
            })
            .collect();
        if let Some(thought) = config(value, "thought_level")
            && let Some(model) = catalog
                .models
                .iter_mut()
                .find(|m| Some(&m.id) == catalog.default_model.as_ref())
        {
            model.thinking_options = driver_api::models::options(&thought["options"]);
            model.default_thinking = thought["currentValue"].as_str().map(Into::into);
        }
    }
    catalog
}
pub(super) async fn select(
    connection: &ConnectionTo<Agent>,
    state: &Value,
    session_id: &SessionId,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<Value, DriverError> {
    if model.is_none() && thinking.is_none() {
        return Ok(state.clone());
    }
    let mut updated = state.clone();
    if let Some(model_config) = config(state, "model") {
        if let Some(model) = model {
            let response = raw(
                connection,
                "session/set_config_option",
                json!({"sessionId": session_id,"configId":model_config["id"],"value":model}),
            )
            .await?;
            updated = super::session_state::merge(&updated, &response);
            if config(&updated, "model").and_then(|c| c["currentValue"].as_str()) != Some(model) {
                return Err(DriverError::Protocol("selection_unverifiable".into()));
            }
        }
        if let Some(thinking) = thinking {
            let thought = config(&updated, "thought_level").ok_or_else(|| {
                DriverError::CapabilityUnavailable("此模型没有思考强度选项".into())
            })?;
            if !flatten_options(&thought["options"])
                .iter()
                .any(|o| o["value"] == thinking)
            {
                return Err(DriverError::CapabilityUnavailable(
                    "此模型不支持所选思考强度".into(),
                ));
            }
            let result = raw(
                connection,
                "session/set_config_option",
                json!({"sessionId":session_id,"configId":thought["id"],"value":thinking}),
            )
            .await?;
            updated = super::session_state::merge(&updated, &result);
            if config(&updated, "thought_level").and_then(|c| c["currentValue"].as_str())
                != Some(thinking)
            {
                return Err(DriverError::Protocol("selection_unverifiable".into()));
            }
        }
    } else {
        let catalog = catalog(state);
        let model = model
            .or(catalog.default_model.as_deref())
            .ok_or_else(|| DriverError::CapabilityUnavailable("模型目录不可用".into()))?;
        let mut params = json!({"sessionId":session_id,"modelId":model});
        if let Some(effort) = thinking {
            params["reasoningEffort"] = json!(effort);
        }
        let result = raw(connection, "session/set_model", params).await?;
        updated = super::session_state::merge(&updated, &result);
        if let Some(selected) = result
            .pointer("/_meta/model/Ok")
            .or_else(|| result.pointer("/models/currentModelId"))
            .and_then(Value::as_str)
        {
            if selected != model {
                return Err(DriverError::Protocol("selection_unverifiable".into()));
            }
            updated["models"]["currentModelId"] = json!(selected);
        } else if result.pointer("/_meta/model/Err").is_some() {
            return Err(DriverError::Protocol("selection_unverifiable".into()));
        }
    }
    Ok(updated)
}

/// Native session initialization/history replay can exceed the metadata budget.
/// Keep notification handlers installed before these requests, and retain all
/// vendor extension fields in their responses.
pub(super) async fn session_request(
    connection: &ConnectionTo<Agent>,
    method: &'static str,
    params: Value,
    stderr: Option<&StderrTail>,
) -> Result<Value, DriverError> {
    let message = agent_client_protocol::UntypedMessage::new(method, params)
        .map_err(|_| DriverError::Protocol("Invalid session request".into()))?;
    let stderr = stderr.map(StderrTail::begin_request);
    tokio::time::timeout(
        std::time::Duration::from_mins(1),
        connection.send_request(message).block_task(),
    )
    .await
    .map_err(|_| {
        DriverError::Disconnected(
            if method == "session/load" {
                "会话恢复超时"
            } else {
                "会话初始化超时"
            }
            .into(),
        )
    })?
    .map_err(|e| map_session_error(&e, "Agent 无法创建或恢复会话", stderr.as_ref()))
}

#[cfg(test)]
mod tests {
    use crate::{AcpDriver, AcpLaunch};
    use driver_api::{AgentDriver, SessionRequest, SessionResumeCapability};
    #[tokio::test]
    #[ignore = "reads local ACP catalogs, creates only a metadata session without a model turn"]
    async fn local_model_catalog() {
        let binary = std::env::var("DOGCODES_ACP_BINARY").unwrap();
        let provider = std::env::var("DOGCODES_ACP_PROVIDER").unwrap();
        let args = if provider == "grok" {
            vec!["agent".into(), "stdio".into()]
        } else if provider == "codebuddy" {
            vec!["--acp".into()]
        } else {
            vec!["acp".into()]
        };
        let driver = AcpDriver::new(
            &provider,
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: binary.into(),
                args,
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let selected = std::env::var("DOGCODES_TEST_MODEL").ok();
        let catalog = tokio::time::timeout(
            std::time::Duration::from_secs(35),
            driver.model_catalog_for(selected.as_deref()),
        )
        .await
        .unwrap()
        .unwrap();
        if let Some(model) = selected {
            let entry = catalog.models.iter().find(|m| m.id == model).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let session = driver
                .create_selected_session(
                    SessionRequest {
                        history_mcp: None,
                        permission_mode: None,
                        working_directory: directory.path().to_string_lossy().into_owned(),
                        additional_directories: vec![],
                    },
                    Some(&model),
                    entry.default_thinking.as_deref(),
                )
                .await
                .unwrap();
            driver.shutdown(session).await.unwrap();
        }
        assert!(!catalog.models.is_empty());
        if let Ok(absent) = std::env::var("DOGCODES_TEST_ABSENT_MODEL") {
            assert!(!catalog.models.iter().any(|model| model.id == absent));
        }
        println!("{}", serde_json::to_string(&catalog).unwrap());
    }
}
