//! Daemon-owned original send records. The existing Core event log decides closure.
pub(crate) mod watch;
use agent_core::{
    Core,
    submission_intents::{
        MAX_INTENT_BYTES, SubmissionStage, fingerprint, normalize, stage_command,
        submission_command,
    },
};
use agent_domain::{CommandId, TaskId};
use agent_protocol::submission::{
    SubmissionIntent, SubmissionIntentCall, SubmissionIntentEntry, SubmissionIntentPage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    schema_version: u8,
    operation_id: CommandId,
    entry: SubmissionIntentEntry,
}
struct Store {
    root: PathBuf,
    io_gate: std::sync::Mutex<()>,
    changes: crate::state_changes::Changes,
}
pub struct SubmissionIntents {
    store: Arc<Store>,
    gate: tokio::sync::Mutex<()>,
}
fn canonical(id: &CommandId) -> Result<(), String> {
    if id.as_str().len() != 30
        || !id.as_str()[4..]
            .bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
    {
        return Err("发送操作标识不是规范格式".into());
    }
    Ok(())
}
fn regular(path: &Path, directory: bool) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if (directory && m.is_dir()) || (!directory && m.is_file()) => Ok(true),
        Ok(_) => Err("发送记录路径异常，未覆盖或重放操作".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}
impl Store {
    fn path(&self, id: &CommandId) -> Result<PathBuf, String> {
        canonical(id)?;
        Ok(self.root.join(format!("{id}.json")))
    }
    fn read(&self, id: &CommandId) -> Result<Option<SubmissionIntentEntry>, String> {
        let path = self.path(id)?;
        if !regular(&self.root, true)? || !regular(&path, false)? {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take((MAX_INTENT_BYTES + 4097) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_INTENT_BYTES + 4096 {
            return Err("发送记录过大，未继续操作".into());
        }
        let stored: Stored =
            serde_json::from_slice(&bytes).map_err(|_| "发送记录损坏，请保留记录并核实")?;
        if stored.schema_version != 1 || stored.operation_id != *id {
            return Err("发送记录身份不一致".into());
        }
        match &stored.entry {
            SubmissionIntentEntry::Pending { intent } => {
                if intent.operation_id != *id
                    || normalize(*intent.clone()).map_err(|e| e.to_string())? != **intent
                {
                    return Err("发送记录内容不一致".into());
                }
            }
            SubmissionIntentEntry::Closed {
                fingerprint,
                resolution,
            } => {
                if matches!(
                    resolution,
                    agent_protocol::submission::SubmissionResolution::Submitted { sequence: 0, .. }
                        | agent_protocol::submission::SubmissionResolution::TaskDeleted {
                            deletion_sequence: 0,
                            ..
                        }
                ) || fingerprint.len() != 64
                    || !fingerprint
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err("发送结束记录损坏".into());
                }
            }
        }
        Ok(Some(stored.entry))
    }
    fn scan(&self) -> Result<(Vec<CommandId>, bool), String> {
        if !regular(&self.root, true)? {
            return Ok((vec![], false));
        }
        let mut ids = vec![];
        let mut incomplete = false;
        for item in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let Ok(item) = item else {
                incomplete = true;
                continue;
            };
            let Ok(name) = item.file_name().into_string() else {
                incomplete = true;
                continue;
            };
            // An interrupted unpublished temp file is not an accepted intent.
            if name.starts_with(".pending-")
                && Path::new(&name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            {
                continue;
            }
            let id = name
                .strip_suffix(".json")
                .and_then(|name| CommandId::parse(name).ok())
                .filter(|id| canonical(id).is_ok());
            if let Some(id) = id {
                ids.push(id);
            } else {
                incomplete = true;
            }
        }
        ids.sort();
        Ok((ids, incomplete))
    }
    fn ids(&self) -> Result<Vec<CommandId>, String> {
        let (ids, incomplete) = self.scan()?;
        if incomplete {
            return Err("发送记录目录含无法识别的条目，请保留记录并核实".into());
        }
        Ok(ids)
    }
    fn write(&self, id: &CommandId, entry: SubmissionIntentEntry) -> Result<(), String> {
        let path = self.path(id)?;
        if !regular(&self.root, true)? {
            std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        }
        regular(&path, false)?;
        let mut file = tempfile::Builder::new()
            .prefix(".pending-")
            .suffix(".tmp")
            .tempfile_in(&self.root)
            .map_err(|e| e.to_string())?;
        let stored = Stored {
            schema_version: 1,
            operation_id: id.clone(),
            entry,
        };
        file.write_all(&serde_json::to_vec(&stored).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
        file.persist(&path).map_err(|e| e.to_string())?;
        // Publication is observable even if a later directory sync fails. This
        // hint carries no outcome; readers must verify the durable record.
        self.changes.changed();
        #[cfg(unix)]
        for directory in [Some(self.root.as_path()), self.root.parent()]
            .into_iter()
            .flatten()
        {
            std::fs::File::open(directory)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn close_deleted(
        &self,
        id: &CommandId,
        resolution: agent_protocol::submission::SubmissionResolution,
    ) -> Result<(), String> {
        let Some(entry) = self.read(id)? else {
            return Ok(());
        };
        let fingerprint = match entry {
            SubmissionIntentEntry::Pending { intent } => {
                fingerprint(&intent).map_err(|e| e.to_string())?
            }
            SubmissionIntentEntry::Closed {
                fingerprint,
                resolution: old,
            } => {
                if old == resolution {
                    return Ok(());
                }
                fingerprint
            }
        };
        self.write(
            id,
            SubmissionIntentEntry::Closed {
                fingerprint,
                resolution,
            },
        )
    }
    fn matching(&self, intent: &SubmissionIntent) -> Result<Option<SubmissionIntentEntry>, String> {
        let found = self.read(&intent.operation_id)?;
        if let Some(entry) = &found {
            let expected = fingerprint(intent).map_err(|e| e.to_string())?;
            let actual = match entry {
                SubmissionIntentEntry::Pending { intent } => {
                    fingerprint(intent).map_err(|e| e.to_string())?
                }
                SubmissionIntentEntry::Closed { fingerprint, .. } => fingerprint.clone(),
            };
            if let SubmissionIntentEntry::Closed { resolution, .. } = entry {
                let task_id = match resolution {
                    agent_protocol::submission::SubmissionResolution::Submitted {
                        task_id, ..
                    }
                    | agent_protocol::submission::SubmissionResolution::TaskDeleted {
                        task_id,
                        ..
                    }
                    | agent_protocol::submission::SubmissionResolution::Invalidated { task_id } => {
                        task_id
                    }
                };
                if *task_id != intent.task.task_id {
                    return Err("发送结束记录的会话不一致".into());
                }
            }
            if actual != expected {
                return Err("原发送标识已绑定其他内容，未继续操作".into());
            }
        }
        Ok(found)
    }
    fn put(&self, intent: SubmissionIntent) -> Result<SubmissionIntentEntry, String> {
        if let Some(entry) = self.matching(&intent)? {
            return Ok(entry);
        }
        for id in self.ids()? {
            let entry = self.read(&id)?.ok_or("发送记录读取中断，请重新核实")?;
            if let SubmissionIntentEntry::Pending { intent: pending } = entry
                && (pending.task.task_id == intent.task.task_id || pending.scope == intent.scope)
            {
                return Err("此会话或草稿已有待核实发送，请先处理原操作".into());
            }
        }
        let id = intent.operation_id.clone();
        let entry = SubmissionIntentEntry::Pending {
            intent: Box::new(intent),
        };
        self.write(&id, entry.clone())?;
        Ok(entry)
    }
    fn list(&self, after: Option<CommandId>, limit: usize) -> Result<SubmissionIntentPage, String> {
        if !(1..=100).contains(&limit) {
            return Err("发送记录分页大小应为 1 至 100".into());
        }
        if let Some(id) = &after {
            canonical(id)?;
        }
        let mut pending = vec![];
        let (ids, unrecognized_entries) = self.scan()?;
        for id in ids {
            if after.as_ref().is_some_and(|after| &id <= after) {
                continue;
            }
            // Keep unreadable identities discoverable: Get reports their error,
            // while other entries and pages can still be recovered.
            if matches!(
                self.read(&id),
                Ok(Some(SubmissionIntentEntry::Pending { .. })) | Err(_)
            ) {
                pending.push(id);
                if pending.len() > limit {
                    break;
                }
            }
        }
        let next_cursor = if pending.len() > limit {
            pending.pop();
            pending.last().cloned()
        } else {
            None
        };
        Ok(SubmissionIntentPage {
            operation_ids: pending,
            next_cursor,
            unrecognized_entries,
        })
    }
}
impl SubmissionIntents {
    pub fn new(root: PathBuf) -> Self {
        Self {
            store: Arc::new(Store {
                root,
                io_gate: std::sync::Mutex::default(),
                changes: crate::state_changes::Changes::new(),
            }),
            gate: tokio::sync::Mutex::default(),
        }
    }
    pub(crate) fn subscribe(&self) -> Result<crate::state_changes::Subscription, String> {
        self.store.changes.subscribe()
    }
    #[cfg(test)]
    pub(crate) fn active_subscriptions(&self) -> usize {
        self.store.changes.active_count()
    }
    async fn io<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Store) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            // Cancellation of the async caller must not release serialization
            // while a blocking atomic publication is still in progress.
            let _guard = store
                .io_gate
                .lock()
                .map_err(|_| "发送记录存储不可用".to_string())?;
            work(&store)
        })
        .await
        .map_err(|_| "发送记录后台处理失败".to_string())?
    }
    pub async fn reconcile_deleted(
        &self,
        core: &Core,
        task: Option<&TaskId>,
    ) -> Result<(), String> {
        let _guard = self.gate.lock().await;
        let target = task.cloned();
        let (groups, mut incomplete) = self
            .io(move |store| {
                let mut groups = std::collections::BTreeMap::<TaskId, Vec<CommandId>>::new();
                let (ids, mut incomplete) = store.scan()?;
                for id in ids {
                    let entry = match store.read(&id) {
                        Ok(Some(entry)) => entry,
                        Ok(None) => continue,
                        Err(_) => {
                            incomplete = true;
                            continue;
                        }
                    };
                    let task = match entry {
                        SubmissionIntentEntry::Pending { intent } => intent.task.task_id,
                        SubmissionIntentEntry::Closed { resolution, .. } => match resolution {
                            agent_protocol::submission::SubmissionResolution::Submitted {
                                task_id,
                                ..
                            }
                            | agent_protocol::submission::SubmissionResolution::Invalidated {
                                task_id,
                            } => task_id,
                            agent_protocol::submission::SubmissionResolution::TaskDeleted {
                                ..
                            } => continue,
                        },
                    };
                    if target.as_ref().is_none_or(|target| target == &task) {
                        groups.entry(task).or_default().push(id);
                    }
                }
                Ok((groups, incomplete))
            })
            .await?;
        for (task, ids) in groups {
            let resolution = match core.submission_task_deletion(&task).await {
                Ok(Some(resolution)) => resolution,
                Ok(None) => continue,
                Err(_) => {
                    incomplete = true;
                    continue;
                }
            };
            match self
                .io(move |store| {
                    let mut failed = false;
                    for id in ids {
                        // A failed publication must not prevent independent records
                        // with verified tombstones from losing duplicate prompt data.
                        failed |= store.close_deleted(&id, resolution.clone()).is_err();
                    }
                    Ok(failed)
                })
                .await
            {
                Ok(failed) => incomplete |= failed,
                Err(_) => incomplete = true,
            }
        }
        if incomplete {
            Err(
                "部分发送记录无法核实或清理；可核实的记录已处理，请保留异常记录并检查本机存储"
                    .into(),
            )
        } else {
            Ok(())
        }
    }
    pub async fn call(
        &self,
        core: &Arc<Core>,
        call: SubmissionIntentCall,
    ) -> Result<Value, String> {
        // One daemon owns the journal. No per-project writer lock or UI ownership.
        let _guard = self.gate.lock().await;
        match call {
            SubmissionIntentCall::Start {
                operation_id,
                scope,
                task_id,
                project_id,
                target,
                input,
            } => {
                let found = {
                    let id = operation_id.clone();
                    self.io(move |store| store.read(&id)).await?
                };
                let intent = match found {
                    Some(entry @ SubmissionIntentEntry::Closed { .. }) => return Ok(json!(entry)),
                    Some(SubmissionIntentEntry::Pending { intent }) => {
                        // A retried start after a lost reply must carry the same request.
                        if intent.scope != scope
                            || intent.task.project_id != project_id
                            || intent.target != target
                            || intent.input != *input
                            || task_id
                                .as_ref()
                                .is_some_and(|task| task != &intent.task.task_id)
                        {
                            return Err("原发送标识已绑定其他内容，未继续操作".into());
                        }
                        *intent
                    }
                    None => {
                        let intent = freeze(
                            core,
                            operation_id,
                            scope,
                            task_id,
                            project_id,
                            target,
                            *input,
                        )
                        .await?;
                        if let Some(resolution) = core
                            .submission_task_deletion(&intent.task.task_id)
                            .await
                            .map_err(|e| e.to_string())?
                        {
                            return Ok(json!(self.close(&intent, resolution).await?));
                        }
                        let original = intent.clone();
                        match self.io(move |store| store.put(original)).await? {
                            SubmissionIntentEntry::Pending { intent } => *intent,
                            closed @ SubmissionIntentEntry::Closed { .. } => {
                                return Ok(json!(closed));
                            }
                        }
                    }
                };
                Ok(json!(self.drive(core, &intent).await?))
            }
            SubmissionIntentCall::Continue { operation_id } => {
                match self.io(move |store| store.read(&operation_id)).await? {
                    None => Err("原发送记录不存在，请刷新后核实".into()),
                    Some(SubmissionIntentEntry::Pending { intent }) => {
                        Ok(json!(self.drive(core, &intent).await?))
                    }
                    Some(closed) => Ok(json!(closed)),
                }
            }
            SubmissionIntentCall::Retire { operation_id } => {
                match self.io(move |store| store.read(&operation_id)).await? {
                    None => Err("原发送记录不存在，请刷新后核实".into()),
                    Some(SubmissionIntentEntry::Pending { intent }) => {
                        Ok(json!(self.retire(core, &intent).await?))
                    }
                    Some(closed) => Ok(json!(closed)),
                }
            }
            SubmissionIntentCall::Resolve { intent } => {
                let intent = normalize(*intent).map_err(|e| e.to_string())?;
                let original = intent.clone();
                let found = self.io(move |store| store.matching(&original)).await?;
                if let Some(deleted) = core
                    .submission_task_deletion(&intent.task.task_id)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    return Ok(json!(deleted));
                }
                if let Some(SubmissionIntentEntry::Closed { resolution, .. }) = found {
                    return Ok(json!(resolution));
                }
                Ok(json!(
                    core.submission_intent_resolution(&intent)
                        .await
                        .map_err(|e| e.to_string())?
                ))
            }
            SubmissionIntentCall::Put { intent } => {
                let intent = normalize(*intent).map_err(|e| e.to_string())?;
                let original = intent.clone();
                self.io(move |store| store.matching(&original)).await?;
                if let Some(resolution) = core
                    .submission_task_deletion(&intent.task.task_id)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    let entry = SubmissionIntentEntry::Closed {
                        fingerprint: fingerprint(&intent).map_err(|e| e.to_string())?,
                        resolution,
                    };
                    let result = entry.clone();
                    self.io(move |store| store.write(&intent.operation_id, entry))
                        .await?;
                    return Ok(json!(result));
                }
                Ok(json!(self.io(move |store| store.put(intent)).await?))
            }
            SubmissionIntentCall::Get {
                operation_id,
                expected,
            } => {
                if let Some(expected) = expected {
                    let expected = normalize(*expected).map_err(|e| e.to_string())?;
                    if expected.operation_id != operation_id {
                        return Err("查询与原发送身份不一致".into());
                    }
                    Ok(json!(
                        self.io(move |store| store.matching(&expected)).await?
                    ))
                } else {
                    Ok(json!(
                        self.io(move |store| store.read(&operation_id)).await?
                    ))
                }
            }
            SubmissionIntentCall::List { after, limit } => {
                Ok(json!(self.io(move |store| store.list(after, limit)).await?))
            }
            SubmissionIntentCall::Close { intent } => {
                let intent = normalize(*intent).map_err(|e| e.to_string())?;
                let original = intent.clone();
                let found = self.io(move |store| store.matching(&original)).await?;
                if let Some(found @ SubmissionIntentEntry::Closed { .. }) = &found
                    && core
                        .submission_task_deletion(&intent.task.task_id)
                        .await
                        .map_err(|e| e.to_string())?
                        .is_none()
                {
                    return Ok(json!(found));
                }
                // A legacy or competing local intent can close without first
                // claiming the task's pending slot, but only with Core evidence.
                let resolution = core
                    .submission_intent_resolution(&intent)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("尚未核实原消息已受理或已作废，恢复记录仍保留")?;
                let entry = SubmissionIntentEntry::Closed {
                    fingerprint: fingerprint(&intent).map_err(|e| e.to_string())?,
                    resolution,
                };
                let result = entry.clone();
                self.io(move |store| store.write(&intent.operation_id, entry))
                    .await?;
                Ok(json!(result))
            }
        }
    }
}

/// Builds the immutable intent from the service's own view of the task, so no
/// client has to derive versions, context packages or creation identities.
async fn freeze(
    core: &Arc<Core>,
    operation_id: CommandId,
    scope: String,
    task_id: Option<TaskId>,
    project_id: agent_domain::ProjectId,
    target: agent_domain::ExecutorRef,
    input: agent_protocol::submission::SubmissionInput,
) -> Result<SubmissionIntent, String> {
    canonical(&operation_id)?;
    let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
    if !snapshot.projects.iter().any(|p| p.project_id == project_id) {
        return Err("原项目已不可用。".into());
    }
    let existing = match &task_id {
        Some(id) => {
            let task = snapshot
                .tasks
                .iter()
                .find(|task| &task.task_id == id && task.project_id == project_id)
                .ok_or("原会话不可发送，请刷新后重试。")?;
            if task.active_primary_run.is_some() {
                return Err("原会话不可发送，请等待回复结束或刷新后重试。".into());
            }
            Some(task.clone())
        }
        None => None,
    };
    let context = match &existing {
        Some(task) if task.preferred_executor != target => {
            let request = crate::review_repairs::context_request(&snapshot, task, &target);
            let built = core
                .build_context_preview(request.clone())
                .await
                .map_err(|e| e.to_string())?;
            Some(agent_protocol::submission::SubmissionContext {
                operation_id: CommandId::new(),
                request,
                hash: built.payload_sha256,
            })
        }
        _ => None,
    };
    let task = existing
        .clone()
        .unwrap_or_else(|| agent_protocol::TaskSummary {
            updated_at: None,
            task_id: TaskId::new(),
            project_id,
            title: "新对话".into(),
            preferred_executor: target.clone(),
            last_turn_executor: None,
            active_primary_run: None,
            stream_version: 0,
        });
    let intent = SubmissionIntent {
        schema_version: 1,
        operation_id,
        scope,
        requested_at: chrono::Utc::now().to_rfc3339(),
        submission: (existing.is_some() && context.is_none()).then(|| task.clone()),
        creation: existing
            .is_none()
            .then(|| agent_protocol::submission::SubmissionCreation {
                operation_id: CommandId::new(),
            }),
        task,
        input,
        target,
        context,
    };
    normalize(intent).map_err(|e| e.to_string())
}

impl SubmissionIntents {
    async fn close(
        &self,
        intent: &SubmissionIntent,
        resolution: agent_protocol::submission::SubmissionResolution,
    ) -> Result<SubmissionIntentEntry, String> {
        let entry = SubmissionIntentEntry::Closed {
            fingerprint: fingerprint(intent).map_err(|e| e.to_string())?,
            resolution,
        };
        let (id, written) = (intent.operation_id.clone(), entry.clone());
        self.io(move |store| store.write(&id, written)).await?;
        Ok(entry)
    }
    async fn resolved(
        &self,
        core: &Arc<Core>,
        intent: &SubmissionIntent,
    ) -> Result<Option<SubmissionIntentEntry>, String> {
        match core
            .submission_intent_resolution(intent)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(resolution) => Ok(Some(self.close(intent, resolution).await?)),
            None => Ok(None),
        }
    }
    async fn stage(core: &Arc<Core>, intent: &SubmissionIntent) -> Result<SubmissionStage, String> {
        core.submission_stage(intent)
            .await
            .map_err(|e| e.to_string())
    }
    /// Executes only the original commands with their frozen identities. A failure
    /// leaves the intent pending for an explicit continue or retire.
    async fn drive(
        &self,
        core: &Arc<Core>,
        intent: &SubmissionIntent,
    ) -> Result<SubmissionIntentEntry, String> {
        if let Some(entry) = self.resolved(core, intent).await? {
            return Ok(entry);
        }
        if Self::stage(core, intent).await? == SubmissionStage::Unresolved {
            let command = stage_command(intent).ok_or("缺少原发送阶段。")?;
            if let Err(error) = core.execute(command).await {
                // A lost reply may follow acceptance; only the original receipt decides.
                if Self::stage(core, intent).await? == SubmissionStage::Unresolved {
                    return Err(error.to_string());
                }
            }
        }
        let SubmissionStage::Ready { version } = Self::stage(core, intent).await? else {
            return Ok(self
                .resolved(core, intent)
                .await?
                .ok_or("原操作结果尚未确认，请先核实。")?);
        };
        let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
        let current = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == intent.task.task_id);
        if !current.is_some_and(|task| {
            task.active_primary_run.is_none()
                && task.project_id == intent.task.project_id
                && task.stream_version == version
                && task.preferred_executor == intent.target
        }) {
            return Err("会话已变化，未用新版本重发旧消息；请核实或放弃原发送。".into());
        }
        let failure = core
            .execute(submission_command(intent, version))
            .await
            .err();
        match self.resolved(core, intent).await? {
            Some(entry) => Ok(entry),
            None => Err(failure.map_or_else(
                || "发送结果尚未确认，请先核实原请求。".to_string(),
                |error| error.to_string(),
            )),
        }
    }
    /// Fences the original stage and message so neither can be accepted later.
    async fn retire(
        &self,
        core: &Arc<Core>,
        intent: &SubmissionIntent,
    ) -> Result<SubmissionIntentEntry, String> {
        if let Some(entry) = self.resolved(core, intent).await? {
            return Ok(entry);
        }
        if let Some(command) = stage_command(intent) {
            core.invalidate_pending_command(&command)
                .await
                .map_err(|e| e.to_string())?;
        }
        if let SubmissionStage::Ready { version } = Self::stage(core, intent).await? {
            core.invalidate_pending_command(&submission_command(intent, version))
                .await
                .map_err(|e| e.to_string())?;
        }
        self.resolved(core, intent)
            .await?
            .ok_or_else(|| "无法确认原发送是否已停止受理。".into())
    }
}

#[tauri::command]
pub async fn core_submission_intents(
    state: tauri::State<'_, crate::DesktopState>,
    call: SubmissionIntentCall,
) -> Result<Value, String> {
    state.core.submission_intents(call).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        CoreRepository,
        submission_intents::{stage_command, submission_command},
    };
    use agent_domain::{ExecutorRef, ProjectId, ProviderId, TaskId};
    use agent_protocol::{
        CommandEnvelope, CoreCommand, RegisterProject, TaskSummary,
        submission::{SubmissionCreation, SubmissionInput},
    };
    use agent_store::SqliteStore;
    async fn fixture() -> (
        tempfile::TempDir,
        SubmissionIntents,
        Arc<Core>,
        Arc<SqliteStore>,
        SubmissionIntent,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteStore::open(dir.path().join("state.db")).unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let project_id = ProjectId::new();
        core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "fixture".into(),
                path: dir.path().to_string_lossy().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
        // New sends are admitted only for a Ready installation, as in the app.
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                provider_id: "fake".into(),
                executable: "fixture:fake".into(),
                fingerprint: "deterministic-test-fixture".into(),
                status: agent_protocol::InstallationStatus::Ready,
                version: None,
                version_text: None,
                diagnostic: None,
                elapsed_millis: 0,
            },
        ));
        probe.actor = agent_protocol::ActorRef::SystemRecovery;
        core.execute(probe).await.unwrap();
        let installation = core
            .bootstrap()
            .await
            .unwrap()
            .installations
            .into_iter()
            .find(|i| i.provider_id == "fake")
            .unwrap();
        let target = ExecutorRef {
            provider_id: ProviderId::parse("fake").unwrap(),
            installation_id: Some(installation.installation_id.to_string()),
        };
        core.register_driver(Arc::new(driver_api::FakeDriver::with_executor(
            target.clone(),
        )))
        .await;
        let intent = SubmissionIntent {
            schema_version: 1,
            operation_id: CommandId::new(),
            scope: "draft:original".into(),
            requested_at: "2026-10-16T10:00:00.000Z".into(),
            task: TaskSummary {
                updated_at: None,
                task_id: TaskId::new(),
                project_id,
                title: "新对话".into(),
                preferred_executor: target.clone(),
                last_turn_executor: None,
                active_primary_run: None,
                stream_version: 0,
            },
            input: SubmissionInput {
                prompt: "private original message".into(),
                model: None,
                thinking: None,
                permission_mode: None,
                attachments: vec![],
            },
            target,
            creation: Some(SubmissionCreation {
                operation_id: CommandId::new(),
            }),
            context: None,
            submission: None,
        };
        let host = SubmissionIntents::new(dir.path().join("submission-intents"));
        (dir, host, core, store, intent)
    }
    fn put(intent: &SubmissionIntent) -> SubmissionIntentCall {
        SubmissionIntentCall::Put {
            intent: Box::new(intent.clone()),
        }
    }
    fn close(intent: &SubmissionIntent) -> SubmissionIntentCall {
        SubmissionIntentCall::Close {
            intent: Box::new(intent.clone()),
        }
    }
    fn get(intent: &SubmissionIntent) -> SubmissionIntentCall {
        SubmissionIntentCall::Get {
            operation_id: intent.operation_id.clone(),
            expected: Some(Box::new(intent.clone())),
        }
    }
    #[tokio::test]
    async fn absent_queries_and_unproven_closure_are_read_only() {
        let (dir, host, core, store, intent) = fixture().await;
        let head = store.event_head().await.unwrap();
        assert!(host.call(&core, get(&intent)).await.unwrap().is_null());
        assert_eq!(
            host.call(
                &core,
                SubmissionIntentCall::List {
                    after: None,
                    limit: 50
                }
            )
            .await
            .unwrap()["operationIds"],
            json!([])
        );
        assert!(host.call(&core, close(&intent)).await.is_err());
        assert!(!dir.path().join("submission-intents").exists());
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn intent_survives_restart_and_binds_original_content_without_executing() {
        let (dir, host, core, store, intent) = fixture().await;
        let head = store.event_head().await.unwrap();
        let registered = host.call(&core, put(&intent)).await.unwrap();
        drop(host);
        let reopened = SubmissionIntents::new(dir.path().join("submission-intents"));
        assert_eq!(
            reopened.call(&core, get(&intent)).await.unwrap(),
            registered
        );
        assert_eq!(
            reopened.call(&core, put(&intent)).await.unwrap(),
            registered
        );
        let mut changed = intent.clone();
        changed.input.prompt.push('!');
        assert!(reopened.call(&core, put(&changed)).await.is_err());
        assert!(reopened.call(&core, get(&changed)).await.is_err());
        assert!(reopened.call(&core, close(&changed)).await.is_err());
        assert_eq!(store.event_head().await.unwrap(), head);
        assert!(core.bootstrap().await.unwrap().tasks.is_empty());
    }
    #[tokio::test]
    async fn competing_intents_for_same_task_or_scope_do_not_both_register() {
        let (_dir, host, core, _store, intent) = fixture().await;
        let mut other = intent.clone();
        other.operation_id = CommandId::new();
        other.creation.as_mut().unwrap().operation_id = CommandId::new();
        let (a, b) = tokio::join!(
            host.call(&core, put(&intent)),
            host.call(&core, put(&other))
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let mut third = intent.clone();
        third.operation_id = CommandId::new();
        third.task.task_id = TaskId::new();
        assert!(host.call(&core, put(&third)).await.is_err());
        third.scope = "another draft".into();
        third.task.task_id = intent.task.task_id.clone();
        assert!(host.call(&core, put(&third)).await.is_err());
    }
    #[tokio::test]
    async fn closure_requires_evidence_and_late_put_cannot_resurrect_original() {
        let (dir, host, core, store, intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        let head = store.event_head().await.unwrap();
        assert!(host.call(&core, close(&intent)).await.is_err());
        assert_eq!(store.event_head().await.unwrap(), head);
        core.invalidate_pending_command(&stage_command(&intent).unwrap())
            .await
            .unwrap();
        let result = host.call(&core, close(&intent)).await.unwrap();
        assert_eq!(result["resolution"]["status"], "invalidated");
        assert_eq!(host.call(&core, put(&intent)).await.unwrap(), result);
        let bytes = std::fs::read(
            dir.path()
                .join(format!("submission-intents/{}.json", intent.operation_id)),
        )
        .unwrap();
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains("private original message")
        );
        drop(host);
        let reopened = SubmissionIntents::new(dir.path().join("submission-intents"));
        assert_eq!(reopened.call(&core, close(&intent)).await.unwrap(), result);
        assert!(core.execute(stage_command(&intent).unwrap()).await.is_err());
        assert_eq!(
            reopened
                .call(
                    &core,
                    SubmissionIntentCall::List {
                        after: None,
                        limit: 1
                    }
                )
                .await
                .unwrap()["operationIds"],
            json!([])
        );
    }
    #[tokio::test]
    async fn accepted_creation_requires_final_receipt_even_when_checkpoint_claims_completion() {
        let (_dir, host, core, store, mut intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let mut checkpoint = intent.task.clone();
        checkpoint.stream_version = 1;
        intent.submission = Some(checkpoint);
        assert!(host.call(&core, close(&intent)).await.is_err());
        // A current projection can move forward; retirement still binds version 1.
        let mut rename =
            CommandEnvelope::local(CoreCommand::RenameTask(agent_protocol::RenameTask {
                task_id: intent.task.task_id.clone(),
                title: "later".into(),
            }));
        rename.expected_stream_version = Some(1);
        core.execute(rename).await.unwrap();
        core.invalidate_pending_command(&submission_command(&intent, 1))
            .await
            .unwrap();
        let head = store.event_head().await.unwrap();
        assert_eq!(
            host.call(&core, close(&intent)).await.unwrap()["resolution"]["status"],
            "invalidated"
        );
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn accepted_submission_closes_from_original_events_without_ui_checkpoint() {
        let (_dir, host, core, _store, intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let ack = core.execute(submission_command(&intent, 1)).await.unwrap();
        let result = host.call(&core, close(&intent)).await.unwrap();
        assert_eq!(result["resolution"]["status"], "submitted");
        assert_eq!(result["resolution"]["sequence"], json!(ack.first_sequence));
        assert!(
            result["resolution"]["turnId"]
                .as_str()
                .unwrap()
                .starts_with("trn_")
        );
        assert_eq!(host.call(&core, put(&intent)).await.unwrap(), result);
    }
    #[tokio::test]
    async fn accepted_context_uses_original_event_fence_after_later_task_changes() {
        use agent_protocol::{
            ContextPackageKind, ContextPreviewRequest, ContextStrength,
            submission::SubmissionContext,
        };
        let (_dir, host, core, store, mut intent) = fixture().await;
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        intent.task.stream_version = 1;
        intent.creation = None;
        intent.target.provider_id = ProviderId::parse("another").unwrap();
        let request = ContextPreviewRequest {
            task_id: intent.task.task_id.clone(),
            kind: ContextPackageKind::Handoff,
            strength: ContextStrength::Standard,
            source_executor: intent.task.preferred_executor.clone(),
            target_executor: intent.target.clone(),
            after_sequence: 0,
            through_sequence: store.event_head().await.unwrap(),
        };
        let built = core.build_context_preview(request.clone()).await.unwrap();
        intent.context = Some(SubmissionContext {
            operation_id: CommandId::new(),
            request,
            hash: built.payload_sha256,
        });
        host.call(&core, put(&intent)).await.unwrap();
        let ack = core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let boundary = core
            .sync_events(ack.last_sequence.unwrap() - 1, 1)
            .await
            .unwrap()
            .remove(0)
            .stream_version;
        assert!(boundary > intent.task.stream_version);
        assert!(host.call(&core, close(&intent)).await.is_err());
        let mut rename =
            CommandEnvelope::local(CoreCommand::RenameTask(agent_protocol::RenameTask {
                task_id: intent.task.task_id.clone(),
                title: "later".into(),
            }));
        rename.expected_stream_version = Some(boundary);
        core.execute(rename).await.unwrap();
        core.invalidate_pending_command(&submission_command(&intent, boundary))
            .await
            .unwrap();
        let head = store.event_head().await.unwrap();
        assert_eq!(
            host.call(&core, close(&intent)).await.unwrap()["resolution"]["status"],
            "invalidated"
        );
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn unregistered_competing_intent_can_close_only_after_original_barrier() {
        let (_dir, host, core, _store, intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        let mut second = intent.clone();
        second.operation_id = CommandId::new();
        second.creation.as_mut().unwrap().operation_id = CommandId::new();
        assert!(host.call(&core, put(&second)).await.is_err());
        assert!(host.call(&core, close(&second)).await.is_err());
        core.invalidate_pending_command(&stage_command(&second).unwrap())
            .await
            .unwrap();
        assert_eq!(
            host.call(&core, close(&second)).await.unwrap()["resolution"]["status"],
            "invalidated"
        );
        assert_eq!(
            host.call(&core, get(&intent)).await.unwrap()["state"],
            "pending"
        );
        assert_eq!(
            host.call(&core, put(&second)).await.unwrap()["state"],
            "closed"
        );
    }
    async fn delete_command(
        core: &Arc<Core>,
        intent: &SubmissionIntent,
    ) -> CommandEnvelope<CoreCommand> {
        let task = core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.task_id == intent.task.task_id)
            .unwrap();
        let mut command =
            CommandEnvelope::local(CoreCommand::DeleteTask(agent_protocol::TaskLifecycle {
                task_id: task.task_id,
            }));
        command.expected_stream_version = Some(task.stream_version);
        command
    }
    #[tokio::test]
    async fn deletion_resolves_purged_creation_and_invalidation_events_without_replaying() {
        for invalidate in [false, true] {
            let (dir, host, core, store, intent) = fixture().await;
            host.call(&core, put(&intent)).await.unwrap();
            core.execute(stage_command(&intent).unwrap()).await.unwrap();
            if invalidate {
                core.invalidate_pending_command(&submission_command(&intent, 1))
                    .await
                    .unwrap();
            }
            let ack = core
                .execute(delete_command(&core, &intent).await)
                .await
                .unwrap();
            let head = store.event_head().await.unwrap();
            let path = dir
                .path()
                .join(format!("submission-intents/{}.json", intent.operation_id));
            let before = std::fs::read(&path).unwrap();
            let resolved = host
                .call(
                    &core,
                    SubmissionIntentCall::Resolve {
                        intent: Box::new(intent.clone()),
                    },
                )
                .await
                .unwrap();
            assert_eq!(resolved["status"], "task_deleted");
            assert_eq!(resolved["deletionSequence"], json!(ack.last_sequence));
            assert_eq!(std::fs::read(&path).unwrap(), before); // Resolve is strictly read-only.
            let closed = host.call(&core, close(&intent)).await.unwrap();
            assert_eq!(closed["resolution"], resolved);
            assert!(
                !String::from_utf8(std::fs::read(&path).unwrap())
                    .unwrap()
                    .contains("private original message")
            );
            assert_eq!(host.call(&core, put(&intent)).await.unwrap(), closed);
            assert!(core.execute(submission_command(&intent, 1)).await.is_err());
            assert_eq!(store.event_head().await.unwrap(), head);
        }
    }
    #[tokio::test]
    async fn accepted_send_then_deleted_reports_deletion_instead_of_claiming_cancellation() {
        let (dir, host, core, store, intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let send = submission_command(&intent, 1);
        let ack = core.execute(send.clone()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if core.bootstrap().await.unwrap().tasks[0]
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
        core.execute(delete_command(&core, &intent).await)
            .await
            .unwrap();
        assert_eq!(core.command_receipt(&send).await.unwrap(), Some(ack));
        assert_eq!(
            core.sync_events(1, 100)
                .await
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "turn.queued")
                .count(),
            0
        );
        drop(host);
        let reopened = SubmissionIntents::new(dir.path().join("submission-intents"));
        let head = store.event_head().await.unwrap();
        reopened.reconcile_deleted(&core, None).await.unwrap();
        assert_eq!(
            reopened.call(&core, get(&intent)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn daemon_delete_cleans_shared_intent_and_other_task_remains_pending() {
        let (dir, _host, core, store, intent) = fixture().await;
        let client = crate::daemon::CoreClient::local_at(core.clone(), dir.path());
        client.submission_intents(put(&intent)).await.unwrap();
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let mut other = intent.clone();
        other.operation_id = CommandId::new();
        other.task.task_id = TaskId::new();
        other.scope = "draft:other".into();
        other.creation.as_mut().unwrap().operation_id = CommandId::new();
        client.submission_intents(put(&other)).await.unwrap();
        let deletion = delete_command(&core, &intent).await;
        client.execute(deletion.clone()).await.unwrap();
        assert_eq!(
            client.submission_intents(get(&intent)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        assert_eq!(
            client.submission_intents(get(&other)).await.unwrap()["state"],
            "pending"
        );
        let path = dir
            .path()
            .join(format!("submission-intents/{}.json", intent.operation_id));
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("private original message")
        );
        let head = store.event_head().await.unwrap();
        client.execute(deletion).await.unwrap();
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn missing_task_is_not_deletion_evidence_and_late_registration_uses_real_tombstone() {
        let (dir, host, core, store, intent) = fixture().await;
        assert!(
            host.call(
                &core,
                SubmissionIntentCall::Resolve {
                    intent: Box::new(intent.clone())
                }
            )
            .await
            .unwrap()
            .is_null()
        );
        assert!(!dir.path().join("submission-intents").exists());
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        core.execute(delete_command(&core, &intent).await)
            .await
            .unwrap();
        let head = store.event_head().await.unwrap();
        assert_eq!(
            host.call(&core, put(&intent)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        let mut changed = intent.clone();
        changed.input.prompt.push('!');
        assert!(
            host.call(
                &core,
                SubmissionIntentCall::Resolve {
                    intent: Box::new(changed)
                }
            )
            .await
            .is_err()
        );
        assert_eq!(store.event_head().await.unwrap(), head);
    }
    #[tokio::test]
    async fn deletion_reconciliation_also_updates_previous_closed_acceptance() {
        let (_dir, host, core, _store, intent) = fixture().await;
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        core.invalidate_pending_command(&submission_command(&intent, 1))
            .await
            .unwrap();
        host.call(&core, close(&intent)).await.unwrap();
        core.execute(delete_command(&core, &intent).await)
            .await
            .unwrap();
        host.reconcile_deleted(&core, Some(&intent.task.task_id))
            .await
            .unwrap();
        assert_eq!(
            host.call(&core, get(&intent)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
    }
    #[tokio::test]
    async fn list_pages_exclude_closed_and_do_not_lose_pending_ids() {
        let (_dir, host, core, _store, original) = fixture().await;
        let mut ids = vec![];
        for index in 0..4 {
            let mut intent = original.clone();
            intent.operation_id = CommandId::new();
            intent.scope = format!("draft:{index}");
            intent.task.task_id = TaskId::new();
            intent.creation.as_mut().unwrap().operation_id = CommandId::new();
            host.call(&core, put(&intent)).await.unwrap();
            if index == 1 {
                core.invalidate_pending_command(&stage_command(&intent).unwrap())
                    .await
                    .unwrap();
                host.call(&core, close(&intent)).await.unwrap();
            } else {
                ids.push(intent.operation_id);
            }
        }
        ids.sort();
        let mut after = None;
        let mut seen = vec![];
        loop {
            let page: SubmissionIntentPage = serde_json::from_value(
                host.call(&core, SubmissionIntentCall::List { after, limit: 1 })
                    .await
                    .unwrap(),
            )
            .unwrap();
            seen.extend(page.operation_ids);
            after = page.next_cursor;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(seen, ids);
    }
    #[tokio::test]
    async fn corrupt_record_is_not_replaced_or_hidden_by_discovery() {
        let (dir, host, core, _store, intent) = fixture().await;
        host.call(&core, put(&intent)).await.unwrap();
        let path = dir
            .path()
            .join(format!("submission-intents/{}.json", intent.operation_id));
        std::fs::write(&path, b"broken").unwrap();
        assert!(host.call(&core, get(&intent)).await.is_err());
        assert!(host.call(&core, put(&intent)).await.is_err());
        assert!(host.call(&core, close(&intent)).await.is_err());
        let page: SubmissionIntentPage = serde_json::from_value(
            host.call(
                &core,
                SubmissionIntentCall::List {
                    after: None,
                    limit: 10,
                },
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page.operation_ids, vec![intent.operation_id]);
        assert!(!page.unrecognized_entries);
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
    #[tokio::test]
    async fn damaged_entries_do_not_stop_other_deletion_cleanup_or_hide_discovery() {
        let (dir, host, core, _store, damaged) = fixture().await;
        host.call(&core, put(&damaged)).await.unwrap();
        let mut deleted = damaged.clone();
        deleted.operation_id = CommandId::new();
        deleted.task.task_id = TaskId::new();
        deleted.scope = "draft:deleted".into();
        deleted.creation.as_mut().unwrap().operation_id = CommandId::new();
        host.call(&core, put(&deleted)).await.unwrap();
        let mut live = deleted.clone();
        live.operation_id = CommandId::new();
        live.task.task_id = TaskId::new();
        live.scope = "draft:live".into();
        live.creation.as_mut().unwrap().operation_id = CommandId::new();
        host.call(&core, put(&live)).await.unwrap();
        core.execute(stage_command(&deleted).unwrap())
            .await
            .unwrap();
        core.execute(delete_command(&core, &deleted).await)
            .await
            .unwrap();
        let root = dir.path().join("submission-intents");
        let bad_path = root.join(format!("{}.json", damaged.operation_id));
        std::fs::write(&bad_path, b"broken original").unwrap();
        let unknown = root.join("unrecognized.json");
        std::fs::write(&unknown, b"unrecognized original").unwrap();
        // Restart reconciliation must still compact independently verified data.
        let reopened = SubmissionIntents::new(root.clone());
        assert!(
            reopened
                .reconcile_deleted(&core, None)
                .await
                .unwrap_err()
                .contains("部分")
        );
        assert_eq!(
            reopened.call(&core, get(&deleted)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        assert_eq!(
            reopened.call(&core, get(&live)).await.unwrap()["state"],
            "pending"
        );
        assert!(
            !std::fs::read_to_string(root.join(format!("{}.json", deleted.operation_id)))
                .unwrap()
                .contains("private original message")
        );
        assert_eq!(std::fs::read(&bad_path).unwrap(), b"broken original");
        assert_eq!(std::fs::read(&unknown).unwrap(), b"unrecognized original");
        let mut after = None;
        let mut found = vec![];
        loop {
            let page: SubmissionIntentPage = serde_json::from_value(
                reopened
                    .call(&core, SubmissionIntentCall::List { after, limit: 1 })
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(page.unrecognized_entries);
            found.extend(page.operation_ids);
            after = page.next_cursor;
            if after.is_none() {
                break;
            }
        }
        let mut expected = vec![damaged.operation_id.clone(), live.operation_id.clone()];
        expected.sort();
        assert_eq!(found, expected);
        // Unknown ownership still prevents admitting a new logical operation.
        let mut new = live.clone();
        new.operation_id = CommandId::new();
        new.task.task_id = TaskId::new();
        new.scope = "draft:new".into();
        new.creation.as_mut().unwrap().operation_id = CommandId::new();
        assert!(reopened.call(&core, put(&new)).await.is_err());
        // A previously registered healthy request can still be queried/reused.
        assert_eq!(
            reopened.call(&core, put(&live)).await.unwrap()["state"],
            "pending"
        );
        assert!(!root.join(format!("{}.json", new.operation_id)).exists());
    }
    #[tokio::test]
    async fn shared_hint_primitive_keeps_review_task_scope_validation() {
        let (_dir, _host, core, _store, intent) = fixture().await;
        let changes = crate::review::watch::Changes::new();
        assert!(
            changes
                .open(core.clone(), intent.task.task_id.to_string())
                .await
                .is_err()
        );
        core.execute(stage_command(&intent).unwrap()).await.unwrap();
        let mut subscription = changes
            .open(core.clone(), intent.task.task_id.to_string())
            .await
            .unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            crate::workspace_service::watch::WatchMessage::Invalidated
        ));
        core.execute(delete_command(&core, &intent).await)
            .await
            .unwrap();
        changes.changed();
        assert!(subscription.next().await.is_err());
    }
    #[tokio::test]
    async fn publication_hints_follow_put_close_and_deletion_without_query_writes() {
        use crate::workspace_service::watch::WatchMessage;
        use std::time::Duration;
        let (_dir, host, core, _store, intent) = fixture().await;
        let mut subscription = host.subscribe().unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        host.call(&core, put(&intent)).await.unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        host.call(&core, put(&intent)).await.unwrap();
        host.call(&core, get(&intent)).await.unwrap();
        host.call(
            &core,
            SubmissionIntentCall::List {
                after: None,
                limit: 50,
            },
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), subscription.next())
                .await
                .is_err()
        );
        core.invalidate_pending_command(&stage_command(&intent).unwrap())
            .await
            .unwrap();
        host.call(&core, close(&intent)).await.unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        let mut deleted = intent.clone();
        deleted.operation_id = CommandId::new();
        deleted.task.task_id = TaskId::new();
        deleted.scope = "draft:deleted-watch".into();
        deleted.creation.as_mut().unwrap().operation_id = CommandId::new();
        host.call(&core, put(&deleted)).await.unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        core.execute(stage_command(&deleted).unwrap())
            .await
            .unwrap();
        core.execute(delete_command(&core, &deleted).await)
            .await
            .unwrap();
        host.reconcile_deleted(&core, Some(&deleted.task.task_id))
            .await
            .unwrap();
        assert!(matches!(
            subscription.next().await.unwrap(),
            WatchMessage::Invalidated
        ));
        assert_eq!(
            host.call(&core, get(&deleted)).await.unwrap()["resolution"]["status"],
            "task_deleted"
        );
        host.reconcile_deleted(&core, Some(&deleted.task.task_id))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), subscription.next())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn unknown_entry_without_usable_ids_is_reported_instead_of_empty_success() {
        let (dir, host, core, _store, intent) = fixture().await;
        let root = dir.path().join("submission-intents");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("unknown.json"), b"keep").unwrap();
        let page: SubmissionIntentPage = serde_json::from_value(
            host.call(
                &core,
                SubmissionIntentCall::List {
                    after: None,
                    limit: 1,
                },
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(page.operation_ids.is_empty());
        assert!(page.next_cursor.is_none());
        assert!(page.unrecognized_entries);
        assert!(host.call(&core, put(&intent)).await.is_err());
        assert_eq!(std::fs::read(root.join("unknown.json")).unwrap(), b"keep");
    }
    #[tokio::test]
    async fn bad_schema_stage_identity_and_checkpoint_are_rejected_without_writes() {
        let (dir, host, core, _store, original) = fixture().await;
        let mut cases = vec![original.clone(); 5];
        cases[0].schema_version = 2;
        cases[1].creation.as_mut().unwrap().operation_id = original.operation_id.clone();
        cases[2].task.stream_version = 3;
        cases[3].requested_at = "invalid".into();
        let mut checkpoint = original.task.clone();
        checkpoint.stream_version = 8;
        cases[4].submission = Some(checkpoint);
        for intent in cases {
            assert!(host.call(&core, put(&intent)).await.is_err());
        }
        assert!(!dir.path().join("submission-intents").exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_record_cannot_redirect_a_read_or_write() {
        let (dir, host, core, _store, intent) = fixture().await;
        let root = dir.path().join("submission-intents");
        std::fs::create_dir(&root).unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"private").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(format!("{}.json", intent.operation_id)))
            .unwrap();
        assert!(host.call(&core, get(&intent)).await.is_err());
        assert!(host.call(&core, put(&intent)).await.is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"private");
    }

    async fn ready(core: &Arc<Core>, provider: &str) -> ExecutorRef {
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                provider_id: provider.into(),
                executable: format!("fixture:{provider}"),
                fingerprint: "deterministic-test-fixture".into(),
                status: agent_protocol::InstallationStatus::Ready,
                version: None,
                version_text: None,
                diagnostic: None,
                elapsed_millis: 0,
            },
        ));
        probe.actor = agent_protocol::ActorRef::SystemRecovery;
        core.execute(probe).await.unwrap();
        let installation = core
            .bootstrap()
            .await
            .unwrap()
            .installations
            .into_iter()
            .find(|i| i.provider_id == provider)
            .unwrap();
        let executor = ExecutorRef {
            provider_id: ProviderId::parse(provider).unwrap(),
            installation_id: Some(installation.installation_id.to_string()),
        };
        core.register_driver(Arc::new(driver_api::FakeDriver::with_executor(
            executor.clone(),
        )))
        .await;
        executor
    }
    fn start(
        intent: &SubmissionIntent,
        operation: &CommandId,
        task: Option<TaskId>,
        target: &ExecutorRef,
    ) -> SubmissionIntentCall {
        SubmissionIntentCall::Start {
            operation_id: operation.clone(),
            scope: intent.scope.clone(),
            task_id: task,
            project_id: intent.task.project_id.clone(),
            target: target.clone(),
            input: Box::new(intent.input.clone()),
        }
    }
    async fn queued(core: &Arc<Core>) -> usize {
        core.sync_events(0, 10_000)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "turn.queued")
            .count()
    }
    async fn idle(core: &Arc<Core>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .any(|t| t.active_primary_run.is_some())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    fn submitted_task(value: &Value) -> TaskId {
        assert_eq!(value["state"], "closed", "{value}");
        assert_eq!(value["resolution"]["status"], "submitted", "{value}");
        TaskId::parse(value["resolution"]["taskId"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn service_start_creates_a_conversation_and_sends_once_even_when_retried() {
        let (_dir, host, core, _store, intent) = fixture().await;
        let operation = CommandId::new();
        let first = host
            .call(&core, start(&intent, &operation, None, &intent.target))
            .await
            .unwrap();
        let task = submitted_task(&first);
        idle(&core).await;
        assert_eq!(queued(&core).await, 1);
        let snapshot = core.bootstrap().await.unwrap();
        assert!(snapshot.tasks.iter().any(|t| t.task_id == task));
        // A retry after a lost reply returns the original closure without resending.
        let retried = host
            .call(&core, start(&intent, &operation, None, &intent.target))
            .await
            .unwrap();
        assert_eq!(retried, first);
        assert_eq!(queued(&core).await, 1);
    }

    #[tokio::test]
    async fn service_start_switches_agent_with_its_own_context_package() {
        let (_dir, host, core, _store, intent) = fixture().await;
        let first = host
            .call(
                &core,
                start(&intent, &CommandId::new(), None, &intent.target),
            )
            .await
            .unwrap();
        let task = submitted_task(&first);
        idle(&core).await;
        let other = ready(&core, "other").await;
        let mut next = intent.clone();
        next.scope = format!("task:{task}");
        let switched = host
            .call(
                &core,
                start(&next, &CommandId::new(), Some(task.clone()), &other),
            )
            .await
            .unwrap();
        assert_eq!(submitted_task(&switched), task);
        idle(&core).await;
        let current = core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .into_iter()
            .find(|t| t.task_id == task)
            .unwrap();
        assert_eq!(current.preferred_executor, other);
        assert_eq!(queued(&core).await, 2);
        assert!(
            core.sync_events(0, 10_000)
                .await
                .unwrap()
                .iter()
                .any(|e| e.event_type == "task.preferred_executor_changed")
        );
    }

    #[tokio::test]
    async fn pending_send_is_continued_or_retired_by_the_service_without_replay() {
        let (_dir, host, core, _store, intent) = fixture().await;
        // A frozen original that no client finished, as after a crash before any mutation.
        let frozen = freeze(
            &core,
            CommandId::new(),
            intent.scope.clone(),
            None,
            intent.task.project_id.clone(),
            intent.target.clone(),
            intent.input.clone(),
        )
        .await
        .unwrap();
        host.call(&core, put(&frozen)).await.unwrap();
        // The same draft cannot start a second send while the original is pending.
        assert!(
            host.call(
                &core,
                start(&intent, &CommandId::new(), None, &intent.target)
            )
            .await
            .unwrap_err()
            .contains("待核实发送")
        );
        // A retry of the same identity must carry the same content.
        let mut changed = start(&intent, &frozen.operation_id, None, &intent.target);
        if let SubmissionIntentCall::Start { input, .. } = &mut changed {
            input.prompt = "different".into();
        }
        assert!(
            host.call(&core, changed)
                .await
                .unwrap_err()
                .contains("其他内容")
        );
        let continued = host
            .call(
                &core,
                SubmissionIntentCall::Continue {
                    operation_id: frozen.operation_id.clone(),
                },
            )
            .await
            .unwrap();
        submitted_task(&continued);
        idle(&core).await;
        assert_eq!(queued(&core).await, 1);

        let mut second = intent.clone();
        second.scope = "draft:second".into();
        let abandoned = freeze(
            &core,
            CommandId::new(),
            second.scope.clone(),
            None,
            second.task.project_id.clone(),
            second.target.clone(),
            second.input.clone(),
        )
        .await
        .unwrap();
        host.call(&core, put(&abandoned)).await.unwrap();
        let retired = host
            .call(
                &core,
                SubmissionIntentCall::Retire {
                    operation_id: abandoned.operation_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(retired["resolution"]["status"], "invalidated", "{retired}");
        // The fenced creation can no longer be accepted, and continuing is read-only.
        assert!(
            core.execute(stage_command(&abandoned).unwrap())
                .await
                .is_err()
        );
        let after = host
            .call(
                &core,
                SubmissionIntentCall::Continue {
                    operation_id: abandoned.operation_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(after, retired);
        assert_eq!(queued(&core).await, 1);
        assert!(
            !core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .any(|t| t.task_id == abandoned.task.task_id)
        );
    }
}
