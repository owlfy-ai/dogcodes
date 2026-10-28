use super::*;

#[test]
fn expired_check_budget_fails_closed_and_removes_progress_hook() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE large_fixture(value); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO large_fixture SELECT x FROM n;").unwrap();
    connection
        .pragma_update(None, "user_version", STORE_FORMAT_VERSION)
        .unwrap();
    assert!(check_database_with_budget(&connection, std::time::Duration::ZERO).is_err());
    check_database(&connection).unwrap();
}

#[test]
fn corrupt_database_is_rejected_without_modifying_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    let original = b"not a SQLite database: private fixture";
    std::fs::write(&path, original).unwrap();
    assert!(SqliteStore::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!directory.path().join("app.db-wal").exists());
}

#[test]
fn healthy_database_can_be_created_and_reopened_with_committed_wal_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    let store = SqliteStore::open(&path).unwrap();
    store.connection.lock().unwrap().execute_batch(
        "CREATE TABLE recovery_fixture(value TEXT); INSERT INTO recovery_fixture VALUES ('committed');"
    ).unwrap();
    let reopened = SqliteStore::open(&path).unwrap();
    let value: String = reopened
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT value FROM recovery_fixture", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "committed");
}

#[test]
fn reopening_does_not_repair_or_backfill_existing_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    let store = SqliteStore::open(&path).unwrap();
    store
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE task_artifacts;")
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    let exists: bool = reopened
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='task_artifacts')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        !exists,
        "opening existing data must not perform schema upgrades"
    );
}

fn user_version(path: &std::path::Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn empty_database_initializes_only_the_current_format() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    drop(SqliteStore::open(&path).unwrap());
    assert_eq!(user_version(&path), STORE_FORMAT_VERSION);
}

#[test]
fn unstamped_existing_database_is_refused_without_adopting_or_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    drop(SqliteStore::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 0)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let error = SqliteStore::open(&path).err().unwrap().to_string();
    assert!(error.contains(UNSUPPORTED_FORMAT_ERROR), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(user_version(&path), 0);
}

#[test]
fn an_unstamped_view_only_database_is_not_treated_as_empty() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE VIEW existing_view AS SELECT 'keep' AS value;")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let error = SqliteStore::open(&path).err().unwrap().to_string();
    assert!(error.contains(UNSUPPORTED_FORMAT_ERROR), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(user_version(&path), 0);
}

#[test]
fn reopening_does_not_backfill_missing_indexes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    let store = SqliteStore::open(&path).unwrap();
    store
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP INDEX events_task_type_sequence;")
        .unwrap();
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    let exists: bool = store.connection.lock().unwrap().query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='events_task_type_sequence')",
        [], |row| row.get(0),
    ).unwrap();
    assert!(!exists);
}

#[test]
fn newer_format_is_refused_without_modifying_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.db");
    drop(SqliteStore::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", STORE_FORMAT_VERSION + 1)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let error = SqliteStore::open(&path).err().unwrap().to_string();
    assert!(error.contains(NEWER_FORMAT_ERROR), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(user_version(&path), STORE_FORMAT_VERSION + 1);
}
