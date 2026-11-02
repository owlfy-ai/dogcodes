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
            || (category == "thought_level" && c["id"] == "effortLevel")
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
                let mut model = ModelOption::new(id, row["name"].as_str().unwrap_or(id));
                let meta = &row["_meta"]["kiro"];
                if id != "auto" && meta["hasEffort"] != false {
                    model.thinking_options = kiro_options(&meta["effortLevels"]);
                    model.default_thinking = meta["defaultEffortLevel"]
                        .as_str()
                        .filter(|id| model.thinking_options.iter().any(|o| o.id == *id))
                        .map(Into::into);
                }
                Some(model)
            })
            .collect();
        if let Some(thought) = config(value, "thought_level")
            && let Some(model) = catalog
                .models
                .iter_mut()
                .find(|m| Some(&m.id) == catalog.default_model.as_ref())
        {
            model.thinking_options = driver_api::models::options(&thought["options"]);
            model.default_thinking = thought["currentValue"]
                .as_str()
                .filter(|id| model.thinking_options.iter().any(|o| o.id == *id))
                .map(Into::into);
            // Kiro's automatic routing never accepts an effort selection.
            if model.id == "auto" && thought["id"] == "effortLevel" {
                model.thinking_options.clear();
                model.default_thinking = None;
            }
        }
    }
    catalog
}
fn kiro_options(value: &Value) -> Vec<driver_api::ThinkingOption> {
    let normalized: Vec<_> = value
        .as_array()
        .into_iter()
        .flatten()
        .map(|option| {
            option
                .as_str()
                .map_or_else(|| option.clone(), |id| json!({"value":id,"name":id}))
        })
        .collect();
    let mut options = driver_api::models::options(&json!(normalized));
    let mut seen = std::collections::HashSet::new();
    options.retain(|o| seen.insert(o.id.clone()));
    options
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
            let selected_catalog = catalog(&updated);
            if !selected_catalog.models.iter().any(|m| {
                Some(&m.id) == selected_catalog.default_model.as_ref()
                    && m.thinking_options.iter().any(|o| o.id == thinking)
            }) {
                return Err(DriverError::CapabilityUnavailable(
                    "此模型不支持所选思考强度".into(),
                ));
            }
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
    use super::*;
    use crate::{AcpDriver, AcpLaunch};
    use driver_api::{AgentDriver, SessionRequest, SessionResumeCapability};
    fn kiro(model: &str) -> Value {
        json!({"configOptions":[{"id":"model","currentValue":model,"options":[
            {"value":"adjustable","name":"Adjustable","_meta":{"kiro":{"hasEffort":true,"effortLevels":["low","high","max","low"],"defaultEffortLevel":"max"}}},
            {"value":"fixed","_meta":{"kiro":{"hasEffort":false,"effortLevels":["high"]}}},
            {"value":"auto"}
        ]},{"id":"effortLevel","currentValue":"high","options":[{"value":"low"},{"value":"high"}]}]})
    }
    #[test]
    fn kiro_effort_is_model_specific_and_current_options_are_authoritative() {
        let state = kiro("adjustable");
        let c = catalog(&state);
        assert_eq!(
            c.models[0]
                .thinking_options
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            ["low", "high"]
        );
        assert_eq!(c.models[0].default_thinking.as_deref(), Some("high"));
        assert!(c.models[1].thinking_options.is_empty());
        assert!(c.models[2].thinking_options.is_empty());
        let mut template = state;
        template["configOptions"].as_array_mut().unwrap().pop();
        let c = catalog(&template);
        assert_eq!(c.models[0].thinking_options.len(), 3);
        assert_eq!(c.models[0].default_thinking.as_deref(), Some("max"));
        assert!(catalog(&kiro("auto")).models[2].thinking_options.is_empty());
    }
    #[tokio::test]
    async fn kiro_selection_refreshes_effort_and_requires_native_confirmation() {
        use crate::{Agent, ByteStreams, Client, Responder, on_receive_request};
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        let (client, agent) = tokio::io::duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client);
        let (ar, aw) = tokio::io::split(agent);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        let server = tokio::spawn(async move {
            Agent
                .builder()
                .on_receive_request(
                    async move |request: agent_client_protocol::UntypedMessage,
                                responder: Responder<Value>,
                                _connection| {
                        let params = request.params;
                        observed.lock().unwrap().push(params.clone());
                        let mut next = kiro(
                            params["value"]
                                .as_str()
                                .filter(|_| params["configId"] == "model")
                                .unwrap_or("adjustable"),
                        );
                        if params["value"] == "low" {
                            next["configOptions"][1]["currentValue"] = json!("low");
                        }
                        if params["value"] == "high" {
                            next["configOptions"][1]["currentValue"] = json!("low");
                        }
                        if params["value"] == "fixed" {
                            next["configOptions"].as_array_mut().unwrap().pop();
                        }
                        responder.respond(next)
                    },
                    on_receive_request!(),
                )
                .connect_with(
                    ByteStreams::new(aw.compat_write(), ar.compat()),
                    async |_| std::future::pending::<Result<(), crate::AcpError>>().await,
                )
                .await
        });
        Client
            .builder()
            .connect_with(
                ByteStreams::new(cw.compat_write(), cr.compat()),
                async |connection| {
                    let id = SessionId::new("s");
                    let state = select(
                        &connection,
                        &kiro("auto"),
                        &id,
                        Some("adjustable"),
                        Some("low"),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        catalog(&state).models[0].default_thinking.as_deref(),
                        Some("low")
                    );
                    assert!(matches!(
                        select(&connection, &state, &id, Some("fixed"), Some("high")).await,
                        Err(DriverError::CapabilityUnavailable(_))
                    ));
                    assert!(matches!(
                        select(&connection, &state, &id, None, Some("max")).await,
                        Err(DriverError::CapabilityUnavailable(_))
                    ));
                    // The fixture intentionally refuses the high setting; a successful RPC is insufficient.
                    let mut low = kiro("adjustable");
                    low["configOptions"][1]["currentValue"] = json!("low");
                    assert!(matches!(
                        select(&connection, &low, &id, None, Some("high")).await,
                        Err(DriverError::Protocol(_))
                    ));
                    let requested = calls.lock().unwrap();
                    assert_eq!(requested[0]["configId"], "model");
                    assert_eq!(requested[1]["configId"], "effortLevel");
                    Ok::<_, crate::AcpError>(())
                },
            )
            .await
            .unwrap();
        server.abort();
    }
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
                        app_mcps: vec![],
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
