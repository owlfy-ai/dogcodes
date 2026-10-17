//! Incremental, bounded-memory reads of one already-bound native rollout.
use driver_api::usage::RequestUsageReport;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    fs::{File, Metadata},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const MAX_LINE: usize = 1024 * 1024;
const MAX_FACTS: usize = 10_000;
const SCAN_TIME: Duration = Duration::from_millis(500);

struct Stamp {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}
impl Stamp {
    fn new(meta: &Metadata) -> Self {
        Self {
            length: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino())
            },
        }
    }
    fn replaced(&self, next: &Self) -> bool {
        #[cfg(unix)]
        if self.identity != next.identity {
            return true;
        }
        self.length == next.length && self.modified != next.modified
    }
}

#[derive(Default)]
pub(super) struct Reader {
    path: PathBuf,
    thread: String,
    stamp: Option<Stamp>,
    offset: u64,
    anchor: Vec<u8>,
    pending: Vec<u8>,
    oversized: bool,
    identified: bool,
    models: HashMap<String, String>,
    request_turns: HashMap<String, String>,
    report: RequestUsageReport,
}
pub(super) struct Readback {
    pub report: RequestUsageReport,
    pub exact_turns: BTreeSet<String>,
    pub model: Option<String>,
    pub caught_up: bool,
}
impl Reader {
    pub fn read(&mut self, path: &Path, thread: &str, turn: Option<&str>) -> Option<Readback> {
        self.read_budget(path, thread, turn, u64::MAX)
    }
    fn read_budget(
        &mut self,
        path: &Path,
        thread: &str,
        turn: Option<&str>,
        bytes: u64,
    ) -> Option<Readback> {
        let started = Instant::now();
        let mut file = File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() {
            return None;
        }
        let next = Stamp::new(&metadata);
        if self.path != path
            || self.thread != thread
            || next.length < self.offset
            || self.stamp.as_ref().is_some_and(|old| old.replaced(&next))
            || anchor(&mut file, self.offset).ok()? != self.anchor
        {
            *self = Self {
                path: path.into(),
                thread: thread.into(),
                ..Self::default()
            };
        }
        self.stamp = Some(next);
        let end = metadata.len().min(self.offset.saturating_add(bytes));
        file.seek(SeekFrom::Start(self.offset)).ok()?;
        let mut reader = BufReader::with_capacity(64 * 1024, file.take(end - self.offset));
        while self.offset < end && started.elapsed() < SCAN_TIME {
            let chunk = reader.fill_buf().ok()?;
            if chunk.is_empty() {
                break;
            }
            let length = chunk
                .iter()
                .position(|b| *b == b'\n')
                .map_or(chunk.len(), |n| n + 1);
            let complete = chunk[length - 1] == b'\n';
            if !self.oversized && self.pending.len().saturating_add(length) <= MAX_LINE {
                self.pending.extend_from_slice(&chunk[..length]);
            } else {
                self.pending.clear();
                self.oversized = true;
                self.report.truncated = true;
            }
            reader.consume(length);
            self.offset = self.offset.saturating_add(length as u64);
            if complete {
                if !self.oversized {
                    let line = std::mem::take(&mut self.pending);
                    if !self.row(&line) {
                        *self = Self::default();
                        return None;
                    }
                } else if !self.identified {
                    *self = Self::default();
                    return None;
                }
                self.oversized = false;
            }
        }
        let mut file = reader.into_inner().into_inner();
        self.anchor = anchor(&mut file, self.offset).ok()?;
        if !self.identified {
            return None;
        }
        let mut report = self.report.clone();
        // A scan may catch up on older history after the pre-prompt baseline.
        // Only the bound native turn may introduce new facts during settlement.
        if let Some(turn) = turn {
            report.records.retain(|r| {
                self.request_turns
                    .get(&r.request_id)
                    .is_some_and(|t| t == turn)
            });
        }
        report.truncated |=
            self.offset < metadata.len() || !self.pending.is_empty() || self.oversized;
        Some(Readback {
            exact_turns: report
                .records
                .iter()
                .filter_map(|r| self.request_turns.get(&r.request_id).cloned())
                .collect(),
            report,
            model: turn.and_then(|t| self.models.get(t).cloned()),
            caught_up: self.offset == metadata.len() && self.pending.is_empty() && !self.oversized,
        })
    }
    fn row(&mut self, line: &[u8]) -> bool {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            self.report.truncated = true;
            return self.identified;
        };
        if !self.identified {
            self.identified = row["type"] == "session_meta" && row["payload"]["id"] == self.thread;
            return self.identified;
        }
        if row["type"] == "session_meta" {
            return row["payload"]["id"] == self.thread;
        }
        if row["type"] == "turn_context" {
            if let (Some(turn), Some(model)) = (
                row["payload"]["turn_id"].as_str(),
                row["payload"]["model"].as_str().filter(|s| !s.is_empty()),
            ) {
                if self.models.len() >= MAX_FACTS && !self.models.contains_key(turn) {
                    if let Some(old) = self.models.keys().next().cloned() {
                        self.models.remove(&old);
                    }
                    self.report.truncated = true;
                }
                self.models.insert(turn.into(), model.into());
                for record in &mut self.report.records {
                    if self
                        .request_turns
                        .get(&record.request_id)
                        .is_some_and(|t| t == turn)
                    {
                        record.model = Some(model.into());
                    }
                }
            }
            return true;
        }
        let payload = match row["type"].as_str() {
            Some("token_usage_record") => &row["payload"],
            Some("compacted") => &row["payload"]["latest_token_usage_record"],
            _ => return true,
        };
        if payload["thread_id"] != self.thread
            || payload["session_id"]
                .as_str()
                .is_some_and(|s| s != self.thread)
        {
            return true;
        }
        let Some(id) = payload["response_id"].as_str() else {
            return true;
        };
        let mut normalized = serde_json::Map::new();
        for (native, field) in [
            ("input_tokens", "inputTokens"),
            ("output_tokens", "outputTokens"),
            ("cached_input_tokens", "cachedInputTokens"),
            ("cache_write_input_tokens", "cacheWriteInputTokens"),
            ("reasoning_output_tokens", "reasoningOutputTokens"),
            ("total_tokens", "totalTokens"),
        ] {
            if let Some(value) = payload["usage"].get(native) {
                normalized.insert(field.into(), value.clone());
            }
        }
        if let Some(mut record) =
            super::live_usage::response(&Value::Object(normalized), id, "codex.rollout")
        {
            if self.report.records.len() >= MAX_FACTS
                && !self
                    .report
                    .records
                    .iter()
                    .any(|r| r.request_id == record.request_id)
            {
                let old = self.report.records.remove(0);
                self.request_turns.remove(&old.request_id);
                self.report.truncated = true;
            }
            if let Some(turn) = payload["turn_id"].as_str() {
                record.model = self.models.get(turn).cloned();
                if self.request_turns.len() < MAX_FACTS
                    || self.request_turns.contains_key(&record.request_id)
                {
                    self.request_turns
                        .insert(record.request_id.clone(), turn.into());
                }
            }
            self.report.observe(record);
        }
        true
    }
}
fn anchor(file: &mut File, offset: u64) -> std::io::Result<Vec<u8>> {
    let length = offset.min(64);
    file.seek(SeekFrom::Start(offset - length))?;
    let mut bytes = vec![0; usize::try_from(length).unwrap_or(64)];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    fn meta(id: &str) -> String {
        // Nested `type` precedes the top-level type, as in sorted native metadata.
        json!({"ordinal":0,"payload":{"base_instructions":{"type":"nested"},"id":id},"type":"session_meta"}).to_string() + "\n"
    }
    fn request(id: &str) -> Value {
        json!({"type":"token_usage_record","payload":{"thread_id":"s","session_id":"s","turn_id":"t","response_id":id,"usage":{"input_tokens":100,"output_tokens":10}}})
    }
    fn append(path: &Path, text: &str) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    #[test]
    fn large_tool_output_does_not_block_requests_or_force_full_rereads() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut output = file.reopen().unwrap();
        output.write_all(meta("s").as_bytes()).unwrap();
        writeln!(
            output,
            "{}",
            json!({"type":"turn_context","payload":{"turn_id":"t","model":"actual"}})
        )
        .unwrap();
        output
            .write_all(b"{\"type\":\"response_item\",\"payload\":{\"text\":\"")
            .unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..640 {
            output.write_all(&chunk).unwrap();
        }
        output.write_all(b"\"}}\n").unwrap();
        writeln!(output, "{}", request("r1")).unwrap();
        output.flush().unwrap();
        let mut reader = Reader::default();
        while reader.offset < file.as_file().metadata().unwrap().len() {
            reader.read(file.path(), "s", Some("t"));
        }
        let result = reader.read(file.path(), "s", Some("t")).unwrap();
        assert_eq!(result.report.records.len(), 1);
        assert_eq!(result.report.records[0].model.as_deref(), Some("actual"));
        assert_eq!(reader.offset, file.as_file().metadata().unwrap().len());
        let offset = reader.offset;
        assert_eq!(
            reader.read(file.path(), "s", Some("t")).unwrap().report,
            result.report
        );
        assert_eq!(reader.offset, offset);
        let row = request("r2").to_string();
        let split = row.len() / 2;
        append(file.path(), &row[..split]);
        assert_eq!(
            reader
                .read(file.path(), "s", Some("t"))
                .unwrap()
                .report
                .records
                .len(),
            1
        );
        append(file.path(), &(row[split..].to_owned() + "\n"));
        let result = reader.read(file.path(), "s", Some("t")).unwrap();
        assert_eq!(result.report.records.len(), 2);
        assert_eq!(result.report.records[1].request_id, "response:r2");
        assert_eq!(result.model.as_deref(), Some("actual"));
        assert!(reader.pending.len() <= MAX_LINE);
    }
    #[test]
    fn budgeted_reads_continue_at_partial_rows_and_reconcile_late_model_and_compaction() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let r = request("r");
        std::fs::write(file.path(), meta("s") + &r.to_string() + "\n").unwrap();
        let mut reader = Reader::default();
        while reader.offset < file.as_file().metadata().unwrap().len() {
            reader.read_budget(file.path(), "s", Some("t"), 17);
        }
        let first = reader.read(file.path(), "s", Some("t")).unwrap();
        assert_eq!(first.report.records.len(), 1);
        assert!(first.report.records[0].model.is_none());
        append(
            file.path(),
            &(json!({"type":"turn_context","payload":{"turn_id":"t","model":"confirmed"}})
                .to_string()
                + "\n"),
        );
        append(
            file.path(),
            &(json!({"type":"compacted","payload":{"latest_token_usage_record":r["payload"]}})
                .to_string()
                + "\n"),
        );
        let mut foreign = request("foreign");
        foreign["payload"]["thread_id"] = json!("child");
        append(file.path(), &(foreign.to_string() + "\n"));
        let result = reader.read(file.path(), "s", Some("t")).unwrap();
        assert_eq!(result.report.records.len(), 1);
        assert_eq!(result.report.records[0].model.as_deref(), Some("confirmed"));
        assert_eq!(result.exact_turns, BTreeSet::from(["t".into()]));
    }
    #[test]
    fn truncation_and_replacement_revalidate_identity_and_reset_cached_facts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, meta("s") + &request("old").to_string() + "\n").unwrap();
        let mut reader = Reader::default();
        assert_eq!(
            reader.read(&path, "s", None).unwrap().report.records.len(),
            1
        );
        std::fs::write(&path, meta("s")).unwrap();
        assert!(
            reader
                .read(&path, "s", None)
                .unwrap()
                .report
                .records
                .is_empty()
        );
        let replacement = dir.path().join("new.jsonl");
        std::fs::write(
            &replacement,
            meta("other") + &request("foreign").to_string() + "\n",
        )
        .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(reader.read(&path, "s", None).is_none());
        std::fs::write(&path, meta("s") + &request("new").to_string() + "\n").unwrap();
        assert_eq!(
            reader.read(&path, "s", None).unwrap().report.records[0].request_id,
            "response:new"
        );
    }
    #[test]
    fn bounded_cache_keeps_accepting_new_turns_after_history_limit() {
        let mut reader = Reader {
            thread: "s".into(),
            identified: true,
            ..Reader::default()
        };
        for n in 0..MAX_FACTS {
            let id = format!("response:old{n}");
            reader.report.records.push(driver_api::usage::UsageRequest {
                request_id: id.clone(),
                input_tokens: Some(1),
                ..Default::default()
            });
            reader.request_turns.insert(id, "old".into());
        }
        assert!(reader.row(request("new").to_string().as_bytes()));
        assert_eq!(reader.report.records.len(), MAX_FACTS);
        assert!(!reader.request_turns.contains_key("response:old0"));
        assert_eq!(
            reader.request_turns.get("response:new").map(String::as_str),
            Some("t")
        );
        assert!(reader.report.truncated);
    }
    #[test]
    fn history_catchup_does_not_attribute_older_requests_to_the_bound_turn() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut old = request("old");
        old["payload"]["turn_id"] = json!("previous");
        std::fs::write(
            file.path(),
            meta("s") + &old.to_string() + "\n" + &request("current").to_string() + "\n",
        )
        .unwrap();
        let mut reader = Reader::default();
        reader.read_budget(
            file.path(),
            "s",
            None,
            u64::try_from(meta("s").len()).unwrap(),
        );
        let current = reader.read(file.path(), "s", Some("t")).unwrap();
        assert_eq!(current.report.records.len(), 1);
        assert_eq!(current.report.records[0].request_id, "response:current");
        assert_eq!(current.exact_turns, BTreeSet::from(["t".into()]));
    }
}
