//! Supplement `OpenCode`'s advertised catalog with confirmed native HTTP 410 retirements.
//! Read only model/error metadata; never migrate the native DB or retain message contents.
use driver_api::ModelCatalog;
use rusqlite::{Connection, OpenFlags};
use std::{collections::HashMap, path::Path, time::Duration};

pub(super) fn filter_retired(catalog: &mut ModelCatalog) {
    let Some(path) = super::request_usage::opencode_database_path() else {
        return;
    };
    filter_from_database(catalog, &path);
}

fn filter_from_database(catalog: &mut ModelCatalog, path: &Path) {
    let Ok(db) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return;
    };
    if db.busy_timeout(Duration::from_millis(100)).is_err() {
        return;
    }
    let mut model_results: HashMap<String, (i64, bool)> = HashMap::new();
    for (table, filter) in [
        ("session_message", "type = 'assistant'"),
        ("message", "json_extract(data, '$.role') = 'assistant'"),
    ] {
        // Newest bounded records, projected to public model and provider failure facts.
        let sql = format!(
            "SELECT time_created,
                COALESCE(json_extract(data, '$.model.providerID'), json_extract(data, '$.providerID')),
                COALESCE(json_extract(data, '$.model.id'), json_extract(data, '$.modelID')),
                COALESCE(json_extract(data, '$.error.status'), json_extract(data, '$.error.data.statusCode')),
                substr(COALESCE(json_extract(data, '$.error.message'), json_extract(data, '$.error.data.message')), 1, 4096),
                json_extract(data, '$.error') IS NULL AND json_extract(data, '$.time.completed') IS NOT NULL
             FROM {table} WHERE json_valid(data) AND {filter}
             ORDER BY time_created DESC, id DESC LIMIT 2048"
        );
        let Ok(mut query) = db.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, bool>(5)?,
            ))
        }) else {
            continue;
        };
        for row in rows.flatten() {
            let (time, Some(provider), Some(model), status, error, success) = row else {
                continue;
            };
            let retired = status == Some(410)
                && error.is_some_and(|message| {
                    let message = message.to_ascii_lowercase();
                    message.contains("model")
                        && (message.contains("has been deprecated")
                            || message.contains("has been retired"))
                });
            // Transient errors don't hide models. A later success clears a retirement.
            if !retired && !success {
                continue;
            }
            let key = format!("{provider}/{model}");
            let state = model_results.entry(key).or_insert((time, retired));
            if time > state.0 {
                *state = (time, retired);
            }
        }
    }
    catalog.models.retain(|model| {
        !model_results
            .get(&model.id)
            .is_some_and(|(_, retired)| *retired)
    });
    // Preserve the actual native default ID, so callers can detect it is unavailable.
    // Never silently switch providers or select a possibly paid model.
}

#[cfg(test)]
mod tests {
    use super::*;
    use driver_api::ModelOption;
    use serde_json::json;

    #[test]
    fn only_confirmed_retirements_are_hidden_and_later_success_restores_them() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = Connection::open(&path).unwrap();
            let table = if legacy { "message" } else { "session_message" };
            db.execute(
                &format!(
                    "CREATE TABLE {table}(id TEXT, type TEXT, time_created INTEGER, data TEXT)"
                ),
                [],
            )
            .unwrap();
            let insert = |id: &str, time: i64, data: serde_json::Value| {
                db.execute(
                    &format!("INSERT INTO {table} VALUES (?1, 'assistant', ?2, ?3)"),
                    rusqlite::params![id, time, data.to_string()],
                )
                .unwrap();
            };
            for (name, status, message) in [
                ("retired", 410, "Model retired has been deprecated."),
                ("quota", 429, "Model quota exceeded"),
                ("network", 503, "Model is unavailable"),
                ("auth", 403, "Model access denied"),
                ("other", 410, "Endpoint has been deprecated"),
            ] {
                let error = if legacy {
                    json!({"data":{"statusCode":status,"message":message}})
                } else {
                    json!({"status":status,"message":message})
                };
                let data = if legacy {
                    json!({"role":"assistant","providerID":"p","modelID":name,"error":error})
                } else {
                    json!({"model":{"providerID":"p","id":name},"error":error})
                };
                insert(name, 1, data);
            }
            // A newer transient error doesn't invalidate an explicit retirement.
            insert(
                "retry",
                2,
                json!({"role":"assistant","model":{"providerID":"p","id":"retired"},"error":{"status":503,"message":"unavailable"}}),
            );
            let initial = ModelCatalog {
                default_model: Some("p/retired".into()),
                models: ["retired", "quota", "network", "auth", "other"]
                    .into_iter()
                    .map(|id| ModelOption::new(&format!("p/{id}"), id))
                    .collect(),
            };
            let mut catalog = initial.clone();
            filter_from_database(&mut catalog, &path);
            assert_eq!(catalog.models.len(), 4);
            assert_eq!(catalog.default_model, initial.default_model);
            assert!(!catalog.models.iter().any(|m| m.id == "p/retired"));
            insert(
                "success",
                3,
                json!({"role":"assistant","model":{"providerID":"p","id":"retired"},"time":{"completed":3}}),
            );
            let mut catalog = initial.clone();
            filter_from_database(&mut catalog, &path);
            assert_eq!(catalog, initial);
        }
    }

    #[test]
    fn unavailable_database_preserves_the_native_catalog_without_creating_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        let mut catalog = ModelCatalog {
            models: vec![ModelOption::new("p/m", "M")],
            default_model: Some("p/m".into()),
        };
        let initial = catalog.clone();
        filter_from_database(&mut catalog, &path);
        assert_eq!(catalog, initial);
        assert!(!path.exists());
    }
}
