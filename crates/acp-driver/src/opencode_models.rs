//! Supplement `OpenCode`'s advertised catalog from its own request history: hide
//! confirmed HTTP 410 retirements, and mark models the signed-in account was refused.
//! Read only model/error metadata; never migrate the native DB or retain message contents.
use driver_api::ModelCatalog;
use rusqlite::{Connection, OpenFlags};
use std::{collections::HashMap, path::Path, time::Duration};

/// Zen (`opencode/…`) bills pay-as-you-go models to one account balance, so one refusal
/// for funds applies to every paid Zen model. Go (`opencode-go/…`) is the subscription.
const ZEN: &str = "opencode";

pub(super) fn filter_retired(catalog: &mut ModelCatalog) {
    let Some(path) = super::request_usage::opencode_database_path() else {
        return;
    };
    filter_from_database(catalog, &path);
}

#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    Success,
    Retired,
    /// The provider refused this account for the model, with a stable failure kind.
    Refused(&'static str),
}

fn outcome(status: Option<i64>, error: Option<&str>, success: bool) -> Option<Outcome> {
    if success {
        return Some(Outcome::Success);
    }
    let message = error?;
    let lower = message.to_ascii_lowercase();
    if status == Some(410)
        && lower.contains("model")
        && (lower.contains("has been deprecated") || lower.contains("has been retired"))
    {
        return Some(Outcome::Retired);
    }
    // Transient errors (rate limits, outages) say nothing about the account's access.
    match driver_api::failure::classify(message)? {
        kind @ ("insufficient_balance" | "access_denied" | "subscription_required") => {
            Some(Outcome::Refused(kind))
        }
        _ => None,
    }
}

fn is_free(model: &str) -> bool {
    model.ends_with("-free")
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
    let mut latest: HashMap<String, (i64, Outcome)> = HashMap::new();
    // The newest refusal for funds, and the newest paid Zen success (a top-up proves itself).
    let (mut out_of_funds, mut paid_success) = (None::<i64>, None::<i64>);
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
            let Some(outcome) = outcome(status, error.as_deref(), success) else {
                continue;
            };
            if provider == ZEN && !is_free(&model) {
                let newest = match outcome {
                    Outcome::Refused("insufficient_balance") => &mut out_of_funds,
                    Outcome::Success => &mut paid_success,
                    _ => &mut None,
                };
                *newest = Some(newest.map_or(time, |seen| seen.max(time)));
            }
            let key = format!("{provider}/{model}");
            let state = latest.entry(key).or_insert((time, outcome));
            if time > state.0 {
                *state = (time, outcome);
            }
        }
    }
    catalog.models.retain(|model| {
        !latest
            .get(&model.id)
            .is_some_and(|(_, outcome)| *outcome == Outcome::Retired)
    });
    // Funds still missing: no paid Zen request has succeeded since the refusal.
    let unfunded_since = out_of_funds.filter(|refused| paid_success.is_none_or(|ok| ok < *refused));
    for model in &mut catalog.models {
        let zen_paid = model
            .id
            .strip_prefix("opencode/")
            .is_some_and(|id| !is_free(id));
        model.unavailable_reason = match (latest.get(&model.id), unfunded_since) {
            // Funds are account-wide: a later paid success clears every Zen refusal for them.
            (Some((_, Outcome::Refused("insufficient_balance"))), None) if zen_paid => None,
            (Some((_, Outcome::Refused(kind))), _) => Some((*kind).to_owned()),
            (Some((time, Outcome::Success)), Some(refused)) if zen_paid && *time < refused => {
                Some("insufficient_balance".to_owned())
            }
            (Some((_, Outcome::Success)), _) => None,
            (_, Some(_)) if zen_paid => Some("insufficient_balance".to_owned()),
            _ => None,
        };
    }
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
            // Only the refusal for this account is marked; the catalog is otherwise intact.
            let refused: Vec<_> = catalog
                .models
                .iter()
                .filter_map(|m| Some((m.id.as_str(), m.unavailable_reason.as_deref()?)))
                .collect();
            assert_eq!(refused, [("p/auth", "access_denied")]);
            for model in &mut catalog.models {
                model.unavailable_reason = None;
            }
            assert_eq!(catalog, initial);
        }
    }

    #[test]
    fn refused_models_are_marked_and_zen_funds_apply_to_every_paid_zen_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = Connection::open(&path).unwrap();
        db.execute(
            "CREATE TABLE session_message(id TEXT, type TEXT, time_created INTEGER, data TEXT)",
            [],
        )
        .unwrap();
        let insert =
            |id: &str, time: i64, provider: &str, model: &str, error: Option<serde_json::Value>| {
                let mut data =
                    json!({"model":{"providerID":provider,"id":model},"time":{"completed":time}});
                if let Some(error) = error {
                    data["error"] = error;
                }
                db.execute(
                    "INSERT INTO session_message VALUES (?1, 'assistant', ?2, ?3)",
                    rusqlite::params![id, time, data.to_string()],
                )
                .unwrap();
            };
        // Shapes recorded by OpenCode 2.2.
        insert("old", 1, "opencode", "claude-opus-5-5", None);
        insert(
            "funds",
            2,
            "opencode",
            "deepseek-v4.1-flash",
            Some(
                json!({"type":"provider.quota","message":"Insufficient account funds","status":402}),
            ),
        );
        insert(
            "region",
            3,
            "opencode-go",
            "deepseek-v4.1-flash",
            Some(
                json!({"type":"provider.invalid-request","message":"This Go model is hosted outside Europe and the United States. Select Global at https://opencode.ai/console/org_x/settings/privacy#regions to use it.","status":400}),
            ),
        );
        insert(
            "busy",
            4,
            "opencode-go",
            "glm-5.3",
            Some(
                json!({"type":"provider.rate-limit","message":"Rate limit exceeded","status":429}),
            ),
        );
        let ids = [
            "opencode/deepseek-v4.1-flash",
            "opencode/claude-opus-5-5",
            "opencode/glm-5.3-flash-free",
            "opencode-go/deepseek-v4.1-flash",
            "opencode-go/glm-5.3",
            "ollama/qwen3-vl:2b",
        ];
        let initial = ModelCatalog {
            default_model: None,
            models: ids.iter().map(|id| ModelOption::new(id, id)).collect(),
        };
        let reasons = |catalog: &ModelCatalog| {
            catalog
                .models
                .iter()
                .map(|m| m.unavailable_reason.clone())
                .collect::<Vec<_>>()
        };
        let mut catalog = initial.clone();
        filter_from_database(&mut catalog, &path);
        let (balance, denied) = (
            Some("insufficient_balance".to_owned()),
            Some("access_denied".to_owned()),
        );
        assert_eq!(
            reasons(&catalog),
            [balance.clone(), balance, None, denied.clone(), None, None]
        );
        // A paid Zen success after the refusal shows the balance was topped up.
        insert("topped", 5, "opencode", "claude-opus-5-5", None);
        let mut catalog = initial.clone();
        filter_from_database(&mut catalog, &path);
        assert_eq!(reasons(&catalog), [None, None, None, denied, None, None]);
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
