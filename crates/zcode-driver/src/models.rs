use super::{Connection, DriverError, Value, json};
#[cfg(test)]
use super::{SessionHandle, ZcodeDriver};
use driver_api::{ModelCatalog, ModelOption};

// Preserve ZCode's structured reference including optional variants losslessly.
pub(super) fn catalog(value: &Value) -> ModelCatalog {
    let settings = &value["settings"];
    let mut catalog = ModelCatalog {
        default_model: settings
            .pointer("/model/current")
            .filter(|v| v.is_object())
            .map(Value::to_string),
        ..Default::default()
    };
    for row in settings["model"]["available"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if row["disabledReason"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        {
            continue;
        }
        let mut reference = row["ref"].clone();
        let Some(id) = reference["modelId"].as_str().map(str::to_owned) else {
            continue;
        };
        if reference["providerId"].as_str().is_none() {
            continue;
        }
        // New registry options omit `enabled` and require the reasoning level
        // inside the model reference before session/setModel is accepted.
        if row["reasoning"].get("enabled").is_none()
            && let Some(level) = row["reasoning"]["defaultLevel"].as_str()
        {
            reference["options"] = json!({"reasoningLevel":level});
        }
        let mut model =
            ModelOption::new(&reference.to_string(), row["label"].as_str().unwrap_or(&id));
        if row["reasoning"]["enabled"] != false {
            model.thinking_options = driver_api::models::options(&row["reasoning"]["levels"]);
            model.default_thinking = row["reasoning"]["defaultLevel"].as_str().map(Into::into);
        }
        catalog.models.push(model);
    }
    catalog
}
#[cfg(test)]
impl ZcodeDriver {
    pub(super) async fn select_model(
        &self,
        handle: &SessionHandle,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<(), DriverError> {
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
            .ok_or_else(super::scopes::stale)?;
        let _admission = state.admission.lock().await;
        state.validate(handle)?;
        let mut attempt = self.attempt(&state, handle);
        let mut guard = self.connection.lock().await;
        let connection = guard.as_mut().ok_or_else(super::scopes::stale)?;
        let result = transaction(
            connection,
            &handle.native_session_id,
            &state.workspace,
            model,
            thinking,
        )
        .await;
        if result.is_ok()
            || result
                .as_ref()
                .is_err_and(|e| driver_api::failure::kind(e) == "selection_rejected")
        {
            attempt.committed = true;
        }
        result
    }
}

struct Selection {
    model: Option<Value>,
    thinking: Option<String>,
}
fn target(
    before: &Value,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<Selection, DriverError> {
    let rejected = || DriverError::Protocol("selection_rejected".into());
    let mut reference = model
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|_| rejected())?;
    if let Some(value) = &reference
        && (value["providerId"].as_str().is_none_or(str::is_empty)
            || value["modelId"].as_str().is_none_or(str::is_empty))
    {
        return Err(rejected());
    }
    let separate = before.pointer("/settings/thoughtLevel/enabled") == Some(&Value::Bool(true));
    if let Some(thinking) = thinking {
        if separate {
            return Ok(Selection {
                model: reference,
                thinking: Some(thinking.into()),
            });
        }
        if reference.is_none() {
            reference = before.pointer("/settings/model/current").cloned();
        }
        let reference = reference
            .as_mut()
            .filter(|r| r["options"].is_object())
            .ok_or_else(rejected)?;
        reference["options"]["reasoningLevel"] = json!(thinking);
    }
    Ok(Selection {
        model: reference,
        thinking: None,
    })
}
async fn apply(
    connection: &mut Connection,
    id: &str,
    selection: &Selection,
) -> Result<(), DriverError> {
    if let Some(model) = &selection.model {
        connection
            .request(
                "session/setModel",
                json!({"sessionId":id,"model":model,"persistAsWorkspaceLastUsed":false}),
                None,
            )
            .await?;
    }
    if let Some(thinking) = &selection.thinking {
        connection
            .request(
                "session/setThoughtLevel",
                json!({"sessionId":id,"thoughtLevel":thinking,"persistAsWorkspaceLastUsed":false}),
                None,
            )
            .await?;
    }
    Ok(())
}
fn matches(snapshot: &Value, selection: &Selection) -> bool {
    let current = &snapshot["settings"]["model"]["current"];
    selection.model.as_ref().is_none_or(|model| {
        model
            .as_object()
            .is_some_and(|fields| fields.iter().all(|(k, v)| current.get(k) == Some(v)))
    }) && selection.thinking.as_ref().is_none_or(|thinking| {
        snapshot["settings"]["thoughtLevel"]["current"].as_str() == Some(thinking)
    })
}
pub(super) async fn transaction(
    connection: &mut Connection,
    id: &str,
    workspace: &str,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<(), DriverError> {
    if model.is_none() && thinking.is_none() {
        return Ok(());
    }
    let before = connection
        .request("session/read", json!({"sessionId":id}), None)
        .await?;
    super::resume::validate(&before, id, workspace)?;
    let selection = target(&before, model, thinking)?;
    let applied = async {
        apply(connection, id, &selection).await?;
        let after = connection
            .request("session/read", json!({"sessionId":id}), None)
            .await?;
        super::resume::validate(&after, id, workspace)?;
        if matches(&after, &selection) {
            Ok(())
        } else {
            Err(DriverError::Protocol(
                "ZCode did not apply the requested selection".into(),
            ))
        }
    }
    .await;
    if applied.is_ok() {
        return Ok(());
    }
    if let Some(model) = before
        .pointer("/settings/model/current")
        .filter(|m| m.is_object())
    {
        let old = Selection {
            model: Some(model.clone()),
            thinking: before
                .pointer("/settings/thoughtLevel/current")
                .and_then(Value::as_str)
                .map(Into::into),
        };
        let restored = async {
            apply(connection, id, &old).await?;
            let after = connection
                .request("session/read", json!({"sessionId":id}), None)
                .await?;
            super::resume::validate(&after, id, workspace)?;
            Ok::<bool, DriverError>(
                after["settings"]["model"]["current"] == before["settings"]["model"]["current"]
                    && after["settings"]["thoughtLevel"]["current"]
                        == before["settings"]["thoughtLevel"]["current"],
            )
        }
        .await;
        if matches!(restored, Ok(true)) {
            return Err(DriverError::Protocol("selection_rejected".into()));
        }
    }
    Err(DriverError::Protocol(
        "ZCode selection and rollback remain unconfirmed".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZcodeLaunch;
    use driver_api::{AgentDriver, SessionResumeCapability};
    #[test]
    fn registry_catalog_preserves_required_reasoning_selection() {
        let model = json!({"providerId":"test","modelId":"model"});
        let value = json!({"settings":{"model":{"current":{"providerId":"test","modelId":"model","options":{"reasoningLevel":"high"}},"available":[
            {"ref":model,"reasoning":{"levels":[{"value":"low","label":"low"},{"value":"high","label":"high"}],"defaultLevel":"high"}}
        ]}}});
        let parsed = catalog(&value);
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].default_thinking.as_deref(), Some("high"));
        assert_eq!(
            parsed.default_model.as_deref(),
            Some(parsed.models[0].id.as_str())
        );
        let legacy = catalog(
            &json!({"settings":{"model":{"available":[{"ref":model,"reasoning":{"enabled":true,"defaultLevel":"high","levels":[]}}]}}}),
        );
        assert_eq!(legacy.models[0].id, model.to_string());
    }
    #[tokio::test]
    #[ignore = "reads the locally installed ZCode model catalog, no model turn"]
    async fn local_model_catalog() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            ZcodeLaunch {
                app_binary: "/Applications/ZCode.app/Contents/MacOS/ZCode".into(),
                cli_script: "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs".into(),
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let catalog = driver.model_catalog().await.unwrap();
        assert!(!catalog.models.is_empty());
        let workspace = tempfile::tempdir().unwrap();
        let model = &catalog.models[0];
        let handle = driver
            .create_session(driver_api::SessionRequest {
                working_directory: workspace.path().to_string_lossy().into_owned(),
                app_mcps: vec![],
                permission_mode: None,
                additional_directories: vec![],
            })
            .await
            .unwrap();
        let selected = driver
            .select_model(&handle, Some(&model.id), model.default_thinking.as_deref())
            .await;
        driver.shutdown(handle).await.unwrap();
        selected.unwrap();
        println!("model catalog and selection passed without a model turn");
    }
}
