//! Public tool identity survives endpoint/token rotation and native session reloads.
use crate::{Core, CoreError};
use agent_domain::TaskId;
use agent_protocol::EventSource;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const BOUND: &str = "computer_use.namespace_bound";

fn valid(name: &str) -> bool {
    name.strip_prefix("dogcodes_cu_").is_some_and(|suffix| {
        suffix.len() == 12
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn legacy(text: &str) -> Option<String> {
    let activity: Value = serde_json::from_str(text).ok()?;
    if activity["type"] != "tool" {
        return None;
    }
    let (name, tool) = activity["name"]
        .as_str()?
        .strip_prefix("mcp__")?
        .split_once("__")?;
    (valid(name) && super::runtime::TOOLS.contains(&tool)).then(|| name.to_owned())
}

pub(super) async fn for_task(core: &Core, task: &TaskId) -> Result<String, CoreError> {
    let head = core.repository.event_head().await?;
    let bound = core
        .repository
        .task_events_of_types(task, 0, head, &[BOUND], 1)
        .await?;
    if let Some(event) = bound.first() {
        return event.payload["name"]
            .as_str()
            .filter(|name| valid(name))
            .map(str::to_owned)
            .ok_or_else(|| CoreError::Internal("invalid computer-use namespace binding".into()));
    }
    // One-time migration: retain a namespace that the native history already uses.
    // No old URL, token, process, or endpoint is restored from the timeline.
    let timeline = core.repository.task_timeline(task).await?;
    let name = timeline
        .iter()
        .rev()
        .filter(|item| item.kind == "activity")
        .find_map(|item| legacy(&item.text))
        .unwrap_or_else(|| {
            format!(
                "dogcodes_cu_{}",
                &format!("{:x}", Sha256::digest(task.as_str().as_bytes()))[..12]
            )
        });
    core.append_runtime_event(
        task,
        BOUND,
        json!({"taskId":task,"name":name}),
        EventSource::Hub,
    )
    .await?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrates_only_well_formed_computer_tool_identities() {
        assert_eq!(
            legacy(r#"{"type":"tool","name":"mcp__dogcodes_cu_c04b722635ff__list_windows"}"#)
                .as_deref(),
            Some("dogcodes_cu_c04b722635ff")
        );
        for name in [
            "mcp__dogcodes_other__list_windows",
            "mcp__dogcodes_cu_../../secret__click",
            "mcp__dogcodes_cu_c04b722635ff__shell",
        ] {
            assert_eq!(
                legacy(&json!({"type":"tool","name":name}).to_string()),
                None
            );
        }
        assert_eq!(
            legacy(r#"{"type":"message","name":"mcp__dogcodes_cu_c04b722635ff__click"}"#),
            None
        );
    }
}
