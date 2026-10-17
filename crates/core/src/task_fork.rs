//! Fork retained history at a terminal turn boundary into an independent session.
use crate::{AppendRequest, Core, CoreError, TurnWorkspaceScope};
use agent_domain::{CommandId, ProjectId, TaskId, TurnId};
use agent_protocol::{ArtifactMetadata, EventSource, NewEvent, StreamRef, StreamType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SNAPSHOT_MEDIA: &str = "application/vnd.dogcodes.fork-snapshot.v1+json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForkEnvironment {
    Workspace,
    Worktree,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkRequest {
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub new_task_id: TaskId,
    pub environment: ForkEnvironment,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkReceipt {
    pub request: ForkRequest,
    pub task_available: bool,
    pub completed: bool,
    pub status: String,
    pub stage: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForkPlan {
    source: agent_protocol::TaskSummary,
    project: agent_protocol::ProjectSummary,
    through: u64,
    messages: Vec<Value>,
    snapshot: Option<Value>,
    snapshot_error: Option<String>,
}

struct ActiveFork(
    std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<TaskId, TaskId>>>,
    TaskId,
);
impl Drop for ActiveFork {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.lock() {
            active.remove(&self.1);
        }
    }
}

impl Core {
    pub(crate) fn fork_source_active(&self, source: &TaskId) -> bool {
        self.active_forks
            .lock()
            .map_or(true, |active| active.values().any(|task| task == source))
    }

    async fn fork_intent(&self, target: &TaskId) -> Result<Option<ForkRequest>, CoreError> {
        let Some(ack) = self
            .repository
            .command_ack(&format!("fork:{target}:intent"))
            .await?
        else {
            return Ok(None);
        };
        let sequence = ack
            .first_sequence
            .filter(|seq| *seq > 0 && Some(*seq) == ack.last_sequence)
            .ok_or_else(|| CoreError::Internal("Fork 原请求范围损坏".into()))?;
        let events = self.repository.sync_events(sequence - 1, 1).await?;
        let event = events
            .first()
            .filter(|event| event.sequence == sequence && event.event_type == "task.fork_requested")
            .ok_or_else(|| CoreError::NotFound("Fork 原请求记录不可用，未重试".into()))?;
        let request: ForkRequest = serde_json::from_value(event.payload["request"].clone())
            .map_err(|_| CoreError::Internal("Fork 原请求记录损坏".into()))?;
        if &request.new_task_id != target || event.stream.stream_id != request.task_id.as_str() {
            return Err(CoreError::Conflict("Fork 原请求归属不匹配".into()));
        }
        Ok(Some(request))
    }

    /// A missing final receipt is not evidence that no checkout was created.
    /// This query never creates or repairs a worktree.
    pub async fn fork_receipt(
        &self,
        source: &TaskId,
        target: &TaskId,
    ) -> Result<Option<ForkReceipt>, CoreError> {
        self.require_task(source).await?;
        let intent = self.fork_intent(target).await?;
        if intent
            .as_ref()
            .is_some_and(|request| &request.task_id != source)
        {
            return Err(CoreError::Conflict("Fork 回执不属于此会话".into()));
        }
        let Some(ack) = self
            .repository
            .command_ack(&format!("fork:{target}"))
            .await?
        else {
            let Some(request) = intent else {
                return Ok(None);
            };
            let mut cursor = 0;
            let mut progress = None;
            let head = self.repository.event_head().await?;
            loop {
                let page = self
                    .repository
                    .task_events_of_types(source, cursor, head, &["task.fork_progress"], 200)
                    .await?;
                let Some(last) = page.last() else {
                    break;
                };
                cursor = last.sequence;
                for event in page {
                    if event.payload["newTaskId"] == json!(target) {
                        progress = Some(event.payload);
                    }
                }
            }
            let active = self
                .active_forks
                .lock()
                .map_err(|_| CoreError::Internal("Fork 状态不可用".into()))?
                .contains_key(target);
            let failed = progress.as_ref().is_some_and(|p| p["status"] == "failed");
            return Ok(Some(ForkReceipt {
                request,
                task_available: false,
                completed: false,
                status: if failed {
                    "failed"
                } else if active {
                    "running"
                } else {
                    "interrupted"
                }
                .into(),
                stage: progress
                    .as_ref()
                    .and_then(|p| p["stage"].as_str().map(str::to_owned)),
                error: if failed {
                    progress
                        .as_ref()
                        .and_then(|p| p["error"].as_str().map(str::to_owned))
                } else if active {
                    None
                } else {
                    Some("原 Fork 已中断，未自动重新创建；可以核实后接续原请求".into())
                },
            }));
        };
        let first = ack
            .first_sequence
            .filter(|seq| *seq > 0)
            .ok_or_else(|| CoreError::Internal("Fork 回执缺少事件范围".into()))?;
        let last = ack
            .last_sequence
            .ok_or_else(|| CoreError::Internal("Fork 回执缺少事件范围".into()))?;
        if last < first || last - first > 1 {
            return Err(CoreError::Internal("Fork 回执事件范围无效".into()));
        }
        let events = self.repository.sync_events(first - 1, 2).await?;
        let event = events
            .iter()
            .find(|event| {
                event.sequence <= last
                    && event.event_type == "task.forked"
                    && event.stream.stream_id == target.as_str()
            })
            .ok_or_else(|| {
                CoreError::NotFound("原 Fork 已受理，但创建记录不可用；未重新创建".into())
            })?;
        let request = ForkRequest {
            task_id: serde_json::from_value(event.payload["sourceTaskId"].clone())
                .map_err(|_| CoreError::Internal("Fork 来源记录损坏".into()))?,
            turn_id: serde_json::from_value(event.payload["sourceTurnId"].clone())
                .map_err(|_| CoreError::Internal("Fork 轮次记录损坏".into()))?,
            new_task_id: serde_json::from_value(event.payload["taskId"].clone())
                .map_err(|_| CoreError::Internal("Fork 目标记录损坏".into()))?,
            environment: serde_json::from_value(event.payload["environment"].clone())
                .map_err(|_| CoreError::Internal("Fork 环境记录损坏".into()))?,
        };
        if &request.task_id != source || &request.new_task_id != target {
            return Err(CoreError::Conflict("Fork 回执不属于此会话".into()));
        }
        if intent.as_ref().is_some_and(|original| original != &request) {
            return Err(CoreError::Conflict("Fork 原请求与结果不匹配".into()));
        }
        let task_available = self.repository.task_summary(target).await?.is_some();
        Ok(Some(ForkReceipt {
            request,
            task_available,
            completed: true,
            status: "succeeded".into(),
            stage: Some("completed".into()),
            error: None,
        }))
    }
    pub(crate) async fn record_fork_snapshot(&self, scope: &TurnWorkspaceScope) {
        let result = match self
            .workspace_context
            .capture_fork_snapshot(&scope.project_paths)
            .await
        {
            Ok(snapshot) => self
                .record_workspace_artifact(
                    scope,
                    snapshot,
                    "workspace.fork_snapshot_captured",
                    SNAPSHOT_MEDIA,
                )
                .await
                .map_err(|_| "轮末文件快照保存失败".to_owned()),
            Err(error) => Err(error),
        };
        if let Err(message) = result {
            let _ = self
                .append_runtime_event(
                    &scope.task_id,
                    "workspace.fork_snapshot_failed",
                    json!({"taskId":scope.task_id,"turnId":scope.turn_id,"message":message}),
                    EventSource::Hub,
                )
                .await;
        }
    }

    async fn fork_snapshot(&self, request: &ForkRequest, through: u64) -> Result<Value, CoreError> {
        let mut cursor = 0;
        let mut selected = None;
        let mut failure = None;
        loop {
            let page = self
                .repository
                .task_events_of_types(
                    &request.task_id,
                    cursor,
                    through,
                    &[
                        "workspace.fork_snapshot_captured",
                        "workspace.fork_snapshot_failed",
                    ],
                    500,
                )
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = last.sequence;
            for event in page {
                if event.payload["turnId"] != json!(request.turn_id) {
                    continue;
                }
                if event.event_type == "workspace.fork_snapshot_captured" {
                    selected = Some(event.payload["artifact"].clone());
                } else {
                    failure = event.payload["message"].as_str().map(str::to_owned);
                }
            }
        }
        let metadata: ArtifactMetadata = serde_json::from_value(selected.ok_or_else(|| {
            CoreError::InvalidArgument(failure.map_or_else(
                || "本轮没有可恢复的文件快照，无法从本轮成果创建 worktree。旧版轮次无法补录当时的文件；仍可在当前工作区 Fork。".into(),
                |reason| format!("本轮文件快照不可用：{reason}。仍可在当前工作区 Fork。"),
            ))
        })?).map_err(|_| CoreError::Internal("轮末快照元数据无效".into()))?;
        let artifact = self
            .repository
            .artifact(&metadata.artifact_id)
            .await?
            .ok_or_else(|| CoreError::NotFound("轮末文件快照已不可用".into()))?;
        if metadata.media_type != SNAPSHOT_MEDIA
            || artifact.metadata.media_type != SNAPSHOT_MEDIA
            || artifact.metadata.payload_sha256 != metadata.payload_sha256
            || artifact.content.len() as u64 != metadata.byte_length
            || artifact.content.len() > 1_048_576
            || hex::encode(Sha256::digest(&artifact.content)) != metadata.payload_sha256
        {
            return Err(CoreError::Internal("轮末文件快照校验失败".into()));
        }
        serde_json::from_slice(&artifact.content)
            .map_err(|_| CoreError::Internal("轮末文件快照格式无效".into()))
    }

    pub async fn fork_task(&self, request: ForkRequest) -> Result<TaskId, CoreError> {
        // Serialize retries independently of review admission/cancellation.
        let _fork_guard = self.fork_gate.lock().await;
        let source_guard = self.active_reviews.lock().await;
        let key = format!("fork:{}", request.new_task_id);
        let intent = self.fork_intent(&request.new_task_id).await?;
        if intent.as_ref().is_some_and(|original| original != &request) {
            return Err(CoreError::Conflict("Fork 标识已用于不同的原请求".into()));
        }
        if self.repository.command_ack(&key).await?.is_some() {
            self.require_task(&request.new_task_id).await?;
            let events = self
                .repository
                .task_events_of_types(
                    &request.new_task_id,
                    0,
                    self.repository.event_head().await?,
                    &["task.forked"],
                    1,
                )
                .await?;
            if !events.first().is_some_and(|event| {
                event.payload["sourceTaskId"] == json!(request.task_id)
                    && event.payload["sourceTurnId"] == json!(request.turn_id)
                    && event.payload["environment"] == json!(request.environment)
            }) {
                return Err(CoreError::Conflict("Fork 请求与已创建的会话不一致".into()));
            }
            return Ok(request.new_task_id);
        }
        if intent.is_none()
            && self
                .repository
                .command_ack(&format!("{key}:project"))
                .await?
                .is_some()
        {
            return Err(CoreError::Conflict(
                "旧 Fork 已创建部分项目但缺少原请求，无法安全重试；请保留 Worktree 并核实".into(),
            ));
        }
        let plan = if intent.is_some() {
            let ack = self
                .repository
                .command_ack(&format!("{key}:intent"))
                .await?
                .ok_or_else(|| CoreError::Internal("Fork 原请求不可用".into()))?;
            let sequence = ack
                .first_sequence
                .ok_or_else(|| CoreError::Internal("Fork 原请求范围损坏".into()))?;
            let events = self
                .repository
                .sync_events(sequence.saturating_sub(1), 1)
                .await?;
            let value = events
                .first()
                .filter(|e| e.sequence == sequence)
                .and_then(|e| e.payload.get("plan"))
                .ok_or_else(|| {
                    CoreError::Conflict(
                        "旧 Fork 缺少完整恢复计划，请保留已有 Worktree 并核实；未重新创建".into(),
                    )
                })?;
            let plan: ForkPlan = serde_json::from_value(value.clone())
                .map_err(|_| CoreError::Internal("Fork 恢复计划损坏，未继续".into()))?;
            self.require_task(&request.task_id).await?;
            if plan.source.task_id != request.task_id {
                return Err(CoreError::Conflict("Fork 恢复计划归属不匹配".into()));
            }
            plan
        } else {
            self.prepare_fork_plan(&request).await?
        };
        if intent.is_none() {
            let outcome = self
                .repository
                .append(AppendRequest {
                    request_fingerprint: None,
                    command_id: CommandId::new(),
                    idempotency_key: format!("{key}:intent"),
                    expected_stream_version: None,
                    artifacts: vec![],
                    events: vec![NewEvent::hub(
                        StreamRef {
                            stream_type: StreamType::Task,
                            stream_id: request.task_id.to_string(),
                        },
                        "task.fork_requested",
                        json!({"taskId":request.task_id,"request":request,"plan":plan}),
                    )],
                })
                .await?;
            self.publish(&outcome.events);
        }
        self.active_forks
            .lock()
            .map_err(|_| CoreError::Internal("Fork 状态不可用".into()))?
            .insert(request.new_task_id.clone(), request.task_id.clone());
        let _active = ActiveFork(self.active_forks.clone(), request.new_task_id.clone());
        drop(source_guard);
        self.record_fork_progress(&request, "preparing", None)
            .await?;
        let result = self.finish_fork(&request, &plan).await;
        if let Err(error) = &result {
            // An append can commit before returning an infrastructure error.
            // Final durable facts win; otherwise retain the failed phase.
            if let Ok(Some(receipt)) = self
                .fork_receipt(&request.task_id, &request.new_task_id)
                .await
            {
                if receipt.completed && receipt.task_available {
                    return Ok(request.new_task_id);
                }
                let _ = self
                    .record_fork_progress(
                        &request,
                        receipt.stage.as_deref().unwrap_or("preparing"),
                        Some(error.to_string()),
                    )
                    .await;
            }
        }
        result
    }

    async fn record_fork_progress(
        &self,
        request: &ForkRequest,
        stage: &str,
        error: Option<String>,
    ) -> Result<(), CoreError> {
        let event = NewEvent::hub(
            StreamRef {
                stream_type: StreamType::Task,
                stream_id: request.task_id.to_string(),
            },
            "task.fork_progress",
            json!({"taskId":request.task_id,"newTaskId":request.new_task_id,"stage":stage,
                "status":if error.is_some() { "failed" } else { "running" },"error":error}),
        );
        let outcome = self
            .repository
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: event.event_id.to_string(),
                expected_stream_version: None,
                artifacts: vec![],
                events: vec![event],
            })
            .await?;
        self.publish(&outcome.events);
        Ok(())
    }

    async fn prepare_fork_plan(&self, request: &ForkRequest) -> Result<ForkPlan, CoreError> {
        let source = self.require_task(&request.task_id).await?;
        if source
            .active_primary_run
            .as_ref()
            .is_some_and(|run| run.turn_id == request.turn_id)
        {
            return Err(CoreError::Conflict("请等待本轮对话结束后再 fork".into()));
        }
        if self
            .repository
            .task_summary(&request.new_task_id)
            .await?
            .is_some()
        {
            return Err(CoreError::Conflict("目标会话已存在".into()));
        }
        let head = self.repository.event_head().await?;
        let mut cursor = 0;
        let mut through = None;
        loop {
            let page = self
                .repository
                .task_events_of_types(
                    &request.task_id,
                    cursor,
                    head,
                    &[
                        "turn.completed",
                        "turn.failed",
                        "turn.cancelled",
                        "turn.interrupted",
                        "turn.connection_lost",
                    ],
                    1000,
                )
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            if let Some(event) = page
                .iter()
                .rfind(|event| event.payload["turnId"].as_str() == Some(request.turn_id.as_str()))
            {
                through = Some(event.sequence);
            }
            cursor = last.sequence;
        }
        let through =
            through.ok_or_else(|| CoreError::InvalidArgument("找不到已结束的源轮次".into()))?;
        let mut messages: Vec<Value> = Vec::new();
        let mut bytes = 0;
        cursor = 0;
        loop {
            let page = self
                .repository
                .frozen_history_messages(&request.task_id, cursor, through, "", 500)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = last.sequence;
            for message in page {
                bytes += message.text.len();
                if bytes > 16 * 1024 * 1024 {
                    return Err(CoreError::InvalidArgument(
                        "对话上下文过大，无法完整 fork".into(),
                    ));
                }
                if message.kind == "task.forked" {
                    let payload: Value = serde_json::from_str(&message.text)
                        .map_err(|e| CoreError::Internal(e.to_string()))?;
                    let inherited: Vec<Value> =
                        serde_json::from_str(payload["text"].as_str().ok_or_else(|| {
                            CoreError::Internal("继承的 fork 上下文不可用".into())
                        })?)
                        .map_err(|e| CoreError::Internal(e.to_string()))?;
                    messages.extend(inherited);
                } else {
                    messages.push(json!({"kind":message.kind,"text":message.text,"sequence":message.sequence,"turnId":message.turn_id}));
                }
            }
        }
        let snapshot = self.repository.bootstrap().await?;
        let project = snapshot
            .projects
            .iter()
            .find(|p| p.project_id == source.project_id)
            .ok_or_else(|| CoreError::NotFound("源项目不存在".into()))?;
        let (snapshot, snapshot_error) = if request.environment == ForkEnvironment::Worktree {
            match self.fork_snapshot(request, through).await {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error.to_string())),
            }
        } else {
            (None, None)
        };
        Ok(ForkPlan {
            source,
            project: project.clone(),
            through,
            messages,
            snapshot,
            snapshot_error,
        })
    }

    async fn finish_fork(
        &self,
        request: &ForkRequest,
        plan: &ForkPlan,
    ) -> Result<TaskId, CoreError> {
        let key = format!("fork:{}", request.new_task_id);
        let project_id = if request.environment == ForkEnvironment::Worktree {
            let id = ProjectId::parse(request.new_task_id.as_str().replacen("tsk_", "prj_", 1))
                .map_err(|e| CoreError::InvalidArgument(e.to_string()))?;
            let project_key = format!("{key}:project");
            if self.repository.command_ack(&project_key).await?.is_none() {
                let turn_snapshot = plan.snapshot.clone().ok_or_else(|| {
                    CoreError::InvalidArgument(
                        plan.snapshot_error
                            .clone()
                            .unwrap_or_else(|| "Fork 恢复计划缺少文件快照".into()),
                    )
                })?;
                self.record_fork_progress(request, "restoring_worktrees", None)
                    .await?;
                let paths = self
                    .workspace_context
                    .fork_worktrees(turn_snapshot, request.new_task_id.as_str())
                    .await
                    .map_err(CoreError::InvalidArgument)?;
                let (path, directories) = paths
                    .split_first()
                    .ok_or_else(|| CoreError::Internal("快照中没有项目目录".into()))?;
                let event = NewEvent::hub(
                    StreamRef {
                        stream_type: StreamType::Project,
                        stream_id: id.to_string(),
                    },
                    "project.registered",
                    json!({"projectId":id,"displayName":format!("{} · Fork", plan.project.display_name),"path":path,"additionalDirectories":directories}),
                );
                let outcome = self
                    .repository
                    .append(AppendRequest {
                        request_fingerprint: None,
                        command_id: CommandId::new(),
                        idempotency_key: project_key.clone(),
                        expected_stream_version: Some(0),
                        artifacts: vec![],
                        events: vec![event],
                    })
                    .await?;
                self.publish(&outcome.events);
            }
            let current = self
                .repository
                .bootstrap()
                .await?
                .projects
                .into_iter()
                .find(|project| project.project_id == id)
                .ok_or_else(|| CoreError::Conflict("Fork 中间项目已不可用，未重新创建".into()))?;
            let ack = self
                .repository
                .command_ack(&project_key)
                .await?
                .ok_or_else(|| CoreError::Internal("Fork 项目回执不可用".into()))?;
            let sequence = ack
                .first_sequence
                .ok_or_else(|| CoreError::Internal("Fork 项目回执缺少范围".into()))?;
            let events = self
                .repository
                .sync_events(sequence.saturating_sub(1), 1)
                .await?;
            let original = events
                .first()
                .filter(|e| e.sequence == sequence && e.event_type == "project.registered")
                .ok_or_else(|| CoreError::Conflict("Fork 中间项目记录不可用，未继续".into()))?;
            if original.payload["path"] != json!(current.path)
                || original.payload["additionalDirectories"]
                    != json!(current.additional_directories)
            {
                return Err(CoreError::Conflict(
                    "Fork 中间项目目录已更改，请核实原 Worktree；未继续创建会话".into(),
                ));
            }
            id
        } else {
            plan.source.project_id.clone()
        };
        self.record_fork_progress(request, "creating_task", None)
            .await?;
        let stream = StreamRef {
            stream_type: StreamType::Task,
            stream_id: request.new_task_id.to_string(),
        };
        let created = NewEvent::hub(
            stream.clone(),
            "task.created",
            json!({
                "taskId":request.new_task_id,"projectId":project_id,
                "title":format!("{} · Fork", plan.source.title.chars().take(110).collect::<String>()),
                "preferredExecutor":plan.source.preferred_executor
            }),
        );
        let forked = NewEvent::hub(
            stream,
            "task.forked",
            json!({
                "taskId":request.new_task_id,"sourceTaskId":request.task_id,"sourceTurnId":request.turn_id,
                "sourceSequence":plan.through,"environment":request.environment,
                "restoredTurnSnapshot":request.environment == ForkEnvironment::Worktree,
                "text":serde_json::to_string(&plan.messages).map_err(|e| CoreError::Internal(e.to_string()))?
            }),
        );
        // Creation and retained context commit together; native session IDs are never copied.
        let outcome = self
            .repository
            .append(AppendRequest {
                request_fingerprint: None,
                command_id: CommandId::new(),
                idempotency_key: key,
                expected_stream_version: Some(0),
                artifacts: vec![],
                events: vec![created, forked],
            })
            .await?;
        self.publish(&outcome.events);
        Ok(request.new_task_id.clone())
    }
}
