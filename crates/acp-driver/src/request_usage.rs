//! Read only the native session bound to this driver, with bounded files and no prompts.
use driver_api::usage::{RequestUsageReport, SessionUsageSnapshot, UsageRequest, number, token};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
};
const LIMIT: u64 = 32 * 1024 * 1024;
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
fn configured(key: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(key).map_or(fallback, PathBuf::from)
}
fn read(file: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(file).ok()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return None;
    }
    let f = std::fs::File::open(file).ok()?;
    let mut reader: Box<dyn Read> = if file.extension().is_some_and(|s| s == "zstd") {
        Box::new(zstd::stream::read::Decoder::new(f).ok()?)
    } else {
        Box::new(f)
    };
    let mut s = String::new();
    reader
        .by_ref()
        .take(LIMIT + 1)
        .read_to_string(&mut s)
        .ok()?;
    (s.len() as u64 <= LIMIT).then_some(s)
}
fn records(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.split_inclusive('\n')
        .filter(|s| s.ends_with('\n'))
        .filter_map(|s| serde_json::from_str(s).ok())
}
fn session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    let mut found = None;
    for entry in std::fs::read_dir(root).ok()?.take(4096) {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let path = entry.path().join(id);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            if found.is_some() {
                return None;
            }
            found = Some(path);
        }
    }
    found
}
fn bound(value: &Value, id: &str, cwd: &Path) -> bool {
    value["id"] == id
        && value["cwd"]
            .as_str()
            .and_then(|s| Path::new(s).canonicalize().ok())
            .as_deref()
            == Some(cwd)
}
pub(super) fn read_native(provider: &str, cwd: &Path, id: &str) -> Option<SessionUsageSnapshot> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return None;
    }
    let cwd = cwd.canonicalize().ok()?;
    match provider {
        "deepseek-harness" => dsh(&cwd, id),
        "opencode" => opencode(&cwd, id),
        "kimi" => kimi(&cwd, id),
        "grok" => grok(&cwd, id),
        "codebuddy" => super::session_usage::read_native(provider, &cwd, id),
        _ => None,
    }
}
fn snapshot(mut report: RequestUsageReport) -> SessionUsageSnapshot {
    if report.records.is_empty() && report.unavailable_reason.is_none() {
        report.unavailable_reason = Some("原生会话尚无可确认的请求用量".into());
    }
    SessionUsageSnapshot {
        request_usage: Some(report),
        ..SessionUsageSnapshot::new()
    }
}
fn dsh(cwd: &Path, id: &str) -> Option<SessionUsageSnapshot> {
    let root = configured("DSH_HOME", home()?.join(".dsh"));
    let dir = session_dir(&root.join("sessions"), id)?;
    let file = ["session.v4.jsonl.zstd", "session.v4.jsonl"]
        .into_iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())?;
    let text = read(&file)?;
    let mut rows = records(&text);
    if !bound(&rows.next()?, id, cwd) {
        return None;
    }
    let mut report = RequestUsageReport::default();
    for row in rows {
        if let Some(record) = dsh_record(&row) {
            report.observe(record);
        }
    }
    Some(snapshot(report))
}
fn dsh_record(row: &Value) -> Option<UsageRequest> {
    if row["type"] != "assistant/message" || row["surfaceOp"] != "append" {
        return None;
    }
    let data = &row["data"];
    let message = &data["message"];
    let u = &data["usage"];
    if message["source"]["kind"] != "model" {
        return None;
    }
    let read = u["cacheReadTokens"].as_u64()?;
    let write = u["cacheWriteTokens"].as_u64()?;
    Some(UsageRequest {
        request_id: message["id"].as_str()?.into(),
        model: message["source"]["model"].as_str().map(str::to_owned),
        source: "dsh.journal.v4".into(),
        input_tokens: u["inputTokens"]
            .as_u64()?
            .checked_add(read)?
            .checked_add(write),
        output_tokens: u["outputTokens"].as_u64(),
        cache_read_tokens: Some(read),
        cache_write_tokens: Some(write),
        reasoning_tokens: u["reasoningTokens"].as_u64(),
        ..UsageRequest::default()
    })
}
fn kimi(cwd: &Path, id: &str) -> Option<SessionUsageSnapshot> {
    let dir = session_dir(&home()?.join(".kimi-code/sessions"), id)?;
    let state: Value = serde_json::from_str(&read(&dir.join("state.json"))?).ok()?;
    if !bound(&state, id, cwd) {
        return None;
    }
    let text = read(&dir.join("agents/main/wire.jsonl"))?;
    let row = records(&text)
        .filter(|r| r["type"] == "usage.record" && r["usageScope"] == "turn")
        .last()?;
    let u = &row["usage"];
    Some(SessionUsageSnapshot {
        model: row["model"].as_str().map(str::to_owned),
        native_summary_id: row["time"].as_u64().map(|time| format!("kimi-turn:{time}")),
        input_tokens: u["inputOther"].as_u64(),
        output_tokens: u["output"].as_u64(),
        cache_read_tokens: u["inputCacheRead"].as_u64(),
        cache_write_tokens: u["inputCacheCreation"].as_u64(),
        totals_scope: Some("turn".into()),
        request_usage: Some(RequestUsageReport::unavailable(
            "Kimi 提供本轮摘要，未提供可确认的逐请求身份",
        )),
        ..SessionUsageSnapshot::new()
    })
}
pub(super) fn grok_report(update: &Value, report: &mut RequestUsageReport) {
    if update["sessionUpdate"] != "turn_completed" {
        return;
    }
    let Some(prompt) = update["prompt_id"].as_str() else {
        return;
    };
    for (model, u) in update["usage"]["modelUsage"]
        .as_object()
        .into_iter()
        .flatten()
    {
        // A turn can contain many model calls. Only the native explicit single-call case
        // establishes that this aggregate represents exactly one request.
        if u["modelCalls"].as_u64() != Some(1) {
            continue;
        }
        let write = token(u, &["cacheCreationTokens"]);
        report.observe(UsageRequest {
            request_id: format!("{prompt}:{model}"),
            model: Some(model.clone()),
            source: "grok.single_model_call".into(),
            input_tokens: token(u, &["inputTokens"]),
            output_tokens: token(u, &["outputTokens"]),
            cache_read_tokens: write
                .filter(|w| *w == 0)
                .and_then(|_| token(u, &["cachedReadTokens"])),
            cache_write_tokens: write.filter(|w| *w == 0),
            reasoning_tokens: token(u, &["reasoningTokens"]),
            cost_usd: super::grok_usage::cost(u),
            ..UsageRequest::default()
        });
    }
}
fn grok(cwd: &Path, id: &str) -> Option<SessionUsageSnapshot> {
    let root = configured("GROK_HOME", home()?.join(".grok"));
    let dir = session_dir(&root.join("sessions"), id)?;
    let encoded = dir.parent()?.file_name()?.to_str()?;
    let url = url::Url::parse(&format!("file:///{encoded}")).ok()?;
    // URL path segments are encoded as one folder; decode without interpreting URL authority.
    let decoded = url.to_file_path().ok()?;
    if decoded.canonicalize().ok().as_deref() != Some(cwd) {
        return None;
    }
    let text = read(&dir.join("updates.jsonl"))?;
    let mut report = RequestUsageReport::unavailable("Grok 本轮摘要未能拆分为单次模型请求");
    let mut latest = None;
    let mut costs = std::collections::BTreeMap::new();
    for row in records(&text) {
        let update = &row["params"]["update"];
        if update["sessionUpdate"] == "turn_completed" {
            grok_report(update, &mut report);
            let mut s = driver_api::usage::acp(&update["usage"]);
            s.totals_scope = Some("turn".into());
            s.native_summary_id = update["prompt_id"].as_str().map(str::to_owned);
            s.cost_usd = super::grok_usage::cost(&update["usage"]);
            if let Some(prompt) = &s.native_summary_id {
                costs.insert(prompt.clone(), s.cost_usd);
            }
            latest = Some(s);
        }
    }
    let mut s = latest.unwrap_or_else(SessionUsageSnapshot::new);
    s.session_cost_usd = costs
        .values()
        .try_fold(0.0, |sum, cost| Some(sum + (*cost)?))
        .filter(|n| n.is_finite());
    s.request_usage = Some(report);
    Some(s)
}
pub(super) fn opencode_database_path() -> Option<PathBuf> {
    let root = configured("XDG_DATA_HOME", home()?.join(".local/share")).join("opencode");
    Some(root.join(std::env::var_os("OPENCODE_DB").unwrap_or_else(|| "opencode.db".into())))
}
fn opencode(cwd: &Path, id: &str) -> Option<SessionUsageSnapshot> {
    let file = opencode_database_path()?;
    let db = rusqlite::Connection::open_with_flags(
        file,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    db.busy_timeout(std::time::Duration::from_millis(100))
        .ok()?;
    let mut report = RequestUsageReport::default();
    for (session_table, table, filter) in [
        ("session_v2", "session_message", " AND type = 'assistant'"),
        ("session", "message", ""),
    ] {
        let dir: Option<String> = db
            .query_row(
                &format!("SELECT directory FROM {session_table} WHERE id=?1"),
                [id],
                |r| r.get(0),
            )
            .ok();
        if dir
            .and_then(|s| Path::new(&s).canonicalize().ok())
            .as_deref()
            != Some(cwd)
        {
            continue;
        }
        let Ok(mut query) = db.prepare(&format!(
            "SELECT id,data FROM {table} WHERE session_id=?1{filter} LIMIT 10001"
        )) else {
            continue;
        };
        let rows = query
            .query_map([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .ok()?;
        for row in rows {
            let (key, data) = row.ok()?;
            if data.len() as u64 > LIMIT {
                report.truncated = true;
                continue;
            }
            let m: Value = serde_json::from_str(&data).ok()?;
            if table == "message" && m["role"] != "assistant" {
                continue;
            }
            if let Some(record) = opencode_record(&key, &m) {
                report.observe(record);
            }
        }
    }
    Some(snapshot(report))
}
fn opencode_record(id: &str, m: &Value) -> Option<UsageRequest> {
    let t = &m["tokens"];
    let input = t["input"].as_u64()?;
    let output = t["output"].as_u64()?;
    let read = t["cache"]["read"].as_u64()?;
    let write = t["cache"]["write"].as_u64()?;
    let reasoning = t["reasoning"].as_u64()?;
    // OpenCode writes all-zero placeholders for provider failures (e.g. HTTP 426).
    // A failed request without actual token evidence must not appear as free usage.
    if !m["error"].is_null() && input == 0 && output == 0 && read == 0 && write == 0 {
        return None;
    }
    if !m["time"]["completed"].is_number() {
        return None;
    }
    Some(UsageRequest {
        request_id: id.into(),
        model: m["model"]["id"]
            .as_str()
            .or_else(|| m["modelID"].as_str())
            .map(str::to_owned),
        source: "opencode.database".into(),
        input_tokens: input.checked_add(read)?.checked_add(write),
        output_tokens: output.checked_add(reasoning),
        cache_read_tokens: Some(read),
        cache_write_tokens: Some(write),
        reasoning_tokens: Some(reasoning),
        cost_usd: number(m, &["cost"]),
        ..UsageRequest::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn multi_call_grok_aggregate_is_not_a_request() {
        let mut report = RequestUsageReport::default();
        grok_report(
            &json!({"sessionUpdate":"turn_completed","prompt_id":"p","usage":{"modelUsage":{"m":{"modelCalls":2,"inputTokens":100}}}}),
            &mut report,
        );
        assert!(report.records.is_empty());
        let value = json!({"sessionUpdate":"turn_completed","prompt_id":"p","usage":{"modelUsage":{"m":{"modelCalls":1,"inputTokens":100,"outputTokens":4}}}});
        grok_report(&value, &mut report);
        grok_report(&value, &mut report);
        assert_eq!(report.records.len(), 1);
    }
    #[test]
    fn provider_failure_zero_placeholder_is_not_metered() {
        let m = json!({"error":{"name":"APIError"},"time":{"completed":1},"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}});
        assert!(opencode_record("r", &m).is_none());
    }
    #[test]
    fn distinct_cache_and_reasoning_conventions_are_normalized() {
        let d=dsh_record(&json!({"type":"assistant/message","surfaceOp":"append","data":{"message":{"id":"r","source":{"kind":"model","model":"d"}},"usage":{"inputTokens":10,"outputTokens":4,"cacheReadTokens":90,"cacheWriteTokens":5,"reasoningTokens":2}}})).unwrap();
        assert_eq!(d.input_tokens, Some(105));
        assert_eq!(d.output_tokens, Some(4));
        let o=opencode_record("r",&json!({"time":{"completed":1},"tokens":{"input":10,"output":4,"reasoning":2,"cache":{"read":90,"write":5}}})).unwrap();
        assert_eq!(o.input_tokens, Some(105));
        assert_eq!(o.output_tokens, Some(6));
    }
}

#[cfg(test)]
mod live {
    #[test]
    #[ignore = "reads only the explicitly bound native session; no model calls"]
    fn native_usage_for_bound_session() {
        let provider = std::env::var("DOGCODES_USAGE_PROVIDER").unwrap();
        let cwd = std::env::var("DOGCODES_USAGE_CWD").unwrap();
        let id = std::env::var("DOGCODES_USAGE_SESSION").unwrap();
        let usage = super::read_native(&provider, std::path::Path::new(&cwd), &id)
            .expect("native source unavailable");
        println!("{}", serde_json::to_string(&usage).unwrap());
        if let Ok(expected) = std::env::var("DOGCODES_USAGE_EXPECTED_REQUESTS") {
            assert_eq!(
                usage.request_usage.unwrap().records.len(),
                expected.parse::<usize>().unwrap()
            );
        }
    }
}
