//! Service-owned PTYs. Views never own or stop processes; output readers are
//! independent of subscribers. Launch intent is recorded before spawning.
mod input_stream;
mod launch_intents;
#[cfg(any(windows, test))]
mod startup;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const RUNNING_LIMIT: usize = 64;
const COMPLETED_OUTPUT_LIMIT: usize = 32;

/// Original recipe request, persisted alongside the resolved command before
/// spawning. Replays compare this request rather than today's recipe catalog.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectLaunchRequest {
    pub task_id: String,
    pub generation: String,
    pub directory_index: usize,
    pub command_id: String,
    pub cols: u16,
    pub rows: u16,
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchIntent {
    #[serde(default)]
    pub project_launch: Option<ProjectLaunchRequest>,
    pub operation_id: String,
    pub project_id: String,
    pub directory_index: usize,
    pub task_id: Option<String>,
    pub command_id: Option<String>,
    pub label: String,
    pub cwd: PathBuf,
    pub command: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl LaunchIntent {
    fn validate_binding(&self) -> Result<(), String> {
        if let Some(request) = &self.project_launch
            && (request.operation_id != self.operation_id
                || self.task_id.as_deref() != Some(request.task_id.as_str())
                || self.command_id.as_deref() != Some(request.command_id.as_str())
                || request.directory_index != self.directory_index
                || request.cols != self.cols
                || request.rows != self.rows
                || request.generation.is_empty()
                || self.command.is_none())
        {
            return Err("项目启动记录与原请求不一致，请核实原操作".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
    Unknown,
}
impl TerminalStatus {
    fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// A close is bound to the terminal observed by the caller. The current info is
/// returned unchanged on an epoch mismatch; callers must not treat it as stopped.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalStopRequest {
    pub terminal_id: String,
    pub expected_epoch: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub epoch: String,
    pub project_id: String,
    pub directory_index: usize,
    pub task_id: Option<String>,
    pub command_id: Option<String>,
    pub label: String,
    pub cwd: PathBuf,
    pub command: Option<String>,
    pub status: TerminalStatus,
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub output_end: u64,
    pub created_at: i64,
    /// Current PTY size. One terminal has one size shared by every attached
    /// client; the latest successful resize wins and is reported here.
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputBatch {
    pub info: TerminalInfo,
    pub from: u64,
    pub next: u64,
    pub gap: bool,
    pub bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    intent: LaunchIntent,
    info: TerminalInfo,
}
struct Output {
    info: TerminalInfo,
    first: u64,
    bytes: VecDeque<u8>,
}
struct Process {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
    armed: bool,
}
impl Process {
    fn stop(&mut self) -> Result<(), String> {
        if !self.armed {
            return Ok(());
        }
        #[cfg(unix)]
        for group in [
            self.master.process_group_leader(),
            self.pid.and_then(|pid| i32::try_from(pid).ok()),
        ]
        .into_iter()
        .flatten()
        {
            if group > 0 {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(group),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill.exe")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                // CREATE_NO_WINDOW: no console window flashes while killing the tree.
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        self.child.kill().map_err(|error| error.to_string())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Session {
    intent: LaunchIntent,
    output: Mutex<Output>,
    process: Mutex<Option<Process>>,
    input: Mutex<Option<Box<dyn Write + Send>>>,
    input_stream: Mutex<input_stream::State>,
    changed: tokio::sync::Notify,
    state_changes: Arc<crate::state_changes::Changes>,
}
impl Session {
    #[cfg(windows)]
    fn initialize_cursor(&self, directory: Option<&Path>) -> Result<(), String> {
        {
            let mut input = self.input.lock().map_err(|_| "终端输入不可用")?;
            let writer = input.as_mut().ok_or("终端输入已关闭")?;
            writer
                .write_all(b"\x1b[1;1R")
                .and_then(|()| writer.flush())
                .map_err(|error| format!("Windows 终端初始化失败：{error}"))?;
        }
        let mut output = self.output.lock().map_err(|_| "终端状态不可用")?;
        if output.info.status == TerminalStatus::Starting {
            output.info.status = TerminalStatus::Running;
            save(directory, &self.intent, &output.info)?;
        }
        drop(output);
        self.changed.notify_waiters();
        self.state_changes.changed();
        Ok(())
    }
    fn info(&self) -> Result<TerminalInfo, String> {
        Ok(self
            .output
            .lock()
            .map_err(|_| "终端状态不可用")?
            .info
            .clone())
    }
    fn append(&self, bytes: &[u8]) {
        if let Ok(mut output) = self.output.lock() {
            output.bytes.extend(bytes);
            output.info.output_end += bytes.len() as u64;
            let remove = output.bytes.len().saturating_sub(OUTPUT_LIMIT);
            output.bytes.drain(..remove);
            output.first += remove as u64;
        }
        self.changed.notify_waiters();
    }
    fn batch(&self, after: u64) -> Result<OutputBatch, String> {
        let output = self.output.lock().map_err(|_| "终端输出不可用")?;
        let from = after.clamp(output.first, output.info.output_end);
        let bytes: Vec<_> = output
            .bytes
            .iter()
            .skip(usize::try_from(from - output.first).unwrap_or(usize::MAX))
            .take(READ_LIMIT)
            .copied()
            .collect();
        Ok(OutputBatch {
            info: output.info.clone(),
            from,
            next: from + bytes.len() as u64,
            gap: from != after || output.info.status == TerminalStatus::Unknown,
            bytes,
        })
    }
}

fn reap_child(session: &Session) -> std::io::Result<portable_pty::ExitStatus> {
    loop {
        let ended = {
            let mut slot = session
                .process
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(process) = slot.as_mut() else {
                return Err(std::io::Error::other("terminal process missing"));
            };
            match process.child.try_wait() {
                Ok(None) => None,
                status => {
                    process.armed = false;
                    Some((slot.take(), status))
                }
            }
        };
        if let Some((process, status)) = ended {
            // Closing ConPTY may emit a final frame. On Windows this runs on
            // its own worker while the output thread continues draining it.
            drop(process);
            return status.and_then(|status| {
                status.ok_or_else(|| std::io::Error::other("terminal was not reaped"))
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;

fn release_completed_output(sessions: &HashMap<String, Arc<Session>>) {
    // Select the newest completed buffers with bounded auxiliary memory. Do not
    // clone full TerminalInfo/command payloads or sort every historical session.
    let mut retained = BTreeSet::new();
    for (id, session) in sessions {
        if let Ok(output) = session.output.lock()
            && !output.info.status.active()
        {
            retained.insert((output.info.created_at, id.as_str()));
            if retained.len() > COMPLETED_OUTPUT_LIMIT {
                retained.pop_first();
            }
        }
    }
    for (id, session) in sessions {
        if let Ok(mut output) = session.output.lock()
            && !output.info.status.active()
            && !retained.contains(&(output.info.created_at, id.as_str()))
        {
            // clear() retains the allocation forever because ended sessions stay
            // addressable for receipt checks. Drop the allocation, keep identity
            // and offsets, and let reconnects report the missing output range.
            output.bytes = VecDeque::new();
            output.first = output.info.output_end;
        }
    }
}

pub struct TerminalHost {
    epoch: String,
    directory: Option<PathBuf>,
    sessions: Sessions,
    retired: Mutex<BTreeSet<String>>,
    admission: tokio::sync::Mutex<()>,
    recovery_error: Option<String>,
    changes: Arc<crate::state_changes::Changes>,
}
impl Default for TerminalHost {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().to_string(),
            directory: None,
            sessions: Arc::default(),
            retired: Mutex::default(),
            admission: tokio::sync::Mutex::new(()),
            recovery_error: None,
            changes: Arc::new(crate::state_changes::Changes::new()),
        }
    }
}
impl TerminalHost {
    pub(crate) fn shutdown(&self) {
        if let Ok(sessions) = self.sessions.lock() {
            for session in sessions.values() {
                if let Ok(mut process) = session.process.lock()
                    && let Some(process) = process.as_mut()
                {
                    let _ = process.stop();
                }
            }
        }
    }
}

impl Drop for TerminalHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if !(2..=1000).contains(&cols) || !(1..=500).contains(&rows) {
        return Err("无效的终端尺寸".into());
    }
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}
impl TerminalHost {
    #[must_use]
    pub fn persistent(directory: PathBuf) -> Self {
        let mut host = Self::default();
        host.directory = Some(directory.clone());
        if let Err(error) = host.recover(&directory) {
            host.recovery_error =
                Some(format!("无法核实之前的终端记录，请保留数据后检查：{error}"));
        }
        host
    }
    pub(crate) fn with_changes(mut self, changes: Arc<crate::state_changes::Changes>) -> Self {
        self.changes = changes;
        if let Ok(mut sessions) = self.sessions.lock() {
            // Recovered sessions are immutable while the host is being constructed.
            for session in sessions.values_mut() {
                if let Some(session) = Arc::get_mut(session) {
                    session.state_changes = self.changes.clone();
                }
            }
        }
        self
    }
    fn recover(&self, directory: &Path) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        for file in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let file = file.map_err(|e| e.to_string())?;
            if file
                .path()
                .extension()
                .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let bytes = std::fs::read(file.path()).map_err(|e| e.to_string())?;
            let mut record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            uuid::Uuid::parse_str(&record.intent.operation_id)
                .map_err(|_| "invalid terminal operation identity")?;
            if record.info.id != record.intent.operation_id
                || file.path().file_stem().and_then(|s| s.to_str()) != Some(record.info.id.as_str())
            {
                return Err("terminal record identity mismatch".into());
            }
            record.intent.validate_binding()?;
            if record.info.status.active() {
                record.info.status = TerminalStatus::Unknown;
                record.info.error = Some(
                    "后台已重启，原进程状态无法确认；未自动重新执行，也不会按旧进程号操作。".into(),
                );
            }
            let session = Arc::new(Session {
                intent: record.intent,
                output: Mutex::new(Output {
                    first: record.info.output_end,
                    info: record.info,
                    bytes: VecDeque::new(),
                }),
                process: Mutex::new(None),
                input: Mutex::new(None),
                input_stream: Mutex::default(),
                changed: tokio::sync::Notify::new(),
                state_changes: self.changes.clone(),
            });
            self.sessions
                .lock()
                .map_err(|_| "终端状态不可用")?
                .insert(session.intent.operation_id.clone(), session);
        }
        Ok(())
    }
    fn session(&self, id: &str) -> Result<Arc<Session>, String> {
        self.sessions
            .lock()
            .map_err(|_| "终端状态不可用")?
            .get(id)
            .cloned()
            .ok_or_else(|| "终端不存在或属于已清除的后台实例".into())
    }
    pub fn has_active(&self) -> bool {
        self.sessions.lock().map_or(true, |sessions| {
            sessions
                .values()
                .any(|session| session.info().map_or(true, |info| info.status.active()))
        })
    }
    pub fn get(&self, id: &str) -> Result<TerminalInfo, String> {
        self.session(id)?.info()
    }
    pub fn list(&self, project_id: &str) -> Result<Vec<TerminalInfo>, String> {
        if let Some(error) = &self.recovery_error {
            return Err(error.clone());
        }
        let mut result: Vec<_> = self
            .sessions
            .lock()
            .map_err(|_| "终端状态不可用")?
            .values()
            .map(|session| session.info())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|info| info.project_id == project_id)
            .collect();
        result.sort_by_key(|info| std::cmp::Reverse(info.created_at));
        Ok(result)
    }
    fn find_session(&self, id: &str) -> Result<Option<Arc<Session>>, String> {
        if let Some(error) = &self.recovery_error {
            return Err(error.clone());
        }
        Ok(self
            .sessions
            .lock()
            .map_err(|_| "终端状态不可用")?
            .get(id)
            .cloned())
    }
    pub fn existing(&self, intent: &LaunchIntent) -> Result<Option<TerminalInfo>, String> {
        if self.launch_retired(&intent.operation_id)? {
            return Err("原启动已取消，未重新运行命令".into());
        }
        if let Some(session) = self.find_session(&intent.operation_id)? {
            if session.intent != *intent {
                return Err("终端操作标识已用于不同请求，请核实原操作".into());
            }
            return session.info().map(Some);
        }
        Ok(None)
    }
    pub fn existing_project_launch(
        &self,
        request: &ProjectLaunchRequest,
    ) -> Result<Option<TerminalInfo>, String> {
        uuid::Uuid::parse_str(&request.operation_id).map_err(|_| "无效的运行操作标识")?;
        if self.launch_retired(&request.operation_id)? {
            return Err("原启动已取消，未重新运行命令".into());
        }
        let Some(session) = self.find_session(&request.operation_id)? else {
            return Ok(None);
        };
        match &session.intent.project_launch {
            Some(original) if original == request => session.info().map(Some),
            Some(_) => Err("运行操作标识已用于不同请求，请核实原操作".into()),
            None => Err("该终端记录未绑定此项目启动请求，请查看原终端状态；未重新启动".into()),
        }
    }
    pub async fn start(&self, intent: LaunchIntent) -> Result<TerminalInfo, String> {
        if let Some(existing) = self.existing(&intent)? {
            return Ok(existing);
        }
        let shell = crate::terminal_installation::user_shell(
            &agent_registry::DiscoveryContext::from_current_process(),
        )
        .await;
        self.start_resolved(intent, shell).await
    }
    async fn start_resolved(
        &self,
        intent: LaunchIntent,
        shell: Option<PathBuf>,
    ) -> Result<TerminalInfo, String> {
        let _admission = self.admission.lock().await;
        if let Some(error) = &self.recovery_error {
            return Err(error.clone());
        }
        uuid::Uuid::parse_str(&intent.operation_id).map_err(|_| "无效的终端操作标识")?;
        let dimensions = size(intent.cols, intent.rows)?;
        intent.validate_binding()?;
        if let Some(existing) = self.existing(&intent)? {
            return Ok(existing);
        }
        if self
            .sessions
            .lock()
            .map_err(|_| "终端状态不可用")?
            .values()
            .filter(|session| session.info().is_ok_and(|info| info.status.active()))
            .count()
            >= RUNNING_LIMIT
        {
            return Err("运行中的终端过多，请先停止不再需要的终端".into());
        }
        if intent.project_id.is_empty()
            || intent.label.chars().count() > 200
            || intent
                .command
                .as_ref()
                .is_some_and(|command| command.len() > 128 * 1024)
        {
            return Err("无效的终端启动请求".into());
        }
        let cwd =
            std::fs::canonicalize(&intent.cwd).map_err(|e| format!("无法打开项目目录：{e}"))?;
        if !cwd.is_dir() {
            return Err("项目目录不存在".into());
        }
        let info = TerminalInfo {
            id: intent.operation_id.clone(),
            epoch: self.epoch.clone(),
            project_id: intent.project_id.clone(),
            directory_index: intent.directory_index,
            task_id: intent.task_id.clone(),
            command_id: intent.command_id.clone(),
            label: intent.label.clone(),
            cwd: cwd.clone(),
            command: intent.command.clone(),
            status: TerminalStatus::Starting,
            exit_code: None,
            error: None,
            output_end: 0,
            created_at: chrono::Utc::now().timestamp_millis(),
            cols: intent.cols,
            rows: intent.rows,
        };
        // A persisted intent prevents replaying a launch after a crash between
        // process creation and its reply. An old epoch is never started again.
        save(self.directory.as_deref(), &intent, &info)?;
        let session = Arc::new(Session {
            intent,
            output: Mutex::new(Output {
                info,
                first: 0,
                bytes: VecDeque::new(),
            }),
            process: Mutex::new(None),
            input: Mutex::new(None),
            input_stream: Mutex::default(),
            changed: tokio::sync::Notify::new(),
            state_changes: self.changes.clone(),
        });
        self.sessions
            .lock()
            .map_err(|_| "终端状态不可用")?
            .insert(session.intent.operation_id.clone(), session.clone());
        self.changes.changed();
        let spawning = session.clone();
        let directory = self.directory.clone();
        let sessions = self.sessions.clone();
        let result = tokio::task::spawn_blocking(move || {
            spawn(
                spawning,
                cwd,
                dimensions,
                shell.as_deref(),
                directory,
                sessions,
            )
        })
        .await;
        let failure = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some((TerminalStatus::Failed, error)),
            Err(_) => Some((
                TerminalStatus::Unknown,
                "终端启动结果无法确认，未自动重发".into(),
            )),
        };
        if let Some((status, error)) = failure {
            let mut output = session.output.lock().map_err(|_| "终端状态不可用")?;
            output.info.status = status;
            output.info.error = Some(error);
            save(self.directory.as_deref(), &session.intent, &output.info)?;
            drop(output);
            session.changed.notify_waiters();
            session.state_changes.changed();
        }
        session.info()
    }
    pub async fn read(&self, id: &str, after: u64) -> Result<OutputBatch, String> {
        let session = self.session(id)?;
        let changed = session.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let batch = session.batch(after)?;
        if !batch.bytes.is_empty() || batch.gap || !batch.info.status.active() {
            return Ok(batch);
        }
        let _ = tokio::time::timeout(Duration::from_secs(15), changed).await;
        session.batch(after)
    }
    #[cfg(test)]
    pub async fn write(&self, id: &str, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > READ_LIMIT {
            return Err("单次终端输入过长".into());
        }
        let session = self.session(id)?;
        tokio::task::spawn_blocking(move || {
            if session.info()?.status != TerminalStatus::Running {
                return Err("终端未运行，未重放输入".into());
            }
            // Linux may leave a blocking PTY write asleep even after the slave
            // dies. Nonblocking writes must observe stop between partial writes.
            let mut input = session.input.lock().map_err(|_| "终端输入不可用")?;
            let writer = input.as_mut().ok_or("终端未运行，未重放输入")?;
            let mut remaining = bytes.as_slice();
            while !remaining.is_empty() {
                if session.info()?.status != TerminalStatus::Running {
                    return Err("终端已停止，输入结果无法确认，未重放输入".into());
                }
                match writer.write(remaining) {
                    Ok(0) => return Err("终端输入未完成，未重放输入".into()),
                    Ok(count) => remaining = &remaining[count..],
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(format!("终端输入结果无法确认，未重放输入：{error}")),
                }
            }
            writer
                .flush()
                .map_err(|error| format!("终端输入结果无法确认，未重放输入：{error}"))
        })
        .await
        .map_err(|_| "终端输入结果无法确认，未重放输入")?
    }
    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let dimensions = size(cols, rows)?;
        let session = self.session(id)?;
        let resized = session.clone();
        tokio::task::spawn_blocking(move || {
            resized
                .process
                .lock()
                .map_err(|_| "终端不可用")?
                .as_mut()
                .ok_or("终端未运行")?
                .master
                .resize(dimensions)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "终端尺寸更新失败")??;
        if let Ok(mut output) = session.output.lock() {
            output.info.cols = cols;
            output.info.rows = rows;
        }
        session.changed.notify_waiters();
        Ok(())
    }
    pub async fn stop(&self, id: &str, expected_epoch: &str) -> Result<TerminalInfo, String> {
        let _admission = self.admission.lock().await;
        self.stop_admitted(id, expected_epoch).await
    }
    async fn stop_admitted(&self, id: &str, expected_epoch: &str) -> Result<TerminalInfo, String> {
        let session = self.session(id)?;
        let info = session.info()?;
        if info.epoch != expected_epoch {
            return Ok(info);
        }
        if info.status == TerminalStatus::Unknown {
            return Err("后台已重启，无法确认原进程身份；未按旧进程号停止进程".into());
        }
        if !info.status.active() {
            return Ok(info);
        }
        let stopping = session.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut process = stopping.process.lock().map_err(|_| "终端不可用")?;
            // A concurrent reaper may have removed the process after the status
            // query. Never overwrite its terminal status with Stopping.
            if let Some(process) = process.as_mut() {
                stopping
                    .output
                    .lock()
                    .map_err(|_| "终端状态不可用")?
                    .info
                    .status = TerminalStatus::Stopping;
                process.stop()?;
            }
            Ok(())
        })
        .await
        .map_err(|_| "停止结果无法确认，请核实终端状态")??;
        session.changed.notify_waiters();
        session.state_changes.changed();
        session.info()
    }
}

fn save(
    directory: Option<&Path>,
    intent: &LaunchIntent,
    info: &TerminalInfo,
) -> Result<(), String> {
    let Some(directory) = directory else {
        return Ok(());
    };
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    serde_json::to_writer(
        &mut file,
        &Record {
            intent: intent.clone(),
            info: info.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    file.as_file_mut().sync_all().map_err(|e| e.to_string())?;
    file.persist(directory.join(format!("{}.json", info.id)))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn command(text: Option<&str>, shell: Option<&Path>, cwd: &Path) -> CommandBuilder {
    let Some(text) = text else {
        return CommandBuilder::new_default_prog();
    };
    #[cfg(not(windows))]
    if let Some(shell) = shell.filter(|path| {
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("zsh" | "bash" | "sh" | "fish")
        )
    }) {
        let mut command = CommandBuilder::new(shell);
        command.args([
            "-lic",
            "exec /bin/sh -c 'cd \"$DOGCODES_RUN_CWD\" && eval \"$DOGCODES_RUN_COMMAND\"'",
        ]);
        command.env("DOGCODES_RUN_CWD", cwd);
        command.env("DOGCODES_RUN_COMMAND", text);
        return command;
    }
    #[cfg(windows)]
    let mut command = {
        let _ = (shell, cwd);
        let mut value = CommandBuilder::new("powershell.exe");
        value.args(["-NoLogo", "-NoProfile", "-Command"]);
        value
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut value = CommandBuilder::new("/bin/sh");
        value.arg("-lc");
        value
    };
    command.arg(text);
    command
}
fn spawn(
    session: Arc<Session>,
    cwd: PathBuf,
    dimensions: PtySize,
    shell: Option<&Path>,
    directory: Option<PathBuf>,
    sessions: Sessions,
) -> Result<(), String> {
    let pair = native_pty_system()
        .openpty(dimensions)
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        use nix::fcntl::{FcntlArg, OFlag, fcntl};
        let fd = pair.master.as_raw_fd().ok_or("终端不支持可中断输入")?;
        let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(|e| e.to_string())?;
        fcntl(
            fd,
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(|e| e.to_string())?;
    }
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let mut command = command(session.intent.command.as_deref(), shell, &cwd);
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut process = Process {
        master: pair.master,
        pid: child.process_id(),
        child,
        armed: true,
    };
    // If recording the successful spawn fails, terminate and reap this exact
    // child before reporting failure; never leave an unrecorded process running.
    let setup = (|| {
        let mut output = session.output.lock().map_err(|_| "终端状态不可用")?;
        // ConPTY inherits its initial cursor through an output/input handshake.
        // Keep writes gated until the service answers it, even with no view.
        output.info.status = if cfg!(windows) {
            TerminalStatus::Starting
        } else {
            TerminalStatus::Running
        };
        save(directory.as_deref(), &session.intent, &output.info)
    })();
    if let Err(error) = setup {
        let _ = process.stop();
        let _ = process.child.wait();
        process.armed = false;
        return Err(error);
    }
    *session.input.lock().map_err(|_| "终端输入不可用")? = Some(writer);
    *session.process.lock().map_err(|_| "终端状态不可用")? = Some(process);
    session.state_changes.changed();
    let reading = session.clone();
    #[cfg(windows)]
    let (exit_send, exit_receive) =
        std::sync::mpsc::channel::<std::io::Result<portable_pty::ExitStatus>>();
    let thread = std::thread::Builder::new()
        .name("terminal-output".into())
        .spawn(move || {
            let session = reading;
            let mut buffer = [0; 8192];
            #[cfg(windows)]
            let mut cursor = startup::StartupCursor::default();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        #[cfg(not(windows))]
                        session.append(&buffer[..count]);
                        #[cfg(windows)]
                        {
                            let (visible, respond) = cursor.consume(&buffer[..count]);
                            if respond
                                && let Err(error) = session.initialize_cursor(directory.as_deref())
                            {
                                if let Ok(mut output) = session.output.lock() {
                                    output.info.error = Some(error);
                                }
                                if let Ok(mut process) = session.process.lock()
                                    && let Some(process) = process.as_mut()
                                {
                                    let _ = process.stop();
                                }
                                break;
                            }
                            if !visible.is_empty() {
                                session.append(&visible);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            // Process identity is disarmed under the same lock as stop. On
            // Windows, child exit closes ConPTY before its output reaches EOF.
            #[cfg(windows)]
            let status = exit_receive
                .recv()
                .unwrap_or_else(|_| Err(std::io::Error::other("terminal reaper unavailable")));
            #[cfg(not(windows))]
            let status = reap_child(&session);
            if let Ok(mut output) = session.output.lock() {
                output.info.status = TerminalStatus::Exited;
                match status {
                    Ok(status) => output.info.exit_code = Some(status.exit_code()),
                    Err(error) => {
                        output.info.status = TerminalStatus::Unknown;
                        output.info.error = Some(format!("无法核实退出结果：{error}"));
                    }
                }
                if let Err(error) = save(directory.as_deref(), &session.intent, &output.info) {
                    output.info.error = Some(format!("进程已结束，但状态保存失败：{error}"));
                }
            }
            // Publish exit before waiting for input: a nonblocking writer must
            // see that the child died even when no explicit stop was requested.
            if let Ok(mut input) = session.input.lock() {
                input.take();
            }
            session.changed.notify_waiters();
            session.state_changes.changed();
            // Keep a bounded amount of completed output in memory. Metadata remains
            // available, and reconnects explicitly report evicted byte ranges.
            if let Ok(sessions) = sessions.lock() {
                release_completed_output(&sessions);
            }
        });
    if let Err(error) = thread {
        let mut process = session
            .process
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mut process) = process.take() {
            let _ = process.stop();
            let _ = process.child.wait();
            process.armed = false;
        }
        if let Ok(mut input) = session.input.lock() {
            input.take();
        }
        return Err(format!("无法读取终端输出：{error}"));
    }
    #[cfg(windows)]
    {
        let waiting = session.clone();
        // The service owns this worker until child exit; views never own it.
        drop(tokio::task::spawn_blocking(move || {
            let _ = exit_send.send(reap_child(&waiting));
        }));
    }
    Ok(())
}

pub mod rpc;

#[cfg(test)]
mod tests;
