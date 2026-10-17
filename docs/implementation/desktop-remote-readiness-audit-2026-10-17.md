# DT-08/09/10 面向手机远控的代码核对

日期：2026-10-17。对应 DT-08/09/10。目的：以后续“手机 App 通过共享服务完成日常 PC 端操作”为目标，核对当前代码缺口是否已在文档记载、有无遗漏或描述不准确。本次只读源码与统计注册表，未修改产品代码、未运行产品测试或打包。状态仍以 [当前工作清单](../CURRENT_WORK.md) 为准。

## 1. 判定口径

手机端将以独立客户端（Flutter/Dart）接入同一后台服务。因此 DT-08/09/10 的“完成”应满足：

- **业务执行与事实在后台**：手机不需要复刻桌面 TypeScript 中的业务状态机，也不依赖 Tauri 进程内的逻辑。
- **跨客户端，而非跨窗口**：一个客户端发起的操作，另一个客户端可以发现、核实并在允许时接续。同一 WebView 的 localStorage 只能覆盖同机桌面窗口，不能覆盖手机。
- **入口可被远程网关白名单化**：后台方法清单准确，无已退役但仍可调用的入口。

## 2. 与文档一致的部分（已核实）

| 范围 | 代码事实 |
| --- | --- |
| 终端 / 项目进程 | daemon `terminal_host::TerminalHost` 持有 PTY；资源有 id/epoch；`read(id, after)` 为带 offset 的长轮询（15 秒），1 MiB 输出缓存并返回 `gap`；窗口标签只用于桌面入口授权，不再是资源 owner |
| Diff / Git / 文件 / 撤销 | `review::service` 在 daemon 内运行；Git 组合工作流（`git_workflow`）的意图与接续在后台 |
| 文件查询 / 监听 | `workspace_service` 与 `workspace_watch` 在 daemon；窗口仅保存可取消的转发任务 |
| 项目运行配置 / 分析 | `project_runs` 在 daemon，带回执与版本条件 |
| 附件 | `attachment_service` 在 daemon，固定上传身份与摘要 |
| 共享偏好 | Agent 启用/排序、历史置顶/归档/排序由 Core System 流持有 |
| 普通发送 | 后台 `submission_intents` 支持分页发现，其他客户端可把后台记录缓存到本机后接续 |
| 身份 | daemon 与 Tauri 都强制 `ActorRef::LocalUser`，Core 多处要求 LocalUser；`ActorRef::PairedDevice` 已定义但未贯通（RA-02 已记载） |
| 订阅 | 尚无统一订阅（已记载为剩余项） |

## 3. 文档未记载的缺口

### G1 多步业务流程由前端编排，恢复记录只在本机 localStorage（影响最大）

下列流程的阶段推进、待核实记录及恢复发现都在桌面 TypeScript 中，记录存于 localStorage：

| 文件 | 内容 | 后台发现 |
| --- | --- | --- |
| `submissionOperations.ts`（426 行） | 新建任务 → 切换 Agent 时构建上下文 → 确认上下文包 → 提交 | 有（`submission_intents`），阶段推进仍在前端 |
| `reviewRepairOperations.ts`（163 行） | 开始修复：上下文确认 → 提交到主 Agent | 无 |
| `reviewStartOperations.ts` | 审查启动 | 无 |
| `forkOperations.ts` | Fork | 无（后台只有按已知 ID 查询的 `fork_receipt`） |
| `approvalOperations.ts`、`cancelOperations.ts` | 审批回复、停止 | 无（只能按已知身份查询回执） |
| `gitOperations.ts`、`reviewMutations.ts` | Git、文件修改 | 后台已有分页发现 `core_review_operations`，但移除“操作记录”界面后**前端已无调用方** |
| `projectRunOperations.ts`、`attachmentUploads.ts` | 项目运行操作、附件上传 | 无 |

合计约 1.7k 行状态机与校验。影响：

- 手机端需要用 Dart 重写这些状态机，两端容易出现语义分叉。
- 在手机上发起、断线的修复/Fork/审查启动，PC 无法发现或接续，反之亦然。
- 既有文档中的“持久化”“跨窗口恢复”实际指同一 WebView 的本机存储，不等于跨客户端恢复。

建议：参照 `git_workflow` 和 `submission_intents`，把多步流程改为后台持有意图与阶段推进的工作流，客户端只提交意图、查询/订阅状态，并显式核实或放弃。先做开始修复和带 Agent 切换的发送，再做 Fork/审查启动；审批与停止至少补后台的待核实发现。

### G2 项目注册/编辑的校验、去重和互斥在 Tauri 进程

`apps/desktop/src-tauri/src/lib.rs` 的 `validate_project_directories`、`register_local_multi_project`、`DesktopState.project_gate`：

- 路径 canonicalize、可读性检查、32 目录上限、按名称+路径去重、生成 `ProjectId` 都在桌面进程完成。
- `project_gate` 是进程内互斥，两个客户端之间不生效；注册没有客户端操作 ID，并发或重试可能产生重复项目。
- Core 对 `RegisterProject` 只检查非空，附加目录只检查绝对路径和存在，不规范化主路径。

手机“选择 PC 已有目录添加项目”是已确认的首版功能，绕过桌面进程就会失去这些校验。建议把校验、去重和操作身份移到 daemon/Core，Tauri 只保留系统目录选择框；手机端还需要宿主目录浏览接口（RA 范围）。

### G3 Agent 升级在 Tauri 进程执行

`agent_updates::core_upgrade_agent` 用 `tauri::async_runtime::spawn` 执行安装命令，状态存于 Tauri 托管的 `AgentUpdates`，进度通过 `app.emit` 发给窗口。退出桌面会中断正在进行的安装，另一个客户端也看不到进度。按已确认范围，手机只显示安装状态和额度，升级通过远控在 PC 界面完成，所以这不是手机 API 的前置项；但它与 DT-08“执行脱离窗口”的表述不一致，至少应在 DT-08 中明确为例外或迁入后台。

### G4 后台服务的可达性与保活

- daemon 只由桌面按需启动，没有登录自启；电脑重启后在用户打开桌面前，手机无法连接。
- 代码中没有防自动睡眠（macOS 电源断言、Windows `SetThreadExecutionState`、Linux inhibit）。

二者都属于远控首版需求（RA-02G、P4），不是 DT-08 本机验收条件；在此登记，避免误以为“关闭窗口后台继续”已经覆盖远程可达。

### G5 终端多客户端语义

- `resize` 是后写入者生效，手机与 PC 同时附着同一终端时会互相改变尺寸。
- 只有 1 MiB 字节尾部，没有终端屏幕快照；对全屏 TUI（vim、Agent CLI 等），附着时重放尾部可能显示错乱。

远控评估已提出屏幕快照语义，但 DT-08 剩余项没有列出。建议在 DT-08 中明确多附着时的尺寸归属，并把屏幕快照/重置列为手机接入前的已知限制或待办。

### G6 后台仍暴露已退役入口

侧边栏 Chat 已退出产品，但 daemon 仍接受 `start_sidebar_chat`、`cancel_sidebar_chat`、`clear_sidebar_chat`、`sidebar_chat`、`sidebar_chat_receipt`、`sidebar_chat_clear_receipt`、`chat_executors`、`sidebar_chat_watch`。远程网关将基于方法白名单构建，需要先移除这些入口或明确排除。

### G7 订阅分散、无范围过滤

除全局 `subscribe` 外，还有 `review_watch`、`project_runs_watch`、`workspace_watch`、`submission_watch`、`sidebar_chat_watch` 五条独立流，每条占一个 TCP 连接；全局事件流不按项目过滤。“统一订阅”已列为剩余项；这里补充远程所需的按授权项目过滤，以及单连接多路复用。

## 4. 文档不准确之处（已同步修正）

| 位置 | 问题 | 处理 |
| --- | --- | --- |
| [入口清单](desktop-operation-inventory-2026-10-16.md)、CURRENT_WORK DT-09 | 写 115 项（生产 113 项）；代码 `generate_handler!` 实际为 116 项（生产 114 项，验收特性 2 项）。表格缺 `core_project_runs_watch`、`core_project_runs_unwatch`、`core_review_file_close` | 更正计数并补表 |
| [共享状态记录](desktop-shared-state-2026-10-16.md) | “已提取全部 100 个 handler”；状态表仍把侧边问答列为现行状态 | 更正 |
| [远控评估](remote-access-assessment-2026-10-16.md) §3.2 第 1、3、4 条 | 仍写 Diff/Git、文件、PTY、项目运行依赖 Tauri State/窗口，TerminalHost 以窗口标签作 owner；这些已由 DT-08 迁入 daemon | 加注当前状态，并补 G1/G2 |
| 修复/Fork/审查启动/审批/停止的实施记录及 CURRENT_WORK | “持久化”“跨窗口恢复”易被理解为后台跨客户端恢复 | CURRENT_WORK 加注实际是本机客户端存储；各历史记录不逐篇改写 |
| [后台服务](core-daemon.md) | 订阅说明仍提到灵动岛 | 加注已移除 |

## 5. 已处理（2026-10-17）

**G2 项目注册迁入后台。** 新增 `apps/desktop/src-tauri/src/project_registry.rs`，路径规范化、可读性检查、32 目录上限、按名称+规范路径复用、`ProjectId` 分配都在 daemon 内完成；daemon 新增 `register_project`/`update_project` 方法，由服务级 `ServiceControl.projects` 互斥串行化（编辑同时持有审查范围 gate），新建或编辑后照常调度运行目录分析。Tauri 的 `core_register_local_project`/`core_update_local_project` 只保留窗口入口校验并转发，移除进程内 `project_gate`；前端命令名不变。新增真实 loopback 测试：两个独立客户端并发注册同一目录只产生一个项目，规范化路径、相对路径拒绝、编辑规范化与过期版本拒绝均通过。剩余：通用 `execute` 仍接受原始 `RegisterProject`/`UpdateProject`（大量测试夹具依赖），远程网关不应暴露该原始入口，接入时一并收口；手机端所需的宿主目录浏览接口属 RA 范围。

**G6 移除已退役的侧边问答后台入口。** daemon 删除 `chat_executors`、`sidebar_chat`、`sidebar_chat_receipt`、`start_sidebar_chat`、`cancel_sidebar_chat`、`clear_sidebar_chat`、`sidebar_chat_clear_receipt` 方法及 `sidebar_chat_watch` 流，`CoreClient` 对应方法、订阅枚举分支和空闲退出中的问答检查一并删除，消除 `review-acceptance` 特性下的两条未使用告警。6 项只覆盖问答的 daemon 测试随之移除；审查测试共用的只读假驱动改名为 `ReadonlyTestDriver`/`readonly_test_driver`。Core 的 `sidebar_chat` 模块及已有持久事件仍保留，是否删除另行决定。

验证：`cargo fmt --all -- --check`；`cargo clippy -p desktop --all-targets --locked -- -D warnings` 及加 `--features review-acceptance` 均无告警；`cargo test -p desktop --lib --tests --locked` 216 项通过、7 项按原规则忽略。未运行前端测试（前端未改动）、未打包。

**G1（第一项）开始修复改由后台驱动。** 新增 `apps/desktop/src-tauri/src/review_repairs.rs` 与 daemon 方法 `review_repair`（start/get/resume/abandon），桌面入口 `core_review_repair`。服务在受理时冻结所选问题、目标 Agent、任务版本，切换 Agent 时在服务内构建上下文包并记录哈希；随后以固定命令身份依次确认上下文、核实 `task.preferred_executor_changed` 事件和会话版本、提交修复轮次。每个审查运行一份记录（`review-repairs/<taskId>/<runId>.json`，原子写入），永久删除会话时一并清理。客户端断开不影响服务继续执行；服务重启后待完成记录保留，由任一客户端显式“继续”或“放弃”，不自动重放。放弃时用原提交命令作废，迟到的同一命令不会再被受理；已完成的 Agent 切换不撤销。前端 `reviewRepairOperations.ts` 改为薄客户端（不再写 localStorage），恢复面板读取服务记录并每 3 秒刷新，因此另一窗口或设备发起的修复也可见；协议移除 `submitReviewFix`/`reviewFixReceipt`，改为 `reviewRepair(call)`。旧版本留在 localStorage 的 `dogcodes:pending-review-repair:*` 记录不再读取（产品未发布，不做迁移）。

验证：新增 3 项真实 loopback 测试——服务驱动提交且同 ID 重试不重复提交、同 ID 不同内容拒绝；冻结后由另一客户端发现并继续切换 Agent 的修复，只提交一次；会话忙时拒绝且不留记录，放弃后原提交命令被作废、之后可发起新修复。desktop Rust 219 项通过（7 项按原规则忽略），两种特性下严格 Clippy 与格式检查通过；前端类型检查通过，138 个文件 994 项测试通过。未打包，未用真实 Agent 验收；会话删除清理路径未单独加测试。剩余 G1：带 Agent 切换的普通发送、审查启动、Fork，以及审批/停止的后台待核实发现。

**G1（第二项）普通发送改由后台驱动。** 在既有 `submission_intents` 服务上新增 `start`/`continue`/`retire` 调用：服务用自己的任务视图冻结发送（新对话时分配任务与创建身份；切换 Agent 时构建上下文包并记录哈希），写入原有后台意图记录后依次执行“创建会话或确认上下文 → 以原事件核实版本 → 提交消息”，每步使用固定命令身份；同一身份重试必须内容相同，已结束则直接返回结果。Core 新增只读的 `submission_stage`（从原有结果判定中拆出，判定逻辑不变）。失败时意图保持待处理，任一客户端可显式继续或放弃；放弃会作废尚未受理的创建/上下文及消息命令。前端 `submissionOperations.ts` 由约 430 行本机状态机改为薄客户端：不再写 localStorage，待处理列表来自后台分页发现；App 发送改为一次调用，新对话在服务返回后再切换到对应会话；“核实发送结果”若证明原发送已结束，会请求后台持久结束记录。浏览器演示客户端提供同形的内存实现，保证网页演示和界面测试继续可用。旧 localStorage 中的 `dogcodes:pending-submission:*` 不再读取。

验证：新增 3 项服务测试——新对话创建并发送、同 ID 重试不重复发送；切换 Agent 时服务自建上下文并发送；冻结后的待处理发送可继续、同草稿第二次发送被拒、同 ID 改内容被拒、放弃后原创建命令被作废且继续为只读。原有 22 项发送意图测试全部通过。desktop Rust 222 项、agent-core 87 项、agent-protocol 6 项通过，全工作区可编译，相关 crate 严格 Clippy 与格式检查通过；前端类型检查通过，137 个文件 959 项测试通过（删除只针对本机缓存同步的 `submissionSharedRecovery.test.ts`，其余发送测试按新接口重写）。未打包，未用真实 Agent 验收。剩余 G1：审查启动、Fork，以及审批/停止的后台待核实发现。

**G1 收口：其余记录重新归类。** 逐个核对审查启动（`reviewStartOperations.ts`）、Fork（`forkOperations.ts`）、审批与停止：它们各是一条后台命令，服务已按运行/任务/审批身份持有执行、重放与回执，结果通过共享状态（审查运行列表、Fork 回执与会话列表、待审批列表、任务运行状态）对所有客户端可见。客户端本机记录只用于在回复丢失时用同一身份重试自己的请求，不构成跨客户端业务事实；手机端实现同样的少量重试逻辑即可，不需要后台工作流。因此 G1 中真正的多步编排（开始修复、普通发送）已全部迁入后台，G1 视为完成。Git/文件操作的后台分页发现 `core_review_operations` 保留，供以后需要跨端操作历史时接入。

**G5 终端多附着：尺寸归属已明确。** 一个终端只有一个 PTY 尺寸，所有附着客户端共享；最近一次成功的调整生效。`TerminalInfo` 新增 `cols`/`rows`（持久记录带默认值，旧记录可读），启动时取原意图尺寸，调整成功后更新并唤醒长轮询，因此手机等后接入的客户端能读到实际尺寸，再决定按该尺寸渲染或主动调整。终端屏幕快照仍未实现：重新附着只重放最多 1 MiB 的输出尾部并在缓存截断时报告缺口，全屏 TUI 可能需要一次重绘（例如发送 Ctrl+L）；此项列为手机接入阶段（RA-02）的已知限制。终端专项 15 项测试通过。

**G7 项目订阅与协议客户端证明。** 服务新增 `subscribe_project`：只推送一个项目的会话与项目流，以及安装、额度、共享设置等全局流；订阅后新建的会话凭其创建事件加入范围；全局与项目订阅共用同一套回放和掉队补齐逻辑（过滤后的序号有间隔，最后送达的序号仍是有效续传游标）。新增 `remote_client_tests`：两个只走服务协议、不经过窗口的客户端，在真实 loopback 服务上完成注册项目、新建会话发送、另一端发现并完成待处理发送、共享设置、终端启动/读取/尺寸共享，以及项目订阅隔离，作为“手机可只依赖服务接口完成日常操作”的可执行证据。手机功能与服务方法的逐项映射见[服务接口映射](service-api-contract-2026-10-17.md)。按设备授权过滤和单连接多路复用仍属远程阶段。

## 6. 建议的处理顺序

1. ~~G2~~、~~G6~~ 已处理，见第 5 节。
2. G1 已完成（见第 5 节）；审批/停止补后台发现；恢复界面重新接入 Git/文件的后台发现。
3. G5 尺寸归属已完成，屏幕快照随手机阶段处理。
4. G3 在 DT-08 中定为例外或迁入后台；G4、G7 并入 RA-02 的保活与统一订阅。

DT-08/09/10 的验收应增加一条：**另一个客户端（不只是另一个窗口）可以发现并核实或接续已发起的操作**。
