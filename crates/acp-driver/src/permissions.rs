use super::{Agent, ConnectionTo, DriverError, SessionId, models};
use driver_api::{PermissionCatalog, permissions};
use serde_json::{Value, json};
pub(super) fn catalog(state: &Value) -> PermissionCatalog {
    if state.pointer("/_meta/x.ai~1sessionDetail").is_some() {
        return permissions::grok();
    }
    if let Some(mode) = mode_config(state) {
        let modes = mode["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let id = row["value"].as_str()?;
                Some(driver_api::permissions::PermissionMode {
                    id: id.into(),
                    label: row["name"].as_str().unwrap_or(id).into(),
                    description: row["description"]
                        .as_str()
                        .unwrap_or("Agent 原生权限模式")
                        .into(),
                    dangerous: matches!(id, "bypassPermissions" | "fullAccess")
                        || (mode["id"] == "autopilot" && id == "on"),
                })
            })
            .collect();
        return PermissionCatalog {
            modes,
            default_mode_id: if mode["id"] == "autopilot" {
                Some("off".into())
            } else {
                mode["currentValue"].as_str().map(Into::into)
            },
        };
    }
    let native = permissions::acp(state);
    if native.modes.is_empty() {
        PermissionCatalog::new(
            "configured",
            &[(
                "configured",
                "原生权限规则",
                "此 Agent 未报告可切换的权限模式；审批请求仍会显示在对话中。",
                false,
            )],
        )
    } else {
        native
    }
}
pub(super) async fn select(
    connection: &ConnectionTo<Agent>,
    state: &Value,
    id: &SessionId,
    mode: Option<&str>,
) -> Result<Value, DriverError> {
    let mut updated = state.clone();
    if let Some(config) = mode_config(state) {
        // Kiro's modes select workflows; autopilot is the actual permission
        // control. Default to supervised, independently of its CLI default.
        let mode = mode.or_else(|| (config["id"] == "autopilot").then_some("off"));
        if let Some(mode) = mode {
            catalog(state).validate(Some(mode))?;
            let confirmed = models::raw(
                connection,
                "session/set_config_option",
                json!({"sessionId":id,"configId":config["id"],"value":mode}),
            )
            .await?;
            updated = super::session_state::merge(&updated, &confirmed);
            if mode_config(&updated).and_then(|v| v["currentValue"].as_str()) != Some(mode) {
                return Err(DriverError::Protocol("Agent 未确认权限模式".into()));
            }
        }
        return Ok(updated);
    }
    // Cursor's agent/plan/ask are work modes, not approval policies.
    if mode.is_none()
        && state["modes"]["availableModes"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == "agent"))
    {
        return Ok(updated);
    }
    let mode = mode.unwrap_or_else(|| {
        if state.pointer("/_meta/x.ai~1sessionDetail").is_some() {
            "ask"
        } else if state["modes"]["availableModes"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["id"] == "default"))
        {
            "default"
        } else {
            "configured"
        }
    });
    catalog(state).validate(Some(mode))?;
    if state.pointer("/_meta/x.ai~1sessionDetail").is_some() {
        // Grok accepts these flags on new/load; they are added before either request.
        return Ok(updated);
    }
    if mode != "configured" {
        models::raw(
            connection,
            "session/set_mode",
            json!({"sessionId":id,"modeId":mode}),
        )
        .await?;
        updated["modes"]["currentModeId"] = json!(mode);
    }
    Ok(updated)
}
pub(super) fn meta(params: &mut Value, mode: Option<&str>) {
    if let Some(mode) = mode.filter(|m| ["ask", "auto", "always-approve"].contains(m)) {
        params["_meta"] = json!({"yoloMode":mode=="always-approve","autoMode":mode=="auto"});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kiro_uses_supervised_autopilot_policy_instead_of_workflow_modes() {
        let state = json!({"configOptions":[
            {"id":"mode","type":"select","currentValue":"vibe","options":[{"value":"vibe","name":"Default"}]},
            {"id":"autopilot","type":"select","currentValue":"on","options":[{"value":"on","name":"Autopilot"},{"value":"off","name":"Supervised"}]}
        ]});
        let catalog = catalog(&state);
        assert_eq!(catalog.default_mode_id.as_deref(), Some("off"));
        assert!(catalog.modes[0].dangerous);
        assert!(!catalog.modes[1].dangerous);
        assert!(catalog.validate(Some("vibe")).is_err());
        assert_eq!(catalog.analysis_mode().unwrap().as_deref(), Some("off"));
        assert_eq!(catalog.review_mode().as_deref(), Some("on"));
    }
    #[test]
    fn grok_modes_are_not_confused_with_reasoning_modes() {
        let state = json!({"_meta":{"x.ai/sessionDetail":{}},"configOptions":[{"id":"reasoning_effort","category":"thought_level"}]});
        let modes = catalog(&state);
        assert_eq!(modes.default_mode_id.as_deref(), Some("ask"));
        assert!(modes.validate(Some("xhigh")).is_err());
        let mut p = json!({});
        meta(&mut p, Some("always-approve"));
        assert_eq!(p["_meta"], json!({"yoloMode":true,"autoMode":false}));
        meta(&mut p, Some("ask"));
        assert_eq!(p["_meta"], json!({"yoloMode":false,"autoMode":false}));
    }
    #[test]
    fn kimi_labels_preserve_native_ids_and_mark_auto_as_never_ask() {
        let mut modes = catalog(&json!({"modes":{"currentModeId":"yolo","availableModes":[
            {"id":"default"},{"id":"plan"},{"id":"auto"},{"id":"yolo"}
        ]}}));
        kimi_labels(&mut modes);
        assert_eq!(modes.default_mode_id.as_deref(), Some("yolo"));
        assert_eq!(
            modes
                .modes
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["default", "yolo", "plan", "auto"]
        );
        assert_eq!(modes.modes[1].label, "按需询问");
        assert!(!modes.modes[1].dangerous);
        assert_eq!(modes.modes[3].label, "从不询问");
        assert!(modes.modes[3].dangerous);
        assert_eq!(modes.review_mode().as_deref(), Some("auto"));
    }
    #[test]
    fn codebuddy_config_modes_preserve_native_ids_and_default() {
        let state = json!({"configOptions":[{"id":"mode","type":"select","currentValue":"default","options":[
            {"value":"default","name":"Default"},{"value":"fullAccess","name":"Full Access"}
        ]}]});
        let c = catalog(&state);
        assert_eq!(c.default_mode_id.as_deref(), Some("default"));
        assert!(c.modes[1].dangerous);
        assert!(c.validate(Some("plan")).is_err());
    }
    #[test]
    fn missing_native_modes_only_offers_configured_rules() {
        let modes = catalog(&json!({}));
        assert_eq!(modes.modes.len(), 1);
        assert!(modes.validate(Some("yolo")).is_err());
    }
}

// Kimi 0.42's ACP labels lag behind its CLI permission semantics. Keep the
// native IDs: yolo means Ask When Needed; auto means Never Ask.
pub(super) fn kimi_labels(catalog: &mut PermissionCatalog) {
    for mode in &mut catalog.modes {
        let (label, description, dangerous) = match mode.id.as_str() {
            "default" => (
                "始终询问",
                "仅自动读取；其他操作均需先征得你的批准。",
                false,
            ),
            "yolo" => (
                "按需询问",
                "常规编辑和命令自动执行；高风险操作、提问和计划仍会询问。",
                false,
            ),
            "auto" => ("从不询问", "不中断任务；所有操作和决策均自动进行。", true),
            "plan" => ("计划模式", "先制定计划；需要批准的操作仍会询问。", false),
            _ => continue,
        };
        mode.label = label.into();
        mode.description = description.into();
        mode.dangerous = dangerous;
    }
    catalog.modes.sort_by_key(|mode| match mode.id.as_str() {
        "default" => 0,
        "yolo" => 1,
        "plan" => 2,
        "auto" => 3,
        _ => 4,
    });
}

pub(super) fn cursor_catalog() -> PermissionCatalog {
    PermissionCatalog::new(
        "default",
        &[
            (
                "default",
                "操作前确认",
                "按 Cursor 权限规则执行；需要审批的操作在此确认。",
                false,
            ),
            (
                "full-access",
                "完全访问",
                "自动批准操作；明确禁止的操作和团队限制仍然生效。",
                true,
            ),
        ],
    )
}

pub(super) fn cursor_launch_args(args: &mut Vec<String>, force: bool) {
    args.retain(|arg| !matches!(arg.as_str(), "--force" | "-f" | "--yolo" | "--auto-review"));
    if !args.iter().any(|arg| arg == "--disable-auto-update") {
        args.insert(0, "--disable-auto-update".into());
    }
    if force {
        args.insert(0, "--force".into());
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    #[test]
    fn switching_back_removes_all_approval_overrides_without_changing_acp() {
        let mut args = vec!["--yolo".into(), "acp".into(), "--auto-review".into()];
        cursor_launch_args(&mut args, true);
        assert_eq!(args, vec!["--force", "--disable-auto-update", "acp"]);
        cursor_launch_args(&mut args, false);
        assert_eq!(args, vec!["--disable-auto-update", "acp"]);
    }
    #[test]
    fn approval_catalog_does_not_offer_work_modes_or_unverified_auto_review() {
        let catalog = cursor_catalog();
        assert_eq!(catalog.default_mode_id.as_deref(), Some("default"));
        assert!(catalog.validate(Some("full-access")).is_ok());
        for mode in ["agent", "plan", "ask", "auto-review"] {
            assert!(catalog.validate(Some(mode)).is_err());
        }
    }
}

fn mode_config(state: &Value) -> Option<&Value> {
    let options = state["configOptions"].as_array()?;
    options
        .iter()
        .find(|option| option["id"] == "autopilot" && option["type"] == "select")
        .or_else(|| {
            options
                .iter()
                .find(|option| option["id"] == "mode" && option["type"] == "select")
        })
}
