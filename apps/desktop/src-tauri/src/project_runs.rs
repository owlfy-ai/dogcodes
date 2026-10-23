//! Persisted, platform-specific project launch recipes. Analysis never launches them.
use crate::{
    DesktopState,
    review_window::require_task_window,
    terminal_host::{LaunchIntent, TerminalInfo},
};
use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId};
use agent_protocol::ProjectSummary;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

pub mod operations;
pub mod watch;
use operations::StoredCatalog;
pub use operations::{AnalyzeRequest, EditRequest, MutationError, RunOperation};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCommand {
    pub id: String,
    pub label: String,
    pub command: String,
    pub cwd: String,
    pub source: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub user_renamed: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDirectory {
    pub path: String,
    pub status: String,
    pub commands: Vec<RunCommand>,
    pub error: Option<String>,
    #[serde(default)]
    pub user_modified: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalog {
    pub project_id: String,
    pub platform: String,
    pub shell: String,
    pub generation: String,
    pub executor: Option<ExecutorRef>,
    pub directories: Vec<RunDirectory>,
}
#[derive(Clone)]
pub struct ProjectRunHost {
    cache: Arc<Mutex<HashMap<String, StoredCatalog>>>,
    directory: PathBuf,
    gate: Arc<tokio::sync::Semaphore>,
    active: Arc<std::sync::atomic::AtomicUsize>,
    admission: Arc<tokio::sync::Mutex<()>>,
    uncertain_storage: Arc<std::sync::atomic::AtomicBool>,
    // Access only with `cache` locked first. These contain the last durable
    // commands plus terminal analysis state, never an unsaved Agent result.
    finalizations: Arc<Mutex<HashMap<String, StoredCatalog>>>,
    cleanup_pending: Arc<Mutex<HashSet<(String, String)>>>,
    pub(crate) changes: Arc<crate::state_changes::Changes>,
}
impl ProjectRunHost {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            cache: Arc::default(),
            directory,
            gate: Arc::new(tokio::sync::Semaphore::new(2)),
            active: Arc::default(),
            admission: Arc::default(),
            uncertain_storage: Arc::default(),
            finalizations: Arc::default(),
            cleanup_pending: Arc::default(),
            changes: Arc::new(crate::state_changes::Changes::new()),
        }
    }
    fn file(&self, id: &str) -> PathBuf {
        self.directory
            .join(format!("{id}-{}.json", std::env::consts::OS))
    }
    pub fn has_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Acquire) > 0
    }
    fn load(&self, id: &str) -> Result<Option<StoredCatalog>, String> {
        // Read and install under one lock, so an older disk snapshot cannot
        // overwrite an edit or an analysis accepted by another client.
        let mut cache = self.cache.lock().map_err(|_| "项目分析状态不可用")?;
        self.flush_finalization(&mut cache, id)?;
        if let Some(value) = cache.get(id) {
            return Ok(Some(value.clone()));
        }
        let bytes = match std::fs::read(self.file(id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法读取运行配置，已保留原文件：{error}")),
        };
        let mut catalog: StoredCatalog = serde_json::from_slice(&bytes)
            .map_err(|error| format!("运行配置损坏，已保留原文件：{error}"))?;
        if catalog.project_id != id || catalog.platform != std::env::consts::OS {
            return Err("运行配置归属不匹配，已保留原文件".into());
        }
        let interrupted =
            catalog.interrupt_analysis("interrupted", "后台重启，原分析已中断，未自动重新执行");
        if interrupted {
            self.save(&catalog).map_err(|error| {
                format!("上次分析已中断，恢复状态尚未保存；请检查磁盘后重试：{error}")
            })?;
        }
        cache.insert(id.into(), catalog.clone());
        Ok(Some(catalog))
    }
    fn flush_finalization(
        &self,
        cache: &mut HashMap<String, StoredCatalog>,
        id: &str,
    ) -> Result<(), String> {
        self.require_known_storage()?;
        let mut finalizations = self
            .finalizations
            .lock()
            .map_err(|_| "项目分析恢复状态不可用")?;
        if let Some(finalization) = finalizations.get(id) {
            if cache
                .get(id)
                .is_none_or(|c| c.generation != finalization.generation)
            {
                return Err("分析恢复版本不匹配，已保留原运行配置；请重新打开后台核实".into());
            }
            self.save(finalization).map_err(|error| {
                format!("分析已停止，状态尚未保存；请检查磁盘后重试，未重新执行分析：{error}")
            })?;
            cache.insert(id.into(), finalization.clone());
            finalizations.remove(id);
            self.changes.changed();
        }
        Ok(())
    }
    fn require_known_storage(&self) -> Result<(), String> {
        if self
            .uncertain_storage
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err("运行配置写入结果待核实，请检查磁盘后重启后台；未重新执行操作".into());
        }
        Ok(())
    }
    fn save(&self, catalog: &StoredCatalog) -> Result<(), String> {
        self.require_known_storage()?;
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        // NamedTempFile replaces the existing file atomically on all supported
        // platforms; never delete a valid Windows cache before renaming.
        let mut file =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, catalog).map_err(|e| e.to_string())?;
        file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
        file.persist(self.file(&catalog.project_id))
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Err(error) = std::fs::File::open(&self.directory).and_then(|file| file.sync_all()) {
            // The rename may already be visible. Do not retry against a stale
            // in-memory catalog if directory durability could not be confirmed.
            self.uncertain_storage
                .store(true, std::sync::atomic::Ordering::Release);
            return Err(format!("运行配置写入结果待核实：{error}"));
        }
        Ok(())
    }
    #[cfg(test)]
    fn edit(
        &self,
        id: &str,
        generation: &str,
        index: usize,
        command_id: &str,
        edit: RunEdit,
    ) -> Result<RunCatalog, String> {
        self.edit_recorded(id, generation, index, command_id, edit, None)
            .map_err(|e| e.to_string())
    }
    fn edit_recorded(
        &self,
        id: &str,
        generation: &str,
        index: usize,
        command_id: &str,
        edit: RunEdit,
        operation: Option<(&str, serde_json::Value)>,
    ) -> Result<RunCatalog, MutationError> {
        self.load(id)?
            .ok_or_else(|| MutationError::rejected("catalog_missing", "请先分析项目"))?;
        let mut cache = self.cache.lock().map_err(|_| "项目分析状态不可用")?;
        self.flush_finalization(&mut cache, id)?;
        let mut catalog = cache
            .get(id)
            .cloned()
            .ok_or_else(|| MutationError::rejected("catalog_missing", "请先分析项目"))?;
        if let Some((operation_id, request)) = &operation
            && catalog.replay(operation_id, request)?
        {
            return Ok(catalog.catalog);
        }
        if catalog.generation != generation {
            return Err(MutationError::rejected(
                "version_conflict",
                "运行项已更新，请刷新后重试",
            ));
        }
        if catalog.directories.iter().any(|d| d.status == "analyzing") {
            return Err(MutationError::rejected(
                "analysis_running",
                "分析过程中不能修改运行项",
            ));
        }
        let directory = catalog
            .directories
            .get_mut(index)
            .ok_or_else(|| MutationError::rejected("directory_missing", "目录不存在"))?;
        let position = directory
            .commands
            .iter()
            .position(|c| c.id == command_id)
            .ok_or_else(|| MutationError::rejected("command_missing", "运行项已不存在"))?;
        match edit {
            RunEdit::Rename { label } => {
                let label = label.trim();
                if label.is_empty()
                    || label.chars().count() > 60
                    || label.chars().any(char::is_control)
                {
                    return Err(MutationError::rejected(
                        "invalid_request",
                        "名称需为 1–60 个字符，且不能包含换行",
                    ));
                }
                if directory.commands[position].label != label {
                    directory.commands[position].label = label.into();
                    directory.commands[position].user_renamed = true;
                    directory.user_modified = true;
                }
            }
            RunEdit::Delete => {
                directory.commands.remove(position);
                directory.user_modified = true;
            }
            RunEdit::Pin { pinned } => {
                directory.commands[position].pinned = pinned;
            }
        }
        directory.commands.sort_by_key(|c| !c.pinned);
        catalog.generation = uuid::Uuid::new_v4().to_string();
        if let Some((operation_id, request)) = operation {
            catalog.record(operation_id, request, "succeeded");
        }
        self.save(&catalog)?;
        cache.insert(id.into(), catalog.clone());
        self.changes.changed();
        Ok(catalog.catalog)
    }
    fn finish(
        &self,
        id: &str,
        generation: &str,
        index: usize,
        result: Result<Vec<RunCommand>, String>,
    ) -> bool {
        let Ok(mut cache) = self.cache.lock() else {
            return false;
        };
        if self.cleanup_pending.lock().map_or(true, |pending| {
            pending.contains(&(id.into(), generation.into()))
        }) {
            return false;
        }
        let Some(original) = cache.get(id).filter(|c| c.generation == generation) else {
            return false;
        };
        let Ok(mut finalizations) = self.finalizations.lock() else {
            return false;
        };
        if finalizations.contains_key(id) {
            return false;
        }
        let mut catalog = original.clone();
        let Some(directory) = catalog.directories.get_mut(index) else {
            return false;
        };
        match result {
            Ok(commands) => {
                let mut retained: Vec<_> = directory
                    .commands
                    .iter()
                    .filter(|c| c.pinned)
                    .cloned()
                    .collect();
                for command in commands {
                    if !retained
                        .iter()
                        .any(|c| c.cwd == command.cwd && c.command == command.command)
                    {
                        retained.push(command);
                    }
                }
                directory.user_modified = retained.iter().any(|c| c.user_renamed);
                directory.commands = retained;
                directory.status = "ready".into();
                directory.error = None;
            }
            Err(error) => {
                directory.status = "error".into();
                directory.error = Some(error);
            }
        }
        catalog.settle_analysis();
        if let Err(error) = self.save(&catalog) {
            // Roll back to the durable commands. A later read may persist this
            // terminal state after disk repair, but must never publish the
            // generated commands or silently restart the remaining Agent work.
            let mut failed = original.clone();
            failed.interrupt_analysis(
                "failed",
                &format!("分析结果未能保存，请检查磁盘后重新分析：{error}"),
            );
            finalizations.insert(id.into(), failed);
            self.changes.changed();
            return false;
        }
        cache.insert(id.into(), catalog);
        self.changes.changed();
        true
    }
    fn interrupt(&self, id: &str, generation: &str) {
        self.interrupt_with_message(id, generation, "项目分析已中断，请重新分析；未自动重新执行");
    }
    fn interrupt_with_message(&self, id: &str, generation: &str, message: &str) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        let Some(mut catalog) = cache
            .get(id)
            .filter(|c| c.generation == generation)
            .cloned()
        else {
            return;
        };
        let Ok(mut finalizations) = self.finalizations.lock() else {
            return;
        };
        if finalizations.contains_key(id) || !catalog.interrupt_analysis("interrupted", message) {
            return;
        }
        if self.save(&catalog).is_ok() {
            cache.insert(id.into(), catalog);
        } else {
            finalizations.insert(id.into(), catalog);
        }
        self.changes.changed();
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum RunEdit {
    Rename { label: String },
    Delete,
    Pin { pinned: bool },
}
const CONFIRM_OVERWRITE: &str = "run_commands_confirmation_required";
fn has_user_edits(catalog: &RunCatalog) -> bool {
    catalog.directories.iter().any(|d| d.user_modified)
}
fn prepare_analysis(
    cached: Option<&RunCatalog>,
    paths: &[String],
    overwrite_generation: Option<&str>,
) -> Result<Vec<RunDirectory>, String> {
    if let Some(catalog) = cached
        && has_user_edits(catalog)
        && overwrite_generation != Some(catalog.generation.as_str())
    {
        return Err(CONFIRM_OVERWRITE.into());
    }
    Ok(paths
        .iter()
        .map(|path| {
            let previous = cached.and_then(|c| c.directories.iter().find(|d| &d.path == path));
            RunDirectory {
                path: path.clone(),
                status: "analyzing".into(),
                commands: previous.map(|d| d.commands.clone()).unwrap_or_default(),
                error: None,
                user_modified: previous.is_some_and(|d| d.user_modified),
            }
        })
        .collect())
}

fn roots(project: &ProjectSummary) -> Vec<String> {
    std::iter::once(project.path.clone())
        .chain(project.additional_directories.clone())
        .collect()
}
fn shell() -> &'static str {
    if cfg!(windows) {
        "PowerShell"
    } else {
        "POSIX sh"
    }
}

pub async fn analyze(
    host: ProjectRunHost,
    core: Arc<Core>,
    project: ProjectSummary,
    executor: Option<ExecutorRef>,
    force: bool,
    overwrite_generation: Option<String>,
) -> Result<RunCatalog, String> {
    analyze_recorded(
        host,
        core,
        project,
        executor,
        force,
        overwrite_generation,
        None,
    )
    .await
    .map_err(|e| e.to_string())
}
async fn analyze_recorded(
    host: ProjectRunHost,
    core: Arc<Core>,
    project: ProjectSummary,
    executor: Option<ExecutorRef>,
    force: bool,
    overwrite_generation: Option<String>,
    operation: Option<AnalyzeRequest>,
) -> Result<RunCatalog, MutationError> {
    let admission = host.admission.clone();
    let _admission = admission.lock().await;
    let id = project.project_id.to_string();
    let paths = roots(&project);
    let cached = host.load(&id)?;
    if let Some(request) = &operation {
        uuid::Uuid::parse_str(&request.operation_id)
            .map_err(|_| MutationError::rejected("invalid_request", "无效的运行操作标识"))?;
        if let Some(cached) = &cached
            && cached.replay(
                &request.operation_id,
                &serde_json::to_value(request).map_err(|e| e.to_string())?,
            )?
        {
            return Ok(cached.catalog.clone());
        }
        if cached.as_ref().map(|c| c.generation.as_str())
            != Some(request.expected_generation.as_str())
        {
            return Err(MutationError::rejected(
                "version_conflict",
                "运行配置已更新，请刷新后再分析",
            ));
        }
    }
    if !force
        && let Some(cached) = &cached
        && cached.platform == std::env::consts::OS
        && (cached.directories.iter().map(|d| &d.path).eq(paths.iter()) || has_user_edits(cached))
    {
        return Ok(cached.catalog.clone());
    }
    let available = core
        .available_execution_candidates()
        .await
        .map_err(|e| e.to_string())?;
    if executor
        .as_ref()
        .is_some_and(|executor| !available.contains(executor))
    {
        return Err(MutationError::rejected(
            "executor_unavailable",
            "分析 Agent 当前不可用或已禁用",
        ));
    }
    let requested = executor.or_else(|| {
        cached
            .as_ref()
            .and_then(|c| c.executor.clone())
            .filter(|e| available.contains(e))
    });
    let chosen = requested.or_else(|| {
        available
            .iter()
            .find(|e| !e.provider_id.as_str().starts_with("fake"))
            .cloned()
    });
    if operation.is_some() && chosen.is_none() {
        return Err(MutationError::rejected(
            "executor_unavailable",
            "没有可用的分析 Agent，请连接 Agent 后重新分析",
        ));
    }
    // Replay above precedes new-work admission. Hold the Core guard through the
    // atomic catalog write and dispatch, not through scanning or model execution.
    let agent_admission =
        if let Some(executor) = &chosen {
            Some(core.admit_execution(executor).await.map_err(|error| {
                MutationError::rejected("executor_unavailable", &error.to_string())
            })?)
        } else {
            None
        };
    let execution = agent_admission
        .as_ref()
        .map(agent_core::ExecutionAdmission::execution);
    let generation = uuid::Uuid::new_v4().to_string();
    let mut catalog = RunCatalog {
        project_id: id.clone(),
        platform: std::env::consts::OS.into(),
        shell: shell().into(),
        generation: generation.clone(),
        executor: chosen.clone(),
        directories: vec![],
    };
    {
        let mut cache = host.cache.lock().map_err(|_| "项目分析状态不可用")?;
        // Background completion may fail while Agent admission is awaited.
        // Save its terminal state before creating a replacement generation.
        host.flush_finalization(&mut cache, &id)?;
        if host
            .cleanup_pending
            .lock()
            .map_err(|_| "分析清理状态不可用")?
            .iter()
            .any(|(project_id, _)| project_id == &id)
        {
            return Err(MutationError::rejected(
                "analysis_cleanup_pending",
                "原分析 Agent 会话关闭待核实，后台正在清理，请稍后重试",
            ));
        }
        // Coalesce concurrent openings, but allow an explicit refresh to supersede old work.
        if !force
            && let Some(current) = cache.get(&id)
            && (current.directories.iter().map(|d| &d.path).eq(paths.iter())
                || has_user_edits(current))
        {
            return Ok(current.catalog.clone());
        }
        catalog.directories = prepare_analysis(
            cache.get(&id).map(|c| &c.catalog),
            &paths,
            overwrite_generation.as_deref(),
        )
        .map_err(|message| MutationError::rejected("confirmation_required", &message))?;
        let mut stored = StoredCatalog {
            catalog: catalog.clone(),
            operations: cache
                .get(&id)
                .map(|c| c.operations.clone())
                .unwrap_or_default(),
        };
        for receipt in stored
            .operations
            .values_mut()
            .filter(|r| r.result.status == "running")
        {
            receipt.result.status = "superseded".into();
        }
        if let Some(request) = &operation {
            stored.record(
                &request.operation_id,
                serde_json::to_value(request).map_err(|e| e.to_string())?,
                "running",
            );
        }
        host.save(&stored)?;
        cache.insert(id.clone(), stored);
    }
    host.changes.changed();
    let running = Arc::new(ActiveAnalysis::new(
        host.clone(),
        id.clone(),
        generation.clone(),
    ));
    tokio::spawn(async move {
        for (index, path) in paths.iter().enumerate() {
            let Ok(_permit) = host.gate.acquire().await else {
                return;
            };
            if !host
                .cache
                .lock()
                .is_ok_and(|cache| cache.get(&id).is_some_and(|c| c.generation == generation))
            {
                return;
            }
            let result = async {
                let execution = execution.as_ref().ok_or("没有可用的分析 Agent，请连接 Agent 后重新分析")?;
                let root = PathBuf::from(path); let scan_root = root.clone();
                let files = tokio::task::spawn_blocking(move || scan(&scan_root)).await.map_err(|_| "读取目录失败")??;
                if files.is_empty() { return Ok(vec![]); }
                let packet = serde_json::json!({ "platform": std::env::consts::OS, "shell": shell(), "files": files }).to_string();
                let response = execution.analyze_project_commands_with_lifetime(packet, running.clone()).await?;
                parse_commands(&response, &root, &files)
            }.await;
            if running.stopped.load(std::sync::atomic::Ordering::Acquire)
                || !host.finish(&id, &generation, index, result)
            {
                return;
            }
        }
    });
    drop(agent_admission);
    Ok(catalog)
}

struct ActiveAnalysis {
    stopped: std::sync::atomic::AtomicBool,
    host: ProjectRunHost,
    id: String,
    generation: String,
}
impl ActiveAnalysis {
    fn new(host: ProjectRunHost, id: String, generation: String) -> Self {
        host.active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self {
            stopped: std::sync::atomic::AtomicBool::new(false),
            host,
            id,
            generation,
        }
    }
}
impl agent_core::AnalysisLifetime for ActiveAnalysis {
    fn cleanup_pending(&self, message: &str) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut pending) = self.host.cleanup_pending.lock() {
            pending.insert((self.id.clone(), self.generation.clone()));
        }
        self.host
            .interrupt_with_message(&self.id, &self.generation, message);
    }
}
impl Drop for ActiveAnalysis {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.host.cleanup_pending.lock() {
            pending.remove(&(self.id.clone(), self.generation.clone()));
        }
        self.host.interrupt(&self.id, &self.generation);
        self.host
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub async fn schedule(
    host: &ProjectRunHost,
    core: Arc<Core>,
    project_id: ProjectId,
) -> Result<RunCatalog, String> {
    let project = core
        .bootstrap()
        .await
        .map_err(|e| e.to_string())?
        .projects
        .into_iter()
        .find(|project| project.project_id == project_id)
        .ok_or("项目不存在")?;
    analyze(host.clone(), core, project, None, false, None).await
}
async fn task_project(core: &Core, task_id: &str) -> Result<ProjectSummary, String> {
    task_project_for_mutation(core, task_id)
        .await
        .map_err(|e| e.to_string())
}
async fn task_project_for_mutation(
    core: &Core,
    task_id: &str,
) -> Result<ProjectSummary, MutationError> {
    let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
    let task = snapshot
        .tasks
        .iter()
        .find(|task| task.task_id.as_str() == task_id)
        .ok_or_else(|| MutationError::rejected("task_missing", "会话不存在"))?;
    snapshot
        .projects
        .iter()
        .find(|project| project.project_id == task.project_id)
        .cloned()
        .ok_or_else(|| MutationError::rejected("project_missing", "项目不存在"))
}

// Only desktop entry-point checks live here. The same business operations run in
// the service for every client, including the isolated acceptance host.
#[tauri::command]
pub async fn core_project_runs(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<RunCatalog, String> {
    require_task_window(window.label(), &task_id)?;
    state.core.project_runs(&task_id).await
}
#[tauri::command]
pub async fn core_project_runs_analyze(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: AnalyzeRequest,
) -> Result<RunCatalog, MutationError> {
    require_task_window(window.label(), &request.task_id)
        .map_err(|message| MutationError::rejected("access_denied", &message))?;
    state.core.project_runs_analyze(request).await
}
#[tauri::command]
pub async fn core_project_run_edit(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: EditRequest,
) -> Result<RunCatalog, MutationError> {
    require_task_window(window.label(), &request.task_id)
        .map_err(|message| MutationError::rejected("access_denied", &message))?;
    state.core.project_run_edit(request).await
}
#[tauri::command]
pub async fn core_project_run_operation(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    operation_id: String,
) -> Result<Option<RunOperation>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .project_run_operation(&task_id, &operation_id)
        .await
}
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn core_project_run_open(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    generation: String,
    directory_index: usize,
    command_id: String,
    cols: u16,
    rows: u16,
    operation_id: String,
) -> Result<TerminalInfo, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .project_run_open(LaunchRequest {
            task_id,
            generation,
            directory_index,
            command_id,
            cols,
            rows,
            operation_id,
        })
        .await
}

pub use crate::terminal_host::ProjectLaunchRequest as LaunchRequest;
async fn launch(
    host: &ProjectRunHost,
    core: &Arc<Core>,
    terminals: &crate::terminal_host::TerminalHost,
    request: LaunchRequest,
) -> Result<TerminalInfo, String> {
    let _admission = host.admission.lock().await;
    if let Some(existing) = terminals.existing_project_launch(&request)? {
        return Ok(existing);
    }
    let project = task_project(core, &request.task_id).await?;
    let catalog = host
        .load(project.project_id.as_str())?
        .ok_or("请先分析项目")?;
    if catalog.generation != request.generation
        || catalog.platform != std::env::consts::OS
        || !catalog
            .directories
            .iter()
            .map(|d| &d.path)
            .eq(roots(&project).iter())
    {
        return Err("运行配置已更新，请刷新后再运行".into());
    }
    let directory = catalog
        .directories
        .get(request.directory_index)
        .filter(|d| d.status != "analyzing")
        .ok_or("目录尚未分析完成")?;
    let command = directory
        .commands
        .iter()
        .find(|c| c.id == request.command_id)
        .ok_or("运行命令已变化，请刷新")?;
    let cwd = resolve_cwd(Path::new(&directory.path), &command.cwd)?;
    terminals
        .start(LaunchIntent {
            project_launch: Some(request.clone()),
            operation_id: request.operation_id,
            project_id: project.project_id.to_string(),
            directory_index: request.directory_index,
            task_id: Some(request.task_id),
            command_id: Some(request.command_id),
            label: command.label.clone(),
            cwd,
            command: Some(command.command.clone()),
            cols: request.cols,
            rows: request.rows,
        })
        .await
}

pub async fn dispatch(
    core: &Arc<Core>,
    host: &ProjectRunHost,
    terminals: &crate::terminal_host::TerminalHost,
    method: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use serde_json::{from_value, json};
    match method {
        "project_runs" => {
            let task_id: String = from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(
                analyze(
                    host.clone(),
                    core.clone(),
                    task_project(core, &task_id).await?,
                    None,
                    false,
                    None
                )
                .await?
            ))
        }
        "project_runs_analyze" => {
            let result: Result<RunCatalog, MutationError> = async {
                let request: AnalyzeRequest = from_value(args).map_err(|e| {
                    MutationError::rejected("invalid_request", &format!("分析请求无效：{e}"))
                })?;
                analyze_recorded(
                    host.clone(),
                    core.clone(),
                    task_project_for_mutation(core, &request.task_id).await?,
                    request.executor.clone(),
                    true,
                    request.overwrite_generation.clone(),
                    Some(request),
                )
                .await
            }
            .await;
            // Domain outcomes stay inside the response; outer RPC errors are
            // transport/infrastructure failures with an unknown outcome.
            Ok(json!(result))
        }
        "project_run_edit" => {
            let result: Result<RunCatalog, MutationError> = async {
                let request: EditRequest = from_value(args).map_err(|e| {
                    MutationError::rejected("invalid_request", &format!("编辑请求无效：{e}"))
                })?;
                let _admission = host.admission.lock().await;
                let project = task_project_for_mutation(core, &request.task_id).await?;
                host.edit_recorded(
                    project.project_id.as_str(),
                    &request.generation,
                    request.directory_index,
                    &request.command_id,
                    request.edit.clone(),
                    Some((&request.operation_id, json!(request))),
                )
            }
            .await;
            Ok(json!(result))
        }
        "project_run_operation" => {
            let (task_id, operation_id): (String, String) =
                from_value(args).map_err(|e| e.to_string())?;
            let project = task_project(core, &task_id).await?;
            let catalog = host.load(project.project_id.as_str())?;
            Ok(json!(catalog.and_then(|c| {
                c.operations
                    .get(&operation_id)
                    .filter(|r| r.request["taskId"] == task_id)
                    .map(|r| {
                        let mut result = r.result.clone();
                        result.request = Some(r.request.clone());
                        result
                    })
            })))
        }
        "project_run_open" => Ok(json!(
            launch(
                host,
                core,
                terminals,
                from_value(args).map_err(|e| e.to_string())?
            )
            .await?
        )),
        _ => Err("unknown project run operation".into()),
    }
}

#[derive(Clone, Serialize)]
struct InputFile {
    path: String,
    text: String,
    truncated: bool,
    executable: Option<bool>,
}
fn executable(path: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::metadata(path).ok()?.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
fn relevant(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("readme")
        || [
            "package.json",
            "pyproject.toml",
            "cargo.toml",
            "go.mod",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "makefile",
            "justfile",
            "dockerfile",
            "compose.yml",
            "compose.yaml",
            "docker-compose.yml",
            "docker-compose.yaml",
            "procfile",
            "deno.json",
            "deno.jsonc",
            "pubspec.yaml",
            "gemfile",
            "pnpm-lock.yaml",
            "package-lock.json",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ]
        .contains(&name.as_str())
        || [".sh", ".ps1", ".cmd", ".bat"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}
fn scan(root: &Path) -> Result<Vec<InputFile>, String> {
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut queue = std::collections::VecDeque::from([(root.clone(), 0)]);
    let mut files = vec![];
    let mut budget = 96_000_usize;
    let mut visited = 0;
    while let Some((directory, depth)) = queue.pop_front() {
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|e| e.to_string())?
            .flatten()
            .take(4001 - visited)
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            visited += 1;
            if visited > 4000 || files.len() >= 120 || budget == 0 {
                return Ok(files);
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || [
                    "node_modules",
                    "target",
                    "vendor",
                    "dist",
                    "build",
                    "coverage",
                    "venv",
                    "__pycache__",
                ]
                .contains(&name.as_str())
            {
                continue;
            }
            if kind.is_dir() && depth < 3 {
                queue.push_back((entry.path(), depth + 1));
            } else if kind.is_file() && relevant(&name) {
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                if name.contains("lock") && name != "Dockerfile" {
                    files.push(InputFile {
                        path: relative,
                        text: "[lockfile present]".into(),
                        truncated: true,
                        executable: None,
                    });
                    continue;
                }
                let limit = budget.min(12_000);
                let mut bytes = vec![];
                std::fs::File::open(entry.path())
                    .map_err(|e| e.to_string())?
                    .take((limit + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                let truncated = bytes.len() > limit;
                bytes.truncate(limit);
                budget -= bytes.len();
                if bytes.contains(&0) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                files.push(InputFile {
                    path: relative,
                    text,
                    truncated,
                    executable: executable(&entry.path()),
                });
            }
        }
    }
    Ok(files)
}
fn resolve_cwd(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.contains('\\')
        || Path::new(relative)
            .components()
            .any(|c| !matches!(c, Component::CurDir | Component::Normal(_)))
    {
        return Err("运行目录必须位于项目内".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let target = root
        .join(relative)
        .canonicalize()
        .map_err(|_| "运行目录不存在")?;
    if !target.starts_with(&root) || !target.is_dir() {
        return Err("运行目录必须位于项目内".into());
    }
    Ok(target)
}
fn parse_commands(text: &str, root: &Path, files: &[InputFile]) -> Result<Vec<RunCommand>, String> {
    #[derive(Deserialize)]
    struct Response {
        commands: Vec<Recipe>,
    }
    #[derive(Deserialize)]
    struct Recipe {
        label: String,
        command: String,
        #[serde(default = "dot")]
        cwd: String,
        source: String,
    }
    fn dot() -> String {
        ".".into()
    }
    let text = text.trim();
    let text = if text.starts_with("```") {
        text.split_once('\n')
            .and_then(|(_, rest)| rest.rsplit_once("```").map(|(json, _)| json))
            .ok_or("Agent 返回格式不正确")?
    } else {
        text
    };
    let response: Response =
        serde_json::from_str(text).map_err(|_| "Agent 返回格式不正确，请重新分析")?;
    if response.commands.len() > 24 {
        return Err("Agent 返回了过多命令".into());
    }
    let mut seen = HashSet::new();
    let mut result = vec![];
    for recipe in response.commands {
        let label = recipe.label.trim();
        let command = recipe.command.trim();
        if label.is_empty()
            || label.chars().count() > 60
            || label.chars().any(char::is_control)
            || command.is_empty()
            || command.len() > 2048
            || command.chars().any(char::is_control)
            || !files.iter().any(|f| f.path == recipe.source)
        {
            return Err("Agent 返回的命令缺少有效名称、来源或单行命令，请重新分析".into());
        }
        resolve_cwd(root, &recipe.cwd)?;
        if seen.insert((recipe.cwd.clone(), command.to_owned())) {
            result.push(RunCommand {
                id: uuid::Uuid::new_v4().to_string(),
                label: label.into(),
                command: command.into(),
                cwd: recipe.cwd,
                source: recipe.source,
                pinned: false,
                user_renamed: false,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_run_evidence_but_not_dependencies_secrets_or_symlinks() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"dev":"vite","prod":"vite --mode production"}}"#,
        )
        .unwrap();
        std::fs::write(root.path().join(".env"), "SECRET=do-not-read").unwrap();
        std::fs::create_dir(root.path().join("scripts")).unwrap();
        std::fs::write(
            root.path().join("scripts/deploy.sh"),
            "#!/bin/sh\necho deploy",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("node_modules")).unwrap();
        std::fs::write(root.path().join("node_modules/package.json"), "ignored").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("linked.sh")).unwrap();
        let files = scan(root.path()).unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["package.json", "scripts/deploy.sh"]
        );
    }
    #[test]
    fn validates_sources_directories_and_single_line_commands_and_deduplicates() {
        let root = tempfile::tempdir().unwrap();
        let files = vec![InputFile {
            path: "package.json".into(),
            text: "{}".into(),
            truncated: false,
            executable: None,
        }];
        let recipe = serde_json::json!({"label":"运行本地调试", "command":"npm run dev", "cwd":".", "source":"package.json"});
        let response = serde_json::json!({"commands":[recipe.clone(), recipe.clone()]}).to_string();
        let commands =
            parse_commands(&format!("```json\n{response}\n```"), root.path(), &files).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command, "npm run dev");
        for (field, value) in [
            ("cwd", "../"),
            ("source", "invented.sh"),
            ("command", "npm run dev\nrm x"),
            ("command", ""),
            ("label", ""),
        ] {
            let mut invalid = recipe.clone();
            invalid[field] = value.into();
            assert!(
                parse_commands(
                    &serde_json::json!({"commands":[invalid]}).to_string(),
                    root.path(),
                    &files
                )
                .is_err()
            );
        }
        assert!(
            parse_commands("{\"commands\":[]}", root.path(), &files)
                .unwrap()
                .is_empty()
        );
        assert!(resolve_cwd(root.path(), root.path().to_str().unwrap()).is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("outside")).unwrap();
            assert!(resolve_cwd(root.path(), "outside").is_err());
        }
    }
    #[test]
    fn caches_results_across_restart_and_ignores_a_superseded_analysis() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().into());
        let catalog = RunCatalog {
            project_id: "project".into(),
            platform: std::env::consts::OS.into(),
            shell: shell().into(),
            executor: None,
            generation: "new".into(),
            directories: vec![RunDirectory {
                path: "/repo".into(),
                status: "analyzing".into(),
                commands: vec![],
                error: None,
                user_modified: false,
            }],
        };
        host.cache
            .lock()
            .unwrap()
            .insert("project".into(), catalog.into());
        host.finish("project", "old", 0, Err("stale".into()));
        assert_eq!(
            host.load("project").unwrap().unwrap().directories[0].status,
            "analyzing"
        );
        host.finish(
            "project",
            "new",
            0,
            Ok(vec![RunCommand {
                id: "id".into(),
                label: "运行".into(),
                command: "npm run dev".into(),
                cwd: ".".into(),
                source: "package.json".into(),
                pinned: false,
                user_renamed: false,
            }]),
        );
        let restored = ProjectRunHost::new(root.path().into())
            .load("project")
            .unwrap()
            .unwrap();
        assert_eq!(restored.directories[0].commands[0].command, "npm run dev");
        assert_eq!(restored.directories[0].status, "ready");
    }
    fn editable_catalog() -> RunCatalog {
        serde_json::from_value(serde_json::json!({
            "projectId":"project", "platform":std::env::consts::OS, "shell":shell(), "generation":"v1", "executor":null,
            "directories":[{"path":"/repo", "status":"ready", "error":null, "commands":[
                {"id":"dev","label":"调试","command":"npm run dev","cwd":".","source":"package.json"},
                {"id":"build","label":"构建","command":"npm run build","cwd":".","source":"package.json"},
                {"id":"deploy","label":"部署","command":"sh deploy.sh","cwd":".","source":"deploy.sh"}
            ]}]
        })).unwrap()
    }
    #[test]
    fn saves_edits_and_requires_current_confirmation_then_preserves_only_pins() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().into());
        let original = editable_catalog();
        assert!(!has_user_edits(&original)); // old caches deserialize without new fields
        host.cache
            .lock()
            .unwrap()
            .insert("project".into(), original.clone().into());
        let renamed = host
            .edit(
                "project",
                "v1",
                0,
                "dev",
                RunEdit::Rename {
                    label: "我的调试".into(),
                },
            )
            .unwrap();
        assert!(
            host.edit("project", "v1", 0, "dev", RunEdit::Delete)
                .is_err()
        );
        let pinned = host
            .edit(
                "project",
                &renamed.generation,
                0,
                "dev",
                RunEdit::Pin { pinned: true },
            )
            .unwrap();
        let renamed = host
            .edit(
                "project",
                &pinned.generation,
                0,
                "build",
                RunEdit::Rename {
                    label: "我的构建".into(),
                },
            )
            .unwrap();
        let deleted = host
            .edit("project", &renamed.generation, 0, "deploy", RunEdit::Delete)
            .unwrap();
        let host = ProjectRunHost::new(root.path().into());
        let mut saved = host.load("project").unwrap().unwrap();
        assert_eq!(saved.directories[0].commands.len(), 2);
        assert_eq!(saved.directories[0].commands[0].label, "我的调试");
        assert!(saved.directories[0].commands[0].pinned);
        let paths = vec!["/repo".into()];
        assert_eq!(
            prepare_analysis(Some(&saved), &paths, None).unwrap_err(),
            CONFIRM_OVERWRITE
        );
        assert!(prepare_analysis(Some(&saved), &paths, Some(&pinned.generation)).is_err());
        saved.directories =
            prepare_analysis(Some(&saved), &paths, Some(&deleted.generation)).unwrap();
        host.cache
            .lock()
            .unwrap()
            .insert("project".into(), saved.clone());
        assert!(
            host.edit("project", &saved.generation, 0, "dev", RunEdit::Delete)
                .is_err()
        );
        host.finish(
            "project",
            &saved.generation,
            0,
            Err("analysis failed".into()),
        );
        let failed = host.load("project").unwrap().unwrap();
        assert!(has_user_edits(&failed));
        assert_eq!(failed.directories[0].commands[1].label, "我的构建");
        host.finish(
            "project",
            &saved.generation,
            0,
            Ok(original.directories[0].commands.clone()),
        );
        let refreshed = ProjectRunHost::new(root.path().into())
            .load("project")
            .unwrap()
            .unwrap();
        let commands = &refreshed.directories[0].commands;
        assert_eq!(commands.len(), 3); // the pinned command was not duplicated
        assert_eq!(commands[0].id, "dev");
        assert_eq!(commands[0].label, "我的调试");
        assert!(commands[0].pinned);
        assert_eq!(commands[1].label, "构建");
        assert_eq!(commands[2].id, "deploy"); // deletions can return only after consent
        // A preserved custom name must still prompt after the item is unpinned.
        assert!(has_user_edits(&refreshed));
        let unpinned = host
            .edit(
                "project",
                &refreshed.generation,
                0,
                "dev",
                RunEdit::Pin { pinned: false },
            )
            .unwrap();
        assert!(prepare_analysis(Some(&unpinned), &paths, None).is_err());
        host.finish(
            "project",
            &unpinned.generation,
            0,
            Ok(original.directories[0].commands.clone()),
        );
        assert!(!has_user_edits(&host.load("project").unwrap().unwrap()));
    }
    #[test]
    fn pinning_alone_needs_no_confirmation_and_empty_deleted_lists_still_do() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().into());
        host.cache
            .lock()
            .unwrap()
            .insert("project".into(), editable_catalog().into());
        let pinned = host
            .edit("project", "v1", 0, "deploy", RunEdit::Pin { pinned: true })
            .unwrap();
        assert_eq!(pinned.directories[0].commands[0].id, "deploy");
        assert!(!has_user_edits(&pinned));
        for label in ["", "\n", "line\nbreak"] {
            assert!(
                host.edit(
                    "project",
                    &pinned.generation,
                    0,
                    "dev",
                    RunEdit::Rename {
                        label: label.into()
                    }
                )
                .is_err()
            );
        }
        let mut catalog = pinned;
        for id in ["dev", "build", "deploy"] {
            catalog = host
                .edit("project", &catalog.generation, 0, id, RunEdit::Delete)
                .unwrap();
        }
        let saved = ProjectRunHost::new(root.path().into())
            .load("project")
            .unwrap()
            .unwrap();
        assert!(saved.directories[0].commands.is_empty());
        assert!(prepare_analysis(Some(&saved), &["/repo".into()], None).is_err());
    }
    #[test]
    fn project_runs_corrupt_cache_is_preserved_and_failed_edit_does_not_publish() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().into());
        std::fs::write(host.file("project"), b"broken").unwrap();
        assert!(host.load("project").unwrap_err().contains("已保留"));
        assert_eq!(std::fs::read(host.file("project")).unwrap(), b"broken");
        let blocked = root.path().join("blocked");
        std::fs::write(&blocked, b"keep").unwrap();
        let host = ProjectRunHost::new(blocked);
        host.cache
            .lock()
            .unwrap()
            .insert("project".into(), editable_catalog().into());
        assert!(
            host.edit(
                "project",
                "v1",
                0,
                "dev",
                RunEdit::Rename {
                    label: "new".into()
                }
            )
            .is_err()
        );
        let current = host.load("project").unwrap().unwrap();
        assert_eq!(current.generation, "v1");
        assert_ne!(current.directories[0].commands[0].label, "new");
    }

    #[test]
    fn project_runs_restart_marks_analysis_interrupted_and_preserves_user_commands() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().into());
        let mut catalog = editable_catalog();
        catalog.directories[0].status = "analyzing".into();
        catalog.directories[0].commands[0].pinned = true;
        catalog.directories[0].commands[0].label = "我的命令".into();
        catalog.directories[0].user_modified = true;
        host.save(&catalog.into()).unwrap();
        let restored = ProjectRunHost::new(root.path().into());
        let loaded = restored.load("project").unwrap().unwrap();
        assert_eq!(loaded.directories[0].status, "error");
        assert!(
            loaded.directories[0]
                .error
                .as_ref()
                .unwrap()
                .contains("中断")
        );
        assert_eq!(loaded.directories[0].commands[0].label, "我的命令");
        assert!(loaded.directories[0].commands[0].pinned);
        assert!(has_user_edits(&loaded));
        assert!(!restored.has_active());
    }

    #[tokio::test]
    async fn project_runs_queued_analysis_is_service_owned_and_duplicate_reads_coalesce() {
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().join("catalogs"));
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let project = ProjectSummary {
            project_id: ProjectId::new(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            display_name: "test".into(),
            task_count: 0,
            stream_version: 1,
            updated_at: None,
        };
        let permit = host.gate.acquire_many(2).await.unwrap();
        let (first, second) = tokio::join!(
            analyze(
                host.clone(),
                core.clone(),
                project.clone(),
                None,
                false,
                None
            ),
            analyze(
                host.clone(),
                core.clone(),
                project.clone(),
                None,
                false,
                None
            )
        );
        let first = first.unwrap();
        assert_eq!(first.generation, second.unwrap().generation);
        assert!(host.has_active());
        assert_eq!(host.active.load(std::sync::atomic::Ordering::Acquire), 1);
        drop(permit);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while host.has_active() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let result = host.load(project.project_id.as_str()).unwrap().unwrap();
        assert_eq!(result.directories[0].status, "error");
        assert!(
            result.directories[0]
                .error
                .as_ref()
                .unwrap()
                .contains("没有可用")
        );
        assert_eq!(
            analyze(host.clone(), core, project, None, false, None)
                .await
                .unwrap()
                .generation,
            first.generation
        );
        assert!(!host.has_active());
    }
    #[tokio::test]
    async fn project_runs_rejects_disabled_executor_before_spawning_analysis() {
        use driver_api::AgentDriver;
        let root = tempfile::tempdir().unwrap();
        let host = ProjectRunHost::new(root.path().join("catalogs"));
        let core = Arc::new(Core::new(Arc::new(
            agent_store::SqliteStore::open_in_memory().unwrap(),
        )));
        let driver = Arc::new(driver_api::FakeDriver::new("codex").unwrap());
        let executor = driver.executor().clone();
        core.register_driver(driver.clone()).await;
        let mut request = agent_protocol::CommandEnvelope::local(
            agent_protocol::CoreCommand::SetAgentPreferences(agent_protocol::AgentPreferences {
                order: vec!["codex".into()],
                disabled: vec!["codex".into()],
            }),
        );
        request.expected_stream_version = Some(0);
        core.execute(request).await.unwrap();
        let project = ProjectSummary {
            project_id: ProjectId::new(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
            display_name: "test".into(),
            task_count: 0,
            stream_version: 1,
            updated_at: None,
        };
        let error = analyze(host.clone(), core, project, Some(executor), true, None)
            .await
            .unwrap_err();
        assert!(error.contains("禁用"));
        assert!(!host.has_active());
        assert!(driver.received_requests().await.is_empty());
    }
}

#[cfg(test)]
mod operation_tests;
