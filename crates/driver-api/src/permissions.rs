//! Native permission policies. IDs are opaque outside the owning driver.
use crate::DriverError;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionCatalog {
    pub modes: Vec<PermissionMode>,
    pub default_mode_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionMode {
    pub id: String,
    pub label: String,
    pub description: String,
    pub dangerous: bool,
}
impl PermissionCatalog {
    /// Reuse only modes advertised by this installation; never invent a mode
    /// or inherit a known bypass setting for a historical analysis session.
    pub fn analysis_mode(&self) -> Result<Option<String>, DriverError> {
        let safe = |id: &str| self.modes.iter().find(|m| m.id == id && !m.dangerous);
        for id in [
            "read-only",
            "plan",
            "ask",
            "default",
            "build",
            "desktop-approvals",
            "configured",
        ] {
            if let Some(mode) = safe(id) {
                return Ok(Some(mode.id.clone()));
            }
        }
        if self.modes.is_empty() {
            return Ok(None);
        }
        self.default_mode_id
            .as_deref()
            .and_then(safe)
            .or_else(|| self.modes.iter().find(|m| !m.dangerous))
            .map(|m| Some(m.id.clone()))
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable("Agent 未提供可用于审查的普通权限模式".into())
            })
    }
    /// Review runs explicitly use full native permissions; the prompt defines the no-edit intent.
    #[must_use]
    pub fn review_mode(&self) -> Option<String> {
        for id in [
            "full-access",
            "fullAccess",
            "bypassPermissions",
            "always-approve",
            "yolo",
            "dangerously-skip-permissions",
            "auto-approve",
        ] {
            if let Some(mode) = self
                .modes
                .iter()
                .find(|mode| mode.id == id && mode.dangerous)
            {
                return Some(mode.id.clone());
            }
        }
        if let Some(mode) = self.modes.iter().find(|mode| mode.dangerous) {
            return Some(mode.id.clone());
        }
        // Some agents expose work modes only. Keep their normal execution mode;
        // the review runner answers any vendor approval requests for this run.
        for id in [
            "agent",
            "build",
            "default",
            "configured",
            "desktop-approvals",
        ] {
            if self.modes.iter().any(|mode| mode.id == id) {
                return Some(id.into());
            }
        }
        self.default_mode_id.clone()
    }
    #[must_use]
    pub fn new(default: &str, rows: &[(&str, &str, &str, bool)]) -> Self {
        Self {
            default_mode_id: Some(default.into()),
            modes: rows
                .iter()
                .map(|(id, label, description, dangerous)| PermissionMode {
                    id: (*id).into(),
                    label: (*label).into(),
                    description: (*description).into(),
                    dangerous: *dangerous,
                })
                .collect(),
        }
    }
    pub fn validate(&self, id: Option<&str>) -> Result<(), DriverError> {
        if id.is_some_and(|id| !self.modes.iter().any(|m| m.id == id)) {
            return Err(DriverError::CapabilityUnavailable(
                "当前 Agent 不支持所选权限策略".into(),
            ));
        }
        Ok(())
    }
}
#[must_use]
pub fn codex() -> PermissionCatalog {
    PermissionCatalog::new(
        "default",
        &[
            (
                "read-only",
                "只读",
                "只读沙箱；需要扩大权限时请求审批。",
                false,
            ),
            (
                "default",
                "默认",
                "工作区写入沙箱；受保护的操作请求审批。",
                false,
            ),
            ("full-access", "完全访问", "关闭沙箱并跳过审批。", true),
        ],
    )
}
#[must_use]
pub fn claude() -> PermissionCatalog {
    PermissionCatalog::new(
        "default",
        &[
            (
                "plan",
                "计划模式",
                "探索并制定计划；使用 Claude 原生计划模式。",
                false,
            ),
            (
                "default",
                "默认",
                "编辑文件和其他受保护操作前请求审批。",
                false,
            ),
            (
                "acceptEdits",
                "接受编辑",
                "允许文件编辑；其他受保护操作仍需审批。",
                false,
            ),
            (
                "bypassPermissions",
                "跳过权限检查",
                "跳过 Claude Code 工具权限检查。",
                true,
            ),
        ],
    )
}
#[must_use]
pub fn antigravity() -> PermissionCatalog {
    PermissionCatalog::new(
        "desktop-approvals",
        &[
            (
                "desktop-approvals",
                "桌面审批",
                "每次工具执行前在此审批；审批桥不可用时拒绝执行。",
                false,
            ),
            (
                "dangerously-skip-permissions",
                "跳过权限检查",
                "自动允许 Antigravity 工具操作。",
                true,
            ),
        ],
    )
}
pub fn acp(state: &serde_json::Value) -> PermissionCatalog {
    let modes = &state["modes"];
    let mut catalog = PermissionCatalog {
        default_mode_id: modes["currentModeId"].as_str().map(Into::into),
        modes: vec![],
    };
    if let Some(rows) = modes["availableModes"].as_array() {
        for row in rows.iter().take(32) {
            if let Some(id) = row["id"].as_str() {
                catalog.modes.push(PermissionMode {
                    id: id.into(),
                    label: row["name"].as_str().unwrap_or(id).into(),
                    description: row["description"]
                        .as_str()
                        .unwrap_or("Agent 原生会话模式")
                        .into(),
                    dangerous: matches!(
                        id,
                        "yolo" | "bypassPermissions" | "full-access" | "auto-approve"
                    ),
                });
            }
        }
    }
    catalog
}
#[must_use]
pub fn grok() -> PermissionCatalog {
    PermissionCatalog::new(
        "ask",
        &[
            ("ask", "询问", "受保护的工具操作前请求审批。", false),
            (
                "auto",
                "自动判断",
                "由 Grok 判断哪些工具操作可自动执行。",
                false,
            ),
            (
                "always-approve",
                "始终允许",
                "允许所有工具操作，无需审批。",
                true,
            ),
        ],
    )
}
#[must_use]
pub fn zcode() -> PermissionCatalog {
    PermissionCatalog::new(
        "build",
        &[
            ("build", "变更前确认", "改文件前先问我。", false),
            ("edit", "自动编辑", "自动编辑文件。", false),
            ("plan", "计划模式", "编辑前先出计划。", false),
            ("yolo", "完全访问", "减少确认次数。", true),
        ],
    )
}

#[must_use]
pub fn claude_with_auto(supported: bool) -> PermissionCatalog {
    let mut catalog = claude();
    if supported {
        catalog.modes.insert(
            3,
            PermissionMode {
                id: "auto".into(),
                label: "自动判断".into(),
                description: "由 Claude 自动分类权限请求。".into(),
                dangerous: false,
            },
        );
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_uses_full_permissions_without_changing_restricted_analysis() {
        for (catalog, id) in [
            (codex(), "full-access"),
            (claude(), "bypassPermissions"),
            (grok(), "always-approve"),
            (zcode(), "yolo"),
            (antigravity(), "dangerously-skip-permissions"),
        ] {
            assert_eq!(catalog.review_mode().as_deref(), Some(id));
            assert_ne!(catalog.analysis_mode().unwrap().as_deref(), Some(id));
        }
        assert_eq!(
            PermissionCatalog::new(
                "ask",
                &[("ask", "ask", "", false), ("agent", "agent", "", false)]
            )
            .review_mode()
            .as_deref(),
            Some("agent")
        );
    }
    #[test]
    fn auto_is_offered_only_when_native_claude_supports_it() {
        assert!(claude_with_auto(false).validate(Some("auto")).is_err());
        assert!(claude_with_auto(true).validate(Some("auto")).is_ok());
        assert!(codex().validate(Some("bypassPermissions")).is_err());
    }
    #[test]
    fn acp_preserves_native_ids_and_current_mode() {
        let catalog = acp(
            &serde_json::json!({"modes":{"currentModeId":"plan","availableModes":[{"id":"plan","name":"Plan"},{"id":"yolo","name":"YOLO"}]}}),
        );
        assert_eq!(catalog.default_mode_id.as_deref(), Some("plan"));
        assert_eq!(catalog.modes[1].id, "yolo");
        assert!(catalog.modes[1].dangerous);
    }
}
