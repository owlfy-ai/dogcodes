//! Native model catalogs. Options are only selectable when the driver can apply them.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub models: Vec<ModelOption>,
    pub default_model: Option<String>,
}
/// Metadata already returned by a successful, task-free readiness handshake.
#[derive(Clone, Debug, Default)]
pub struct StartupCatalogs {
    pub models: Option<ModelCatalog>,
    pub permissions: Option<crate::PermissionCatalog>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub thinking_options: Vec<ThinkingOption>,
    pub default_thinking: Option<String>,
    /// A failure kind (see `failure::user_message`) when the native agent has already
    /// rejected this model for the signed-in account. It stays selectable: the account
    /// may have changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingOption {
    pub id: String,
    pub label: String,
}

impl ModelOption {
    #[must_use]
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            thinking_options: vec![],
            default_thinking: None,
            unavailable_reason: None,
        }
    }
}
#[must_use]
pub fn options(value: &Value) -> Vec<ThinkingOption> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row
                .get("id")
                .or_else(|| row.get("reasoningEffort"))
                .or_else(|| row.get("value"))?
                .as_str()?;
            if id.trim().is_empty() {
                return None;
            }
            Some(ThinkingOption {
                id: id.into(),
                label: row
                    .get("label")
                    .or_else(|| row.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .into(),
            })
        })
        .collect()
}
pub fn codex_page(value: &Value) -> ModelCatalog {
    let models: Vec<_> = value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["hidden"] != true)
        .filter_map(|row| {
            let id = row["model"].as_str().or_else(|| row["id"].as_str())?;
            let mut model = ModelOption::new(id, row["displayName"].as_str().unwrap_or(id));
            model.thinking_options = options(&row["supportedReasoningEfforts"]);
            model.default_thinking = row["defaultReasoningEffort"].as_str().map(Into::into);
            Some(model)
        })
        .collect();
    let default_model = value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["isDefault"] == true)
        .and_then(|r| r["model"].as_str().or_else(|| r["id"].as_str()))
        .map(Into::into);
    ModelCatalog {
        models,
        default_model,
    }
}
pub fn acp(value: &Value) -> ModelCatalog {
    let state = value
        .get("models")
        .or_else(|| value.pointer("/_meta/modelState"))
        .unwrap_or(value);
    let mut catalog = ModelCatalog {
        default_model: state["currentModelId"].as_str().map(Into::into),
        ..Default::default()
    };
    for row in state["availableModels"].as_array().into_iter().flatten() {
        let Some(id) = row["modelId"].as_str() else {
            continue;
        };
        let mut model = ModelOption::new(id, row["name"].as_str().unwrap_or(id));
        model.thinking_options = options(&row["_meta"]["reasoningEfforts"]);
        model.default_thinking = row["_meta"]["reasoningEffort"].as_str().map(Into::into);
        catalog.models.push(model);
    }
    catalog
}
const ANTIGRAVITY_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

#[must_use]
pub fn antigravity_model_parts(id: &str) -> (&str, Option<&str>) {
    id.rsplit_once('-')
        .filter(|(_, effort)| ANTIGRAVITY_EFFORTS.contains(effort))
        .map_or((id, None), |(base, effort)| (base, Some(effort)))
}

#[must_use]
pub fn antigravity(ids: Vec<String>) -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    for id in ids {
        let (base, effort) = antigravity_model_parts(&id);
        let index = catalog
            .models
            .iter()
            .position(|m| m.id == base)
            .unwrap_or_else(|| {
                let index = catalog.models.len();
                catalog.models.push(ModelOption::new(base, base));
                index
            });
        let model = &mut catalog.models[index];
        if let Some(effort) = effort
            && !model.thinking_options.iter().any(|e| e.id == effort)
        {
            model.thinking_options.push(ThinkingOption {
                id: effort.into(),
                label: effort.into(),
            });
        }
    }
    for model in &mut catalog.models {
        model
            .thinking_options
            .sort_by_key(|o| ANTIGRAVITY_EFFORTS.iter().position(|e| *e == o.id));
        // Keep the existing default when the catalog adds an opt-in XHigh tier.
        model.default_thinking = model
            .thinking_options
            .iter()
            .rfind(|o| o.id != "xhigh")
            .or_else(|| model.thinking_options.first())
            .map(|o| o.id.clone());
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preserves_native_per_model_efforts() {
        let c = acp(
            &json!({"models":{"currentModelId":"grok-4.6","availableModels":[{"modelId":"grok-4.6","name":"Grok 4.6","_meta":{"reasoningEfforts":[{"id":"high","label":"High Effort"}],"reasoningEffort":"high"}},{"modelId":"plain","name":"Plain"}]}}),
        );
        assert_eq!(c.models[0].thinking_options[0].id, "high");
        assert!(c.models[1].thinking_options.is_empty());
        assert_eq!(c.default_model.as_deref(), Some("grok-4.6"));
    }
    #[test]
    fn groups_only_verified_antigravity_effort_suffixes() {
        let c = antigravity(vec![
            "gemini-4-neon-xhigh".into(),
            "gemini-4-neon-high".into(),
            "gemini-4-neon-low".into(),
            "gemini-4-neon-medium".into(),
            "gemini-4-hydrogen-high".into(),
            "gemini-4-hydrogen-xhigh".into(),
            "gemini-4-carbon-high".into(),
            "claude-thinking".into(),
        ]);
        assert_eq!(
            c.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "gemini-4-neon",
                "gemini-4-hydrogen",
                "gemini-4-carbon",
                "claude-thinking",
            ]
        );
        let efforts = |index: usize| {
            c.models[index]
                .thinking_options
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(efforts(0), ["low", "medium", "high", "xhigh"]);
        assert_eq!(c.models[0].default_thinking.as_deref(), Some("high"));
        assert_eq!(efforts(1), ["high", "xhigh"]);
        assert_eq!(c.models[1].default_thinking.as_deref(), Some("high"));
        assert_eq!(efforts(2), ["high"]);
        assert!(efforts(3).is_empty());
    }
}
