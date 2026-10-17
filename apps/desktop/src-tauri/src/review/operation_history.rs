//! Discover durable operations without any caller-local request cache. Read-only.
use super::{
    git_operations::GitOutcome,
    mutation_operations::MutationOutcome,
    service::{ReviewCall, ReviewContext},
};
use crate::{DesktopState, review_window::require_task_window};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationKind {
    Git,
    File,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HistoryQuery {
    pub task_id: String,
    pub kind: OperationKind,
    pub after: Option<String>,
    pub limit: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationSummary {
    operation_id: String,
    action: String,
    status: String,
    description: String,
    completed_files: Option<usize>,
    error: Option<String>,
    closed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryPage {
    entries: Vec<OperationSummary>,
    next_cursor: Option<String>,
}
fn compact(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}
fn unavailable(operation_id: String, error: Option<String>) -> OperationSummary {
    OperationSummary {
        operation_id,
        action: "unknown".into(),
        status: "unavailable".into(),
        description: String::new(),
        completed_files: None,
        error: error.map(|value| compact(&value, 1000)),
        closed: false,
    }
}
pub(super) async fn query(
    state: &ReviewContext<'_>,
    request: HistoryQuery,
) -> Result<HistoryPage, String> {
    super::service::require_task(state.core, &request.task_id).await?;
    let git = state.review.git_operations.clone();
    let files = state.review.mutations.clone();
    tokio::task::spawn_blocking(move || {
        let (entries, next_cursor) = match request.kind {
            OperationKind::Git => {
                let page = git.page(&request.task_id, request.after.as_deref(), request.limit)?;
                let entries = page
                    .entries
                    .into_iter()
                    .map(|entry| {
                        let Some(receipt) = entry.receipt else {
                            return unavailable(entry.operation_id, entry.error);
                        };
                        let receipt = match super::git_workflow::with_closure(&git, receipt) {
                            Ok(receipt) => receipt,
                            Err(error) => return unavailable(entry.operation_id, Some(error)),
                        };
                        let (status, error) = match receipt.outcome {
                            GitOutcome::Running => ("running", None),
                            GitOutcome::Unknown => ("unknown", None),
                            GitOutcome::Succeeded {
                                completed,
                                target_error,
                                ..
                            } => (
                                if completed {
                                    "succeeded"
                                } else {
                                    "confirmation_required"
                                },
                                target_error,
                            ),
                            GitOutcome::Failed { message } => {
                                ("failed", Some(compact(&message, 1000)))
                            }
                        };
                        OperationSummary {
                            operation_id: entry.operation_id,
                            action: compact(&receipt.request.action, 40),
                            status: status.into(),
                            description: compact(&receipt.request.message, 240),
                            completed_files: None,
                            closed: receipt.closed,
                            error,
                        }
                    })
                    .collect();
                (entries, page.next_cursor)
            }
            OperationKind::File => {
                let page = files.page(&request.task_id, request.after.as_deref(), request.limit)?;
                let entries = page
                    .entries
                    .into_iter()
                    .map(|entry| {
                        let Some(receipt) = entry.receipt else {
                            return unavailable(entry.operation_id, entry.error);
                        };
                        let args = receipt
                            .request
                            .arguments
                            .get("request")
                            .unwrap_or(&receipt.request.arguments);
                        let action = match receipt.request.command.as_str() {
                            "core_turn_undo" => "undo",
                            "core_review_discard" => "discard",
                            "core_review_ignore" => "ignore",
                            "core_review_index" => {
                                if args["action"] == "unstage" {
                                    "unstage"
                                } else {
                                    "stage"
                                }
                            }
                            "core_review_batch_files" => {
                                if args["action"] == "ignore" {
                                    "batch_ignore"
                                } else {
                                    "batch_discard"
                                }
                            }
                            _ => "unknown",
                        };
                        let description = args["path"].as_str().map_or_else(
                            || {
                                args["paths"]
                                    .as_array()
                                    .map(|paths| format!("{} 个文件", paths.len()))
                                    .unwrap_or_default()
                            },
                            |path| compact(path, 240),
                        );
                        let mut completed_files = None;
                        let (status, error) = match receipt.outcome {
                            MutationOutcome::Running => ("running", None),
                            MutationOutcome::Unknown => ("unknown", None),
                            MutationOutcome::Failed { message } => {
                                ("failed", Some(compact(&message, 1000)))
                            }
                            MutationOutcome::Succeeded { result } => {
                                completed_files = result["completed"].as_array().map(Vec::len);
                                match result["error"].as_str() {
                                    Some(error) => ("partial", Some(compact(error, 1000))),
                                    None => ("succeeded", None),
                                }
                            }
                        };
                        OperationSummary {
                            operation_id: entry.operation_id,
                            action: action.into(),
                            status: status.into(),
                            description,
                            completed_files,
                            closed: receipt.closed,
                            error,
                        }
                    })
                    .collect();
                (entries, page.next_cursor)
            }
        };
        Ok(HistoryPage {
            entries,
            next_cursor,
        })
    })
    .await
    .map_err(|_| "操作记录读取中断，请重新查询")?
}

#[tauri::command]
pub async fn core_review_operations(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: HistoryQuery,
) -> Result<HistoryPage, String> {
    require_task_window(window.label(), &request.task_id)?;
    state.core.review(ReviewCall::Operations(request)).await
}
