# dogcodes 详细设计文档

当前实施范围、完成状态和排程统一见 [当前工作清单](CURRENT_WORK.md)（2026-10-14）。本文的远期方案不等于当前已承诺交付；下列带日期的更新仅保留决策过程，不作为执行队列。

当前范围（2026-10-16）：悬浮岛窗口、入口及专属实现已移除，F3 退出范围；桌面和手机 App 可复用的 Core/后台服务接口及状态投影继续保留。项目运行方式自动分析与一键运行已实现并补录为 D10，见 §17.11 及[功能核对记录](implementation/project-run-capabilities-2026-10-14.md)。

### 历史决策更新

> 2026-09-27 审查简化：三类入口与既有 Agent 保留；Agent 在项目目录自行读取代码和 Git 差异，取消审查专用冻结材料、分组检索与强制二次复核。逐轮入口检查当前实现，Git 历史/分支按所选对象比较。非 Git 撤销及 Diff 所需原始内容记录保留。替代下方历史审查编排方案，见 [审查简化记录](implementation/review-simplification-2026-09-27.md)。

> 2026-09-27 会话永久删除：删除入口仅位于已归档会话；确认后不可恢复，清理消息、审查报告、历史 Diff 及会话快照，移除“最近删除”界面。共享内容仍有其他会话引用时保留；项目源码和 Agent 原生历史不删除。替代 2026-09-25 的可恢复删除方案。见 [实施记录](implementation/conversation-permanent-deletion-2026-09-27.md)。

> 2026-09-25 会话删除：已实现可恢复删除、最近删除列表和恢复；普通/置顶/归档会话复用侧栏入口，确认后执行。主任务或审查排队/执行/取消中禁止删除。删除状态持久化，保留消息、审查、历史 Diff 与原生会话；恢复原列表位置，不自动清空，不删除项目代码。具体设计与验证见 [实施记录](implementation/conversation-deletion-2026-09-25.md)。本条更新下方历史“删除未实现”的状态。

> 2026-09-24 更新：完整会话导出暂缓；已实现会话手动重命名并持久保存，后续消息不覆盖。灵动岛接入独立审查的类型、Agent、状态、分组进度，审批优先，终态展示 60 秒；不改变主 Agent 与取消隔离。详见 [实现记录](implementation/conversation-island-2026-09-24.md)。

> 2026-09-24 审查修复闭环：逐轮与项目 Diff 共用问题选择；发回原对话主 Agent 修复，记录原报告、选择范围和修复轮次。复审独立使用修复轮次的新冻结版本及既有审查 Agent；原材料重试保持原语义，不自动标记问题已解决。实现、真实并发和桌面验收边界见 [实施记录](implementation/review-closure-2026-09-24.md)。

> 2026-09-24 审查编排升级：实现按目录/大小分组、冻结材料的上下文检索、独立 Session 的 AI 二次复核；逐轮与项目 Diff 共用三个原入口和既有 Agent。展示各组进度、材料缺口和复核结论，失败/取消保留初审；旧快照不读取当前文件补历史。实现边界见 [审查编排记录](implementation/review-workflow-2026-09-24.md)，更新下方此前“后续实施”的状态。


> 2026-09-23 审查质量升级：在代码审查、安全检查、性能分析及逐轮/项目 Diff 双入口上，增加结构化问题、历史位置校验与跳转、基础规则和逐文件覆盖；继续复用已有 Agent 和 UI。覆盖自报与程序位置校验不等于缺陷已确认。分组、同版本上下文检索和独立 AI 复核后续实施。详见 [实现与验证记录](implementation/review-quality-2026-09-23.md)。

> 2026-09-22 审查入口决策：保留逐轮历史 Diff 与三个审查入口；项目 Diff 面板同时提供代码审查、安全检查、性能分析及独立 Agent 选择。项目审查采用当前比较范围的固定快照，支持原材料重试，结果明确关联比较来源。实现见 [项目审查记录](implementation/project-review-2026-09-22.md)。


> 2026-09-19 实施修订：Hub 只读委派通过已完成轮次的底部操作栏发起。复用全部已接入且就绪的 Agent，在独立临时目录和独立原生 Session 中分析冻结输入；共享连接型驱动另建独立连接，菜单按主对话规则对安装去重；主 Agent、主 Run、SessionMap 均不切换。`review.started` 固定请求、来源、输入及 SHA-256，`review.updated` 保存状态与结果；重试创建新 Run、复用原输入。取消只作用于审查会话，服务重启将失联审查标为 interrupted、不自动重放。结果按来源附在原轮次并在主任务下一轮交付有界摘录。历史 Diff 校验 Artifact 后呈现保存的补丁/前后内容，无当前文件回退。Fork 创建同项目新任务，原子保存截至选中轮次的历史来源。Copilot SDK 暂缓。详见 [实现与验收边界](implementation/turn-review-2026-09-19.md)。
## 多 Coding Agent 本地中控 · 首版架构与实现基线

| 项目 | 内容 |
|---|---|
| 文档版本 | **v0.2 实施对齐修订**（保留原文件名以维护引用） |
| 日期 | 2026-09-19 |
| 状态 | M1 实施基线；核心会话链路已落地，Copilot SDK 暂缓；逐轮只读 Delegation 首批已实现、真实 Agent 与剩余验收待完成；M1.5/M2 部分能力提前交付 |
| 需求基线 | `agent-hub-requirements-v1.1.md` |
| 首发平台 | macOS 14+；Windows 10/11 x64 随后 |
| 技术栈 | Rust + Tauri 2 + React/TypeScript + SQLite |
| 首版运行形态 | 桌面 UI + 独立 Core 后台服务；无用户 CLI、无 Hub 云服务；硬件网关后续接入同一 Core |

---

## 0. 本文结论

1. **首版交付桌面应用及其本地 Core 后台服务**。服务按需启动并独立持有 Agent 连接，桌面关闭不影响执行；不创建面向用户的 CLI。
2. **Core 与 Tauri 壳解耦但同进程运行**。Core 只依赖领域端口，不依赖 `tauri`、WebView、窗口对象或平台 UI API。
3. **命令与事件协议传输无关**。Tauri 只是首个 Adapter；未来 daemon 可以复用同一协议。
4. **Canonical Event Store 是 Hub 状态事实源**，查询表是事务内更新、可重建的 Projection；UI 不是事实源。
5. **UI 实时流使用 Tauri Channel**，每个事件带全局 `sequence`；窗口重载或漏收后用 `sync(after_sequence)` 补齐。
6. **Agent 原生会话、认证和配置归 Agent 所有**。Hub 仅保存原生 Session ID、能力快照和本地映射，不复制供应商完整会话库，不读取凭据。
7. **项目文件归项目所有**。代码和文档保留在原目录、候选 worktree 或临时副本；Hub 只持久化必要的 Patch、Checkpoint 元数据、评审与可追溯产物。
8. **Hub 数据只在本机落盘且不上传**。首版不做应用层加密，依赖操作系统账户权限、FileVault/BitLocker；不默认上传遥测。
9. **ACP 首版固定稳定 v1**。draft v2 只进入独立实验与 fixture，不进入 Production Driver；供应商扩展保留为 opaque extension。
10. **Codex 首选 stdio App Server**。启动时按用户安装版本生成/校验协议能力；不依赖实验 WebSocket。
11. **Copilot SDK 禁用 bundled CLI**。只连接用户已安装且版本兼容的 Copilot CLI；若 M0 不能满足该边界，则 Copilot 退出 M1 纵向切片，而不是随应用重分发 runtime。
12. **权限与沙箱继续由 Agent 执行**。Hub 只保存并透传 Agent 给出的审批/提问请求与选项；worktree 是污染隔离，不是安全沙箱。
13. **按协作目的保存来源**：Handoff 使用确定性包；Review 保存范围描述并由 Agent 自行读取；未来 Comparison 的候选共享同一 Checkpoint/Common Task Package。
14. **Git 并行以 BaselineTree 为根**。Candidate Delta 永远等于 `CandidateFinalTree - BaselineTree`，不得把用户比较前已有 dirty 内容重复写回。
15. **详细设计中的供应商方法名不是永久公共 API**。Driver 层吸收版本变化；兼容矩阵与生成 schema 由 M0 固定。
16. **硬件不是后接的第二套中控**。从首版起，桌面主界面、手机 App 与未来硬件共享版本化 `CompanionProjection`、`CommandEnvelope` 和 Task 执行者语义；硬件只连接未来的 Device Gateway，不直接连接 Agent。首版不实现 USB/Wi-Fi、配对与语音转写。

## 1. 设计目标与非目标

### 1.1 设计目标

- 用一个稳定的 Core 统一多个 Agent 的安装发现、会话生命周期、流式事件、审批、提问、Diff、用量与额度。
- 在不共享供应商原生 Session 的前提下，形成可审计的 Canonical History，并支持 Agent 间 Handoff/Reconciliation。
- 为 Delegation、Review、Parallel Compare 和 Cross Review 提供确定性编排、隔离、取消、失败恢复和结果采纳。
- 支持主窗口关闭/隐藏后 Agent 继续执行；客户端只消费 Core 状态，不拥有业务状态。
- 所有不可逆写入均有预检、用户确认、事务日志与失败后的可恢复路径。
- 通过能力模型表达“原生、适配、补齐、提示约束、不支持”，不因统一协议而伪造能力。

### 1.2 首版非目标

- 不实现通用 Agent、模型代理或 API Key 管理器。
- 实现本地 Core 后台服务；不实现远程 Web 控制、用户 CLI 或多用户协作服务器。
- 不替代 Agent 的权限系统、沙箱、终端执行器、MCP 配置或原生会话存储。
- 不保证不同 Agent 的内部推理、工具调用或上下文可以无损迁移。
- 不提供“自动选择最优 Agent”的黑盒路由；额度感知推荐必须由用户确认。
- 不在 M1 实现社区动态 Driver、Skill 安装同步、写入型 Delegation、代码候选采纳；悬浮岛已移除，不再列入后续窗口规划。
- 不把临时 worktree、只读 Prompt 或隐藏窗口宣传为安全隔离。

## 2. 设计原则

### 2.1 六边形边界

Core 只定义领域 Port：`EventStore`、`ProjectionStore`、`ArtifactStore`、`AgentDriver`、`QuotaProvider`、`WorkspaceBackend`、`Clock`、`IdGenerator`、`SecretRedactor`、`PlatformNotifier`，并从首版提供 `CompanionProjection` 的查询与有序订阅。Tauri、SQLite、Git CLI、ACP、Codex App Server、系统窗口与未来 Device Gateway 均为 Adapter。首版不为尚未实现的 USB/Wi-Fi 或语音链路创建空 Port；`DeviceTransport` 与 `TranscriptionProvider` 在硬件阶段作为 Adapter 边界加入，不反向改变 Core 命令语义。

### 2.2 事实、投影与原始帧分层

- **Canonical Event**：Hub 已理解并承诺兼容的领域事实。
- **Projection**：为 UI/搜索构建的当前状态，可由事件重建。
- **Raw Frame**：脱敏后的供应商原始输入，仅用于兼容调试与 fixture；不是业务事实。
- **Artifact**：体积较大或需要独立版本的内容，如 Patch、Diff、终端输出、测试报告、Handoff 文档。

三者不可互相冒充。Driver 无法识别的字段进入 `vendor_extensions` 或 Raw Frame，同时产生可见的 `driver.unknown_event_observed` 诊断事件，不静默丢弃。

### 2.3 明确所有权

| 数据 | 所有者 | Hub 行为 |
|---|---|---|
| Agent 登录态、Token、Cookie、API Key | Agent | 不读取、不保存、不迁移 |
| Agent 原生会话库 | Agent | 保存 Session ID 映射；必要时调用官方 list/resume |
| 项目代码和文档 | 用户项目 | 原目录/worktree 中执行；不上传 |
| Canonical History | Hub | SQLite 本地保存，供统一 UI 与交接使用 |
| Candidate Patch、Review、Handoff | Hub | 本地 Artifact 保存，按保留策略清理 |
| 供应商额度状态 | Agent/供应商 | Hub 保存只读快照和来源，不修改账户 |

### 2.4 可恢复优先于自动化

任何会改变用户项目或 Agent 配置的操作都必须具备：预检 → 预览 → 用户确认 → 操作日志 → 验证 → 可恢复/人工处理入口。自动清理只针对 Hub 创建且所有权可证明的资源。

### 2.5 版本变化被限制在 Adapter

领域事件和 UI Contract 不直接暴露供应商枚举或不稳定 payload。每个 Driver 负责将其固定版本 schema 映射到 Canonical Event；供应商新增字段通过扩展容器保留。

## 3. 系统上下文与运行拓扑

```text
┌──────────────────────────────────────────────────────────────────┐
│ dogcodes Desktop Process                                     │
│                                                                  │
│  React Main Window       React Island Window                     │
│          │ command/channel       │ read-only channel             │
│          └──────────────┬─────────┘                               │
│                         ▼                                        │
│               Tauri Contract Adapter                             │
│                         │                                        │
│               ┌─────────▼─────────┐                              │
│               │ Rust Core Runtime │                              │
│               ├───────────────────┤                              │
│               │ Domain Services   │                              │
│               │ Event/Projection  │                              │
│               │ Orchestrator      │                              │
│               │ Process Supervisor│                              │
│               └───┬──────┬─────┬──┘                              │
│                   │      │     │                                 │
│              SQLite/CAS  Git   Platform Overlay                  │
└───────────────────┼──────┼───────────────────────────────────────┘
                    │      │ structured argv / stdio / RPC
        ┌───────────▼──────▼───────────────────────────────────────┐
        │ User-installed Agent processes                           │
        │ Codex · Cursor · Copilot · Claude Code · ...              │
        └──────────────────────────────────────────────────────────┘
```

未来硬件阶段在 Core 外增加 Device Gateway。硬件只消费经过裁剪的 `CompanionProjection`，并把受限设备动作转换为现有 Core Command；它不读取 Event Store、不直连 Agent，也不获得 Tauri 命令全集。

### 3.1 进程模型

- 桌面应用单实例。第二次启动只激活主窗口并转发打开的项目路径。
- 每个 Hub 启动的 Agent Runtime 是独立子进程或独立 SDK 连接。
- Core 使用 `tokio` 多线程 runtime；数据库写入和单条原生连接的协议收发按各自契约串行化。不同 Agent 或同一 Agent 的多个独立对话可以同时修改同一项目的共享目录，不设项目级单写者锁或普通对话排队。Driver 按独立 Session 路由消息、审批和取消，不能把连接内的协议顺序要求扩大为 Provider 或项目级执行互斥。现有 Review 文件事务及 Agent 升级的操作互斥与此区分，不用它们限制普通对话之间的并发。
- 主窗口和审查窗口可以销毁并重建；Core 生命周期绑定桌面进程而不是窗口。
- 用户在外部终端启动的 Agent 不纳入进程监督，只能通过显式 Hook 观察。

### 3.2 启动顺序

1. 获取应用单实例锁。
2. 解析操作系统应用数据目录并检查权限。
3. 先只读检查既有 SQLite；健康库设置 WAL、foreign keys、busy timeout。当前未发布阶段只初始化空库，不执行旧数据迁移、补列或启动回填。
4. 扫描未完成操作日志、遗留 Candidate Workspace 与自有子进程记录；只报告，不盲目删除。
5. 重建/校验必要 Projection，启动 Core actor。
6. 扫描 Agent Installation 与能力缓存；昂贵探针延后后台执行。
7. 创建主窗口。
8. UI 获取 `BootstrapSnapshot`，随后订阅有序事件流。
9. 恢复可恢复的运行状态；不能恢复的 Instance 标为 `interrupted`，提供重新连接/重试。

### 3.3 关闭顺序

- 关闭主窗口默认隐藏到托盘，不结束运行任务。
- 用户显式退出时，若有活动任务，展示 `继续后台 / 中断并退出 / 取消`；Core 后台服务独立存活，因此“继续后台”允许桌面退出；中断必须显式提交到服务，不能仅靠退出 UI。
- 中断流程：停止接受新命令 → 向 Driver 发 interrupt → 等待宽限期 → 终止 Hub 自己创建的进程树 → flush Event Store → 标记未完成操作 → 关闭 DB。
- 永不终止外部终端中由用户创建的 Agent。

## 4. 仓库与模块结构

建议使用 Cargo workspace + pnpm workspace。首版不创建 CLI 包。

```text
dogcodes/
├── Cargo.toml
├── rust-toolchain.toml
├── pnpm-workspace.yaml
├── package.json
├── apps/
│   └── desktop/
│       ├── src/                         # React UI
│       ├── src-tauri/                   # Tauri binary / commands / capabilities
│       └── tests/
├── crates/
│   ├── domain/                          # IDs、值对象、状态机、领域错误
│   ├── protocol/                        # Command/Event/Query DTO + TS 生成
│   ├── core/                            # Use cases、actor、orchestrator
│   ├── store/                           # SQLite Event Store/Projection/CAS
│   ├── driver-api/                      # AgentDriver/QuotaProvider traits
│   ├── acp-driver/                      # 稳定 ACP v1 client adapter
│   ├── codex-driver/                    # Codex App Server adapter
│   ├── copilot-driver/                  # Copilot Rust SDK external-runtime adapter
│   ├── runtime/                         # 进程、stdio、PTY、监督、重启
│   ├── workspace/                       # Git/非 Git checkpoint/workspace/adoption
│   ├── quota/                           # collector 调度与归一化
│   ├── redaction/                       # 脱敏规则
│   └── platform/                        # macOS/Windows overlay + notification
├── packages/
│   ├── protocol/                        # Rust 生成的 TypeScript 类型
│   ├── ui/                              # 共享 UI primitives
│   └── test-fixtures/
├── drivers/
│   └── manifests/                       # 内置 Provider manifest
├── fixtures/
│   ├── acp/
│   ├── codex/
│   └── copilot/
├── docs/
│   ├── adr/
│   ├── compatibility/
│   └── design/
└── scripts/                             # 生成、许可证、fixture 脱敏校验
```

硬件兼容所需的 DTO（`CompanionProjection`、`ActorRef::PairedDevice`、版本与幂等字段）从首版定义在 `agent-protocol`，投影归约逻辑从首版实现在 `agent-core`/`agent-store`。只有开始硬件阶段时才创建 `companion-protocol`、`device-gateway` 与 `transcription`；首版不创建空 crate。USB 与 Wi-Fi 传输依赖 Companion Protocol，不反向进入 Core。

### 4.1 依赖方向

```text
domain <- protocol
domain <- driver-api
domain/protocol/driver-api <- core
core ports <- store/runtime/workspace/quota adapters
core + adapters <- desktop/src-tauri
protocol generated TS <- React UI
```

约束：`domain`、`protocol`、`core` 禁止依赖 `tauri`；Driver crate 禁止直接写 SQLite；UI 禁止直接访问数据库或 Agent 进程。

### 4.2 Rust 与 TypeScript 类型同步

- Wire DTO 在 `agent-protocol` 使用 `serde` 定义，字段统一 `camelCase`。
- 使用代码生成输出 TypeScript 和 JSON Schema，并在 CI 检查生成结果是否有未提交差异。
- Domain 内部类型不直接上 Wire；通过显式 Mapper 避免数据库/供应商结构泄漏到 UI。
- 所有 ID 使用强类型 newtype；Wire 表达为带前缀的字符串。

## 5. 核心领域模型

### 5.1 标识符

内部唯一值使用 ULID，Wire 字符串增加类型前缀。ULID 便于诊断和本地排序，但业务顺序只依赖 Event `sequence`。

| 类型 | 示例 |
|---|---|
| Project / Task / Turn | `prj_01...` / `tsk_01...` / `trn_01...` |
| Installation / Instance / SessionMap | `ins_01...` / `ain_01...` / `ses_01...` |
| Artifact / Checkpoint | `art_01...` / `chk_01...` |
| Comparison / Candidate / Review | `cmp_01...` / `can_01...` / `rev_01...` |
| Pending Interaction | `pin_01...` |

### 5.2 聚合与关系

```text
Project 1 ── * Task
Project 1 ── * WorkspaceRegistration
Task    1 ── * Turn
Task    1 ── * AgentSessionMap
Turn    1 ── 1 ExecutionRun
Turn    1 ── * Artifact
Task    1 ── * DelegationRun
Task    1 ── * ReviewRun
Task    1 ── * ComparisonRun
ComparisonRun 1 ── 1 Checkpoint
ComparisonRun 1 ── * CandidateRun
CandidateRun  1 ── 1 CandidateWorkspace
CandidateRun  1 ── * ReviewRun
Installation  1 ── * AgentInstance
Installation  1 ── * AgentSessionMap
```

### 5.3 关键状态机

**ExecutionRun**

```text
queued -> starting -> running -> waiting_for_input -> running
                         |   -> waiting_for_approval -> running
                         |   -> interrupting -> interrupted
                         |   -> completed
                         └──> failed
starting/running -> disconnected -> reconnecting -> running|failed
```

**ComparisonRun**

```text
draft -> preparing_checkpoint -> materializing -> queued/running
      -> candidates_frozen -> reviewing(optional) -> awaiting_decision
      -> adopting -> adopted
      -> archived_without_adoption
any nonterminal -> cancelling -> cancelled
preparation failure -> failed_preparation
```

**CandidateRun**

```text
queued -> materializing -> ready -> running -> freezing -> frozen
                                      |          └-> failed_freeze
                                      ├-> failed
                                      └-> cancelling -> cancelled
frozen -> adopted|rejected|retained
```

**ReviewRun**

```text
draft -> snapshotting -> running -> completed -> superseded(optional)
                           |-> failed
                           └-> cancelled
```

非法状态迁移在 Domain 层返回 `Conflict`，不依靠 UI 隐藏按钮维持不变量。

### 5.3.1 Task 执行者语义

Task 不保存含义模糊的“当前 Agent”。从首版起使用以下状态：

```text
TaskExecutionState
├── preferred_executor            # 下一轮默认 Agent
├── active_primary_run?           # 当前主 Turn；执行者由 Run 冻结
├── secondary_runs[]              # Delegation / Review / Candidate 等
├── handoff_policy
└── version                       # 乐观并发与外部控制端校验
```

领域不变量：

- `ExecutionRun.executor` 在 Run 创建时冻结，启动后不可改投；取消必须是独立显式命令。
- 修改 `preferred_executor` 只影响下一轮，不中断、不重启当前 `active_primary_run`。
- 新 Turn 的目标 Agent 按“本轮显式 override → `preferred_executor`”解析并写入 Run；提交后不再随 Task 设置变化。
- 若目标 Agent 与 Task 上一原生 Session Agent 不同，Core 执行 Handoff；切回旧 Agent 时执行 Reconciliation。
- Project 不拥有唯一“当前 Agent”，因为不同 Task 以及同一 Task 的 Review/Delegation/Candidate 可以并行。
- 所有写入 `preferred_executor` 的入口均携带 Task `expected_stream_version`；过期选择返回 `Conflict` 和最新 Task 摘要。

### 5.4 Project Context Index

Project Context 只保存索引和必要 Hub Artifact，不复制整个代码库：

```text
ContextEntry
├── id
├── project_id
├── kind: requirement | glossary | adr | standard | artifact | source_file
├── title
├── locator: local_path | artifact_id | canonical_turn_range
├── content_hash
├── trigger_hints[]
├── sensitivity: normal | potentially_sensitive
├── discovered_by: user | hub | agent
└── updated_at
```

首版索引用户显式添加的需求文件，以及项目中的 `AGENTS.md`、`CLAUDE.md`、`CONTEXT.md`、ADR 和贡献规范。读取发生在本机，只在构建具体上下文包时按需加载。

## 6. Core 命令、查询与事件协议

### 6.1 Command Envelope

```rust
struct CommandEnvelope<T> {
    schema_version: u16,
    command_id: CommandId,
    idempotency_key: String,
    actor: ActorRef,              // local_user | system_recovery | paired_device
    expected_stream_version: Option<u64>,
    issued_at: Timestamp,
    payload: T,
}
```

```rust
enum ActorRef {
    LocalUser,
    SystemRecovery,
    PairedDevice(DeviceId),       // 首版保留可序列化枚举；硬件阶段启用入口
}
```

`actor` 只表达审计身份，不授予权限。Tauri Adapter 与未来 Device Gateway 分别完成认证/授权，再提交同一 Command；Core 仍校验 payload、scope 对应能力和 `expected_stream_version`。不接受任意字符串 Actor，避免未来接入端伪装系统身份。

长任务不得让 Tauri command 持续等待；命令提交后由事件流报告进展。相同 `idempotency_key` 返回原 Ack，不重复启动 Agent 或写项目。

```rust
struct CommandAck {
    command_id: CommandId,
    accepted: bool,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
}
```

### 6.2 Event Envelope

```rust
struct EventEnvelope {
    schema_version: u16,
    sequence: u64,                // SQLite 全局单调序号
    event_id: EventId,
    stream: StreamRef,            // type + id
    stream_version: u64,
    event_type: String,
    occurred_at: Timestamp,
    recorded_at: Timestamp,
    correlation_id: CorrelationId,
    causation_id: Option<String>,
    source: EventSource,
    payload: serde_json::Value,
    vendor_extensions: Option<serde_json::Value>,
}
```

- `occurred_at` 是 Agent/业务发生时间，`recorded_at` 是 Hub 落库时间。
- 只承诺 `sequence` 的 Hub 全局顺序；供应商顺序另存 `source_sequence`。
- `stream_version` 用于聚合并发控制。
- 事件不可原地修改；当前未发布阶段不实现旧 payload Upcaster 或数据库升级，未来发布兼容策略需独立设计。

### 6.3 Query 与同步

主要查询：

- `bootstrap()`：设置、Project/Task 摘要、Installation、活动运行、最近额度和最新 sequence。
- `sync_events(after_sequence, limit)`：补齐持久化事件。
- `get_task_timeline(task_id, cursor, filters)`：Projection 分页。
- `get_artifact_metadata/open_artifact_range`：分段读取大 Artifact。
- `search_history(query, project_id?, task_id?, cursor)`：FTS5。
- `get_capability_matrix()`、`get_quota_snapshots()`、`get_recovery_items()`。
- `get_pending_interaction_detail(interaction_id)`：返回安全呈现的完整摘要、原生 option 与当前版本；不返回 Raw Frame。
- `get_companion_snapshot()`、`subscribe_companion(after_revision)`：为桌面、手机 App 及未来设备提供经过裁剪的状态；revision 缺口必须重新获取 Snapshot。

### 6.4 UI 有序订阅

1. UI 调用 `bootstrap()` 获得 `lastSequence = N`。
2. UI 通过 Tauri Channel 调用 `subscribe(afterSequence=N)`。
3. Core 在同一订阅 actor 中先回放 `>N`，再无缝切到 live broadcast。
4. UI 只接受下一个连续 sequence；发现缺口时暂停应用后续事件并调用 `sync_events`。
5. 客户端分别订阅 Core 状态；桌面和手机 App 可复用 `CompanionProjection` 与事件同步。未来硬件复用同一 Projection 的低带宽视图。
6. Channel 断开不影响 Core，重连从最后确认的 sequence 恢复。

Tauri 普通 Event 只用于显示、聚焦、主题等低频窗口信号，不承载有序业务流。

### 6.5 Canonical Event 分类

| 分类 | 示例 |
|---|---|
| Project/Task | `project.registered`, `task.created`, `task.archived` |
| Installation | `agent.discovered`, `agent.probe_completed`, `capability.changed` |
| Runtime | `instance.starting`, `instance.ready`, `instance.exited`, `instance.disconnected` |
| Turn | `turn.queued`, `turn.started`, `turn.completed`, `turn.failed` |
| Item | `message.delta`, `tool.started`, `tool.completed`, `diff.updated`, `plan.updated`, `terminal.chunk` |
| Interaction | `approval.requested/resolved`, `question.requested/resolved` |
| Collaboration | `handoff.*`, `reconciliation.*`, `delegation.*`, `review.*`, `comparison.*`, `candidate.*` |
| Workspace | `checkpoint.*`, `workspace.*`, `adoption.*`, `cleanup.*` |
| Quota | `quota.snapshot_recorded`, `quota.collection_failed`, `quota.became_stale` |
| Diagnostics | `driver.unknown_event_observed`, `recovery.item_detected` |

流式文本不按 token 永久落一条 Event。Driver 向内存流发送小 chunk，Store Writer 按 `50ms 或 8KiB` 合并写 `message.delta`；完成时写 `message.completed` 和全文 Projection/Artifact。

### 6.6 错误模型

```rust
enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    CapabilityUnavailable,
    AgentNotConfigured,
    AgentVersionUnsupported,
    AgentDisconnected,
    PermissionRequestExpired,
    WorkspaceBusy,
    WorkspaceMaterializationFailed,
    AdoptionConflict,
    StorageUnavailable,
    ResourceExhausted,
    Cancelled,
    Internal,
}
```

Wire 错误包含 `code`、安全的用户消息、`retryable`、`correlationId` 和可选 `details`。供应商 stderr 与原始 payload 不直接拼入用户消息，而进入脱敏诊断。

## 7. 本地存储设计

### 7.1 数据目录与边界

使用 Tauri/操作系统标准应用数据目录：

```text
dogcodes/
├── app.db
├── artifacts/sha256/ab/<hash>
├── raw-events/<date>/<instance-id>.jsonl.zst
├── operations/<operation-id>.json
├── workspaces/<comparison-or-delegation-id>/
├── backups/agent-config/
├── logs/
└── cache/
```

- 目录按当前用户最小权限创建。
- 不复制 Agent 凭据或完整原生会话。
- 代码/文档保留在用户项目或候选 Workspace；CAS 只放 Hub 必须保留的交接、评审、Patch、日志和证据。
- 首版不使用 SQLCipher；设置和 UI 明示“数据仅保存在本机”。

### 7.2 SQLite 配置

- `journal_mode=WAL`
- `foreign_keys=ON`
- `synchronous=NORMAL`；Checkpoint/adoption 操作日志另做 fsync 与原子 rename
- `busy_timeout=5000`
- 单 DB Writer actor；读连接池只查询 Projection
- 未发布阶段只维护当前数据结构；只初始化空库，不编写旧数据迁移、补字段或跨版本兼容逻辑。

采用 `rusqlite` + 专用数据库线程/actor，避免多个 async task 直接争抢连接。Event append 与同步 Projection 更新位于同一事务。

### 7.3 核心表

```text
events(
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT UNIQUE,
  stream_type TEXT, stream_id TEXT, stream_version INTEGER,
  event_type TEXT, schema_version INTEGER,
  occurred_at INTEGER, recorded_at INTEGER,
  correlation_id TEXT, causation_id TEXT,
  source_json TEXT, payload_json TEXT, extensions_json TEXT,
  UNIQUE(stream_type, stream_id, stream_version)
)

command_dedup(idempotency_key PRIMARY KEY, command_id, ack_json, expires_at)
projects(...)                 tasks(...)
turns(...)                    timeline_items(...)
installations(...)            capability_snapshots(...)
agent_instances(...)          agent_session_maps(...)
pending_interactions(...)     artifacts(...)
context_entries(...)          checkpoints(...)
comparison_runs(...)          candidate_runs(...)
review_runs(...)              quota_snapshots(...)
operation_journal(...)
projection_offsets(name PRIMARY KEY, last_sequence)
timeline_fts USING fts5(task_id UNINDEXED, item_id UNINDEXED, text)
```

Projection 可以冗余查询字段，但不得成为唯一证据。Provider 原始额度值与归一化字段同时保留。

### 7.4 Artifact Store

Artifact 使用 SHA-256 内容寻址：

```rust
struct ArtifactMeta {
    id: ArtifactId,
    sha256: String,
    media_type: String,
    byte_length: u64,
    storage_class: ArtifactClass,
    compression: Option<Compression>,
    redaction_state: RedactionState,
    provenance: ArtifactProvenance,
    retention: RetentionClass,
}
```

写入流程：临时文件 → 流式 hash/可选压缩 → fsync → 原子 rename → 事务写 ArtifactMeta。相同内容复用 blob。删除采用引用标记清扫；只要 Checkpoint、采纳证据或 Review 仍引用就不能删除。

### 7.5 Raw Event Recorder

- 默认只为 Beta/Experimental Driver 开启短期保留；Production 在诊断模式下开启。
- 写入前执行结构化字段屏蔽和高熵 secret 扫描；无法安全分类的字段默认不记录。
- 按 Instance 分片压缩 JSONL，记录 parser/driver 版本。
- 默认 7 天或 200MB，以先到者为准；用户可关闭。
- Fixture 必须二次脱敏，CI 执行 secret 扫描。

### 7.6 保留与删除

| 数据 | 默认策略 |
|---|---|
| Canonical History | 保留；用户删除 Task 才删除 Hub 副本 |
| Agent 原生 Session | 不删除；Task 删除只断开映射 |
| Candidate Patch/Review/采纳证据 | 随 Comparison 记录保留 |
| 未采纳候选工作目录 | 结果持久化后自动清理 |
| Raw Frame | 7 天/200MB |
| 终端大日志 | 30 天/1GB，可配置 |
| Quota Snapshot | 90 天，趋势可降采样 |
| 诊断日志 | 14 天/200MB |

删除 Task 前显示将删除的 Hub 数据；不删除项目文件、Agent 配置或 Agent 原生会话。Blob 通过后台 GC 删除，并提供短暂回收站窗口。

## 8. Agent Driver 与运行时

### 8.1 Driver Port

```rust
#[async_trait]
trait AgentDriver: Send + Sync {
    fn provider_id(&self) -> ProviderId;
    async fn probe(&self, installation: &Installation) -> Result<ProbeReport>;
    async fn capabilities(&self, ctx: &DriverContext) -> Result<CapabilityReport>;
    async fn list_sessions(&self, query: SessionQuery) -> Result<Page<NativeSession>>;
    async fn create_session(&self, req: CreateSession) -> Result<SessionHandle>;
    async fn resume_session(&self, req: ResumeSession) -> Result<SessionHandle>;
    async fn prompt(&self, handle: &SessionHandle, req: PromptRequest)
        -> Result<DriverEventStream>;
    async fn interrupt(&self, handle: &SessionHandle, turn: NativeTurnRef) -> Result<()>;
    async fn respond_to_interaction(
        &self, handle: &SessionHandle, response: InteractionResponse
    ) -> Result<()>;
    async fn list_models(&self, ctx: &DriverContext) -> Result<Vec<ModelInfo>>;
    async fn set_mode(&self, handle: &SessionHandle, mode: ModeSelection) -> Result<()>;
    async fn shutdown(&self, handle: SessionHandle) -> Result<()>;
}
```

登录、安装、doctor 属于 `InstallationService`，不混入活动 Session 接口。Quota 使用独立 `QuotaProvider`。

### 8.2 DriverEvent

```rust
enum DriverEvent {
    SessionReady { native_session_id: String },
    TurnStarted { native_turn_id: Option<String> },
    TextDelta { item_id: String, text: String },
    ReasoningDelta { item_id: String, text: String, visibility: Visibility },
    ToolStarted(ToolCall),
    ToolProgress(ToolProgress),
    ToolCompleted(ToolResult),
    DiffUpdated(DiffSnapshot),
    TerminalOutput(TerminalChunk),
    PlanUpdated(PlanSnapshot),
    UsageUpdated(UsageSnapshot),
    ApprovalRequested(PermissionInteraction),
    UserQuestionRequested(QuestionInteraction),
    TurnCompleted(TurnOutcome),
    VendorExtension { method: String, payload: Value },
    ProtocolWarning(DriverWarning),
}
```

映射规则以“保留供应商语义”为优先：无法无损转成通用 Tool 时保留 `VendorExtension`，UI 用降级卡片展示原始安全摘要。

### 8.3 Capability 模型

```rust
struct CapabilityValue {
    support: SupportLevel,  // Native | Adapter | Polyfill | PromptOnly | Unsupported
    source: CapabilitySource,
    since_version: Option<Version>,
    limitations: Vec<String>,
    runtime_verified_at: Option<Timestamp>,
}

struct CapabilityReport {
    protocol: ProtocolDescriptor,
    features: BTreeMap<CapabilityKey, CapabilityValue>,
    vendor_extensions: Value,
}
```

静态 manifest 只提供预期能力；连接握手和真实探针可以降级但不能无证据升级。能力变化产生新 Snapshot，不覆盖历史 Turn 的能力证据。

### 8.4 Provider Manifest

每个内置 Provider 使用带 schema version 的 TOML/JSON manifest：

```text
id/display_name/homepage
discovery: candidate_executables, platform_paths, version_args
supported_versions: min, recommended, denylist
launch: executable, structured_args, env_allowlist, cwd_policy
protocol: kind, version, framing
login: status_command, login_command, browser_url
capability_expectations
quota_collectors
external_hooks
```

所有命令均为 executable + argv 数组，禁止 shell 字符串。Manifest 不允许定义读取凭据文件的路径。

### 8.5 安装发现与探针

1. 扫描 PATH 和 manifest 明确声明的平台路径。
2. 对每个真实路径去重，记录文件标识、版本和来源。
3. 运行无敏感信息的 `--version`/status 探针，设超时和输出上限。
4. 判断 `Installed / NotConfigured / Ready / UnsupportedVersion / Broken`。
5. 用户选择默认 Installation；多版本并存时不擅自替换。
6. 探针结果缓存并带 executable fingerprint；文件变化后失效。

### 8.6 Runtime Supervisor

- 每个 Instance 一个 actor，拥有 stdin/stdout/stderr、Driver connection、取消令牌和有限队列。
- stdout 只给协议解析器；stderr 进入限流脱敏诊断，绝不与 JSON-RPC 混流。
- 启动超时、握手超时、空闲心跳和最大帧大小均可配置。
- macOS/Unix 以独立进程组启动 Hub 子进程；Windows 使用 Job Object。退出时只清理带本次 ownership token 的子进程树。
- PID 记录必须附启动时间、可执行路径和 ownership token；禁止仅凭 PID 杀进程。
- 协议输入/输出使用有界队列。流式文本可合并；审批、提问、完成和错误事件不可丢弃。
- Driver 崩溃采用有限指数退避；只有声明支持 reconnect/resume 才自动恢复，否则标记 interrupted 并询问用户。

### 8.7 审批与提问

`PendingInteraction` 必须先落库再展示：

```text
id, task_id, turn_id, instance_id, native_request_id,
kind, prompt, options[], command_summary?, diff_ref?,
requested_at, expires_at?, state, interaction_version, response?
```

- UI 只能提交供应商提供的 option ID；Hub 不发明“永久允许”。
- Driver 将原生 option 规范化为 `option_id + display_label + semantic_role?`；`semantic_role` 只能是经该 Driver 版本契约验证的 `approve_once | reject | cancel | approve_persistent | other`，未知时保持 `other`，不得按按钮文案猜测。
- 只有同时存在唯一 `approve_once` 与唯一 `reject|cancel`、摘要可安全展示且请求未过期时，Projection 才发布 `quick_approval_eligible=true`。`approve_persistent` 永不成为客户端或硬件单击默认。
- 响应命令幂等；重复点击返回已有结果。
- 响应必须携带 `interaction_id`、原生 `option_id`、`expected_interaction_version` 与幂等键；Core 在事务内完成 compare-and-set，只有胜出的第一次响应送往 Driver。
- Turn 完成、Agent 通知 resolved 或连接丢失后立即关闭 Interaction。
- App 重启后，只有 Driver 能证明请求仍有效时才允许继续响应，否则显示“已失效”。

### 8.8 登录与 PTY

- 优先调用 Agent 官方浏览器 OAuth/device flow 命令。
- 必须交互时打开临时 PTY 窗口；PTY 输出不进入 Canonical History/Raw Recorder。
- 登录结束立即销毁 PTY；Hub 重新运行状态探针确认 Ready。
- Hub 不解析 Token、不把环境变量注入为第三方 Key、不提供 Key 表单。

## 9. 首批 Driver 专项设计

### 9.1 Codex App Server Driver

**传输**：首版只使用 Hub 启动的 `codex app-server --stdio` JSONL；实验 WebSocket 不进入生产路径。每条 JSON 严格限制大小，stderr 独立读取。

**握手**：连接后只发送一次 `initialize`，`clientInfo.name` 使用经确认的 dogcodes 标识，再发送 `initialized`。未完成握手不发其他请求。

**版本策略**：

- M0 对支持的 Codex 版本运行 `codex app-server generate-json-schema`，将 schema 指纹与兼容 fixture 纳入矩阵。
- Driver 代码只依赖本项目固定的最小 Canonical 子集；额外字段通过 `flatten`/opaque JSON 保留。
- 遇到 server overloaded 类错误采用带 jitter 的有限重试；`turn/start` 等非天然幂等方法只有在确认服务端未接受时才重试。

**主要映射**：

| Codex 概念 | Hub 概念 |
|---|---|
| Thread | Native Agent Session |
| Turn | Native Turn Ref / ExecutionRun |
| Item lifecycle | Tool/Message/Terminal/Diff timeline item |
| server-initiated approval request | PendingInteraction |
| `turn/diff/updated` | DiffSnapshot Artifact + `diff.updated` |
| `thread/tokenUsage/updated` | UsageSnapshot，不等于订阅额度 |
| account/rate-limit surface | QuotaProvider Snapshot |

审批响应完全使用服务端给出的可选 decision；若某版本无法渲染新型 form，请求只能 decline/cancel 或跳转到受支持客户端，不能伪造字段。

### 9.2 ACP Driver

- 首版固定官方 Rust SDK的 stable v1 builder；draft v2 feature 不编译进 release 默认特性。
- 每个 Agent 进程/连接独立初始化并记录双方 protocol version、implementation version 和 capabilities。
- ACP 标准 Session Update 映射为 Canonical DriverEvent；Agent 自定义 method/字段保留为 VendorExtension。
- 文件读写、终端和权限回调实现为 ACP Client 能力，但行为仍遵从 Agent 请求，不额外扩大权限。
- Canonical History 已通过原生 Session 的 mcpServers 注入 stdio MCP 服务；协议扩展仍按各 Driver 真实能力与兼容记录选择。
- ACP Registry 仅作为发现线索；Production 状态必须来自本项目真实账号 contract test。

### 9.3 Copilot Rust SDK Driver

- Cargo 依赖关闭默认 `bundled-cli` feature。
- Hub 显式探测用户 Copilot CLI 路径和版本，再通过 SDK的 external/path transport 连接。
- 不启用示例中的 approve-all；实现真实 PermissionHandler，将请求转为 PendingInteraction。
- SDK 与 CLI 是配套版本。若已安装 CLI 不兼容，UI 显示版本诊断和官方升级入口，不下载或嵌入另一份 runtime。
- M0 必须验证：外部 CLI 登录态复用、create/resume、流式事件、权限、取消和退出清理。任一主能力无法满足则从 M1 切片移出。

### 9.4 其他 Driver 扩展方式

Claude Code ACP adapter、Cursor、Kimi、Antigravity、Grok、OpenCode、DeepSeek、ZCode 在实现前分别提交一份兼容记录：官方入口、安装探测、认证状态、协议版本、能力 fixture、额度来源和已知降级。新增 Driver 只能依赖 `driver-api` 与 runtime ports，不得依赖 UI 或直接写 Store。

## 10. 普通会话与统一时间线

### 10.1 新建 Turn 顺序

```text
User submit
  -> resolve executor (turn override || Task.preferred_executor)
  -> validate Project/Task/Installation/Capability and expected Task version
  -> acquire project write/read lease
  -> append turn.queued
  -> create/resume native Session
  -> build deterministic Prompt Package
  -> append turn.started
  -> Driver.prompt
  -> map and persist stream items
  -> resolve/expire pending interactions
  -> collect post-turn git summary + usage/quota opportunistically
  -> append turn.completed|failed|interrupted
  -> release lease
```

用户输入在启动 Agent 前落入 Canonical History。若 Agent 启动失败，Turn 仍可见并允许原 Agent重试或换 Agent。

### 10.2 双层 Session 模型

`AgentSessionMap`：

```text
task_id
provider_id / installation_id
native_session_id
last_native_turn_id
workspace_locator
driver_version / native_agent_version
created_at / last_used_at
resume_capability
status: active | resumable | missing | incompatible | detached
```

同一 Task 可拥有多个 Agent SessionMap。桌面端或未来设备切换 Agent 只修改 Task 的 `preferred_executor`，不改变已启动 Run 的 `executor`，也不改变 Canonical 时间线。删除 Task 默认只删除映射，不调用 Agent 原生 delete。

### 10.3 Timeline Item 统一模型

UI 基础 item 类型：UserMessage、AgentMessage、ReasoningSummary、Plan、ToolCall、Terminal、FileChange、Diff、Approval、Question、TestResult、Usage、Handoff、Delegation、Review、Candidate、Error、VendorExtension。

每个 item 含：Task/Turn/Run、Agent/Model/Mode、Native Session/Turn/Item、Workspace、时间、状态、Artifact refs、Capability provenance。Reasoning 只有供应商明确提供且允许展示时才记录；不推断隐藏推理。

### 10.4 Git 读侧快照

每个写入 Turn 开始和结束时采集：repo root、HEAD、branch/detached、worktree identity、status porcelain v2、changed paths、diff stat。完整 Diff 按需生成 Artifact。采集失败不使对话失败，但生成显式诊断并降低 Review/Comparison 可用性。

## 11. Canonical History 与上下文构建

### 11.1 ContextPointer

借鉴可组合 Skill 的渐进上下文思想，Hub 不把全部历史塞给每个 Agent，而传递带触发条件的引用：

```rust
struct ContextPointer {
    id: ContextEntryId,
    title: String,
    kind: ContextKind,
    locator: ContextLocator,
    content_hash: String,
    trigger_hints: Vec<String>,
    byte_length: u64,
    sensitivity: Sensitivity,
}
```

M1 使用五个 stdio MCP 工具 `getTurns/searchHistory/getArtifact/getDecisionLog/getChangesSinceCheckpoint`。每轮仅冻结 Canonical 上界，Core 从 SQLite 分页读取，不预加载全史；用户消息不含工具说明。列表按原生消息身份重建上界内的 Canonical 片段，支持跨片段短语搜索并避免可变 Timeline 投影泄入上界后的内容；单条文字 32 KiB 分块，响应预算 256 KiB，决策/变更每请求最多扫描 1024 个事件；对话查询由 SQLite 聚合后分页。沿 nextCursor/nextOffset 可读取超出旧 16 MiB/20,000 条/200 页快照限制的事实。附件仍受单附件 1 MiB、任务归属、来源上界、哈希及 UTF-8 限制。

端点按任务与 executor 隔离，每轮绑定具体 SessionMap，结束/取消/错误撤销；MCP 配置通过原生会话带外注入，不改用户全局配置。返回数据按已存储内容读取，不增加查询脱敏。实际字节与审计边界见 [历史查询实现](implementation/history-query.md)。

### 11.2 确定性 Context Package

```text
ContextPackage v1
├── objective
├── user_constraints[]
├── acceptance_criteria[]
├── completed_work[]
├── decisions[] {statement, source_ref}
├── pending_work[]
├── relevant_files[] {path, status, content_hash}
├── change_summary {base, head, diff_stat, artifact_ref?}
├── tests[] {command, outcome, artifact_ref?}
├── recent_turn_refs[]
├── context_pointers[]
├── delivery_facts[] {sequence, field, sha256, offset, end, total_bytes}
└── provenance {builder_version, built_at, source_sequences}
```

所有字段由 Hub 从用户输入、事件和项目状态确定性构建。缺数据用 `unknown`/空数组，不让 Hub 编造结论。

### 11.3 语义摘要

本节执行留痕要求适用于上下文语义摘要。Git 提交备注的“AI 总结”由用户主动触发，沿用选定 Agent 生成备注并返回编辑框；不要求独立 ExecutionRun、Canonical 执行事件或输入/输出 Artifact，不作为 M1 待补功能。

- 只有用户选择或流程明确请求时才调用某个已安装 Agent。
- 摘要是独立 ExecutionRun，显示 Agent、模型、额度作用域、输入的 sequence 范围和输出 Artifact。
- 摘要不得覆盖确定性字段，只能附加 `semantic_summary`。
- 失败/取消后使用确定性模板继续。

### 11.4 Handoff

触发：当前执行 Agent A 与目标 Agent B 不同。

1. 冻结用户本轮输入前的 Source Sequence 范围。
2. 生成标准 Context Package；按精简/标准/完整选择最近 Turn 和 Diff 内容。
3. 对敏感字段和疑似 secret 做脱敏；引用已有 Artifact，不重复正文。
4. 普通发送自动构建和确认包，不弹出逐次预览。显式编辑形成新版本，并清空不能重新证明的自动交付范围。
5. 创建/恢复 B 的原生 Session，发送 Handoff + 当前用户指令。
6. Driver.prompt 返回流后记录最终 payload hash、来源及事实字节范围；此边界称 Driver 接受，不声称模型收到并理解。

### 11.5 Reconciliation

切回已有 SessionMap A 时，按冻结上界扫描事实，扣除 A 的完整同哈希交付字节范围及自身原生轮次。以前被省略或仅部分交付的事实不受 lastSeenSequence 过滤。该字段只保留原生执行检查点及路由冲突检查。差集仍按交接预算选取，省略项可按需查询，不假装整个序号区间已经交付。

`history.facts_delivered` 是持久化交付账本：交接包在 Driver.prompt 返回流后记录 `driver_prompt_accepted`；MCP 工具结果成功写到 stdout 并 flush 后，桥回传临时 receipt，Core 记录 `mcp_stdout_flushed`。最多保留 64 个待确认响应，超限背压。事实包含源 sequence/字段/哈希/字节区间；区间必须完整覆盖才可排除。引用不确认附件正文，脱敏不确认原文，未知/过期 receipt 不建立覆盖。确认不表示模型理解。新 SessionMap 不继承旧收据，投影重建从 Canonical 事件保留证据。

原生调用拒绝或输出写出失败不产生成功收据；崩溃导致确认丢失时保守重交。取消撤销后续访问，无法取回已交付字节。增量扫描及收据合并仍随任务历史增长，应在后续性能数据支持时引入索引投影，不能以全史内存快照恢复旧限制。

### 11.6 Skill 与 Workflow 边界

- M1 只发现项目中已存在的 Agent 指令文件/Skill，作为 ContextEntry 展示；不安装、不更新、不跨 Agent 复制。
- Hub 自己的 Handoff、Review、Comparison 是确定性 Workflow，不依赖某个供应商 Skill 才能运行。
- M3 可增加 `SkillInventory` 与 `WorkflowTemplate`，区分 `manual / agentSuggested / automatic`；默认禁止自动执行会写文件或消耗多份额度的 Workflow。

## 12. 多 Agent 协作编排

### 12.1 统一 Run 抽象

普通 Turn、Delegation、Review、Candidate 和语义摘要都复用：

```text
ExecutionRun
├── kind
├── parent_run_id?
├── task_id / turn_id?
├── installation_id / native_session_id?
├── input_package_ref
├── workspace_ref
├── write_policy
├── isolation
├── status
├── started_at / finished_at
└── output_artifacts[]
```

Orchestrator 负责关系与状态，Driver 只负责一次原生执行。

### 12.2 Delegation

当前交付为逐轮/项目 Diff 的审查型只读委派。复用现有 Driver，使用独立原生 Session 与 ReviewRun，不替换主 Task 的 `preferred_executor` 或 `active_primary_run`；支持独立取消、按原比较范围重试及带来源的结果回流。

审查直接在项目目录运行；主目录和额外目录传给独立原生会话。审查采用完全权限：Codex danger-full-access/never、Claude bypassPermissions、Grok always-approve、ZCode yolo；其他 Agent 按原生目录选择完全权限，Kimi 按校正后的 auto 语义选择。没有原生绕过模式时，使用普通执行模式并自动回答本次允许；不写入永久允许规则。读取可跨目录，禁止修改由提示词约束，不宣称硬只读沙箱。问答请求仍明确失败。启动预算 1 分钟，单次审查总预算 15 分钟，不再按组设置 3 分钟阶段。

### 12.3 Review

当前只提供代码审查、安全检查、性能分析三个入口，逐轮与项目 Diff 共用 UI 和已有 Agent。

- 输入：轻量目录/模式/base/target/文件名描述，逐轮另附有界需求回复摘录。保留 sourceSnapshot 字段作为比较来源标识，不代表新的冻结代码包。路径和 Git 对象 ID 不做通用高熵替换，消息和报告正文仍脱敏。
- 执行：一次原生工具调用会话；移除宿主分组、冻结检索和独立二次复核引擎，也不采集未改动的关联文件。
- 结果：workspaceReview=true 标记新语义；结构化 findings 或 Markdown 回退。新位置是 Agent 自报，不提供伪造的历史 Diff 跳转/覆盖率。旧类型和来源读取器仅服务已保存的旧报告。
- 重试：新 Run、新 Session，复用范围描述；工作区重新读取最新状态，历史 Git 比较沿用具体对象。旧冻结记录不走新重试，从入口重新发起。
- 展示：复用 ConversationActivity.ActivityRow 和 MarkdownMessage；运行中流式显示回复/工具状态，结束后显示报告并折叠保留执行过程，失败/取消保持部分输出可见。前端共享事件订阅并按序号去重、补齐和重连，既有轮询作为持久状态回读。
- 存储：review.started 保存范围及其 hash，review.activity 约每 250ms 批量保存本次变动的展示项，review.updated 保存状态/结果与最终活动项。活动以独立 runId/revision 投影，不写入主对话工具时间线。取消/失败也刷新尾部输出，服务重启保留已持久化过程。审查不创建额外代码副本；Diff 预览、非 Git 基线、整轮撤销及恢复事务所需 blob/Artifact 保留，不整目录删除 review/。
- 修复：保留选中问题→主 Agent 修复→独立检查当前实现；持久关联原报告和修复轮次。

实际契约见 turn_review.rs、turn_review/workspace.rs 和 packages/protocol/src/index.ts。实施与验证见 [审查简化记录](implementation/review-simplification-2026-09-27.md)。

### 12.4 Comparison Run

创建时冻结 Common Task Package，计算内容 hash；每个 Candidate 都保存同一 hash。Agent 专属模型/模式作为 CandidateConfig，不修改公共任务正文。

调度规则：全局 semaphore 默认 2、硬上限 4；项目上限取全局与项目配置较小值。候选逐个排队，失败不连坐。启动前保存各 Agent 当时可得的 Quota Snapshot，仅提示不阻断。

候选结束必须执行 Freeze；只有 Frozen Candidate 可进入 Review、Cross Review 或 Adoption。Freeze 失败时保留原始回复和 Workspace，禁止自动清理，等待恢复。

### 12.5 Cross Review

- 两候选默认建立 A→B、B→A 两个 ReviewRun，也可指定第三 Reviewer。
- 三候选以上默认由用户指定统一 Reviewer，每个候选一个独立 ReviewRun，共享相同 Profile。
- 盲评只从 Review Package 隐去显示名/模型，不承诺从内容风格中不可推断作者。
- Reviewer 只读取 Frozen Artifact/Result Commit；Reviewer 产生写入时写到 Disposable Workspace 并在完成后丢弃。
- 综合方案是新的显式 ExecutionRun，输入必须引用候选与评审，输出不自动成为胜出者。

## 13. 项目并发与 Lease

```rust
enum LeaseKind {
    MainWorkspaceWrite,
    MainWorkspaceRead,
    CandidateWorkspaceWrite,
    SnapshotRead,
}
```

- 同一 Main Workspace 最多一个写 Lease。
- Read Lease 可并行，但当 Agent 只有 PromptConstrained read-only 时，不把它当强安全保证。
- Candidate Workspace 之间可并行写，各自拥有独立路径。
- Lease 带 owner Run、心跳和取消令牌；进程异常后进入 `suspect`，恢复扫描确认无活进程才释放。
- 用户外部修改无法被 Lease 阻止；Git/文件 fingerprint 在关键操作前后检测漂移。

## 14. Checkpoint 与 Candidate Workspace

### 14.1 WorkspaceBackend

```rust
#[async_trait]
trait WorkspaceBackend {
    async fn inspect(&self, root: &Path) -> Result<WorkspaceReport>;
    async fn create_checkpoint(&self, req: CheckpointRequest) -> Result<Checkpoint>;
    async fn materialize_candidate(
        &self, checkpoint: &Checkpoint, candidate: CandidateId
    ) -> Result<CandidateWorkspace>;
    async fn freeze_candidate(&self, workspace: &CandidateWorkspace)
        -> Result<CandidateSnapshot>;
    async fn preview_adoption(&self, req: AdoptionRequest) -> Result<AdoptionPreview>;
    async fn adopt(&self, confirmed: ConfirmedAdoption) -> Result<AdoptionResult>;
    async fn cleanup(&self, owned: OwnedWorkspace) -> Result<CleanupResult>;
}
```

实现：`GitWorkspaceBackend`、`DirectoryWorkspaceBackend`、`NoWorkspaceBackend`。任何 materialize 失败都禁止退回共享可写目录。

### 14.2 Git 前置检查

1. 解析 repo root、git common dir、当前 worktree、HEAD、branch 和 object format。
2. 获取 MainWorkspaceWrite Lease，短暂冻结 Hub 自己的新写 Turn。
3. 运行 `git status --porcelain=v2 -z --untracked-files=all`，记录路径时保持原始字节/平台编码安全处理。
4. 检查进行中的 merge/rebase/cherry-pick/bisect；存在时禁止创建写入型 Comparison。
5. 枚举 submodule。首版遇到 dirty submodule 直接阻止并说明；递归快照作为后续增强。
6. 枚举 ignored 排除摘要并估算候选空间。用户可显式选择必要 ignored 文件纳入；默认不纳入。
7. 检查磁盘空间、Git 版本和 worktree 支持。

Git 操作统一使用系统 Git CLI 的结构化 argv 与显式 `GIT_INDEX_FILE`，不拼 shell。最小 Git 版本在 M0 根据 `merge-tree`、worktree 和 path 安全测试确定。

### 14.3 Git Checkpoint 算法

Checkpoint 必须不修改真实 HEAD、branch 和 index：

1. 保存 `HeadCommit` 与 `HeadTree`。
2. 将真实 index 复制到 Hub 临时 index，执行 `git write-tree` 得到 `IndexTree`。
3. 在另一个临时 index 中以 `IndexTree` 为起点，对工作区执行等价 `git add -A`，得到包含 staged、unstaged 和允许 untracked 内容的 `BaselineTree`。
4. 用户显式纳入的 ignored 路径只在临时 index 中强制添加。
5. 保存分层证据：`HeadTree -> IndexTree` staged Patch、`IndexTree -> BaselineTree` working Patch、untracked/ignored 选择清单、文件 mode/symlink、属性与 submodule 状态。
6. 用 `git commit-tree BaselineTree -p HeadCommit` 创建隐藏 `BaselineCommit`，消息只含 Checkpoint ID；写入私有 `refs/dogcodes/checkpoints/<id>` 防止 GC，不更新用户分支。
7. 从 BaselineTree 导出路径 manifest 和内容 hash；Checkpoint 记录创建时真实 index checksum，证明没有被 Hub 改动。
8. 临时 index 和中间文件 fsync 后纳入 Operation Journal，再安全删除。

```text
Checkpoint
├── repository_identity
├── head_commit / head_tree
├── index_tree
├── baseline_tree / baseline_commit
├── staged_patch_ref / working_patch_ref
├── included_untracked_manifest
├── excluded_ignored_summary
├── file_manifest_ref
├── submodule_manifest
├── real_index_checksum_before/after
└── builder/git versions
```

### 14.4 Git Candidate 物化

1. 为候选创建临时分支 `dogcodes/candidate/<comparison>/<candidate>`，起点为 BaselineCommit。
2. `git worktree add` 到 Hub 应用数据目录下的 owned path。
3. 校验 HEAD tree 等于 BaselineTree。
4. 恢复显式纳入但无法由普通 checkout 表达的必要内容。
5. 逐文件抽样 + manifest 全量元数据校验；关键小文件全 hash，大文件可先 size/mtime 后后台 hash，但 Agent 启动前必须完成最终一致性结论。
6. 写 ownership marker：Hub installation ID、Comparison/Candidate ID、repo identity、branch、created_at。
7. 校验成功才将 Candidate 标为 Ready 并交给 Agent。

候选分支和私有 refs 是 Hub 临时资源，不属于用户发布历史；UI 清楚标记。

### 14.5 Candidate Freeze

Agent Turn 结束不等于候选冻结。Freeze 流程：

1. 停止该 Candidate 新输入并确认无活动工具/终端。
2. 记录 candidate HEAD、Agent 自有提交列表和工作区状态。
3. 新建临时 index，以 candidate HEAD 为起点加入所有非 ignored 最终内容，得到 `CandidateFinalTree`；不改变候选真实 index。
4. 用 `git commit-tree CandidateFinalTree -p candidateHEAD` 创建 `CandidateResultCommit` 并保存私有 result ref。
5. 生成 `CandidateDelta = diff(BaselineTree, CandidateFinalTree)`，包含 binary/full-index、rename、mode、symlink。
6. 保存 changed-path manifest、测试结果、回复 Artifact、提交图和生成版本。
7. 验证 ResultCommit tree、Patch 应用结果与 CandidateFinalTree 一致。
8. 标记 Frozen；之后 Reviewer 读取 ResultCommit/Artifact，不读取可变工作目录。

如果候选重写历史导致 BaselineCommit 不再是祖先，“保留候选提交”不可用，但完整 Candidate Delta 仍可用。

### 14.6 采纳预览

任何采纳先重新捕获 `TargetCurrentTree`，比较 Checkpoint 后目标漂移：

```text
AdoptionPreview
├── mode availability + reason
├── baseline / candidate / target identities
├── candidate changed paths and diff
├── target drift paths
├── overlapping paths
├── three-way merge result or conflicts
├── commits to transplant
├── expected filesystem operations
├── index preservation statement
└── resources cleaned after success
```

预览带目标 fingerprint 和过期时间；确认时 fingerprint 不一致必须重新预览。

### 14.7 应用完整变更

目标是把 Candidate Delta 叠加到当前目标，而非重放 Baseline 内容：

1. 用 `BaselineTree`（base）、`TargetCurrentTree`（ours）、`CandidateFinalTree`（theirs）做三方 tree merge。
2. 有冲突时不写目标目录，生成 Conflict Artifact 并进入用户处理。
3. 无冲突得到 `MergedTree`，再计算 `TargetCurrentTree -> MergedTree` 的实际写入集。
4. 创建 Adoption Operation Journal，备份受影响的现有文件、symlink 和 mode。
5. 二次校验目标 fingerprint；逐路径写入/删除并记录完成位点。
6. **不修改真实 index**：用户已有 staged snapshot 保持不变；候选新变化表现为 unstaged/untracked。若平台或路径状态使这一保证无法成立，写入前停止。
7. 重新构建目标 final tree 验证等于 MergedTree。
8. 写 `adoption.completed`，然后清理候选资源。

应用中崩溃时，启动恢复根据 Operation Journal 选择继续验证或从备份恢复；不得猜测成功。

### 14.8 保留候选提交

该模式只在以下条件全部满足时开放：

- Candidate 提交图是 BaselineCommit 之后的可识别线性/可排序提交；
- 目标真实 index 和 worktree处于 Git 可安全 cherry-pick 的状态；
- 目标 branch/ref 与预览一致；
- 用户确认将保留的提交列表；
- 隐藏 BaselineCommit 不在移植列表。

Hub 按顺序 cherry-pick Checkpoint 之后的候选有效提交，必要时把 CandidateResultCommit 作为最后结果提交。发生冲突立即停止，保留 Git 原生冲突状态并跳转主界面；不自动 stash、reset 或丢弃用户内容。若 Checkpoint 本身是 dirty，通常此模式不可用，UI 推荐“应用完整变更”。

### 14.9 非 Git 目录

**Checkpoint**：生成目录 manifest（相对路径、类型、mode、size、hash、symlink target），默认排除缓存、构建产物和用户配置的规则，排除清单可见。

**物化**：macOS 优先 APFS clonefile/CoW，Windows 尝试受支持的块克隆，失败后普通复制；复制前估算空间。symlink 默认复制链接本身，不跟随目录外目标。

**Freeze**：对 Baseline manifest 与 Candidate manifest 做新增/修改/删除/mode/symlink Delta，文本生成 Patch，binary 保存 blob 引用。

**采纳**：不提供“保留候选分支”，只提供“应用完整变更/暂不采纳”。目标目录相对 Baseline 有漂移时，文本尝试三方合并预览；binary 或类型冲突必须人工决定。写入同样使用 Operation Journal 和备份。

### 14.10 无工作目录

纯聊天候选不创建文件空间。每个 Candidate 使用独立原生 Session，结果冻结为消息 Artifact；采纳只表示用户选择/生成综合方案，不产生文件写回。

### 14.11 清理

清理顺序：确认所有结果 Artifact 已持久化 → 停止 Hub 自有 Instance → 移除 worktree → 删除候选临时 branch/result ref → 删除 Checkpoint ref（无其他候选引用时）→ 清理临时目录 → 写 cleanup.completed。

只清理 ownership marker 与数据库均证明属于本 Hub 的路径/ref。失败进入 Recovery Center；不递归删除未解析变量、软链接目标或 repo root。

## 15. Quota Center

### 15.1 QuotaProvider

```rust
#[async_trait]
trait QuotaProvider: Send + Sync {
    fn provider_id(&self) -> ProviderId;
    fn surfaces(&self) -> Vec<QuotaScope>;
    async fn collect(&self, ctx: QuotaContext) -> Result<Vec<QuotaSnapshot>>;
}
```

Collector 不获得 Agent 凭据；只调用已登录 Agent 暴露的官方 RPC/SDK/结构化状态，或消费正常会话已经产生的状态字段。`collect()` 禁止发送模型 Prompt。

### 15.2 Snapshot

```text
provider / installation / account_label / plan_name
billing_surface / quota_scope
window_kind / model_scope
used_ratio / remaining_ratio / remaining_units / unit
resets_at
source / confidence
raw_provider_value
parser_version
collected_at / stale_after
```

`UsageSnapshot` 与 `QuotaSnapshot` 是不同类型。Turn token usage、上下文长度或费用估算不能填入订阅剩余额度。

### 15.3 来源与可信度

来源优先级：官方 API/SDK → App Server/RPC → 官方 CLI/状态行结构化字段 → 本地结构化状态 → 本地估算 → unavailable。

可信度：`ProviderReported / Derived / Estimated / Unknown`。UI 不把 Estimated 与 ProviderReported 排成同一个精确排行榜；供应商只给百分比时不换算成虚假请求条数。

### 15.4 调度与过期

- 应用启动后延迟收集，避免同时拉起所有 Agent。
- 活动 Agent 在正常 Turn 状态事件到达时可被动更新。
- 后台主动刷新最短间隔由 Provider 声明，默认 5 分钟。
- 失败采用退避，不影响会话；保留上次值并标记“上次已知”。
- 进入 `stale_after` 后卡片变灰，不再显示为当前余额。
- Comparison 创建页读取现有快照，不为展示而触发 Prompt。

### 15.5 首批 Collector

- Codex：App Server account/rate-limit 能力；方法与字段按安装版本 schema 适配。
- Claude Code：消费官方 statusLine 正常响应附带的 rate-limit 字段；首次响应前显示 Unknown。
- Qoder：只有 SDK 能复用现有登录且 Hub 不接触凭据时启用 Credits collector。
- 其他 Provider 无可靠接口时显示 Unknown + 官方额度页面入口，不抓取网页 Cookie。

## 16. 外部 Agent 会话观察

- 默认关闭，且不是主会话 Driver 的替代。
- Hook manifest 描述支持版本、事件字段、配置文件路径、安装 Patch、恢复 Patch 和 secret 字段。
- 开启前展示配置 diff，创建带 hash 的备份；只在原文件仍匹配预期时自动卸载，否则进入人工合并。
- Hook 事件进入独立 `ExternalObservedRun`，默认只读，只显示状态/项目/Agent/时间；不可从 Hub向外部 Session发送消息、审批或取消。
- 事件通过本机命名管道/Unix socket 或受控 append 文件进入；端点只允许当前用户访问，payload 有版本、大小上限和随机 installation token。
- 首个对象最终在 M1.5 根据 M0 稳定性选择；Claude Code + Codex 可以并行验证，但不强行双发。

## 17. 桌面 UI 设计

### 17.1 前端架构

- React + TypeScript + Vite，主窗口与独立审查窗口共享入口。
- Rust Protocol 生成 TypeScript DTO；所有 Tauri invoke 封装在 `coreClient`，组件不直接写字符串命令名。
- TanStack Query 管理分页查询和 Snapshot；有序 Event Reducer 更新活动 Task；Zustand 只保存选中项、面板宽度、草稿等非权威 UI 状态。
- 时间线采用虚拟列表；大日志、Diff、Artifact 按需分段加载。
- 设计系统使用 CSS variables + 可访问性 primitive；主题支持系统明暗模式，不把业务状态编码成颜色 alone。
- Markdown 禁止原始 HTML，链接和本地路径经安全路由打开；代码高亮放 Worker，避免阻塞流式 UI。

### 17.2 信息架构

```text
AppShell
├── Titlebar / GlobalCommandBar
├── ProjectTaskSidebar
├── MainContent
│   ├── TaskTimeline
│   ├── ComparisonWorkspace
│   ├── ReviewWorkspace
│   └── RecoveryCenter
├── AgentInspector
│   ├── CurrentAgent
│   ├── CapabilityMatrix
│   ├── QuotaCards
│   └── HandoffPreview
└── Composer
    ├── ModeSelector
    ├── Agent/Model/Mode
    ├── Context/Workspace indicator
    └── Send/Cancel
```

全局页面：Projects、Agents、Quota Center、Settings、Diagnostics/Recovery。首版不出现 API Key 或 Hub 登录页面。

### 17.3 Project/Task Sidebar

- Project 行显示 Git branch、dirty、活动 Run 数和待审批标记。
- Task 显示当前 Agent、最近状态、未读数；归档与删除分离。
- Project 移除只断开引用；删除磁盘目录不是可用操作。
- 拖拽排序只更新 Hub Projection，不修改 Agent 原生项目列表。

### 17.4 时间线

- Message 支持增量文本，但完成前保持 stable item ID，避免每 chunk 新节点。
- Tool/Terminal/Diff/Plan 使用独立卡片，默认折叠冗长输出。
- Pending Approval/Question 固定在当前 Turn 与 Composer 上方，解决后保留结果卡片。
- 每个 item 可查看 provenance：Agent、模型、Driver/Agent版本、Native Session/Turn、Workspace、时间和 Artifact。
- VendorExtension 使用“未完全识别的 Agent 事件”卡片，显示脱敏摘要、Driver版本与反馈入口。
- 错误卡片区分用户可修复、Agent/版本问题、Hub bug；提供复制 correlation ID 和导出诊断。

### 17.5 Composer

模式：普通发送、切换 Agent、委派、评审、并行比较。切换时先打开 Handoff Preview；比较时进入 Candidate 配置页，不直接发送。

发送前校验：Agent Ready、所需 capability、Workspace Lease、上下文包大小、候选数与 Quota 快照。警告不应伪装成阻断；真正不满足不变量时按钮禁用并给出修复路径。

### 17.6 Handoff Preview

分栏显示：确定性字段、将展开的 ContextPointer、可选最近对话、可选语义摘要。用户编辑创建新 Handoff version；保留“Hub 构建原稿”和“最终发送版本”的 diff。

### 17.7 Review UI

项目 Diff 的目录范围与文件焦点分开保存：`directoryId` 为空代表所有目录，`rootId` 仅保留当前文件/Git 操作焦点。目录选择在比较方式之前；只有具体 Git 目录提供提交/分支选择，切换目录清除原 reference、父提交、文件和预览。单目录隐藏目录选择，非 Git 隐藏 Git 模式并显示会话变更；混合目录的暂存相关模式标明“仅 Git”。服务端按相同规则选择目录并生成一个冻结快照；列表和统计使用比较快照；项目审查只取其范围描述，让 Agent 重新读取相应 Git 对象或当前工作区，不复制 blob。目录清单独立保留，确保筛选后仍能切回其他目录。分离窗口传递目录范围，迟到的查询/历史响应不覆盖新范围。详见 [实现与验证](implementation/review-directory-scope-2026-09-26.md)。

- 顶部固定被评审 Artifact/Commit、Checkpoint、Review Profile、Reviewer。
- Findings 按 axis 分栏，不强行合成单分数。
- 每项显示严重度、证据和位置；可选择“发回作者”“忽略”“标记已处理”。
- 作者修复后生成新 Review target version；旧报告保留并显示 superseded，而非覆盖。
- UI 同时显示 Agent Write Policy 与 Workspace Isolation，不用单一“只读”徽章掩盖差异。

### 17.8 Comparison UI

阶段化页面：配置 → Checkpoint/空间准备 → 并行运行 → 候选冻结 → 比较/互评 → 采纳确认。

候选卡固定展示：Agent/模型/模式、状态、耗时、Quota scope、回复、Diff、测试、评审和 frozen hash。支持分栏、单候选聚焦、同步滚动（文档）和统一 changed-files 导航（代码）。

采纳确认必须显示 AdoptionPreview 的全部核心字段；确认按钮绑定 preview fingerprint，目标变化后自动失效。未采纳候选清理前，UI 明示将保留哪些 Artifact、删除哪些临时资源。

### 17.9 Agent 与额度页

Agent 卡片：Installation 路径、版本、配置状态、默认通道、能力矩阵、兼容状态、官方安装/登录入口。无 Key 输入。

Quota 卡按 Provider 原生窗口分组，显示 scope、来源、可信度、采集时间和 reset；Unknown 是一等状态。趋势图只连接同一 Provider/Account/Scope/Window 的样本。

### 17.10 可访问性与快捷键

- 所有审批/候选状态可由文字和图标识别；键盘可完成发送、取消、审批选项、切换面板。
- 流式更新使用非打断式 aria-live；Token chunk 不逐字朗读，完成句段后合并通知。
- 支持 reduced motion、高对比度、字体缩放和 Windows DPI。
- 快捷键经命令注册表统一定义，避免窗口间冲突。

### 17.11 项目运行面板（已实现）

- `ProjectRunPanel.tsx` 提供运行页、分析 Agent 选择、按目录分组的运行项及多标签终端输出；`ProjectRunItem.tsx` 提供重命名、固定、删除和覆盖确认。
- 桌面 `project_runs.rs` 有界扫描 README、构建配置、包管理器/锁文件线索及脚本，将当前平台和 shell 一并交给 Core 的 `project_commands.rs`。Core 使用独立分析 Session 生成包含名称、命令、相对工作目录和来源文件的 JSON。
- 分析与执行分离；解析时校验来源、工作目录及单行命令，结果按项目/平台保存为本地 JSON。重新分析保留固定项，版本校验避免旧列表覆盖新编辑或执行过期配置。
- 点击运行项通过 `terminal.rs` 的 PTY 执行已保存命令，显示输出，支持终端输入、停止和结束后再次运行。进程由桌面终端宿主管理，不等同于后台 Core 持有的 Agent 任务生命周期。
- 调试功能执行项目已有调试命令；当前没有通用断点调试器、命令正文编辑器或专门的预览地址识别功能。已有测试源码覆盖分析响应、扫描/解析、保存/刷新及界面交互；本次仅核对源码和用户截图，未重跑测试或真实模型。

## 18. 悬浮岛移除与共享状态保留

2026-10-16：按用户要求移除悬浮岛独立窗口、主窗口启用按钮、启动恢复、专属 IPC/权限、样式、预览和验收代码。不再保留悬浮岛 ViewModel 或原生窗口适配。

`CompanionProjection` 是共享任务状态投影，仍供桌面会话列表的审查状态标记使用，并保留给手机 App 及未来设备复用。Core 的 `CommandEnvelope`、`CoreCommand`、`PendingApproval` 以及后台服务的 `bootstrap`、`sync_events`、`execute`、`pending_approvals` 等通信能力保留。审批的版本校验、幂等和 Driver 回传继续由 Core 负责。

本次只移除窗口专属实现，不新增手机网络传输或配对服务；现有后台服务通信仍按其原有本地边界工作。详见[移除记录](implementation/island-removal-2026-10-16.md)。

## 19. 安全与隐私

### 19.1 威胁边界

- Agent 与项目内容均视为不可信输入，可能包含恶意 Markdown、ANSI、路径、超大帧或提示注入。
- Hub 不防止用户授权 Agent 后 Agent 对系统造成的影响；该边界属于 Agent 权限/沙箱。
- Hub 必须保护自身数据库、配置修改、UI 渲染和 owned workspace 清理不被输入诱导越界。

### 19.2 控制措施

- Tauri capability 最小化：WebView 无任意文件系统/进程权限，只能调用列入 allowlist 的命令。
- CSP 禁止远程脚本和任意 inline script；远程图片默认不加载，需用户确认。
- Markdown 禁止 HTML；ANSI 解析白名单；外部链接显示域名后用系统浏览器打开。
- Artifact 通过受控自定义协议/命令读取，校验 Artifact ID，不接受 UI 传任意绝对路径。
- 所有子进程使用结构化 argv、显式 cwd 和环境变量 allowlist；默认继承用户正常 Agent 所需环境，但日志不输出完整 env。
- 帧、行、输出、Artifact、候选数和并发均设上限；zip/压缩内容防解压炸弹。
- 配置/Hook 修改有预览、备份、hash 和撤销；文件漂移后不强写。
- 自动更新与安装包必须签名；release 构建生成 SBOM 和第三方许可证清单。
- 未来 Device Gateway 默认不暴露完整 Core/UI API；每台硬件独立配对、独立 scope、可撤销，设备命令带幂等键与 expected Task version。
- Wi-Fi 链路使用加密连接和配对后的设备身份；USB 物理连接仍需用户在桌面端确认首次配对，不能把“插线”自动等同于全部授权。

### 19.3 脱敏

Redactor 依次执行：已知敏感字段名 → Provider 特定规则 → Authorization/URL userinfo → 常见 Key 模式 → 高熵候选。脱敏保留类型和尾部少量辨识符但不可复用。Raw Frame、诊断导出和崩溃上下文必须经过 Redactor；Canonical 用户消息不擅自改写，但导出前给用户预览。

### 19.4 本地数据说明

dogcodes 不上传 Canonical History、代码、Artifact 或额度。项目文件仍由 Agent 在用户选择的工作目录执行；Hub 本地保存统一时间线和协作所需证据。首版不做应用层加密，依赖 OS 账户权限和磁盘加密；未来若增加加密，必须覆盖 DB、FTS、CAS 和日志整体，不能只加密 SQLite。未来硬件配对密钥属于 dogcodes 自有设备身份，不是第三方 Agent 凭据，应保存在 macOS Keychain/Windows Credential Manager，而不是普通 SQLite 字段。

## 20. 故障恢复与一致性

连接丢失不作为永久占用：服务启动恢复匹配旧 task/turn/run 后，以 `turn.connection_lost` 关闭本地轮次、失效旧审批并释放占用；Companion 显示“连接中断”，不伪造 completed。用户可直接发送“继续”，由替代会话携带已有历史和检查当前结果的提示；取消人工核实外部结束的 UI 与接口。桌面 UI 退出时后台服务仍在运行，则直接恢复原服务的实时状态。详见 [执行状态一致性](implementation/execution-state-consistency.md)。


### 20.1 Recovery Center

启动扫描并展示：中断 Run、悬空 PendingInteraction、未完成 Adoption、遗留 owned worktree/ref、配置备份未确认、Artifact 临时文件、Projection 落后和 DB 检查异常。

每项提供 `检查 / 恢复 / 重试 / 保留 / 安全清理`，默认不自动执行破坏性操作。

### 20.2 事件与 Projection

- Event + Projection 在同一 SQLite 事务写入，正常情况下 offset 一致。
- Projection schema 变化或校验失败时，从某 sequence 重建到新临时表，校验后原子替换。
- UI 永远可以通过 sequence 重新同步，不依赖内存 broadcast 的可靠性。

### 20.3 Agent 断开

- EOF/stderr exit 先记录 instance.disconnected/exited，再关闭所有未完成 item。
- PendingInteraction 标为 unresolved/expired，禁止继续响应旧 native request。
- 原生支持 resume 时给“一键恢复”；不支持时创建新 Session并生成 Handoff。
- 自动重试不得重复用户写入 Turn；需要新的 ExecutionRun attempt，沿用 Turn 并明确 attempt number。

### 20.4 数据库异常

- 启动执行轻量 quick_check；失败时进入只读恢复界面，不继续启动 Agent。
- 当前不提供数据库升级、损坏库浏览或自动修库；启动失败仅开放独立诊断导出，不静默创建空库覆盖原库。
- 提供导出可读取事件/Artifact 元数据的诊断工具，首版通过 GUI触发而非 CLI。

## 21. 性能与背压

### 21.1 队列优先级

| 等级 | 内容 | 策略 |
|---|---|---|
| P0 | Approval、Question、完成、失败、取消 | 不丢；队列满时反压 Driver reader |
| P1 | Tool lifecycle、Diff、Plan、状态 | 合并同 item 的进度但保留边界 |
| P2 | Text delta | 50ms/8KiB 合并 |
| P3 | Terminal chunk、诊断 | 内存限额后 spool 到 Artifact，UI 按范围读 |

单慢 UI subscriber 不阻塞 Store/Driver；其 channel 超限后断开并要求按 sequence 重连。

### 21.2 预算

- Event 到主 UI P95 < 500ms。
- Core 空闲 < 150MB，完整应用不含 Agent < 300MB。
- 10 万 Timeline item FTS P95 < 500ms。
- Bootstrap 不加载消息全文、Diff 或终端；目标 < 1 秒展示 shell，后台补充状态。

### 21.3 测量

关键跨度：command accepted、Agent spawn/handshake、first delta、approval wait、turn complete、event commit、UI applied、checkpoint、materialize、freeze、adoption。指标仅本地存储；用户主动导出后才离开设备。

## 22. 测试策略

### 22.1 单元与属性测试

- 聚合状态机合法/非法迁移。
- Event upcaster、序列缺口、idempotency、Projection rebuild。
- Capability 降级和未知扩展保留。
- Context Package 确定性：相同事件范围产生相同 hash。
- Quota 归一化不混淆 scope/window/usage。
- Redactor corpus 和误报边界。
- 当前 Run 执行期间修改 `preferred_executor` 不产生 interrupt，下一 Turn 才使用新 Agent。
- `CompanionProjection` 重建、revision 缺口恢复，以及敏感字段裁剪不变量。

### 22.2 Driver Contract Test Kit

统一测试：discover/version、handshake、create/resume、stream ordering、cancel、approval、question、diff、unknown event、invalid frame、oversize frame、crash/reconnect、shutdown。每个 Driver 使用脱敏 fixture 和 fake process；真实账号测试不进入公共 CI。

兼容矩阵记录：Agent版本、平台、通道、通过的 capability、fixture hash、测试日期、已知限制和支持状态。

### 22.3 Git 场景矩阵

至少覆盖：clean、仅 staged、仅 unstaged、staged+unstaged 同文件、untracked、ignored 选择、rename、delete、binary、symlink、executable bit、空仓库、detached HEAD、多个 worktree、路径含空格/Unicode/换行、目标漂移无冲突/有冲突、候选有提交/无提交/改写历史、dirty submodule 阻断、Freeze/Adoption 中途崩溃。

每个测试验证：真实 HEAD/index 不被 Checkpoint 改动；所有候选 BaselineTree 相同；Candidate Delta 不含原 dirty 基线的重复变化；隐藏提交不进入用户分支；清理不触碰非 owned 资源。

### 22.4 UI 测试

- Reducer 以录制事件流做 deterministic test。
- Component test 覆盖每类卡片、Unknown/Estimated/Stale、Capability 降级。
- E2E 使用 fake Driver：流式对话、审批、切换、断线、重载补事件、Comparison、Adoption Preview。
- E2E 验证不同客户端消费同一 `CompanionProjection` revision 后得到一致的核心状态。
- 共享审批测试覆盖一次性批准、拒绝、原生问答、多端竞争响应、过期请求与重复点击幂等。
- 视觉回归覆盖明暗、字体缩放和 Windows DPI。
- 可访问性测试覆盖键盘、aria、对比度和 reduced motion。

### 22.5 故障注入

在 spawn、Event commit、Artifact rename、worktree add、Freeze、Adoption 每个步骤注入退出/IO error，重启后验证 Recovery Center 给出唯一且安全的下一步。

## 23. 日志与诊断

- `tracing` 结构化日志，默认 info；每个 Command/Run/Driver 带 correlation ID。
- 日志分 Core、Driver、Workspace、Store、UI bridge、Platform；不记录完整 Prompt、环境变量或凭据。
- Diagnostics Bundle 默认只含版本、能力矩阵、错误、性能统计和脱敏日志；加入会话/Raw Frame/路径必须逐项勾选预览。
- 不接入默认远程 telemetry/crash SDK。将来如增加必须 opt-in，并与会话内容分离。

## 24. 构建、发布与配置

### 24.1 工具链

- 固定 Rust stable toolchain 与 `Cargo.lock`。
- pnpm 固定版本与 lockfile；根 `package.json` 使用 `private: true`，首版不发布 npm 包。
- npm 名称 `dogcodes` 仅保留；未来发布前重新检查占用。首版无 `dogcodes` binary。
- CI：fmt/clippy/test、TS lint/typecheck/test、protocol generation diff、license/SBOM、secret scan、平台构建。

### 24.2 Tauri 安全配置

主窗口和独立审查窗口使用不同 capability 配置；审查窗口的宿主命令校验所属 Task。更新器、shell/open、dialog 等插件按页面最小授权，不启用全局宽泛权限。

### 24.3 发布

- macOS：Developer ID 签名、公证 DMG；Homebrew cask 指向同一签名产物和 checksum。
- Windows：签名安装包；自动更新只接受签名 manifest/产物。
- App identifier 使用官网域名 `dog.codes` 对应的反向域名标识 `codes.dog.desktop`。软件尚未发布，品牌名称及相关标识直接更新，不保留旧名称兼容或数据迁移逻辑。
- Release 附 Apache-2.0、第三方许可证、兼容矩阵和数据边界说明。

## 25. 实施顺序

当前执行顺序只维护在 [当前工作清单](CURRENT_WORK.md)。下表用于区分已落地的设计范围与远期方案，不替代清单的验收状态。

| 设计范围 | 当前对应范围 |
| --- | --- |
| §4–7 / §10 会话、Core、事件、SQLite | 主链路、重命名和永久删除已实现；FTS5 与导入另列后续 |
| §9 Driver | 多家真实链路已有证据；Copilot 暂缓，Claude 最新审查记录受 Key 过期影响 |
| §11 历史与交接 | 自动交接、MCP 按需读取和逐事实交付已实现；后续增强的真实重验仍待补齐 |
| §12.2 审查型只读委派 | 独立 Session/Run、取消/重试与来源回流已实现；通用委派未排期 |
| §12.3 指定评审 | 当前三类入口、Agent 自主读取、结构化结果及当前代码修复复审闭环；旧分组/冻结检索/强制独立复核已移除 |
| §12.4–14 候选比较/采纳 | 远期规划；不能用已完成的一键审查代替，也不自动纳入本期 |
| §15 额度 | 现有多家显示已实现；账号验收与后续提醒/趋势分开 |
| §17.11 项目运行 | 自动分析启动/调试/构建等命令及一键终端运行已实现，补录 D10 |
| §18 悬浮岛移除 | 独立窗口及专属实现已移除，共享状态与通信保留 |
| §20 恢复 | 当前范围已实现；高级文件系统/adoption 恢复及 worktree 清理退出本期 |
| §24 发布 | 下一阶段准备 macOS 测试包与发布基础；发布版尚未完成 |

以下 25.1–25.3 为原阶段拆分，保留设计演进用途，**不是当前待办**。其中旧 Copilot 排期、交接预览、未完成审查等表述不覆盖最新产品决策。

### 25.1 历史规划：M0 验证仓库

先建立最终 monorepo 骨架，但 Agent probe 放 `experiments/`，不让实验协议污染 Core：

1. Protocol/Event 最小类型与 TS 生成。
2. SQLite Event append/Projection/replay spike。
3. Codex stdio App Server handshake + stream + approval probe。
4. Cursor ACP stable v1 probe。
5. Copilot external runtime probe；验证禁用 bundled CLI。
6. Git dirty BaselineTree/worktree/freeze/adoption spike。
7. Tauri Channel 顺序/重载补偿 spike。
8. macOS top overlay 和 Windows DPI 最小验证。

每个 probe 输出兼容记录、fixture 和 go/no-go，不直接成为 Product Driver。

### 25.2 历史规划：M1 纵向切片

建议按可演示的垂直切片交付：

1. 注册 Project → 创建 Task → fake Driver流式 Timeline → `CompanionProjection` → 重启恢复；同时验证 `preferred_executor`/active Run 与 Actor/幂等/版本语义。
2. Codex真实会话 → 审批/取消/Diff → 原生 Session resume。
3. Cursor ACP真实会话 → capability降级/未知事件。
4. Copilot真实会话或 M0批准的替代 SDK切片。
5. A→B→A Handoff/Reconciliation，含预览和确定性 hash。
6. 只读 Delegation 与 Review基础卡片。
7. 一个真实 QuotaProvider + Unknown/Stale UI。
8. Recovery Center 与诊断导出。

### 25.3 历史规划：M1.5–M3

- M1.5（历史规划）：悬浮岛已退出范围；外部 Hook 另行排期。共享 `CompanionProjection` 与审批状态机继续保留给桌面、手机 App 和未来设备。
- M2：完整 Checkpoint、文档 Comparison、Review闭环、Canonical检索。
- M2.5：代码/Debug候选、Freeze、两种实际采纳与清理。
- M3：写入 Delegation、Cross Review、第二批 Agent、社区进程型插件协议。
- H1（独立硬件实施阶段）：Companion Protocol、USB/Wi-Fi Device Gateway、配对与权限、语音采集/转写/确认发送、设备固件兼容测试。H1 不反向扩大 M1–M4 软件首版范围。

未来 Driver 插件采用独立进程 + 版本化 JSON-RPC/stdio，不采用 Rust动态库 ABI，以降低崩溃和编译器版本耦合。

## 26. 需求追踪矩阵

| 需求域 | 详细设计位置 | 首次交付 |
|---|---|---|
| FR-1 Agent集成/生命周期 | §8、§9 | M0/M1 |
| FR-2 权限、安全、登录 | §8.7–8.8、§19 | M1 |
| FR-3 项目/任务/Session | §5、§7、§10 | M1 |
| FR-3.10 项目运行分析与一键运行 | §17.11 | 已实现，D10 |
| FR-3 Checkpoint/Candidate Delta | §14 | M2/M2.5 |
| FR-4 统一对话/切换 | §6、§10、§11、§17 | M1 |
| FR-4 Canonical按需检索 | §5.4、§11.1 | M2 |
| FR-5 额度中心 | §15、§17.9 | M1起 |
| FR-6 悬浮岛 | §18 | 已移除，退出范围；共享审批保留 |
| Companion Hardware 架构基线 | §5.3.1、§6、§18、附录 A | M1 定义模型；H1 实现设备 |
| Delegation | §12.1–12.2 | M1只读/M3写 |
| Review | §12.3、§17.7 | M2 |
| Parallel Compare | §12.4、§14、§17.8 | M2/M2.5 |
| Cross Review | §12.5 | M3 |
| 性能/可靠性 | §20–§23 | 全阶段 |
| 发布 | §24 | M4 |

## 27. 已决事项、设计默认值与后续确认点

### 27.1 已决事项

| 事项 | 结论 |
|---|---|
| 首版 CLI | 不做用户 CLI；Core 服务和本地 RPC 作为桌面的执行基础实现 |
| npm发布 | 首版不发布；名称保留为 `dogcodes` |
| Hub应用层落盘加密 | 首版不做；本地存储、不上传、依赖OS权限/磁盘加密 |
| Agent认证/API Key | 完全由 Agent 自己配置，Hub无表单/凭据存储 |
| 权限/沙箱 | Agent 自管，Hub透传 |
| ACP版本 | release 默认 stable v1，draft v2仅实验 |
| Candidate Git隔离 | Git目录一律独立worktree，含文档任务 |
| 非Git写候选 | 自动复制独立目录 |
| 未采纳候选 | 结果持久化后自动清理目录、worktree、临时分支 |
| 硬件语音交互 | 单击开始；再次单击结束并转写；显示文字；第三次单击确认发送 |
| 硬件切换 Agent | 只修改当前 Task 的下一轮默认 Agent，不中断当前 Run |
| 第一代硬件连接 | USB 有线或 Wi-Fi；复用同一 Companion Protocol |

### 27.2 本设计采用的默认值

这些值不影响项目骨架，可在对应阶段评审时调整：

| 事项 | 默认值 | 最迟确认 |
|---|---|---|
| Review严重度 | Critical/High/Medium/Low/Note，UI可映射P0–P4 | M2 |
| Cross Review盲评 | 默认关闭，用户可开启 | M3 |
| dirty submodule | 首版阻止写入型并行，不做递归快照 | M2 |
| Raw Frame保留 | 7天或200MB | M1设置设计 |
| Terminal大日志 | 30天或1GB | M1设置设计 |
| Comparison并发 | 默认2，全局硬上限4 | M2 |
| 文档结构化Diff | 先并列+文本Diff；Markdown AST Diff是否内置后定 | M2 |
| 首个外部Hook | Claude Code/Codex均做M0 probe，按稳定性选首发 | M1.5 |
| macOS原生层 | Rust + objc2-app-kit优先，Swift helper仅在验证需要时引入 | M1.5 |

### 27.3 M0 事实门

以下不是产品偏好，必须用真实安装版本得出结论：

- Cursor ACP 是否满足 M1 的审批、取消、Session 恢复与 Diff 表达。
- Copilot Rust SDK关闭 bundled CLI 后，能否稳定连接用户已有CLI及其版本约束。
- Codex App Server支持版本、schema生成、rate-limit字段和审批新类型。
- Claude额度字段实际出现条件与scope。
- Git最小版本、Windows worktree/path行为和三方tree merge可用性。

## 28. 建议 ADR

项目搭建时创建以下 ADR，记录“为什么”而不是重复本文实现细节：

1. `0001-embed-core-in-tauri-with-transport-neutral-contract.md`
2. `0002-use-canonical-event-store-and-rebuildable-projections.md`
3. `0003-keep-agent-auth-and-permissions-external.md`
4. `0004-use-stable-acp-v1-and-preserve-vendor-extensions.md`
5. `0005-model-driver-and-quota-provider-separately.md`
6. `0006-materialize-dirty-git-baselines-with-temporary-index.md`
7. `0007-use-process-based-future-driver-plugins.md`
8. `0008-no-cli-or-daemon-in-first-release.md`
9. `0009-local-persistence-without-application-layer-encryption-v1.md`
10. `0010-share-versioned-companion-projection-across-ui-and-future-devices.md`

ADR 只在决策难逆转、存在真实替代方案且未来读者会疑惑时创建；普通库选择不必都写 ADR。

## 29. 官方依据与研究参考

**官方技术依据**

- [Tauri 2：Calling Rust from the Frontend](https://v2.tauri.app/develop/calling-rust/)：command、Channel与Event边界；有序高吞吐数据使用Channel。
- [ACP官方 Rust SDK](https://github.com/agentclientprotocol/rust-sdk)：stable v1 builder、draft v2 feature与测试工具。
- [Codex App Server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)：stdio JSONL、初始化、Thread/Turn/Item、审批、schema生成和rate-limit接口。
- [GitHub Copilot SDK Rust](https://github.com/github/copilot-sdk/blob/main/rust/README.md)：外部runtime、进程生命周期、PermissionHandler及默认bundled CLI行为。
- [Git Worktree](https://git-scm.com/docs/git-worktree)、[git commit-tree](https://git-scm.com/docs/git-commit-tree)、[git merge-tree](https://git-scm.com/docs/git-merge-tree)。
- [SQLite WAL](https://sqlite.org/wal.html)、[SQLite FTS5](https://sqlite.org/fts5.html)。
- [WebSocket RFC 6455](https://www.rfc-editor.org/info/rfc6455/)、[CBOR RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html)、[Opus RFC 6716](https://www.rfc-editor.org/rfc/rfc6716.html)：未来 Companion Hardware 的 Wi-Fi 消息、嵌入式编码与语音帧依据。
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp)：未来本地 `TranscriptionProvider` 候选，不作为软件首版依赖。

**工作流与产品参考**

- [mattpocock/skills](https://github.com/mattpocock/skills)：Context Pointer、Handoff去重复、双轴Review、垂直任务切片；不作为运行时依赖。
- Codexia、Berd、T3 Code、Zed External Agents、OpenCovibe、cdesktop、Verun：见需求文档参考清单。
- CodeIsland、open-vibe-island、AgentDesk、Atoll、agentbro：只研究顶部浮窗行为和平台适配；按各自许可证隔离代码复用。

## 30. 文档维护规则

- 需求边界改变先更新需求文档，再更新本设计和追踪矩阵。
- 供应商协议变化只更新兼容矩阵/Driver设计；除非改变Canonical语义，不修改Core公共协议。
- Wire schema 的破坏性变化必须提升 schema version；当前未发布阶段明确不提供旧数据库/事件迁移，未来发布版需另行制定兼容方案。
- Checkpoint/Adoption算法改变必须补充场景矩阵和故障注入测试。
- 文档中的路径、方法名和默认值在实现后与代码生成物做CI链接检查，避免设计漂移。

---

## 附录 A. Companion Hardware 详细边界

### A.1 目标与边界

未来配套硬件用于低注意力查看和控制：显示 Agent 运行状态与订阅额度、显示各 Project/Task 的默认及当前执行 Agent、选择设备关注的 Task、通过麦克风生成 Prompt，以及选择下一轮 Agent。

硬件不是 Agent Client，不直连 ACP、Codex App Server 或其他 Agent 协议；不读取 Agent 凭据、完整会话、代码、Diff 或终端。它只连接 dogcodes Device Gateway。业务不变量、Handoff、Session 映射和 Turn 创建仍由 Core 执行。

### A.2 拓扑

```text
Agent Drivers -> Rust Core -> CompanionProjection
                               ├── IslandViewModel
                               └── Device Gateway（H1）
                                    ├── USB Transport
                                    └── Wi-Fi WSS Transport
                                             │
                                     Companion Hardware

Hardware microphone -> Audio frames -> TranscriptionProvider（H1）
                                      -> PromptDraft
                                      -> user confirms on device
                                      -> existing Core SendTurn command
```

H1 的 Device Gateway 接入独立 Core 后台服务；桌面 UI 退出不影响硬件连接，服务不可用时硬件显示 Offline。设备本地不排队待发送 Prompt，重连后不得自动补发未确认或未送达的 Prompt。Core 服务设计与实施见 [独立 Core 后台服务](implementation/core-daemon.md)。

### A.3 领域对象

以下对象语义现在固定；`CompanionDevice` 与 `VoiceCapture` 的持久化实现到 H1 再加入：

```text
CompanionDevice
├── device_id / display_name
├── hardware_model / firmware_version
├── protocol_range
├── paired_at / last_seen_at
├── transport_capabilities[]
├── granted_scopes[]
├── status: pairing | online | offline | revoked | incompatible
└── focus: DeviceFocus

DeviceFocus
├── project_id
├── task_id
└── updated_at

VoiceCapture
├── capture_id / device_id / task_id
├── frozen_executor
├── codec / sample_rate / channels
├── status
├── transcript / confidence / language
├── transcription_provider
├── audio_retention
└── expires_at
```

Project 不能拥有“唯一当前 Agent”不变量，因为 Review、Delegation 与 Candidate 可能并行。设备显示 Task 的 `preferred_executor`、`active_primary_run.executor` 与 `secondary_agent_count`；长按只修改 `preferred_executor`。

### A.4 Companion Protocol Envelope

USB 与 Wi-Fi 复用同一逻辑消息：

```text
protocol_version
message_id
device_id
connection_session_id
sequence
ack_sequence
sent_at
message_type
payload
```

消息类型包括：`Hello/VersionNegotiation`、`PairRequest/PairConfirm`、`Snapshot/StateDelta/Ack`、`Command/CommandResult`、`AudioStart/AudioChunk/AudioEnd`、`TranscriptReady`、`Heartbeat`、`Error`。

控制消息建议 CBOR 编码；音频 chunk 使用独立 binary payload 并引用 `capture_id`。所有 decoder 必须限制 frame 大小、集合长度、递归深度、字符串长度和音频总量。协议 envelope 在 H1 单独 crate 实现；首版仅保证 Core DTO 可稳定映射。

### A.5 传输

**USB 有线**：第一代以 USB CDC-ACM/串口式双向通道作为默认技术假设，免局域网配置，适合开发与稳定桌面使用。协议自行提供 frame length、CRC/校验、sequence 和 ack；不能依赖“一次 serial read 等于一条 message”。如果后续“有线”选择 Ethernet，则直接复用 Wi-Fi 的 WSS 通道。

**Wi-Fi**：设备通过局域网发现桌面 Gateway 并建立 WSS 长连接；状态 Delta、Command 和 Opus 音频复用该连接或使用独立 audio stream。断线指数退避，重新连接后按 last acknowledged revision 补状态 Delta，无法连续补齐时下发完整 Snapshot。

BLE 不进入第一代主链路；以后可以用于配网/辅助发现，不为语音流维护第三套完整业务协议。

### A.6 配对、权限与撤销

首次 USB 连接或 Wi-Fi 发现都必须在桌面端显示设备型号与短码，由用户确认。配对后每台设备获得独立身份密钥和 scope：

```text
status.read
quota.read
focus.write
agent.select
interaction.read
interaction.respond
audio.upload
prompt.compose
prompt.send
run.cancel        # 默认不授予
```

密钥放 OS Keychain/Credential Manager，DB 只保存不可逆标识、授权 scope 和设备元数据。Wi-Fi 使用加密连接并固定配对身份；设备可以在 Settings 中单独撤销。`interaction.respond` 只允许响应 Snapshot 中暴露且仍有效的 option，不隐含 `run.cancel`、配置写入或永久授权。Device Gateway 不暴露 Tauri UI 命令全集或 Event Store 查询接口。

### A.7 状态同步

硬件接收的是从 Core `CompanionProjection` 裁剪出的 `CompanionSnapshot`：

```text
revision / generated_at / stale
agents[] {id, name, state, quota_summaries[]}
workspaces[] {
  project_id, display_name, branch?,
  task_id, task_display_name,
  preferred_executor,
  active_run {executor, state, started_at}?,
  secondary_agent_count, pending_count,
  pending_approval? {
    interaction_id, interaction_version, agent,
    kind, redacted_summary, quick_approval_eligible,
    positive_option_id?, negative_option_id?, expires_at?
  }
}
device_focus
```

默认不传绝对工作目录、账号邮箱、完整 Prompt、代码、Diff、终端或原始 Provider 事件。审批只下发经过 Redactor、长度限制与控制字符清理的命令/工具摘要；若裁剪后不足以判断，则 `quick_approval_eligible=false`。用户可选择是否显示 branch 和 Quota 计划名称。设备按 revision ACK；发现 Delta 缺口请求 Snapshot。离线缓存必须显示 stale 与最后更新时间，离线状态绝不允许审批。

### A.8 Device Command

```rust
struct DeviceCommand<T> {
    protocol_version: u16,
    message_id: String,
    device_id: DeviceId,
    idempotency_key: String,
    expected_projection_revision: u64,
    expected_task_version: Option<u64>,
    expected_interaction_version: Option<u64>,
    payload: T,
}
```

Gateway 验证配对和 scope 后，将命令转换成现有 `CommandEnvelope`，设置 `actor = ActorRef::PairedDevice(device_id)`。相同 message/idempotency key 返回原 `CommandAck/CommandResult`。Task 或 Interaction 版本过期时拒绝并附最新安全摘要，禁止把设备旧选择或旧审批应用到已经变化的 Task/请求。

### A.8.1 硬件快捷审批与按键优先级

每个工作目录显示区独立呈现 `waiting_for_approval`。聚焦该区域后，如果 `quick_approval_eligible=true`：

```text
PendingApproval
  --单击--> Responding(positive_option_id) -> Resolved | Stale
  --长按--> Responding(negative_option_id) -> Resolved | Stale
```

- 待审批状态优先于普通运行、语音录入和 Agent 选择：此时单击不开始录音，长按不打开 Agent 列表。
- 单击只提交 Driver 已验证的 `approve_once`；长按只提交 Driver 已验证的 `reject|cancel`，两者均只作用于该 `interaction_id`，不取消整个 Run。
- 多个目录或多个请求待审批时，用户必须先聚焦具体目录/请求；不提供“一键全部批准”。
- 设备显示 Agent、目录短名、审批类型、脱敏摘要和明确操作提示；文本支持滚动/分页。
- `quick_approval_eligible=false` 时显示“需桌面端审批”，单击/长按均不产生响应命令。
- 请求被主界面、手机 App 或 Agent 先行解决后，revision 更新立即退出审批模式；设备迟到输入按旧 interaction version 返回 `Stale`，绝不映射到下一请求。

### A.9 语音交互状态机

已确认交互：

```text
Idle
  --单击--> Recording
Recording
  --再次单击--> Uploading/Transcribing
Transcribing
  --成功--> AwaitingConfirmation（硬件显示转写文字）
AwaitingConfirmation
  --第三次单击--> Sending -> Sent
  --取消/超时--> Cancelled
任意错误 -> Error -> Idle/Retry
```

- 只有当前聚焦区域没有有效待审批快捷动作时，单击才进入 `Recording`；按键解释优先级以 §A.8.1 为准。
- 录音开始时冻结 `DeviceFocus`、Task version 和当时的 `preferred_executor`；确认发送前重新校验 Task version。
- 转写期间若 Agent 选择变化，Prompt 草稿必须显示新目标并再次确认；不能静默改投其他 Agent。
- 默认由桌面端 `TranscriptionProvider` 执行识别；第一实现优先本地模型/系统能力，不要求第三方 API Key。高性能硬件未来可直接发送 Transcript，仍走同一 `PromptDraft`/确认流程。
- 建议音频为 16kHz 单声道 Opus；USB 调试可支持 PCM。`AudioStart` 声明 codec，双方不能猜测。
- 原始音频默认在转写完成或失败后删除，不进入 Canonical History；Transcript 在用户第三次单击确认后作为普通 User Prompt 保存。用户取消的 Transcript 只保留最小诊断元数据。
- 转写结果显示不下时支持滚动/分页，不能只显示前几个字后让用户盲发。

### A.10 Agent 选择语义

长按打开已安装且对当前 Task 可用的 Agent 列表；不可用、未配置或缺关键 Capability 的 Agent 禁用并显示原因。选择后只更新：

```text
Task.preferred_executor = selected_agent
```

当前 `ExecutionRun` 继续，不发送 interrupt。硬件显示“下一轮：Agent B”。下一次确认 Prompt 时：

- 若 B 与当前可恢复 Session Agent 相同，正常 resume；
- 若不同，Core 生成 Handoff 并创建/恢复 B Session；
- 硬件不展示完整 Handoff 编辑器，使用该 Task 已在桌面端确认的默认 Handoff 强度；首次硬件跨 Agent 发送或配置变化后，可以要求桌面端一次性授权“允许设备使用默认 Handoff”。

### A.11 额度

硬件只消费已有 `QuotaSnapshot` 的低带宽映射：计划、scope、窗口、remaining、reset、confidence、collected_at/stale。设备不得通过频繁刷新触发模型调用；`quota.read` 不包含账户 Token，账号标签和邮箱默认不下发。

### A.12 固件与协议兼容

- `Hello` 交换硬件型号、firmware、最小/最大 Companion Protocol 版本和 transport/audio capabilities。
- 没有共同版本时进入 Incompatible，只显示升级提示，不尝试宽松解析。
- 未知可选字段忽略；未知必需 feature 拒绝对应 Command，不断开整个只读状态连接。
- 固件升级包必须验证签名；Gateway 不允许设备上传可执行代码到桌面。

### A.13 测试门槛

- 相同测试向量在 USB 与 Wi-Fi 传输后产生一致的 DeviceCommand/Core Command/Projection 结果。
- 断线、重复 frame、乱序、丢包、过期 revision 和设备重启不会重复发送 Prompt。
- 单击开始、再次单击结束、第三次确认与长按选择做硬件输入去抖和状态机测试。
- 待审批状态下单击只发送一次 `approve_once`，长按只发送一次 `reject|cancel`；不触发录音、Agent 选择或 `run.cancel`。
- 多请求、已过期、其他端抢先响应、复杂 form、未知 option role 和截断摘要均不会误批或串批下一请求。
- 当前 Run 执行时选择新 Agent 不产生 interrupt 事件。
- 音频超限、转写失败、Prompt 确认超时均不产生 Agent Turn。
- 被撤销设备不能读取撤销后的新状态或发送命令。
- `CompanionSnapshot` 不得包含绝对路径、Prompt、代码、Diff、账号标识、原始额度凭据或 Agent 认证信息。

### A.14 首版必须实现与硬件阶段再实现

**软件首版必须实现**：Core 不依赖 Tauri 窗口；`TaskExecutionState` 区分 `preferred_executor` 与 active Run；`CommandEnvelope` 有封闭的 Actor、幂等键和 expected version；`PendingInteraction` 有原生 option ID、经 Driver 验证的 semantic role、interaction version 与 compare-and-set 响应；从 Canonical Event 维护版本化 `CompanionProjection`；桌面和手机 App 可复用该 Projection；Quota Snapshot 可裁剪；状态同步可按 revision 恢复；相应不变量进入 M1 测试。

**硬件阶段再实现**：LAN 监听、USB 驱动、设备配对、Keychain 设备密钥、Companion wire codec、CBOR/Opus、语音模型、固件升级、硬件 Settings UI，以及 `CompanionDevice`/`VoiceCapture` 持久化。它们不得要求重写上述 Core 语义。

---

**下一步**：历史增强验证收口后，补齐 Copilot SDK / Hub 只读 Delegation 和 M1 剩余验收。高级文件系统恢复、adoption 恢复和 worktree 清理已退出当前交付；本文后续阶段架构描述不自动恢复这些待办。当前工作入口以 [实施状态](IMPLEMENTATION_STATUS.md) 为准。
