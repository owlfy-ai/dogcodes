//! Native permission policies. IDs are opaque outside the owning driver.
use crate::DriverError;
use serde::{Deserialize, Serialize};

/// Native IDs that disable approvals entirely. Agents that report modes
/// dynamically are classified with this one list.
pub const DANGEROUS_MODE_IDS: &[&str] = &[
    "full-access",
    "fullAccess",
    "danger-full-access",
    "bypassPermissions",
    "always-approve",
    "yolo",
    "dangerously-skip-permissions",
    "auto-approve",
    "allow-all",
];

#[must_use]
pub fn is_dangerous_mode_id(id: &str) -> bool {
    DANGEROUS_MODE_IDS.contains(&id)
}
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
                DriverError::CapabilityUnavailable("backendText.noReviewPermissionMode".into())
            })
    }
    /// The mode that grants the agent the most, judged from the catalog: the
    /// first known full-access ID, else any mode the agent marks dangerous.
    /// `None` when the catalog offers nothing beyond its normal modes.
    #[must_use]
    pub fn most_permissive_mode(&self) -> Option<String> {
        DANGEROUS_MODE_IDS
            .iter()
            .find_map(|id| {
                self.modes
                    .iter()
                    .find(|mode| mode.id == *id && mode.dangerous)
            })
            .or_else(|| self.modes.iter().find(|mode| mode.dangerous))
            .map(|mode| mode.id.clone())
    }
    /// New sessions default to the most permissive mode the catalog offers;
    /// catalogs without one keep their own default.
    #[must_use]
    pub fn preferring_most_permissive(mut self) -> Self {
        if let Some(mode) = self.most_permissive_mode() {
            self.default_mode_id = Some(mode);
        }
        self
    }
    /// Review runs explicitly use full native permissions; the prompt defines the no-edit intent.
    #[must_use]
    pub fn review_mode(&self) -> Option<String> {
        if let Some(mode) = self.most_permissive_mode() {
            return Some(mode);
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
                "backendText.permissionPolicyUnsupported".into(),
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
                "permissionPicker.mode.read",
                "permissionPicker.description.codexReadOnly",
                false,
            ),
            (
                "default",
                "permissionPicker.mode.onRequest",
                "permissionPicker.description.codexDefault",
                false,
            ),
            (
                "full-access",
                "permissionPicker.mode.full",
                "permissionPicker.description.codexFullAccess",
                true,
            ),
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
                "permissionPicker.mode.plan",
                "permissionPicker.description.claudePlan",
                false,
            ),
            (
                "default",
                "permissionPicker.mode.ask",
                "permissionPicker.description.claudeDefault",
                false,
            ),
            (
                "acceptEdits",
                "permissionPicker.mode.edit",
                "permissionPicker.description.claudeAcceptEdits",
                false,
            ),
            (
                "dontAsk",
                "permissionPicker.mode.deny",
                "permissionPicker.description.claudeDontAsk",
                false,
            ),
            (
                "bypassPermissions",
                "permissionPicker.mode.bypass",
                "permissionPicker.description.claudeBypass",
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
                "permissionPicker.mode.ask",
                "permissionPicker.description.antigravityApprovals",
                false,
            ),
            (
                "plan",
                "permissionPicker.mode.plan",
                "permissionPicker.description.antigravityPlan",
                false,
            ),
            (
                "dangerously-skip-permissions",
                "permissionPicker.mode.bypass",
                "permissionPicker.description.antigravitySkip",
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
                    description: row["description"].as_str().unwrap_or_default().into(),
                    dangerous: is_dangerous_mode_id(id),
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
            (
                "ask",
                "permissionPicker.mode.ask",
                "permissionPicker.description.grokAsk",
                false,
            ),
            (
                "auto",
                "permissionPicker.mode.auto",
                "permissionPicker.description.grokAuto",
                false,
            ),
            (
                "always-approve",
                "permissionPicker.mode.bypass",
                "permissionPicker.description.grokAlwaysApprove",
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
            (
                "build",
                "permissionPicker.mode.ask",
                "permissionPicker.description.zcodeBuild",
                false,
            ),
            (
                "edit",
                "permissionPicker.mode.edit",
                "permissionPicker.description.zcodeEdit",
                false,
            ),
            (
                "plan",
                "permissionPicker.mode.plan",
                "permissionPicker.description.zcodePlan",
                false,
            ),
            (
                "auto",
                "permissionPicker.mode.auto",
                "permissionPicker.description.zcodeAuto",
                false,
            ),
            (
                "yolo",
                "permissionPicker.mode.bypass",
                "permissionPicker.description.zcodeYolo",
                true,
            ),
        ],
    )
}

#[must_use]
pub fn claude_with_auto(supported: bool) -> PermissionCatalog {
    claude_for(supported, true)
}

/// Claude Code exits at startup when root requests a bypass outside a
/// declared sandbox, so such hosts never see the mode.
#[must_use]
pub fn claude_for(auto: bool, bypass: bool) -> PermissionCatalog {
    let mut catalog = claude();
    if !bypass {
        catalog.modes.retain(|mode| mode.id != "bypassPermissions");
    }
    if auto {
        catalog.modes.insert(
            3,
            PermissionMode {
                id: "auto".into(),
                label: "permissionPicker.mode.auto".into(),
                description: "permissionPicker.description.claudeAuto".into(),
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
    fn defaults_prefer_the_most_permissive_mode_the_catalog_offers() {
        for (catalog, id) in [
            (codex(), "full-access"),
            (claude(), "bypassPermissions"),
            (grok(), "always-approve"),
            (zcode(), "yolo"),
            (antigravity(), "dangerously-skip-permissions"),
            (claude_for(true, false), "default"),
        ] {
            assert_eq!(
                catalog
                    .preferring_most_permissive()
                    .default_mode_id
                    .as_deref(),
                Some(id)
            );
        }
        let both = PermissionCatalog::new(
            "default",
            &[
                ("default", "", "", false),
                ("bypassPermissions", "", "", true),
                ("fullAccess", "", "", true),
            ],
        );
        assert_eq!(both.most_permissive_mode().as_deref(), Some("fullAccess"));
        let custom = PermissionCatalog::new(
            "ask",
            &[("ask", "", "", false), ("never-ask", "", "", true)],
        );
        assert_eq!(custom.most_permissive_mode().as_deref(), Some("never-ask"));
        let empty = PermissionCatalog::default().preferring_most_permissive();
        assert_eq!(empty.default_mode_id, None);
    }
    #[test]
    fn auto_is_offered_only_when_native_claude_supports_it() {
        assert!(claude_with_auto(false).validate(Some("auto")).is_err());
        assert!(claude_with_auto(true).validate(Some("auto")).is_ok());
        assert!(codex().validate(Some("bypassPermissions")).is_err());
    }
    #[test]
    fn unsandboxed_root_never_sees_claude_bypass() {
        let catalog = claude_for(true, false);
        assert!(catalog.validate(Some("bypassPermissions")).is_err());
        assert!(catalog.validate(Some("dontAsk")).is_ok());
        assert_eq!(catalog.review_mode().as_deref(), Some("default"));
        let ids: Vec<_> = claude_for(true, true)
            .modes
            .into_iter()
            .map(|mode| mode.id)
            .collect();
        assert_eq!(
            ids,
            [
                "plan",
                "default",
                "acceptEdits",
                "auto",
                "dontAsk",
                "bypassPermissions"
            ]
        );
    }
    #[test]
    fn native_modes_added_to_static_catalogs_stay_safe() {
        assert!(zcode().validate(Some("auto")).is_ok());
        assert!(antigravity().validate(Some("plan")).is_ok());
        for catalog in [zcode(), antigravity(), claude()] {
            assert!(
                catalog
                    .modes
                    .iter()
                    .all(|mode| mode.dangerous == is_dangerous_mode_id(&mode.id))
            );
        }
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
