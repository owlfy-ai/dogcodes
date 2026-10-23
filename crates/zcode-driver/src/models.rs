use super::{Connection, DriverError, SessionHandle, Value, ZcodeDriver, json};
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
impl ZcodeDriver {
    pub(super) async fn select_model(
        &self,
        handle: &SessionHandle,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<(), DriverError> {
        if model.is_none() && thinking.is_none() {
            return Ok(());
        }
        let mut guard = self.ensure_connection().await?;
        select_native(
            guard.as_mut().expect("connection ensured"),
            &handle.native_session_id,
            model,
            thinking,
        )
        .await
    }
}

pub(super) async fn select_native(
    connection: &mut Connection,
    session_id: &str,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<(), DriverError> {
    if let Some(model) = model {
        let mut reference: Value = serde_json::from_str(model)
            .map_err(|_| DriverError::Protocol("invalid ZCode model reference".into()))?;
        if reference.get("options").is_none() {
            // Previously saved selections have no registry reasoning option.
            // Recover the vendor's default, never invent a level.
            let snapshot = connection
                .request("session/read", json!({"sessionId":session_id}), None)
                .await?;
            if let Some(row) = snapshot
                .pointer("/settings/model/available")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|row| {
                    row["ref"]["providerId"] == reference["providerId"]
                        && row["ref"]["modelId"] == reference["modelId"]
                })
                && row["reasoning"].get("enabled").is_none()
                && let Some(level) = row["reasoning"]["defaultLevel"].as_str()
            {
                reference["options"] = json!({"reasoningLevel":level});
            }
        }
        if reference.get("options").is_some()
            && let Some(level) = thinking
        {
            reference["options"]["reasoningLevel"] = json!(level);
        }
        connection
            .request(
                "session/setModel",
                json!({"sessionId":session_id,"model":reference}),
                None,
            )
            .await?;
    }
    if let Some(thought) = thinking {
        connection
            .request(
                "session/setThoughtLevel",
                json!({"sessionId":session_id,"thoughtLevel":thought}),
                None,
            )
            .await?;
    }
    Ok(())
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
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
            })
            .await
            .unwrap();
        let selected = driver
            .select_model(&handle, Some(&model.id), model.default_thinking.as_deref())
            .await;
        // Also verify selections persisted by pre-upgrade dogcodes.
        let mut legacy: Value = serde_json::from_str(&model.id).unwrap();
        legacy.as_object_mut().unwrap().remove("options");
        let restored = driver
            .select_model(&handle, Some(&legacy.to_string()), None)
            .await;
        driver.shutdown(handle).await.unwrap();
        selected.unwrap();
        restored.unwrap();
        println!("model catalog and selection passed without a model turn");
    }
}
