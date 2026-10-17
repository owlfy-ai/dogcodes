//! Scoped reads for turn startup. Never materialize sidebar or message history.
use super::{
    Connection, EventEnvelope, OptionalExtension, RepositoryError, TaskId, TaskSummary,
    load_session_maps, load_tasks, params, row_to_event, sql_json_error, storage_error, to_i64,
    update_pending_run_state,
};
use agent_core::{TaskExecutionState, TaskWorkspace};

pub(super) fn task_summary(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<Option<TaskSummary>, RepositoryError> {
    let deleted = connection.query_row(
            "SELECT event_type='task.deleted' FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type IN ('task.deleted','task.restored') ORDER BY sequence DESC LIMIT 1",
            [task_id.as_str()], |row| row.get::<_, bool>(0),
        ).optional().map_err(storage_error)?.unwrap_or(false);
    if deleted {
        return Ok(None);
    }
    let mut task = load_tasks(connection, Some(task_id))?.pop();
    if let Some(task) = &mut task {
        update_pending_run_state(connection, task)?;
    }
    Ok(task)
}

pub(super) fn execution_state(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<Option<TaskExecutionState>, RepositoryError> {
    let Some(task) = task_summary(connection, task_id)? else {
        return Ok(None);
    };
    let workspace = connection
        .query_row(
            "SELECT path, additional_directories_json FROM projects WHERE project_id = ?1",
            [task.project_id.as_str()],
            |row| {
                Ok(TaskWorkspace {
                    path: row.get(0)?,
                    additional_directories: serde_json::from_str(&row.get::<_, String>(1)?)
                        .map_err(sql_json_error)?,
                })
            },
        )
        .optional()
        .map_err(storage_error)?;
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    Ok(Some(TaskExecutionState {
        task,
        workspace,
        session_maps: load_session_maps(connection, Some(task_id))?,
    }))
}

pub(super) fn events_of_types(
    connection: &Connection,
    task_id: &TaskId,
    after: u64,
    through: u64,
    types: &[&str],
    limit: usize,
) -> Result<Vec<EventEnvelope>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, event_id, stream_type, stream_id, stream_version,
                event_type, schema_version, occurred_at, recorded_at,
                correlation_id, causation_id, source_json, payload_json, extensions_json
         FROM events WHERE stream_type = 'task' AND stream_id = ?1
           AND event_type IN (SELECT value FROM json_each(?4))
           AND sequence > ?2 AND sequence <= ?3
         ORDER BY sequence LIMIT ?5",
        )
        .map_err(storage_error)?;
    statement
        .query_map(
            params![
                task_id.as_str(),
                after,
                through,
                serde_json::to_string(types)
                    .map_err(|error| RepositoryError::Unavailable(error.to_string()))?,
                to_i64(limit as u64)?
            ],
            row_to_event,
        )
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
}
