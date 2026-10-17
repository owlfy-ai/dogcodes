//! Compact review status projection shared by desktop and companion clients.
use super::{RepositoryError, sql_json_error, storage_error};
use agent_domain::TaskId;
use agent_protocol::CompanionReview;
use rusqlite::Connection;
use std::collections::BTreeMap;

pub(super) fn load(
    connection: &Connection,
) -> Result<BTreeMap<TaskId, Vec<CompanionReview>>, RepositoryError> {
    let mut statement = connection.prepare(
        "SELECT stream_id, json_object(
            'runId', json_extract(payload_json, '$.review.request.runId'),
            'providerId', json_extract(payload_json, '$.review.request.executor.providerId'),
            'kind', json_extract(payload_json, '$.review.request.kind'),
            'status', json_extract(payload_json, '$.review.status'),
            'createdAt', json_extract(payload_json, '$.review.createdAt'),
            'finishedAt', json_extract(payload_json, '$.review.finishedAt'),
            'completedGroups', (SELECT count(*) FROM json_each(payload_json, '$.review.workflow.groups') WHERE json_extract(value, '$.status') = 'completed'),
            'totalGroups', COALESCE(json_array_length(payload_json, '$.review.workflow.groups'), 0))
         FROM events WHERE sequence IN (
            SELECT MAX(sequence) FROM events WHERE event_type IN ('review.started', 'review.updated') AND stream_type = 'task'
            GROUP BY stream_id, json_extract(payload_json, '$.review.request.runId'))
         AND (json_extract(payload_json, '$.review.status') IN ('queued', 'running', 'cancelling')
              OR julianday(json_extract(payload_json, '$.review.finishedAt')) >= julianday('now', '-60 seconds'))
         ORDER BY sequence DESC"
    ).map_err(storage_error)?;
    let records = statement
        .query_map([], |row| {
            let task_id = super::parse_id::<TaskId>(row.get::<_, String>(0)?)?;
            let review = serde_json::from_str::<CompanionReview>(&row.get::<_, String>(1)?)
                .map_err(sql_json_error)?;
            Ok((task_id, review))
        })
        .map_err(storage_error)?;
    let mut result: BTreeMap<TaskId, Vec<CompanionReview>> = BTreeMap::new();
    for record in records {
        let (task_id, review) = record.map_err(storage_error)?;
        result.entry(task_id).or_default().push(review);
    }
    Ok(result)
}
