//! Mobile business operations outside the workspace tool pages.
use super::{ActorRef, Arc, Core, ServiceControl, Value, failure, json, parse_sequence};
use serde::Deserialize;

pub(super) const METHODS: &[&str] = &[
    "history_preferences",
    "query_history",
    "subagent_history",
    "fork_task",
    "fork_receipt",
    "review_executors",
    "start_turn_review",
    "cancel_turn_review",
    "turn_reviews",
    "review_source",
    "build_context_preview",
    "build_edited_context_preview",
    "task_artifacts",
    "read_artifact",
    "review_repair",
    "register_project",
    "host_directories",
    "timeline_tail",
    "timeline_item",
    "paired_devices",
    "revoke_device",
    "attachment_upload_begin",
    "attachment_upload_status",
    "attachment_upload_chunk",
    "attachment_upload_commit",
    "attachment_upload_abort",
    "attachment_receipt",
    "attachment_read_chunk",
    "workspace_attachment_info",
    "workspace_attachment_import",
];
pub(super) async fn call(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    mut args: Value,
    actor: ActorRef,
) -> Result<Value, String> {
    match method {
        "workspace_attachment_info" | "workspace_attachment_import" => {
            attachment(core, control, method, args).await
        }
        method if method.starts_with("attachment_") && METHODS.contains(&method) => {
            crate::attachment_service::dispatch(control.attachments.clone(), method, args).await
        }
        "register_project" => {
            let (request,): (crate::project_registry::ProjectRegistration,) =
                serde_json::from_value(args).map_err(failure)?;
            let result = {
                let _guard = control.projects.lock().await;
                crate::project_registry::register_as(core, request, actor).await?
            };
            if result.created {
                let _ = crate::project_runs::schedule(
                    &control.project_runs,
                    core.clone(),
                    result.project_id.clone(),
                )
                .await;
            }
            Ok(json!(result))
        }
        "review_repair" => {
            parse_sequence(args.pointer_mut("/0/baseVersion"))?;
            let (call,): (crate::review_repairs::RepairCall,) =
                serde_json::from_value(args).map_err(failure)?;
            control.repairs.call_as(core, call, actor).await
        }
        "task_artifacts" | "read_artifact" => {
            let arguments = args.as_array_mut().ok_or("invalid artifact request")?;
            if arguments.is_empty() {
                return Err("invalid artifact request".into());
            }
            arguments[0] = json!(actor);
            if method == "read_artifact" {
                parse_sequence(args.pointer_mut("/1/offset"))?;
            }
            super::super::dispatch(core, method, args).await
        }
        "timeline_tail" => {
            let query: TailQuery = serde_json::from_value(args).map_err(failure)?;
            let snapshot =
                super::super::dispatch(core, "timeline_snapshot", json!([query.task_id])).await?;
            tail(
                snapshot,
                query.before_turn.as_deref(),
                query.until_turn.as_deref(),
                TAIL,
            )
        }
        "timeline_item" => {
            let query: ItemQuery = serde_json::from_value(args).map_err(failure)?;
            let snapshot =
                super::super::dispatch(core, "timeline_snapshot", json!([query.task_id])).await?;
            timeline_item(snapshot, &query.item_id, TAIL.hard)
        }
        "host_directories" => tokio::task::spawn_blocking(move || directories(args))
            .await
            .map_err(failure)?,
        "paired_devices" => {
            let state = control.remote.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            Ok(json!(ready.book.grants().iter().map(|grant| json!({
                "deviceId":grant.device_id,"revoked":grant.revoked,"revision":grant.revision.to_string()
            })).collect::<Vec<_>>()))
        }
        "revoke_device" => {
            let device: String = serde_json::from_value(args).map_err(failure)?;
            control.remote.revoke_device(&device).await?;
            Ok(json!({"revoked":true,"deviceId":device}))
        }
        method if METHODS.contains(&method) => super::super::dispatch(core, method, args).await,
        _ => Err("remote method unavailable".into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Import {
    source: crate::workspace_service::DirectoryRequest,
    upload_id: String,
    request: Option<agent_protocol::AttachmentUploadRequest>,
}
async fn attachment(
    core: &Arc<Core>,
    control: &ServiceControl,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let request: Import = serde_json::from_value(args).map_err(failure)?;
    uuid::Uuid::parse_str(&request.upload_id).map_err(|_| "附件操作标识无效")?;
    let base = crate::workspace_service::directory_base(core, &request.source).await?;
    let store = control.attachments.clone();
    let import = method == "workspace_attachment_import";
    tokio::task::spawn_blocking(move || {
        if import {
            let expected = request.request.as_ref().ok_or("缺少已核实的附件身份")?;
            if expected.upload_id != request.upload_id {
                return Err("附件操作标识冲突".into());
            }
            if let Some(receipt) = store.receipt(expected)? {
                return Ok(json!(receipt));
            }
        }
        let target = crate::workspace_service::resolve(&base, &request.source.path)?;
        if !target.is_file() {
            return Err("不是普通项目文件".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&target)
            .map_err(|_| "无法读取项目文件")?
            .take(crate::attachment_service::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取项目文件")?;
        if bytes.len() > crate::attachment_service::MAX_BYTES {
            return Err("附件超过 25 MB".into());
        }
        let name = target
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("文件名无效")?
            .to_string();
        let mime = match target
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "pdf" => "application/pdf",
            "json" => "application/json",
            "txt" | "md" | "rs" | "ts" | "tsx" | "js" | "css" | "html" => "text/plain",
            _ => "application/octet-stream",
        };
        let identity = agent_protocol::AttachmentUploadRequest {
            upload_id: request.upload_id,
            name,
            mime_type: mime.into(),
            byte_length: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
        };
        if !import {
            return Ok(json!(identity));
        }
        if request.request.as_ref() != Some(&identity) {
            return Err("项目文件已改变，请重新核实后添加附件".into());
        }
        Ok(json!(store.save(
            &identity,
            &base64::engine::general_purpose::STANDARD.encode(bytes)
        )?))
    })
    .await
    .map_err(failure)?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TailQuery {
    task_id: String,
    /// A turn cursor (`earlierBefore` of the previous page).
    #[serde(default)]
    before_turn: Option<String>,
    /// Extend the page back until this turn is included (history-search jumps).
    #[serde(default)]
    until_turn: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemQuery {
    task_id: String,
    item_id: String,
}
#[derive(Clone, Copy)]
struct TailLimits {
    /// Soft page size; far below the 8 MB frame so a page stays quick.
    budget: usize,
    turns: usize,
    /// Hard ceiling for any page, under the frame limit with room for framing.
    hard: usize,
}
const TAIL: TailLimits = TailLimits {
    budget: 2 * 1024 * 1024,
    turns: 20,
    hard: 6 * 1024 * 1024,
};
/// Items join their turn by `turnId`, wherever they sit: a review result or
/// background output can be written long after newer turns began.
fn turn_key(item: &Value) -> String {
    match &item["turnId"] {
        Value::String(turn) => format!("turn:{turn}"),
        _ => format!("item:{}", item["itemId"].as_str().unwrap_or_default()),
    }
}
/// The newest whole turns before `before`, turns ordered by first appearance.
/// Within `budget`/`turns`, except that one turn is always sent and `until`
/// keeps extending the page (up to `hard`) until that turn is in it. A page
/// over `hard` has its largest items trimmed (`truncated`); `timeline_item`
/// reads one in full. `earlierBefore` is the cursor for the next older page.
fn tail(
    snapshot: Value,
    before: Option<&str>,
    until: Option<&str>,
    limits: TailLimits,
) -> Result<Value, String> {
    let mut items = snapshot["items"]
        .as_array()
        .ok_or("invalid timeline snapshot")?
        .clone();
    items.iter_mut().for_each(compact_tool);
    let items = &items;
    let mut order: Vec<String> = Vec::new();
    let mut sizes: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::default();
    for item in items {
        let key = turn_key(item);
        let bytes = serde_json::to_vec(item).map_err(failure)?.len();
        let entry = sizes.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            (0, 0)
        });
        entry.0 += bytes;
        entry.1 += 1;
    }
    let position = |key: &str| order.iter().position(|k| k == key);
    let end = match before {
        Some(key) => position(key).ok_or("更早的内容已变化，请刷新会话后重试")?,
        None => order.len(),
    };
    let target = match until {
        Some(key) => Some(position(key).ok_or("找不到这一轮对话，它可能已被删除")?),
        None => None,
    };
    let (mut start, mut bytes) = (end, 0);
    while start > 0 {
        let size = sizes[&order[start - 1]].0;
        let selected = end - start;
        let reaching = target.is_some_and(|t| t < start) && bytes + size <= limits.hard;
        let fits = selected < limits.turns && bytes + size <= limits.budget;
        if selected > 0 && !reaching && !fits {
            break;
        }
        start -= 1;
        bytes += size;
    }
    let chosen: std::collections::HashSet<&String> = order[start..end].iter().collect();
    let mut page: Vec<Value> = items
        .iter()
        .filter(|item| chosen.contains(&turn_key(item)))
        .cloned()
        .collect();
    if bytes > limits.hard {
        let count: usize = chosen.iter().map(|key| sizes[*key].1).sum();
        let cap = (limits.hard * 3 / 4 / count.max(1)).max(2048);
        for item in &mut page {
            trim_item(item, cap);
        }
    }
    Ok(json!({
        "items": page,
        "lastSequence": snapshot["lastSequence"],
        "earlierBefore": (start > 0).then(|| order[start].clone()),
    }))
}
fn timeline_item(snapshot: Value, item_id: &str, limit: usize) -> Result<Value, String> {
    let mut item = snapshot["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["itemId"] == item_id))
        .ok_or("这条内容已不存在，请刷新会话")?
        .clone();
    compact_tool(&mut item);
    if serde_json::to_vec(&item).map_err(failure)?.len() > limit {
        return Err("内容过大，无法在手机上完整显示，请在电脑上查看".into());
    }
    Ok(item)
}
/// The phone shows what a tool did, not what it returned: outputs, streamed
/// output, diffs and long arguments stay on the PC. A failed step keeps the
/// end of its output (why it stopped) and a subagent the start (its result).
/// Thinking keeps only its status and duration. Agent messages are untouched.
fn compact_tool(item: &mut Value) {
    const INPUT_CAP: usize = 512;
    const PREVIEW: usize = 2048;
    if item["kind"] != "activity" {
        return;
    }
    let Some(Ok(mut data)) = item["text"].as_str().map(serde_json::from_str::<Value>) else {
        return;
    };
    let kind = data["type"].as_str().unwrap_or_default();
    if kind.is_empty() || kind == "message" {
        return;
    }
    if kind == "thought" {
        if let Some(map) = data.as_object_mut() {
            for key in ["text", "liveText", "textTruncated"] {
                map.remove(key);
            }
            map.insert("compact".into(), json!(true));
        }
        item["text"] = json!(data.to_string());
        return;
    }
    let name = data["name"].as_str().unwrap_or_default();
    let subagent = kind == "subagent"
        || name.eq_ignore_ascii_case("agent")
        || name.eq_ignore_ascii_case("task")
        || name.starts_with("Task:");
    let failed = matches!(data["status"].as_str(), Some("failed" | "error"));
    let Some(map) = data.as_object_mut() else {
        return;
    };
    let output = map.remove("output");
    let content = map.remove("content");
    let streamed = map.remove("text");
    let result = output
        .or(content)
        .or(streamed)
        .map(|value| output_string(&value))
        .filter(|text| !text.trim().is_empty());
    if let Some(text) = result {
        if subagent {
            map.insert("outputPreview".into(), json!(prefix(&text, PREVIEW)));
        } else if failed {
            map.insert(
                "outputPreview".into(),
                json!(last_lines(&text, 20, PREVIEW)),
            );
        }
    }
    if let Some(Value::Array(changes)) = map.get_mut("changes") {
        for change in changes.iter_mut().filter_map(Value::as_object_mut) {
            change.remove("diff");
        }
    }
    if let Some(input) = map.get_mut("input") {
        shrink_strings(input, INPUT_CAP);
    }
    map.insert("compact".into(), json!(true));
    item["text"] = json!(data.to_string());
}
fn output_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .map(output_string)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) if map.contains_key("stdout") || map.contains_key("stderr") => {
            ["stdout", "stderr"]
                .iter()
                .filter_map(|key| map.get(*key))
                .map(output_string)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        }
        Value::Object(map) if map.get("text").is_some_and(Value::is_string) => {
            output_string(&map["text"])
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
/// The last `lines` lines, at most about `cap` bytes.
fn last_lines(text: &str, lines: usize, cap: usize) -> String {
    let text = text.trim_end();
    let start = text
        .char_indices()
        .rev()
        .filter(|(_, c)| *c == '\n')
        .nth(lines - 1)
        .map_or(0, |(i, _)| i + 1);
    let mut start = start.max(text.len().saturating_sub(cap));
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}
/// Shortens an item's text to about `cap` bytes. Structured activity text
/// stays valid JSON: only its long strings are cut.
fn trim_item(item: &mut Value, cap: usize) {
    let Some(text) = item["text"].as_str() else {
        return;
    };
    if text.len() <= cap {
        return;
    }
    let full = text.len();
    let trimmed = match serde_json::from_str::<Value>(text) {
        Ok(mut data) if data.is_object() => {
            shrink_strings(&mut data, (cap / 4).max(512));
            data.to_string()
        }
        _ => format!("{}…", prefix(text, cap)),
    };
    item["text"] = json!(trimmed);
    item["truncated"] = json!(true);
    item["fullBytes"] = json!(full);
}
fn shrink_strings(value: &mut Value, cap: usize) {
    match value {
        Value::String(text) if text.len() > cap => *text = format!("{}…", prefix(text, cap)),
        Value::Array(values) => values.iter_mut().for_each(|v| shrink_strings(v, cap)),
        Value::Object(map) => map.values_mut().for_each(|v| shrink_strings(v, cap)),
        _ => {}
    }
}
fn prefix(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryQuery {
    path: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    show_hidden: bool,
}
/// Without a path, Windows lists its drives and other systems open the home
/// directory. Hidden entries appear only when the phone asks for them.
fn directories(args: Value) -> Result<Value, String> {
    let query: DirectoryQuery = serde_json::from_value(args).map_err(failure)?;
    let path = match query.path {
        Some(path) => std::path::PathBuf::from(path),
        None if cfg!(windows) => return Ok(drives()),
        None => std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|home| home.is_dir())
            .unwrap_or_else(|| "/".into()),
    };
    if !path.is_absolute() {
        return Err("目录路径必须为绝对路径".into());
    }
    let path = crate::project_registry::plain_path(
        path.canonicalize()
            .map_err(|_| "无法读取目录，请检查系统权限")?,
    );
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|_| "无法读取目录，请检查系统权限")?
    {
        // One unreadable entry (a vanished file, a broken link) must not hide
        // the rest of the directory.
        let Ok(entry) = entry else { continue };
        // A symlinked folder counts: its target is a directory.
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        };
        if is_dir && (query.show_hidden || !hidden(&entry)) {
            entries.push(json!({"name":entry.file_name().to_string_lossy(),"path":entry.path()}));
        }
        if entries.len() > 20_000 {
            return Err("目录过大，请输入更具体的目录路径".into());
        }
    }
    // As Finder and Explorer show it: case-insensitive, then exact.
    entries.sort_by_cached_key(|entry| {
        let name = entry["name"].as_str().unwrap_or_default().to_owned();
        (name.to_lowercase(), name)
    });
    let total = entries.len();
    Ok(
        json!({"path":path,"parent":path.parent(),"entries":entries.into_iter().skip(query.offset).take(200).collect::<Vec<_>>(),"next":(query.offset.saturating_add(200)<total).then_some(query.offset+200)}),
    )
}
fn drives() -> Value {
    let roots: Vec<_> = (b'A'..=b'Z')
        .map(|drive| format!("{}:\\", char::from(drive)))
        .filter(|path| std::path::Path::new(path).is_dir())
        .map(|path| json!({"name":path,"path":path}))
        .collect();
    json!({"path":null,"parent":null,"entries":roots,"next":null})
}
/// Dot entries everywhere, plus what Finder or Explorer hide by flag.
fn hidden(entry: &std::fs::DirEntry) -> bool {
    if entry.file_name().to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_HIDDEN: u32 = 0x8000;
        if entry
            .metadata()
            .is_ok_and(|m| m.st_flags() & UF_HIDDEN != 0)
        {
            return true;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if entry
            .metadata()
            .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(value: &Value) -> Vec<&str> {
        value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn hidden_directories_are_listed_only_on_request() {
        let root = tempfile::tempdir().unwrap();
        for name in [".git", "src", "docs"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        std::fs::write(root.path().join("file.txt"), "").unwrap();
        let path = root.path().to_str().unwrap();
        let shown = directories(json!({"path": path})).unwrap();
        assert_eq!(names(&shown), ["docs", "src"]);
        let all = directories(json!({"path": path, "showHidden": true})).unwrap();
        assert_eq!(names(&all), [".git", "docs", "src"]);
    }

    /// Items as (turn, item id, text) in store order.
    fn timeline(items: &[(&str, &str, &str)]) -> Value {
        json!({
            "items": items
                .iter()
                .map(|(turn, id, text)| json!({"itemId": id, "turnId": turn, "text": text}))
                .collect::<Vec<_>>(),
            "lastSequence": 42,
        })
    }
    fn ids(page: &Value) -> Vec<&str> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["itemId"].as_str().unwrap())
            .collect()
    }
    const ROOMY: TailLimits = TailLimits {
        budget: usize::MAX,
        turns: 2,
        hard: usize::MAX,
    };

    #[test]
    fn tail_pages_whole_turns_from_newest_to_oldest() {
        let snapshot = timeline(&[
            ("a", "a0", ""),
            ("a", "a1", ""),
            ("b", "b0", ""),
            ("c", "c0", ""),
        ]);
        let page = tail(snapshot.clone(), None, None, ROOMY).unwrap();
        assert_eq!(ids(&page), ["b0", "c0"]);
        assert_eq!(page["lastSequence"], 42);
        assert_eq!(page["earlierBefore"], "turn:b");
        let older = tail(snapshot.clone(), Some("turn:b"), None, ROOMY).unwrap();
        assert_eq!(ids(&older), ["a0", "a1"]);
        assert!(older["earlierBefore"].is_null());
        assert!(tail(snapshot, Some("turn:gone"), None, ROOMY).is_err());
    }

    #[test]
    fn an_item_written_late_still_travels_with_its_turn() {
        // A review result for turn a lands after turn c started.
        let snapshot = timeline(&[
            ("a", "a0", ""),
            ("b", "b0", ""),
            ("c", "c0", ""),
            ("a", "a-review", ""),
        ]);
        let page = tail(snapshot.clone(), None, None, ROOMY).unwrap();
        assert_eq!(ids(&page), ["b0", "c0"]);
        let older = tail(snapshot, Some("turn:b"), None, ROOMY).unwrap();
        assert_eq!(ids(&older), ["a0", "a-review"]);
    }

    #[test]
    fn until_turn_extends_the_page_past_the_turn_limit() {
        let snapshot = timeline(&[
            ("a", "a0", ""),
            ("b", "b0", ""),
            ("c", "c0", ""),
            ("d", "d0", ""),
        ]);
        let page = tail(snapshot.clone(), None, Some("turn:a"), ROOMY).unwrap();
        assert_eq!(ids(&page), ["a0", "b0", "c0", "d0"]);
        assert!(page["earlierBefore"].is_null());
        assert!(tail(snapshot, None, Some("turn:gone"), ROOMY).is_err());
    }

    #[test]
    fn tail_respects_the_byte_budget_but_always_sends_one_turn() {
        let snapshot = timeline(&[
            ("a", "a0", ""),
            ("b", "b0", ""),
            ("c", "c0", ""),
            ("c", "c1", ""),
        ]);
        let one = serde_json::to_vec(&snapshot["items"][0]).unwrap().len();
        let tight = TailLimits {
            budget: one,
            turns: 20,
            hard: usize::MAX,
        };
        assert_eq!(
            ids(&tail(snapshot.clone(), None, None, tight).unwrap()),
            ["c0", "c1"]
        );
        let roomier = TailLimits {
            budget: one * 3,
            turns: 20,
            hard: usize::MAX,
        };
        let page = tail(snapshot, None, None, roomier).unwrap();
        assert_eq!(ids(&page), ["b0", "c0", "c1"]);
        assert_eq!(page["earlierBefore"], "turn:b");
    }

    #[test]
    fn an_oversized_turn_is_trimmed_under_the_hard_limit_and_readable_in_full() {
        let long = "y".repeat(200_000);
        let snapshot = timeline(&[("a", "m1", &long), ("a", "m2", &long)]);
        let limits = TailLimits {
            budget: 10_000,
            turns: 20,
            hard: 100_000,
        };
        let page = tail(snapshot.clone(), None, None, limits).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 100_000);
        for item in page["items"].as_array().unwrap() {
            assert_eq!(item["truncated"], true);
            assert_eq!(item["fullBytes"], 200_000);
        }
        let full = timeline_item(snapshot.clone(), "m1", usize::MAX).unwrap();
        assert_eq!(full["text"], long);
        assert!(timeline_item(snapshot.clone(), "m1", 1000).is_err());
        assert!(timeline_item(snapshot, "gone", usize::MAX).is_err());
    }

    fn activity(data: Value) -> Value {
        json!({"itemId": "t", "turnId": "a", "kind": "activity", "text": data.to_string()})
    }
    fn data(item: &Value) -> Value {
        serde_json::from_str(item["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn tool_steps_reach_the_phone_as_summaries() {
        let mut ok = activity(json!({
            "type": "tool", "name": "Bash", "status": "completed",
            "input": {"command": "cat big.log", "content": "z".repeat(5000)},
            "output": {"stdout": "x".repeat(100_000)},
            "text": "streamed",
            "changes": [{"path": "src/a.rs", "diff": "+".repeat(9000), "additions": 3}],
        }));
        compact_tool(&mut ok);
        let ok = data(&ok);
        assert_eq!(ok["compact"], true);
        assert_eq!(ok["input"]["command"], "cat big.log");
        assert!(ok["input"]["content"].as_str().unwrap().len() < 600);
        assert!(ok.get("output").is_none() && ok.get("text").is_none());
        assert!(ok.get("outputPreview").is_none());
        assert_eq!(
            ok["changes"][0],
            json!({"path": "src/a.rs", "additions": 3})
        );

        let lines: Vec<String> = (1..=50).map(|n| format!("line {n}")).collect();
        let mut failed = activity(json!({
            "type": "tool", "name": "Bash", "status": "failed",
            "output": {"stdout": lines.join("\n"), "stderr": "boom"},
        }));
        compact_tool(&mut failed);
        let preview = data(&failed)["outputPreview"].as_str().unwrap().to_owned();
        assert!(preview.ends_with("boom"));
        assert!(preview.starts_with("line 32"));

        let mut agent = activity(json!({
            "type": "subagent", "status": "completed", "output": "结论：已修复",
        }));
        compact_tool(&mut agent);
        assert_eq!(data(&agent)["outputPreview"], "结论：已修复");

        let mut thought = activity(json!({
            "type": "thought", "status": "completed", "durationMs": 3100,
            "text": "先读配置", "textTruncated": true, "liveText": "tail",
        }));
        compact_tool(&mut thought);
        assert_eq!(
            data(&thought),
            json!({"type": "thought", "status": "completed", "durationMs": 3100, "compact": true})
        );

        let message = activity(json!({"type": "message", "text": "x".repeat(9000)}));
        let mut kept = message.clone();
        compact_tool(&mut kept);
        assert_eq!(kept, message);
        let mut plain = json!({"itemId": "m", "kind": "assistant_message", "text": "hi"});
        compact_tool(&mut plain);
        assert_eq!(plain["text"], "hi");
    }

    #[test]
    fn summaries_let_more_turns_fit_a_page() {
        let heavy =
            activity(json!({"type": "tool", "status": "completed", "output": "x".repeat(50_000)}));
        let snapshot = json!({
            "items": [
                {"itemId": "a", "turnId": "a", "text": ""},
                {"itemId": "t", "turnId": "b", "kind": "activity", "text": heavy["text"]},
            ],
            "lastSequence": 1,
        });
        let limits = TailLimits {
            budget: 5_000,
            turns: 20,
            hard: usize::MAX,
        };
        let page = tail(snapshot, None, None, limits).unwrap();
        assert_eq!(ids(&page), ["a", "t"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_listed_and_names_sort_case_insensitively() {
        let root = tempfile::tempdir().unwrap();
        for name in ["Zeta", "apple", "Mango"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        std::os::unix::fs::symlink(root.path().join("apple"), root.path().join("linked")).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), root.path().join("broken"))
            .unwrap();
        let listing = directories(json!({"path": root.path().to_str().unwrap()})).unwrap();
        assert_eq!(names(&listing), ["apple", "linked", "Mango", "Zeta"]);
    }

    #[cfg(unix)]
    #[test]
    fn without_a_path_the_home_directory_opens() {
        let home = std::env::var("HOME").unwrap();
        let listing = directories(json!({"path": null})).unwrap();
        assert_eq!(
            listing["path"].as_str().unwrap(),
            std::path::Path::new(&home)
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        assert!(listing["parent"].is_string());
    }
}
