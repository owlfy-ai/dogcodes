# M1-S5 Handoff / Reconciliation 实施记录

更新日期：2026-09-03

## 目标与边界

本工作包落实需求 §11.0–§11.2 和详细设计 §11：不同 Agent 不共享供应商原生 Session，
由 Hub 从 Canonical History 确定性生成可预览、可审计的 Context Package。Hub 不隐式调用
模型；语义摘要只能作为带 ExecutionRun、Provider、输入 sequence 与 Artifact 来源的附加项。

当前已完成“冻结事实 → 只读/编辑预览 → 审阅 hash 确认 → 原稿/最终 Artifact 版本关系与
Canonical Event 原子落盘 → capability-gated resume → 有序 ContextDelivery 或显式 opt-out →
sent/failed/applied 审计”的 Fake Driver A→B→A 纵向切片，并已完成 Git/dependency facts、
Turn 起止 Git Snapshot、版本化 Test Artifact、受限 Artifact 查看/导出与 resume failure 分类首切片。
Artifact 原生保存交互尚待 UI 验收。完整
Git/Checkpoint 场景矩阵和外部 Product Driver 尚未实现，因此 M1-S5 仍为进行中。

## 已落地数据契约

`agent-protocol` 定义：

- `ContextPackageKind = handoff | reconciliation`
- `ContextStrength = compact | standard | full`
- `ContextPackageV1` 的 objective、constraints、acceptance criteria、completed work、decision、
  pending work、files、change summary、tests、recent turns、ContextPointer、semantic summary 和 provenance
- `ContextProvenance` 同时记录 builder version、稳定 builtAt、冻结区间与实际选中 sequence
- `SemanticSummary` 必须携带 source Run、Provider、Model、输入 sequence 和输出 Artifact 引用
- `BuiltContextPackage` 返回最终 JSON payload 的 SHA-256 与字节数
- package 显式包含 source/target `ExecutorRef`，交接方向属于被 hash 保护的内容
- package 的 `relevantFiles/changeSummary/dependencyChanges/tests` 分别承载工作树变更、HEAD→
  working-tree fingerprint、依赖清单变化和 Canonical test facts
- `ConfirmContextPackage` 包含完整冻结请求和用户已审阅的 `expectedPayloadSha256`
- `ContextEditRequest` 携带冻结请求、Hub 原稿 hash 和编辑草稿；`OptOutReconciliation` 携带
  Reconciliation 请求与已审阅的预览 hash
- `ArtifactMetadata` 记录 `art_<ULID>`、payload SHA-256、media type、字节数、稳定 createdAt 与脱敏 provenance
- `RecordTestResult` 与 `TestReportV1` 定义采集适配器边界；命令、outcome 和可选报告先脱敏，
  Artifact media type 固定为 `application/vnd.dogcodes.test-report.v1+json`

Rust DTO 与 `packages/protocol` TypeScript mirror 使用相同 camelCase wire。

## 当前数据流

```text
React Agent selector
  -> Tauri core_build_context_preview
  -> Core validates Task and canonical head
  -> SQLite task_events(task, after, through, limit=10001)
  -> deterministic reducer + ContextPointer hash
  -> ContextPackageV1 + payload SHA-256
  -> Handoff Preview
  -> optional edit -> Core validates immutable fields, redacts and returns a new safe hash
  -> user confirms OR explicitly opts out of Reconciliation
  -> ConfirmContextPackage(expectedStreamVersion, reviewed hash)
  -> Core rebuilds the exact frozen package and verifies the hash
  -> SQLite transaction
       -> SHA-256-addressed immutable artifact_blobs row
       -> immutable Hub-original and final artifacts keyed by art_<ULID>
       -> artifact_versions(final -> original, user_edit), when edited
       -> handoff.package_confirmed
       -> task.preferred_executor_changed
       -> projection + dedup Ack
  -> user submits next turn
  -> workspace.snapshot_captured(turn_start) OR workspace.snapshot_failed(stable reason)
  -> Core requires a confirmed/failed-retry transfer when last dispatched executor differs
  -> create target session OR resume only a Supported+Resumable target
  -> Driver.prompt(context[] first, user prompt second), or empty context after explicit opt-out
  -> handoff|reconciliation.sent|failed OR reconciliation.opt_out_applied|failed
  -> driver stream + terminal lastSeenSequence
  -> session.resumable OR session.detached
  -> workspace.snapshot_captured(turn_end) OR workspace.snapshot_failed(stable reason)
```

预览是只读查询，不写 Canonical Event。Agent 选择在预览前不会变化；确认时若 Task version
或 payload hash 已变化，操作会被拒绝且 Artifact/Event 均不写入。确认事件的语义严格是
`confirmed`，不能作为已向目标 Agent 发送成功的证据。

## Artifact 与确认不变量

- `ArtifactId` 遵循全局 `art_<ULID>` 规范；payload SHA-256 是独立、完整的内容地址。
- Core 只序列化已经过 `SecretRedactor` 的最终 `ContextPackageV1`；SQLite 再次计算 SHA-256，
  校验 ID/hash/byteLength，并执行 1 MiB 硬上限。
- Artifact insert、两个 Task Event、Projection 与 command dedup Ack 共用一个 SQLite 事务；
  stream version 冲突发生在 Artifact insert 之前。
- 内容相同复用同一 SHA-256 blob；每个确认版本仍有自己的 Artifact ULID，既有版本不覆盖。
- 编辑确认保存 Hub 构建原稿和最终安全版本；`artifact_versions` 以 `user_edit` 记录 parent，
  Projection rebuild 重建关系。未脱敏编辑草稿只存在于本地命令路径，不进入 Event/Artifact。
- Core 在业务校验前读取 dedup Ack；同一确认命令重试返回原 sequence 区间，不重复追加事件。
- `handoff/reconciliation.package_confirmed|sent|failed` 均进入可重建 Projection；confirmed
  不冒充 sent，失败使用固定安全 reason 并保留同一 Artifact 供重试。

## SessionMap 当前切片

- Rust/TS `AgentSessionMapSummary` 覆盖设计 §10.2 字段，并增加 Reconciliation 所需的
  `lastSeenSequence`；状态为 active/resumable/missing/incompatible/detached，resume capability
  为 supported/unsupported/unknown。
- Core 创建 native session 后写 `session.ready`；driver 开始原生 turn 时记录
  `lastNativeTurnId`；`turn.completed/failed` 用该 terminal Event sequence 推进已见基线。
- `AgentDriver` 提供 `resume_capability` 与 `resume_session`；只有 Driver 和 Projection 同时为
  Supported+Resumable 才恢复。Fake Driver 明确支持并在释放 live handle 后写 `session.resumable`；
  Unknown/Unsupported 仍写 `session.detached`。
- resume error 按语义映射为 `session.missing/incompatible/detached` 和固定 failure reason；原始
  Driver 文本不进入这些 Event，也不经后续 `turn.failed` 泄露。失败状态可由 Projection 重建。
- SQLite 以 Task + Provider + Installation key 唯一定位当前映射；新 native session 的事件
  替换映射内容，随机 `SessionMapId` 也来自 Event，Projection rebuild 不生成新事实。
- Bootstrap 暴露完整本地 SessionMap；裁剪的 CompanionProjection 不包含 native session id
  或 workspace locator。

## 发送顺序与防绕过

- `PromptRequest` 携带有序 `ContextDelivery[]`；Port 契约规定所有 Context 必须先于用户 prompt
  交付，返回 `Ok` 才表示完整请求已被底层 Agent 接受。
- Core 通过最后一个实际 `driver.turn_started` 的完整 ExecutorRef 判断是否跨 Agent；仅修改
  preferred executor 不能绕过 confirmed Context Transfer。
- Reconciliation 的 `afterSequence` 必须精确等于目标映射的 `lastSeenSequence`；范围或 Session
  状态变化会在预览/确认阶段返回 Conflict/InvalidArgument。
- Core 从 CAS 重新读取 Artifact 并复算 SHA-256/byteLength；完整性失败不调用 Driver。
- Driver 在接受请求前失败时写 transfer failed 和 turn failed，不写 sent。对应 Artifact 继续
  作为 pending retry；重试成功后才转 sent。
- 显式 opt-out 是独立命令和一次性运行门；取消/X 不等于 opt-out。它要求目标 Session 仍可
  恢复，Driver 拒绝完整请求时写 `opt_out_failed` 并保留重试，接受后先写 `opt_out_applied`
  再消费 driver stream。
- 同一目标按最新 confirmed/opted-out sequence 选择动作；较新终态会遮蔽旧 failed 动作。

## 确定性与最小化规则

- 读取区间固定为 `(afterSequence, throughSequence]`；through 不得超过查询时 Canonical Head。
- SQLite 同时限定 `stream_type = task` 和 `stream_id`，不会混入其他 Task。
- Diff Artifact 查询还必须限定同一 `(afterSequence, throughSequence]`，后续同 fingerprint
  的新 Artifact 不能改写旧包引用；空区间没有 Diff 引用。
- 上限为 10,000 个 Task Event，超过时显式 `ResourceExhausted`，不得静默截断。
- `builtAt` 取 through 边界 Event 的 `recordedAt`，不是当前时钟；同一范围重复构建完全一致。
- source sequence 排序、去重并验证处于冻结范围内。
- 精简/标准/完整当前分别保留最近 1/5/全部 Turn 引用与 completed-work 引用。
- 不展开用户 prompt、Agent message、terminal、完整 Diff 或 Raw Frame；使用带内容 hash 的
  Canonical History pointer 引用冻结事件数组。
- 未知事实输出 `unknown` 或空数组，不生成推测文本。
- semantic summary 默认为 null，不触发任何模型调用。
- Desktop 注入的 `GitWorkspaceContextProvider` 使用绝对 executable 和有界结构化 argv；只读取
  HEAD、porcelain v2 `--branch -z` status 与 staged/unstaged diff stat，不读取完整 Diff。输出上限 256 KiB、
  文件数 1000，普通文件只在 ≤1 MiB 时读取并记录 SHA-256。
- relevant file 按路径稳定排序；dependency changes 从受限 manifest/lockfile basename 集合派生。
  snapshot fingerprint 同时覆盖 status/stat/路径状态和内容 hash；工作树漂移会使确认重建 hash
  与已审阅版本不同并拒绝写入。
- 可展开文本在 hash 前经过默认 Redaction v2；初建和编辑重哈希使用同一强制入口。新增
  quoted JSON/Basic/URL/PEM/高熵候选，重复处理不重复计数。结构校验通过的冻结历史 locator
  不被自由文本启发式误删；任意 URL 仍处理，详情见 `m1-s5-redaction-v2.md`。
- 测试采集只接受已有结果，不执行 `RecordTestResult.command`。Core 将脱敏后的 `TestReportV1`
  作为最多 1 MiB 的不可变 Artifact 原子提交；`test.completed` 只含安全摘要和 Artifact 引用。

## 已验证

- 相同事实、范围与时间产生相同 payload/hash。
- 编辑副本产生新 hash，原 package 不变。
- Reconciliation 允许“冻结范围存在但增量为空”。
- 选中 sequence 超出冻结范围会失败。
- Core 拒绝倒置范围、未来 Canonical Head 和超过事件预算的输入。
- SQLite 测试证明其他 Task 事件不会进入 package，即便 through 覆盖它们。
- Demo Client 预览不改变 Task preferred executor。
- Demo Client 错误 hash 拒绝切换，正确确认后才产生 handoff timeline 并更新下一轮执行者。
- Core 测试证明 secret/email 不出现在最终 wire；编辑加入 Bearer 后会先脱敏、增加命中数，
  再产生新 hash，原 package 不变。
- SQLite 集成测试证明错误 hash 零写入；正确确认得到连续两个 Task Event、可按内容地址读取
  精确脱敏 payload；重复同一命令返回原 Ack 且 Canonical Head 不增加。
- Fake Driver 集成测试验证 session/native turn/terminal baseline/resumable 全链路。
- A→B→A 验证 B 收到 Handoff，A 恢复原 native session 并先收到从旧 baseline 开始的
  Reconciliation；两个实际 payload hash 均等于用户确认的 Artifact。
- 测试证明绕过 Preview 的跨 Agent prompt 被 Core 拦截；一次注入的 Driver reject 不记录 sent、
  不接受用户 prompt，使用同一 Artifact 重试后才成功。
- 清空 Projection 后，SessionMap、Context Transfer 状态和 Timeline 均可由 Event 重建。
- 编辑确认同时保存原稿/最终 Artifact，最终版本不含新加入的 Bearer secret；parent 关系在
  Projection rebuild 后保持一致，UI 展示安全字段 diff。
- 显式 opt-out 的 Fake B→A 测试证明恢复同一 A Session、ContextDelivery 为空、prompt reject
  后可重试，且 `session.resumed < opt_out_applied < driver.turn_started`。
- 临时真实 Git 仓库证明相同工作树 snapshot 完全一致，普通文件内容变化会改变 fingerprint；
  可变 fake provider 证明 preview 后漂移会使确认 Conflict 且 Artifact/Event 零写入。
- Git 矩阵测试已覆盖 staged+unstaged 同文件、rename/delete、binary、symlink、Unicode 和超过
  1000 路径时显式降级为 unknown。
- 扩展矩阵另覆盖空仓库、clean、仅 staged/unstaged、ignored 默认排除、executable bit、
  detached HEAD、linked worktree 与含空格/换行路径。
- `GitWorkspaceSnapshotV1` 的 wire test 锁定 repo root、HEAD、branch/detached、哈希化 worktree
  identity、status format/fingerprint；真实 linked worktree 测试证明 identity 不同。
- Turn 边界集成测试证明 start Snapshot 先于 `turn.started`、end Snapshot 晚于 terminal event；
  `not_repository` 失败只写稳定诊断，两个边界失败时 Driver 仍完成 Turn，Projection rebuild 一致。
- `workspace_snapshots` Projection 与 Core/Tauri/TS 查询按 Task 最新优先返回 1–100 条摘要；成功项
  保留 Git/change summary/count，失败项保留稳定 diagnostic，rebuild 后逐字段相同。
- 本机用户显式 Diff 命令以 0700 临时目录、独立 `GIT_INDEX_FILE` 和临时 object store 从
  HEAD/空树构造最终 tree；staged/unstaged/untracked/binary 均进入 binary/full-index Patch，
  真实 index/status 字节不变且真实 object store 不增加文件。
- Patch 上限 1 MiB，前后 fingerprint 漂移或输出截断均零写入。成功时 Artifact、
  `workspace.diff_captured` 与可重建 Projection 原子提交；只有同 fingerprint 的 Context 才引用。
- React 提供固化按钮并提示原始 Patch 可能含源码/敏感内容；Artifact metadata ruleset 0 明确表示
  未脱敏，命令拒绝 SystemRecovery/RemoteDevice 等非本机用户 actor。
- Test Artifact 集成测试证明只有 `SystemRecovery` 可写入，Bearer/邮箱在持久化前脱敏，报告
  正文不进入 Event，Context Package 引用同一 Artifact，Timeline rebuild 一致。
- resume failure 异步集成测试证明 missing/incompatible 状态、稳定 failureKind、原始错误零泄漏，
  并在 Projection rebuild 后保持一致。
- 下拉切换先预览、确认后更新，页面 1440×900 无头 Chrome 基础渲染无布局溢出。

验证基线：Rust workspace `106 passed, 1 ignored`；前端 `25 passed`，fmt、clippy、typecheck
和 production build 通过。
2026-09-03 Tauri/Vite 启动冒烟通过，app data 数据库只读核验 `user_version = 2`、
`quick_check = ok`、task_artifacts 表存在。Browser 通过只读查看与确认门槛检查；原生保存/取消
尚待 UI 验收，不能以文件写入单测替代。详细证据见下方 Artifact 实施记录。

## 2026-09-03：查看入口前的基础边界收口

本节记录 Artifact 查看入口之前的基础边界阶段（当时尚未新增查看 UI），落点如下：

- Store 的 `migrate` 引入 `user_version = 1`。旧版没有版本标记（0），新表、全部 Projection
  回填及版本标记在一个 Immediate 事务内；任何 replay 错误全部回滚。未来数据库版本显式拒绝。
  Canonical Event、Artifact/blob、dedup Ack 不删除或重写。后续 schema 修改必须新增迁移版本。
- `CoreRepository::workspace_diff_artifact_ref` 新增 after/through 参数；SQL 同时限定 Task、
  fingerprint 和 sequence。集成测试先冻结预览再固化同 fingerprint 的新 Artifact，验证旧包
  逐字段/hash 不变，空区间/其他 Task 不引用，Projection rebuild 保真。
- Runtime 的 `run_bounded_bytes` 返回原始 stdout/stderr，复用原有输出预算、期限与 drain；
  原 `run_bounded` 保留用于文字探针。Git Patch 走 bytes；status/path 走严格 UTF-8 解码，
  不能把 `from_utf8_lossy` 结果当作可导出的原始 Patch。测试使用非 UTF-8 文件正文验证保真，
  并从 Git 原始输出验证文字接口拒绝非法编码；未在 macOS APFS 上声称覆盖非法字节文件名。
- 每条 Git 业务命令前，以同一环境有界查询生效配置的 `filter.*.clean/process` 键，含 include。
  命中就返回 `external_filter_configured`，不打印配置名/值、不运行过滤器、也不静默禁用转换后
  伪称结果等价。所有命令统一关闭 fsmonitor、hooks、自动维护和可选 index 缓存，所有 diff
  关闭 external diff/textconv。测试的 marker 程序均未执行，用户 index 与 object store 保持不变。
- 设置 `GIT_NO_LAZY_FETCH=1`、空 `GIT_ALLOW_PROTOCOL`、`GIT_TERMINAL_PROMPT=0`，避免补拉或
  交互认证。依据：[Git 环境变量](https://git-scm.com/docs/git#_environment_variables)；过滤器
  行为依据：[gitattributes filter](https://git-scm.com/docs/gitattributes#_filter)。
- 子目录项目的 status/stat/Patch 均以 `-- .` 限定，porcelain repo-relative 路径从 repo root
  取 hash；同仓库子目录外文件不进入 Patch。HEAD 直接解析同一 status 的 branch.oid，仅
  `(initial)` 表示 unborn；超时或损坏输出不再隐式变为空仓库。repo/git/object 路径仅去 Git
  输出的末尾换行，不再用 trim 丢弃合法首尾空白。
- Demo `captureWorkspaceDiff` 明确报不可用；测试证明 Task、sequence 和 Timeline 未变化。

新增/扩展回归入口：Store 的 `legacy_migration_backfills_*`、`migration_refuses_*`、
`captures_an_immutable_diff_artifact_*`；Workspace 的 `rejects_clean_and_process_*`、
`does_not_execute_fsmonitor_*`、`preserves_non_utf8_patch_*`、`rejects_non_utf8_text_*`、
`keeps_subdirectory_*`；Runtime 的 `preserves_non_utf8_bytes_*`；Demo Client 的不可用测试。

注意边界：Patch 是 HEAD 到最终工作目录的净差异，不保留 staged/unstaged 两层独立状态；
若工作树还原 HEAD，暂存层独有内容不会出现在 Patch。现有 fingerprint 不对大文件/符号链接
计算正文 hash，不能用作完整 tree identity。过滤器检查与业务命令之间仍有配置竞争窗口，
子模块自有配置、用户全局转换语义和 Windows 路径/权限尚未验证；这些防护不等于恶意 `.git`
通用沙箱，后续扩展 Git 支持前须补专项验证。

## 2026-09-03：受限 Artifact 查看/导出

已落地 Task 归属 Projection/v2 原子迁移、原稿/最终稿分页、LocalUser/主窗口限制、32 KiB
分段读取及完整性校验、只读弹窗、敏感内容确认和原生字节不覆盖导出。正文保持原始 bytes，
JSON 格式化和非 UTF-8 hex 仅用于显示。跨 Task/actor、错误 hash/预算、损坏 CAS、迁移与
rebuild、恶意 HTML/ANSI/双向字符和本地文件写入边界已进入回归。

接口、文件入口、验证结果、未完成原生对话框验收与下一 agent 操作步骤集中维护在
[`m1-s5-artifacts.md`](./m1-s5-artifacts.md)，此处不复制实现细节。

后续只读结构化 Diff 已完成文件/hunk/原新行号、模式/rename/binary、预算与无效格式降级，
并有真实 Git 输出回归和浏览器内存测试入口。详见
[`m1-s5-diff-view.md`](./m1-s5-diff-view.md)。此查看器不是 M2 的完整 ReviewRun/Findings 系统。

## 2026-09-03：结构化脱敏与离线诊断副本

Raw Frame schema/path allowlist、有界离线 JSONL 导出与二次检查已实现。未知/敏感键默认
丢弃，所有输入/输出有结构与字节预算；先验证再原子不覆盖落盘，版本/来源和规范 JSONL
受二次检查。只输出诊断副本，不改变 live 审批 ID 或 Canonical 用户输入，不启动 runtime。
完整接口、预算、14 项新增回归及下一步边界见
[`m1-s5-raw-diagnostics.md`](./m1-s5-raw-diagnostics.md)。这不代表生产 Recorder 或 CI 已接入。

## 下一接手步骤

1. 完成 macOS 原生保存/取消与已有文件拒绝的 UI 冒烟。结构化 Diff 已完成并有浏览器
   复现入口；保持 Task/Artifact/hash 绑定，不增加任意路径查询、补丁应用或正文自动展开。
   Computer Use 未识别当前未打包进程，验收步骤见 Artifact 实施记录；不能标记为已通过。
2. 为 Test Artifact 接入受控采集器时保持“结构化 argv、固定 allowlist、明确用户动作”；不得在
   Context builder 或后台扫描中执行任意 package script。
3. 结构化 allowlist、离线副本导出与二次扫描已实现；审计旧 M0 summary report 后继续
   生产 Recorder 分片/压缩/保留和 fixture CI 接线。不能把旧 summary fixture 当作新诊断格式；
   字段/版本扩展与接手步骤见 Raw Diagnostics 实施记录。
4. Codex Beta 已复用同一 Port/Context/错误分类，继续完成真实 Turn、Cancel 与 Interaction；
   ACP/Copilot 接入仍须各自 M0 事实，不能复用 Fake 或 Codex capability 结论。详见
   [`m1-s4-codex-driver.md`](./m1-s4-codex-driver.md)。

## 明确未完成

- 原生 Artifact 保存对话框 UI 验收、完整 ReviewRun/Findings/复审版本链，以及设计 §22.3 的 Checkpoint/Adoption/崩溃场景矩阵；受限查看/导出、只读结构化 Diff 与自动回归已完成。
- 受控 Test Runner/外部采集适配器；Test Artifact 接收与持久化契约已经完成。
- 生产 Raw Frame Recorder、仓库级 fixture CI 和完整敏感数据审计；结构化 allowlist 与离线导出/检查已完成，高熵候选不等于全覆盖保证。
- Reconciliation opt-out 的 Recovery Center 操作。
- Codex/Cursor 等外部 Product Driver 的同等闭环（仍受各自 M0 证据门约束）。
