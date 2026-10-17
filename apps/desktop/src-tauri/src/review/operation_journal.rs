//! Durable identities shared by Git and file mutations. No action replay after an interrupted intent.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(crate) trait JournalRequest:
    Clone + PartialEq + Serialize + serde::de::DeserializeOwned
{
    fn task_id(&self) -> &str;
    fn operation_id(&self) -> &str;
}
pub(crate) trait JournalOutcome: Clone + Serialize + serde::de::DeserializeOwned {
    type Output;
    fn running() -> Self;
    fn unknown() -> Self;
    fn is_running(&self) -> bool;
    fn from_result(result: &Result<Self::Output, String>) -> Self;
    fn result(self) -> Result<Self::Output, String>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationReceipt<R, O> {
    pub request: R,
    pub outcome: O,
    #[serde(default)]
    pub closed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalEntry<R, O> {
    pub operation_id: String,
    pub receipt: Option<OperationReceipt<R, O>>,
    pub error: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalPage<R, O> {
    pub entries: Vec<JournalEntry<R, O>>,
    pub next_cursor: Option<String>,
}
pub(crate) struct OperationJournal<R, O> {
    directory: PathBuf,
    gate: Mutex<()>,
    active: Arc<Mutex<BTreeSet<String>>>,
    uncertain: Mutex<BTreeSet<String>>,
    record: std::marker::PhantomData<(R, O)>,
    changes: Option<Arc<crate::state_changes::Changes>>,
}
struct Active(
    Arc<Mutex<BTreeSet<String>>>,
    String,
    Option<Arc<crate::state_changes::Changes>>,
);
impl Drop for Active {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.lock() {
            active.remove(&self.1);
        }
        if let Some(changes) = &self.2 {
            changes.changed();
        }
    }
}
impl<R: JournalRequest, O: JournalOutcome> OperationJournal<R, O> {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            gate: Mutex::new(()),
            active: Arc::default(),
            uncertain: Mutex::default(),
            record: std::marker::PhantomData,
            changes: None,
        }
    }
    pub fn with_changes(mut self, changes: Arc<crate::state_changes::Changes>) -> Self {
        self.changes = Some(changes);
        self
    }
    pub(super) fn file(&self, task: &str, operation: &str) -> Result<PathBuf, String> {
        agent_domain::TaskId::parse(task).map_err(|_| "无效的会话标识")?;
        let id = uuid::Uuid::parse_str(operation).map_err(|_| "无效的操作标识")?;
        if id.to_string() != operation {
            return Err("无效的操作标识".into());
        }
        Ok(self.directory.join(task).join(format!("{operation}.json")))
    }
    fn read(&self, task: &str, operation: &str) -> Result<Option<OperationReceipt<R, O>>, String> {
        self.read_with_limit(task, operation, None)
    }
    fn read_with_limit(
        &self,
        task: &str,
        operation: &str,
        limit: Option<usize>,
    ) -> Result<Option<OperationReceipt<R, O>>, String> {
        let file = self.file(task, operation)?;
        let file = match std::fs::File::open(file) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法读取操作记录：{error}")),
        };
        let mut bytes = Vec::new();
        file.take(limit.map_or(u64::MAX, |limit| limit as u64 + 1))
            .read_to_end(&mut bytes)
            .map_err(|error| format!("无法读取操作记录：{error}"))?;
        if limit.is_some_and(|limit| bytes.len() > limit) {
            return Err("记录较大，请单独查看详情".into());
        }
        let receipt: OperationReceipt<R, O> =
            serde_json::from_slice(&bytes).map_err(|_| "操作记录损坏，不能重发此操作")?;
        if receipt.request.task_id() != task || receipt.request.operation_id() != operation {
            return Err("操作记录归属不匹配".into());
        }
        Ok(Some(receipt))
    }
    fn persist(&self, receipt: &OperationReceipt<R, O>) -> Result<(), String> {
        let path = self.file(receipt.request.task_id(), receipt.request.operation_id())?;
        let directory = path.parent().ok_or("操作记录目录无效")?;
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(receipt).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
        file.persist(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            // Also persist newly created task/journal directory entries.
            for ancestor in directory.ancestors() {
                std::fs::File::open(ancestor)
                    .and_then(|file| file.sync_all())
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some(changes) = &self.changes {
            changes.changed();
        }
        Ok(())
    }
    fn visible(
        &self,
        mut receipt: OperationReceipt<R, O>,
    ) -> Result<OperationReceipt<R, O>, String> {
        let id = format!(
            "{}/{}",
            receipt.request.task_id(),
            receipt.request.operation_id()
        );
        if self
            .uncertain
            .lock()
            .map_err(|_| "操作记录不可用")?
            .contains(&id)
            || (receipt.outcome.is_running()
                && !self
                    .active
                    .lock()
                    .map_err(|_| "操作记录不可用")?
                    .contains(&id))
        {
            receipt.outcome = O::unknown();
        }
        Ok(receipt)
    }
    pub fn get(
        &self,
        task: &str,
        operation: &str,
    ) -> Result<Option<OperationReceipt<R, O>>, String> {
        let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
        self.read(task, operation)?
            .map(|receipt| self.visible(receipt))
            .transpose()
    }
    /// Idempotent tracking closure. It never changes the recorded action result.
    pub fn close(&self, task: &str, operation: &str) -> Result<OperationReceipt<R, O>, String> {
        let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
        let mut receipt = self
            .read(task, operation)?
            .ok_or("原操作记录不存在，请先核实")?;
        if !receipt.closed {
            receipt.closed = true;
            self.persist(&receipt)?;
        }
        self.visible(receipt)
    }
    /// Bounded keyset pagination. Scan names without retaining the full directory;
    /// mutations do not need to wait for the directory scan. Each selected receipt
    /// is read under the short journal gate, preserving current outcome semantics.
    pub fn page(
        &self,
        task: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<JournalPage<R, O>, String> {
        agent_domain::TaskId::parse(task).map_err(|_| "无效的会话标识")?;
        if !(1..=50).contains(&limit) {
            return Err("每页操作记录数量须为 1 至 50".into());
        }
        if let Some(cursor) = after {
            self.file(task, cursor)?;
        }
        let directory = match std::fs::read_dir(self.directory.join(task)) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(JournalPage {
                    entries: Vec::new(),
                    next_cursor: None,
                });
            }
            Err(error) => return Err(format!("无法读取操作记录目录：{error}")),
        };
        let mut ids = BTreeSet::new();
        for entry in directory {
            let entry = entry.map_err(|error| format!("无法枚举操作记录：{error}"))?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            // Temporary files are not published operation records. Invalid JSON
            // record names indicate storage damage rather than an empty history.
            self.file(task, id)
                .map_err(|_| "操作记录文件名损坏，请检查后台存储")?;
            if after.is_some_and(|cursor| id <= cursor) {
                continue;
            }
            ids.insert(id.to_owned());
            if ids.len() > limit + 1 {
                ids.pop_last();
            }
        }
        let more = ids.len() > limit;
        if more {
            ids.pop_last();
        }
        let next_cursor = if more { ids.last().cloned() } else { None };
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let result = (|| {
                let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
                let path = self.file(task, &id)?;
                let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                if !metadata.is_file() {
                    return Err("操作记录不是普通文件".into());
                }
                // A single full receipt remains queryable by ID. Listing does not
                // allocate unbounded payloads or silently drop oversized entries.
                if metadata.len() > 256 * 1024 {
                    return Err("记录较大，请单独查看详情".into());
                }
                self.read_with_limit(task, &id, Some(256 * 1024))?
                    .ok_or_else(|| "记录已改变，请刷新".to_owned())
                    .and_then(|receipt| self.visible(receipt))
            })();
            entries.push(match result {
                Ok(receipt) => JournalEntry {
                    operation_id: id,
                    receipt: Some(receipt),
                    error: None,
                },
                Err(error) => JournalEntry {
                    operation_id: id,
                    receipt: None,
                    error: Some(error),
                },
            });
        }
        Ok(JournalPage {
            entries,
            next_cursor,
        })
    }
    pub async fn execute<F, Fut>(&self, request: R, action: F) -> Result<O::Output, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<O::Output, String>>,
    {
        let mut receipt = OperationReceipt {
            request,
            outcome: O::running(),
            closed: false,
        };
        let active = {
            let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
            if let Some(previous) =
                self.read(receipt.request.task_id(), receipt.request.operation_id())?
            {
                if previous.request != receipt.request {
                    return Err("操作标识已用于不同请求，请核实原操作".into());
                }
                return self.visible(previous)?.outcome.result();
            }
            // Never start an action if the durable intent cannot be confirmed.
            if let Err(error) = self.persist(&receipt) {
                return Err(format!("保存操作记录失败，未启动操作：{error}"));
            }
            let id = format!(
                "{}/{}",
                receipt.request.task_id(),
                receipt.request.operation_id()
            );
            self.active
                .lock()
                .map_err(|_| "操作记录不可用")?
                .insert(id.clone());
            Active(self.active.clone(), id, self.changes.clone())
        };
        let result = action().await;
        receipt.outcome = O::from_result(&result);
        {
            let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
            if let Err(error) = self.persist(&receipt) {
                self.uncertain
                    .lock()
                    .map_err(|_| "操作记录不可用")?
                    .insert(format!(
                        "{}/{}",
                        receipt.request.task_id(),
                        receipt.request.operation_id()
                    ));
                return Err(format!(
                    "操作已返回，但保存结果失败；请核实仓库，未重发操作：{error}"
                ));
            }
        }
        drop(active);
        result
    }
    pub fn forget_task(&self, task: &str) -> Result<(), String> {
        agent_domain::TaskId::parse(task).map_err(|_| "无效的会话标识")?;
        let _gate = self.gate.lock().map_err(|_| "操作记录不可用")?;
        match std::fs::remove_dir_all(self.directory.join(task)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("清理操作记录失败：{error}")),
        }
    }
}
