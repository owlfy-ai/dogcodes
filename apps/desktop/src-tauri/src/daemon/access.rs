//! Access class of every service method. The service rejects unclassified
//! methods, so a new method cannot be reachable without a decision here. A
//! future remote gateway exposes only non-host methods and maps paired-device
//! permissions onto these classes; the local desktop keeps full access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MethodAccess {
    /// Reads state; no side effects.
    Read,
    /// Changes state or starts work; always carries an operation identity or
    /// version condition where a retry could matter.
    Write,
    /// Long-lived notification or event stream.
    Stream,
    /// Acts on this computer's UI, files outside projects, agent installation
    /// or service lifecycle. Never exposed to remote clients.
    HostOnly,
}

pub(crate) fn method_access(method: &str) -> Option<MethodAccess> {
    use MethodAccess::{HostOnly, Read, Stream, Write};
    Some(match method {
        "agent_preferences"
        | "approval_receipt"
        | "attachment_read"
        | "attachment_receipt"
        | "bootstrap"
        | "build_context_preview"
        | "build_edited_context_preview"
        | "command_receipt"
        | "fork_receipt"
        | "history_preferences"
        | "model_catalog_for_task"
        | "pending_approvals"
        | "permission_catalog"
        | "ping"
        | "prepare_approval_response"
        | "project_runs"
        | "query_history"
        | "quota_snapshots_for"
        | "read_artifact"
        | "recovery_snapshot"
        | "registered_executors"
        | "review_executors"
        | "review_source"
        | "subagent_history"
        | "sync_events"
        | "task_artifacts"
        | "task_deletion"
        | "terminal_get"
        | "terminal_list"
        | "terminal_read"
        | "timeline"
        | "timeline_snapshot"
        | "turn_reviews"
        | "workspace_directory"
        | "workspace_find_files"
        | "workspace_preview"
        | "workspace_roots"
        | "workspace_snapshots" => Read,
        "analyze_project_commands"
        | "attachment_save"
        | "cancel_turn_review"
        | "execute"
        | "fork_task"
        | "invalidate_approval_response"
        | "invalidate_pending_command"
        | "prepare_session_rebuild"
        | "project_run_edit"
        | "project_run_operation"
        | "project_runs_analyze"
        | "register_project"
        | "review"
        | "review_repair"
        | "set_running_permission_mode"
        | "start_turn_review"
        | "submission_intents"
        | "summarize_commit"
        | "terminal_resize"
        | "terminal_start"
        | "terminal_stop"
        | "terminal_input"
        | "update_project" => Write,
        "subscribe" | "subscribe_project" | "review_watch" | "project_runs_watch"
        | "submission_watch" | "workspace_watch" => Stream,
        "remote_control"
        | "retire_if_idle"
        | "workspace_lease"
        | "scan_agent"
        | "project_run_open"
        | "verified_local_artifact" => HostOnly,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every method literal passed to the service client in these files.
    fn called_methods() -> std::collections::BTreeSet<String> {
        let sources = [
            include_str!("../daemon.rs"),
            include_str!("workspace.rs"),
            include_str!("review_watch.rs"),
            include_str!("../submission_intents.rs"),
        ];
        let mut found = std::collections::BTreeSet::new();
        for source in sources {
            for line in source.lines() {
                for (index, _) in line.match_indices("(\"") {
                    let before = &line[..index];
                    // Method name after the last `.`: `call`, `connect` or `call::<T>`.
                    let callee = before.rsplit_once('.').map_or("", |(_, tail)| tail);
                    if !(callee == "call"
                        || callee == "connect"
                        || (callee.starts_with("call::<") && callee.ends_with('>')))
                    {
                        continue;
                    }
                    let rest = &line[index + 2..];
                    if let Some(end) = rest.find('"') {
                        let name = &rest[..end];
                        if !name.is_empty()
                            && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                        {
                            found.insert(name.to_string());
                        }
                    }
                }
            }
        }
        found
    }

    #[test]
    fn every_called_method_is_classified() {
        let methods = called_methods();
        assert!(
            methods.len() > 40,
            "scanner found too few methods: {methods:?}"
        );
        let missing: Vec<_> = methods
            .iter()
            .filter(|m| m.as_str() != "not_a_method" && method_access(m).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "classify these service methods: {missing:?}"
        );
    }

    #[test]
    fn host_only_methods_cover_lifecycle_and_local_ui() {
        for method in [
            "retire_if_idle",
            "workspace_lease",
            "scan_agent",
            "project_run_open",
        ] {
            assert_eq!(method_access(method), Some(MethodAccess::HostOnly));
        }
        assert_eq!(method_access("not_a_method"), None);
        assert_eq!(method_access("start_sidebar_chat"), None);
    }
}
