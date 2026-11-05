//! One operation on a project's machine, carried over a pair of byte streams as lines
//! of JSON. The asking side sends the operation, then answers any questions the
//! operation has about tasks. The machine holding the files never sees the database.
use crate::{
    MutationGuard, ReviewFacts, VerifiedArtifact, project_scan,
    review::{ReviewHost, service::ReviewCall},
    workspace_service::{DirectoryRequest, watch::WatchService},
};
use agent_domain::{ArtifactId, ExecutorRef, RunId, TaskId};
use agent_protocol::{
    ArtifactCursor, ArtifactPage, BootstrapSnapshot, WorkspaceCollectionDiagnostic,
    WorkspaceDiagnosticKind,
    workspace_context::{
        WorkspaceContextProvider, WorkspaceContextSnapshot, WorkspaceDiffCapture,
        WorkspaceTurnCapture,
    },
};
use agent_runtime::SharedTarget;
use agent_workspace::GitWorkspaceContextProvider;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};

type PendingAnswers = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

const LOST: &str = "与服务器的连接已断开，这项操作的结果未知，请重新连接后核实";

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum Operation {
    Review(ReviewCall),
    Workspace { method: String, args: Value },
    WatchDirectory(DirectoryRequest),
    ScanProject { root: String },
    Context(ContextCall),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum ContextCall {
    ForkSnapshot { paths: Vec<String> },
    ForkWorktrees { snapshot: Value, name: String },
    DeleteTask { task: String },
    ValidateRoots { directories: Vec<String> },
    RecoverTurn { path: String, checkpoint: Value },
    BeginTurn { path: String },
    Checkpoint { capture: String },
    Finish { capture: String },
    Collect { path: String },
    CaptureDiff { path: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
enum Ask {
    Bootstrap,
    TryMutation,
    TaskArtifacts {
        task: TaskId,
        before: Option<ArtifactCursor>,
        limit: usize,
    },
    VerifiedArtifact {
        task: TaskId,
        artifact: ArtifactId,
        sha256: String,
    },
    SummarizeCommit {
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    },
    ReviewRunExists {
        task: TaskId,
        run: RunId,
    },
    StartProjectReview {
        request: Value,
        input: Option<Value>,
    },
    GitCredential {
        task: TaskId,
        prompt: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FromMachine {
    Ask { id: u64, ask: Ask },
    Event(Value),
    Done(Result<Value, String>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FromAsker {
    Answer {
        id: u64,
        answer: Result<Value, String>,
    },
}

async fn write_line<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &impl Serialize,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    writer.write_all(&line).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}

/// `None` is the end of the stream.
async fn read_line<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut BufReader<R>,
) -> Result<Option<T>, String> {
    let mut line = Vec::new();
    let count = reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    serde_json::from_slice(&line)
        .map(Some)
        .map_err(|e| e.to_string())
}

// ---- The machine holding the files ----

/// Everything that outlives one operation on that machine.
pub struct Machine {
    review: ReviewHost,
    watches: WatchService,
    context: GitWorkspaceContextProvider,
    captures: Mutex<HashMap<String, Arc<dyn WorkspaceTurnCapture>>>,
    next_capture: AtomicU64,
}

impl Machine {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        let review = directory.join("review");
        Self {
            context: GitWorkspaceContextProvider::default().with_review_directory(review.clone()),
            review: ReviewHost::new(review),
            watches: WatchService::default(),
            captures: Mutex::default(),
            next_capture: AtomicU64::new(1),
        }
    }

    async fn context(&self, call: ContextCall) -> Result<Value, String> {
        fn value<T: Serialize>(result: T) -> Result<Value, String> {
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        let capture = |id: &str| {
            self.captures
                .lock()
                .map_err(|_| "capture state unavailable".to_owned())
                .map(|captures| captures.get(id).cloned())
        };
        match call {
            ContextCall::ForkSnapshot { paths } => {
                value(self.context.capture_fork_snapshot(&paths).await?)
            }
            ContextCall::ForkWorktrees { snapshot, name } => {
                value(self.context.fork_worktrees(snapshot, &name).await?)
            }
            ContextCall::DeleteTask { task } => {
                self.review.forget_task(&task)?;
                value(self.context.delete_task_review_data(&task).await?)
            }
            ContextCall::ValidateRoots { directories } => {
                value(self.context.validate_workspace_roots(&directories).await)
            }
            ContextCall::RecoverTurn { path, checkpoint } => {
                value(self.context.recover_turn(&path, checkpoint).await)
            }
            ContextCall::BeginTurn { path } => {
                let begun = self.context.begin_turn(&path).await;
                value(begun.and_then(|capture| {
                    let id = self
                        .next_capture
                        .fetch_add(1, Ordering::Relaxed)
                        .to_string();
                    self.captures
                        .lock()
                        .map_err(|_| unavailable())?
                        .insert(id.clone(), Arc::from(capture));
                    Ok(id)
                }))
            }
            ContextCall::Checkpoint { capture: id } => {
                let found = capture(&id)?.ok_or_else(|| "capture is gone".to_owned())?;
                value(found.checkpoint().await)
            }
            ContextCall::Finish { capture: id } => {
                let found = capture(&id)?.ok_or_else(|| "capture is gone".to_owned())?;
                let result = found.finish().await;
                if let Ok(mut captures) = self.captures.lock() {
                    captures.remove(&id);
                }
                value(result)
            }
            ContextCall::Collect { path } => value(self.context.collect(&path).await),
            ContextCall::CaptureDiff { path } => value(self.context.capture_diff(&path).await),
        }
    }
}

fn unavailable() -> WorkspaceCollectionDiagnostic {
    WorkspaceCollectionDiagnostic {
        kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
    }
}

/// Answers from the far side of the operation's streams. The mutation guard lives
/// over there, for as long as this operation runs.
struct AskingFacts {
    out: mpsc::UnboundedSender<FromMachine>,
    pending: PendingAnswers,
    next: AtomicU64,
}

impl AskingFacts {
    async fn ask<T: DeserializeOwned>(&self, ask: Ask) -> Result<T, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| LOST.to_owned())?
            .insert(id, reply);
        self.out
            .send(FromMachine::Ask { id, ask })
            .map_err(|_| LOST.to_owned())?;
        let value = answer.await.map_err(|_| LOST.to_owned())??;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

#[async_trait]
impl ReviewFacts for AskingFacts {
    fn on_hub(&self) -> bool {
        false
    }
    async fn bootstrap(&self) -> Result<BootstrapSnapshot, String> {
        self.ask(Ask::Bootstrap).await
    }
    async fn try_workspace_mutation(&self) -> Result<MutationGuard, String> {
        self.ask::<Value>(Ask::TryMutation).await?;
        Ok(Box::new(()))
    }
    async fn task_artifacts(
        &self,
        task: &TaskId,
        before: Option<&ArtifactCursor>,
        limit: usize,
    ) -> Result<ArtifactPage, String> {
        self.ask(Ask::TaskArtifacts {
            task: task.clone(),
            before: before.cloned(),
            limit,
        })
        .await
    }
    async fn verified_artifact(
        &self,
        task: &TaskId,
        artifact: &ArtifactId,
        sha256: &str,
    ) -> Result<VerifiedArtifact, String> {
        self.ask(Ask::VerifiedArtifact {
            task: task.clone(),
            artifact: artifact.clone(),
            sha256: sha256.to_owned(),
        })
        .await
    }
    async fn summarize_commit(
        &self,
        executor: ExecutorRef,
        changes: String,
        language: Option<String>,
    ) -> Result<String, String> {
        self.ask(Ask::SummarizeCommit {
            executor,
            changes,
            language,
        })
        .await
    }
    async fn ask_git_credential(
        &self,
        task: &TaskId,
        prompt: &str,
    ) -> Result<Option<String>, String> {
        self.ask(Ask::GitCredential {
            task: task.clone(),
            prompt: prompt.to_owned(),
        })
        .await
    }
    async fn review_run_exists(&self, task: &TaskId, run: &RunId) -> Result<bool, String> {
        self.ask(Ask::ReviewRunExists {
            task: task.clone(),
            run: run.clone(),
        })
        .await
    }
    async fn start_project_review(
        &self,
        request: Value,
        input: Option<Value>,
    ) -> Result<Value, String> {
        self.ask(Ask::StartProjectReview { request, input }).await
    }
}

/// Runs one operation to its end. Dropping the future abandons it.
pub async fn serve<R, W>(machine: Arc<Machine>, reader: R, mut writer: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let operation: Operation = match read_line(&mut reader).await {
        Ok(Some(operation)) => operation,
        Ok(None) => return,
        Err(error) => {
            let _ = write_line(&mut writer, &FromMachine::Done(Err(error))).await;
            return;
        }
    };
    let (out, mut outgoing) = mpsc::unbounded_channel();
    let pending: PendingAnswers = Arc::default();
    let answers = pending.clone();
    let listener = tokio::spawn(async move {
        while let Ok(Some(FromAsker::Answer { id, answer })) = read_line(&mut reader).await {
            if let Some(reply) = answers.lock().ok().and_then(|mut all| all.remove(&id)) {
                let _ = reply.send(answer);
            }
        }
        // Nobody is left to answer: every open question fails rather than hangs.
        if let Ok(mut all) = answers.lock() {
            all.clear();
        }
    });
    let facts = Arc::new(AskingFacts {
        out: out.clone(),
        pending,
        next: AtomicU64::new(1),
    });
    let sender = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if write_line(&mut writer, &message).await.is_err() {
                return;
            }
        }
    });
    let result = run(&machine, facts.clone(), operation, &out).await;
    let _ = out.send(FromMachine::Done(result));
    drop(out);
    drop(facts);
    let _ = sender.await;
    listener.abort();
}

async fn run(
    machine: &Arc<Machine>,
    facts: Arc<AskingFacts>,
    operation: Operation,
    out: &mpsc::UnboundedSender<FromMachine>,
) -> Result<Value, String> {
    match operation {
        Operation::Review(call) => {
            crate::review::service::dispatch(facts.as_ref(), &machine.review, call).await
        }
        Operation::Workspace { method, args } => {
            crate::workspace_service::dispatch(facts.as_ref(), &method, args).await
        }
        Operation::WatchDirectory(request) => {
            let mut subscription = machine.watches.open(facts, request).await?;
            out.send(FromMachine::Event(json!("ready")))
                .map_err(|_| LOST.to_owned())?;
            loop {
                let message = subscription.next().await?;
                let message = serde_json::to_value(message).map_err(|e| e.to_string())?;
                out.send(FromMachine::Event(message))
                    .map_err(|_| LOST.to_owned())?;
            }
        }
        Operation::ScanProject { root } => {
            let files = tokio::task::spawn_blocking(move || {
                project_scan::scan(std::path::Path::new(&root))
            })
            .await
            .map_err(|e| e.to_string())??;
            serde_json::to_value(files).map_err(|e| e.to_string())
        }
        Operation::Context(call) => machine.context(call).await,
    }
}

// ---- The side that asks ----

async fn answer(
    facts: &dyn ReviewFacts,
    ask: Ask,
    guards: &mut Vec<MutationGuard>,
) -> Result<Value, String> {
    fn value<T: Serialize>(result: T) -> Result<Value, String> {
        serde_json::to_value(result).map_err(|e| e.to_string())
    }
    match ask {
        Ask::Bootstrap => value(facts.bootstrap().await?),
        Ask::TryMutation => {
            guards.push(facts.try_workspace_mutation().await?);
            Ok(Value::Null)
        }
        Ask::TaskArtifacts {
            task,
            before,
            limit,
        } => value(facts.task_artifacts(&task, before.as_ref(), limit).await?),
        Ask::VerifiedArtifact {
            task,
            artifact,
            sha256,
        } => value(facts.verified_artifact(&task, &artifact, &sha256).await?),
        Ask::SummarizeCommit {
            executor,
            changes,
            language,
        } => value(facts.summarize_commit(executor, changes, language).await?),
        Ask::ReviewRunExists { task, run } => value(facts.review_run_exists(&task, &run).await?),
        Ask::StartProjectReview { request, input } => {
            facts.start_project_review(request, input).await
        }
        Ask::GitCredential { task, prompt } => {
            value(facts.ask_git_credential(&task, &prompt).await?)
        }
    }
}

/// An operation running on another machine. Dropping it stops the operation.
pub struct RunningOperation {
    child: agent_runtime::OwnedChild,
    input: agent_runtime::ProcessInput,
    output: BufReader<agent_runtime::ProcessStream>,
    guards: Vec<MutationGuard>,
}

impl RunningOperation {
    pub async fn start(target: &SharedTarget, operation: &Operation) -> Result<Self, String> {
        let mut child = target
            .spawn_operation()
            .map_err(|_| "这个项目所在的服务器未连接，请先连接后重试".to_owned())?;
        let (Some(mut input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(LOST.to_owned());
        };
        write_line(&mut input, operation).await?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
            guards: Vec::new(),
        })
    }

    /// The next thing the operation reports, answering its questions meanwhile.
    /// `Ok(None)` is its final result having been taken by [`finish`].
    async fn advance(&mut self, facts: &dyn ReviewFacts) -> Result<Step, String> {
        loop {
            let message: Option<FromMachine> = read_line(&mut self.output).await?;
            match message.ok_or_else(|| LOST.to_owned())? {
                FromMachine::Ask { id, ask } => {
                    let answer = answer(facts, ask, &mut self.guards).await;
                    write_line(&mut self.input, &FromAsker::Answer { id, answer }).await?;
                }
                FromMachine::Event(event) => return Ok(Step::Event(event)),
                FromMachine::Done(result) => return result.map(Step::Done),
            }
        }
    }

    /// The next event of a streaming operation.
    pub async fn next_event(&mut self, facts: &dyn ReviewFacts) -> Result<Value, String> {
        match self.advance(facts).await? {
            Step::Event(event) => Ok(event),
            Step::Done(_) => Err("目录监听已结束".to_owned()),
        }
    }

    pub async fn finish(mut self, facts: &dyn ReviewFacts) -> Result<Value, String> {
        let result = loop {
            if let Step::Done(value) = self.advance(facts).await? {
                break value;
            }
        };
        let _ = self.child.wait().await;
        Ok(result)
    }
}

enum Step {
    Event(Value),
    Done(Value),
}

pub async fn call(
    target: &SharedTarget,
    facts: &dyn ReviewFacts,
    operation: &Operation,
) -> Result<Value, String> {
    RunningOperation::start(target, operation)
        .await?
        .finish(facts)
        .await
}

pub async fn scan_project(
    target: &SharedTarget,
    root: &str,
) -> Result<Vec<project_scan::InputFile>, String> {
    let operation = Operation::ScanProject {
        root: root.to_owned(),
    };
    let value = call(target, &NoFacts, &operation).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// For operations that never ask anything about tasks.
struct NoFacts;

#[async_trait]
impl ReviewFacts for NoFacts {
    fn on_hub(&self) -> bool {
        true
    }
    async fn bootstrap(&self) -> Result<BootstrapSnapshot, String> {
        Err("not available here".into())
    }
    async fn try_workspace_mutation(&self) -> Result<MutationGuard, String> {
        Err("not available here".into())
    }
    async fn task_artifacts(
        &self,
        _task: &TaskId,
        _before: Option<&ArtifactCursor>,
        _limit: usize,
    ) -> Result<ArtifactPage, String> {
        Err("not available here".into())
    }
    async fn verified_artifact(
        &self,
        _task: &TaskId,
        _artifact: &ArtifactId,
        _sha256: &str,
    ) -> Result<VerifiedArtifact, String> {
        Err("not available here".into())
    }
    async fn summarize_commit(
        &self,
        _executor: ExecutorRef,
        _changes: String,
        _language: Option<String>,
    ) -> Result<String, String> {
        Err("not available here".into())
    }
    async fn review_run_exists(&self, _task: &TaskId, _run: &RunId) -> Result<bool, String> {
        Err("not available here".into())
    }
    async fn start_project_review(
        &self,
        _request: Value,
        _input: Option<Value>,
    ) -> Result<Value, String> {
        Err("not available here".into())
    }
}

/// The working directories of a project on another machine, as the core sees them.
pub struct RemoteWorkspace {
    target: SharedTarget,
}

impl RemoteWorkspace {
    #[must_use]
    pub fn new(target: SharedTarget) -> Self {
        Self { target }
    }

    async fn context(&self, call: ContextCall) -> Result<Value, String> {
        self::call(&self.target, &NoFacts, &Operation::Context(call)).await
    }

    async fn diagnosed<T: DeserializeOwned>(
        &self,
        call: ContextCall,
    ) -> Result<T, WorkspaceCollectionDiagnostic> {
        let value = self.context(call).await.map_err(|_| unavailable())?;
        serde_json::from_value::<Result<T, WorkspaceCollectionDiagnostic>>(value)
            .map_err(|_| unavailable())?
    }
}

struct RemoteCapture {
    target: SharedTarget,
    capture: String,
}

impl RemoteCapture {
    async fn call<T: DeserializeOwned>(
        &self,
        call: ContextCall,
    ) -> Result<T, WorkspaceCollectionDiagnostic> {
        let value = self::call(&self.target, &NoFacts, &Operation::Context(call))
            .await
            .map_err(|_| unavailable())?;
        serde_json::from_value::<Result<T, WorkspaceCollectionDiagnostic>>(value)
            .map_err(|_| unavailable())?
    }
}

#[async_trait]
impl WorkspaceTurnCapture for RemoteCapture {
    async fn checkpoint(&self) -> Result<Option<Value>, WorkspaceCollectionDiagnostic> {
        self.call(ContextCall::Checkpoint {
            capture: self.capture.clone(),
        })
        .await
    }
    async fn finish(&self) -> Result<Value, WorkspaceCollectionDiagnostic> {
        self.call(ContextCall::Finish {
            capture: self.capture.clone(),
        })
        .await
    }
}

#[async_trait]
impl WorkspaceContextProvider for RemoteWorkspace {
    async fn capture_fork_snapshot(&self, paths: &[String]) -> Result<Value, String> {
        self.context(ContextCall::ForkSnapshot {
            paths: paths.to_vec(),
        })
        .await
    }
    async fn fork_worktrees(&self, snapshot: Value, name: &str) -> Result<Vec<String>, String> {
        let value = self
            .context(ContextCall::ForkWorktrees {
                snapshot,
                name: name.to_owned(),
            })
            .await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
    async fn delete_task_review_data(&self, task_id: &str) -> Result<(), String> {
        self.context(ContextCall::DeleteTask {
            task: task_id.to_owned(),
        })
        .await
        .map(|_| ())
    }
    async fn validate_workspace_roots(
        &self,
        directories: &[String],
    ) -> Result<(), WorkspaceCollectionDiagnostic> {
        self.diagnosed(ContextCall::ValidateRoots {
            directories: directories.to_vec(),
        })
        .await
    }
    async fn recover_turn(
        &self,
        workspace_path: &str,
        checkpoint: Value,
    ) -> Result<Value, WorkspaceCollectionDiagnostic> {
        self.diagnosed(ContextCall::RecoverTurn {
            path: workspace_path.to_owned(),
            checkpoint,
        })
        .await
    }
    async fn begin_turn(
        &self,
        workspace_path: &str,
    ) -> Result<Box<dyn WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
        let capture: String = self
            .diagnosed(ContextCall::BeginTurn {
                path: workspace_path.to_owned(),
            })
            .await?;
        Ok(Box::new(RemoteCapture {
            target: self.target.clone(),
            capture,
        }))
    }
    async fn collect(&self, workspace_path: &str) -> WorkspaceContextSnapshot {
        match self
            .context(ContextCall::Collect {
                path: workspace_path.to_owned(),
            })
            .await
            .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
        {
            Ok(snapshot) => snapshot,
            Err(_) => WorkspaceContextSnapshot {
                diagnostic: Some(unavailable()),
                ..WorkspaceContextSnapshot::default()
            },
        }
    }
    async fn capture_diff(
        &self,
        workspace_path: &str,
    ) -> Result<WorkspaceDiffCapture, WorkspaceCollectionDiagnostic> {
        self.diagnosed(ContextCall::CaptureDiff {
            path: workspace_path.to_owned(),
        })
        .await
    }
}
