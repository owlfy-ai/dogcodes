//! `SQLite` canonical event store and rebuildable read projections.

use std::{path::Path, sync::Mutex};

use agent_core::{
    AppendOutcome, AppendRequest, CoreRepository, PendingContextTransfer,
    PendingReconciliationOptOut, RepositoryError, StoredArtifact,
};
use agent_domain::{ArtifactId, ExecutorRef, InstallationId, ProviderId, RunState, TaskId};
use agent_protocol::{
    ActiveRunSummary, AgentSessionMapSummary, ArtifactCursor, ArtifactMetadata, BootstrapSnapshot,
    ChangeSummary, CommandAck, CompanionProjection, CompanionTerminal, CompanionWorkspace,
    ContextPackageKind, CreateTask, EventEnvelope, EventSource, GitWorkspaceSnapshotV1,
    InstallationSummary, PendingApproval, ProjectSummary, RecentCompletion, RegisterProject,
    ResumeCapability, SCHEMA_VERSION, SessionMapStatus, StreamRef, StreamType, TaskArtifactSummary,
    TaskSummary, TimelineItem, WorkspaceDiagnosticKind, WorkspaceSnapshotPhase,
    WorkspaceSnapshotSummary,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod agent_preferences;
mod companion_reviews;
mod history_preferences;
mod shared_preferences;
mod startup_reads;
mod task_lifecycle;

pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        let path = path.as_ref();
        // Inspect existing data without changing journal settings.
        // Missing files are normal on first launch; other metadata failures are not.
        match std::fs::metadata(path) {
            Ok(_) => {
                let reader =
                    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .map_err(storage_error)?;
                check_database(&reader)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(RepositoryError::Unavailable(
                    "database inspection unavailable".into(),
                ));
            }
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        Self::from_connection(connection)
    }

    /// Read only: whether an existing database was written by a newer format.
    /// Missing or unreadable files report false; opening them reports the real error.
    #[must_use]
    pub fn written_by_newer_version(path: impl AsRef<Path>) -> bool {
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|c| c.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)))
            .is_ok_and(|format| format > STORE_FORMAT_VERSION)
    }

    pub fn open_in_memory() -> Result<Self, RepositoryError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, RepositoryError> {
        check_database(&connection)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        initialize_schema(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn rebuild_projections(&self) -> Result<(), RepositoryError> {
        let mut connection = self.connection.lock().map_err(poison_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;
        rebuild_in_transaction(&transaction)?;
        transaction.commit().map_err(storage_error)
    }
}

#[cfg(test)]
mod startup_tests;

/// On-disk format of the database, stored in `PRAGMA user_version`. Bump it only
/// together with a migration. A build refuses a database with a newer format
/// instead of reading or changing it, so downgrading never damages user data.
pub const STORE_FORMAT_VERSION: i64 = 1;
/// Error text identifying a database written by a newer format.
pub const NEWER_FORMAT_ERROR: &str = "database written by a newer dogcodes version";

fn check_format(connection: &Connection) -> Result<i64, RepositoryError> {
    let format: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if format > STORE_FORMAT_VERSION {
        return Err(RepositoryError::Unavailable(NEWER_FORMAT_ERROR.into()));
    }
    Ok(format)
}

fn check_database(connection: &Connection) -> Result<(), RepositoryError> {
    check_format(connection)?;
    check_database_with_budget(connection, std::time::Duration::from_secs(5))
}

fn check_database_with_budget(
    connection: &Connection,
    budget: std::time::Duration,
) -> Result<(), RepositoryError> {
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(storage_error)?;
    let started = std::time::Instant::now();
    connection.progress_handler(1_000, Some(move || started.elapsed() >= budget));
    let result =
        connection.query_row::<String, _, _>("PRAGMA quick_check(1)", [], |row| row.get(0));
    connection.progress_handler(0, None::<fn() -> bool>);
    let result = result
        .map_err(|_| RepositoryError::Unavailable("database integrity check unavailable".into()))?;
    if result != "ok" {
        return Err(RepositoryError::Unavailable(
            "database integrity check failed".into(),
        ));
    }
    Ok(())
}

// Initialize an empty database. Existing data is never rewritten; format changes
// need an explicit migration together with a STORE_FORMAT_VERSION bump.
fn initialize_schema(connection: &mut Connection) -> Result<(), RepositoryError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let format = check_format(&transaction)?;
    let has_tables: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')",
        [], |row| row.get(0),
    ).map_err(storage_error)?;
    if !has_tables {
        transaction.execute_batch(SCHEMA).map_err(storage_error)?;
    }
    // Databases created before the stamp share format 1; recording it changes no data.
    if format < STORE_FORMAT_VERSION {
        transaction
            .pragma_update(None, "user_version", STORE_FORMAT_VERSION)
            .map_err(storage_error)?;
    }
    // Additive indexes also apply to existing databases; projections need no rebuild.
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS events_task_type_sequence ON events(stream_type, stream_id, event_type, sequence);
         CREATE INDEX IF NOT EXISTS events_kind_task_sequence ON events(stream_type, event_type, stream_id, sequence);"
    ).map_err(storage_error)?;
    transaction.commit().map_err(storage_error)
}

fn rebuild_in_transaction(transaction: &Transaction<'_>) -> Result<(), RepositoryError> {
    transaction
        .execute_batch(
            "DELETE FROM task_artifacts;
                 DELETE FROM timeline_items;
                 DELETE FROM recent_completion;
                 DELETE FROM context_transfers;
                 DELETE FROM reconciliation_opt_outs;
                 DELETE FROM artifact_versions;
                 DELETE FROM agent_session_maps;
                 DELETE FROM workspace_snapshots;
                 DELETE FROM workspace_diff_artifacts;
                 DELETE FROM installations;
                 DELETE FROM tasks;
                 DELETE FROM projects;",
        )
        .map_err(storage_error)?;

    let events = {
        let mut statement = transaction
            .prepare(
                "SELECT sequence, event_id, stream_type, stream_id, stream_version,
                            event_type, schema_version, occurred_at, recorded_at,
                            correlation_id, causation_id, source_json, payload_json, extensions_json
                     FROM events ORDER BY sequence",
            )
            .map_err(storage_error)?;
        statement
            .query_map([], row_to_event)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?
    };
    for event in &events {
        apply_projection(transaction, event)?;
    }
    Ok(())
}

#[async_trait]
impl CoreRepository for SqliteStore {
    async fn history_preferences(
        &self,
    ) -> Result<agent_protocol::HistoryPreferencesSnapshot, RepositoryError> {
        self.read_history_preferences()
    }
    async fn agent_preferences(
        &self,
    ) -> Result<agent_protocol::AgentPreferencesSnapshot, RepositoryError> {
        self.read_agent_preferences()
    }
    async fn command_ack(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<CommandAck>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                "SELECT ack_json FROM command_dedup WHERE idempotency_key = ?1",
                [idempotency_key],
                |row| {
                    let json = row.get::<_, String>(0)?;
                    serde_json::from_str(&json).map_err(sql_json_error)
                },
            )
            .optional()
            .map_err(storage_error)
    }

    async fn deleted_task_ack(
        &self,
        task_id: &TaskId,
        command_id: &agent_domain::CommandId,
        key: &str,
        request_fingerprint: &str,
    ) -> Result<Option<CommandAck>, RepositoryError> {
        let mut connection = self.connection.lock().map_err(poison_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;
        if let Some(saved) = transaction
            .query_row(
                "SELECT ack_json FROM command_dedup WHERE idempotency_key=?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
        {
            let ack: CommandAck = serde_json::from_str(&saved).map_err(json_error)?;
            agent_core::command_identity::verify(&ack, request_fingerprint)
                .map_err(|error| RepositoryError::Unavailable(error.to_string()))?;
            return Ok(Some(ack));
        }
        let sequence = transaction.query_row("SELECT sequence FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type='task.deleted' AND json_extract(payload_json,'$.permanent')=1", [task_id.as_str()], |row| row.get::<_, u64>(0)).optional().map_err(storage_error)?;
        let Some(sequence) = sequence else {
            return Ok(None);
        };
        let ack = CommandAck {
            request_fingerprint: Some(request_fingerprint.to_owned()),
            command_id: command_id.clone(),
            accepted: true,
            first_sequence: Some(sequence),
            last_sequence: Some(sequence),
        };
        transaction.execute("INSERT INTO command_dedup(idempotency_key,command_id,ack_json,created_at) VALUES(?1,?2,?3,?4)", params![key, command_id.as_str(), serde_json::to_string(&ack).map_err(json_error)?, Utc::now().timestamp_millis()]).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(Some(ack))
    }

    async fn pending_task_cleanups(&self) -> Result<Vec<TaskId>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection.prepare("SELECT stream_id FROM events WHERE stream_type='task' AND event_type='task.deleted' AND json_extract(payload_json,'$.permanent')=1 AND json_extract(payload_json,'$.cleanupPending')=1").map_err(storage_error)?;
        statement
            .query_map([], |row| parse_id(row.get(0)?))
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn complete_task_cleanup(&self, task_id: &TaskId) -> Result<(), RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection.execute("UPDATE events SET payload_json=json_set(payload_json,'$.cleanupPending',json('false')) WHERE stream_type='task' AND stream_id=?1 AND event_type='task.deleted' AND json_extract(payload_json,'$.permanent')=1", [task_id.as_str()]).map_err(storage_error)?;
        Ok(())
    }

    async fn append(&self, request: AppendRequest) -> Result<AppendOutcome, RepositoryError> {
        let mut connection = self.connection.lock().map_err(poison_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;

        if let Some(ack_json) = transaction
            .query_row(
                "SELECT ack_json FROM command_dedup WHERE idempotency_key = ?1",
                [&request.idempotency_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
        {
            let ack: CommandAck = serde_json::from_str(&ack_json).map_err(json_error)?;
            // Admission and identity comparison must share the same transaction.
            // Both absent preserves legacy/internal append behavior, but a bound
            // Core command cannot collide with an unbound internal operation.
            if ack.request_fingerprint != request.request_fingerprint {
                return Err(RepositoryError::Unavailable(
                    "请求标识已用于不同的命令或版本条件，未重复执行".into(),
                ));
            }
            return Ok(AppendOutcome {
                ack,
                events: vec![],
                duplicate: true,
            });
        }

        if request.events.is_empty() {
            return Err(RepositoryError::Unavailable(
                "an append must contain at least one event".into(),
            ));
        }
        let first_stream = &request.events[0].stream;
        if request
            .events
            .iter()
            .any(|event| event.stream != *first_stream)
        {
            return Err(RepositoryError::Unavailable(
                "one append may only target a single stream".into(),
            ));
        }

        let actual_version = current_stream_version(&transaction, first_stream)?;
        if let Some(expected) = request.expected_stream_version
            && expected != actual_version
        {
            return Err(RepositoryError::Conflict {
                expected,
                actual: actual_version,
            });
        }

        task_lifecycle::validate_append(&transaction, &request)?;
        for event in &request.events {
            if event.event_type == "project.updated" {
                let project: RegisterProject =
                    serde_json::from_value(event.payload.clone()).map_err(json_error)?;
                let directories =
                    serde_json::to_string(&project.additional_directories).map_err(json_error)?;
                let roots_changed = transaction.query_row(
                    "SELECT path<>?2 OR additional_directories_json<>?3 FROM projects WHERE project_id=?1",
                    params![project.project_id.as_str(), project.path, directories], |row| row.get::<_, bool>(0),
                ).map_err(storage_error)?;
                if roots_changed {
                    let active = transaction.query_row(
                        "SELECT EXISTS(SELECT 1 FROM tasks WHERE project_id=?1 AND active_run_id IS NOT NULL)
                         OR EXISTS(SELECT 1 FROM events WHERE sequence IN (
                           SELECT MAX(e.sequence) FROM events e JOIN tasks t ON t.task_id=e.stream_id
                           WHERE t.project_id=?1 AND e.event_type IN ('review.started','review.updated')
                           GROUP BY json_extract(e.payload_json,'$.review.request.runId'))
                         AND json_extract(payload_json,'$.review.status') IN ('queued','running','cancelling'))",
                        [project.project_id.as_str()], |row| row.get::<_, bool>(0),
                    ).map_err(storage_error)?;
                    if active {
                        return Err(RepositoryError::Unavailable(
                            "请先停止项目中正在运行的任务和审查，再修改目录".into(),
                        ));
                    }
                }
            }
        }

        for artifact in &request.artifacts {
            validate_artifact(artifact)?;
            transaction
                .execute(
                    "INSERT INTO artifact_blobs(payload_sha256, byte_length, content)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(payload_sha256) DO NOTHING",
                    params![
                        artifact.metadata.payload_sha256,
                        to_i64(artifact.metadata.byte_length)?,
                        artifact.content,
                    ],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO artifacts(
                        artifact_id, payload_sha256, media_type,
                        redaction_ruleset_version, redaction_count, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        artifact.metadata.artifact_id.to_string(),
                        artifact.metadata.payload_sha256,
                        artifact.metadata.media_type,
                        i64::from(artifact.metadata.redaction_ruleset_version),
                        i64::from(artifact.metadata.redaction_count),
                        artifact.metadata.created_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
        }

        let mut recorded = Vec::with_capacity(request.events.len());
        let recorded_at = Utc::now();
        for (index, event) in request.events.into_iter().enumerate() {
            let stream_version = actual_version + index as u64 + 1;
            transaction
                .execute(
                    "INSERT INTO events(
                        event_id, stream_type, stream_id, stream_version, event_type,
                        schema_version, occurred_at, recorded_at, correlation_id,
                        causation_id, source_json, payload_json, extensions_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        event.event_id.to_string(),
                        event.stream.stream_type.as_str(),
                        event.stream.stream_id,
                        to_i64(stream_version)?,
                        event.event_type,
                        i64::from(SCHEMA_VERSION),
                        event.occurred_at.timestamp_millis(),
                        recorded_at.timestamp_millis(),
                        event.correlation_id.to_string(),
                        event.causation_id,
                        serde_json::to_string(&event.source).map_err(json_error)?,
                        serde_json::to_string(&event.payload).map_err(json_error)?,
                        event
                            .vendor_extensions
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(json_error)?,
                    ],
                )
                .map_err(storage_error)?;
            let sequence = u64::try_from(transaction.last_insert_rowid()).map_err(|error| {
                RepositoryError::Unavailable(format!("invalid event sequence: {error}"))
            })?;
            let envelope = EventEnvelope {
                schema_version: SCHEMA_VERSION,
                sequence,
                event_id: event.event_id,
                stream: event.stream,
                stream_version,
                event_type: event.event_type,
                occurred_at: event.occurred_at,
                recorded_at,
                correlation_id: event.correlation_id,
                causation_id: event.causation_id,
                source: event.source,
                payload: event.payload,
                vendor_extensions: event.vendor_extensions,
            };
            apply_projection(&transaction, &envelope)?;
            recorded.push(envelope);
        }

        let ack = CommandAck {
            request_fingerprint: request.request_fingerprint,
            command_id: request.command_id,
            accepted: true,
            first_sequence: recorded.first().map(|event| event.sequence),
            last_sequence: recorded.last().map(|event| event.sequence),
        };
        transaction
            .execute(
                "INSERT INTO command_dedup(idempotency_key, command_id, ack_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    request.idempotency_key,
                    ack.command_id.to_string(),
                    serde_json::to_string(&ack).map_err(json_error)?,
                    recorded_at.timestamp_millis(),
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(AppendOutcome {
            ack,
            events: recorded,
            duplicate: false,
        })
    }

    async fn bootstrap(&self) -> Result<BootstrapSnapshot, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut projects = {
            let mut statement = connection
                .prepare(
                    "SELECT p.project_id, p.display_name, p.path, COUNT(t.task_id), p.additional_directories_json, p.stream_version,
                            (SELECT occurred_at FROM events WHERE stream_type='project' AND stream_id=p.project_id ORDER BY stream_version DESC LIMIT 1)
                     FROM projects p LEFT JOIN tasks t ON t.project_id = p.project_id
                     GROUP BY p.project_id ORDER BY p.display_name COLLATE NOCASE",
                )
                .map_err(storage_error)?;
            statement
                .query_map([], |row| {
                    Ok(ProjectSummary {
                        updated_at: row.get::<_, Option<i64>>(6)?.map(from_millis).transpose()?,
                        stream_version: row.get(5)?,
                        additional_directories: serde_json::from_str(&row.get::<_, String>(4)?)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                        project_id: parse_id(row.get::<_, String>(0)?)?,
                        display_name: row.get(1)?,
                        path: row.get(2)?,
                        task_count: row.get(3)?,
                    })
                })
                .map_err(storage_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let deleted_ids = task_lifecycle::deleted_ids(&connection)?;
        let (mut deleted_tasks, mut tasks): (Vec<_>, Vec<_>) = load_tasks(&connection, None)?
            .into_iter()
            .partition(|task| deleted_ids.contains_key(&task.task_id));
        deleted_tasks.sort_by_key(|task| std::cmp::Reverse(deleted_ids[&task.task_id]));
        for project in &mut projects {
            project.task_count = u32::try_from(
                tasks
                    .iter()
                    .filter(|task| task.project_id == project.project_id)
                    .count(),
            )
            .unwrap_or(u32::MAX);
        }
        for task in &mut tasks {
            update_pending_run_state(&connection, task)?;
        }
        let installations = load_installations(&connection)?;
        let session_maps = load_session_maps(&connection, None)?
            .into_iter()
            .filter(|session| !deleted_ids.contains_key(&session.task_id))
            .collect();
        let active_runs = tasks
            .iter()
            .filter_map(|task| task.active_primary_run.clone())
            .collect::<Vec<_>>();
        let last_sequence = connection
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(storage_error)?;
        let companion = companion_projection(&connection, &tasks, last_sequence)?;
        Ok(BootstrapSnapshot {
            projects,
            tasks,
            deleted_tasks,
            installations,
            active_runs,
            session_maps,
            last_sequence,
            companion,
        })
    }

    async fn sync_events(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, event_id, stream_type, stream_id, stream_version,
                        event_type, schema_version, occurred_at, recorded_at,
                        correlation_id, causation_id, source_json, payload_json, extensions_json
                 FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(params![after_sequence, to_i64(limit as u64)?], row_to_event)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn frozen_history_messages(
        &self,
        task_id: &TaskId,
        after: u64,
        through: u64,
        text: &str,
        limit: usize,
    ) -> Result<Vec<agent_core::history::FrozenMessage>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection.prepare(
            "WITH fragments AS (
                SELECT sequence, occurred_at, event_type, json_extract(payload_json,'$.turnId') AS turn_id,
                    CASE event_type WHEN 'turn.queued' THEN json_extract(payload_json,'$.prompt')
                        WHEN 'message.delta' THEN json_extract(payload_json,'$.text') ELSE payload_json END AS body,
                    CASE event_type WHEN 'turn.queued' THEN 'prompt' WHEN 'message.delta' THEN 'text' ELSE 'payload' END AS field,
                    CASE event_type WHEN 'message.delta' THEN 'message:' || coalesce(json_extract(payload_json,'$.turnId'),'') || ':' || coalesce(json_extract(payload_json,'$.itemId'),'') ELSE 'event:' || sequence END AS group_key
                FROM events WHERE stream_type='task' AND stream_id=?1 AND sequence<=?3
                    AND event_type IN ('turn.queued','message.delta','test.completed','activity.updated','tool.started','tool.completed','approval.requested','approval.resolved','workspace.diff_captured','workspace.operation_changes_captured','session.rebuild_context_prepared','handoff.package_confirmed','reconciliation.package_confirmed','handoff.sent','reconciliation.sent','turn.failed','turn.interrupted','review.updated','task.forked')
             ), messages AS (
                SELECT min(sequence) AS sequence, min(occurred_at) AS occurred_at, event_type, turn_id,
                    group_concat(body,'' ORDER BY sequence) AS body,
                    json_group_array(json_object('sequence',sequence,'field',field,'length',length(CAST(body AS BLOB))) ORDER BY sequence) AS fragments
                FROM fragments GROUP BY group_key
             ) SELECT sequence,occurred_at,event_type,turn_id,body,fragments FROM messages
                WHERE sequence>?2 AND instr(lower(body),lower(?4))>0 ORDER BY sequence LIMIT ?5"
        ).map_err(storage_error)?;
        statement
            .query_map(
                params![
                    task_id.as_str(),
                    to_i64(after)?,
                    to_i64(through)?,
                    text,
                    to_i64(limit as u64)?
                ],
                |row| {
                    Ok(agent_core::history::FrozenMessage {
                        sequence: row.get(0)?,
                        occurred_at: row.get(1)?,
                        kind: row.get(2)?,
                        turn_id: row.get(3)?,
                        text: row.get(4)?,
                        fragments: serde_json::from_str(&row.get::<_, String>(5)?)
                            .map_err(sql_json_error)?,
                    })
                },
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn history_items(
        &self,
        task_id: &TaskId,
        after: u64,
        through: u64,
        text: &str,
        limit: usize,
    ) -> Result<Vec<TimelineItem>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection.prepare(
            "SELECT item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at, (SELECT json_extract(payload_json, '$.attachments') FROM events WHERE events.sequence = timeline_items.sequence AND event_type = 'turn.queued') FROM timeline_items
             WHERE task_id = ?1 AND sequence > ?2 AND sequence <= ?3
             AND instr(lower(text), lower(?4)) > 0 ORDER BY sequence LIMIT ?5"
        ).map_err(storage_error)?;
        statement
            .query_map(
                params![
                    task_id.as_str(),
                    to_i64(after)?,
                    to_i64(through)?,
                    text,
                    to_i64(limit as u64)?
                ],
                |row| {
                    Ok(TimelineItem {
                        turn_completion: None,
                        attachments: row
                            .get::<_, Option<String>>(8)?
                            .and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or_default(),
                        item_id: row.get(0)?,
                        task_id: parse_id(row.get::<_, String>(1)?)?,
                        turn_id: row.get::<_, Option<String>>(2)?.map(parse_id).transpose()?,
                        kind: row.get(3)?,
                        text: row.get(4)?,
                        provider_id: row.get(5)?,
                        sequence: row.get(6)?,
                        occurred_at: from_millis(row.get(7)?)?,
                    })
                },
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn task_summary(&self, task_id: &TaskId) -> Result<Option<TaskSummary>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        startup_reads::task_summary(&connection, task_id)
    }

    async fn event_head(&self) -> Result<u64, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(storage_error)
    }

    async fn task_execution_state(
        &self,
        task_id: &TaskId,
    ) -> Result<Option<agent_core::TaskExecutionState>, RepositoryError> {
        let mut connection = self.connection.lock().map_err(poison_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let state = startup_reads::execution_state(&transaction, task_id)?;
        transaction.commit().map_err(storage_error)?;
        Ok(state)
    }

    async fn task_events_of_types(
        &self,
        task_id: &TaskId,
        after: u64,
        through: u64,
        types: &[&str],
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        startup_reads::events_of_types(&connection, task_id, after, through, types, limit)
    }

    async fn task_timeline(&self, task_id: &TaskId) -> Result<Vec<TimelineItem>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        load_timeline(&connection, task_id)
    }

    async fn task_timeline_snapshot(
        &self,
        task_id: &TaskId,
    ) -> Result<agent_protocol::TimelineSnapshot, RepositoryError> {
        let mut connection = self.connection.lock().map_err(poison_error)?;
        // The transaction also protects against another SQLite connection writing
        // between the projection read and the cursor read.
        let transaction = connection.transaction().map_err(storage_error)?;
        let items = load_timeline(&transaction, task_id)?;
        let last_sequence = transaction
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(agent_protocol::TimelineSnapshot {
            items,
            last_sequence,
        })
    }

    /// Folds `approval.requested`/`approval.resolved` task events into the
    /// set of approvals still awaiting an operator decision. Derived from the
    /// Canonical stream so rebuilds cannot diverge.
    async fn pending_approvals(
        &self,
        task_id: &TaskId,
        limit: usize,
    ) -> Result<Vec<PendingApproval>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        pending_approvals_from_connection(&connection, task_id, limit)
    }

    async fn workspace_snapshots(
        &self,
        task_id: &TaskId,
        limit: usize,
    ) -> Result<Vec<WorkspaceSnapshotSummary>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT task_id, turn_id, run_id, phase_json, sequence, captured_at,
                        git_json, change_summary_json, relevant_file_count,
                        dependency_change_count, diagnostic_json
                 FROM workspace_snapshots WHERE task_id = ?1
                 ORDER BY sequence DESC LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(params![task_id.as_str(), to_i64(limit as u64)?], |row| {
                let phase_json = row.get::<_, String>(3)?;
                let git_json = row.get::<_, Option<String>>(6)?;
                let change_summary_json = row.get::<_, Option<String>>(7)?;
                let diagnostic_json = row.get::<_, Option<String>>(10)?;
                Ok(WorkspaceSnapshotSummary {
                    task_id: parse_id(row.get::<_, String>(0)?)?,
                    turn_id: parse_id(row.get::<_, String>(1)?)?,
                    run_id: parse_id(row.get::<_, String>(2)?)?,
                    phase: serde_json::from_str::<WorkspaceSnapshotPhase>(&phase_json)
                        .map_err(sql_json_error)?,
                    sequence: row.get(4)?,
                    captured_at: from_millis(row.get(5)?)?,
                    git: git_json
                        .map(|value| serde_json::from_str::<GitWorkspaceSnapshotV1>(&value))
                        .transpose()
                        .map_err(sql_json_error)?,
                    change_summary: change_summary_json
                        .map(|value| serde_json::from_str::<ChangeSummary>(&value))
                        .transpose()
                        .map_err(sql_json_error)?,
                    relevant_file_count: u32::try_from(row.get::<_, u64>(8)?)
                        .map_err(|error| sql_conversion_error(error.to_string()))?,
                    dependency_change_count: u32::try_from(row.get::<_, u64>(9)?)
                        .map_err(|error| sql_conversion_error(error.to_string()))?,
                    diagnostic: diagnostic_json
                        .map(|value| serde_json::from_str::<WorkspaceDiagnosticKind>(&value))
                        .transpose()
                        .map_err(sql_json_error)?,
                })
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn workspace_diff_artifact_ref(
        &self,
        task_id: &TaskId,
        status_fingerprint: &str,
        after_sequence: u64,
        through_sequence: u64,
    ) -> Result<Option<String>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                "SELECT artifact_id FROM workspace_diff_artifacts
                 WHERE task_id = ?1 AND status_fingerprint = ?2
                   AND sequence > ?3 AND sequence <= ?4
                 ORDER BY sequence DESC LIMIT 1",
                params![
                    task_id.as_str(),
                    status_fingerprint,
                    to_i64(after_sequence)?,
                    to_i64(through_sequence)?
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|value| value.map(|artifact_id| format!("artifact:{artifact_id}")))
            .map_err(storage_error)
    }

    async fn task_events(
        &self,
        task_id: &TaskId,
        after_sequence: u64,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, event_id, stream_type, stream_id, stream_version,
                        event_type, schema_version, occurred_at, recorded_at,
                        correlation_id, causation_id, source_json, payload_json, extensions_json
                 FROM events
                 WHERE stream_type = 'task' AND stream_id = ?1
                   AND sequence > ?2 AND sequence <= ?3
                 ORDER BY sequence LIMIT ?4",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    task_id.as_str(),
                    after_sequence,
                    through_sequence,
                    to_i64(limit as u64)?
                ],
                row_to_event,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn recent_task_events(
        &self,
        task_id: &TaskId,
        after_sequence: u64,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, event_id, stream_type, stream_id, stream_version,
                        event_type, schema_version, occurred_at, recorded_at,
                        correlation_id, causation_id, source_json, payload_json, extensions_json
                 FROM events
                 WHERE stream_type = 'task' AND stream_id = ?1
                   AND sequence > ?2 AND sequence <= ?3
                 ORDER BY sequence DESC LIMIT ?4",
            )
            .map_err(storage_error)?;
        let mut events = statement
            .query_map(
                params![
                    task_id.as_str(),
                    after_sequence,
                    through_sequence,
                    to_i64(limit as u64)?
                ],
                row_to_event,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        events.reverse();
        Ok(events)
    }

    async fn installation_id(
        &self,
        provider_id: &str,
        executable: &str,
    ) -> Result<Option<InstallationId>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                "SELECT installation_id FROM installations
                 WHERE provider_id = ?1 AND executable = ?2",
                params![provider_id, executable],
                |row| parse_id(row.get::<_, String>(0)?),
            )
            .optional()
            .map_err(storage_error)
    }

    async fn artifact(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Option<StoredArtifact>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                "SELECT a.payload_sha256, a.media_type, b.byte_length,
                        CASE WHEN length(b.content) <= 1048576 THEN b.content ELSE NULL END,
                        a.redaction_ruleset_version, a.redaction_count, a.created_at
                 FROM artifacts a
                 JOIN artifact_blobs b ON b.payload_sha256 = a.payload_sha256
                 WHERE a.artifact_id = ?1",
                [artifact_id.as_str()],
                |row| {
                    Ok(StoredArtifact {
                        metadata: ArtifactMetadata {
                            artifact_id: artifact_id.clone(),
                            payload_sha256: row.get(0)?,
                            media_type: row.get(1)?,
                            byte_length: row.get(2)?,
                            redaction_ruleset_version: row.get(4)?,
                            redaction_count: row.get(5)?,
                            created_at: from_millis(row.get(6)?)?,
                        },
                        content: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(storage_error)
    }

    async fn task_artifacts(
        &self,
        task_id: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<Vec<TaskArtifactSummary>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT metadata_json, source_sequence, source_kind FROM task_artifacts
             WHERE task_id = ?1 AND (?2 IS NULL OR source_sequence < ?2
                 OR (source_sequence = ?2 AND artifact_id < ?3))
             ORDER BY source_sequence DESC, artifact_id DESC LIMIT ?4",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    task_id.as_str(),
                    before.map(|cursor| to_i64(cursor.sequence)).transpose()?,
                    before.map(|cursor| cursor.artifact_id.as_str()),
                    to_i64(limit as u64)?,
                ],
                row_to_task_artifact,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    async fn task_artifact(
        &self,
        task_id: &TaskId,
        artifact_id: &ArtifactId,
    ) -> Result<Option<TaskArtifactSummary>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                "SELECT metadata_json, source_sequence, source_kind FROM task_artifacts
             WHERE task_id = ?1 AND artifact_id = ?2",
                params![task_id.as_str(), artifact_id.as_str()],
                row_to_task_artifact,
            )
            .optional()
            .map_err(storage_error)
    }

    async fn pending_context_transfer(
        &self,
        task_id: &TaskId,
        target_executor: &ExecutorRef,
    ) -> Result<Option<PendingContextTransfer>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT artifact_id, kind, source_executor_json, target_executor_json,
                        source_after_sequence, source_through_sequence, confirmed_sequence, status
                 FROM context_transfers
                 WHERE task_id = ?1
                 ORDER BY confirmed_sequence DESC",
            )
            .map_err(storage_error)?;
        let transfers = statement
            .query_map([task_id.as_str()], |row| {
                let kind_json = format!("\"{}\"", row.get::<_, String>(1)?);
                Ok((
                    row.get::<_, String>(7)?,
                    PendingContextTransfer {
                        artifact_id: parse_id(row.get::<_, String>(0)?)?,
                        kind: serde_json::from_str::<ContextPackageKind>(&kind_json)
                            .map_err(sql_json_error)?,
                        source_executor: serde_json::from_str(&row.get::<_, String>(2)?)
                            .map_err(sql_json_error)?,
                        target_executor: serde_json::from_str(&row.get::<_, String>(3)?)
                            .map_err(sql_json_error)?,
                        source_after_sequence: row.get(4)?,
                        source_through_sequence: row.get(5)?,
                        confirmed_sequence: row.get(6)?,
                    },
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        Ok(transfers
            .into_iter()
            .find(|(_, transfer)| &transfer.target_executor == target_executor)
            .and_then(|(status, transfer)| {
                matches!(status.as_str(), "confirmed" | "failed").then_some(transfer)
            }))
    }

    async fn pending_reconciliation_opt_out(
        &self,
        task_id: &TaskId,
        target_executor: &ExecutorRef,
    ) -> Result<Option<PendingReconciliationOptOut>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let mut statement = connection
            .prepare(
                "SELECT source_executor_json, target_executor_json, source_after_sequence,
                        source_through_sequence, opted_out_sequence, status
                 FROM reconciliation_opt_outs
                 WHERE task_id = ?1
                 ORDER BY opted_out_sequence DESC",
            )
            .map_err(storage_error)?;
        let decisions = statement
            .query_map([task_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(5)?,
                    PendingReconciliationOptOut {
                        source_executor: serde_json::from_str(&row.get::<_, String>(0)?)
                            .map_err(sql_json_error)?,
                        target_executor: serde_json::from_str(&row.get::<_, String>(1)?)
                            .map_err(sql_json_error)?,
                        source_after_sequence: row.get(2)?,
                        source_through_sequence: row.get(3)?,
                        opted_out_sequence: row.get(4)?,
                    },
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        Ok(decisions
            .into_iter()
            .find(|(_, decision)| &decision.target_executor == target_executor)
            .and_then(|(status, decision)| {
                matches!(status.as_str(), "pending" | "failed").then_some(decision)
            }))
    }

    async fn last_dispatched_executor(
        &self,
        task_id: &TaskId,
    ) -> Result<Option<ExecutorRef>, RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        connection
            .query_row(
                // Every driver passes through this hub boundary, regardless of
                // whether its protocol emits a native TurnStarted notification.
                "SELECT json_extract(payload_json, '$.executor') FROM events
                 WHERE stream_type = 'task' AND stream_id = ?1
                   AND event_type = 'history.native_prompt_accepted'
                 ORDER BY sequence DESC LIMIT 1",
                [task_id.as_str()],
                |row| serde_json::from_str(&row.get::<_, String>(0)?).map_err(sql_json_error),
            )
            .optional()
            .map_err(storage_error)
    }
}

fn validate_artifact(artifact: &agent_core::NewArtifact) -> Result<(), RepositoryError> {
    const MAX_ARTIFACT_BYTES: usize = 1_048_576;
    if artifact.content.len() > MAX_ARTIFACT_BYTES
        || artifact.metadata.byte_length != artifact.content.len() as u64
    {
        return Err(RepositoryError::Unavailable(
            "artifact byte length is invalid".into(),
        ));
    }
    let digest = hex::encode(Sha256::digest(&artifact.content));
    if digest != artifact.metadata.payload_sha256 {
        return Err(RepositoryError::Unavailable(
            "artifact content does not match its content address".into(),
        ));
    }
    Ok(())
}

fn current_stream_version(
    transaction: &Transaction<'_>,
    stream: &StreamRef,
) -> Result<u64, RepositoryError> {
    transaction
        .query_row(
            "SELECT COALESCE(MAX(stream_version), 0) FROM events
             WHERE stream_type = ?1 AND stream_id = ?2",
            params![stream.stream_type.as_str(), stream.stream_id],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn apply_projection(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
) -> Result<(), RepositoryError> {
    match event.event_type.as_str() {
        "project.registered" => {
            let project: RegisterProject =
                serde_json::from_value(event.payload.clone()).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO projects(project_id, display_name, path, stream_version, additional_directories_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        project.project_id.to_string(),
                        project.display_name,
                        project.path,
                        to_i64(event.stream_version)?,
                        serde_json::to_string(&project.additional_directories).map_err(json_error)?,
                    ],
                )
                .map_err(storage_error)?;
        }
        "project.updated" => {
            let project: RegisterProject =
                serde_json::from_value(event.payload.clone()).map_err(json_error)?;
            let directories =
                serde_json::to_string(&project.additional_directories).map_err(json_error)?;
            // Native sessions retain their original working directories. Rebuild
            // them from retained conversation history before the next turn.
            transaction.execute(
                "UPDATE agent_session_maps SET status='rebuild_pending' WHERE task_id IN
                 (SELECT task_id FROM tasks WHERE project_id=?1) AND EXISTS
                 (SELECT 1 FROM projects WHERE project_id=?1 AND (path<>?2 OR additional_directories_json<>?3))",
                params![project.project_id.as_str(), project.path, directories],
            ).map_err(storage_error)?;
            transaction.execute(
                "UPDATE projects SET display_name=?2, path=?3, additional_directories_json=?4, stream_version=?5 WHERE project_id=?1",
                params![project.project_id.as_str(), project.display_name, project.path, directories, to_i64(event.stream_version)?],
            ).map_err(storage_error)?;
        }
        "task.forked" => {
            transaction.execute("INSERT INTO timeline_items(item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at) VALUES (?1, ?2, NULL, 'fork_context', ?3, NULL, ?4, ?5)",
                params![event.event_id.to_string(), payload_string(event, "taskId")?, event.payload.to_string(), to_i64(event.sequence)?, event.occurred_at.timestamp_millis()]).map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "review.updated" => {
            let review = &event.payload["review"];
            let status = review["status"].as_str();
            let retained = review["result"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
                || review["report"]["findings"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty());
            if status == Some("completed")
                || (retained && matches!(status, Some("failed" | "cancelled" | "interrupted")))
            {
                transaction.execute("INSERT INTO timeline_items(item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at) VALUES (?1, ?2, ?3, 'review_result', ?4, ?5, ?6, ?7)",
                    params![event.event_id.to_string(), payload_string(event, "taskId")?, event.payload.get("turnId").and_then(Value::as_str), event.payload.to_string(), event.payload.pointer("/review/request/executor/providerId").and_then(Value::as_str), to_i64(event.sequence)?, event.occurred_at.timestamp_millis()]).map_err(storage_error)?;
            }
            update_task_stream_version(transaction, event)?;
        }
        "task.created" => {
            let task: CreateTask =
                serde_json::from_value(event.payload.clone()).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO tasks(
                        task_id, project_id, title, preferred_executor_json, stream_version
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        task.task_id.to_string(),
                        task.project_id.to_string(),
                        task.title,
                        serde_json::to_string(&task.preferred_executor).map_err(json_error)?,
                        to_i64(event.stream_version)?,
                    ],
                )
                .map_err(storage_error)?;
        }
        "handoff.package_confirmed" | "reconciliation.package_confirmed" => {
            let task_id = payload_string(event, "taskId")?;
            let kind = payload_string(event, "kind")?;
            let source = event
                .payload
                .pointer("/sourceExecutor/providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing source executor".into()))?;
            let target = event
                .payload
                .pointer("/targetExecutor/providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            let artifact_id = event
                .payload
                .pointer("/artifact/artifactId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing artifact id".into()))?;
            let source_executor = event
                .payload
                .get("sourceExecutor")
                .ok_or_else(|| RepositoryError::Unavailable("missing source executor".into()))?;
            let target_executor = event
                .payload
                .get("targetExecutor")
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            transaction
                .execute(
                    "INSERT INTO context_transfers(
                        artifact_id, task_id, kind, source_executor_json, target_executor_json,
                        source_after_sequence, source_through_sequence, status,
                        confirmed_sequence, last_event_sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'confirmed', ?8, ?8)",
                    params![
                        artifact_id,
                        task_id,
                        kind,
                        serde_json::to_string(source_executor).map_err(json_error)?,
                        serde_json::to_string(target_executor).map_err(json_error)?,
                        payload_u64(event, "sourceAfterSequence")?,
                        payload_u64(event, "sourceThroughSequence")?,
                        to_i64(event.sequence)?,
                    ],
                )
                .map_err(storage_error)?;
            if let Some(parent_artifact_id) = event
                .payload
                .get("parentArtifactId")
                .and_then(serde_json::Value::as_str)
            {
                transaction
                    .execute(
                        "INSERT INTO artifact_versions(
                            artifact_id, parent_artifact_id, relation, created_sequence
                         ) VALUES (?1, ?2, 'user_edit', ?3)",
                        params![artifact_id, parent_artifact_id, to_i64(event.sequence)?],
                    )
                    .map_err(storage_error)?;
            }
            let timeline_kind = if event.event_type.starts_with("handoff.") {
                "handoff"
            } else {
                "reconciliation"
            };
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        format!("{timeline_kind}-{}", event.event_id),
                        task_id,
                        timeline_kind,
                        format!("Confirmed {source} → {target} · {artifact_id}"),
                        target,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
        }
        "reconciliation.opted_out" => {
            let task_id = payload_string(event, "taskId")?;
            let source_executor = event
                .payload
                .get("sourceExecutor")
                .ok_or_else(|| RepositoryError::Unavailable("missing source executor".into()))?;
            let target_executor = event
                .payload
                .get("targetExecutor")
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            let source = source_executor
                .get("providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing source provider".into()))?;
            let target = target_executor
                .get("providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing target provider".into()))?;
            transaction
                .execute(
                    "INSERT INTO reconciliation_opt_outs(
                        task_id, source_executor_json, target_executor_json,
                        source_after_sequence, source_through_sequence, preview_payload_sha256,
                        status, opted_out_sequence, last_event_sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
                    params![
                        task_id,
                        serde_json::to_string(source_executor).map_err(json_error)?,
                        serde_json::to_string(target_executor).map_err(json_error)?,
                        payload_u64(event, "sourceAfterSequence")?,
                        payload_u64(event, "sourceThroughSequence")?,
                        payload_string(event, "previewPayloadSha256")?,
                        to_i64(event.sequence)?,
                    ],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, NULL, 'reconciliation', ?3, ?4, ?5, ?6)",
                    params![
                        event.event_id.to_string(),
                        task_id,
                        format!("Opted out of reconciliation {source} → {target}"),
                        target,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "reconciliation.opt_out_applied" | "reconciliation.opt_out_failed" => {
            let applied = event.event_type == "reconciliation.opt_out_applied";
            let status = if applied { "applied" } else { "failed" };
            let opted_out_sequence = payload_u64(event, "optedOutSequence")?;
            transaction
                .execute(
                    "UPDATE reconciliation_opt_outs SET status = ?1,
                        last_event_sequence = ?2, session_map_id = ?3, failure = ?4
                     WHERE opted_out_sequence = ?5",
                    params![
                        status,
                        to_i64(event.sequence)?,
                        event
                            .payload
                            .get("sessionMapId")
                            .and_then(serde_json::Value::as_str),
                        event
                            .payload
                            .get("reason")
                            .and_then(serde_json::Value::as_str),
                        to_i64(opted_out_sequence)?,
                    ],
                )
                .map_err(storage_error)?;
            let target = event
                .payload
                .pointer("/targetExecutor/providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, ?3, 'reconciliation', ?4, ?5, ?6, ?7)",
                    params![
                        event.event_id.to_string(),
                        payload_string(event, "taskId")?,
                        event
                            .payload
                            .get("turnId")
                            .and_then(serde_json::Value::as_str),
                        if applied {
                            "Reconciliation opt-out applied".to_string()
                        } else {
                            format!(
                                "Reconciliation opt-out failed · {}",
                                event
                                    .payload
                                    .get("reason")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("unknown")
                            )
                        },
                        target,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "handoff.sent" | "reconciliation.sent" => {
            let artifact_id = payload_string(event, "artifactId")?;
            transaction
                .execute(
                    "UPDATE context_transfers SET status = 'sent', last_event_sequence = ?1,
                        session_map_id = ?2, failure = NULL WHERE artifact_id = ?3",
                    params![
                        to_i64(event.sequence)?,
                        payload_string(event, "sessionMapId")?,
                        artifact_id,
                    ],
                )
                .map_err(storage_error)?;
            let kind = if event.event_type.starts_with("handoff.") {
                "handoff"
            } else {
                "reconciliation"
            };
            let target = event
                .payload
                .pointer("/targetExecutor/providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        event.event_id.to_string(),
                        payload_string(event, "taskId")?,
                        payload_string(event, "turnId")?,
                        kind,
                        format!("Sent {kind} · {artifact_id}"),
                        target,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "handoff.failed" | "reconciliation.failed" => {
            let artifact_id = payload_string(event, "artifactId")?;
            transaction
                .execute(
                    "UPDATE context_transfers SET status = 'failed', last_event_sequence = ?1,
                        session_map_id = ?2, failure = ?3 WHERE artifact_id = ?4",
                    params![
                        to_i64(event.sequence)?,
                        event
                            .payload
                            .get("sessionMapId")
                            .and_then(serde_json::Value::as_str),
                        payload_string(event, "reason")?,
                        artifact_id,
                    ],
                )
                .map_err(storage_error)?;
            let kind = if event.event_type.starts_with("handoff.") {
                "handoff"
            } else {
                "reconciliation"
            };
            let target = event
                .payload
                .pointer("/targetExecutor/providerId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| RepositoryError::Unavailable("missing target executor".into()))?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        event.event_id.to_string(),
                        payload_string(event, "taskId")?,
                        kind,
                        format!(
                            "Failed {kind}: {} · {artifact_id}",
                            payload_string(event, "reason")?
                        ),
                        target,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "task.deleted" if event.payload["permanent"] == true => {
            task_lifecycle::purge(transaction, &event.stream.stream_id, event.sequence)?;
        }
        "task.deleted" | "task.restored" => {
            update_task_stream_version(transaction, event)?;
        }
        "task.renamed" => {
            transaction
                .execute(
                    "UPDATE tasks SET title = ?1, stream_version = ?2 WHERE task_id = ?3",
                    params![
                        payload_string(event, "title")?,
                        to_i64(event.stream_version)?,
                        event.stream.stream_id
                    ],
                )
                .map_err(storage_error)?;
        }
        "task.preferred_executor_changed" => {
            let executor: ExecutorRef = serde_json::from_value(
                event
                    .payload
                    .get("executor")
                    .cloned()
                    .ok_or_else(|| RepositoryError::Unavailable("missing executor".into()))?,
            )
            .map_err(json_error)?;
            transaction
                .execute(
                    "UPDATE tasks SET preferred_executor_json = ?1, stream_version = ?2
                     WHERE task_id = ?3",
                    params![
                        serde_json::to_string(&executor).map_err(json_error)?,
                        to_i64(event.stream_version)?,
                        event.stream.stream_id,
                    ],
                )
                .map_err(storage_error)?;
        }
        "agent.probe_completed" => {
            let installation: InstallationSummary =
                serde_json::from_value(event.payload.clone()).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO installations(
                        installation_id, provider_id, executable, fingerprint, status,
                        version, version_text, diagnostic, probe_elapsed_millis, probed_at,
                        stream_version
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(provider_id, executable) DO UPDATE SET
                        fingerprint = excluded.fingerprint,
                        status = excluded.status,
                        version = excluded.version,
                        version_text = excluded.version_text,
                        diagnostic = excluded.diagnostic,
                        probe_elapsed_millis = excluded.probe_elapsed_millis,
                        probed_at = excluded.probed_at,
                        stream_version = excluded.stream_version",
                    params![
                        installation.installation_id.to_string(),
                        installation.provider_id,
                        installation.executable,
                        installation.fingerprint,
                        serde_json::to_value(installation.status)
                            .map_err(json_error)?
                            .as_str()
                            .unwrap_or("broken"),
                        installation.version,
                        installation.version_text,
                        installation.diagnostic,
                        to_i64(installation.probe_elapsed_millis)?,
                        installation.probed_at.timestamp_millis(),
                        to_i64(event.stream_version)?,
                    ],
                )
                .map_err(storage_error)?;
        }
        "agent.installation_removed" => {
            let installation_id = payload_string(event, "installationId")?;
            transaction
                .execute(
                    "DELETE FROM installations WHERE installation_id = ?1",
                    [installation_id],
                )
                .map_err(storage_error)?;
        }
        "turn.queued" | "turn.autonomous.started" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let run_id = payload_string(event, "runId")?;
            let autonomous = event.event_type == "turn.autonomous.started";
            let prompt = if autonomous {
                String::new()
            } else {
                payload_string(event, "prompt")?
            };
            let executor = event
                .payload
                .get("executor")
                .ok_or_else(|| RepositoryError::Unavailable("missing executor".into()))?;
            let started_at = parse_payload_time(event, "startedAt")?;
            transaction
                .execute(
                    "UPDATE tasks SET active_run_id = ?1, active_turn_id = ?2,
                        active_executor_json = ?3, active_run_state = ?7,
                        active_started_at = ?4, stream_version = ?5 WHERE task_id = ?6",
                    params![
                        run_id,
                        turn_id,
                        serde_json::to_string(executor).map_err(json_error)?,
                        started_at.timestamp_millis(),
                        to_i64(event.stream_version)?,
                        task_id,
                        if autonomous { "running" } else { "queued" },
                    ],
                )
                .map_err(storage_error)?;
            if !autonomous {
                transaction
                    .execute(
                        "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, ?3, 'user_message', ?4, NULL, ?5, ?6)",
                        params![
                            format!("user-{turn_id}"),
                            task_id,
                            turn_id,
                            prompt,
                            to_i64(event.sequence)?,
                            event.occurred_at.timestamp_millis(),
                        ],
                    )
                    .map_err(storage_error)?;
            }
        }
        "turn.connection_lost" => {
            // Close only the matching local turn. This is not an external execution result.
            let matches: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE task_id=?1 AND active_run_id=?2 AND active_turn_id=?3)",
                params![payload_string(event, "taskId")?, payload_string(event, "runId")?, payload_string(event, "turnId")?],
                |row| row.get(0),
            ).map_err(storage_error)?;
            if matches {
                update_active_session_checkpoint(transaction, event, "detached")?;
                clear_active_run(transaction, event)?;
                transaction.execute(
                    "INSERT INTO timeline_items(item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at) VALUES (?1, ?2, ?3, 'connection.interrupted', ?4, NULL, ?5, ?6)",
                    params![event.event_id.to_string(), payload_string(event,"taskId")?, payload_string(event,"turnId")?,
                        "上次执行连接已中断，可以直接发送消息继续。Agent 将结合已有历史检查当前结果。", to_i64(event.sequence)?, event.occurred_at.timestamp_millis()],
                ).map_err(storage_error)?;
            } else {
                update_task_stream_version(transaction, event)?;
            }
        }
        "turn.started" => update_run_state(transaction, event, "running")?,
        "turn.cancel_requested" | "turn.interrupt_sent" => {
            update_matching_run_state(transaction, event, "interrupting")?;
        }
        "turn.cancel_failed" => update_matching_run_state(transaction, event, "running")?,
        "session.resumed" => {
            let session_map_id = payload_string(event, "sessionMapId")?;
            let native_session_id = payload_string(event, "nativeSessionId")?;
            let resumed_at = parse_payload_time(event, "resumedAt")?;
            transaction
                .execute(
                    "UPDATE agent_session_maps SET native_session_id = ?1, last_used_at = ?2,
                        resume_capability = ?3, status = 'active', last_seen_sequence = ?5,
                        model = ?6, thinking = ?7
                     WHERE session_map_id = ?4",
                    params![
                        native_session_id,
                        resumed_at.timestamp_millis(),
                        payload_string(event, "resumeCapability")?,
                        session_map_id,
                        event.sequence,
                        event
                            .payload
                            .get("model")
                            .and_then(serde_json::Value::as_str),
                        event
                            .payload
                            .get("thinking")
                            .and_then(serde_json::Value::as_str),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "driver.session_ready" => {
            // A driver may learn its durable native ID only after the first prompt.
            // Bind it to the active turn and exact installation, never another session.
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let native_id = payload_string(event, "nativeSessionId")?;
            if let Some(provider) = driver_provider(event) {
                transaction.execute(
                    "UPDATE agent_session_maps SET native_session_id = ?1
                     WHERE task_id = ?2 AND provider_id = ?4 AND status = 'active'
                     AND EXISTS (SELECT 1 FROM tasks t WHERE t.task_id = ?2
                       AND t.active_turn_id = ?3 AND t.active_run_id IS NOT NULL
                       AND json_extract(t.active_executor_json, '$.providerId') = provider_id
                       AND COALESCE(json_extract(t.active_executor_json, '$.installationId'), '') = installation_key)",
                    params![native_id, task_id, turn_id, provider],
                ).map_err(storage_error)?;
            }
        }
        "session.ready" => {
            let task_id = payload_string(event, "taskId")?;
            let session_map_id = payload_string(event, "sessionMapId")?;
            let executor: ExecutorRef = serde_json::from_value(
                event
                    .payload
                    .get("executor")
                    .cloned()
                    .ok_or_else(|| RepositoryError::Unavailable("missing executor".into()))?,
            )
            .map_err(json_error)?;
            let native_session_id = payload_string(event, "nativeSessionId")?;
            let workspace_locator = payload_string(event, "workspaceLocator")?;
            let created_at = parse_payload_time(event, "createdAt")?;
            let resume_capability = payload_string(event, "resumeCapability")?;
            let status = payload_string(event, "status")?;
            let installation_key = executor.installation_id.clone().unwrap_or_default();
            transaction
                .execute(
                    "INSERT INTO agent_session_maps(
                        session_map_id, task_id, provider_id, installation_id, installation_key,
                        native_session_id, last_native_turn_id, workspace_locator, driver_version,
                        native_agent_version, created_at, last_used_at, resume_capability, status,
                        last_seen_sequence, model, thinking
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13, ?14, ?15)
                     ON CONFLICT(task_id, provider_id, installation_key) DO UPDATE SET
                        session_map_id = excluded.session_map_id,
                        installation_id = excluded.installation_id,
                        native_session_id = excluded.native_session_id,
                        last_native_turn_id = NULL,
                        workspace_locator = excluded.workspace_locator,
                        driver_version = excluded.driver_version,
                        native_agent_version = excluded.native_agent_version,
                        model = excluded.model,
                        thinking = excluded.thinking,
                        created_at = excluded.created_at,
                        last_used_at = excluded.last_used_at,
                        resume_capability = excluded.resume_capability,
                        status = excluded.status,
                        last_seen_sequence = excluded.last_seen_sequence",
                    params![
                        session_map_id,
                        task_id,
                        executor.provider_id.to_string(),
                        executor.installation_id,
                        installation_key,
                        native_session_id,
                        workspace_locator,
                        event
                            .payload
                            .get("driverVersion")
                            .and_then(serde_json::Value::as_str),
                        event
                            .payload
                            .get("nativeAgentVersion")
                            .and_then(serde_json::Value::as_str),
                        created_at.timestamp_millis(),
                        resume_capability,
                        status,
                        to_i64(event.sequence)?,
                        event.payload.get("model").and_then(serde_json::Value::as_str),
                        event.payload.get("thinking").and_then(serde_json::Value::as_str),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "session.rebuild_requested" => {
            let id = payload_string(event, "sessionMapId")?;
            transaction.execute("UPDATE agent_session_maps SET status='rebuild_pending' WHERE session_map_id=?1",[id]).map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "session.detached" => {
            let session_map_id = payload_string(event, "sessionMapId")?;
            let detached_at = parse_payload_time(event, "detachedAt")?;
            transaction
                .execute(
                    "UPDATE agent_session_maps SET status = 'detached', last_used_at = ?1
                     WHERE session_map_id = ?2",
                    params![detached_at.timestamp_millis(), session_map_id],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "session.missing" | "session.incompatible" => {
            let session_map_id = payload_string(event, "sessionMapId")?;
            let (status, timestamp_field) = if event.event_type == "session.missing" {
                ("missing", "missingAt")
            } else {
                ("incompatible", "incompatibleAt")
            };
            let recorded_at = parse_payload_time(event, timestamp_field)?;
            transaction
                .execute(
                    "UPDATE agent_session_maps SET status = ?1, last_used_at = ?2
                     WHERE session_map_id = ?3",
                    params![status, recorded_at.timestamp_millis(), session_map_id],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "session.resumable" => {
            let session_map_id = payload_string(event, "sessionMapId")?;
            let resumable_at = parse_payload_time(event, "resumableAt")?;
            transaction
                .execute(
                    "UPDATE agent_session_maps SET status = 'resumable', last_used_at = ?1
                     WHERE session_map_id = ?2",
                    params![resumable_at.timestamp_millis(), session_map_id],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "driver.turn_started" => {
            let task_id = payload_string(event, "taskId")?;
            if let Some(provider_id) = driver_provider(event) {
                transaction
                    .execute(
                        "UPDATE agent_session_maps SET last_native_turn_id = ?1,
                            last_used_at = ?2, status = 'active'
                         WHERE task_id = ?3 AND provider_id = ?4 AND status = 'active'",
                        params![
                            event
                                .payload
                                .get("nativeTurnId")
                                .and_then(serde_json::Value::as_str),
                            event.occurred_at.timestamp_millis(),
                            task_id,
                            provider_id,
                        ],
                    )
                    .map_err(storage_error)?;
            }
            update_task_stream_version(transaction, event)?;
        }
        "activity.updated" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let native = payload_string(event, "itemId")?;
            let id = format!("tool:{task_id}:{turn_id}:{native}");
            let old: Option<String> = transaction
                .query_row(
                    "SELECT text FROM timeline_items WHERE item_id=?1 AND kind='activity'",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            let mut data = old
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if let Some(patch) = event
                .payload
                .get("data")
                .and_then(serde_json::Value::as_object)
            {
                for (k, v) in patch {
                    if v.is_null() {
                        continue;
                    }
                    if k == "type" && data[k] == "subagent" && v == "tool" {
                        continue;
                    }
                    if k == "parentId" && v == "" && data[k].as_str().is_some_and(|s| !s.is_empty())
                    {
                        continue;
                    }
                    if k == "textDelta" {
                        let text = format!(
                            "{}{}",
                            data["text"].as_str().unwrap_or(""),
                            v.as_str().unwrap_or("")
                        );
                        data["text"] = serde_json::json!(text);
                        continue;
                    }
                    if k == "status"
                        && ["completed", "failed", "interrupted", "cancelled"]
                            .contains(&data[k].as_str().unwrap_or(""))
                        && ["pending", "running", "in_progress"].contains(&v.as_str().unwrap_or(""))
                    {
                        continue;
                    }
                    if k == "startedAt" && !data[k].is_null() {
                        continue;
                    }
                    data[k] = v.clone();
                }
            }
            transaction.execute("INSERT INTO timeline_items(item_id,task_id,turn_id,kind,text,provider_id,sequence,occurred_at) VALUES(?1,?2,?3,'activity',?4,?5,?6,?7) ON CONFLICT(item_id) DO UPDATE SET kind='activity',text=excluded.text",params![id,task_id,turn_id,data.to_string(),driver_provider(event),to_i64(event.sequence)?,event.occurred_at.timestamp_millis()]).map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "message.delta" => {
            let native_item_id = payload_string(event, "itemId")?;
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let item_id = format!("message:{task_id}:{turn_id}:{native_item_id}");
            let text = payload_string(event, "text")?;
            let provider = driver_provider(event);
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, ?3, 'assistant_message', ?4, ?5, ?6, ?7)
                     ON CONFLICT(item_id) DO UPDATE SET
                        text = timeline_items.text || excluded.text",
                    params![
                        item_id,
                        task_id,
                        turn_id,
                        text,
                        provider,
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "tool.started" | "tool.completed" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let tool_name = payload_string(event, "toolName")?;
            let text = match event
                .payload
                .get("summary")
                .and_then(|value| value.as_str())
            {
                Some(summary) => summary.to_owned(),
                None => match event.event_type.as_str() {
                    "tool.started" => format!("Started {tool_name}"),
                    "tool.completed" => format!("Completed {tool_name}"),
                    _ => unreachable!("tool projection only handles tool lifecycle events"),
                },
            };
            // Native item IDs collapse start/completion into one activity row.
            // Drivers may omit itemId; those events remain separate rows.
            let item_id = match event.payload.get("itemId").and_then(|value| value.as_str()) {
                Some(item_id) => format!("tool:{task_id}:{turn_id}:{item_id}"),
                None => event.event_id.to_string(),
            };
            let timeline_kind = if event.event_type == "tool.completed"
                && event
                    .payload
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
            {
                "tool.failed"
            } else {
                event.event_type.as_str()
            };
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(item_id) DO UPDATE SET
                        kind = excluded.kind,
                        text = CASE
                            WHEN timeline_items.kind = 'tool.started'
                             AND excluded.kind IN ('tool.completed', 'tool.failed')
                            THEN timeline_items.text
                            ELSE excluded.text
                        END,
                        provider_id = excluded.provider_id
                        WHERE timeline_items.kind != 'activity'",
                    params![
                        item_id,
                        task_id,
                        turn_id,
                        timeline_kind,
                        text,
                        driver_provider(event),
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "driver.protocol_warning" | "driver.unknown_event_observed" => {
            // Driver diagnostics remain in the canonical event log for support and
            // audit purposes, but they are not conversation content. Keeping them
            // out of the Timeline also prevents raw diagnostic JSON from being
            // rendered as repeated assistant messages.
            update_task_stream_version(transaction, event)?;
        }
        "workspace.operation_changes_captured" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let operation = payload_string(event, "operationId")?;
            let id = format!("tool:{task_id}:{turn_id}:{operation}");
            let old: Option<String> = transaction
                .query_row(
                    "SELECT text FROM timeline_items WHERE item_id=?1 AND kind='activity'",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            let mut data = old
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or_else(
                    || serde_json::json!({"type":"tool","name":"edit","status":"completed"}),
                );
            data["artifact"] = event.payload["artifact"].clone();
            if data["parentId"].is_null() {
                data["parentId"] = event.payload["parentId"].clone();
            }
            if data["sessionId"].is_null() {
                data["sessionId"] = event.payload["sessionId"].clone();
            }
            transaction.execute("INSERT INTO timeline_items(item_id,task_id,turn_id,kind,text,provider_id,sequence,occurred_at) VALUES(?1,?2,?3,'activity',?4,NULL,?5,?6) ON CONFLICT(item_id) DO UPDATE SET kind='activity',text=excluded.text",params![id,task_id,turn_id,data.to_string(),to_i64(event.sequence)?,event.occurred_at.timestamp_millis()]).map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "workspace.turn_changes_captured" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            transaction.execute("INSERT OR IGNORE INTO timeline_items(item_id,task_id,turn_id,kind,text,provider_id,sequence,occurred_at) VALUES(?1,?2,?3,'turn_files',?4,NULL,?5,?6)",params![event.event_id.to_string(),task_id,turn_id,event.payload.get("artifact").unwrap_or(&serde_json::Value::Null).to_string(),to_i64(event.sequence)?,event.occurred_at.timestamp_millis()]).map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "workspace.snapshot_captured" | "workspace.snapshot_failed" => {
            let task_id = payload_string(event, "taskId")?;
            let turn_id = payload_string(event, "turnId")?;
            let run_id = payload_string(event, "runId")?;
            let phase = serde_json::from_value::<WorkspaceSnapshotPhase>(
                event
                    .payload
                    .get("phase")
                    .cloned()
                    .ok_or_else(|| missing_payload_field(event, "phase"))?,
            )
            .map_err(json_error)?;
            let captured = event.event_type == "workspace.snapshot_captured";
            let (git_json, change_summary_json, relevant_file_count, dependency_change_count) =
                if captured {
                    let git = serde_json::from_value::<GitWorkspaceSnapshotV1>(
                        event
                            .payload
                            .get("git")
                            .cloned()
                            .ok_or_else(|| missing_payload_field(event, "git"))?,
                    )
                    .map_err(json_error)?;
                    let change_summary = serde_json::from_value::<ChangeSummary>(
                        event
                            .payload
                            .get("changeSummary")
                            .cloned()
                            .ok_or_else(|| missing_payload_field(event, "changeSummary"))?,
                    )
                    .map_err(json_error)?;
                    let relevant_file_count = event
                        .payload
                        .get("relevantFiles")
                        .and_then(serde_json::Value::as_array)
                        .ok_or_else(|| missing_payload_field(event, "relevantFiles"))?
                        .len();
                    let dependency_change_count = event
                        .payload
                        .get("dependencyChanges")
                        .and_then(serde_json::Value::as_array)
                        .ok_or_else(|| missing_payload_field(event, "dependencyChanges"))?
                        .len();
                    (
                        Some(serde_json::to_string(&git).map_err(json_error)?),
                        Some(serde_json::to_string(&change_summary).map_err(json_error)?),
                        relevant_file_count,
                        dependency_change_count,
                    )
                } else {
                    (None, None, 0, 0)
                };
            let diagnostic_json = if captured {
                None
            } else {
                let diagnostic = serde_json::from_value::<WorkspaceDiagnosticKind>(
                    event
                        .payload
                        .get("failureKind")
                        .cloned()
                        .ok_or_else(|| missing_payload_field(event, "failureKind"))?,
                )
                .map_err(json_error)?;
                Some(serde_json::to_string(&diagnostic).map_err(json_error)?)
            };
            transaction
                .execute(
                    "INSERT INTO workspace_snapshots(
                        sequence, task_id, turn_id, run_id, phase_json, captured_at,
                        git_json, change_summary_json, relevant_file_count,
                        dependency_change_count, diagnostic_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        to_i64(event.sequence)?,
                        task_id,
                        turn_id,
                        run_id,
                        serde_json::to_string(&phase).map_err(json_error)?,
                        event.occurred_at.timestamp_millis(),
                        git_json,
                        change_summary_json,
                        to_i64(relevant_file_count as u64)?,
                        to_i64(dependency_change_count as u64)?,
                        diagnostic_json,
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "workspace.diff_captured" => {
            let task_id = payload_string(event, "taskId")?;
            let base = payload_string(event, "base")?;
            let status_fingerprint = payload_string(event, "statusFingerprint")?;
            let artifact_id = event
                .payload
                .get("artifact")
                .and_then(|artifact| artifact.get("artifactId"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| missing_payload_field(event, "artifact.artifactId"))?;
            transaction
                .execute(
                    "INSERT INTO workspace_diff_artifacts(
                        artifact_id, task_id, status_fingerprint, base, sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        artifact_id,
                        task_id,
                        status_fingerprint,
                        base,
                        to_i64(event.sequence)?,
                    ],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, NULL, 'diff', ?3, NULL, ?4, ?5)",
                    params![
                        event.event_id.to_string(),
                        task_id,
                        format!("Workspace diff captured · artifact:{artifact_id}"),
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "test.completed" => {
            let task_id = payload_string(event, "taskId")?;
            let command = payload_string(event, "command")?;
            let outcome = payload_string(event, "outcome")?;
            transaction
                .execute(
                    "INSERT INTO timeline_items(
                        item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                     ) VALUES (?1, ?2, NULL, 'test_result', ?3, NULL, ?4, ?5)",
                    params![
                        event.event_id.to_string(),
                        task_id,
                        format!("{command} · {outcome}"),
                        to_i64(event.sequence)?,
                        event.occurred_at.timestamp_millis(),
                    ],
                )
                .map_err(storage_error)?;
            update_task_stream_version(transaction, event)?;
        }
        "turn.completed" => {
            let task_id = payload_string(event, "taskId")?;
            let run = transaction
                .query_row(
                    "SELECT active_run_id, active_executor_json FROM tasks WHERE task_id = ?1",
                    [&task_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(storage_error)?;
            let executor: ExecutorRef = serde_json::from_str(&run.1).map_err(json_error)?;
            let completed_at = parse_payload_time(event, "completedAt")?;
            transaction
                .execute(
                    "UPDATE agent_session_maps SET last_seen_sequence = ?1, last_used_at = ?2
                     WHERE task_id = ?3 AND provider_id = ?4 AND status = 'active'",
                    params![
                        to_i64(event.sequence)?,
                        completed_at.timestamp_millis(),
                        task_id,
                        executor.provider_id.to_string(),
                    ],
                )
                .map_err(storage_error)?;
            clear_active_run(transaction, event)?;
            transaction
                .execute("DELETE FROM recent_completion", [])
                .map_err(storage_error)?;
            transaction
                .execute(
                    "INSERT INTO recent_completion(task_id, run_id, provider_id, completed_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        task_id,
                        run.0,
                        executor.provider_id.to_string(),
                        completed_at.timestamp_millis()
                    ],
                )
                .map_err(storage_error)?;
        }
        "turn.failed" => {
            if event.payload.get("failureKind").and_then(Value::as_str) == Some("quota_exhausted") {
                let task_id = payload_string(event, "taskId")?;
                let turn_id = payload_string(event, "turnId")?;
                let provider_id = event
                    .payload
                    .pointer("/executor/providerId")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let reset_at = event
                    .payload
                    .get("resetsAtUnix")
                    .and_then(Value::as_i64)
                    .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0));
                let text = reset_at.map_or_else(
                    || "Coding Agent 额度已用尽；请等待额度恢复或切换 Agent。".to_owned(),
                    |reset_at| {
                        format!(
                            "Coding Agent 额度已用尽；预计于 {} 重置，也可以切换 Agent。",
                            reset_at.format("%Y-%m-%d %H:%M UTC")
                        )
                    },
                );
                transaction
                    .execute(
                        "INSERT INTO timeline_items(
                            item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at
                         ) VALUES (?1, ?2, ?3, 'quota.exhausted', ?4, ?5, ?6, ?7)",
                        params![
                            event.event_id.to_string(),
                            task_id,
                            turn_id,
                            text,
                            provider_id,
                            to_i64(event.sequence)?,
                            event.occurred_at.timestamp_millis(),
                        ],
                    )
                    .map_err(storage_error)?;
            }
            if event.payload.get("failureKind").and_then(Value::as_str) != Some("quota_exhausted") {
                let text = driver_api::failure::turn_failure_text(
                    event
                        .payload
                        .get("failureKind")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    event.payload.get("agentError").and_then(Value::as_str),
                );
                transaction.execute(
                    "INSERT INTO timeline_items(item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at)
                     VALUES (?1, ?2, ?3, 'turn.failed', ?4, ?5, ?6, ?7)",
                    params![event.event_id.to_string(), payload_string(event, "taskId")?,
                        payload_string(event, "turnId")?, text,
                        event.payload.pointer("/executor/providerId").and_then(Value::as_str).or_else(|| driver_provider(event)),
                        to_i64(event.sequence)?, event.occurred_at.timestamp_millis()],
                ).map_err(storage_error)?;
            }
            let status = if event.payload["sessionReusable"] == true
                && matches!(
                    event.payload["failureKind"].as_str(),
                    Some("interrupted" | "autonomous_turn_busy" | "selection_rejected")
                ) {
                "active"
            } else {
                "detached"
            };
            update_active_session_checkpoint(transaction, event, status)?;
            clear_active_run(transaction, event)?;
        }
        _ => {
            if event.stream.stream_type == StreamType::Task {
                update_task_stream_version(transaction, event)?;
            }
        }
    }
    project_task_artifacts(transaction, event)
}

fn row_to_task_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskArtifactSummary> {
    Ok(TaskArtifactSummary {
        metadata: serde_json::from_str(&row.get::<_, String>(0)?).map_err(sql_json_error)?,
        source_sequence: row.get(1)?,
        source_kind: row.get(2)?,
    })
}

fn project_task_artifacts(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
) -> Result<(), RepositoryError> {
    if !matches!(
        event.event_type.as_str(),
        "handoff.package_confirmed"
            | "reconciliation.package_confirmed"
            | "workspace.diff_captured"
            | "workspace.turn_changes_captured"
            | "workspace.operation_changes_captured"
            | "session.rebuild_context_prepared"
            | "test.completed"
    ) {
        return Ok(());
    }
    let task_id = payload_string(event, "taskId")?;
    if event.stream.stream_type != StreamType::Task || event.stream.stream_id != task_id {
        return Err(RepositoryError::Unavailable(
            "artifact event does not match its task stream".into(),
        ));
    }
    for field in ["artifact", "originalArtifact"] {
        let Some(value) = event.payload.get(field) else {
            if field == "artifact" {
                return Err(missing_payload_field(event, field));
            }
            continue;
        };
        let metadata: ArtifactMetadata =
            serde_json::from_value(value.clone()).map_err(json_error)?;
        let source_kind = if field == "originalArtifact" {
            format!("{}.original", event.event_type)
        } else {
            event.event_type.clone()
        };
        transaction.execute(
            "INSERT INTO task_artifacts(task_id, artifact_id, source_sequence, source_kind, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(task_id, artifact_id) DO NOTHING",
            params![task_id, metadata.artifact_id.as_str(), to_i64(event.sequence)?,
                source_kind, serde_json::to_string(&metadata).map_err(json_error)?],
        ).map_err(storage_error)?;
    }
    Ok(())
}

fn update_run_state(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
    state: &str,
) -> Result<(), RepositoryError> {
    transaction
        .execute(
            "UPDATE tasks SET active_run_state = ?1, stream_version = ?2 WHERE task_id = ?3",
            params![state, to_i64(event.stream_version)?, event.stream.stream_id],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn update_matching_run_state(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
    state: &str,
) -> Result<(), RepositoryError> {
    let task_id = payload_string(event, "taskId")?;
    let turn_id = payload_string(event, "turnId")?;
    let run_id = payload_string(event, "runId")?;
    transaction
        .execute(
            "UPDATE tasks SET active_run_state = CASE
                WHEN active_run_id = ?1 AND active_turn_id = ?2 THEN ?3
                ELSE active_run_state END,
                stream_version = ?4 WHERE task_id = ?5",
            params![
                run_id,
                turn_id,
                state,
                to_i64(event.stream_version)?,
                task_id
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn update_task_stream_version(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
) -> Result<(), RepositoryError> {
    transaction
        .execute(
            "UPDATE tasks SET stream_version = ?1 WHERE task_id = ?2",
            params![to_i64(event.stream_version)?, event.stream.stream_id],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn clear_active_run(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
) -> Result<(), RepositoryError> {
    transaction
        .execute(
            "UPDATE tasks SET active_run_id = NULL, active_turn_id = NULL,
                active_executor_json = NULL, active_run_state = NULL,
                active_started_at = NULL, stream_version = ?1 WHERE task_id = ?2",
            params![to_i64(event.stream_version)?, event.stream.stream_id],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn update_active_session_checkpoint(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
    status: &str,
) -> Result<(), RepositoryError> {
    let executor_json = transaction
        .query_row(
            "SELECT active_executor_json FROM tasks WHERE task_id = ?1",
            [&event.stream.stream_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(storage_error)?;
    if let Some(executor_json) = executor_json {
        let executor: ExecutorRef = serde_json::from_str(&executor_json).map_err(json_error)?;
        transaction
            .execute(
                "UPDATE agent_session_maps SET last_seen_sequence = ?1, last_used_at = ?2,
                    status = ?3 WHERE task_id = ?4 AND provider_id = ?5 AND status = 'active'",
                params![
                    to_i64(event.sequence)?,
                    event.occurred_at.timestamp_millis(),
                    status,
                    event.stream.stream_id,
                    executor.provider_id.to_string(),
                ],
            )
            .map_err(storage_error)?;
    }
    Ok(())
}

fn prompt_history_title(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut title: String = chars.by_ref().take(32).collect();
    if chars.next().is_some() {
        title.push('…');
    }
    title
}

fn load_timeline(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<Vec<TimelineItem>, RepositoryError> {
    // Facts survive crashes before the final summary and duplicate native notifications.
    let mut request_facts =
        std::collections::HashMap::<String, driver_api::usage::RequestUsageReport>::new();
    let mut facts = connection.prepare("SELECT json_extract(payload_json, '$.turnId'), json_extract(payload_json, '$.request') FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type='usage.request_recorded' ORDER BY sequence").map_err(storage_error)?;
    let rows = facts
        .query_map([task_id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage_error)?;
    for row in rows {
        let (turn, data) = row.map_err(storage_error)?;
        if let Ok(request) = serde_json::from_str(&data) {
            request_facts.entry(turn).or_default().observe(request);
        }
    }
    // Derive timing from canonical primary-turn events; late subagent events
    // must not extend the primary turn.
    let mut timings = connection.prepare(
            "SELECT json_extract(payload_json, '$.turnId'),
                    COALESCE(MIN(CASE WHEN event_type = 'turn.started' THEN occurred_at END), MIN(occurred_at)),
                    MAX(CASE WHEN event_type IN ('turn.completed', 'turn.failed', 'turn.connection_lost') THEN occurred_at END),
                    MAX(CASE WHEN event_type = 'turn.usage_recorded' THEN json_extract(payload_json, '$.usage') END),
                    MIN(CASE WHEN event_type = 'turn.first_output' THEN json_extract(payload_json, '$.elapsedMs') END)
             FROM events WHERE stream_type = 'task' AND stream_id = ?1
               AND event_type IN ('turn.queued', 'turn.autonomous.started', 'turn.started', 'turn.completed', 'turn.failed', 'turn.connection_lost', 'turn.usage_recorded', 'turn.first_output')
             GROUP BY json_extract(payload_json, '$.turnId')
             HAVING MAX(CASE WHEN event_type IN ('turn.completed', 'turn.failed', 'turn.connection_lost') THEN occurred_at END) IS NOT NULL"
        ).map_err(storage_error)?;
    let completions = timings
        .query_map([task_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                agent_protocol::TurnCompletion {
                    first_output_ms: row.get(4)?,
                    usage: row
                        .get::<_, Option<String>>(3)?
                        .and_then(|value| serde_json::from_str(&value).ok())
                        .or_else(|| {
                            request_facts
                                .get(&row.get::<_, String>(0).ok()?)
                                .and_then(agent_core::summarize_recorded_requests)
                        }),
                    started_at: from_millis(row.get(1)?)?,
                    completed_at: from_millis(row.get(2)?)?,
                },
            ))
        })
        .map_err(storage_error)?
        .collect::<Result<std::collections::HashMap<_, _>, _>>()
        .map_err(storage_error)?;
    let mut statement = connection
            .prepare(
                "SELECT item_id, task_id, turn_id, kind, text, provider_id, sequence, occurred_at, (SELECT json_extract(payload_json, '$.attachments') FROM events WHERE events.sequence = timeline_items.sequence AND event_type = 'turn.queued')
                 FROM timeline_items
                 WHERE task_id = ?1
                 ORDER BY sequence",
            )
            .map_err(storage_error)?;
    statement
        .query_map([task_id.as_str()], |row| {
            let turn_id = row.get::<_, Option<String>>(2)?.map(parse_id).transpose()?;
            Ok(TimelineItem {
                turn_completion: turn_id
                    .as_ref()
                    .and_then(|id: &agent_domain::TurnId| completions.get(id.as_str()))
                    .cloned(),
                attachments: row
                    .get::<_, Option<String>>(8)?
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default(),
                item_id: row.get(0)?,
                task_id: parse_id(row.get::<_, String>(1)?)?,
                turn_id,
                kind: row.get(3)?,
                text: row.get(4)?,
                provider_id: row.get(5)?,
                sequence: row.get(6)?,
                occurred_at: from_millis(row.get(7)?)?,
            })
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn update_pending_run_state(
    connection: &Connection,
    task: &mut TaskSummary,
) -> Result<(), RepositoryError> {
    if let Some(run) = task.active_primary_run.as_mut()
        && !matches!(run.state, RunState::Interrupting | RunState::Disconnected)
    {
        let pending = pending_approvals_from_connection(connection, &task.task_id, 50)?;
        if pending.iter().any(|p| p.question.is_some()) {
            run.state = RunState::WaitingForInput;
        } else if !pending.is_empty() {
            run.state = RunState::WaitingForApproval;
        }
    }
    Ok(())
}

fn load_tasks(
    connection: &Connection,
    task_id: Option<&TaskId>,
) -> Result<Vec<TaskSummary>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT task_id, project_id, title, preferred_executor_json,
                    active_run_id, active_turn_id, active_executor_json,
                    active_run_state, active_started_at, stream_version,
                    CASE WHEN EXISTS(SELECT 1 FROM events WHERE stream_type = 'task'
                        AND stream_id = tasks.task_id AND event_type = 'task.renamed')
                    THEN NULL ELSE (SELECT text FROM timeline_items
                     WHERE timeline_items.task_id = tasks.task_id AND kind = 'user_message'
                     ORDER BY sequence DESC LIMIT 1) END,
                    (SELECT occurred_at FROM events WHERE stream_type='task' AND stream_id=tasks.task_id ORDER BY stream_version DESC LIMIT 1),
                    (SELECT json_extract(payload_json, '$.executor') FROM events
                     WHERE stream_type='task' AND stream_id=tasks.task_id AND event_type IN ('turn.queued','turn.autonomous.started')
                     ORDER BY stream_version DESC LIMIT 1)
             FROM tasks WHERE (?1 IS NULL OR task_id = ?1) ORDER BY rowid",
        )
        .map_err(storage_error)?;
    statement
        .query_map([task_id.map(TaskId::as_str)], |row| {
            let active_run_id = row.get::<_, Option<String>>(4)?;
            let active_primary_run = if let Some(run_id) = active_run_id {
                let executor_json = row.get::<_, String>(6)?;
                Some(ActiveRunSummary {
                    run_id: parse_id(run_id)?,
                    turn_id: parse_id(row.get::<_, String>(5)?)?,
                    executor: serde_json::from_str(&executor_json).map_err(sql_json_error)?,
                    state: parse_run_state(&row.get::<_, String>(7)?)?,
                    started_at: from_millis(row.get(8)?)?,
                })
            } else {
                None
            };
            Ok(TaskSummary {
                updated_at: row
                    .get::<_, Option<i64>>(11)?
                    .map(from_millis)
                    .transpose()?,
                task_id: parse_id(row.get::<_, String>(0)?)?,
                project_id: parse_id(row.get::<_, String>(1)?)?,
                title: row
                    .get::<_, Option<String>>(10)?
                    .map(|prompt| prompt_history_title(&prompt))
                    .filter(|title| !title.is_empty())
                    .unwrap_or(row.get(2)?),
                preferred_executor: serde_json::from_str(&row.get::<_, String>(3)?)
                    .map_err(sql_json_error)?,
                last_turn_executor: row
                    .get::<_, Option<String>>(12)?
                    .map(|json| serde_json::from_str(&json).map_err(sql_json_error))
                    .transpose()?,
                active_primary_run,
                stream_version: row.get(9)?,
            })
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn load_installations(
    connection: &Connection,
) -> Result<Vec<InstallationSummary>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT installation_id, provider_id, executable, fingerprint, status,
                    version, version_text, diagnostic, probe_elapsed_millis, probed_at
             FROM installations ORDER BY provider_id, executable",
        )
        .map_err(storage_error)?;
    statement
        .query_map([], |row| {
            let status_json = format!("\"{}\"", row.get::<_, String>(4)?);
            Ok(InstallationSummary {
                installation_id: parse_id(row.get::<_, String>(0)?)?,
                provider_id: row.get(1)?,
                executable: row.get(2)?,
                fingerprint: row.get(3)?,
                status: serde_json::from_str(&status_json).map_err(sql_json_error)?,
                version: row.get(5)?,
                version_text: row.get(6)?,
                diagnostic: row.get(7)?,
                probe_elapsed_millis: row.get(8)?,
                probed_at: from_millis(row.get(9)?)?,
            })
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn load_session_maps(
    connection: &Connection,
    task_id: Option<&TaskId>,
) -> Result<Vec<AgentSessionMapSummary>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT session_map_id, task_id, provider_id, installation_id,
                    native_session_id, last_native_turn_id, workspace_locator,
                    driver_version, native_agent_version, created_at, last_used_at,
                    resume_capability, status, last_seen_sequence, model, thinking
             FROM agent_session_maps WHERE (?1 IS NULL OR task_id = ?1) ORDER BY task_id, provider_id, installation_key",
        )
        .map_err(storage_error)?;
    statement
        .query_map([task_id.map(TaskId::as_str)], |row| {
            let resume_json = format!("\"{}\"", row.get::<_, String>(11)?);
            let status_json = format!("\"{}\"", row.get::<_, String>(12)?);
            Ok(AgentSessionMapSummary {
                model: row.get(14)?,
                thinking: row.get(15)?,
                session_map_id: parse_id(row.get::<_, String>(0)?)?,
                task_id: parse_id(row.get::<_, String>(1)?)?,
                executor: ExecutorRef {
                    provider_id: ProviderId::parse(row.get::<_, String>(2)?)
                        .map_err(|error| sql_conversion_error(error.to_string()))?,
                    installation_id: row.get(3)?,
                },
                native_session_id: row.get(4)?,
                last_native_turn_id: row.get(5)?,
                workspace_locator: row.get(6)?,
                driver_version: row.get(7)?,
                native_agent_version: row.get(8)?,
                created_at: from_millis(row.get(9)?)?,
                last_used_at: from_millis(row.get(10)?)?,
                resume_capability: serde_json::from_str::<ResumeCapability>(&resume_json)
                    .map_err(sql_json_error)?,
                status: serde_json::from_str::<SessionMapStatus>(&status_json)
                    .map_err(sql_json_error)?,
                last_seen_sequence: row.get(13)?,
            })
        })
        .map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error)
}

fn pending_approvals_from_connection(
    connection: &Connection,
    task_id: &TaskId,
    limit: usize,
) -> Result<Vec<PendingApproval>, RepositoryError> {
    let mut statement = connection
            .prepare(
                "SELECT sequence, event_type, payload_json FROM events
                 WHERE stream_type = 'task' AND stream_id = ?1
                   AND event_type IN ('approval.requested', 'approval.resolved',
                                      'turn.completed', 'turn.failed', 'turn.connection_lost', 'turn.cancel_requested', 'turn.cancel_failed', 'approval.expired')
                 ORDER BY sequence ASC",
            )
            .map_err(storage_error)?;
    let mut rows = statement
        .query(params![task_id.to_string()])
        .map_err(storage_error)?;
    let mut pending: Vec<PendingApproval> = Vec::new();
    let mut closed = std::collections::BTreeSet::new();
    let mut stopping = std::collections::BTreeSet::new();
    while let Some(row) = rows.next().map_err(storage_error)? {
        let sequence: i64 = row.get(0).map_err(storage_error)?;
        let event_type: String = row.get(1).map_err(storage_error)?;
        let payload: Value = serde_json::from_str(&row.get::<_, String>(2).map_err(storage_error)?)
            .map_err(json_error)?;
        match event_type.as_str() {
            "approval.requested" => {
                let Ok(approval) = serde_json::from_value::<PendingApproval>(json!({
                    "approvalId": payload.get("approvalId"),
                    "turnId": payload.get("turnId"),
                    "toolName": payload.get("toolName"),
                    "commandSummary": payload.get("commandSummary"),
                    "options": payload.get("options"),
                    "question": payload.get("question"),
                    "requestedAtSequence": sequence,
                })) else {
                    continue;
                };
                if closed.contains(&approval.turn_id) {
                    continue;
                }
                pending.retain(|existing| {
                    existing.approval_id != approval.approval_id
                        || existing.turn_id != approval.turn_id
                });
                pending.push(approval);
            }
            // Resolved (approved/denied) drops one approval; a terminal
            // turn event closes every remaining interaction (design §8.7).
            "turn.completed" | "turn.failed" | "turn.connection_lost" => {
                if let Some(turn_id) = payload["turnId"]
                    .as_str()
                    .and_then(|id| agent_domain::TurnId::parse(id).ok())
                {
                    pending.retain(|approval| approval.turn_id != turn_id);
                    closed.insert(turn_id);
                } else {
                    pending.clear();
                }
            }
            "turn.cancel_requested" | "turn.cancel_failed" => {
                if let Some(turn_id) = payload["turnId"]
                    .as_str()
                    .and_then(|id| agent_domain::TurnId::parse(id).ok())
                {
                    if event_type == "turn.cancel_requested" {
                        stopping.insert(turn_id);
                    } else {
                        stopping.remove(&turn_id);
                    }
                }
            }
            _ => {
                let approval_id = payload.get("approvalId").and_then(Value::as_str);
                pending.retain(|existing| {
                    Some(existing.approval_id.as_str()) != approval_id
                        || payload["turnId"]
                            .as_str()
                            .is_some_and(|id| existing.turn_id.as_str() != id)
                });
            }
        }
    }
    pending.retain(|approval| !stopping.contains(&approval.turn_id));
    let boundary = pending.len().saturating_sub(limit);
    pending.drain(..boundary);
    Ok(pending)
}

fn companion_terminal(
    connection: &Connection,
    task_id: &TaskId,
) -> Result<Option<CompanionTerminal>, RepositoryError> {
    connection.query_row(
        "SELECT event_type, occurred_at, payload_json, source_json FROM events WHERE stream_type = 'task' AND stream_id = ?1 AND event_type IN ('turn.completed', 'turn.failed', 'turn.connection_lost') AND json_extract(payload_json,'$.turnId') = (SELECT json_extract(payload_json,'$.turnId') FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type IN ('turn.queued','turn.autonomous.started') ORDER BY sequence DESC LIMIT 1) AND sequence > COALESCE((SELECT MAX(sequence) FROM events WHERE stream_type='task' AND stream_id=?1 AND event_type IN ('turn.queued','turn.autonomous.started')),0) ORDER BY sequence DESC LIMIT 1",
        [task_id.as_str()],
        |row| {
            let event_type: String = row.get(0)?;
            let payload: Value = serde_json::from_str(&row.get::<_, String>(2)?).map_err(sql_json_error)?;
            let state = if event_type == "turn.connection_lost" { RunState::Disconnected }
                else if event_type == "turn.completed" { RunState::Completed }
                else if payload["failureKind"] == "interrupted" { RunState::Interrupted }
                else { RunState::Failed };
            let source: EventSource = serde_json::from_str(&row.get::<_, String>(3)?).map_err(sql_json_error)?;
            let provider_id = match source { EventSource::Driver { provider_id, .. } => Some(provider_id), EventSource::Hub => None };
            Ok(CompanionTerminal { provider_id, state, occurred_at: from_millis(row.get(1)?)? })
        },
    ).optional().map_err(storage_error)
}

fn companion_projection(
    connection: &Connection,
    tasks: &[TaskSummary],
    revision: u64,
) -> Result<CompanionProjection, RepositoryError> {
    let mut reviews = companion_reviews::load(connection)?;
    let workspaces = tasks
        .iter()
        .map(|task| {
            let task_reviews = reviews.remove(&task.task_id).unwrap_or_default();
            Ok(CompanionWorkspace {
                last_terminal: companion_terminal(connection, &task.task_id)?,
                project_id: task.project_id.clone(),
                task_id: task.task_id.clone(),
                display_name: task.title.clone(),
                preferred_executor: task.preferred_executor.clone(),
                active_primary_run: task.active_primary_run.clone(),
                secondary_agent_count: u32::try_from(
                    task_reviews
                        .iter()
                        .filter(|review| {
                            matches!(review.status.as_str(), "queued" | "running" | "cancelling")
                        })
                        .count(),
                )
                .unwrap_or(u32::MAX),
                reviews: task_reviews,
                pending_interaction_count: if task.active_primary_run.is_some() {
                    u32::try_from(
                        pending_approvals_from_connection(connection, &task.task_id, 50)?.len(),
                    )
                    .unwrap_or(u32::MAX)
                } else {
                    0
                },
            })
        })
        .collect::<Result<Vec<_>, RepositoryError>>()?;
    let recent_completion = connection
        .query_row(
            "SELECT task_id, run_id, provider_id, completed_at FROM recent_completion LIMIT 1",
            [],
            |row| {
                Ok(RecentCompletion {
                    task_id: parse_id(row.get::<_, String>(0)?)?,
                    run_id: parse_id(row.get::<_, String>(1)?)?,
                    provider_id: row.get(2)?,
                    completed_at: from_millis(row.get(3)?)?,
                })
            },
        )
        .optional()
        .map_err(storage_error)?;
    Ok(CompanionProjection {
        schema_version: SCHEMA_VERSION,
        revision,
        generated_at: Utc::now(),
        stale: false,
        workspaces,
        recent_completion: recent_completion
            .filter(|completion| tasks.iter().any(|task| task.task_id == completion.task_id)),
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventEnvelope> {
    let stream_type = match row.get::<_, String>(2)?.as_str() {
        "project" => StreamType::Project,
        "task" => StreamType::Task,
        "installation" => StreamType::Installation,
        "quota" => StreamType::Quota,
        "system" => StreamType::System,
        other => return Err(sql_conversion_error(format!("unknown stream type {other}"))),
    };
    let extensions = row
        .get::<_, Option<String>>(13)?
        .map(|json| serde_json::from_str(&json).map_err(sql_json_error))
        .transpose()?;
    Ok(EventEnvelope {
        sequence: row.get(0)?,
        event_id: parse_id(row.get::<_, String>(1)?)?,
        stream: StreamRef {
            stream_type,
            stream_id: row.get(3)?,
        },
        stream_version: row.get(4)?,
        event_type: row.get(5)?,
        schema_version: row.get(6)?,
        occurred_at: from_millis(row.get(7)?)?,
        recorded_at: from_millis(row.get(8)?)?,
        correlation_id: parse_id(row.get::<_, String>(9)?)?,
        causation_id: row.get(10)?,
        source: serde_json::from_str(&row.get::<_, String>(11)?).map_err(sql_json_error)?,
        payload: serde_json::from_str(&row.get::<_, String>(12)?).map_err(sql_json_error)?,
        vendor_extensions: extensions,
    })
}

fn payload_string(event: &EventEnvelope, field: &str) -> Result<String, RepositoryError> {
    event
        .payload
        .get(field)
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| RepositoryError::Unavailable(format!("missing payload field {field}")))
}

fn missing_payload_field(_event: &EventEnvelope, field: &str) -> RepositoryError {
    RepositoryError::Unavailable(format!("missing payload field {field}"))
}

fn payload_u64(event: &EventEnvelope, field: &str) -> Result<u64, RepositoryError> {
    event
        .payload
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| RepositoryError::Unavailable(format!("missing payload field {field}")))
}

fn parse_payload_time(
    event: &EventEnvelope,
    field: &str,
) -> Result<DateTime<Utc>, RepositoryError> {
    let value = payload_string(event, field)?;
    value
        .parse::<DateTime<Utc>>()
        .map_err(|error| RepositoryError::Unavailable(error.to_string()))
}

fn driver_provider(event: &EventEnvelope) -> Option<&str> {
    match &event.source {
        EventSource::Driver { provider_id, .. } => Some(provider_id.as_str()),
        EventSource::Hub => None,
    }
}

fn parse_run_state(value: &str) -> rusqlite::Result<RunState> {
    match value {
        "queued" => Ok(RunState::Queued),
        "starting" => Ok(RunState::Starting),
        "running" => Ok(RunState::Running),
        "waiting_for_input" => Ok(RunState::WaitingForInput),
        "waiting_for_approval" => Ok(RunState::WaitingForApproval),
        "interrupting" => Ok(RunState::Interrupting),
        "interrupted" => Ok(RunState::Interrupted),
        "completed" => Ok(RunState::Completed),
        "failed" => Ok(RunState::Failed),
        "disconnected" => Ok(RunState::Disconnected),
        "reconnecting" => Ok(RunState::Reconnecting),
        other => Err(sql_conversion_error(format!("unknown run state {other}"))),
    }
}

fn parse_id<T>(value: String) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error: T::Err| sql_conversion_error(error.to_string()))
}

fn from_millis(value: i64) -> rusqlite::Result<DateTime<Utc>> {
    Utc.timestamp_millis_opt(value)
        .single()
        .ok_or_else(|| sql_conversion_error(format!("invalid timestamp {value}")))
}

fn to_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value)
        .map_err(|error| RepositoryError::Unavailable(format!("integer overflow: {error}")))
}

fn storage_error(error: rusqlite::Error) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}

fn json_error(error: serde_json::Error) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}

fn poison_error<T>(error: std::sync::PoisonError<T>) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}

fn sql_json_error(error: serde_json::Error) -> rusqlite::Error {
    sql_conversion_error(error.to_string())
}

fn sql_conversion_error(message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}

const SCHEMA: &str = r"
CREATE TABLE events(
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  stream_type TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  occurred_at INTEGER NOT NULL,
  recorded_at INTEGER NOT NULL,
  correlation_id TEXT NOT NULL,
  causation_id TEXT,
  source_json TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  extensions_json TEXT,
  UNIQUE(stream_type, stream_id, stream_version)
);

CREATE TABLE command_dedup(
  idempotency_key TEXT PRIMARY KEY,
  command_id TEXT NOT NULL,
  ack_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE artifact_blobs(
  payload_sha256 TEXT PRIMARY KEY,
  byte_length INTEGER NOT NULL,
  content BLOB NOT NULL
);

CREATE TABLE artifacts(
  artifact_id TEXT PRIMARY KEY,
  payload_sha256 TEXT NOT NULL REFERENCES artifact_blobs(payload_sha256),
  media_type TEXT NOT NULL,
  redaction_ruleset_version INTEGER NOT NULL,
  redaction_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE artifact_versions(
  artifact_id TEXT PRIMARY KEY REFERENCES artifacts(artifact_id),
  parent_artifact_id TEXT NOT NULL REFERENCES artifacts(artifact_id),
  relation TEXT NOT NULL,
  created_sequence INTEGER NOT NULL,
  CHECK(artifact_id <> parent_artifact_id)
);

CREATE TABLE projects(
  project_id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  path TEXT NOT NULL,
  additional_directories_json TEXT NOT NULL,
  stream_version INTEGER NOT NULL
);

CREATE TABLE tasks(
  task_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  title TEXT NOT NULL,
  preferred_executor_json TEXT NOT NULL,
  active_run_id TEXT,
  active_turn_id TEXT,
  active_executor_json TEXT,
  active_run_state TEXT,
  active_started_at INTEGER,
  stream_version INTEGER NOT NULL
);

CREATE TABLE agent_session_maps(
  session_map_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  provider_id TEXT NOT NULL,
  installation_id TEXT,
  installation_key TEXT NOT NULL,
  native_session_id TEXT NOT NULL,
  last_native_turn_id TEXT,
  workspace_locator TEXT NOT NULL,
  driver_version TEXT,
  native_agent_version TEXT,
  model TEXT,
  thinking TEXT,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  resume_capability TEXT NOT NULL,
  status TEXT NOT NULL,
  last_seen_sequence INTEGER NOT NULL,
  UNIQUE(task_id, provider_id, installation_key)
);

CREATE TABLE workspace_snapshots(
  sequence INTEGER PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  turn_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  phase_json TEXT NOT NULL,
  captured_at INTEGER NOT NULL,
  git_json TEXT,
  change_summary_json TEXT,
  relevant_file_count INTEGER NOT NULL,
  dependency_change_count INTEGER NOT NULL,
  diagnostic_json TEXT,
  CHECK((git_json IS NOT NULL AND diagnostic_json IS NULL) OR
        (git_json IS NULL AND diagnostic_json IS NOT NULL))
);

CREATE INDEX workspace_snapshot_task_sequence
ON workspace_snapshots(task_id, sequence DESC);

CREATE TABLE workspace_diff_artifacts(
  artifact_id TEXT PRIMARY KEY REFERENCES artifacts(artifact_id),
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  status_fingerprint TEXT NOT NULL,
  base TEXT NOT NULL,
  sequence INTEGER NOT NULL UNIQUE
);

CREATE INDEX workspace_diff_task_fingerprint
ON workspace_diff_artifacts(task_id, status_fingerprint, sequence DESC);

CREATE TABLE context_transfers(
  artifact_id TEXT PRIMARY KEY REFERENCES artifacts(artifact_id),
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  kind TEXT NOT NULL,
  source_executor_json TEXT NOT NULL,
  target_executor_json TEXT NOT NULL,
  source_after_sequence INTEGER NOT NULL,
  source_through_sequence INTEGER NOT NULL,
  status TEXT NOT NULL,
  confirmed_sequence INTEGER NOT NULL,
  last_event_sequence INTEGER NOT NULL,
  session_map_id TEXT,
  failure TEXT
);

CREATE INDEX context_transfer_task_status
ON context_transfers(task_id, status, confirmed_sequence DESC);

CREATE TABLE reconciliation_opt_outs(
  opted_out_sequence INTEGER PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  source_executor_json TEXT NOT NULL,
  target_executor_json TEXT NOT NULL,
  source_after_sequence INTEGER NOT NULL,
  source_through_sequence INTEGER NOT NULL,
  preview_payload_sha256 TEXT NOT NULL,
  status TEXT NOT NULL,
  last_event_sequence INTEGER NOT NULL,
  session_map_id TEXT,
  failure TEXT
);

CREATE INDEX reconciliation_opt_out_task_status
ON reconciliation_opt_outs(task_id, status, opted_out_sequence DESC);

CREATE TABLE installations(
  installation_id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  executable TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  status TEXT NOT NULL,
  version TEXT,
  version_text TEXT,
  diagnostic TEXT,
  probe_elapsed_millis INTEGER NOT NULL,
  probed_at INTEGER NOT NULL,
  stream_version INTEGER NOT NULL,
  UNIQUE(provider_id, executable)
);

CREATE TABLE timeline_items(
  item_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  turn_id TEXT,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  provider_id TEXT,
  sequence INTEGER NOT NULL,
  occurred_at INTEGER NOT NULL
);

CREATE INDEX timeline_task_sequence
ON timeline_items(task_id, sequence);

CREATE TABLE recent_completion(
  task_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  completed_at INTEGER NOT NULL
);
CREATE TABLE task_artifacts(
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  artifact_id TEXT NOT NULL REFERENCES artifacts(artifact_id),
  source_sequence INTEGER NOT NULL REFERENCES events(sequence),
  source_kind TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  PRIMARY KEY(task_id, artifact_id)
);
CREATE INDEX task_artifacts_page ON task_artifacts(task_id, source_sequence DESC, artifact_id DESC);

";

#[cfg(test)]
#[path = "../tests/support/installations.rs"]
mod fixture_installations;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use agent_core::{
        Core, WorkspaceContextProvider, WorkspaceContextSnapshot, WorkspaceDiffCapture,
    };
    use agent_domain::{ExecutorRef, ProjectId, TaskId};
    use agent_protocol::{
        ActorRef, CancelTurn, CaptureWorkspaceDiff, ChangeSummary, CommandEnvelope,
        ConfirmContextPackage, ContextEditRequest, ContextPackageKind, ContextPackageV1,
        ContextPreviewRequest, ContextStrength, CoreCommand, CreateTask, GitWorkspaceSnapshotV1,
        InstallationStatus, OptOutReconciliation, ReconcileMissingInstallation,
        RecordInstallationProbe, RecordTestResult, RegisterProject, RelevantFile,
        RespondToApproval, SetPreferredExecutor, SubmitTurn, TestReportV1,
        WorkspaceCollectionDiagnostic, WorkspaceDiagnosticKind,
    };
    use driver_api::{
        AgentDriver, ContextDeliveryKind, DriverError, DriverEvent, DriverEventStream, FakeDriver,
        PromptRequest, SessionHandle, SessionRequest, SessionResumeCapability,
    };
    use tokio::sync::{oneshot, watch};

    use super::*;

    #[test]
    fn history_title_truncates_unicode_and_normalizes_whitespace() {
        assert_eq!(
            prompt_history_title("  修复\n 会话   标题  "),
            "修复 会话 标题"
        );
        assert_eq!(prompt_history_title(&"你".repeat(32)), "你".repeat(32));
        assert_eq!(
            prompt_history_title(&"你".repeat(33)),
            format!("{}…", "你".repeat(32))
        );
        assert_eq!(
            prompt_history_title(&"😀".repeat(33)),
            format!("{}…", "😀".repeat(32))
        );
    }

    #[test]
    fn history_title_uses_latest_saved_user_message_for_existing_tasks() {
        let store = SqliteStore::open_in_memory().unwrap();
        let connection = store.connection.lock().unwrap();
        let project_id = ProjectId::new().to_string();
        let task_id = TaskId::new().to_string();
        connection
            .execute(
                "INSERT INTO projects VALUES (?1, 'Demo', '/tmp/demo', '[]', 1)",
                params![project_id],
            )
            .unwrap();
        connection.execute("INSERT INTO tasks(task_id, project_id, title, preferred_executor_json, stream_version) VALUES (?1, ?2, '新对话', ?3, 1)",
            params![task_id, project_id, serde_json::to_string(&executor("fake")).unwrap()]).unwrap();
        assert_eq!(load_tasks(&connection, None).unwrap()[0].title, "新对话");
        for (sequence, kind, text) in [
            (1, "user_message", "第一条提示词"),
            (2, "assistant_message", "模型回复"),
            (3, "user_message", "最后一次\n发送的提示词"),
            (4, "assistant_message", "最新模型回复"),
        ] {
            connection.execute("INSERT INTO timeline_items(item_id, task_id, kind, text, sequence, occurred_at) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                params![format!("item-{sequence}"), task_id, kind, text, sequence]).unwrap();
        }
        assert_eq!(
            load_tasks(&connection, None).unwrap()[0].title,
            "最后一次 发送的提示词"
        );
        // Loading derives display titles without mutating the stored task.
        assert_eq!(
            connection
                .query_row("SELECT title FROM tasks", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "新对话"
        );
    }

    #[test]
    fn history_agent_uses_latest_primary_turn_not_preference_or_review() {
        let store = SqliteStore::open_in_memory().unwrap();
        let connection = store.connection.lock().unwrap();
        let project_id = ProjectId::new().to_string();
        let task_id = TaskId::new().to_string();
        connection
            .execute(
                "INSERT INTO projects VALUES (?1, 'Demo', '/tmp/demo', '[]', 1)",
                params![project_id],
            )
            .unwrap();
        connection.execute("INSERT INTO tasks(task_id, project_id, title, preferred_executor_json, stream_version) VALUES (?1, ?2, 'History', ?3, 1)",
            params![task_id, project_id, serde_json::to_string(&executor("kimi")).unwrap()]).unwrap();
        assert!(
            load_tasks(&connection, None).unwrap()[0]
                .last_turn_executor
                .is_none()
        );
        for (version, event_type, provider) in [
            (1, "turn.queued", "codex"),
            (2, "turn.queued", "cursor"),
            (3, "task.preferred_executor_changed", "kimi"),
            (4, "review.started", "claude-code"),
        ] {
            connection.execute("INSERT INTO events(event_id, stream_type, stream_id, stream_version, event_type, schema_version, occurred_at, recorded_at, correlation_id, source_json, payload_json) VALUES (?1, 'task', ?2, ?3, ?4, 1, 0, 0, 'test', '{}', ?5)",
                params![format!("history-agent-{version}"), task_id, version, event_type, serde_json::json!({"executor": executor(provider)}).to_string()]).unwrap();
        }
        let task = load_tasks(&connection, None).unwrap().remove(0);
        assert_eq!(task.last_turn_executor, Some(executor("cursor")));
        assert_eq!(task.preferred_executor, executor("kimi"));
        assert!(task.active_primary_run.is_none());
        assert_eq!(
            load_tasks(&connection, Some(&task.task_id)).unwrap()[0].last_turn_executor,
            task.last_turn_executor
        );
    }

    fn executor(provider: &str) -> ExecutorRef {
        fixture_installations::executor(provider)
    }

    struct InterruptibleDriver {
        executor: ExecutorRef,
        interrupt_tx: watch::Sender<bool>,
        interrupt_called: AtomicBool,
    }

    impl InterruptibleDriver {
        fn new() -> Self {
            let (interrupt_tx, _) = watch::channel(false);
            Self {
                executor: executor("interruptible"),
                interrupt_tx,
                interrupt_called: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl AgentDriver for InterruptibleDriver {
        fn executor(&self) -> &ExecutorRef {
            &self.executor
        }

        fn driver_version(&self) -> &'static str {
            "test"
        }

        fn resume_capability(&self) -> SessionResumeCapability {
            SessionResumeCapability::Unsupported
        }

        async fn create_session(
            &self,
            _request: SessionRequest,
        ) -> Result<SessionHandle, DriverError> {
            Ok(SessionHandle {
                native_session_id: "interruptible-session".into(),
            })
        }

        async fn prompt(
            &self,
            _handle: &SessionHandle,
            _request: PromptRequest,
        ) -> Result<DriverEventStream, DriverError> {
            let mut interrupted = self.interrupt_tx.subscribe();
            Ok(Box::pin(futures::stream::once(async move {
                if !*interrupted.borrow() {
                    interrupted.changed().await.map_err(|_| {
                        DriverError::Disconnected("test interrupt channel closed".into())
                    })?;
                }
                Ok(DriverEvent::TurnInterrupted {
                    summary: "test turn interrupted".into(),
                })
            })))
        }

        async fn interrupt(&self, _handle: &SessionHandle) -> Result<(), DriverError> {
            self.interrupt_called.store(true, Ordering::Release);
            self.interrupt_tx.send_replace(true);
            Ok(())
        }

        async fn shutdown(&self, _handle: SessionHandle) -> Result<(), DriverError> {
            Ok(())
        }
    }

    /// Emits one approval request and keeps the stream open until the hub
    /// answers it, mirroring how real agents block tool execution.
    struct ApprovalDriver {
        question: bool,
        executor: ExecutorRef,
        decision_tx: std::sync::Mutex<Option<oneshot::Sender<String>>>,
        recorded: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl ApprovalDriver {
        fn new(provider: &str) -> Self {
            Self {
                question: false,
                executor: executor(provider),
                decision_tx: std::sync::Mutex::new(None),
                recorded: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentDriver for ApprovalDriver {
        async fn respond_to_question(
            &self,
            _: &SessionHandle,
            id: &str,
            answer: agent_domain::QuestionAnswer,
        ) -> Result<(), DriverError> {
            let value = serde_json::to_string(&answer).unwrap();
            self.recorded
                .lock()
                .unwrap()
                .push((id.into(), value.clone()));
            self.decision_tx
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(value)
                .map_err(|_| DriverError::Protocol("closed".into()))
        }

        fn executor(&self) -> &ExecutorRef {
            &self.executor
        }

        fn driver_version(&self) -> &'static str {
            "test"
        }

        fn resume_capability(&self) -> SessionResumeCapability {
            SessionResumeCapability::Unsupported
        }

        async fn create_session(
            &self,
            _request: SessionRequest,
        ) -> Result<SessionHandle, DriverError> {
            Ok(SessionHandle {
                native_session_id: "approval-session".into(),
            })
        }

        async fn prompt(
            &self,
            _handle: &SessionHandle,
            _request: PromptRequest,
        ) -> Result<DriverEventStream, DriverError> {
            let (decision_tx, decision_rx) = oneshot::channel::<String>();
            *self.decision_tx.lock().unwrap() = Some(decision_tx);
            let pre = futures::stream::iter(vec![
                Ok(DriverEvent::TextDelta {
                    item_id: "item-1".into(),
                    text: "need approval".into(),
                }),
                Ok(if self.question {
                    DriverEvent::QuestionRequested {
                        native_request_id: "perm-1".into(),
                        form: agent_domain::QuestionForm {
                            title: "Pick a color".into(),
                            schema: json!({"type":"object","properties":{"color":{"type":"string","enum":["red","blue"]}},"required":["color"]}),
                        },
                    }
                } else {
                    DriverEvent::ApprovalRequested {
                        native_request_id: "perm-1".into(),
                        tool_name: "Bash".into(),
                        command_summary: "sk-test-1234567890abcdef ls -la /Users/secret-user"
                            .into(),
                        options: vec![
                            driver_api::ApprovalOption {
                                option_id: "allow".into(),
                                label: "Allow once".into(),
                                semantic_role: driver_api::ApprovalSemanticRole::ApproveOnce,
                            },
                            driver_api::ApprovalOption {
                                option_id: "deny".into(),
                                label: "Deny".into(),
                                semantic_role: driver_api::ApprovalSemanticRole::Reject,
                            },
                        ],
                    }
                }),
            ]);
            let tail = futures::stream::once(async move {
                let _decision = decision_rx.await;
                Ok::<_, DriverError>(DriverEvent::TurnCompleted {
                    summary: "approval flow done".into(),
                })
            });
            Ok(Box::pin(futures::StreamExt::chain(pre, tail)))
        }

        async fn interrupt(&self, _handle: &SessionHandle) -> Result<(), DriverError> {
            Err(DriverError::CapabilityUnavailable(
                "cannot interrupt".into(),
            ))
        }

        async fn respond_to_interaction(
            &self,
            _handle: &SessionHandle,
            native_request_id: &str,
            option_id: &str,
        ) -> Result<(), DriverError> {
            self.recorded
                .lock()
                .unwrap()
                .push((native_request_id.to_owned(), option_id.to_owned()));
            if let Some(sender) = self.decision_tx.lock().unwrap().take() {
                let _ = sender.send(option_id.to_owned());
            }
            Ok(())
        }

        async fn shutdown(&self, _handle: SessionHandle) -> Result<(), DriverError> {
            Ok(())
        }
    }

    mod autonomous_tests {
        include!("autonomous_tests.rs");
    }

    mod turn_usage_tests {
        include!("turn_usage_tests.rs");
    }

    /// Replays a fixed `DriverEvent` script on every prompt; the stream always
    /// ends by exhaustion, with or without a terminal event.
    struct ScriptedDriver {
        autonomous_during_resume: std::sync::atomic::AtomicBool,
        autonomous: std::sync::Mutex<Option<DriverEventStream>>,
        autonomous_sender: std::sync::Mutex<
            Option<tokio::sync::mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>,
        >,
        executor: ExecutorRef,
        script: Vec<DriverEvent>,
        reuse: bool,
        reconfigure_supported: bool,
        selection_failure: Option<DriverError>,
        selections: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
        contexts: std::sync::Mutex<Vec<Vec<driver_api::ContextDelivery>>>,
    }

    impl ScriptedDriver {
        fn new(provider: &str, script: Vec<DriverEvent>) -> Self {
            Self {
                autonomous_during_resume: std::sync::atomic::AtomicBool::new(false),
                autonomous: std::sync::Mutex::default(),
                autonomous_sender: std::sync::Mutex::default(),
                executor: executor(provider),
                script,
                reuse: false,
                reconfigure_supported: true,
                selection_failure: None,
                selections: std::sync::Mutex::default(),
                contexts: std::sync::Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl AgentDriver for ScriptedDriver {
        async fn take_autonomous_events(
            &self,
            _handle: &SessionHandle,
        ) -> Option<DriverEventStream> {
            self.autonomous.lock().unwrap().take()
        }

        fn supports_background_events(&self) -> bool {
            self.script.iter().any(|e| matches!(e, DriverEvent::Presentation { data, .. } if data["type"] == "subagent" || data["background"] == true))
        }
        fn supports_live_session_reuse(&self) -> bool {
            self.reuse || self.supports_background_events()
        }

        async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
            Ok(driver_api::ModelCatalog {
                models: vec![
                    driver_api::ModelOption::new("other-model", "Other"),
                    driver_api::ModelOption {
                        id: "fixture-model".into(),
                        label: "Fixture".into(),
                        thinking_options: vec![driver_api::ThinkingOption {
                            id: "high".into(),
                            label: "High".into(),
                        }],
                        default_thinking: Some("high".into()),
                    },
                ],
                default_model: Some("fixture-model".into()),
            })
        }
        async fn create_selected_session(
            &self,
            request: SessionRequest,
            model: Option<&str>,
            thinking: Option<&str>,
        ) -> Result<SessionHandle, DriverError> {
            self.selections
                .lock()
                .unwrap()
                .push((model.map(str::to_owned), thinking.map(str::to_owned)));
            self.create_session(request).await
        }
        async fn reconfigure_session(
            &self,
            id: &str,
            request: SessionRequest,
            model: Option<&str>,
            thinking: Option<&str>,
        ) -> Result<SessionHandle, DriverError> {
            if let Some(error) = &self.selection_failure {
                return Err(error.clone());
            }
            if self
                .autonomous_during_resume
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                let sender = self.autonomous_sender.lock().unwrap();
                let sender = sender.as_ref().unwrap();
                sender
                    .send(Ok(DriverEvent::TurnStarted {
                        native_turn_id: None,
                    }))
                    .unwrap();
                sender
                    .send(Ok(DriverEvent::TextDelta {
                        item_id: "answer".into(),
                        text: "racing continuation".into(),
                    }))
                    .unwrap();
                sender
                    .send(Ok(DriverEvent::TurnCompleted {
                        summary: "done".into(),
                    }))
                    .unwrap();
                return Err(DriverError::AutonomousTurnBusy);
            }
            if !self.reconfigure_supported {
                return Err(DriverError::CapabilityUnavailable(
                    "fixture cannot reconfigure".into(),
                ));
            }
            self.selections
                .lock()
                .unwrap()
                .push((model.map(str::to_owned), thinking.map(str::to_owned)));
            self.resume_selected_session(id, request, model, thinking)
                .await
        }
        async fn resume_selected_session(
            &self,
            id: &str,
            _request: SessionRequest,
            _model: Option<&str>,
            _thinking: Option<&str>,
        ) -> Result<SessionHandle, DriverError> {
            Ok(SessionHandle {
                native_session_id: id.to_owned(),
            })
        }
        fn executor(&self) -> &ExecutorRef {
            &self.executor
        }

        fn driver_version(&self) -> &'static str {
            "test"
        }

        fn resume_capability(&self) -> SessionResumeCapability {
            SessionResumeCapability::Unsupported
        }

        async fn create_session(
            &self,
            _request: SessionRequest,
        ) -> Result<SessionHandle, DriverError> {
            Ok(SessionHandle {
                native_session_id: "scripted-session".into(),
            })
        }

        async fn prompt(
            &self,
            _handle: &SessionHandle,
            request: PromptRequest,
        ) -> Result<DriverEventStream, DriverError> {
            self.contexts.lock().unwrap().push(request.context);
            Ok(Box::pin(futures::stream::iter(
                self.script.clone().into_iter().map(Ok),
            )))
        }

        async fn interrupt(&self, _handle: &SessionHandle) -> Result<(), DriverError> {
            if let Some(sender) = self.autonomous_sender.lock().unwrap().as_ref() {
                let _ = sender.send(Ok(DriverEvent::TurnInterrupted {
                    summary: "stopped".into(),
                }));
                return Ok(());
            }
            Err(DriverError::CapabilityUnavailable(
                "scripted driver cannot interrupt".into(),
            ))
        }

        async fn respond_to_interaction(
            &self,
            _handle: &SessionHandle,
            _request: &str,
            option: &str,
        ) -> Result<(), DriverError> {
            if let Some(sender) = self.autonomous_sender.lock().unwrap().as_ref() {
                assert_eq!(option, "deny");
                let _ = sender.send(Ok(DriverEvent::TurnCompleted {
                    summary: "approval handled".into(),
                }));
                return Ok(());
            }
            Err(DriverError::CapabilityUnavailable(
                "fixture has no approval channel".into(),
            ))
        }

        async fn shutdown(&self, _handle: SessionHandle) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn local_artifact_reads_enforce_task_hash_budgets_and_integrity() {
        use agent_core::{CoreError, NewArtifact};
        use agent_protocol::{ArtifactReadRequest, NewEvent};

        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Artifact access".into(),
                path: "/tmp/artifact-access".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        let other_task = TaskId::new();
        for id in [&task_id, &other_task] {
            core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
                CreateTask {
                    task_id: id.clone(),
                    project_id: project_id.clone(),
                    title: "Read evidence".into(),
                    preferred_executor: executor("fake"),
                },
            )))
            .await
            .unwrap();
        }
        let mut content = vec![b'a'; 40_000];
        content.extend_from_slice(b"\n+non-utf8: \xff\n<script>alert(1)</script>\x1b[2J\n");
        let metadata = ArtifactMetadata {
            artifact_id: ArtifactId::new(),
            media_type: "application/vnd.dogcodes.git-diff.v1.patch".into(),
            payload_sha256: hex::encode(Sha256::digest(&content)),
            byte_length: content.len() as u64,
            created_at: Utc::now(),
            redaction_ruleset_version: 0,
            redaction_count: 0,
        };
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task_id.to_string(),
            },
            "workspace.diff_captured",
            serde_json::json!({ "taskId": task_id, "artifact": metadata,
                "base": "unborn", "statusFingerprint": "fixture" }),
        );
        store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: agent_domain::CommandId::new(),
                idempotency_key: event.event_id.to_string(),
                expected_stream_version: Some(1),
                artifacts: vec![NewArtifact {
                    metadata: metadata.clone(),
                    content: content.clone(),
                }],
                events: vec![event],
            })
            .await
            .unwrap();
        let before = core.bootstrap().await.unwrap();
        let mut request = ArtifactReadRequest {
            task_id: task_id.clone(),
            artifact_id: metadata.artifact_id.clone(),
            expected_payload_sha256: metadata.payload_sha256.clone(),
            offset: 0,
            limit: 32768,
        };
        let page = core
            .task_artifacts(&ActorRef::LocalUser, &task_id, None, 20)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].metadata, metadata);
        assert!(
            core.task_artifacts(&ActorRef::SystemRecovery, &task_id, None, 20)
                .await
                .is_err()
        );
        assert!(
            core.task_artifacts(&ActorRef::LocalUser, &task_id, None, 0)
                .await
                .is_err()
        );
        assert!(
            core.task_artifacts(&ActorRef::LocalUser, &task_id, None, 101)
                .await
                .is_err()
        );
        assert!(
            core.task_artifacts(&ActorRef::LocalUser, &other_task, None, 20)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            core.read_artifact(&ActorRef::SystemRecovery, &request)
                .await
                .is_err()
        );
        // A paired owner has the same task-scoped artifact access as the PC UI.
        let paired = ActorRef::PairedDevice(agent_domain::DeviceId::new());
        assert_eq!(
            serde_json::to_value(core.read_artifact(&paired, &request).await.unwrap()).unwrap(),
            serde_json::to_value(
                core.read_artifact(&ActorRef::LocalUser, &request)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
        request.task_id = other_task;
        assert!(matches!(
            core.read_artifact(&ActorRef::LocalUser, &request).await,
            Err(CoreError::NotFound(_))
        ));
        request.task_id = task_id.clone();
        request.expected_payload_sha256 = "0".repeat(64);
        assert!(matches!(
            core.read_artifact(&ActorRef::LocalUser, &request).await,
            Err(CoreError::Conflict(_))
        ));
        request
            .expected_payload_sha256
            .clone_from(&metadata.payload_sha256);
        for limit in [0, 32769, usize::MAX] {
            request.limit = limit;
            assert!(
                core.read_artifact(&ActorRef::LocalUser, &request)
                    .await
                    .is_err()
            );
        }
        request.limit = 32768;
        request.offset = u64::MAX;
        assert!(
            core.read_artifact(&ActorRef::LocalUser, &request)
                .await
                .is_err()
        );
        request.offset = 0;
        let first = core
            .read_artifact(&ActorRef::LocalUser, &request)
            .await
            .unwrap();
        assert_eq!(first.bytes.len(), 32768);
        request.offset = first.next_offset.unwrap();
        let last = core
            .read_artifact(&ActorRef::LocalUser, &request)
            .await
            .unwrap();
        assert!(last.next_offset.is_none());
        assert_eq!([first.bytes, last.bytes].concat(), content);
        request.offset = metadata.byte_length;
        assert!(
            core.read_artifact(&ActorRef::LocalUser, &request)
                .await
                .unwrap()
                .bytes
                .is_empty()
        );
        request.offset = 0;
        let after = core.bootstrap().await.unwrap();
        assert_eq!(before.tasks, after.tasks);
        assert_eq!(before.last_sequence, after.last_sequence);

        store.rebuild_projections().unwrap();
        assert_eq!(
            core.task_artifacts(&ActorRef::LocalUser, &task_id, None, 20)
                .await
                .unwrap(),
            page
        );
        assert!(
            core.read_artifact(&ActorRef::LocalUser, &request)
                .await
                .is_ok()
        );

        for bad_content in [vec![b'z'; content.len()], vec![b'x'; 1_048_577]] {
            store
                .connection
                .lock()
                .unwrap()
                .execute(
                    "UPDATE artifact_blobs SET content = ?1 WHERE payload_sha256 = ?2",
                    params![bad_content, metadata.payload_sha256],
                )
                .unwrap();
            assert!(
                core.read_artifact(&ActorRef::LocalUser, &request)
                    .await
                    .is_err()
            );
        }
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE artifact_blobs SET content = ?1 WHERE payload_sha256 = ?2",
                params![content, metadata.payload_sha256],
            )
            .unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE artifacts SET media_type = 'text/html' WHERE artifact_id = ?1",
                [metadata.artifact_id.as_str()],
            )
            .unwrap();
        assert!(
            core.read_artifact(&ActorRef::LocalUser, &request)
                .await
                .is_err()
        );
        assert!(
            core.verified_local_artifact(
                &ActorRef::LocalUser,
                &task_id,
                &metadata.artifact_id,
                &metadata.payload_sha256
            )
            .await
            .is_err()
        );
    }

    #[derive(Default)]
    struct RecordingWorkspaceContext {
        calls: AtomicUsize,
        fail: AtomicBool,
    }

    #[async_trait::async_trait]
    impl WorkspaceContextProvider for RecordingWorkspaceContext {
        async fn begin_turn(
            &self,
            _: &str,
        ) -> Result<Box<dyn agent_core::WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic>
        {
            if self.fail.load(Ordering::Acquire) {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::WorkspaceChanged,
                });
            }
            Ok(Box::new(RecordedTurnCapture))
        }
        async fn collect(&self, workspace_path: &str) -> WorkspaceContextSnapshot {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if self.fail.load(Ordering::Acquire) {
                return WorkspaceContextSnapshot {
                    diagnostic: Some(WorkspaceCollectionDiagnostic {
                        kind: WorkspaceDiagnosticKind::NotRepository,
                    }),
                    ..WorkspaceContextSnapshot::default()
                };
            }
            let fingerprint = "snapshot-stable".to_string();
            WorkspaceContextSnapshot {
                relevant_files: vec![RelevantFile {
                    path: "src/lib.rs".into(),
                    status: "modified".into(),
                    content_hash: Some(fingerprint.clone()),
                }],
                dependency_changes: vec![],
                change_summary: ChangeSummary {
                    base: "base-head".into(),
                    head: format!("working-tree:{fingerprint}"),
                    diff_stat: "1 file changed".into(),
                    artifact_ref: None,
                },
                git_snapshot: Some(GitWorkspaceSnapshotV1 {
                    schema_version: 1,
                    repo_root: workspace_path.into(),
                    head: Some("base-head".into()),
                    branch: Some("main".into()),
                    detached: false,
                    worktree_identity: "sha256:worktree".into(),
                    status_format: "porcelain_v2".into(),
                    status_fingerprint: fingerprint,
                }),
                diagnostic: None,
            }
        }

        async fn capture_diff(
            &self,
            _workspace_path: &str,
        ) -> Result<WorkspaceDiffCapture, WorkspaceCollectionDiagnostic> {
            if self.fail.load(Ordering::Acquire) {
                return Err(WorkspaceCollectionDiagnostic {
                    kind: WorkspaceDiagnosticKind::WorkspaceChanged,
                });
            }
            Ok(WorkspaceDiffCapture {
                base: "base-head".into(),
                status_fingerprint: "snapshot-stable".into(),
                content: b"diff --git a/src/lib.rs b/src/lib.rs\n".to_vec(),
            })
        }
    }

    struct RecordedTurnCapture;
    #[async_trait::async_trait]
    impl agent_core::WorkspaceTurnCapture for RecordedTurnCapture {
        async fn finish(&self) -> Result<serde_json::Value, WorkspaceCollectionDiagnostic> {
            Ok(
                json!({"beforeTree":"before","afterTree":"after","files":[{"path":"src/lib.rs","status":"M"}],"patch":"-old\n+new\napi_key=supersecret"}),
            )
        }
    }

    /// Submits a turn through the same optimistic-concurrency contract the UI
    /// uses: the envelope must carry the task's current stream version.
    async fn submit_turn(core: &Arc<Core>, task_id: &TaskId, prompt: &str) {
        let expected = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let snapshot = core.bootstrap().await.unwrap();
                if snapshot.active_runs.is_empty()
                    && let Some(task) = snapshot
                        .tasks
                        .into_iter()
                        .find(|task| task.task_id == *task_id)
                {
                    break task.stream_version;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut envelope = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: vec![],
            permission_mode: None,
            thinking: None,
            model: None,
            task_id: task_id.clone(),
            prompt: prompt.to_owned(),
            executor_override: None,
        }));
        envelope.expected_stream_version = Some(expected);
        core.execute(envelope).await.unwrap();
    }

    async fn wait_for_resumable_session(
        core: &Arc<Core>,
        task_id: &TaskId,
        provider_id: &str,
    ) -> AgentSessionMapSummary {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let snapshot = core.bootstrap().await.unwrap();
                if snapshot.active_runs.is_empty()
                    && let Some(mapping) = snapshot.session_maps.into_iter().find(|mapping| {
                        mapping.task_id == *task_id
                            && mapping.executor.provider_id.as_str() == provider_id
                            && mapping.status == SessionMapStatus::Resumable
                    })
                {
                    break mapping;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    async fn wait_for_session_status(
        core: &Arc<Core>,
        task_id: &TaskId,
        status: SessionMapStatus,
    ) -> AgentSessionMapSummary {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let snapshot = core.bootstrap().await.unwrap();
                if snapshot.active_runs.is_empty()
                    && let Some(mapping) = snapshot
                        .session_maps
                        .into_iter()
                        .find(|mapping| mapping.task_id == *task_id && mapping.status == status)
                {
                    break mapping;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    fn installation_probe(
        executable: &str,
        fingerprint: &str,
        status: InstallationStatus,
        version: &str,
    ) -> CommandEnvelope<CoreCommand> {
        let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            RecordInstallationProbe {
                provider_id: "codex".into(),
                executable: executable.into(),
                fingerprint: fingerprint.into(),
                status,
                version: Some(version.into()),
                version_text: Some(format!("codex-cli {version}")),
                diagnostic: None,
                elapsed_millis: 12,
            },
        ));
        command.actor = ActorRef::SystemRecovery;
        command
    }

    fn missing_installation(
        installation_id: InstallationId,
        executable: &str,
    ) -> CommandEnvelope<CoreCommand> {
        let mut command = CommandEnvelope::local(CoreCommand::ReconcileMissingInstallation(
            ReconcileMissingInstallation {
                installation_id,
                provider_id: "codex".into(),
                executable: executable.into(),
            },
        ));
        command.actor = ActorRef::SystemRecovery;
        command
    }

    #[tokio::test]
    async fn appends_projects_and_tasks_and_rebuilds_projections() {
        let extra = tempfile::tempdir().unwrap();
        let roots = vec![extra.path().to_str().unwrap().to_owned()];
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: roots.clone(),
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "First task".into(),
                preferred_executor: executor("codex"),
            },
        )))
        .await
        .unwrap();

        store.rebuild_projections().unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.projects[0].task_count, 1);
        assert_eq!(snapshot.projects[0].additional_directories, roots);
        assert_eq!(snapshot.tasks[0].task_id, task_id);
        assert_eq!(snapshot.companion.revision, 2);
    }

    #[tokio::test]
    async fn project_updates_preserve_tasks_rebuild_sessions_and_survive_replay() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(FakeDriver::with_executor(executor("fake"))),
        )
        .await;
        let project_id = ProjectId::new();
        let path = first.path().to_str().unwrap().to_owned();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "Before".into(),
                path: path.clone(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id: project_id.clone(),
                title: "Keep conversation".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &task_id, "create session").await;
        wait_for_resumable_session(&core, &task_id, "fake").await;
        let timeline = core.timeline(&task_id).await.unwrap();
        let update = |name: &str, primary: &str, additional: Vec<String>, version| {
            let mut envelope =
                CommandEnvelope::local(CoreCommand::UpdateProject(RegisterProject {
                    project_id: project_id.clone(),
                    display_name: name.into(),
                    path: primary.into(),
                    additional_directories: additional,
                }));
            envelope.expected_stream_version = Some(version);
            envelope
        };
        core.execute(update("Renamed", &path, vec![], 1))
            .await
            .unwrap();
        assert_eq!(
            core.bootstrap().await.unwrap().session_maps[0].status,
            SessionMapStatus::Resumable
        );
        assert!(
            core.execute(update("Stale", &path, vec![], 1))
                .await
                .is_err()
        );
        assert!(
            core.execute(update("Invalid", "relative", vec![], 2))
                .await
                .is_err()
        );
        let new_path = second.path().to_str().unwrap();
        core.execute(update("After", new_path, vec![path.clone()], 2))
            .await
            .unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].project_id, project_id);
        assert_eq!(snapshot.projects[0].display_name, "After");
        assert_eq!(snapshot.projects[0].path, new_path);
        assert_eq!(snapshot.projects[0].additional_directories, vec![path]);
        assert_eq!(snapshot.projects[0].task_count, 1);
        assert_eq!(snapshot.projects[0].stream_version, 3);
        assert_eq!(snapshot.tasks[0].task_id, task_id);
        assert_eq!(
            snapshot.session_maps[0].status,
            SessionMapStatus::RebuildPending
        );
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
        // Sidebar recency comes from durable activity, including edits and runs.
        let events = store.sync_events(0, 1_000).await.unwrap();
        let project_activity = events
            .iter()
            .rev()
            .find(|event| {
                event.stream.stream_type == StreamType::Project
                    && event.stream.stream_id == project_id.to_string()
            })
            .unwrap()
            .occurred_at;
        let task_activity = events
            .iter()
            .rev()
            .find(|event| {
                event.stream.stream_type == StreamType::Task
                    && event.stream.stream_id == task_id.to_string()
            })
            .unwrap()
            .occurred_at;
        assert_eq!(snapshot.projects[0].updated_at, Some(project_activity));
        assert_eq!(snapshot.tasks[0].updated_at, Some(task_activity));
        store.rebuild_projections().unwrap();
        let replayed = core.bootstrap().await.unwrap();
        assert_eq!(replayed.projects, snapshot.projects);
        assert_eq!(replayed.tasks, snapshot.tasks);
        assert_eq!(
            replayed.session_maps[0].status,
            SessionMapStatus::RebuildPending
        );
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn context_preview_reads_only_the_frozen_task_stream_range() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();
        let first_task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: first_task_id.clone(),
                project_id: project_id.clone(),
                title: "First task".into(),
                preferred_executor: executor("codex"),
            },
        )))
        .await
        .unwrap();
        let second_task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: second_task_id,
                project_id,
                title: "Other task".into(),
                preferred_executor: executor("codex"),
            },
        )))
        .await
        .unwrap();

        let preview = core
            .build_context_preview(ContextPreviewRequest {
                task_id: first_task_id,
                kind: ContextPackageKind::Handoff,
                strength: ContextStrength::Compact,
                source_executor: executor("codex"),
                target_executor: executor("fake"),
                after_sequence: 0,
                through_sequence: 3,
            })
            .await
            .unwrap();

        assert!(preview.package.provenance.source_sequences.is_empty());
        assert_eq!(preview.package.provenance.source_through_sequence, 3);
        assert_eq!(preview.package.context_pointers.len(), 1);
    }

    #[tokio::test]
    async fn confirming_context_atomically_stores_artifact_and_handoff_events() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Handoff api_key=supersecret".into(),
                preferred_executor: executor("codex"),
            },
        )))
        .await
        .unwrap();
        let task = core.bootstrap().await.unwrap().tasks.remove(0);
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("codex"),
            target_executor: executor("fake"),
            after_sequence: 0,
            through_sequence: 2,
        };
        let preview = core.build_context_preview(request.clone()).await.unwrap();

        let mut mismatch =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request: request.clone(),
                expected_payload_sha256: "0".repeat(64),
                edited_package: None,
            }));
        mismatch.expected_stream_version = Some(task.stream_version);
        assert!(matches!(
            core.execute(mismatch).await,
            Err(agent_core::CoreError::Conflict(_))
        ));
        assert_eq!(core.bootstrap().await.unwrap().last_sequence, 2);

        let mut confirm =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request,
                expected_payload_sha256: preview.payload_sha256.clone(),
                edited_package: None,
            }));
        confirm.expected_stream_version = Some(task.stream_version);
        let ack = core.execute(confirm.clone()).await.unwrap();
        assert_eq!(ack.first_sequence, Some(3));
        assert_eq!(ack.last_sequence, Some(4));
        assert_eq!(core.execute(confirm).await.unwrap(), ack);
        assert_eq!(core.bootstrap().await.unwrap().last_sequence, 4);

        let events = core.sync_events(2, 2).await.unwrap();
        let artifact_id = ArtifactId::parse(
            events[0]
                .payload
                .pointer("/artifact/artifactId")
                .and_then(serde_json::Value::as_str)
                .unwrap(),
        )
        .unwrap();
        let artifact = store.artifact(&artifact_id).await.unwrap().unwrap();
        assert_eq!(artifact.metadata.payload_sha256, preview.payload_sha256);
        assert_eq!(artifact.metadata.byte_length, artifact.content.len() as u64);
        let stored_package: ContextPackageV1 = serde_json::from_slice(&artifact.content).unwrap();
        assert_eq!(stored_package, preview.package);
        assert!(
            !String::from_utf8(artifact.content)
                .unwrap()
                .contains("supersecret")
        );

        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.tasks[0].preferred_executor, executor("fake"));
        assert_eq!(snapshot.tasks[0].stream_version, 3);
        assert_eq!(events[0].event_type, "handoff.package_confirmed");
        assert_eq!(events[1].event_type, "task.preferred_executor_changed");
        assert_eq!(core.timeline(&task_id).await.unwrap()[0].kind, "handoff");
    }

    #[tokio::test]
    async fn edited_context_persists_an_immutable_original_to_final_version_relation() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Editable handoff".into(),
                preferred_executor: executor("fake-a"),
            },
        )))
        .await
        .unwrap();
        let task = core.bootstrap().await.unwrap().tasks.remove(0);
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("fake-a"),
            target_executor: executor("fake-b"),
            after_sequence: 0,
            through_sequence: 2,
        };
        let original = core.build_context_preview(request.clone()).await.unwrap();
        let mut edited_package = original.package.clone();
        edited_package
            .pending_work
            .push("verify Authorization: Bearer abcdefghijklmnop".into());
        let edited = core
            .build_edited_context_preview(ContextEditRequest {
                request: request.clone(),
                base_payload_sha256: original.payload_sha256.clone(),
                edited_package: edited_package.clone(),
            })
            .await
            .unwrap();
        assert_ne!(edited.payload_sha256, original.payload_sha256);
        assert_eq!(edited.package.provenance.redaction_count, 1);
        assert!(!edited.package.pending_work[0].contains("abcdefghijklmnop"));

        let mut confirm =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request,
                expected_payload_sha256: edited.payload_sha256.clone(),
                edited_package: Some(Box::new(edited_package)),
            }));
        confirm.expected_stream_version = Some(task.stream_version);
        core.execute(confirm).await.unwrap();

        let confirmed = core.sync_events(2, 2).await.unwrap().remove(0);
        let original_artifact_id = ArtifactId::parse(
            confirmed.payload["originalArtifact"]["artifactId"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let final_artifact_id = ArtifactId::parse(
            confirmed.payload["artifact"]["artifactId"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_ne!(original_artifact_id, final_artifact_id);
        assert_eq!(
            confirmed.payload["parentArtifactId"].as_str(),
            Some(original_artifact_id.as_str())
        );
        let stored_original = store
            .artifact(&original_artifact_id)
            .await
            .unwrap()
            .unwrap();
        let stored_final = store.artifact(&final_artifact_id).await.unwrap().unwrap();
        assert_eq!(
            stored_original.metadata.payload_sha256,
            original.payload_sha256
        );
        assert_eq!(stored_final.metadata.payload_sha256, edited.payload_sha256);
        assert!(
            !String::from_utf8(stored_final.content)
                .unwrap()
                .contains("abcdefghijklmnop")
        );

        let relation = || {
            store
                .connection
                .lock()
                .unwrap()
                .query_row(
                    "SELECT parent_artifact_id, relation FROM artifact_versions
                     WHERE artifact_id = ?1",
                    [final_artifact_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap()
        };
        assert_eq!(
            relation(),
            (original_artifact_id.to_string(), "user_edit".into())
        );
        let first_page = core
            .task_artifacts(&ActorRef::LocalUser, &task_id, None, 1)
            .await
            .unwrap();
        assert_eq!(first_page.items.len(), 1);
        let second_page = core
            .task_artifacts(
                &ActorRef::LocalUser,
                &task_id,
                first_page.next_cursor.as_ref(),
                1,
            )
            .await
            .unwrap();
        assert_eq!(second_page.items.len(), 1);
        assert!(second_page.next_cursor.is_none());
        assert_eq!(
            first_page.items[0].source_sequence,
            second_page.items[0].source_sequence
        );
        assert_ne!(
            first_page.items[0].metadata.artifact_id,
            second_page.items[0].metadata.artifact_id
        );
        for item in first_page.items.iter().chain(&second_page.items) {
            assert_eq!(
                item.source_kind.ends_with(".original"),
                item.metadata.artifact_id == original_artifact_id
            );
            core.verified_local_artifact(
                &ActorRef::LocalUser,
                &task_id,
                &item.metadata.artifact_id,
                &item.metadata.payload_sha256,
            )
            .await
            .unwrap();
        }
        store.rebuild_projections().unwrap();
        assert_eq!(
            core.task_artifacts(&ActorRef::LocalUser, &task_id, None, 1)
                .await
                .unwrap(),
            first_page
        );
        assert_eq!(
            relation(),
            (original_artifact_id.to_string(), "user_edit".into())
        );
    }

    #[tokio::test]
    async fn records_a_redacted_versioned_test_artifact_and_references_it_from_context() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Tests".into(),
                path: "/tmp/tests".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Persist test evidence".into(),
                preferred_executor: executor("fake-a"),
            },
        )))
        .await
        .unwrap();
        let payload = CoreCommand::RecordTestResult(RecordTestResult {
            task_id: task_id.clone(),
            command: "cargo test".into(),
            outcome: "passed".into(),
            report: Some(
                "contact dev@example.com; Authorization: Bearer abcdefghijklmnop; \
                candidate Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE; \"cookie\":\"sid=synthetic\""
                    .into(),
            ),
        });
        let mut record = CommandEnvelope::local(payload);
        record.expected_stream_version = Some(1);
        assert!(matches!(
            core.execute(record.clone()).await,
            Err(agent_core::CoreError::InvalidArgument(_))
        ));
        record.actor = ActorRef::SystemRecovery;
        core.execute(record).await.unwrap();

        let event = core.sync_events(2, 1).await.unwrap().remove(0);
        assert_eq!(event.event_type, "test.completed");
        let artifact_id =
            ArtifactId::parse(event.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
        assert_eq!(
            event.payload["artifactRef"].as_str(),
            Some(format!("artifact:{artifact_id}").as_str())
        );
        let artifact = store.artifact(&artifact_id).await.unwrap().unwrap();
        let report: TestReportV1 = serde_json::from_slice(&artifact.content).unwrap();
        assert_eq!(
            core.verified_local_artifact(
                &ActorRef::LocalUser,
                &task_id,
                &artifact_id,
                &artifact.metadata.payload_sha256
            )
            .await
            .unwrap()
            .content,
            artifact.content
        );
        assert_eq!(report.command, "cargo test");
        assert_eq!(report.outcome, "passed");
        assert_eq!(artifact.metadata.redaction_count, 4);
        assert_eq!(artifact.metadata.redaction_ruleset_version, 2);
        let content = String::from_utf8(artifact.content).unwrap();
        assert!(!content.contains("dev@example.com"));
        assert!(!content.contains("abcdefghijklmnop"));
        assert!(!content.contains("Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE"));
        assert!(!content.contains("sid=synthetic"));

        let preview = core
            .build_context_preview(ContextPreviewRequest {
                task_id: task_id.clone(),
                kind: ContextPackageKind::Handoff,
                strength: ContextStrength::Standard,
                source_executor: executor("fake-a"),
                target_executor: executor("fake-b"),
                after_sequence: 0,
                through_sequence: 3,
            })
            .await
            .unwrap();
        assert_eq!(preview.package.tests.len(), 1);
        assert_eq!(
            preview.package.tests[0].artifact_ref.as_deref(),
            event.payload["artifactRef"].as_str()
        );
        let timeline = core.timeline(&task_id).await.unwrap();
        assert_eq!(timeline[0].kind, "test_result");
        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn a_newer_terminal_transfer_masks_an_older_failed_version_for_the_same_target() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Queue".into(),
                path: "/tmp/queue".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Latest action wins".into(),
                preferred_executor: executor("fake-a"),
            },
        )))
        .await
        .unwrap();

        let old_artifact = ArtifactId::new();
        let new_artifact = ArtifactId::new();
        let source_json = serde_json::to_string(&executor("fake-a")).unwrap();
        let target_json = serde_json::to_string(&executor("fake-b")).unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO artifact_blobs(payload_sha256, byte_length, content)
                     VALUES ('queue-test', 2, X'7B7D')",
                    [],
                )
                .unwrap();
            for artifact_id in [&old_artifact, &new_artifact] {
                connection
                    .execute(
                        "INSERT INTO artifacts(
                            artifact_id, payload_sha256, media_type,
                            redaction_ruleset_version, redaction_count, created_at
                         ) VALUES (?1, 'queue-test', 'application/json', 1, 0, 0)",
                        [artifact_id.as_str()],
                    )
                    .unwrap();
            }
            connection
                .execute(
                    "INSERT INTO context_transfers(
                        artifact_id, task_id, kind, source_executor_json, target_executor_json,
                        source_after_sequence, source_through_sequence, status,
                        confirmed_sequence, last_event_sequence
                     ) VALUES (?1, ?2, 'handoff', ?3, ?4, 0, 10, 'failed', 10, 11),
                              (?5, ?2, 'handoff', ?3, ?4, 0, 20, 'sent', 20, 21)",
                    params![
                        old_artifact.as_str(),
                        task_id.as_str(),
                        source_json,
                        target_json,
                        new_artifact.as_str(),
                    ],
                )
                .unwrap();
        }

        assert!(
            store
                .pending_context_transfer(&task_id, &executor("fake-b"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deduplicates_commands_and_enforces_stream_versions() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store));
        let project_id = ProjectId::new();
        let command = CommandEnvelope::local(CoreCommand::RegisterProject(RegisterProject {
            additional_directories: vec![],
            project_id: project_id.clone(),
            display_name: "Demo".into(),
            path: "/tmp/demo".into(),
        }));
        let first = core.execute(command.clone()).await.unwrap();
        let duplicate = core.execute(command).await.unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(core.bootstrap().await.unwrap().last_sequence, 1);

        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "First task".into(),
                preferred_executor: executor("codex"),
            },
        )))
        .await
        .unwrap();
        let mut stale =
            CommandEnvelope::local(CoreCommand::SetPreferredExecutor(SetPreferredExecutor {
                task_id,
                executor: executor("cursor"),
            }));
        stale.expected_stream_version = Some(0);
        assert!(matches!(
            core.execute(stale).await,
            Err(agent_core::CoreError::Repository(
                RepositoryError::Conflict { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn fake_driver_streams_into_timeline_and_companion_projection() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(FakeDriver::with_executor(executor("fake"))),
        )
        .await;
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Stream a turn".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &task_id, "implement the event loop").await;

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let snapshot = core.bootstrap().await.unwrap();
                if snapshot.active_runs.is_empty()
                    && snapshot
                        .session_maps
                        .first()
                        .is_some_and(|mapping| mapping.status == SessionMapStatus::Resumable)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let timeline = core.timeline(&task_id).await.unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].kind, "user_message");
        assert_eq!(
            timeline[1].text,
            "Fake agent received: implement the event loop"
        );
        let events = store
            .task_events(&task_id, 0, i64::MAX as u64, 1000)
            .await
            .unwrap();
        let started = events
            .iter()
            .find(|event| event.event_type == "turn.started")
            .unwrap();
        let completed = events
            .iter()
            .find(|event| event.event_type == "turn.completed")
            .unwrap();
        for item in &timeline {
            let timing = item.turn_completion.as_ref().expect("completed timing");
            assert_eq!(
                timing.started_at.timestamp_millis(),
                started.occurred_at.timestamp_millis()
            );
            assert_eq!(
                timing.completed_at.timestamp_millis(),
                completed.occurred_at.timestamp_millis()
            );
        }
        let snapshot = core.bootstrap().await.unwrap();
        let mapping = &snapshot.session_maps[0];
        assert_eq!(mapping.task_id, task_id);
        assert_eq!(mapping.executor, executor("fake"));
        assert!(mapping.native_session_id.starts_with("fake_"));
        assert!(mapping.last_native_turn_id.is_some());
        assert_eq!(mapping.resume_capability, ResumeCapability::Supported);
        assert_eq!(mapping.status, SessionMapStatus::Resumable);
        assert!(mapping.last_seen_sequence < snapshot.last_sequence);
        let companion = snapshot.companion;
        assert!(companion.workspaces[0].active_primary_run.is_none());
        assert_eq!(companion.recent_completion.unwrap().provider_id, "fake");

        let before_rebuild = core.bootstrap().await.unwrap().session_maps;
        store.rebuild_projections().unwrap();
        assert_eq!(core.bootstrap().await.unwrap().session_maps, before_rebuild);
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn cancel_turn_targets_the_active_native_session_and_rebuilds() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver = Arc::new(InterruptibleDriver::new());
        fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Interrupt".into(),
                path: "/tmp/interrupt".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Interrupt a native turn".into(),
                preferred_executor: executor("interruptible"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &task_id, "wait").await;

        let task = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let snapshot = core.bootstrap().await.unwrap();
                let task = snapshot.tasks[0].clone();
                if task.active_primary_run.is_some()
                    && snapshot
                        .session_maps
                        .first()
                        .is_some_and(|mapping| mapping.status == SessionMapStatus::Active)
                {
                    break task;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let active = task.active_primary_run.clone().unwrap();
        // A restarted Core must not claim ownership of another instance's run.
        let restarted = Arc::new(Core::new(store.clone()));
        let before_diagnostics = core.sync_events(0, 1_000).await.unwrap().len();
        let diagnostics = restarted
            .recovery_snapshot(&ActorRef::LocalUser)
            .await
            .unwrap();
        let orphan = diagnostics
            .items
            .iter()
            .find(|item| {
                item.code == agent_protocol::RecoveryCode::RunOwnershipUnknown
                    && item.run_id.as_ref() == Some(&active.run_id)
            })
            .unwrap();
        assert!(!orphan.can_cancel);
        let exported = serde_json::to_string(&diagnostics).unwrap();
        assert!(!exported.contains("/tmp/interrupt"));
        assert!(!exported.contains("Interrupt a native turn"));
        assert_eq!(
            core.sync_events(0, 1_000).await.unwrap().len(),
            before_diagnostics
        );
        let mut foreign_cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
            task_id: task_id.clone(),
            turn_id: active.turn_id.clone(),
            run_id: active.run_id.clone(),
        }));
        foreign_cancel.expected_stream_version = Some(task.stream_version);
        assert!(matches!(
            restarted.execute(foreign_cancel).await,
            Err(agent_core::CoreError::Conflict(_))
        ));
        assert!(!driver.interrupt_called.load(Ordering::Acquire));
        let mut blocked_submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: vec![],
            permission_mode: None,
            thinking: None,
            model: None,
            task_id: task_id.clone(),
            prompt: "must not replace the active run".into(),
            executor_override: None,
        }));
        blocked_submit.expected_stream_version = Some(task.stream_version);
        assert!(matches!(
            core.execute(blocked_submit).await,
            Err(agent_core::CoreError::Conflict(_))
        ));
        let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
            task_id: task_id.clone(),
            turn_id: active.turn_id,
            run_id: active.run_id,
        }));
        cancel.expected_stream_version = Some(task.stream_version);
        let accepted = core.execute(cancel.clone()).await.unwrap();
        // Resending the same stop after a lost reply returns the original receipt.
        assert_eq!(
            core.execute(cancel.clone()).await.unwrap().command_id,
            accepted.command_id
        );

        let events = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = core.sync_events(0, 1_000).await.unwrap();
                if events
                    .iter()
                    .any(|event| event.event_type == "turn.interrupt_sent")
                    && events.iter().any(|event| {
                        event.event_type == "turn.failed"
                            && event
                                .payload
                                .get("failureKind")
                                .and_then(|value| value.as_str())
                                == Some("interrupted")
                    })
                {
                    break events;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(driver.interrupt_called.load(Ordering::Acquire));
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "turn.cancel_requested")
        );
        assert!(core.bootstrap().await.unwrap().active_runs.is_empty());

        let before_rebuild = core.bootstrap().await.unwrap();
        store.rebuild_projections().unwrap();
        let after_rebuild = core.bootstrap().await.unwrap();
        assert_eq!(after_rebuild.tasks, before_rebuild.tasks);
        assert_eq!(after_rebuild.session_maps, before_rebuild.session_maps);
    }

    #[tokio::test]
    async fn model_and_thinking_are_validated_dispatched_and_rebuilt() {
        for (reuse, reconfigure_supported) in [(false, true), (true, true), (true, false)] {
            let store = Arc::new(SqliteStore::open_in_memory().unwrap());
            let core = Arc::new(Core::new(store.clone()));
            let mut driver = ScriptedDriver::new(
                "scripted",
                vec![DriverEvent::TurnCompleted {
                    summary: "done".into(),
                }],
            );
            driver.reuse = reuse;
            driver.reconfigure_supported = reconfigure_supported;
            let driver = Arc::new(driver);
            fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
            let task_id = scripted_task(&core).await;
            let task = core.bootstrap().await.unwrap().tasks[0].clone();
            let submit = |thinking: &str| {
                let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
                    review_fix: None,
                    attachments: vec![],
                    permission_mode: None,
                    task_id: task_id.clone(),
                    prompt: "fixture".into(),
                    executor_override: None,
                    model: Some("fixture-model".into()),
                    thinking: Some(thinking.into()),
                }));
                command.expected_stream_version = Some(task.stream_version);
                command
            };
            assert!(matches!(
                core.execute(submit("unsupported")).await,
                Err(agent_core::CoreError::InvalidArgument(_))
            ));
            assert!(core.bootstrap().await.unwrap().session_maps.is_empty());
            core.execute(submit("high")).await.unwrap();
            wait_until_idle(&core).await;
            let mappings = core.bootstrap().await.unwrap().session_maps;
            assert_eq!(mappings[0].model.as_deref(), Some("fixture-model"));
            assert_eq!(mappings[0].thinking.as_deref(), Some("high"));
            // Resetting thinking and then the model must reach the driver, never
            // silently inherit the old selection. Supported changes retain native history.
            for (model, thinking) in [
                (Some("other-model"), None),
                (Some("fixture-model"), None),
                (None, None),
                (Some("fixture-model"), Some("high")),
            ] {
                let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
                    review_fix: None,
                    attachments: vec![],
                    permission_mode: None,
                    task_id: task_id.clone(),
                    prompt: "continue with the new selection".into(),
                    executor_override: None,
                    model: model.map(str::to_owned),
                    thinking: thinking.map(str::to_owned),
                }));
                command.expected_stream_version =
                    Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
                core.execute(command).await.unwrap();
                wait_until_idle(&core).await;
                assert_eq!(
                    driver.selections.lock().unwrap().last().cloned(),
                    Some((model.map(str::to_owned), thinking.map(str::to_owned)))
                );
                let current = core.bootstrap().await.unwrap().session_maps;
                assert_eq!(current[0].model.as_deref(), model);
                assert_eq!(current[0].thinking.as_deref(), thinking);
                let preserved = reuse && reconfigure_supported;
                assert_eq!(
                    driver.contexts.lock().unwrap().last().unwrap().is_empty(),
                    preserved
                );
                assert_eq!(
                    current[0].session_map_id == mappings[0].session_map_id,
                    preserved
                );
            }
            let created = driver.selections.lock().unwrap().len();
            let mut unchanged = submit("high");
            unchanged.expected_stream_version =
                Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
            core.execute(unchanged).await.unwrap();
            wait_until_idle(&core).await;
            assert_eq!(
                driver.selections.lock().unwrap().len(),
                created + usize::from(!reuse)
            );
            assert_eq!(
                driver.contexts.lock().unwrap().last().unwrap().is_empty(),
                reuse
            );
            let mut invalid = submit("unsupported");
            invalid.expected_stream_version =
                Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
            assert!(matches!(
                core.execute(invalid).await,
                Err(agent_core::CoreError::InvalidArgument(_))
            ));
            let mappings = core.bootstrap().await.unwrap().session_maps;
            store.rebuild_projections().unwrap();
            assert_eq!(core.bootstrap().await.unwrap().session_maps, mappings);
        }
    }

    #[tokio::test]
    async fn driver_failures_persist_only_stable_reasons() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let private_reason = "sk-test-1234567890abcdef failed in /Users/secret-user";
        let private_warning = "ghp_1234567890abcdefghijklmnopqrstuvwxyz leaked";
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![
                    DriverEvent::TextDelta {
                        item_id: "item-1".into(),
                        text: "partial output".into(),
                    },
                    DriverEvent::TurnFailed {
                        reason: private_reason.into(),
                    },
                    DriverEvent::VendorExtension {
                        method: "_vendor/secret".into(),
                        payload: serde_json::json!({ "token": private_warning }),
                    },
                    DriverEvent::ProtocolWarning {
                        message: private_warning.into(),
                    },
                ],
            )),
        )
        .await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "stream a failing turn").await;
        wait_until_idle(&core).await;

        let events = core.sync_events(0, 10_000).await.unwrap();
        let failure = events
            .iter()
            .find(|event| event.event_type == "turn.failed")
            .unwrap();
        assert_eq!(failure.payload["failureKind"], "driver_turn_failed");
        assert_eq!(failure.payload["message"], "turn execution failed");
        let unknown = events
            .iter()
            .find(|event| event.event_type == "driver.unknown_event_observed")
            .unwrap();
        assert!(unknown.payload.get("safePayload").is_none());
        assert!(unknown.payload["payloadBytes"].as_u64().unwrap() > 0);
        let warning = events
            .iter()
            .find(|event| event.event_type == "driver.protocol_warning")
            .unwrap();
        assert_ne!(warning.payload["message"], private_warning);
        let persisted = serde_json::to_string(&events).unwrap();
        assert!(!persisted.contains(private_reason));
        assert!(!persisted.contains(private_warning));

        let timeline = core.timeline(&task_id).await.unwrap();
        assert!(
            timeline
                .iter()
                .any(|item| { item.kind == "assistant_message" && item.text == "partial output" })
        );
        assert!(!timeline.iter().any(|item| {
            matches!(
                item.kind.as_str(),
                "driver.protocol_warning" | "driver.unknown_event_observed"
            )
        }));

        store.rebuild_projections().unwrap();
        let rebuilt_timeline = core.timeline(&task_id).await.unwrap();
        assert_eq!(rebuilt_timeline, timeline);
    }

    #[tokio::test]
    async fn classified_failures_show_actionable_text_without_native_secrets() {
        for (reason, expected, message) in [
            (
                "Claude Code 身份验证失败 Bearer secret-token",
                "authentication_failed",
                "登录或认证失败",
            ),
            (
                "NOT NULL constraint failed: session_message.seq password=secret-path",
                "database_incompatible",
                "版本不兼容",
            ),
            (
                "request timed out api_key=secret-host",
                "agent_timeout",
                "响应超时",
            ),
            (
                "driver_not_configured",
                "driver_not_configured",
                "尚未配置好",
            ),
            (
                "OpenCode 未返回任何内容或工具活动",
                "agent_no_response",
                "没有返回回复",
            ),
        ] {
            let store = Arc::new(SqliteStore::open_in_memory().unwrap());
            let core = Arc::new(Core::new(store.clone()));
            fixture_installations::register(
                &core,
                store.as_ref(),
                Arc::new(ScriptedDriver::new(
                    "scripted",
                    vec![DriverEvent::TurnFailed {
                        reason: reason.into(),
                    }],
                )),
            )
            .await;
            let task_id = scripted_task(&core).await;
            submit_turn(&core, &task_id, "test failure presentation").await;
            wait_until_idle(&core).await;
            let events = core.sync_events(0, 10_000).await.unwrap();
            let failure = events
                .iter()
                .find(|e| e.event_type == "turn.failed")
                .unwrap();
            assert_eq!(failure.payload["failureKind"], expected);
            // The agent's own error text is kept for the user, minus secrets.
            assert!(!serde_json::to_string(&events).unwrap().contains("secret-"));
            let timeline = core.timeline(&task_id).await.unwrap();
            let failed = timeline
                .iter()
                .find(|item| item.kind == "turn.failed")
                .unwrap();
            assert!(failed.text.contains(message));
            if reason != expected {
                let visible = reason.split_whitespace().next().unwrap();
                assert!(failed.text.contains(visible), "{}", failed.text);
                assert!(failure.payload["agentError"].is_string(), "{reason}");
            }
            store.rebuild_projections().unwrap();
            assert_eq!(timeline, core.timeline(&task_id).await.unwrap());
        }
    }

    #[tokio::test]
    async fn quota_exhaustion_projects_a_safe_visible_timeline_item() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![DriverEvent::TurnQuotaExhausted {
                    resets_at_unix: Some(1_800_000_000),
                }],
            )),
        )
        .await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "consume the final allowance").await;
        wait_until_idle(&core).await;

        let events = core.sync_events(0, 10_000).await.unwrap();
        let failure = events
            .iter()
            .find(|event| event.event_type == "turn.failed")
            .unwrap();
        assert_eq!(failure.payload["failureKind"], "quota_exhausted");
        assert_eq!(failure.payload["resetsAtUnix"], 1_800_000_000_i64);
        assert_eq!(failure.payload["executor"]["providerId"], "scripted");

        let timeline = core.timeline(&task_id).await.unwrap();
        let exhausted = timeline
            .iter()
            .find(|item| item.kind == "quota.exhausted")
            .unwrap();
        assert_eq!(exhausted.provider_id.as_deref(), Some("scripted"));
        assert!(exhausted.text.contains("额度已用尽"));

        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn background_child_settlement_survives_parent_terminal_and_replay() {
        for kind in ["subagent", "tool"] {
            let store = Arc::new(SqliteStore::open_in_memory().unwrap());
            let core = Arc::new(Core::new(store.clone()));
            fixture_installations::register(&core, store.as_ref(), Arc::new(ScriptedDriver::new(
            "scripted",
            vec![
                DriverEvent::Presentation {
                    item_id: "child".into(),
                    data: json!({"type":kind,"status":"running","background":kind == "tool"}),
                },
                DriverEvent::TurnCompleted {
                    summary: "parent done".into(),
                },
                DriverEvent::Presentation {
                    item_id: "unrelated".into(),
                    data: json!({"type":"tool","status":"completed","output":"must_ignore"}),
                },
                DriverEvent::Presentation {
                    item_id: "child".into(),
                    data: json!({"type":kind,"status":"completed","output":"child done","background":kind == "tool"}),
                },
            ],
        )))
        .await;
            let task = scripted_task(&core).await;
            submit_turn(&core, &task, "go").await;
            wait_until_idle(&core).await;
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let events = core.sync_events(0, 1000).await.unwrap();
                    if let Some(child) = events
                        .iter()
                        .find(|e| e.payload["data"]["output"] == "child done")
                    {
                        let parent = events
                            .iter()
                            .find(|e| e.event_type == "turn.completed")
                            .unwrap();
                        assert!(child.sequence > parent.sequence);
                        assert_eq!(child.payload["turnId"], parent.payload["turnId"]);
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let timeline = core.timeline(&task).await.unwrap();
            assert!(timeline.iter().any(|i| i.text.contains("child done")));
            assert!(!timeline.iter().any(|i| i.text.contains("must_ignore")));
            store.rebuild_projections().unwrap();
            assert_eq!(core.timeline(&task).await.unwrap(), timeline);
        }
    }

    #[tokio::test]
    async fn structured_activity_preserves_order_fields_and_published_thought_history() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(&core, store.as_ref(), Arc::new(ScriptedDriver::new("scripted",vec![
            DriverEvent::Presentation{item_id:"thought".into(),data:json!({"type":"thought","delta":"published-display-fragment","status":"running"})},
            DriverEvent::TextDelta{item_id:"message".into(),text:"Before".into()},
            DriverEvent::Presentation{item_id:"read".into(),data:json!({"type":"tool","name":"Read","status":"running","input":{"file_path":"file.txt"}})},
            DriverEvent::ToolStarted{item_id:"read".into(),tool_name:"Read".into(),summary:"old".into()},
            DriverEvent::Presentation{item_id:"read".into(),data:json!({"type":"tool","status":"completed","output":"ok"})},
            DriverEvent::Presentation{item_id:"read".into(),data:json!({"type":"tool","status":"running"})},
            DriverEvent::TextDelta{item_id:"message".into(),text:"After".into()},
            DriverEvent::Presentation{item_id:"read".into(),data:json!({"type":"tool","status":"completed","output":"ok"})},
            DriverEvent::ToolCompleted{item_id:"read".into(),tool_name:"Read".into(),summary:"late completion".into(),success:true},
            DriverEvent::TextDelta{item_id:"message".into(),text:" continued".into()},
            DriverEvent::TurnCompleted{summary:"done".into()},
        ]))).await;
        let task = scripted_task(&core).await;
        submit_turn(&core, &task, "go").await;
        wait_until_idle(&core).await;
        let timeline = core.timeline(&task).await.unwrap();
        let messages: Vec<_> = timeline
            .iter()
            .filter(|i| i.kind == "assistant_message")
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "Before");
        assert_eq!(messages[1].text, "After continued");
        let tool = timeline
            .iter()
            .find(|i| i.kind == "activity" && i.item_id.ends_with(":read"))
            .unwrap();
        let data: serde_json::Value = serde_json::from_str(&tool.text).unwrap();
        assert_eq!(data["name"], "Read");
        assert_eq!(data["status"], "completed");
        assert_eq!(data["input"]["file_path"], "file.txt");
        assert!(messages[0].sequence < tool.sequence && tool.sequence < messages[1].sequence);
        let events = core.sync_events(0, 1000).await.unwrap();
        assert!(
            serde_json::to_string(&events)
                .unwrap()
                .contains("published-display-fragment")
        );
        assert!(
            serde_json::to_string(&timeline)
                .unwrap()
                .contains("published-display-fragment")
        );
        let thought = timeline
            .iter()
            .filter(|item| item.kind == "activity")
            .map(|item| serde_json::from_str::<Value>(&item.text).unwrap())
            .find(|value| value["type"] == "thought")
            .unwrap();
        assert_eq!(thought["status"], "completed");
        assert_eq!(thought["text"], "published-display-fragment");
        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn tool_lifecycle_projects_as_one_user_visible_activity() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![
                    DriverEvent::ToolStarted {
                        item_id: "search-1".into(),
                        tool_name: "web_search".into(),
                        summary: "搜索网页".into(),
                    },
                    DriverEvent::ToolCompleted {
                        item_id: "search-1".into(),
                        tool_name: "web_search".into(),
                        summary: "搜索网页".into(),
                        success: true,
                    },
                    DriverEvent::TurnCompleted {
                        summary: "done".into(),
                    },
                ],
            )),
        )
        .await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "search").await;
        wait_until_idle(&core).await;

        let timeline = core.timeline(&task_id).await.unwrap();
        let activities = timeline
            .iter()
            .filter(|item| item.kind.starts_with("tool."))
            .collect::<Vec<_>>();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].kind, "tool.completed");
        assert_eq!(activities[0].text, "搜索网页");
        assert_eq!(activities[0].provider_id.as_deref(), Some("scripted"));

        let events = core.sync_events(0, 10_000).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool.started")
        );
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool.completed")
        );

        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn repeated_native_message_ids_are_isolated_across_turns_and_replay() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![
                    DriverEvent::TextDelta {
                        item_id: "message".into(),
                        text: "O".into(),
                    },
                    DriverEvent::TextDelta {
                        item_id: "message".into(),
                        text: "K".into(),
                    },
                    DriverEvent::TurnCompleted {
                        summary: "done".into(),
                    },
                ],
            )),
        )
        .await;
        let task = scripted_task(&core).await;
        for _ in 0..2 {
            submit_turn(&core, &task, "test").await;
            wait_until_idle(&core).await;
        }
        let before = core.timeline(&task).await.unwrap();
        let messages: Vec<_> = before
            .iter()
            .filter(|i| i.kind == "assistant_message")
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|i| i.text == "OK"));
        assert_ne!(messages[0].item_id, messages[1].item_id);
        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task).await.unwrap(), before);
    }

    #[tokio::test]
    async fn native_session_identity_survives_projection_rebuild() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![
                    DriverEvent::SessionReady {
                        native_session_id: "durable-native-session".into(),
                    },
                    DriverEvent::TurnCompleted {
                        summary: "done".into(),
                    },
                ],
            )),
        )
        .await;
        let task = scripted_task(&core).await;
        submit_turn(&core, &task, "test").await;
        wait_until_idle(&core).await;
        assert_eq!(
            core.bootstrap().await.unwrap().session_maps[0].native_session_id,
            "durable-native-session"
        );
        let original = core
            .sync_events(0, 100)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.event_type == "driver.session_ready")
            .unwrap();
        let mut late = agent_protocol::NewEvent::hub(
            original.stream.clone(),
            "driver.session_ready",
            original.payload.clone(),
        );
        late.source = original.source;
        late.payload["nativeSessionId"] = json!("late-wrong-session");
        store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: agent_domain::CommandId::new(),
                idempotency_key: late.event_id.to_string(),
                expected_stream_version: None,
                artifacts: vec![],
                events: vec![late],
            })
            .await
            .unwrap();
        assert_eq!(
            core.bootstrap().await.unwrap().session_maps[0].native_session_id,
            "durable-native-session"
        );
        store.rebuild_projections().unwrap();
        assert_eq!(
            core.bootstrap().await.unwrap().session_maps[0].native_session_id,
            "durable-native-session"
        );
    }

    #[tokio::test]
    async fn stream_ending_without_terminal_event_closes_the_turn() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new(
                "scripted",
                vec![DriverEvent::TextDelta {
                    item_id: "item-1".into(),
                    text: "output without an outcome".into(),
                }],
            )),
        )
        .await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "stream without an outcome").await;
        wait_until_idle(&core).await;

        let events = core.sync_events(0, 10_000).await.unwrap();
        let failure = events
            .iter()
            .rfind(|event| event.event_type == "turn.failed")
            .unwrap();
        assert_eq!(failure.payload["failureKind"], "driver_stream_ended");
        let snapshot = core.bootstrap().await.unwrap();
        assert!(snapshot.active_runs.is_empty());
        assert!(snapshot.tasks[0].active_primary_run.is_none());
        // The task must accept a follow-up turn instead of staying pinned.
        submit_turn(&core, &task_id, "retry after the silent stream").await;
        wait_until_idle(&core).await;
    }

    #[tokio::test]
    async fn submit_turn_requires_the_current_task_stream_version() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let task_id = scripted_task(&core).await;

        let missing_version = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: vec![],
            permission_mode: None,
            thinking: None,
            model: None,
            task_id: task_id.clone(),
            prompt: "no expected version".into(),
            executor_override: None,
        }));
        assert!(matches!(
            core.execute(missing_version).await,
            Err(agent_core::CoreError::InvalidArgument(_))
        ));
        let mut stale_version = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: vec![],
            permission_mode: None,
            thinking: None,
            model: None,
            task_id: task_id.clone(),
            prompt: "stale expected version".into(),
            executor_override: None,
        }));
        // A valid version without an available adapter is rejected before any
        // durable acceptance. Then register one to isolate the CAS assertion.
        fixture_installations::seed(store.as_ref(), &executor("scripted")).await;
        let mut unavailable = stale_version.clone();
        unavailable.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        assert!(matches!(
            core.execute(unavailable.clone()).await,
            Err(agent_core::CoreError::DriverUnavailable(_))
        ));
        assert!(
            store
                .command_ack(&unavailable.idempotency_key)
                .await
                .unwrap()
                .is_none()
        );
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new("scripted", vec![])),
        )
        .await;
        stale_version.expected_stream_version = Some(99);
        assert!(matches!(
            core.execute(stale_version).await,
            Err(agent_core::CoreError::Repository(
                agent_core::RepositoryError::Conflict { .. }
            ))
        ));
        let events = core.sync_events(0, 1_000).await.unwrap();
        assert!(!events.iter().any(|event| event.event_type == "turn.queued"));
    }

    #[tokio::test]
    async fn questions_persist_validate_answers_and_deliver_once() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let mut driver = ApprovalDriver::new("scripted");
        driver.question = true;
        let driver = Arc::new(driver);
        fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
        let task = scripted_task(&core).await;
        submit_turn(&core, &task, "ask me").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while core.pending_approvals(&task, 10).await.unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let pending = core.pending_approvals(&task, 10).await.unwrap();
        assert!(pending[0].question.is_some());
        assert!(pending[0].options.is_empty());
        assert_eq!(
            core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .as_ref()
                .unwrap()
                .state,
            RunState::WaitingForInput
        );
        store.rebuild_projections().unwrap();
        assert_eq!(core.pending_approvals(&task, 10).await.unwrap(), pending);
        let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
        let reply = |value: &str| {
            let mut e = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                turn_id: pending[0].turn_id.clone(),
                requested_at_sequence: pending[0].requested_at_sequence,
                task_id: task.clone(),
                approval_id: "perm-1".into(),
                option_id: String::new(),
                answer: Some(agent_domain::QuestionAnswer {
                    cancelled: false,
                    values: json!({"color":value}),
                }),
            }));
            e.expected_stream_version = Some(version);
            e
        };
        assert!(core.execute(reply("invented")).await.is_err());
        assert!(driver.recorded.lock().unwrap().is_empty());
        let (a, b) = tokio::join!(core.execute(reply("red")), core.execute(reply("blue")));
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        wait_until_idle(&core).await;
        assert_eq!(driver.recorded.lock().unwrap().len(), 1);
        assert!(core.pending_approvals(&task, 10).await.unwrap().is_empty());
        assert!(core.execute(reply("red")).await.is_err());
    }

    #[tokio::test]
    async fn concurrent_and_late_approval_replies_cannot_approve_a_new_turn() {
        use agent_domain::CommandId;
        use agent_protocol::NewEvent;
        async fn pending_version(core: &Arc<Core>, task: &TaskId) -> u64 {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if !core.pending_approvals(task, 10).await.unwrap().is_empty() {
                        return core
                            .bootstrap()
                            .await
                            .unwrap()
                            .tasks
                            .into_iter()
                            .find(|t| &t.task_id == task)
                            .unwrap()
                            .stream_version;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap()
        }
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver = Arc::new(ApprovalDriver::new("approvals"));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::clone(&driver) as Arc<dyn AgentDriver>,
        )
        .await;
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Approvals".into(),
                path: "/tmp/approvals".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Approval flow".into(),
                preferred_executor: executor("approvals"),
            },
        )))
        .await
        .unwrap();
        let reply = |version, source: &PendingApproval, option: &str| {
            let mut command =
                CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                    turn_id: source.turn_id.clone(),
                    requested_at_sequence: source.requested_at_sequence,
                    answer: None,
                    task_id: task_id.clone(),
                    approval_id: "perm-1".into(),
                    option_id: option.into(),
                }));
            command.expected_stream_version = Some(version);
            command
        };
        submit_turn(&core, &task_id, "first approval").await;
        let first_version = pending_version(&core, &task_id).await;
        let first_source = core
            .pending_approvals(&task_id, 10)
            .await
            .unwrap()
            .remove(0);
        // Two surfaces act on the same snapshot. Exactly one decision may commit.
        let (left, right) = tokio::join!(
            core.execute(reply(first_version, &first_source, "allow")),
            core.execute(reply(first_version, &first_source, "deny"))
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        assert!(matches!(
            left.as_ref().err().or(right.as_ref().err()),
            Some(
                agent_core::CoreError::Conflict(_)
                    | agent_core::CoreError::Repository(RepositoryError::Conflict { .. })
            )
        ));
        wait_until_idle(&core).await;
        assert_eq!(driver.recorded.lock().unwrap().len(), 1);

        submit_turn(&core, &task_id, "new turn reuses native approval ID").await;
        let second_version = pending_version(&core, &task_id).await;
        let second_source = core
            .pending_approvals(&task_id, 10)
            .await
            .unwrap()
            .remove(0);
        assert!(second_version > first_version);
        // Stream events can update the task version while an old approval is
        // still rendered. Even a fresh version must not authorize the new turn.
        let late_result = core
            .execute(reply(second_version, &first_source, "allow"))
            .await;
        assert!(
            matches!(late_result, Err(agent_core::CoreError::Conflict(_))),
            "{late_result:?}"
        );
        assert_eq!(driver.recorded.lock().unwrap().len(), 1);
        assert_eq!(core.pending_approvals(&task_id, 10).await.unwrap().len(), 1);
        // The same turn can also reuse an approval ID. A newer request event
        // must invalidate a stale button even if its task version was refreshed.
        let requested = core
            .sync_events(0, 10000)
            .await
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.event_type == "approval.requested")
            .unwrap();
        store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: CommandId::new().to_string(),
                expected_stream_version: Some(second_version),
                artifacts: vec![],
                events: vec![NewEvent::hub(
                    requested.stream,
                    "approval.requested",
                    requested.payload,
                )],
            })
            .await
            .unwrap();
        let new_version = pending_version(&core, &task_id).await;
        let new_source = core
            .pending_approvals(&task_id, 10)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(new_source.turn_id, second_source.turn_id);
        assert_ne!(
            new_source.requested_at_sequence,
            second_source.requested_at_sequence
        );
        assert!(matches!(
            core.execute(reply(new_version, &second_source, "allow"))
                .await,
            Err(agent_core::CoreError::Conflict(_))
        ));
        assert_eq!(driver.recorded.lock().unwrap().len(), 1);
        core.execute(reply(new_version, &new_source, "deny"))
            .await
            .unwrap();
        wait_until_idle(&core).await;
        assert_eq!(driver.recorded.lock().unwrap().len(), 2);
        assert_eq!(driver.recorded.lock().unwrap()[1].1, "deny");
        let events = core.sync_events(0, 10000).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "approval.resolved")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn approvals_persist_verbatim_and_reach_the_driver_once() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver = Arc::new(ApprovalDriver::new("approvals"));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::clone(&driver) as Arc<dyn AgentDriver>,
        )
        .await;
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Approvals".into(),
                path: "/tmp/approvals".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Approval flow".into(),
                preferred_executor: executor("approvals"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &task_id, "do something that needs approval").await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let pending = loop {
            let pending = store.pending_approvals(&task_id, 50).await.unwrap();
            if !pending.is_empty() || std::time::Instant::now() > deadline {
                break pending;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(pending.len(), 1);
        assert_eq!(
            core.bootstrap()
                .await
                .unwrap()
                .companion
                .workspaces
                .iter()
                .find(|w| w.task_id == task_id)
                .unwrap()
                .pending_interaction_count,
            1
        );
        assert_eq!(pending[0].approval_id, "perm-1");
        assert_eq!(pending[0].tool_name, "Bash");
        assert!(
            pending[0]
                .options
                .iter()
                .any(|option| option.semantic_role == "approve_once")
        );

        let events = core.sync_events(0, 10_000).await.unwrap();
        let requested = events
            .iter()
            .find(|event| event.event_type == "approval.requested")
            .unwrap();
        assert_eq!(requested.payload["approvalId"], "perm-1");
        let persisted = serde_json::to_string(&events).unwrap();
        assert!(persisted.contains("sk-test-1234567890abcdef"));

        let version = core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .stream_version;
        let mut respond =
            CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                turn_id: pending[0].turn_id.clone(),
                requested_at_sequence: pending[0].requested_at_sequence,
                answer: None,
                task_id: task_id.clone(),
                approval_id: "perm-1".into(),
                option_id: "allow".into(),
            }));
        respond.expected_stream_version = Some(version);
        let query = core.prepare_approval_response(&respond).unwrap();
        assert!(core.approval_receipt(&query).await.unwrap().is_none());
        let accepted = core.execute(respond.clone()).await.unwrap();
        // Retrying the identical logical request must not deliver twice.
        assert_eq!(core.execute(respond).await.unwrap(), accepted);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let receipt = core.approval_receipt(&query).await.unwrap().unwrap();
                assert_eq!(receipt.acknowledgement, accepted);
                if receipt.delivery == agent_core::approval_receipts::ApprovalDelivery::Sent {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if driver.recorded.lock().unwrap().len() == 1 || std::time::Instant::now() > deadline {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            driver.recorded.lock().unwrap().as_slice(),
            [("perm-1".to_owned(), "allow".to_owned())]
        );

        assert_eq!(
            core.bootstrap()
                .await
                .unwrap()
                .companion
                .workspaces
                .iter()
                .find(|w| w.task_id == task_id)
                .unwrap()
                .pending_interaction_count,
            0
        );
        // The decision is recorded exactly once; a replay finds nothing pending.
        let version = core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .stream_version;
        let mut replay =
            CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                turn_id: pending[0].turn_id.clone(),
                requested_at_sequence: pending[0].requested_at_sequence,
                answer: None,
                task_id: task_id.clone(),
                approval_id: "perm-1".into(),
                option_id: "allow".into(),
            }));
        replay.expected_stream_version = Some(version);
        assert!(matches!(
            core.execute(replay).await,
            Err(agent_core::CoreError::Conflict(_))
        ));

        let events = core.sync_events(0, 10_000).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "approval.resolved")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn attachments_reach_driver_and_survive_history_rebuild_without_prompt_serialization() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver = Arc::new(FakeDriver::with_executor(executor("scripted")));
        fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
        let task_id = scripted_task(&core).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file with spaces.txt");
        std::fs::write(&path, "fixture").unwrap();
        let attachments = vec![agent_domain::PromptAttachment {
            integrity: None,
            name: "file with spaces.txt".into(),
            path: path.to_string_lossy().into(),
            mime_type: "text/plain".into(),
        }];
        // Attachment-only messages are valid too.
        let mut envelope = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            attachments: attachments.clone(),
            permission_mode: None,
            thinking: None,
            model: None,
            task_id: task_id.clone(),
            prompt: String::new(),
            executor_override: None,
        }));
        envelope.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        core.execute(envelope).await.unwrap();
        wait_until_idle(&core).await;
        let requests = driver.received_requests().await;
        assert_eq!(requests[0].attachments, attachments);
        assert!(requests[0].prompt.is_empty());
        let before = core.timeline(&task_id).await.unwrap();
        let user = before
            .iter()
            .find(|item| item.kind == "user_message")
            .unwrap();
        assert!(user.text.is_empty());
        assert_eq!(user.attachments, attachments);
        let history = store
            .history_items(&task_id, 0, u64::try_from(i64::MAX).unwrap(), "", 100)
            .await
            .unwrap();
        assert_eq!(
            history
                .iter()
                .find(|item| item.kind == "user_message")
                .unwrap()
                .attachments,
            attachments
        );
        store.rebuild_projections().unwrap();
        assert_eq!(core.timeline(&task_id).await.unwrap(), before);
    }

    async fn scripted_task(core: &Arc<Core>) -> TaskId {
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Scripted".into(),
                path: "/tmp/scripted".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Scripted driver".into(),
                preferred_executor: executor("scripted"),
            },
        )))
        .await
        .unwrap();
        task_id
    }

    async fn wait_until_idle(core: &Arc<Core>) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !core.bootstrap().await.unwrap().active_runs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn captures_an_immutable_diff_artifact_and_reuses_its_reference_for_context() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let workspace = Arc::new(RecordingWorkspaceContext::default());
        let core = Arc::new(Core::with_workspace_context(store.clone(), workspace));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Diff artifact".into(),
                path: "/tmp/diff-artifact".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Capture exact patch".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();

        let task = core.bootstrap().await.unwrap().tasks.remove(0);
        let mut forbidden =
            CommandEnvelope::local(CoreCommand::CaptureWorkspaceDiff(CaptureWorkspaceDiff {
                task_id: task_id.clone(),
            }));
        forbidden.actor = ActorRef::SystemRecovery;
        forbidden.expected_stream_version = Some(task.stream_version);
        assert!(matches!(
            core.execute(forbidden).await,
            Err(agent_core::CoreError::InvalidArgument(_))
        ));
        let mut command =
            CommandEnvelope::local(CoreCommand::CaptureWorkspaceDiff(CaptureWorkspaceDiff {
                task_id: task_id.clone(),
            }));
        command.expected_stream_version = Some(task.stream_version);
        core.execute(command).await.unwrap();

        let events = core.sync_events(0, 100).await.unwrap();
        let event = events
            .iter()
            .find(|event| event.event_type == "workspace.diff_captured")
            .unwrap();
        let artifact_id =
            ArtifactId::parse(event.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
        let artifact = store.artifact(&artifact_id).await.unwrap().unwrap();
        assert_eq!(
            artifact.metadata.media_type,
            "application/vnd.dogcodes.git-diff.v1.patch"
        );
        assert_eq!(artifact.content, b"diff --git a/src/lib.rs b/src/lib.rs\n");
        assert_eq!(artifact.metadata.redaction_ruleset_version, 0);
        assert_eq!(
            store
                .workspace_diff_artifact_ref(&task_id, "snapshot-stable", 0, event.sequence)
                .await
                .unwrap()
                .as_deref(),
            Some(format!("artifact:{artifact_id}").as_str())
        );

        let snapshot = core.bootstrap().await.unwrap();
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("fake"),
            target_executor: executor("fake-b"),
            after_sequence: 0,
            through_sequence: snapshot.last_sequence,
        };
        let preview = core.build_context_preview(request.clone()).await.unwrap();
        assert_eq!(
            preview.package.change_summary.artifact_ref.as_deref(),
            Some(format!("artifact:{artifact_id}").as_str())
        );
        assert!(
            core.timeline(&task_id)
                .await
                .unwrap()
                .iter()
                .any(|item| item.kind == "diff")
        );

        // A later artifact for identical workspace content must not enter an old range.
        let mut later =
            CommandEnvelope::local(CoreCommand::CaptureWorkspaceDiff(CaptureWorkspaceDiff {
                task_id: task_id.clone(),
            }));
        later.expected_stream_version = Some(snapshot.tasks[0].stream_version);
        core.execute(later).await.unwrap();
        assert_eq!(
            core.build_context_preview(request.clone()).await.unwrap(),
            preview
        );
        assert!(
            store
                .workspace_diff_artifact_ref(
                    &task_id,
                    "snapshot-stable",
                    event.sequence,
                    event.sequence,
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .workspace_diff_artifact_ref(&TaskId::new(), "snapshot-stable", 0, event.sequence,)
                .await
                .unwrap()
                .is_none()
        );

        store.rebuild_projections().unwrap();
        assert_eq!(core.build_context_preview(request).await.unwrap(), preview);
        assert_eq!(
            store
                .workspace_diff_artifact_ref(&task_id, "snapshot-stable", 0, event.sequence)
                .await
                .unwrap()
                .as_deref(),
            Some(format!("artifact:{artifact_id}").as_str())
        );
    }

    #[tokio::test]
    async fn captures_bounded_workspace_snapshots_at_turn_boundaries_and_audits_failures() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let workspace = Arc::new(RecordingWorkspaceContext::default());
        let core = Arc::new(Core::with_workspace_context(
            store.clone(),
            workspace.clone(),
        ));
        let driver = Arc::new(FakeDriver::with_executor(executor("fake")));
        fixture_installations::register(&core, store.as_ref(), driver.clone()).await;

        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Workspace snapshots".into(),
                path: "/tmp/workspace-snapshots".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Capture turn boundaries".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();

        submit_turn(&core, &task_id, "capture a successful turn").await;
        wait_for_resumable_session(&core, &task_id, "fake").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let events = core.sync_events(0, 10_000).await.unwrap();
                if events
                    .iter()
                    .filter(|event| event.event_type == "workspace.snapshot_captured")
                    .count()
                    == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let events = core.sync_events(0, 10_000).await.unwrap();
        let captured = events
            .iter()
            .filter(|event| event.event_type == "workspace.snapshot_captured")
            .collect::<Vec<_>>();
        assert_eq!(captured[0].payload["phase"], "turn_start");
        assert_eq!(captured[1].payload["phase"], "turn_end");
        assert_eq!(captured[0].payload["git"]["statusFormat"], "porcelain_v2");
        assert_eq!(captured[0].payload["git"]["branch"], "main");
        let turn_started = events
            .iter()
            .find(|event| event.event_type == "turn.started")
            .unwrap();
        let turn_completed = events
            .iter()
            .find(|event| event.event_type == "turn.completed")
            .unwrap();
        assert!(captured[0].sequence < turn_started.sequence);
        assert!(captured[1].sequence < turn_completed.sequence);
        let changes = events
            .iter()
            .find(|e| e.event_type == "workspace.turn_changes_captured")
            .unwrap();
        assert!(changes.sequence < turn_completed.sequence);
        let artifact_id =
            ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
        let stored = store.artifact(&artifact_id).await.unwrap().unwrap();
        let retrieved = core
            .read_artifact(
                &ActorRef::LocalUser,
                &agent_protocol::ArtifactReadRequest {
                    task_id: task_id.clone(),
                    artifact_id: artifact_id.clone(),
                    expected_payload_sha256: stored.metadata.payload_sha256.clone(),
                    offset: 0,
                    limit: 32768,
                },
            )
            .await
            .expect("turn change artifacts must be retrievable by the conversation UI");
        assert_eq!(retrieved.bytes, stored.content);
        assert!(
            String::from_utf8(stored.content.clone())
                .unwrap()
                .contains("supersecret")
        );
        let head = core.bootstrap().await.unwrap().last_sequence;
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("fake"),
            target_executor: executor("fake-peer"),
            after_sequence: 0,
            through_sequence: head,
        };
        let context = core.build_context_preview(request).await.unwrap();
        assert!(context.package.conversation_text.contains("src/lib.rs"));
        assert!(context.package.conversation_text.contains("+new"));
        assert!(!context.package.conversation_text.contains("supersecret"));
        store.rebuild_projections().unwrap();
        assert_eq!(
            store.artifact(&artifact_id).await.unwrap().unwrap().content,
            stored.content
        );
        let projected = core.workspace_snapshots(&task_id, 10).await.unwrap();
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].phase, WorkspaceSnapshotPhase::TurnEnd);
        assert_eq!(projected[1].phase, WorkspaceSnapshotPhase::TurnStart);
        assert_eq!(projected[0].relevant_file_count, 1);
        assert!(projected[0].diagnostic.is_none());
        assert!(matches!(
            core.workspace_snapshots(&task_id, 0).await,
            Err(agent_core::CoreError::InvalidArgument(_))
        ));

        workspace.fail.store(true, Ordering::Release);
        submit_turn(&core, &task_id, "continue despite snapshot failure").await;
        wait_for_resumable_session(&core, &task_id, "fake").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let events = core.sync_events(0, 10_000).await.unwrap();
                if events
                    .iter()
                    .filter(|event| event.event_type == "workspace.snapshot_failed")
                    .count()
                    == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let events = core.sync_events(0, 10_000).await.unwrap();
        let failures = events
            .iter()
            .filter(|event| event.event_type == "workspace.snapshot_failed")
            .collect::<Vec<_>>();
        assert_eq!(failures[0].payload["phase"], "turn_start");
        assert_eq!(failures[1].payload["phase"], "turn_end");
        assert!(
            failures
                .iter()
                .all(|event| event.payload["failureKind"] == "not_repository")
        );
        assert_eq!(driver.received_requests().await.len(), 2);
        let projected_before_rebuild = core.workspace_snapshots(&task_id, 10).await.unwrap();
        assert_eq!(projected_before_rebuild.len(), 4);
        assert_eq!(
            projected_before_rebuild[0].diagnostic,
            Some(WorkspaceDiagnosticKind::NotRepository)
        );
        assert!(projected_before_rebuild[0].git.is_none());

        let before_rebuild = core.bootstrap().await.unwrap();
        store.rebuild_projections().unwrap();
        let after_rebuild = core.bootstrap().await.unwrap();
        assert_eq!(after_rebuild.projects, before_rebuild.projects);
        assert_eq!(after_rebuild.tasks, before_rebuild.tasks);
        assert_eq!(after_rebuild.installations, before_rebuild.installations);
        assert_eq!(after_rebuild.active_runs, before_rebuild.active_runs);
        assert_eq!(after_rebuild.session_maps, before_rebuild.session_maps);
        assert_eq!(after_rebuild.last_sequence, before_rebuild.last_sequence);
        assert_eq!(
            after_rebuild.companion.workspaces,
            before_rebuild.companion.workspaces
        );
        assert_eq!(
            after_rebuild.companion.recent_completion,
            before_rebuild.companion.recent_completion
        );
        assert_eq!(
            core.workspace_snapshots(&task_id, 10).await.unwrap(),
            projected_before_rebuild
        );
    }

    #[tokio::test]
    async fn resume_failures_persist_stable_states_without_leaking_driver_messages() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver = Arc::new(FakeDriver::with_executor(executor("fake")));
        fixture_installations::register(&core, store.as_ref(), driver.clone()).await;

        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Resume failures".into(),
                path: "/tmp/resume-failures".into(),
            },
        )))
        .await
        .unwrap();

        let missing_task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: missing_task_id.clone(),
                project_id: project_id.clone(),
                title: "Missing session".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &missing_task_id, "create the first session").await;
        wait_for_resumable_session(&core, &missing_task_id, "fake").await;

        let private_missing = "native-session-secret-missing";
        driver.fail_next_resume(DriverError::SessionMissing(private_missing.into()));
        submit_turn(&core, &missing_task_id, "resume the missing session").await;
        wait_for_session_status(&core, &missing_task_id, SessionMapStatus::Missing).await;

        let incompatible_task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: incompatible_task_id.clone(),
                project_id,
                title: "Incompatible session".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &incompatible_task_id, "create another session").await;
        wait_for_resumable_session(&core, &incompatible_task_id, "fake").await;

        let private_incompatible = "native-version-secret-incompatible";
        driver.fail_next_resume(DriverError::SessionIncompatible(
            private_incompatible.into(),
        ));
        submit_turn(
            &core,
            &incompatible_task_id,
            "resume the incompatible session",
        )
        .await;
        wait_for_session_status(&core, &incompatible_task_id, SessionMapStatus::Incompatible).await;

        let events = core.sync_events(0, 10_000).await.unwrap();
        let missing = events
            .iter()
            .find(|event| {
                event.stream.stream_id == missing_task_id.as_str()
                    && event.event_type == "session.missing"
            })
            .unwrap();
        assert_eq!(missing.payload["failureKind"], "target_session_missing");
        let incompatible = events
            .iter()
            .find(|event| {
                event.stream.stream_id == incompatible_task_id.as_str()
                    && event.event_type == "session.incompatible"
            })
            .unwrap();
        assert_eq!(
            incompatible.payload["failureKind"],
            "target_session_incompatible"
        );
        let persisted = serde_json::to_string(&events).unwrap();
        assert!(!persisted.contains(private_missing));
        assert!(!persisted.contains(private_incompatible));
        assert!(
            events
                .iter()
                .filter(|event| event.event_type == "turn.failed")
                .all(|event| event.payload["message"] == "turn execution failed"
                    && event.payload.get("failureKind").is_some())
        );

        let before_rebuild = core.bootstrap().await.unwrap().session_maps;
        store.rebuild_projections().unwrap();
        assert_eq!(core.bootstrap().await.unwrap().session_maps, before_rebuild);
        wait_until_idle(&core).await;
        let version = core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.task_id == missing_task_id)
            .unwrap()
            .stream_version;
        let before_requests = driver.received_requests().await.len();
        assert!(
            core.prepare_session_rebuild(&ActorRef::SystemRecovery, &missing_task_id, version)
                .await
                .is_err()
        );
        core.prepare_session_rebuild(&ActorRef::LocalUser, &missing_task_id, version)
            .await
            .unwrap();
        assert_eq!(
            driver.received_requests().await.len(),
            before_requests,
            "preparation must not replay a prompt"
        );
        assert!(
            core.prepare_session_rebuild(&ActorRef::LocalUser, &missing_task_id, version)
                .await
                .is_err()
        );
        store.rebuild_projections().unwrap();
        assert!(
            core.bootstrap()
                .await
                .unwrap()
                .session_maps
                .iter()
                .any(|m| m.task_id == missing_task_id
                    && m.status == SessionMapStatus::RebuildPending)
        );
        submit_turn(
            &core,
            &missing_task_id,
            "new request after explicit recovery",
        )
        .await;
        wait_for_resumable_session(&core, &missing_task_id, "fake").await;
        let requests = driver.received_requests().await;
        let last = requests.last().unwrap();
        assert_eq!(last.prompt, "new request after explicit recovery");
        assert_eq!(last.context.len(), 1);
        let history = String::from_utf8(last.context[0].content.clone()).unwrap();
        assert!(history.contains("create the first session"));
        assert!(history.contains("do not replay old prompts"));
        assert!(!history.contains("new request after explicit recovery"));
        let artifacts = core
            .task_artifacts(&ActorRef::LocalUser, &missing_task_id, None, 100)
            .await
            .unwrap();
        assert!(
            artifacts
                .items
                .iter()
                .any(|a| a.source_kind == "session.rebuild_context_prepared")
        );
    }

    #[test]
    fn interrupted_runtime_recovers_durable_checkpoint_once() {
        use agent_workspace::GitWorkspaceContextProvider;
        let directory = tempfile::tempdir().unwrap();
        let child_root = std::env::var_os("DOGCODES_CRASH_TEST_ROOT");
        let root = child_root
            .as_ref()
            .map_or_else(|| directory.path().to_path_buf(), std::path::PathBuf::from);
        let workspace = root.join("project");
        let db = root.join("crash.db");
        if child_root.is_some() {
            std::fs::create_dir(&workspace).unwrap();
            std::fs::write(workspace.join("file.txt"), "before\n").unwrap();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let store = Arc::new(SqliteStore::open(&db).unwrap());
                let core = Arc::new(Core::with_workspace_context(
                    store.clone(),
                    Arc::new(GitWorkspaceContextProvider::discover().unwrap()),
                ));
                fixture_installations::register(
                    &core,
                    store.as_ref(),
                    Arc::new(ApprovalDriver::new("approvals")),
                )
                .await;
                let project_id = ProjectId::new();
                let task_id = TaskId::new();
                core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                    RegisterProject {
                        additional_directories: vec![],
                        project_id: project_id.clone(),
                        display_name: "Recovery".into(),
                        path: workspace.to_string_lossy().into_owned(),
                    },
                )))
                .await
                .unwrap();
                core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
                    CreateTask {
                        task_id: task_id.clone(),
                        project_id,
                        title: "Recovery".into(),
                        preferred_executor: executor("approvals"),
                    },
                )))
                .await
                .unwrap();
                submit_turn(&core, &task_id, "wait until crash").await;
                tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    loop {
                        if core
                            .sync_events(0, 1000)
                            .await
                            .unwrap()
                            .iter()
                            .any(|e| e.event_type == "approval.requested")
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            });
            // Abrupt process exit: no Rust destructors, runtime shutdown, or final capture.
            std::process::exit(73);
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::interrupted_runtime_recovers_durable_checkpoint_once",
                "--nocapture",
            ])
            .env("DOGCODES_CRASH_TEST_ROOT", &root)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        std::fs::write(workspace.join("file.txt"), "after crash\n").unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let store = Arc::new(SqliteStore::open(&db).unwrap());
            let core = Core::with_workspace_context(
                store.clone(),
                Arc::new(GitWorkspaceContextProvider::discover().unwrap()),
            );
            core.recover_interrupted_turns().await.unwrap();
            assert!(
                core.bootstrap().await.unwrap().tasks[0]
                    .active_primary_run
                    .is_none()
            );
            assert_eq!(
                core.bootstrap().await.unwrap().companion.workspaces[0]
                    .last_terminal
                    .as_ref()
                    .unwrap()
                    .state,
                RunState::Disconnected
            );
            let events = core.sync_events(0, 1000).await.unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == "approval.expired")
                    .count(),
                1
            );
            let task_id = core.bootstrap().await.unwrap().tasks[0].task_id.clone();
            assert!(
                core.pending_approvals(&task_id, 10)
                    .await
                    .unwrap()
                    .is_empty()
            );
            // Even a fresh snapshot cannot revive an approval from the dead host.
            let task = core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .into_iter()
                .find(|t| t.task_id == task_id)
                .unwrap();
            let source = events
                .iter()
                .find(|event| event.event_type == "approval.requested")
                .unwrap();
            let mut late =
                CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                    turn_id: agent_domain::TurnId::parse(
                        source.payload["turnId"].as_str().unwrap(),
                    )
                    .unwrap(),
                    requested_at_sequence: source.sequence,
                    answer: None,
                    task_id: task_id.clone(),
                    approval_id: "perm-1".into(),
                    option_id: "allow".into(),
                }));
            late.expected_stream_version = Some(task.stream_version);
            let core = Arc::new(core);
            assert!(matches!(
                core.execute(late).await,
                Err(agent_core::CoreError::Conflict(_))
            ));
            assert_eq!(core.sync_events(0, 1000).await.unwrap().len(), events.len());
            let change = events
                .iter()
                .find(|e| e.event_type == "workspace.turn_changes_captured")
                .unwrap();
            assert_eq!(change.payload["attribution"], "workspace_recovery_interval");
            let id = ArtifactId::parse(change.payload["artifact"]["artifactId"].as_str().unwrap())
                .unwrap();
            let artifact = store.artifact(&id).await.unwrap().unwrap();
            let body: Value = serde_json::from_slice(&artifact.content).unwrap();
            assert!(
                body["patch"]
                    .as_str()
                    .unwrap()
                    .contains("-before\n+after crash")
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.event_type == "turn.connection_lost"
                        && e.payload["turnId"].is_string())
                    .count(),
                1
            );
            core.recover_interrupted_turns().await.unwrap();
            assert_eq!(core.sync_events(0, 1000).await.unwrap().len(), events.len());
            store.rebuild_projections().unwrap();
            assert!(
                core.bootstrap().await.unwrap().tasks[0]
                    .active_primary_run
                    .is_none()
            );
            assert_eq!(
                core.bootstrap().await.unwrap().companion.workspaces[0]
                    .last_terminal
                    .as_ref()
                    .unwrap()
                    .state,
                RunState::Disconnected
            );
            assert!(
                !events
                    .iter()
                    .any(|e| e.event_type == "turn.completed" || e.event_type == "turn.failed")
            );
            assert!(
                core.timeline(&task_id)
                    .await
                    .unwrap()
                    .iter()
                    .any(|i| i.kind == "connection.interrupted")
            );
            let driver = Arc::new(FakeDriver::with_executor(executor("approvals")));
            fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
            let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
            let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
                review_fix: None,
                attachments: vec![],
                task_id: task_id.clone(),
                prompt: "继续".into(),
                executor_override: None,
                model: None,
                thinking: None,
                permission_mode: None,
            }));
            submit.expected_stream_version = Some(version);
            core.execute(submit).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    if !driver.received_requests().await.is_empty()
                        && core.bootstrap().await.unwrap().tasks[0]
                            .active_primary_run
                            .is_none()
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let requests = driver.received_requests().await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].prompt, "继续");
            assert!(
                String::from_utf8_lossy(&requests[0].context[0].content)
                    .contains("Before retrying any side effects")
            );
            let prior = events
                .iter()
                .find(|e| e.event_type == "turn.connection_lost")
                .unwrap();
            let stale = agent_protocol::NewEvent::hub(
                prior.stream.clone(),
                "turn.connection_lost",
                prior.payload.clone(),
            );
            store
                .append(AppendRequest {
                    request_fingerprint: None,
                    command_id: agent_domain::CommandId::new(),
                    idempotency_key: stale.event_id.to_string(),
                    expected_stream_version: None,
                    artifacts: vec![],
                    events: vec![stale],
                })
                .await
                .unwrap();
            // A late event from the lost turn cannot override the replacement turn's result.
            assert_eq!(
                core.bootstrap().await.unwrap().companion.workspaces[0]
                    .last_terminal
                    .as_ref()
                    .unwrap()
                    .state,
                RunState::Completed
            );
            store.rebuild_projections().unwrap();
            assert_eq!(
                core.bootstrap().await.unwrap().companion.workspaces[0]
                    .last_terminal
                    .as_ref()
                    .unwrap()
                    .state,
                RunState::Completed
            );
        });
    }

    #[test]
    fn interrupted_multi_root_runtime_recovers_durable_checkpoint_once() {
        use agent_workspace::GitWorkspaceContextProvider;
        let directory = tempfile::tempdir().unwrap();
        let child_root = std::env::var_os("DOGCODES_MULTI_CRASH_TEST_ROOT");
        let root = child_root
            .as_ref()
            .map_or_else(|| directory.path().to_path_buf(), std::path::PathBuf::from);
        let workspace = root.join("project");
        let db = root.join("crash.db");
        let secondary = root.join("secondary");
        if child_root.is_some() {
            std::fs::create_dir(&workspace).unwrap();
            std::fs::create_dir(&secondary).unwrap();
            std::fs::write(secondary.join("file.txt"), "secondary before\n").unwrap();
            std::fs::write(workspace.join("file.txt"), "before\n").unwrap();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let store = Arc::new(SqliteStore::open(&db).unwrap());
                let core = Arc::new(Core::with_workspace_context(
                    store.clone(),
                    Arc::new(GitWorkspaceContextProvider::discover().unwrap()),
                ));
                fixture_installations::register(
                    &core,
                    store.as_ref(),
                    Arc::new(InterruptibleDriver::new()),
                )
                .await;
                let project_id = ProjectId::new();
                let task_id = TaskId::new();
                core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
                    RegisterProject {
                        additional_directories: vec![secondary.to_string_lossy().into_owned()],
                        project_id: project_id.clone(),
                        display_name: "Recovery".into(),
                        path: workspace.to_string_lossy().into_owned(),
                    },
                )))
                .await
                .unwrap();
                core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
                    CreateTask {
                        task_id: task_id.clone(),
                        project_id,
                        title: "Recovery".into(),
                        preferred_executor: executor("interruptible"),
                    },
                )))
                .await
                .unwrap();
                submit_turn(&core, &task_id, "wait until crash").await;
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    loop {
                        if core
                            .sync_events(0, 1000)
                            .await
                            .unwrap()
                            .iter()
                            .any(|e| e.event_type == "workspace.turn_checkpoint_captured")
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            });
            // Abrupt process exit: no Rust destructors, runtime shutdown, or final capture.
            std::process::exit(73);
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::interrupted_multi_root_runtime_recovers_durable_checkpoint_once",
                "--nocapture",
            ])
            .env("DOGCODES_MULTI_CRASH_TEST_ROOT", &root)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        std::fs::write(workspace.join("file.txt"), "after crash\n").unwrap();
        std::fs::write(secondary.join("file.txt"), "secondary after crash\n").unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let store = Arc::new(SqliteStore::open(&db).unwrap());
            let core = Core::with_workspace_context(
                store.clone(),
                Arc::new(GitWorkspaceContextProvider::discover().unwrap()),
            );
            store.connection.lock().unwrap().execute("UPDATE tasks SET active_run_state='disconnected' WHERE active_run_id IS NOT NULL", []).unwrap();
            core.recover_interrupted_turns().await.unwrap();
            assert!(core.bootstrap().await.unwrap().tasks[0].active_primary_run.is_none());
            assert_eq!(core.bootstrap().await.unwrap().companion.workspaces[0].last_terminal.as_ref().unwrap().state, RunState::Disconnected);
            let events = core.sync_events(0, 1000).await.unwrap();
            let change = events
                .iter()
                .find(|e| e.event_type == "workspace.turn_changes_captured")
                .unwrap();
            assert_eq!(change.payload["attribution"], "workspace_recovery_interval");
            let id = ArtifactId::parse(change.payload["artifact"]["artifactId"].as_str().unwrap())
                .unwrap();
            let artifact = store.artifact(&id).await.unwrap().unwrap();
            let body: Value = serde_json::from_slice(&artifact.content).unwrap();
            assert!(
                body["roots"][0]["changes"]["patch"]
                    .as_str()
                    .unwrap()
                    .contains("-before\n+after crash")
            );
            assert_eq!(body["schemaVersion"], 2);
            assert_eq!(body["files"].as_array().unwrap().len(), 2);
            assert!(
                body["roots"][1]["changes"]["patch"]
                    .as_str()
                    .unwrap()
                    .contains("-secondary before\n+secondary after crash")
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.event_type == "turn.connection_lost"
                        && e.payload["turnId"].is_string())
                    .count(),
                1
            );
            core.recover_interrupted_turns().await.unwrap();
            assert_eq!(core.sync_events(0, 1000).await.unwrap().len(), events.len());
            store.rebuild_projections().unwrap();
            assert!(core.bootstrap().await.unwrap().tasks[0].active_primary_run.is_none());
            assert_eq!(core.bootstrap().await.unwrap().companion.workspaces[0].last_terminal.as_ref().unwrap().state, RunState::Disconnected);
        });
    }

    #[tokio::test]
    async fn agent_without_turn_started_continues_after_handoff_and_restart() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(FakeDriver::with_executor(executor("scripted"))),
        )
        .await;
        let mut cursor = ScriptedDriver::new(
            "cursor",
            vec![DriverEvent::TurnCompleted {
                summary: "done".into(),
            }],
        );
        cursor.executor.installation_id = Some(InstallationId::new().to_string());
        let target = cursor.executor.clone();
        fixture_installations::register(&core, store.as_ref(), Arc::new(cursor)).await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "first agent").await;
        wait_until_idle(&core).await;
        let snapshot = core.bootstrap().await.unwrap();
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("scripted"),
            target_executor: target.clone(),
            after_sequence: 0,
            through_sequence: snapshot.last_sequence,
        };
        let preview = core.build_context_preview(request.clone()).await.unwrap();
        let mut confirm =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request,
                expected_payload_sha256: preview.payload_sha256,
                edited_package: None,
            }));
        confirm.expected_stream_version = Some(snapshot.tasks[0].stream_version);
        core.execute(confirm).await.unwrap();
        submit_turn(&core, &task_id, "first Cursor turn").await;
        wait_until_idle(&core).await;
        assert_eq!(
            store.last_dispatched_executor(&task_id).await.unwrap(),
            Some(target.clone())
        );
        submit_turn(&core, &task_id, "continue Cursor").await;
        wait_until_idle(&core).await;
        let events = core.sync_events(0, 10000).await.unwrap();
        assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "driver.turn_started")
                .count(),
            1
        );

        store.rebuild_projections().unwrap();
        assert_eq!(
            store.last_dispatched_executor(&task_id).await.unwrap(),
            Some(target.clone())
        );
        let restarted = Arc::new(Core::new(store.clone()));
        let mut cursor = ScriptedDriver::new(
            "cursor",
            vec![DriverEvent::TurnCompleted {
                summary: "resumed".into(),
            }],
        );
        cursor.executor = target.clone();
        fixture_installations::register(&restarted, store.as_ref(), Arc::new(cursor)).await;
        fixture_installations::register(
            &restarted,
            store.as_ref(),
            Arc::new(FakeDriver::with_executor(executor("scripted"))),
        )
        .await;
        submit_turn(&restarted, &task_id, "continue after restart").await;
        wait_until_idle(&restarted).await;
        assert!(
            !restarted
                .sync_events(0, 10000)
                .await
                .unwrap()
                .iter()
                .any(|e| e.event_type == "turn.failed")
        );

        // A real switch still requires a handoff; a rejected turn cannot become the last executor.
        let snapshot = restarted.bootstrap().await.unwrap();
        let mut switch =
            CommandEnvelope::local(CoreCommand::SetPreferredExecutor(SetPreferredExecutor {
                task_id: task_id.clone(),
                executor: executor("scripted"),
            }));
        switch.expected_stream_version = Some(snapshot.tasks[0].stream_version);
        restarted.execute(switch).await.unwrap();
        submit_turn(&restarted, &task_id, "switch without confirmation").await;
        wait_until_idle(&restarted).await;
        assert_eq!(
            store.last_dispatched_executor(&task_id).await.unwrap(),
            Some(target)
        );
        let timeline = restarted.timeline(&task_id).await.unwrap();
        let failure = timeline.iter().find(|i| i.kind == "turn.failed").unwrap();
        assert!(failure.text.contains("上下文交接"));
        store.rebuild_projections().unwrap();
        assert_eq!(restarted.timeline(&task_id).await.unwrap(), timeline);
    }

    #[tokio::test]
    async fn accepted_prompt_without_native_events_records_executor() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        fixture_installations::register(
            &core,
            store.as_ref(),
            Arc::new(ScriptedDriver::new("scripted", vec![])),
        )
        .await;
        let task_id = scripted_task(&core).await;
        submit_turn(&core, &task_id, "empty stream").await;
        wait_until_idle(&core).await;
        assert_eq!(
            store.last_dispatched_executor(&task_id).await.unwrap(),
            Some(executor("scripted"))
        );
        assert!(
            core.timeline(&task_id)
                .await
                .unwrap()
                .iter()
                .any(|i| i.kind == "turn.failed")
        );
    }

    #[tokio::test]
    async fn fake_drivers_handoff_then_reconcile_before_the_user_prompt() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver_a = Arc::new(FakeDriver::with_executor(executor("fake-a")));
        let driver_b = Arc::new(FakeDriver::with_executor(executor("fake-b")));
        fixture_installations::register(&core, store.as_ref(), driver_a.clone()).await;
        fixture_installations::register(&core, store.as_ref(), driver_b.clone()).await;

        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "A-B-A".into(),
                path: "/tmp/a-b-a".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Verify reconciliation ordering".into(),
                preferred_executor: executor("fake-a"),
            },
        )))
        .await
        .unwrap();

        submit_turn(&core, &task_id, "first on A").await;
        let first_a_session = wait_for_resumable_session(&core, &task_id, "fake-a").await;

        let task = core.bootstrap().await.unwrap().tasks.remove(0);
        let mut bypass =
            CommandEnvelope::local(CoreCommand::SetPreferredExecutor(SetPreferredExecutor {
                task_id: task_id.clone(),
                executor: executor("fake-b"),
            }));
        bypass.expected_stream_version = Some(task.stream_version);
        core.execute(bypass).await.unwrap();
        submit_turn(&core, &task_id, "must be blocked without handoff").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !core.bootstrap().await.unwrap().active_runs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(driver_b.received_requests().await.is_empty());
        let task = core.bootstrap().await.unwrap().tasks.remove(0);
        let mut restore =
            CommandEnvelope::local(CoreCommand::SetPreferredExecutor(SetPreferredExecutor {
                task_id: task_id.clone(),
                executor: executor("fake-a"),
            }));
        restore.expected_stream_version = Some(task.stream_version);
        core.execute(restore).await.unwrap();

        let snapshot = core.bootstrap().await.unwrap();
        let task = snapshot.tasks[0].clone();
        let handoff_request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: executor("fake-a"),
            target_executor: executor("fake-b"),
            after_sequence: 0,
            through_sequence: snapshot.last_sequence,
        };
        let handoff = core
            .build_context_preview(handoff_request.clone())
            .await
            .unwrap();
        let mut confirm_handoff =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request: handoff_request,
                expected_payload_sha256: handoff.payload_sha256.clone(),
                edited_package: None,
            }));
        confirm_handoff.expected_stream_version = Some(task.stream_version);
        core.execute(confirm_handoff).await.unwrap();
        driver_b.fail_next_prompt();
        submit_turn(&core, &task_id, "must not reach B").await;
        wait_for_resumable_session(&core, &task_id, "fake-b").await;
        assert!(driver_b.received_requests().await.is_empty());
        assert_eq!(
            store.last_dispatched_executor(&task_id).await.unwrap(),
            Some(executor("fake-a"))
        );
        assert!(
            store
                .pending_context_transfer(&task_id, &executor("fake-b"))
                .await
                .unwrap()
                .is_some()
        );
        let failed_events = core.sync_events(0, 10_000).await.unwrap();
        assert!(
            failed_events
                .iter()
                .any(|event| event.event_type == "handoff.failed")
        );
        assert!(
            failed_events
                .iter()
                .all(|event| event.event_type != "handoff.sent")
        );

        submit_turn(&core, &task_id, "work on B").await;
        wait_for_resumable_session(&core, &task_id, "fake-b").await;

        let b_requests = driver_b.received_requests().await;
        assert_eq!(b_requests.len(), 1);
        assert_eq!(b_requests[0].prompt, "work on B");
        assert_eq!(b_requests[0].context.len(), 1);
        assert_eq!(b_requests[0].context[0].kind, ContextDeliveryKind::Handoff);
        let delivered: serde_json::Value =
            serde_json::from_slice(&b_requests[0].context[0].content).unwrap();
        assert!(
            delivered["conversationText"]
                .as_str()
                .unwrap()
                .contains("first on A")
        );
        assert!(
            delivered["conversationText"]
                .as_str()
                .unwrap()
                .contains("[Assistant: fake-a]")
        );
        assert!(
            !delivered["conversationText"]
                .as_str()
                .unwrap()
                .contains("work on B")
        );
        assert!(delivered.get("deliveryFacts").is_none());
        assert!(delivered.get("provenance").is_none());
        assert_eq!(
            b_requests[0].context[0].payload_sha256,
            hex::encode(Sha256::digest(&b_requests[0].context[0].content))
        );
        let audit = store
            .artifact(&b_requests[0].context[0].artifact_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit.metadata.payload_sha256, handoff.payload_sha256);
        let audit_package: agent_protocol::ContextPackageV1 =
            serde_json::from_slice(&audit.content).unwrap();
        assert_eq!(audit_package, handoff.package);
        let receipts = core.sync_events(0, 10_000).await.unwrap();
        let delivered_receipt = receipts
            .iter()
            .rev()
            .find(|event| event.event_type == "history.facts_delivered")
            .unwrap();
        assert_eq!(
            delivered_receipt.payload["facts"],
            serde_json::to_value(&handoff.package.delivery_facts).unwrap()
        );

        let snapshot = core.bootstrap().await.unwrap();
        let task = snapshot.tasks[0].clone();
        let reconciliation_request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Reconciliation,
            strength: ContextStrength::Standard,
            source_executor: executor("fake-b"),
            target_executor: executor("fake-a"),
            after_sequence: first_a_session.last_seen_sequence,
            through_sequence: snapshot.last_sequence,
        };
        let reconciliation = core
            .build_context_preview(reconciliation_request.clone())
            .await
            .unwrap();
        // Execution checkpoints do not suppress previously undelivered historical metadata.
        assert!(
            reconciliation
                .package
                .provenance
                .source_sequences
                .iter()
                .all(|sequence| *sequence <= snapshot.last_sequence)
        );
        assert!(
            !reconciliation
                .package
                .delivery_facts
                .iter()
                .any(|fact| fact.field == "prompt"
                    && fact.sequence <= first_a_session.last_seen_sequence)
        );
        let mut confirm_reconciliation =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request: reconciliation_request,
                expected_payload_sha256: reconciliation.payload_sha256.clone(),
                edited_package: None,
            }));
        confirm_reconciliation.expected_stream_version = Some(task.stream_version);
        core.execute(confirm_reconciliation).await.unwrap();
        submit_turn(&core, &task_id, "continue on A").await;
        wait_for_resumable_session(&core, &task_id, "fake-a").await;

        let a_requests = driver_a.received_requests().await;
        assert_eq!(a_requests.len(), 2);
        assert!(a_requests[0].context.is_empty());
        assert_eq!(a_requests[1].prompt, "continue on A");
        assert_eq!(a_requests[1].context.len(), 1);
        let delivered: serde_json::Value =
            serde_json::from_slice(&a_requests[1].context[0].content).unwrap();
        assert!(
            delivered["conversationText"]
                .as_str()
                .unwrap()
                .contains("work on B")
        );
        assert!(
            !delivered["conversationText"]
                .as_str()
                .unwrap()
                .contains("first on A")
        );
        assert_eq!(
            a_requests[1].context[0].kind,
            ContextDeliveryKind::Reconciliation
        );
        assert!(delivered.get("deliveryFacts").is_none());
        assert_eq!(
            a_requests[1].context[0].payload_sha256,
            hex::encode(Sha256::digest(&a_requests[1].context[0].content))
        );
        let resumed_sessions = driver_a.resumed_sessions().await;
        assert_eq!(
            resumed_sessions.as_slice(),
            std::slice::from_ref(&first_a_session.native_session_id)
        );

        let events = core.sync_events(0, 10_000).await.unwrap();
        let reconciliation_sent = events
            .iter()
            .find(|event| event.event_type == "reconciliation.sent")
            .unwrap();
        let resumed = events
            .iter()
            .find(|event| {
                event.event_type == "session.resumed"
                    && event.payload["executor"]["providerId"] == "fake-a"
            })
            .unwrap();
        let driver_turn_started = events
            .iter()
            .find(|event| {
                event.sequence > reconciliation_sent.sequence
                    && event.event_type == "driver.turn_started"
                    && matches!(
                        &event.source,
                        EventSource::Driver { provider_id, .. } if provider_id == "fake-a"
                    )
            })
            .unwrap();
        assert!(resumed.sequence < reconciliation_sent.sequence);
        assert!(reconciliation_sent.sequence < driver_turn_started.sequence);
        assert_eq!(
            reconciliation_sent.payload["payloadSha256"],
            reconciliation.payload_sha256
        );
        assert!(
            store
                .pending_context_transfer(&task_id, &executor("fake-a"))
                .await
                .unwrap()
                .is_none()
        );
        let before_rebuild = core.bootstrap().await.unwrap().session_maps;
        let timeline_before_rebuild = core.timeline(&task_id).await.unwrap();
        store.rebuild_projections().unwrap();
        assert_eq!(core.bootstrap().await.unwrap().session_maps, before_rebuild);
        assert_eq!(
            core.timeline(&task_id).await.unwrap(),
            timeline_before_rebuild
        );
        assert!(
            store
                .pending_context_transfer(&task_id, &executor("fake-a"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn explicit_reconciliation_opt_out_is_retried_and_applied_before_prompt() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let driver_a = Arc::new(FakeDriver::with_executor(executor("fake-a")));
        let driver_b = Arc::new(FakeDriver::with_executor(executor("fake-b")));
        fixture_installations::register(&core, store.as_ref(), driver_a.clone()).await;
        fixture_installations::register(&core, store.as_ref(), driver_b).await;
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Opt-out".into(),
                path: "/tmp/opt-out".into(),
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Audit reconciliation opt-out".into(),
                preferred_executor: executor("fake-a"),
            },
        )))
        .await
        .unwrap();
        submit_turn(&core, &task_id, "first on A").await;
        let a_session = wait_for_resumable_session(&core, &task_id, "fake-a").await;

        let snapshot = core.bootstrap().await.unwrap();
        let task = snapshot.tasks[0].clone();
        let handoff_request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Compact,
            source_executor: executor("fake-a"),
            target_executor: executor("fake-b"),
            after_sequence: 0,
            through_sequence: snapshot.last_sequence,
        };
        let handoff = core
            .build_context_preview(handoff_request.clone())
            .await
            .unwrap();
        let mut confirm =
            CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
                request: handoff_request,
                expected_payload_sha256: handoff.payload_sha256,
                edited_package: None,
            }));
        confirm.expected_stream_version = Some(task.stream_version);
        core.execute(confirm).await.unwrap();
        submit_turn(&core, &task_id, "work on B").await;
        wait_for_resumable_session(&core, &task_id, "fake-b").await;

        let snapshot = core.bootstrap().await.unwrap();
        let task = snapshot.tasks[0].clone();
        let request = ContextPreviewRequest {
            task_id: task_id.clone(),
            kind: ContextPackageKind::Reconciliation,
            strength: ContextStrength::Standard,
            source_executor: executor("fake-b"),
            target_executor: executor("fake-a"),
            after_sequence: a_session.last_seen_sequence,
            through_sequence: snapshot.last_sequence,
        };
        let preview = core.build_context_preview(request.clone()).await.unwrap();
        let mut opt_out =
            CommandEnvelope::local(CoreCommand::OptOutReconciliation(OptOutReconciliation {
                request,
                expected_payload_sha256: preview.payload_sha256,
            }));
        opt_out.expected_stream_version = Some(task.stream_version);
        let ack = core.execute(opt_out.clone()).await.unwrap();
        assert_eq!(core.execute(opt_out).await.unwrap(), ack);

        driver_a.fail_next_prompt();
        submit_turn(&core, &task_id, "rejected once").await;
        wait_for_resumable_session(&core, &task_id, "fake-a").await;
        assert_eq!(driver_a.received_requests().await.len(), 1);
        assert!(
            store
                .pending_reconciliation_opt_out(&task_id, &executor("fake-a"))
                .await
                .unwrap()
                .is_some()
        );

        submit_turn(&core, &task_id, "continue without reconciliation").await;
        wait_for_resumable_session(&core, &task_id, "fake-a").await;
        let requests = driver_a.received_requests().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].prompt, "continue without reconciliation");
        assert!(requests[1].context.is_empty());
        assert!(
            store
                .pending_reconciliation_opt_out(&task_id, &executor("fake-a"))
                .await
                .unwrap()
                .is_none()
        );

        let events = core.sync_events(0, 10_000).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "reconciliation.opt_out_failed")
        );
        let applied = events
            .iter()
            .find(|event| event.event_type == "reconciliation.opt_out_applied")
            .unwrap();
        let resumed = events
            .iter()
            .rfind(|event| {
                event.event_type == "session.resumed"
                    && event.payload["executor"]["providerId"] == "fake-a"
                    && event.sequence < applied.sequence
            })
            .unwrap();
        let turn_started = events
            .iter()
            .find(|event| {
                event.event_type == "driver.turn_started" && event.sequence > applied.sequence
            })
            .unwrap();
        assert!(resumed.sequence < applied.sequence);
        assert!(applied.sequence < turn_started.sequence);

        store.rebuild_projections().unwrap();
        assert!(
            store
                .pending_reconciliation_opt_out(&task_id, &executor("fake-a"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn subscription_replays_then_receives_live_events_without_a_gap() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                additional_directories: vec![],
                project_id: project_id.clone(),
                display_name: "Demo".into(),
                path: "/tmp/demo".into(),
            },
        )))
        .await
        .unwrap();

        let mut subscription = core.subscribe(0).await.unwrap();
        assert_eq!(subscription.replay.len(), 1);
        assert_eq!(subscription.replay[0].sequence, 1);

        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: TaskId::new(),
                project_id,
                title: "Live task".into(),
                preferred_executor: executor("fake"),
            },
        )))
        .await
        .unwrap();
        let live = subscription.live.recv().await.unwrap();
        assert_eq!(live.sequence, 2);
        assert_eq!(live.event_type, "task.created");
    }

    #[tokio::test]
    async fn installation_probe_updates_then_removal_rebuilds_without_a_ghost() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let executable = "/opt/codex/bin/codex";

        core.execute(installation_probe(
            executable,
            "first-fingerprint",
            InstallationStatus::Installed,
            "1.2.3",
        ))
        .await
        .unwrap();
        let first = core.bootstrap().await.unwrap().installations.remove(0);

        assert!(matches!(
            core.execute(missing_installation(
                first.installation_id.clone(),
                "/opt/codex/bin/not-the-current-installation",
            ))
            .await,
            Err(agent_core::CoreError::InvalidArgument(_))
        ));
        assert_eq!(core.bootstrap().await.unwrap().last_sequence, 1);

        core.execute(installation_probe(
            executable,
            "second-fingerprint",
            InstallationStatus::UnsupportedVersion,
            "0.9.0",
        ))
        .await
        .unwrap();
        let snapshot = core.bootstrap().await.unwrap();
        assert_eq!(snapshot.installations.len(), 1);
        assert_eq!(
            snapshot.installations[0].installation_id,
            first.installation_id
        );
        assert_eq!(
            snapshot.installations[0].status,
            InstallationStatus::UnsupportedVersion
        );
        assert_eq!(snapshot.installations[0].fingerprint, "second-fingerprint");

        core.execute(missing_installation(
            first.installation_id.clone(),
            executable,
        ))
        .await
        .unwrap();
        let removed = core.bootstrap().await.unwrap();
        assert!(removed.installations.is_empty());
        assert_eq!(removed.last_sequence, 3);

        store.rebuild_projections().unwrap();
        let rebuilt = core.bootstrap().await.unwrap();
        assert_eq!(rebuilt.installations, removed.installations);
        assert_eq!(rebuilt.last_sequence, 3);
    }
}

#[cfg(test)]
mod streaming_read_tests {
    use super::*;
    use agent_core::Core;
    use agent_domain::{CommandId, ProjectId};
    use agent_protocol::{CommandEnvelope, CoreCommand, NewEvent};
    use std::sync::Arc;

    async fn fixture() -> (Arc<SqliteStore>, Arc<Core>, TaskId) {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "Stream".into(),
                path: "/tmp/stream".into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        let task_id = TaskId::new();
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "Stream".into(),
                preferred_executor: ExecutorRef {
                    provider_id: ProviderId::parse("fake").unwrap(),
                    installation_id: None,
                },
            },
        )))
        .await
        .unwrap();
        (store, core, task_id)
    }

    #[tokio::test]
    async fn task_queries_do_not_load_unrelated_tasks() {
        let (store, core, task) = fixture().await;
        assert_eq!(
            store.task_summary(&task).await.unwrap(),
            Some(core.bootstrap().await.unwrap().tasks[0].clone())
        );
        assert!(store.task_summary(&TaskId::new()).await.unwrap().is_none());
        {
            let connection = store.connection.lock().unwrap();
            connection.execute(
                "INSERT INTO tasks(task_id,project_id,title,preferred_executor_json,stream_version) SELECT ?1,project_id,'unrelated','invalid-json',1 FROM tasks WHERE task_id=?2",
                params![TaskId::new().to_string(), task.as_str()],
            ).unwrap();
        }
        assert!(core.bootstrap().await.is_err());
        let state = store.task_execution_state(&task).await.unwrap().unwrap();
        assert_eq!(state.task.task_id, task);
        assert_eq!(state.workspace.path, "/tmp/stream");
        assert!(state.workspace.additional_directories.is_empty());
        assert!(state.session_maps.is_empty());
        assert_eq!(store.event_head().await.unwrap(), 2);
        assert!(
            store
                .task_execution_state(&TaskId::new())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            core.timeline_snapshot(&task)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(core.pending_approvals(&task, 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn scoped_state_honors_task_deletion() {
        let (store, core, task) = fixture().await;
        let version = store
            .task_summary(&task)
            .await
            .unwrap()
            .unwrap()
            .stream_version;
        let mut envelope =
            CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
                task_id: task.clone(),
            }));
        envelope.expected_stream_version = Some(version);
        core.execute(envelope).await.unwrap();
        assert!(store.task_execution_state(&task).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn filtered_events_skip_noise_and_respect_cursor_head_and_limit() {
        let (store, _, task) = fixture().await;
        let other = TaskId::new();
        let mut events = Vec::new();
        for index in 0..1006 {
            let kind = if index % 501 == 0 {
                "diagnostic.match"
            } else {
                "diagnostic.noise"
            };
            events.push(NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: task.to_string(),
                },
                kind,
                json!({"index":index}),
            ));
        }
        store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: "filtered-events".into(),
                expected_stream_version: None,
                artifacts: vec![],
                events,
            })
            .await
            .unwrap();
        store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: "other-filtered-events".into(),
                expected_stream_version: None,
                artifacts: vec![],
                events: vec![NewEvent::hub(
                    StreamRef {
                        stream_type: StreamType::Task,
                        stream_id: other.to_string(),
                    },
                    "diagnostic.match",
                    json!({}),
                )],
            })
            .await
            .unwrap();
        let head = store.event_head().await.unwrap();
        let expected: Vec<_> = store
            .task_events(&task, 0, head, 2000)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "diagnostic.match")
            .collect();
        // Unrelated payloads must never be deserialized by this read.
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE events SET payload_json = 'invalid' WHERE event_type = 'diagnostic.noise'",
                [],
            )
            .unwrap();
        let first = store
            .task_events_of_types(&task, 0, head, &["diagnostic.match"], 1)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sequence, expected[0].sequence);
        let tail = store
            .task_events_of_types(
                &task,
                first[0].sequence,
                expected[1].sequence,
                &["diagnostic.match"],
                10,
            )
            .await
            .unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sequence, expected[1].sequence);
        assert_eq!(
            store
                .task_events_of_types(&task, 0, head, &["diagnostic.match"], 10)
                .await
                .unwrap()
                .len(),
            3
        );
        assert!(
            store
                .task_events_of_types(&task, 0, head, &[], 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .task_events_of_types(&task, 0, head, &["diagnostic.match"], 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn timeline_cursor_covers_exactly_the_returned_projection() {
        let (store, core, task) = fixture().await;
        let before = core.timeline_snapshot(&task).await.unwrap();
        assert_eq!(before.last_sequence, 2);
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: task.to_string(),
            },
            "message.delta",
            json!({"taskId":task,"turnId":agent_domain::TurnId::new(),"itemId":"answer","text":"你好"}),
        );
        let outcome = store
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: "stream-test".into(),
                expected_stream_version: None,
                artifacts: vec![],
                events: vec![event],
            })
            .await
            .unwrap();
        let after = core.timeline_snapshot(&task).await.unwrap();
        assert_eq!(after.last_sequence, outcome.events[0].sequence);
        assert_eq!(after.items[0].text, "你好");
        assert_eq!(after.items, core.timeline(&task).await.unwrap());
        assert_eq!(
            store
                .sync_events(before.last_sequence, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .sync_events(after.last_sequence, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
