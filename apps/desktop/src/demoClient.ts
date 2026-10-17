import type {
  ArtifactId, ArtifactChunk, ArtifactCursor, ArtifactExportRequest, ArtifactPage, ArtifactReadRequest, TaskArtifactSummary,
  BootstrapSnapshot, CommandReceipt, CommandInvalidation, TaskCreationIdentity, TurnSubmission, PromptAttachment,
  SubmissionIntent, SubmissionIntentEntry, SubmissionIntentPage, SubmissionStartRequest,
  BuiltContextPackage,
  ContextEditRequest,
  ContextPackageV1,
  ContextPreviewRequest,
  CoreClient, HistoryQuery, HistoryPage,
  EventEnvelope,
  PendingApproval,
  QuotaSnapshotView,
  EventStreamMessage,
  ExecutorRef,
  ProjectId,
  TaskId,
  TaskSummary,
  TimelineItem,
  WorkspaceSnapshotSummary,
} from "@dogcodes/protocol";
import { ulid } from "ulid";
import { contextPreviewRequest } from "./contextRoute";

const now = () => new Date().toISOString();
const id = <T extends string>(prefix: string) => `${prefix}_${ulid()}` as T;

const projectId = id<ProjectId>("prj");
const firstTaskId = id<TaskId>("tsk");
const demoCodexInstallationId = `ins_${"2".repeat(26)}` as BootstrapSnapshot["installations"][number]["installationId"];

export class DemoCoreClient implements CoreClient {
  // Browser demo only: fixed command identities and in-memory receipts.
  private commandReceipts = new Map<string, { signature: string; receipt: CommandReceipt }>();
  private invalidatedCommands = new Map<string, string>();
  private invalidatedCreations = new Set<TaskId>();
  private commandReceipt(operationId: string, signature: string): CommandReceipt | null {
    const found = this.commandReceipts.get(operationId);
    const retired = this.invalidatedCommands.get(operationId);
    if ((found && found.signature !== signature) || (retired && retired !== signature)) throw new Error("原操作标识已用于不同请求");
    return found ? structuredClone(found.receipt) : null;
  }
  private assertCommand(operationId: string | undefined, signature: string) {
    if (operationId) {
      this.commandReceipt(operationId, signature);
      if (this.invalidatedCommands.has(operationId)) throw new Error("原操作已放弃，不能继续执行");
    }
  }
  private commandEvent(task: TaskSummary, eventType: string, payload: unknown, operationId: string | undefined, signature: string) {
    const sequence = this.sequence + 1;
    if (operationId) this.commandReceipts.set(operationId, { signature, receipt: { commandId: operationId, accepted: true,
      firstSequence: sequence, lastSequence: sequence, requestFingerprint: `demo:${signature}` } });
    this.emit({ stream: { streamType: "task", streamId: task.taskId }, streamVersion: task.streamVersion,
      eventType, payload, causationId: operationId ?? null });
  }
  private invalidateCommand(taskId: TaskId, originalVersion: number, operationId: string, signature: string, creation = false): CommandInvalidation {
    const receipt = this.commandReceipt(operationId, signature);
    if (receipt) return { status: "accepted", acknowledgement: receipt };
    if (this.invalidatedCommands.has(operationId)) return { status: "invalidated" };
    const task = this.tasks.find(item => item.taskId === taskId);
    if ((!task && !creation) || (task && task.streamVersion < originalVersion)) throw new Error("无法核实原操作版本");
    this.invalidatedCommands.set(operationId, signature);
    if (task) { task.streamVersion++; this.emit(); }
    else this.invalidatedCreations.add(taskId);
    return { status: "invalidated" };
  }
  private cancelReceipts = new Map<string, { signature: string; receipt: import("@dogcodes/protocol").CommandReceipt }>();
  private projects: BootstrapSnapshot["projects"] = [
    {additionalDirectories: [],  projectId, displayName: "dogcodes", path: "/workspace/dogcodes", taskCount: 0 },
    {additionalDirectories: [],  projectId: id<ProjectId>("prj"), displayName: "快速对话", path: "/demo/scratch", taskCount: 0 },
  ];
  async scanAgent(_providerId: string): Promise<BootstrapSnapshot> { return this.bootstrap(); }

  async recoverySnapshot(): Promise<import("@dogcodes/protocol").RecoverySnapshot> {
    return { schemaVersion: 1, generatedAt: now(), lastSequence: this.sequence, truncated: false,
      items: [{ code: "session_unavailable", providerId: "codex", taskId: firstTaskId,
        runId: null, turnId: null, expectedStreamVersion: this.tasks[0]?.streamVersion ?? null, canCancel: false }] };
  }

  async exportDiagnostics(): Promise<"saved" | "cancelled"> {
    throw new Error("诊断包导出请使用桌面应用；当前为浏览器演示");
  }
  private artifacts = new Map<ArtifactId, { taskId: TaskId; summary: TaskArtifactSummary; bytes: Uint8Array }>();

  async taskArtifacts(taskId: TaskId, before: ArtifactCursor | null = null, limit = 20): Promise<ArtifactPage> {
    if (!Number.isInteger(limit) || limit < 1 || limit > 100) throw new Error("无效的 Artifact 列表上限");
    if (!this.tasks.some((task) => task.taskId === taskId)) throw new Error("任务不存在");
    const items = [...this.artifacts.values()].filter((item) => item.taskId === taskId).map((item) => item.summary)
      .filter((item) => !before || item.sourceSequence < before.sequence
        || (item.sourceSequence === before.sequence && item.metadata.artifactId < before.artifactId))
      .sort((a, b) => b.sourceSequence - a.sourceSequence || b.metadata.artifactId.localeCompare(a.metadata.artifactId));
    const page = items.slice(0, limit);
    const last = page.at(-1);
    return structuredClone({ items: page, nextCursor: items.length > limit && last
      ? { sequence: last.sourceSequence, artifactId: last.metadata.artifactId } : null });
  }

  async readArtifact(request: ArtifactReadRequest): Promise<ArtifactChunk> {
    const item = this.artifacts.get(request.artifactId);
    if (!item || item.taskId !== request.taskId) throw new Error("当前任务没有此 Artifact");
    if (request.expectedPayloadSha256 !== item.summary.metadata.payloadSha256) throw new Error("Artifact 哈希已变化");
    if (!Number.isInteger(request.limit) || request.limit < 1 || request.limit > 32768
      || !Number.isInteger(request.offset) || request.offset < 0 || request.offset > item.bytes.length) {
      throw new Error("Artifact 读取范围无效");
    }
    const end = Math.min(item.bytes.length, request.offset + request.limit);
    return { metadata: structuredClone(item.summary.metadata), offset: request.offset,
      bytes: Array.from(item.bytes.slice(request.offset, end)), nextOffset: end < item.bytes.length ? end : null };
  }

  async exportArtifact(_request: ArtifactExportRequest): Promise<"saved" | "cancelled"> {
    throw new Error("浏览器 Demo 不导出本地 Artifact；请使用桌面应用");
  }
  private listeners = new Set<(message: EventStreamMessage) => void>();
  private events: EventEnvelope[] = [];
  private sequence = 6;
  private tasks: TaskSummary[] = [
    {
      taskId: firstTaskId,
      projectId,
      title: "构建首个纵向切片",
      preferredExecutor: { providerId: "codex", installationId: demoCodexInstallationId },
      lastTurnExecutor: { providerId: "codex", installationId: demoCodexInstallationId },
      activePrimaryRun: null,
      streamVersion: 5,
    },
  ];
  private items = new Map<TaskId, TimelineItem[]>([
    [
      firstTaskId,
      [
        {attachments: [], 
          itemId: "welcome",
          taskId: firstTaskId,
          turnId: null,
          kind: "system",
          text: "Core 已就绪。当前为离线 Fake Driver，可验证统一时间线和执行者切换。",
          providerId: null,
          sequence: 1,
          occurredAt: now(),
        },
      ],
    ],
  ]);

  private deletedTasks = new Set<TaskId>();

  async bootstrap(): Promise<BootstrapSnapshot> {
    const visible = this.tasks.filter(task => !this.deletedTasks.has(task.taskId));
    return {
      projects: this.projects.map(project => ({ ...project, taskCount: visible.filter(task => task.projectId === project.projectId).length })),
      tasks: structuredClone(visible),
      deletedTasks: [],
      installations: [
        {
          installationId: demoCodexInstallationId,
          providerId: "codex",
          executable: "demo-codex",
          fingerprint: "demo-codex-v1",
          status: "ready",
          version: "demo",
          versionText: "Codex Demo",
          diagnostic: "浏览器演示数据",
          probeElapsedMillis: 0,
          probedAt: now(),
        },
        {
          installationId: `ins_${"0".repeat(26)}`,
          providerId: "fake",
          executable: "in-process",
          fingerprint: "fake-v1",
          status: "ready",
          version: "1.0.0",
          versionText: "Fake Driver 1.0.0",
          diagnostic: null,
          probeElapsedMillis: 0,
          probedAt: now(),
        },
        {
          installationId: `ins_${"1".repeat(26)}`,
          providerId: "fake-peer",
          executable: "in-process",
          fingerprint: "fake-peer-v1",
          status: "ready",
          version: "1.0.0",
          versionText: "Fake Peer 1.0.0",
          diagnostic: null,
          probeElapsedMillis: 0,
          probedAt: now(),
        },
      ],
      activeRuns: visible.flatMap((task) => task.activePrimaryRun ? [task.activePrimaryRun] : []),
      sessionMaps: [],
      lastSequence: this.sequence,
      companion: {
        schemaVersion: 1,
        revision: this.sequence,
        generatedAt: now(),
        stale: false,
        workspaces: visible.map((task) => ({
          projectId: task.projectId,
          taskId: task.taskId,
          displayName: task.title,
          preferredExecutor: task.preferredExecutor,
          activePrimaryRun: task.activePrimaryRun,
          secondaryAgentCount: 0,
          pendingInteractionCount: 0,
        })),
        recentCompletion: null,
      },
    };
  }

  async queryHistory(query: HistoryQuery): Promise<HistoryPage> {
    if (!this.tasks.some(t => t.taskId === query.taskId)) throw new Error("对话不存在");
    const through = query.throughSequence ?? this.sequence;
    const items = (await this.timeline(query.taskId)).filter(item => item.sequence > query.afterSequence && item.sequence <= through && (query.kind === "turns" || query.kind === "search") && item.text.toLocaleLowerCase().includes(query.text.toLocaleLowerCase())).slice(0, query.limit + 1);
    const more = items.length > query.limit;
    items.length = Math.min(items.length, query.limit);
    return { items, nextCursor: more ? items.at(-1)!.sequence : null, throughSequence: through, coverage: "Browser demonstration only" };
  }

  async timeline(taskId: TaskId): Promise<TimelineItem[]> {
    if (this.deletedTasks.has(taskId)) throw new Error("会话已删除，请先恢复");
    return structuredClone(this.items.get(taskId) ?? []);
  }

  async workspaceSnapshots(
    _taskId: TaskId,
    _limit = 20,
  ): Promise<WorkspaceSnapshotSummary[]> {
    return [];
  }

  async buildContextPreview(request: ContextPreviewRequest): Promise<BuiltContextPackage> {
    const task = this.tasks.find((candidate) => candidate.taskId === request.taskId);
    if (!task) throw new Error("任务不存在");
    if (task.preferredExecutor.providerId !== request.sourceExecutor.providerId) {
      throw new Error("任务执行者已变化，请重新生成预览");
    }
    if (request.afterSequence > request.throughSequence || request.throughSequence > this.sequence) {
      throw new Error("上下文事件区间无效");
    }
    const historyBytes = new TextEncoder().encode("[]");
    const historyHash = await sha256(historyBytes);
    const packageValue: ContextPackageV1 = {
      deliveryFacts: [], conversationText: "",
      schemaVersion: 1,
      kind: request.kind,
      strength: request.strength,
      sourceExecutor: request.sourceExecutor,
      targetExecutor: request.targetExecutor,
      objective: task.title,
      userConstraints: [],
      acceptanceCriteria: [],
      completedWork: [],
      decisions: [],
      pendingWork: [],
      relevantFiles: [],
      dependencyChanges: [],
      changeSummary: { base: "unknown", head: "unknown", diffStat: "unknown", artifactRef: null },
      tests: [],
      recentTurnRefs: [],
      contextPointers: [{
        id: `history:${request.taskId}:${request.afterSequence}-${request.throughSequence}`,
        title: "Frozen task history",
        kind: "canonical_history",
        locator: `task/${request.taskId}/events?after=${request.afterSequence}&through=${request.throughSequence}`,
        contentHash: historyHash,
        triggerHints: ["history", "audit"],
        byteLength: historyBytes.byteLength,
        sensitivity: "project",
      }],
      semanticSummary: null,
      provenance: {
        builderVersion: 1,
        builtAt: "1970-01-01T00:00:00Z",
        redactionRulesetVersion: 1,
        redactionCount: 0,
        sourceAfterSequence: request.afterSequence,
        sourceThroughSequence: request.throughSequence,
        sourceSequences: [],
      },
    };
    const bytes = new TextEncoder().encode(JSON.stringify(packageValue));
    return { package: packageValue, payloadSha256: await sha256(bytes), byteLength: bytes.byteLength };
  }

  async buildEditedContextPreview(edit: ContextEditRequest): Promise<BuiltContextPackage> {
    const base = await this.buildContextPreview(edit.request);
    if (base.payloadSha256 !== edit.basePayloadSha256) {
      throw new Error("交接预览哈希已变化，请重新审阅");
    }
    const candidate = structuredClone(edit.editedPackage);
    if (
      candidate.schemaVersion !== base.package.schemaVersion
      || candidate.kind !== base.package.kind
      || candidate.strength !== base.package.strength
      || JSON.stringify(candidate.sourceExecutor) !== JSON.stringify(base.package.sourceExecutor)
      || JSON.stringify(candidate.targetExecutor) !== JSON.stringify(base.package.targetExecutor)
      || JSON.stringify(candidate.provenance) !== JSON.stringify(base.package.provenance)
    ) {
      throw new Error("编辑内容改变了不可变的路由或来源字段");
    }
    const bytes = new TextEncoder().encode(JSON.stringify(candidate));
    return { package: candidate, payloadSha256: await sha256(bytes), byteLength: bytes.byteLength };
  }

  async confirmContextPackage(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
    editedPackage: ContextPackageV1 | null,
    operationId?: string,
  ): Promise<void> {
    const signature = this.contextSignature(task, request, expectedPayloadSha256, editedPackage);
    if (operationId && this.commandReceipt(operationId, signature)) return;
    this.assertCommand(operationId, signature);
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    const base = await this.buildContextPreview(request);
    const rebuilt = editedPackage
      ? await this.buildEditedContextPreview({
          request,
          basePayloadSha256: base.payloadSha256,
          editedPackage,
        })
      : base;
    if (rebuilt.payloadSha256 !== expectedPayloadSha256) {
      throw new Error("交接预览哈希已变化，请重新审阅");
    }
    this.assertCommand(operationId, signature);
    if (current.streamVersion !== task.streamVersion) throw new Error("任务版本已变化，请刷新后重试");
    current.preferredExecutor = structuredClone(request.targetExecutor);
    current.updatedAt = now();
    current.streamVersion += 2;
    const artifactId = id<`art_${string}`>("art");
    const bytes = new TextEncoder().encode(JSON.stringify(rebuilt.package));
    this.artifacts.set(artifactId, { taskId: task.taskId, bytes, summary: {
      metadata: { artifactId, mediaType: "application/vnd.dogcodes.context-package.v1+json",
        payloadSha256: rebuilt.payloadSha256, byteLength: bytes.length, createdAt: now(),
        redactionRulesetVersion: 0, redactionCount: 0 },
      sourceSequence: this.sequence + 1, sourceKind: "handoff.package_confirmed",
    } });
    this.items.get(task.taskId)?.push({attachments: [], 
      itemId: `handoff-${artifactId}`,
      taskId: task.taskId,
      turnId: null,
      kind: "handoff",
      text: `Confirmed ${request.sourceExecutor.providerId} → ${request.targetExecutor.providerId} · ${artifactId}`,
      providerId: request.targetExecutor.providerId,
      sequence: this.sequence + 1,
      occurredAt: now(),
    });
    this.commandEvent(current, "task.preferred_executor_changed", { taskId: task.taskId, executor: request.targetExecutor }, operationId, signature);
  }

  async optOutReconciliation(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
  ): Promise<void> {
    if (request.kind !== "reconciliation") throw new Error("只能关闭 Reconciliation");
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    const preview = await this.buildContextPreview(request);
    if (preview.payloadSha256 !== expectedPayloadSha256) {
      throw new Error("对账预览哈希已变化，请重新审阅");
    }
    current.preferredExecutor = structuredClone(request.targetExecutor);
    current.updatedAt = now();
    current.streamVersion += 2;
    this.items.get(task.taskId)?.push({attachments: [], 
      itemId: `reconciliation-opt-out-${this.sequence + 1}`,
      taskId: task.taskId,
      turnId: null,
      kind: "reconciliation",
      text: `Opted out of reconciliation ${request.sourceExecutor.providerId} → ${request.targetExecutor.providerId}`,
      providerId: request.targetExecutor.providerId,
      sequence: this.sequence + 1,
      occurredAt: now(),
    });
    this.emit();
  }

  async updateProject(project: import("@dogcodes/protocol").ProjectSummary, displayName: string, path: string, additionalDirectories: string[]): Promise<void> {
    const current = this.projects.find(item => item.projectId === project.projectId);
    if (!current) throw new Error("项目不存在");
    if ((current.streamVersion ?? 1) !== (project.streamVersion ?? 1)) throw new Error("项目已被修改，请重新打开编辑窗口");
    if (!displayName.trim() || !path.trim()) throw new Error("请输入项目名称并添加至少一个目录");
    Object.assign(current, {updatedAt: now(), displayName: displayName.trim(), path, additionalDirectories: [...additionalDirectories], streamVersion: (current.streamVersion ?? 1) + 1});
    this.emit();
  }

  async registerProject(displayName: string, path: string, additionalDirectories: string[] = []): Promise<ProjectId> {
    const existing = this.projects.find(project => project.path === path && project.displayName === displayName && JSON.stringify(project.additionalDirectories) === JSON.stringify(additionalDirectories));
    if (existing) return existing.projectId;
    const projectId = id<ProjectId>("prj");
    this.projects.push({ projectId, displayName, path, additionalDirectories, taskCount: 0, updatedAt: now() });
    this.emit();
    return projectId;
  }

  async syncEvents(afterSequence: number, limit = 10_000): Promise<EventEnvelope[]> {
    return this.events.filter((event) => event.sequence > afterSequence).slice(0, limit);
  }

  private creationSignature(project: ProjectId, title: string, executor: ExecutorRef, identity: TaskCreationIdentity) {
    return JSON.stringify(["create", project, title, executor, identity.taskId]);
  }
  async createTaskReceipt(project: ProjectId, title: string, executor: ExecutorRef, identity: TaskCreationIdentity) {
    return this.commandReceipt(identity.operationId, this.creationSignature(project, title, executor, identity));
  }
  async invalidateTaskCreation(project: ProjectId, title: string, executor: ExecutorRef, identity: TaskCreationIdentity) {
    return this.invalidateCommand(identity.taskId, 0, identity.operationId, this.creationSignature(project, title, executor, identity), true);
  }
  private contextSignature(task: TaskSummary, request: ContextPreviewRequest, hash: string, edited: ContextPackageV1 | null) {
    return JSON.stringify(["context", task.taskId, task.streamVersion, request, hash, edited]);
  }
  async contextPackageReceipt(task: TaskSummary, request: ContextPreviewRequest, hash: string, edited: ContextPackageV1 | null, operationId: string) {
    return this.commandReceipt(operationId, this.contextSignature(task, request, hash, edited));
  }
  async invalidateContextPackage(task: TaskSummary, request: ContextPreviewRequest, hash: string, edited: ContextPackageV1 | null, operationId: string) {
    return this.invalidateCommand(task.taskId, task.streamVersion, operationId, this.contextSignature(task, request, hash, edited));
  }
  async createTask(project: ProjectId, title: string, executor: ExecutorRef, identity?: TaskCreationIdentity): Promise<TaskId> {
    const taskId = identity?.taskId ?? id<TaskId>("tsk");
    const signature = this.creationSignature(project, title, executor, { taskId, operationId: identity?.operationId ?? "" });
    if (identity && this.commandReceipt(identity.operationId, signature)) return taskId;
    this.assertCommand(identity?.operationId, signature);
    if (this.invalidatedCreations.has(taskId) || this.tasks.some(task => task.taskId === taskId)) throw new Error("任务版本已变化");
    const task: TaskSummary = { updatedAt: now(), taskId, projectId: project, title, preferredExecutor: structuredClone(executor), activePrimaryRun: null, streamVersion: 1 };
    this.tasks.push(task);
    this.items.set(taskId, []);
    this.commandEvent(task, "task.created", { taskId, projectId: project, title, preferredExecutor: executor }, identity?.operationId, signature);
    return taskId;
  }

  async forkTask(request: import("@dogcodes/protocol").ForkRequest): Promise<TaskId> {
    if (request.environment === "worktree") throw new Error("创建 Git worktree 请使用桌面应用");
    if (this.tasks.some(task => task.taskId === request.newTaskId)) return request.newTaskId;
    const source = this.tasks.find(task => task.taskId === request.taskId);
    const history = this.items.get(request.taskId) ?? [];
    const last = [...history].reverse().find(item => item.turnId === request.turnId);
    const end = last ? history.indexOf(last) : -1;
    if (!source || end < 0 || source.activePrimaryRun?.turnId === request.turnId
      || !history.some(item => item.turnId === request.turnId && item.turnCompletion)) throw new Error("找不到已结束的源轮次");
    const messages = history.slice(0, end + 1).flatMap(item => {
      if (item.kind === "fork_context") return JSON.parse(JSON.parse(item.text).text) as { kind: string; text: string }[];
      if (item.kind === "user_message" || item.kind === "assistant_message") return [{ kind: item.kind === "user_message" ? "turn.queued" : "message.delta", text: item.text }];
      return [];
    });
    this.tasks.push({ ...structuredClone(source), taskId: request.newTaskId, title: `${source.title} · Fork`, activePrimaryRun: null, streamVersion: 2, updatedAt: now() });
    this.items.set(request.newTaskId, [{ itemId: id("fork"), taskId: request.newTaskId, turnId: null, kind: "fork_context", text: JSON.stringify({ sourceTaskId: request.taskId, sourceTurnId: request.turnId, environment: request.environment, text: JSON.stringify(messages) }), providerId: null, attachments: [], sequence: ++this.sequence, occurredAt: now() }]);
    this.emit();
    return request.newTaskId;
  }

  private renamedTasks = new Set<TaskId>();

  async deleteTask(task: TaskSummary): Promise<void> {
    const current = this.tasks.find(candidate => candidate.taskId === task.taskId);
    if (!current || current.streamVersion !== task.streamVersion || this.deletedTasks.has(task.taskId)) throw new Error("会话状态已变化，请重试");
    if (current.activePrimaryRun) throw new Error("请先停止主任务，再删除会话");
    this.deletedTasks.add(task.taskId);
    this.tasks = this.tasks.filter(candidate => candidate.taskId !== task.taskId);
    this.items.delete(task.taskId);
    this.events = this.events.filter(event => event.stream.streamId !== task.taskId);
    for (const [id, artifact] of this.artifacts) if (artifact.taskId === task.taskId) this.artifacts.delete(id);
    this.renamedTasks.delete(task.taskId);
    this.emit();
  }



  async renameTask(task: TaskSummary, title: string): Promise<void> {
    const current = this.tasks.find(candidate => candidate.taskId === task.taskId);
    if (!current || this.deletedTasks.has(task.taskId) || current.streamVersion !== task.streamVersion) throw new Error("任务版本已变化，请重试");
    if (!title.trim() || Array.from(title.trim()).length > 120 || /[\u0000-\u001f\u007f-\u009f]/u.test(title)) throw new Error("请输入 1–120 个字符的会话名称");
    current.title = title.trim();
    current.updatedAt = now();
    current.streamVersion++;
    this.renamedTasks.add(task.taskId);
    this.emit();
  }

  async setPreferredExecutor(task: TaskSummary, executor: ExecutorRef): Promise<void> {
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || this.deletedTasks.has(task.taskId) || current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    current.preferredExecutor = structuredClone(executor);
    current.updatedAt = now();
    current.streamVersion += 1;
    this.emit();
  }

  async captureWorkspaceDiff(task: TaskSummary): Promise<void> {
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || this.deletedTasks.has(task.taskId) || current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    throw new Error("浏览器 Demo 无法读取本地 Git 工作区；请在桌面应用中固化 Diff");
  }

  async quotaSnapshots(_taskId?: TaskId | null): Promise<QuotaSnapshotView[]> {
    const now = Date.now();
    return [
      {
        providerId: "codex",
        installationId: demoCodexInstallationId,
        quota: {
          windows: [
            { id: "primary", windowMinutes: 300, usedPercent: 39, resetsAtUnix: Math.floor(now / 1000) + 13_420 },
            { id: "secondary", windowMinutes: 10_080, usedPercent: 63, resetsAtUnix: Math.floor(now / 1000) + 86_400 },
          ],
          context: { usedTokens: 55_800, windowTokens: 1_000_000, cacheHitRatePercent: 98.8 },
          collectedAtUnixMs: now,
          staleAfterUnixMs: now + 15_000,
          planType: "demo",
          source: "demo",
          confidence: "provider_reported",
          parserVersion: "demo-v1",
        },
      },
    ];
  }

  async pendingApprovals(_taskId: TaskId, _limit = 20): Promise<PendingApproval[]> {
    return [];
  }

  async respondApproval(_task: Pick<TaskSummary, "taskId" | "streamVersion">, _approvalId: string, _optionId: string): Promise<void> {
    throw new Error("浏览器 Demo 没有真实审批流；请在桌面应用中批准");
  }

  // Browser demo stand-in for the service-driven send: same call shapes, in memory.
  private submissions = new Map<string, SubmissionIntentEntry>();
  private async freezeSubmission(request: SubmissionStartRequest): Promise<SubmissionIntent> {
    const snapshot = await this.bootstrap();
    const existing = request.taskId ? this.tasks.find(task => task.taskId === request.taskId) : undefined;
    if (request.taskId && (!existing || existing.activePrimaryRun || existing.projectId !== request.projectId)) throw new Error("原会话不可发送，请等待回复结束或刷新后重试。");
    const task: TaskSummary = existing ? structuredClone(existing)
      : { taskId: id<TaskId>("tsk"), projectId: request.projectId, title: "新对话", preferredExecutor: structuredClone(request.target), activePrimaryRun: null, streamVersion: 0 };
    const switching = !!existing && (existing.preferredExecutor.providerId !== request.target.providerId
      || (existing.preferredExecutor.installationId ?? null) !== (request.target.installationId ?? null));
    let context: SubmissionIntent["context"] = null;
    if (switching) {
      const contextRequest = contextPreviewRequest(snapshot, task, request.target, "standard");
      context = { operationId: id("cmd"), request: contextRequest, hash: (await this.buildContextPreview(contextRequest)).payloadSha256 };
    }
    return { schemaVersion: 1, operationId: request.operationId, scope: request.scope, requestedAt: now(), task, input: structuredClone(request.input),
      target: structuredClone(request.target), creation: existing ? null : { operationId: id("cmd") }, context, submission: existing && !switching ? task : null };
  }
  private async driveSubmission(intent: SubmissionIntent): Promise<SubmissionIntentEntry> {
    const pending: SubmissionIntentEntry = { state: "pending", intent };
    this.submissions.set(intent.operationId, pending);
    // Like the service, an accepted original stage is never executed again.
    if (intent.creation && !this.commandReceipts.has(intent.creation.operationId)) {
      await this.createTask(intent.task.projectId, intent.task.title, intent.target, { taskId: intent.task.taskId, operationId: intent.creation.operationId });
    } else if (intent.context && !this.commandReceipts.has(intent.context.operationId)) {
      await this.confirmContextPackage(intent.task, intent.context.request, intent.context.hash, null, intent.context.operationId);
    }
    const current = this.tasks.find(task => task.taskId === intent.task.taskId);
    if (!current) throw new Error("任务不存在");
    const { prompt, model, thinking, permissionMode, attachments } = intent.input;
    await this.submitTurn(structuredClone(current), prompt, model, thinking, permissionMode, attachments, intent.operationId);
    // A stubbed submitTurn (tests) records no event; the demo still reports acceptance.
    const event = this.events.find(item => item.eventType === "turn.queued" && item.causationId === intent.operationId);
    const entry: SubmissionIntentEntry = { state: "closed", fingerprint: `demo:${intent.operationId}`,
      resolution: { status: "submitted", taskId: intent.task.taskId, sequence: event?.sequence ?? this.sequence,
        turnId: (event?.payload as { turnId?: string } | undefined)?.turnId ?? id("trn") } };
    this.submissions.set(intent.operationId, entry);
    return entry;
  }
  async submissionStart(request: SubmissionStartRequest): Promise<SubmissionIntentEntry> {
    const found = this.submissions.get(request.operationId);
    if (found?.state === "closed") return found;
    if ([...this.submissions.values()].some(entry => entry.state === "pending" && entry.intent.operationId !== request.operationId
      && (entry.intent.scope === request.scope || entry.intent.task.taskId === request.taskId))) throw new Error("此会话或草稿已有待核实发送，请先处理原操作");
    return this.driveSubmission(found?.state === "pending" ? found.intent : await this.freezeSubmission(request));
  }
  async submissionContinue(operationId: string): Promise<SubmissionIntentEntry> {
    const found = this.submissions.get(operationId);
    if (!found) throw new Error("原发送记录不存在，请刷新后核实");
    return found.state === "pending" ? this.driveSubmission(found.intent) : found;
  }
  async submissionRetire(operationId: string): Promise<SubmissionIntentEntry> {
    const found = this.submissions.get(operationId);
    if (!found) throw new Error("原发送记录不存在，请刷新后核实");
    if (found.state === "closed") return found;
    this.invalidatedCommands.set(operationId, "retired");
    const entry: SubmissionIntentEntry = { state: "closed", fingerprint: `demo:${operationId}`, resolution: { status: "invalidated", taskId: found.intent.task.taskId } };
    this.submissions.set(operationId, entry);
    return entry;
  }
  async submissionIntentGet(operationId: string): Promise<SubmissionIntentEntry | null> {
    return structuredClone(this.submissions.get(operationId) ?? null);
  }
  async submissionIntentList(): Promise<SubmissionIntentPage> {
    return { operationIds: [...this.submissions.entries()].filter(([, entry]) => entry.state === "pending").map(([operationId]) => operationId), nextCursor: null };
  }
  private submissionSignature(task: TaskSummary, input: TurnSubmission) {
    return JSON.stringify(["submit", task.taskId, task.streamVersion, task.preferredExecutor, input.prompt, input.model, input.thinking, input.permissionMode, input.attachments]);
  }
  async submitTurnReceipt(task: TaskSummary, input: TurnSubmission, operationId: string) {
    return this.commandReceipt(operationId, this.submissionSignature(task, input));
  }
  async invalidateTurnSubmission(task: TaskSummary, input: TurnSubmission, operationId: string) {
    return this.invalidateCommand(task.taskId, task.streamVersion, operationId, this.submissionSignature(task, input));
  }
  async submitTurn(task: TaskSummary, prompt: string, model: string | null = null, thinking: string | null = null, permissionMode: string | null = null, attachments: PromptAttachment[] = [], operationId?: string): Promise<void> {
    const signature = this.submissionSignature(task, { prompt, model, thinking, permissionMode, attachments });
    if (operationId && this.commandReceipt(operationId, signature)) return;
    this.assertCommand(operationId, signature);
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || this.deletedTasks.has(task.taskId)) throw new Error("任务不存在");
    if (current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    if (current.activePrimaryRun) throw new Error("请等待当前回复结束");
    const titleCharacters = Array.from(prompt.trim().replace(/\s+/gu, " "));
    if (!this.renamedTasks.has(task.taskId)) current.title = titleCharacters.slice(0, 32).join("") + (titleCharacters.length > 32 ? "…" : "");
    const taskId = current.taskId;
    const turnId = id<`trn_${string}`>("trn");
    const runId = id<`run_${string}`>("run");
    current.lastTurnExecutor = structuredClone(current.preferredExecutor);
    current.activePrimaryRun = {
      turnId,
      runId,
      executor: current.preferredExecutor,
      state: "running",
      startedAt: now(),
    };
    current.updatedAt = now();
    current.streamVersion += 1;
    const timeline = this.items.get(taskId)!;
    timeline.push({attachments: structuredClone(attachments),
      itemId: `user-${turnId}`,
      taskId,
      turnId,
      kind: "user_message",
      text: prompt,
      providerId: null,
      sequence: this.sequence + 1,
      occurredAt: now(),
    });
    this.commandEvent(current, "turn.queued", { taskId, turnId, runId, prompt, executor: task.preferredExecutor, attachments, model, thinking, permissionMode, startedAt: now() }, operationId, signature);

    const reply: TimelineItem = {attachments: [], 
      itemId: `assistant-${turnId}`,
      taskId,
      turnId,
      kind: "assistant_message",
      text: "",
      providerId: current.activePrimaryRun.executor.providerId,
      sequence: this.sequence + 1,
      occurredAt: now(),
    };
    timeline.push(reply);
    for (const chunk of ["已接收任务。", "这是 Fake Driver 的流式响应，", "用于验证有序事件与统一时间线。"] as const) {
      await new Promise((resolve) => globalThis.setTimeout(resolve, 260));
      reply.text += chunk;
      this.emit();
    }
    current.activePrimaryRun = null;
    current.updatedAt = now();
    current.streamVersion += 4;
    this.emit();
  }

  private cancelSignature(task: TaskSummary) {
    const run = task.activePrimaryRun;
    if (!run) throw new Error("当前任务没有运行中的 Turn");
    return JSON.stringify([task.taskId, task.streamVersion, run.turnId, run.runId]);
  }
  async cancelTurnReceipt(task: TaskSummary, operationId: string): Promise<import("@dogcodes/protocol").CommandReceipt | null> {
    const found = this.cancelReceipts.get(operationId);
    if (!found) return null;
    if (found.signature !== this.cancelSignature(task)) throw new Error("停止标识已用于不同的请求");
    return structuredClone(found.receipt);
  }
  async cancelTurn(task: TaskSummary, operationId?: string): Promise<void> {
    const signature = this.cancelSignature(task);
    if (operationId && await this.cancelTurnReceipt(task, operationId)) return;
    const current = this.tasks.find((candidate) => candidate.taskId === task.taskId);
    if (!current || this.deletedTasks.has(task.taskId) || current.streamVersion !== task.streamVersion) {
      throw new Error("任务版本已变化，请刷新后重试");
    }
    if (!current.activePrimaryRun) throw new Error("当前任务没有运行中的 Turn");
    if (current.activePrimaryRun.runId !== task.activePrimaryRun?.runId || current.activePrimaryRun.turnId !== task.activePrimaryRun?.turnId) throw new Error("停止目标已变化");
    current.activePrimaryRun.state = "interrupting";
    current.updatedAt = now();
    current.streamVersion += 1;
    if (operationId) this.cancelReceipts.set(operationId, { signature, receipt: { commandId: operationId, accepted: true,
      firstSequence: null, lastSequence: null, requestFingerprint: `demo:${signature}` } });
    this.emit();
  }

  async subscribe(
    afterSequence: number,
    listener: (message: EventStreamMessage) => void,
  ): Promise<() => void> {
    listener({ type: "replay_started", data: { afterSequence } });
    for (const event of await this.syncEvents(afterSequence)) {
      listener({ type: "event", data: event });
    }
    listener({ type: "caught_up", data: { lastSequence: this.sequence } });
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(overrides: Partial<EventEnvelope> = {}) {
    const sequence = ++this.sequence;
    const event: EventEnvelope = {
      schemaVersion: 1,
      sequence,
      eventId: `evt_demo_${sequence}`,
      stream: { streamType: "system", streamId: "demo" },
      streamVersion: sequence,
      eventType: "demo.projection_changed",
      occurredAt: now(),
      recordedAt: now(),
      correlationId: `cor_demo_${sequence}`,
      causationId: null,
      source: { kind: "hub" },
      payload: {},
      vendorExtensions: null,
      ...overrides,
    };
    this.events.push(event);
    for (const listener of this.listeners) listener({ type: "event", data: event });
  }
}

async function sha256(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", Uint8Array.from(bytes).buffer);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
