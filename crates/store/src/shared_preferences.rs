use super::{
    OptionalExtension, RepositoryError, SqliteStore, json_error, poison_error, storage_error,
};

impl SqliteStore {
    /// Read the settings revision and global subscription cursor in one `SQLite`
    /// snapshot. Existing canonical events remain the only durable fact source.
    pub(super) fn read_preferences<T: serde::de::DeserializeOwned + Default>(
        &self,
        stream: &str,
        event_type: &str,
    ) -> Result<(u64, u64, T), RepositoryError> {
        let connection = self.connection.lock().map_err(poison_error)?;
        let transaction = connection.unchecked_transaction().map_err(storage_error)?;
        let head = transaction
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(storage_error)?;
        let latest = transaction.query_row(
            "SELECT stream_version, event_type, payload_json FROM events WHERE stream_type='system' AND stream_id=?1 ORDER BY stream_version DESC LIMIT 1",
            [stream], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        ).optional().map_err(storage_error)?;
        match latest {
            None => Ok((0, head, T::default())),
            Some((revision, kind, payload)) if kind == event_type => Ok((
                revision,
                head,
                serde_json::from_str(&payload).map_err(json_error)?,
            )),
            Some(_) => Err(RepositoryError::Unavailable(
                "unrecognized shared preferences event".into(),
            )),
        }
    }
}
