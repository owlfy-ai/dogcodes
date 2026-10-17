//! Permanent deletion retains only a tombstone; legacy soft deletions remain readable.
use super::{AppendRequest, RepositoryError, parse_id, storage_error};
use agent_domain::TaskId;
use agent_protocol::StreamType;
use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeMap;

pub(super) fn deleted_ids(
    connection: &Connection,
) -> Result<BTreeMap<TaskId, u64>, RepositoryError> {
    let mut statement = connection.prepare(
        "SELECT stream_id, sequence FROM events WHERE event_type='task.deleted' AND sequence IN (
            SELECT MAX(sequence) FROM events WHERE stream_type='task' AND event_type IN ('task.deleted','task.restored') GROUP BY stream_id)")
        .map_err(storage_error)?;
    statement
        .query_map([], |row| Ok((parse_id(row.get(0)?)?, row.get(1)?)))
        .map_err(storage_error)?
        .collect::<Result<_, _>>()
        .map_err(storage_error)
}

pub(super) fn validate_append(
    connection: &Connection,
    request: &AppendRequest,
) -> Result<(), RepositoryError> {
    let event = &request.events[0];
    if event.stream.stream_type != StreamType::Task {
        return Ok(());
    }
    let id = &event.stream.stream_id;
    let deleted = connection.query_row(
        "SELECT event_type='task.deleted' FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type IN ('task.deleted','task.restored') ORDER BY sequence DESC LIMIT 1",
        [id], |row| row.get::<_,bool>(0)).optional().map_err(storage_error)?.unwrap_or(false);
    let permanent = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type='task.deleted' AND json_extract(payload_json,'$.permanent')=1)",
        [id], |row| row.get::<_, bool>(0)).map_err(storage_error)?;
    if permanent {
        return Err(RepositoryError::Unavailable(
            "会话已彻底删除，无法恢复或继续写入".into(),
        ));
    }
    let lifecycle = request
        .events
        .iter()
        .any(|e| matches!(e.event_type.as_str(), "task.deleted" | "task.restored"));
    if lifecycle
        && (request.events.len() != 1
            || !request.artifacts.is_empty()
            || request.expected_stream_version.is_none())
    {
        return Err(RepositoryError::Unavailable(
            "会话删除或恢复必须是独立的版本化操作".into(),
        ));
    }
    if deleted && event.event_type != "task.restored" {
        return Err(RepositoryError::Unavailable(
            "会话已删除，请先从最近删除中恢复".into(),
        ));
    }
    if event.event_type == "task.restored" && !deleted {
        return Err(RepositoryError::Unavailable("最近删除中没有此会话".into()));
    }
    if event.event_type != "task.deleted" {
        return Ok(());
    }
    let active = connection
        .query_row(
            "SELECT active_run_id IS NOT NULL FROM tasks WHERE task_id=?1",
            [id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_error)?;
    // Check in the same transaction as the append: a review may have started
    // after Core validation. Conversely, a queued review cannot append after deletion.
    let review_active = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE sequence IN (
            SELECT MAX(sequence) FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type IN ('review.started','review.updated')
            GROUP BY json_extract(payload_json,'$.review.request.runId'))
         AND json_extract(payload_json,'$.review.status') IN ('queued','running','cancelling'))",
        [id], |row| row.get::<_,bool>(0)).map_err(storage_error)?;
    if active || review_active {
        return Err(RepositoryError::Unavailable(
            "请先停止主任务和审查，再删除会话".into(),
        ));
    }
    Ok(())
}

/// Called inside the deletion transaction, also safe during projection rebuild.
/// Only artifacts owned/referenced by this task are candidates; shared artifacts
/// and the ancestors of surviving artifact versions remain protected.
pub(super) fn purge(
    connection: &Connection,
    task: &str,
    sequence: u64,
) -> Result<(), RepositoryError> {
    connection
        .execute_batch("CREATE TEMP TABLE deletion_candidates(id TEXT PRIMARY KEY);")
        .map_err(storage_error)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO deletion_candidates
         SELECT artifact_id FROM task_artifacts WHERE task_id=?1
         UNION SELECT artifact_id FROM workspace_diff_artifacts WHERE task_id=?1
         UNION SELECT artifact_id FROM context_transfers WHERE task_id=?1
         UNION SELECT a.artifact_id FROM events e, json_tree(e.payload_json) j
           JOIN artifacts a ON a.artifact_id=j.value
           WHERE e.stream_type='task' AND e.stream_id=?1",
            [task],
        )
        .map_err(storage_error)?;
    // Persisted parent chains can include artifacts no longer directly mentioned.
    connection.execute_batch("INSERT OR IGNORE INTO deletion_candidates
        WITH RECURSIVE ancestors(id) AS (
          SELECT id FROM deletion_candidates UNION
          SELECT v.parent_artifact_id FROM artifact_versions v JOIN ancestors a ON a.id=v.artifact_id)
        SELECT id FROM ancestors;") .map_err(storage_error)?;
    for table in [
        "agent_session_maps",
        "workspace_snapshots",
        "workspace_diff_artifacts",
        "context_transfers",
        "reconciliation_opt_outs",
        "timeline_items",
        "recent_completion",
        "task_artifacts",
        "tasks",
    ] {
        connection
            .execute(&format!("DELETE FROM {table} WHERE task_id=?1"), [task])
            .map_err(storage_error)?;
    }
    connection
        .execute(
            "DELETE FROM events WHERE stream_type='task' AND stream_id=?1 AND sequence<>?2",
            rusqlite::params![task, sequence],
        )
        .map_err(storage_error)?;
    connection.execute_batch("DELETE FROM deletion_candidates WHERE id IN (
        SELECT artifact_id FROM task_artifacts UNION SELECT artifact_id FROM workspace_diff_artifacts
        UNION SELECT artifact_id FROM context_transfers
        UNION SELECT j.value FROM events e, json_tree(e.payload_json) j WHERE j.type='text');
        DELETE FROM deletion_candidates WHERE id IN (
          WITH RECURSIVE live(id) AS (
            SELECT artifact_id FROM artifacts WHERE artifact_id NOT IN (SELECT id FROM deletion_candidates)
            UNION SELECT v.parent_artifact_id FROM artifact_versions v JOIN live l ON l.id=v.artifact_id)
          SELECT id FROM live);
        CREATE TEMP TABLE deletion_hashes AS SELECT payload_sha256 FROM artifacts WHERE artifact_id IN (SELECT id FROM deletion_candidates);
        DELETE FROM artifact_versions WHERE artifact_id IN (SELECT id FROM deletion_candidates);
        DELETE FROM artifacts WHERE artifact_id IN (SELECT id FROM deletion_candidates);
        DELETE FROM artifact_blobs WHERE payload_sha256 IN (SELECT payload_sha256 FROM deletion_hashes)
          AND NOT EXISTS(SELECT 1 FROM artifacts a WHERE a.payload_sha256=artifact_blobs.payload_sha256);
        DROP TABLE deletion_hashes;
        DROP TABLE deletion_candidates;") .map_err(storage_error)?;
    Ok(())
}
