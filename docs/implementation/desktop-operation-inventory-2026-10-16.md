# 桌面操作可靠性：入口清单与核对矩阵

> 2026-10-17：侧边栏 Chat 已退出桌面产品范围，9 项专属 Tauri 入口及前端实现已移除。下文早期计数/问答验收描述为历史状态；内部 Core/daemon 与持久记录暂保留，不再提供桌面问答入口。


2026-10-17 daemon 另增仅供远程网关使用的 `subscribe_project`（无桌面入口），手机功能与服务方法映射见[服务接口映射](service-api-contract-2026-10-17.md)。新增 `core_review_repair` 后注册表为 117 项（生产 115 项，2 项仅验收特性）；开始修复改由 daemon `review_repair` 冻结并驱动，客户端不再拼装修复提交命令。同日较早的核对更正：`generate_handler!` 当时为 116 项（生产 114 项，2 项仅验收特性），CoreCommand 仍为 18 项；此前写作 115/113 且下表漏列 `core_project_runs_watch`、`core_project_runs_unwatch`、`core_review_file_close`，已补。面向手机的核对另发现：发送（切换 Agent）、开始修复、审查启动、Fork、审批、停止等多步流程由前端编排，待核实记录只在本机 localStorage，下文“跨窗口恢复/持久化”指同一桌面 WebView 存储而非跨客户端；`core_review_operations` 后台发现已无前端调用方；daemon 已退役的侧边问答方法已于同日删除，项目注册/编辑改由 daemon `register_project`/`update_project` 统一校验与串行化（Tauri 入口名不变）。见[远控核对](desktop-remote-readiness-audit-2026-10-17.md)。

2026-10-17：新增运行状态订阅/退订和文件操作结束确认，未恢复 Chat 或操作记录界面。受理时驱动捕获、Fork 冻结计划/失败恢复、Git 超时 Unknown、运行配置与操作状态通知及删除后恢复记录核实见[本轮实施记录](desktop-foundation-acceptance-recovery-watch-2026-10-17.md)。下文早期“提交说明/项目分析仍直接查驱动”及未完成的相关描述由本记录补充，不能据旧段落重复开发；长期资源与跨平台候选验收仍保留。

日期：2026-10-16。对应 DT-09。这是当前源码入口的完整清单和逐域核对起点，不是“全部操作已可靠”的证明。后续每个域必须补足具体的操作身份、版本、回执、终态、取消、断连恢复和验收证据，再标完成。

## 1. 清点边界

主会话已接通共享禁用、明确安装身份、Ready 及当前安装检查，且按 run ID 保留已受理运行的原驱动。224 项原生测试与全部 store examples 编译通过，见[主会话就绪准入](desktop-primary-ready-admission-2026-10-16.md)及[运行归属记录](desktop-run-driver-ownership-2026-10-16.md)。回执恢复优先于新任务检查；安装变化不把在跑任务交给替换实例。提交说明及项目分析等其他入口与组合流程的受理边界仍待核对，不将整域可靠性标为通过；没有新增入口或协议命令。

新增通用只读 `task_deletion`/Tauri `core_task_deletion` 永久删除证明，问答恢复记录按证据在客户端清理；24 项原生、36 项前端及最终类型检查通过，见[恢复记录清理](desktop-sidebar-chat-retirement-2026-10-16.md)。本轮新增 1 个 handler，计数和下表同步更新；其他业务域删除恢复仍须逐项核对。

侧边问答的删除/项目目录变更已与问答受理及收尾排序，删除后释放缓存和通知源；等待锁的入口重查会话，28 项原生/存储回归通过，见[删除边界](desktop-sidebar-chat-deletion-2026-10-16.md)。客户端待核实问答/清空记录的永久删除处理和长记录开销仍待推进。

侧边问答已增加按会话后台状态订阅、窗口转发和前端查询合并/重连，22 项原生、26 项前端专项及最终类型检查通过，见[问答状态订阅](desktop-sidebar-chat-watch-2026-10-16.md)。本轮新增 2 个只读订阅/退订 handler，清点数与下表已同步更新。

侧边问答已补后台共享 Agent 禁用设置的候选过滤与新请求准入，并与设置写入排序；恢复/取消不被禁用设置阻断。历史工作块 17 项原生专项通过，见[问答准入记录](desktop-sidebar-chat-admission-2026-10-16.md)。安装就绪、安装身份和当前选择检查也已统一到 Core，80 项 Core + 5 项桌面专项通过，见[安装准入补充](desktop-sidebar-chat-readiness-2026-10-16.md)。审查候选和新受理现已复用后台禁用/Ready/当前安装检查，原请求优先、受理后禁用不停止，51 项原生专项通过，见[审查准入记录](desktop-review-admission-2026-10-16.md)。主会话、提交说明和项目分析等其他入口继续核对。

后续核对发现并修复侧边问答未纳入 `retire_if_idle` 的遗漏；真实 loopback RPC 和 Core 生命周期专项共 15 项通过，见[问答后台退出记录](desktop-sidebar-chat-lifecycle-2026-10-16.md)。本次没有增加注册入口，统一订阅与 Agent 跨入口准入等仍待推进。

从 `apps/desktop/src-tauri/src/lib.rs` 的实际 `generate_handler!` 注册表提取 122 项，其中 2 项只在 `review-acceptance` 特性下存在，生产配置为 120 项。`core_execute` 内部另有 18 个 CoreCommand 变体，不能把它算成单个操作就漏掉发送、审批、删除等分支。命令载荷内部的 Git action、批量文件操作、运行目录和供应商差异仍需继续展开；入口清单不等于所有操作变体已穷举。

本机窗口显示、文件选择、系统浏览器等只需本端适配，不要求手机重放相同 UI 指令。业务执行与事实源需要共用后台；纯查询无需制造持久命令记录；终端输入字节禁止自动重放。

## 2. 按域核对

| 域 | 已见机制 / 当前边界 | 必须补齐的核对与恢复规则 |
| --- | --- | --- |
| Agent / 历史共享设置 | Core 版本条件、持久 Ack、System 流快照与全局游标；客户端保留未确认操作 ID；已有针对性验证 | 实机双窗口、长历史开销；异常设置不自动清空；不能覆盖其他客户端更高版本 |
| 会话、轮次、审批 | CoreCommand 及规范事件/投影；回执已绑定类型化载荷、调用者和版本条件，Core 快速路径与 SQLite 事务内均校验，见[命令身份](desktop-command-identity-2026-10-16.md) | 逐命令核对幂等键与业务意图绑定、冲突、断连及审批已决状态；重发不得产生第二个轮次；永久删除失败及后续清理分开核实 |
| Fork / 审查 / 侧边问答 | 独立 Core API 经 daemon 转发，部分不走 CoreCommand；侧边问答启动已补历史原请求绑定和持久恢复，清空已补版本条件及原请求回执，见[专项记录](desktop-sidebar-chat-recovery-2026-10-16.md) | Fork 已补外部副作用前的原请求绑定、只读回执和跨窗口恢复，见[Fork 记录](desktop-fork-recovery-2026-10-16.md)；独立审查启动的跨窗口恢复已补齐，见[审查记录](desktop-review-start-recovery-2026-10-16.md)；开始修复已补原问题/主 Agent/上下文步骤持久化及显式恢复，见[修复恢复](desktop-review-repair-recovery-2026-10-16.md)；普通主会话命令、多仓库中间状态仍需核对原请求、执行准入、取消及终态、服务重启；已有执行不能因窗口消失而变成已完成 |
| Diff / Git / 文件修改 | ReviewHost 及主要查询/写入、撤销和审查材料已迁至 daemon，桌面只转发与做系统 UI 适配；服务专项累计 33 个不同原生用例最终通过，见[迁移记录](desktop-review-service-2026-10-16.md) | commit/push/pull/sync 已接入持久原请求、意图/回执及桌面核实恢复；Commit & Push 保留后续推送及失败重试身份，见[Git 回执记录](desktop-git-operation-recovery-2026-10-16.md)。暂存/取消暂存、丢弃/忽略、批量文件操作已接入同一持久记录机制及文件恢复入口，保留部分完成结果，见[文件回执记录](desktop-file-operation-recovery-2026-10-16.md)。逐轮撤销已接入相同持久身份，撤销和已知回执的跨进程失效订阅已接通，见[撤销/订阅记录](desktop-undo-state-subscription-2026-10-16.md)。后台回执分页发现与只读详情已实现，见[操作记录](desktop-operation-history-2026-10-16.md)；组合 Git 工作流的后台意图与固定接续身份已实现，见[组合工作流记录](desktop-git-workflow-2026-10-16.md)；共享结束标记及与接续的仲裁已实现，见[结束状态](desktop-git-workflow-close-2026-10-16.md)；接续仓库目标绑定已实现，见[推送目标记录](desktop-git-push-target-2026-10-16.md)；三平台真实进程重启、远端/hook/长记录验收仍待完成 |
| 终端 / 项目运行 | 后台终端启动意图/资源身份/epoch、七项 daemon RPC 已接入桌面终端/项目进程；12 项不同原生用例和前端适配通过，项目配置/分析也已迁入后台，沿用 JSON/generation | 手动分析/编辑已有持久回执、参数绑定、版本条件及明确拒绝/结果未知分类；已补启动原请求的持久绑定（专项验收见运行配置记录）；继续记录增长和三平台实机恢复验收；启动命令防重复；输入不重放，输出缺口明确；后台重启不自动再运行，详见[生命周期记录](desktop-terminal-lifecycle-2026-10-16.md) |
| 文件查询 / 监听 / 租约 | 文件读取/搜索已迁至 daemon，OS watcher 按后台连接管理，桌面仅取消转发任务；`ReviewHost` 的快照/独立阅读者租约也已迁入后台，窗口不持有缓存 | 不给纯查询增加无意义持久写入；快照/文件版本和读取边界明确；订阅断连释放订阅，不停止其他业务；资源过期可重取 |
| 附件 / Artifact / 导出 | 托管附件保存/查询/读取已迁入 daemon，固定上传身份与字节摘要、前端核实/重开恢复已接入；验证状态见[附件恢复](desktop-attachment-recovery-2026-10-16.md) | 已发布未引用文件及崩溃临时目录的安全回收、后台跨设备发现仍待收口；桌面保存对话框与实际写入分开；重复导出不能悄悄覆盖不同文件 |
| Agent 探测 / 登录 / 更新 / 额度 / 模型 | 既有查询、原生进程和部分主窗口专属入口；升级在 Tauri 进程内执行，退出桌面会中断（远控核对 G3） | 区分查询与会启动进程的操作；升级/登录结果未知先重新检测，取消不影响其他 Agent；缓存不能掩盖认证失败 |
| 运行中权限 / 会话重建 / 上下文 | 部分 API 在 CoreCommand 外 | 要求明确目标 Run/版本；竞态不能操作下一轮或重复重建；权限变更以实际供应商确认结果为准 |
| 应用窗口 / 启动 / 诊断 | 本机 UI/系统适配 | 业务状态不依赖窗口标签；重启/退出语义明确；诊断脱敏，导出有真实完成/取消反馈 |
| 事件 / 历史 / 状态查询 | Core 持久事件及快照 API | 快照游标与内容一致、live/replay 无缺口、重复去重和掉队恢复；不拿查询成功推导命令成功 |

### 文件监听与快照的下一工作块

文件查询与监听的服务迁移、连接清理和重连快照已接通，验证与剩余范围见[文件服务记录](desktop-workspace-service-2026-10-16.md)。窗口注册表现在只保存可取消转发任务，按 epoch 隔离旧任务清理；OS watcher 随后台连接释放。

Diff 独立阅读者租约与乱序保护已实现，见[阅读租约记录](desktop-review-leases-2026-10-16.md)。快照/租约/SnapshotOwner/写入 gate，以及读取、续租/释放、暂存、Git action、丢弃/忽略/批量文件操作、逐轮撤销、审查材料生成、永久删除清理现已由同一后台服务持有，见[服务迁移记录](desktop-review-service-2026-10-16.md)。项目目录修改也与该 gate 协调。桌面只保留窗口权限与本机打开/显示动作，路径解析在服务核实。

提交/推送/拉取/同步已补持久原请求及回执、查询入口和桌面恢复面板；Commit & Push 的后续推送和已知失败重试保留固定标识，专项见[Git 回执记录](desktop-git-operation-recovery-2026-10-16.md)。暂存/取消暂存、丢弃/忽略和批量文件修改也已接入持久回执及恢复列表，见[文件回执记录](desktop-file-operation-recovery-2026-10-16.md)。逐轮撤销也已绑定原请求并接入回执恢复；后台失效订阅连接撤销卡片及 Git/文件恢复列表，见[撤销/订阅记录](desktop-undo-state-subscription-2026-10-16.md)。后台回执分页发现已接通，见[操作记录](desktop-operation-history-2026-10-16.md)。组合 Git 工作流的后台意图及固定身份接续已实现，见[工作流记录](desktop-git-workflow-2026-10-16.md)。共享结束标记已接入后台，见[结束状态](desktop-git-workflow-close-2026-10-16.md)。组合推送仓库目标绑定已接通，专项见[目标记录](desktop-git-push-target-2026-10-16.md)。侧边问答启动及清空的原请求绑定、版本条件和跨窗口恢复已接通，最新专项 13 项原生、21 项前端及类型检查通过，见[问答恢复](desktop-sidebar-chat-recovery-2026-10-16.md)。Fork 原请求与恢复入口已接通，7 项原生、17 项前端及类型检查通过，见[Fork 记录](desktop-fork-recovery-2026-10-16.md)。独立审查启动的客户端恢复也已接通，113 项不同前端用例及类型检查通过，见[审查恢复](desktop-review-start-recovery-2026-10-16.md)。CoreCommand 的回执身份绑定已实现，86 项原生用例通过，见[身份记录](desktop-command-identity-2026-10-16.md)。命令只读受理查询已接通 Core/daemon/Tauri 和修复/上下文客户端接口，12 项原生测试及客户端验证通过，见[回执查询](desktop-command-receipt-2026-10-16.md)。开始修复的原意图持久化和分阶段恢复界面已接通，38 项不同前端用例及类型检查通过，见[修复恢复](desktop-review-repair-recovery-2026-10-16.md)。主会话停止的原目标/版本持久跟踪及受理恢复也已接通，47 项不同前端用例及类型检查通过，见[停止恢复](desktop-cancel-recovery-2026-10-16.md)。审批来源绑定及无答案的只读受理/投递查询已接通，见[审批回执](desktop-approval-receipts-2026-10-16.md)；审批跨窗口恢复、显式作废未受理回复及投递状态刷新已接通，见[审批恢复](desktop-approval-recovery-2026-10-16.md)。普通发送/创建的固定身份、只读回执及显式作废基础接口已补齐，见[发送身份基础](desktop-submission-identity-2026-10-16.md)。普通发送/创建的分阶段持久意图及核实/继续/安全放弃 UI 已接通，累计 136 个不同前端用例及类型检查通过，见[发送恢复](desktop-submission-recovery-2026-10-16.md)。附件上传原身份/回执、内容摘要及后台读取已接通，16 项原生、66 项不同前端用例和类型检查通过，见[附件恢复](desktop-attachment-recovery-2026-10-16.md)。普通发送的后台原意图、只读分页发现与有证据的共享结束现已接通，见[共享发送记录](desktop-shared-submission-intents-2026-10-16.md)。普通发送的永久删除标记解析、删除时正文清理及重启补清理也已接通，验证见同一记录。下一工作块核对安全回收、发送意图统一订阅/长期记录开销及其他工作流删除边界，以及取消的真实投递/退出验收、Fork 多阶段外部副作用和其他共享状态；不能把当前专项的通过当作全部写操作已可靠。保持有界缓存及独立阅读者保护，候选实机再覆盖 GUI 真正退出和服务真正重启。

## 3. 已注册 Tauri 入口

本表只记录注册位置，不把函数名当成可靠性或权限审计证据。业务语义与字段以上面的逐域核对及后续实现记录为准。

| 注册 handler | 源码模块 | 范围 |
| --- | --- | --- |
| `external_links::open_website` | `apps/desktop/src-tauri/src/external_links.rs` | 生产入口 |
| `agent_login::open_agent_login` | `apps/desktop/src-tauri/src/agent_login.rs` | 生产入口 |
| `attachments::core_save_attachment` | `apps/desktop/src-tauri/src/attachments.rs` | 生产入口；固定上传身份，后台原子发布 |
| `attachments::core_attachment_upload_receipt` | `apps/desktop/src-tauri/src/attachments.rs` | 生产入口；只读原上传回执 |
| `attachments::core_read_attachment` | `apps/desktop/src-tauri/src/attachments.rs` | 生产入口 |
| `attachments::core_export_attachment` | `apps/desktop/src-tauri/src/attachments.rs` | 生产入口 |
| `project_runs::core_project_runs` | `apps/desktop/src-tauri/src/project_runs.rs` | 生产入口 |
| `project_runs::core_project_runs_analyze` | `apps/desktop/src-tauri/src/project_runs.rs` | 生产入口 |
| `project_runs::core_project_run_edit` | `apps/desktop/src-tauri/src/project_runs.rs` | 生产入口 |
| `project_runs::core_project_run_operation` | `apps/desktop/src-tauri/src/project_runs.rs` | 生产入口 |
| `project_runs::core_project_run_open` | `apps/desktop/src-tauri/src/project_runs.rs` | 生产入口 |
| `project_runs::watch::core_project_runs_watch` | `apps/desktop/src-tauri/src/project_runs/watch.rs` | 生产入口；运行状态变更提示订阅 |
| `project_runs::watch::core_project_runs_unwatch` | `apps/desktop/src-tauri/src/project_runs/watch.rs` | 生产入口；仅释放本窗口转发 |
| `terminal::core_terminal_open` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_list` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_get` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_read` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_input` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_resize` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `terminal::core_terminal_close` | `apps/desktop/src-tauri/src/terminal.rs` | 生产入口 |
| `review_acceptance::report` | `apps/desktop/src-tauri/src/review_acceptance.rs` | 仅验收特性 |
| `review_acceptance::mutate_review_fixture` | `apps/desktop/src-tauri/src/review_acceptance.rs` | 仅验收特性 |
| `review_window::core_review_window_open` | `apps/desktop/src-tauri/src/review_window.rs` | 生产入口 |
| `review_window::core_review_window_close` | `apps/desktop/src-tauri/src/review_window.rs` | 生产入口 |
| `review_window::core_review_window_is_open` | `apps/desktop/src-tauri/src/review_window.rs` | 生产入口 |
| `review_window::core_review_window_running` | `apps/desktop/src-tauri/src/review_window.rs` | 生产入口 |
| `review::core_review_query` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::workspaces::core_review_workspaces` | `apps/desktop/src-tauri/src/review/workspaces.rs` | 生产入口 |
| `task_deletion::core_task_deletion` | `task_deletion.rs` | 生产入口；只读永久删除证明，支持恢复记录清理 |
| `review::core_open_workspace_file` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_workspace_file_applications` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_reveal_workspace_file` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `workspace_browser::bridges::core_workspace_watch` | `apps/desktop/src-tauri/src/workspace_browser/bridges.rs` | 生产入口 |
| `workspace_browser::bridges::core_workspace_unwatch` | `apps/desktop/src-tauri/src/workspace_browser/bridges.rs` | 生产入口 |
| `workspace_browser::core_workspace_find_files` | `apps/desktop/src-tauri/src/workspace_browser.rs` | 生产入口 |
| `workspace_browser::core_workspace_roots` | `apps/desktop/src-tauri/src/workspace_browser.rs` | 生产入口 |
| `workspace_browser::core_workspace_directory` | `apps/desktop/src-tauri/src/workspace_browser.rs` | 生产入口 |
| `workspace_browser::core_workspace_preview` | `apps/desktop/src-tauri/src/workspace_browser.rs` | 生产入口 |
| `core_subagent_history` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `review::core_review_snapshot_lease` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_file` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_file_close` | `apps/desktop/src-tauri/src/review.rs` | 生产入口；文件操作结束确认 |
| `review_repairs::core_review_repair` | `apps/desktop/src-tauri/src/review_repairs.rs` | 生产入口；开始修复的后台工作流（start/get/resume/abandon），服务端记录可跨客户端发现 |
| `review::core_review_git_action` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_git_operation` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::git_workflow::core_review_git_advance` | `apps/desktop/src-tauri/src/review/git_workflow.rs` | 生产入口 |
| `review::git_workflow::core_review_git_close` | `apps/desktop/src-tauri/src/review/git_workflow.rs` | 生产入口 |
| `review::core_review_file_operation` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::operation_history::core_review_operations` | `apps/desktop/src-tauri/src/review/operation_history.rs` | 生产入口 |
| `review::watch::core_review_watch` | `apps/desktop/src-tauri/src/review/watch.rs` | 生产入口 |
| `review::watch::core_review_unwatch` | `apps/desktop/src-tauri/src/review/watch.rs` | 生产入口 |
| `review::core_review_commit_summary` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_history` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_branches` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_index` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::core_review_discard` | `apps/desktop/src-tauri/src/review.rs` | 生产入口 |
| `review::batch_file_actions::core_review_batch_files` | `apps/desktop/src-tauri/src/review/batch_file_actions.rs` | 生产入口 |
| `review_file_actions::core_review_preview` | `apps/desktop/src-tauri/src/review_file_actions.rs` | 生产入口 |
| `review_file_actions::core_review_reveal` | `apps/desktop/src-tauri/src/review_file_actions.rs` | 生产入口 |
| `review_file_actions::core_review_ignore` | `apps/desktop/src-tauri/src/review_file_actions.rs` | 生产入口 |
| `core_pick_project_directory` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_reveal_local_project` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_register_local_project` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_update_local_project` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `restart_application` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `show_main_window` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_startup_status` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_startup_diagnostics` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_export_startup_diagnostics` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_startup_trace` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_bootstrap` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_agent_preferences` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_history_preferences` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `turn_review::core_review_executors` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_turn_reviews` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_fork_task` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_fork_receipt` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_review_source` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_start_turn_review` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_start_project_review` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `turn_review::core_cancel_turn_review` | `apps/desktop/src-tauri/src/turn_review.rs` | 生产入口 |
| `core_recovery_snapshot` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_prepare_session_rebuild` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_export_diagnostics` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_quota_snapshots` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_available_models` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_model_ready_executors` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `agent_updates::core_check_agent_update` | `apps/desktop/src-tauri/src/agent_updates.rs` | 生产入口 |
| `agent_updates::core_upgrade_agent` | `apps/desktop/src-tauri/src/agent_updates.rs` | 生产入口 |
| `agent_updates::core_open_agent_updater` | `apps/desktop/src-tauri/src/agent_updates.rs` | 生产入口 |
| `core_scan_agent` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_permission_catalog` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_set_running_permission_mode` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_timeline` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_timeline_snapshot` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_query_history` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_pending_approvals` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_workspace_snapshots` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_task_artifacts` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `turn_undo::core_turn_undo` | `apps/desktop/src-tauri/src/turn_undo.rs` | 生产入口 |
| `turn_undo::core_turn_undo_status` | `apps/desktop/src-tauri/src/turn_undo.rs` | 生产入口 |
| `core_read_artifact` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_export_artifact` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_build_context_preview` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_build_edited_context_preview` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_execute` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_command_receipt` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口（只读受理查询） |
| `core_invalidate_pending_command` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口（显式作废未受理的创建/提交/上下文阶段，持久幂等） |
| `submission_intents::core_submission_intents` | `apps/desktop/src-tauri/src/submission_intents.rs` | 生产入口；后台原意图登记、只读分页/详情/结果解析、以 Core 证据结束；2026-10-17 起 `start`/`continue`/`retire` 由服务冻结并驱动普通发送 |
| `submission_intents::watch::core_submission_watch` | `apps/desktop/src-tauri/src/submission_intents/watch.rs` | 生产入口；主窗口建立只读变更提示流，初连/重连需查询持久状态 |
| `submission_intents::watch::core_submission_unwatch` | `apps/desktop/src-tauri/src/submission_intents/watch.rs` | 生产入口；仅释放本窗口转发，不停止发送 |
| `core_prepare_approval_response` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口（无写入的审批查询身份准备） |
| `core_invalidate_approval_response` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口（显式作废原版本的未受理回复；持久幂等） |
| `core_approval_receipt` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口（无答案的审批受理/投递查询） |
| `core_sync_events` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_subscribe` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |
| `core_unsubscribe` | `apps/desktop/src-tauri/src/lib.rs` | 生产入口 |

## 4. CoreCommand 变体

以下变体由规范协议定义；它们共享外层 envelope，但各自仍需验证副作用/取消和结果核实。表中三个内部记录命令在桌面 RPC 有显式拦截，不是供 UI 任意伪造的公共写入。

| 变体 | 核对说明 |
| --- | --- |
| `SetAgentPreferences` | 共享设置工作块已有针对性证据 |
| `SetHistoryPreferences` | 共享设置工作块已有针对性证据 |
| `RegisterProject` | 桌面入口改走 daemon `register_project`：服务内规范化、复用与服务级互斥；通用 `execute` 仍接受原始命令，远程网关不得暴露 |
| `UpdateProject` | 桌面入口改走 daemon `update_project`：服务内规范化、版本条件、项目与审查范围 gate；通用 `execute` 同上 |
| `CreateTask` | 主发送路径已固定 taskId/操作身份/版本零，核对原创建事件并支持继续/作废；[发送恢复](desktop-submission-recovery-2026-10-16.md)。其他创建入口及真实后台重启仍需核对 |
| `RenameTask` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `DeleteTask` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `RestoreTask` | 保留协议分支；不代表恢复旧的最近删除 UI |
| `SetPreferredExecutor` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `ConfirmContextPackage` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `OptOutReconciliation` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `SubmitTurn` | 主发送与开始修复已保存原载荷/阶段身份/任务边界并提供恢复；[发送恢复](desktop-submission-recovery-2026-10-16.md)。托管附件完整性专项已通过；其他载荷变体、共享发现及真实供应商仍待验收 |
| `CancelTurn` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `RespondToApproval` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `CaptureWorkspaceDiff` | 继续逐命令核对，不能仅凭共用 envelope 标完成 |
| `RecordTestResult` | 内部记录/恢复；核对桌面 RPC 拒绝和内部来源 |
| `RecordInstallationProbe` | 内部记录/恢复；核对桌面 RPC 拒绝和内部来源 |
| `ReconcileMissingInstallation` | 内部记录/恢复；核对桌面 RPC 拒绝和内部来源 |

## 5. 完成条件

每项核对记录须指向具体实现与故障用例；至少有“请求未到达”“已生效但 Ack 丢失”“另一个客户端先修改”“执行中断连/重启”“取消与完成竞态”的适用性说明。外部副作用若不能判定，就返回结果未知并提供核实路径，不宣称通用 exactly-once。

后续增删 handler 或 CoreCommand 时更新清单。当前尚未完成所有载荷变体及服务/窗口权限审计，因此 DT-09 保持进行中，不能用这份入口清单作为发布放行证据。
