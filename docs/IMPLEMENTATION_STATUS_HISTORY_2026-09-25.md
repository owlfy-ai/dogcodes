> 历史归档（截至 2026-09-25）。以下保留整理前全文，其中“当前”“下一步”“尚未实现”等描述只代表当时阶段，不能作为现行待办或实施约束。现行范围、状态和顺序以 [当前工作清单](CURRENT_WORK.md) 为准。

# dogcodes 实施状态

> 2026-09-25 会话删除：已实现可恢复删除、最近删除列表和恢复；普通/置顶/归档会话复用侧栏入口，确认后执行。主任务或审查排队/执行/取消中禁止删除。删除状态持久化，保留消息、审查、历史 Diff 与原生会话；恢复原列表位置，不自动清空，不删除项目代码。具体设计与验证见 [实施记录](implementation/conversation-deletion-2026-09-25.md)。本条更新下方历史“删除未实现”的状态。

## 2026-09-24 会话与灵动岛更新

已实现持久化会话重命名，后续消息不会覆盖手动名称；灵动岛接入独立审查的 Agent、类型、状态及分组进度，审批优先，区分失败/取消/中断，终态展示 60 秒。真实 macOS 桌面验收已通过重命名、重启保持、Kimi 审查与 Codex 主任务并行、岛内审批、单项取消、受控审查进程失败及终态退场；补正入口聚焦和主任务/审查耗时文案。完整会话导出按用户决定暂缓。实现、验证及剩余范围见 [本轮记录](implementation/conversation-island-2026-09-24.md)，本节更新下方较早的未完成描述。

## 2026-09-24 审查修复闭环与并发验收更新

已接入选择问题→主 Agent 修复→修复轮次新版本复审，复用逐轮/项目 Diff 入口，持久保存来源与处理进度。两个 Codex 对话、Codex 与 Kimi 同项目并发写入及取消隔离通过；桌面完成双入口、历史定位、取消/原材料重试、重启和定向修复验证。修复不完整报告的分组状态及 JSON 加说明的解析兼容问题。范围、最终复验和后续事项见 [本轮记录](implementation/review-closure-2026-09-24.md)，本节更新下方较早未完成描述。

## 真实审查验证（2026-09-24）

Codex 代码/安全审查、Kimi 性能分析此前真实调用通过。本轮其余 Agent 使用同一套 2 组代码缺陷：Grok、ZCode 通过；Cursor、Antigravity 调整冷启动预算及 Antigravity Hook 系统代理继承后最终复验通过，各自两个缺陷均经独立会话复核确认。CodeBuddy 前两次未完整通过；用户要求再次重试后，在未改代码或配置的情况下完整通过 2/2 组、10 次冻结检索和 2 个独立复核会话，两个缺陷均确认，最新状态更新为复验通过。此前失败证据保留，未断言全部由网络波动造成。Claude Key 已过期，本轮跳过。启动等待可取消，62 项相关回归、严格 Clippy、格式和桌面编译通过。此轮四家通过者未主动发起冻结检索，不能冒充该交互已验证；未新增桌面点击/逐轮采集验收。证据与边界见 [真实验收记录](implementation/review-native-2026-09-24.md)。


## 审查编排升级（2026-09-24）

已在原有逐轮/项目 Diff 三类审查中接入按目录和大小分组、冻结版本上下文读取/搜索，以及独立 Session 的 AI 二次复核。复用已有 Agent 与入口；分组进度、覆盖缺口、复核支持/反驳/待核实和执行记录在共用结果区域显示。单组失败、取消和重启保留已完成结果，重试固定原材料。

上下文有界采集，旧快照不补读当前文件；复核必须有可核验原文引用，反驳不会删除初审问题。不新增 OCR CLI/模型依赖。具体预算、兼容性和验收边界见 [审查编排记录](implementation/review-workflow-2026-09-24.md)。本节更新下方历史记录中的“后续实施”状态。


## 审查质量升级（2026-09-23）

在原有三个审查类型与逐轮/项目 Diff 双入口上，新增结构化问题、严重程度排序、代码证据与位置校验、唯一片段行号纠正、精确重复合并，以及点击问题查看持久化送审版本的 Diff。两个入口复用同一结果组件与 Diff 阅读器。按类型、语言和目录匹配基础规则，展示逐文件覆盖（明确为 Agent 自报）与缺失材料，旧文字报告仍可读。

本次未增加模型接入或 OCR 运行依赖；分组审查、同版本上下文检索、独立 AI 二次复核仍属后续阶段。实现、借鉴来源及验证边界见 [审查质量记录](implementation/review-quality-2026-09-23.md)。此节优先于较早的能力描述。


## 项目 Diff 审查（2026-09-22）

保留逐轮入口，同时在项目 Diff 面板加入代码审查、安全检查和性能分析，复用已接入 Agent 与额度展示。以当前比较快照为输入，结果在面板中持久化展示，支持单独取消和使用原快照重试。实现和验收边界见 [项目审查记录](implementation/project-review-2026-09-22.md)。

## 逐轮审查与历史 Diff（2026-09-19）

- 已实现每个已完成轮次的底部操作栏：左侧复制、分叉、完成时间；右侧历史 Diff、代码审查、安全检查、性能分析（2026-09-21 收敛为这三类），独立选择并记住审查 Agent。
- Hub 只读委派复用全部已接入且就绪的 Agent：独立原生 Session 与 Run，独立取消/重试；结果持久化在原轮次下，带来源进入主 Agent 的下一轮上下文。菜单复用主对话的安装去重规则，每个 Agent 只显示一项；共享连接的驱动为审查创建独立连接。
- 历史审查固定该轮已保存的文字与改动材料；重试复用原材料。旧轮次缺少快照时不读取当前文件冒充历史；共享目录按时间段采集，不能据此独占归因给单一 Agent。
- 分叉继承截至选中轮次的历史，并沿用同一项目目录，不还原文件。长历史/审查结果明确按摘录附送，全文保留在任务历史中供按需查询。
- Copilot SDK 按最新决定暂缓。下一步是各已接入 Agent 的真实审查与桌面端联调验收，以及其余 M1 平台/发布验收；本次离线 Driver 契约、Core 集成和浏览器组件验收不能替代真实 Agent 全链路验收。
- 实现与验证见 [逐轮审查记录](implementation/turn-review-2026-09-19.md)。本节优先于下方较早批次的未实现清单。


## 产品范围修订（2026-09-19，优先于下方历史记录）

- 同一项目允许不同 Agent，或同一 Agent 的多个独立对话，同时修改共享目录代码。项目级单写者约束不再是需求，不应作为缺陷修复或 M1 收尾任务；单个对话的活动轮次及审批/取消隔离仍保留。
- Git 提交备注“AI 总结”不要求独立执行留痕；不增加该功能的 ExecutionRun、输入/输出 Artifact 或审计记录要求。其他上下文交接的既有来源记录不受此调整影响。
- M1 本次稳定性收尾已完成：修复服务锁释放缺陷，格式与严格 Clippy 检查通过；逐轮只读委派的当前范围见上节；Copilot SDK 暂缓，继续真实 Agent 联调和剩余验收。
- 同项目并发需分别验证不同 Agent、同一 Agent 多对话，以及 Session、消息、审批和取消的隔离；需求修订和稳定性收尾不代替这些验收。

## 稳定性收尾结果（2026-09-19）

- 服务锁改为作用域退出时显式解锁，补齐共享描述符、跨进程互斥与强杀恢复回归；桌面测试 16 线程连续 20 轮通过。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过；`review-acceptance` 特性下严格 Clippy 和 56 项桌面测试也通过。
- 最终 `cargo test --workspace`：436 通过、61 忽略、0 失败；`pnpm check`：324 项测试、类型检查和构建通过。
- 首轮完整回归出现一次 Claude 测试进程退出超时；单项复测、Claude 测试组连续 5 轮及最终完整回归通过，保留为观察项，未将其标为根因已修复。详见 [修复与验证记录](implementation/m1-stability-2026-09-19.md)。M1 不据此标记整体完成。

> 2026-09-12 补充验收：Codex、Claude Code、ZCode、Antigravity、Grok、Cursor、CodeBuddy 七家真实 IPC/Driver 均通过待审批时 Core 服务硬杀、重启直接继续；旧审批失效、旧命令未重放、原生回复完成。结合此前 Kimi GUI，八家均有直接继续证据；不等于八家 GUI 或运行中 Shell 存活全矩阵。见 [状态一致性](implementation/execution-state-consistency.md)。

> 2026-09-12：已移除异常断连后的人工状态确认门槛。旧本地轮次自动关闭并释放占用，用户可直接继续；结果未知仍显示连接中断，不伪造完成。94 项 Rust、19 项前端专项及 Kimi 真正服务崩溃后的 GUI 直接继续验收通过，见 [状态一致性](implementation/execution-state-consistency.md)。

> 2026-09-12：已接通独立 Core 后台服务，默认桌面改走本地 IPC；服务持有 Driver、数据库锁和历史 MCP。桌面 Rust 31 项（含 6 项服务回归）、Codex/Kimi RPC 客户端硬杀后的真实完成验收、Kimi 桌面/灵动岛两次硬杀恢复、前端 198 项与类型检查已通过，见 [实施记录](implementation/core-daemon.md)。服务异常与桌面退出分开处理，不宣称八家 GUI 全矩阵或硬件链路通过。


> 数据库存储策略（2026-09-11）：项目尚未发布，不支持旧数据迁移或数据库升级。现已删除版本分支、补列、启动回填及旧宿主失败事件兼容。下文迁移记录仅为历史，不是当前要求。实现与验证见 [数据库策略](implementation/database-initialization.md)。

## 当前状态与下一步（2026-09-12）

本节是当前工作入口；后文较早批次的测试数字和失败记录仅说明当时结果，不作为当前待办。Driver 验收、原生 GUI 验收与发布版验收分别计数，不相互替代。

### 已完成

- [x] 历史增强验证：Rust 67 项、前端 198 项、类型检查、前端构建和桌面 cargo check 通过；覆盖超过 16 MiB 的 SQLite 历史、分块确认、投影重建和取消。严格 Clippy 有既有依赖警告；八家真实 Agent/GUI 本次未重验，见 [本次验证记录](implementation/history-query.md)。

- [x] 历史查询改为五个 stdio MCP 工具，八家 Driver 带外注入；移除用户消息中的历史 CLI 说明，保留分页/覆盖范围，查询不再额外脱敏，并按轮次撤销查询权限。实现与证据见 [历史查询](implementation/history-query.md)。

- [x] 七家 Agent 的双目录运行取消、跨进程续接回忆、待审批取消均已有真实 Driver 通过记录，包括接入审批 Hook 后的 Antigravity。
- [x] ZCode CLI 0.16.5 冷恢复补齐原生模型运行配置，保留模型/思考强度；审批回传所选原生 response，真实批准落盘、拒绝未写通过。该 CLI 版本续接能力为 Supported，未知版本不据此提升。
- [x] ZCode GUI 正常退出/重启后复用同一原生会话并回忆历史；桌面和灵动岛批准/拒绝、待审批取消及两端同步通过。
- [x] ZCode GUI 待审批取消后重启，通过新会话与历史包恢复；这是历史恢复通过，不是原生 session ID 不变的续接。
- [x] 恢复中心显式重建通过：准备重建不提交消息；下一条新消息才建立会话、附带历史，不重发旧指令。会话失效状态使用隔离数据库故障注入。
- [x] 正常/数据库损坏模式下诊断导出 Save/Cancel 通过；宿主锁故障导出 Cancel 通过。损坏测试数据库原字节不变、无业务入口与 scratch。
- [x] 多目录逐轮/操作变更采集及历史只读查询已实现；Codex → Kimi → Codex 双目录文件证据交接真实通过。
- [x] GUI 验收中修正旧工具状态残留、空白工具行和加载提示字号缩放；最终 typecheck 与前端 184 项测试通过。

### 下一步（按顺序）

1. [x] 宿主断连与原生执行终态已分离：服务异常恢复自动关闭旧本地轮次并释放占用，显示连接中断、允许直接发送消息继续；旧审批失效，继续时向 Agent 提供检查已有结果的提示，不将未知结果标为完成。不将外部 shell 存活直接判为缺陷，不自动终止 Agent。见 [状态一致性](implementation/execution-state-consistency.md)。
2. [x] 当前八家 Provider 的主要原生 GUI 验收矩阵已检查并完成证据审计；审批、正常原生续接、取消后恢复和断连状态分别计数。Claude/Kimi/Grok/Cursor 发现项已修复复验。Antigravity/Codex/Grok 运行取消不保证终止已启动 Shell；旧 Codex 0.128.0 原生配置不兼容，不计通过。迟到审批的确定性后端回归不等同于八家 GUI 延迟 IPC 穷举。详见 [全 Agent GUI 验收与边界](implementation/all-agents-gui-acceptance.md)。
3. [x] 历史能力增强：Canonical 逐页读取替代 16 MiB 预加载快照；按 SessionMap 记录事实哈希及交付字节范围，切回时补交游标以前未覆盖的事实。确认边界为 Driver 接受/MCP stdout flush，不代表模型理解。验证结果与边界见 [历史查询](implementation/history-query.md)。
4. [ ] 下一工作包：Copilot SDK（或正式替代 SDK 切片）与 Hub 只读 Delegation；补 M1 主题、目录选择、启动错误和打包验收。历史增强的八家真实 Agent/GUI 重验与既有注入验收分别计数。

其余验收边界保留：浅深主题矩阵、原生目录选择、其他启动错误类别、打包发布版；不将这些标为已通过。损坏库浏览/自动修库不属于当前交付，高级文件系统恢复、adoption 恢复和 worktree 清理不重新纳入待办。Copilot SDK、Hub 只读 Delegation 等独立工作包仍未完成，M1 不标记整体完成。

依据：[恢复实施与 Driver 证据](implementation/m1-s7-recovery.md)、[原生 GUI 验收](implementation/recovery-gui-acceptance.md)、[多目录项目](implementation/multi-root-projects.md)、[历史查询](implementation/history-query.md)。

## 取消与恢复实现（已完成）

- 取消期间暂停审批，停止投递失败恢复审批，终态关闭审批；旧轮次回复不得投递到新轮次。宿主中断恢复记录审批失效，保留多目录变更证据。
- 每轮执行前检查全部项目目录；附加目录缺失时不调用原生 Prompt。
- 丢失/不兼容/已脱离会话可准备重建；数据库仅初始化空库，不维护版本升级或旧数据迁移；显式投影重建仍保留。
- 启动失败可独立导出白名单诊断摘要，无需健康 Core/数据库；原生保存不覆盖已有文件。

## 本轮交付结果（2026-09-11）

- [x] 第一项主体：所有项目目录的逐轮/操作采集、目录身份、双目录持久化起点与中断恢复；多目录 v2 Artifact，单目录 v1 保留。
- [x] 第一项真实验收：Codex → Kimi → Codex 两目录同名文件接力；逐项正常审批；Kimi 使用历史 CLI 的 getChangesSinceCheckpoint/getArtifact 读到附加目录旧内容，修改后 Codex 正确对账。非 Git 临时目录未执行 git init。
- [x] 第二项主体：当前对话分页浏览、字面全文检索、决策/变更筛选、附件与文件 Diff 入口；Agent 每轮任务级只读查询快照与五种查询操作；明确页游标、源范围和超限省略。
- [x] 自动化：Core 21、Store 36、双目录集成 1，共 58 项 Rust 测试通过；前端 167 项通过；桌面 cargo check、前端生产构建通过。双目录进程强制退出恢复及重复恢复幂等通过，浏览器 Demo 搜索与版面验证通过。
- [x] 后续补验已覆盖七家 Driver 多目录取消/恢复；原生 GUI 的剩余矩阵、浅深主题与目录选择见本文当前清单。Codex/Kimi 文件交接成功不扩大为七家交接通过。
- [x] 2026-09-12 已完成后续历史增强：逐事实范围记录和超出快照上限的分页访问。lastSeenSequence 仅保留执行检查点语义，具体字节/格式边界见历史查询文档。数据库仅初始化新库，不提供旧数据迁移。

实现细节与边界见 [历史查询](implementation/history-query.md) 和 [多目录项目](implementation/multi-root-projects.md)。此结果覆盖下方历史段落中“附加目录未采集”“历史入口未实现”“文本接力未验收”的旧状态；旧记录仅作历史依据。


## 已交付工作包的范围基线（2026-09-11）

1. 多目录逐轮变更与交接：补齐所有项目目录的起点、净变更、操作快照和中断恢复；同名文件必须保留目录身份。真实跨目录审批、取消、恢复与 A→B→A 文件交接单独验收。
2. 历史按需检索：分页浏览与全文搜索，按任务查询决策、变更和 Artifact；提供当前 Agent 可调用的只读入口，明确交付与省略范围。

已完成的原生模型目录、模型/思考强度独立选择和额度展示修订见 [模型选择](implementation/native-model-selection.md)。模型菜单不提供搜索。ZCode 不监控工具调用额度，剩余百分比紧随进度条，日期前标注“重置时间：”。

Codex→Kimi→Codex 原始问答接力已经通过；剩余验收针对跨目录文件证据，不重复列为未实现。Review 普通查看、暂存、丢弃/撤销已交付；高级文件系统恢复及 worktree 清理不再纳入待办。Copilot SDK 和 Hub 只读 Delegation 尚未完成，原生子 Agent 观察不等于 Hub Delegation；M1 不据此标记整体完成。


多目录项目首版实现：创建弹窗、持久化、原生 adapter/目录提示已接通；前端 91 项、Rust workspace lib、Codex 进程 fixture 和生产 build 通过；测试及边界见 [实施记录](./implementation/multi-root-projects.md)。真实双目录 Codex/Kimi 文件交接已通过；其余 GUI 边界见当前清单。桌面开发库改为 `app-v5.db`，旧库保留不迁移。

2026-09-09 上下文续批：自动交接已取代预览弹窗；逐轮 Git start→end 净变化、变更文件版本及 patch 已保存为 Artifact 并接入交接。详见 [M1-S5B](./implementation/m1-s5-turn-changes.md)。下方旧的预览确认流程仅为历史记录。非 Git 有界采集与真实接力已完成，该批次尚未完成的历史查询与细粒度交付已由 2026-09-12 增强覆盖；完整项目恢复不属于 Diff 范围。

更新日期：2026-09-09

## 最新状态（优先于下方历史基线）

- Review 已精简为差异查看与普通文件操作；当前范围见 [实现说明](implementation/review-panel.md)，本轮测试及原生界面结果见 [交付审计](implementation/review-delivery-audit.md)。

- Antigravity 圆环统一使用 Gemini Models 5 小时额度；模型组发送门禁保持独立，详情继续展示全部额度窗口。

- 所有 Coding Agent CLI 子进程已统一接入代理环境：继承大小写 HTTP/HTTPS/ALL/NO_PROXY，缺失项由原生系统接口补齐，不执行外部命令。macOS 使用 SystemConfiguration，Windows 使用 WinINet/WinHTTP/注册表，Linux 使用 GNOME GLib/GIO、KDE 配置或桌面沙箱 portal；覆盖主会话、额度读取和模型目录。macOS 原生探针已在桌面用户会话直接读到 `127.0.0.1:7890`；清空全部代理环境变量后，Grok ACP Ready 与 billing 真实只读探针仍通过，证明端到端使用原生系统代理，且未发送模型 Turn。PAC/WPAD 到固定 CLI 环境变量的映射和启动后热更新暂不支持。
- 额度旧快照仍参与安全判断并显示采集时间，但不再显示周期出现的“数据待刷新”横幅，也不再让额度圆环因 stale 状态改变透明度，避免刷新周期造成布局与视觉跳动。
- 额度总览卡片副标题已将 `ins_…` 内部标识替换为 Agent 版本号；Agent 下拉菜单不再显示版本号，每项右侧显示与该 Installation 匹配的最严格额度窗口圆环，未知读数显示问号。
- Codex、Grok Build、ZCode 图标已完成浅色/深色主题适配：浅色背景显示黑色图形，深色背景显示白色图形；滤镜只作用于 SVG 内层，不改变图标边框和背景。系统主题与手动主题均覆盖。
- Agent 图标容器已统一为 small 20px / medium 26px，品牌图标保留统一 2px 内边距并使用 contain 填充，改善不同 Logo 之间的视觉大小差异。
- 根据浅色额度监控页面反馈，品牌图标恢复统一 2px 内边距；Cursor 改为主题自适应白色/黑色资源，避免浅色背景下显示为低对比灰色。
- Antigravity 图标已替换为用户提供的 SVG 资源，所有 AgentIcon 使用位置统一生效；Vite build、typecheck 与前端 60 项测试通过。
- Claude Code 图标已替换为用户提供的 D97757 路径 SVG，所有 AgentIcon 使用位置统一生效。
- ZCode 图标已替换为用户提供的三段路径 SVG，所有 AgentIcon 使用位置统一生效。
- Cursor 图标已替换为用户提供的六边形路径 SVG，所有 AgentIcon 使用位置统一生效。
- Grok Build 图标已替换为用户提供的文字标志 SVG，所有 AgentIcon 使用位置统一生效。
- Kimi 图标已从 Kimi 官网公开 `pwa-192.png` 资源提取并固化到 `apps/desktop/src/assets/kimi.png`；AgentIcon 已切换本地 PNG，避免运行时依赖官网网络。
- Codex 图标已从 ChatGPT 官方 `sprites-core-7bb22fec.svg#55180d` 提取为本地 `apps/desktop/src/assets/chatgpt.svg`，AgentIcon 已替换原 `◎` 占位符。
- Agent 列表已隐藏内部 Fake driver；Claude Code、Grok 等 CLI 的多路径重复安装统一按 ready 优先、版本最高去重。Codex 桌面与 CLI 是不同运行时，仍按既定产品语义分别保留。

- 额度异常确定性验收已补充：固定时钟 14 项矩阵及重置/微量余额组件回归。修复小额剩余取整误封禁、过重置时间旧零读数误导；reset/TTL 本地定时重渲染，不依赖网络返回。真实供应商耗尽与网络故障未制造，不等同真实账号故障验收。

- 会话空白缺陷已解决：重复点击当前 task 清空数据却未重新加载是确定根因。统一选择处理、时间线/审批/额度请求代次隔离、额度刷新不阻塞聊天、显式加载失败重试已实现。44 项前端测试/typecheck 通过；真实桌面重启读取、重复点击 5 次及快速往返切换 5 次均保留历史。下方“空白待复现”已被本条覆盖。

- 本轮桌面权限已解阻：真实窗口完成原生模型列表、Gemini/Claude 额度切换、详情、提交锁定、重启恢复及续聊 OK。发现并修复跨轮消息合并：Store v4 按 task/turn/item 隔离，历史投影事务重建，30 项测试通过。核心验收通过；首次切换偶发空白仍待独立复现，真实耗尽账户门禁未覆盖。下方权限阻塞说明为历史记录。

- Antigravity 显式双模型真实验收通过：Gemini gemini-3.1-pro-low、Claude claude-sonnet-4-6，顺序短 turn 完成且无工具调用，57.20 秒。修复 stdin 未实际关闭导致模型交接挂起；Driver 7 tests、前端 41 tests、typecheck 与 Driver clippy 通过。
- 真实桌面进程与 Vite 已启动，但交互验收未通过：CUA 不识别开发版应用，System Events 返回辅助功能权限不足。待权限解阻后验证模型选择/锁定/恢复、额度组圆环；不得把启动成功当作交互验收成功。

- Codex 桌面 Runtime 探测回归已修复：manifest 显式覆盖系统/用户 Applications 内的 ChatGPT/Codex
  bundle，不依赖启动终端 PATH；仍按一个 `codex` Provider、多 Installation 建模并 canonical 去重。
  缺失安装协调会复查已记录 executable 是否实际消失，不再因发现提示变化误发 removed。本机
  0.153.4 已重新探测并注册 Ready。
- 额度 wire 回归已修复：`QuotaSnapshotView` 现在按 TS 契约序列化 `providerId`、
  `installationId`；此前 snake_case 导致 Composer 对所有 Agent 匹配失败并统一显示无读数。
  Kimi/Grok 真实只读额度探针仍通过。
- 额度耗尽 UI 闭环已完成：新鲜 provider-reported 任一窗口为 0% 时圆环和总览明确显示“已用尽”
  并禁用发送；Unknown、stale、已到重置时间和 Demo 不阻止发送。没有订阅窗口的 Agent 若在回合中
  明确报告耗尽，也会通过安全 `quota_exhausted` 时间线提示用户切换 Agent。
- ZCode 1308 已真实映射为结构化耗尽终态；Codex/Claude/Kimi/Grok/Cursor/Antigravity 接入保守
  明确措辞/code 分类。供应商原始错误不持久化，普通 429/瞬时 rate limit 不冒充订阅耗尽。
- Beta Agent 当日真值统一迁移到
  [`beta-agent-acceptance.md`](./implementation/beta-agent-acceptance.md)。2026-09-07 的真实
  Stream/Cancel/Approval 重跑已完成；accepted 回执、未触发审批、空文本均不算通过。
- ZCode Driver 已完成 `session/subscribe`、`session/event` envelope、真实终态等待、同连接 stop、
  session/read context 隔离和临时会话 close 后复查。零模型 lifecycle、真实流式+上下文、真实
  cancel 均通过。审批批准落盘/拒绝未写、冷恢复历史回忆已于 2026-09-11 真实复验通过；旧 1308 耗尽记录仅作历史证据。
- Codex、Claude、Kimi、Grok、Cursor 的严格临时目录 deny/allow 审批均通过；Grok 使用正常审批模式。Antigravity 接入审批 Hook 后真实复验通过，旧 Unsupported 结论不再作为当前状态。
- M1-S7 首切片：侧栏“恢复与诊断”、安装/会话/运行检查、返回会话、停止本实例管理且等待
  审批的运行、原生 JSON 摘要导出已实现。诊断不包含对话、路径、工具参数、凭据。
- Core 新增进程内 Run 归属保护，拒绝新实例停止旧实例的 Run；不自动重放用户指令。
- 已实现既有数据库只读 quick_check/版本预检，失败时显示恢复说明且不启动 Core/Agent。
  恢复模式独立诊断导出已实现并通过 GUI 保存/取消；不支持损坏库会话浏览或自动修库。
- 遗留审批失效协调已实现；任意投影落后检查未据此宣称完成，worktree/备份清理不纳入当前范围。
- 实际 Driver 已扩至 Codex/Claude/Kimi/Grok/Cursor/Antigravity/ZCode 七条；四供应商额度接入状态
  仍以 M1-S6 为准。下面“四条 Driver”、Codex 审批待接、Cursor 未接等段落保留为早期记录。
- 当前实施与验证见 [M1-S7](./implementation/m1-s7-recovery.md)。

## 历史基线（仅保留阶段上下文，Agent 验收状态不得从本节读取）

- 阶段：M1 第一纵向切片之后，多 Agent Product Driver 铺开。
- 当前工作包：多 Agent Product Driver 铺开完成一轮——Codex（App Server JSON-RPC）、Kimi/Grok
  （原生 ACP）、Claude Code（原生 headless stream-json，零额外依赖）四条 Driver 均已打通
  discovery → initialize 级真实就绪探针 → Driver 注册链路；ZCode 完成发现事实（自定义
  app-server 协议），Driver 属独立工作包。
- 桌面形态：Tauri 2 UI + 独立 Core 服务，本地 IPC；不提供用户 CLI。
- 默认真实持久化：操作系统 app data 目录下的 `app.db`。
- 真实 Turn 验收（2026-09-03，操作者授权后执行，固定 no-tools prompt，每驱动三轮：流式+完成 /
  同会话复用 / 中断）：**Claude 2.1.218、Kimi 0.40.1、Codex 0.153.0-alpha.5、Grok Build 1.0.13
  全部三轮通过**（`cargo test -p <driver> -- --ignored`）。验收修复三个真实缺陷：Claude 中断终态
  映射、ACP connection loop 在 Prompt 期间不轮询 cancel、ACP probe 未验证 session 能力导致未登录
  也显示 Ready（现映射 NotConfigured，Grok 未登录时正确表现）。
- 可运行 Driver：`FakeDriver` + 四条真实 Driver 全部进入 Beta（真实 Turn 已验收；
  会话跨进程 resume 仍未验收；PendingInteraction 审批透传 Claude/ACP 已接通，Codex 待接）。
- 当前工作包新增 M1-S6：Codex 独立 QuotaProvider 已接通官方 App Server 账户额度读取；
  Composer 圆环/Popover 与独立额度总览页已接线。Claude 推送、Kimi loopback、Grok billing
  已接入；缺失字段仍显示 Unknown。当前边界和验证结果以 M1-S6 文档及 HANDOFF 置顶为准。

## 已实现

- Cargo/pnpm workspace 和固定工具链。
- 强类型前缀 ID、ExecutionRun 状态机。
- `preferred_executor` 与 active Run executor 分离。
- Command Actor、幂等键、expected stream version。
- Canonical Event append、SQLite WAL、事务内 Projection、rebuild。
- Project/Task 创建、Fake Driver 流式 Timeline。
- CompanionProjection revision 和敏感字段裁剪基础模型。
- Tauri async commands：bootstrap、timeline、execute。
- Tauri Channel 有序业务流：replay、caught-up、live、显式 unsubscribe。
- Channel lag/sequence gap 从 SQLite Event Store 分页补齐。
- 前端 `OrderedEventCursor` 连续应用、去重与缺口恢复。
- React 两栏对话优先 UI、无项目 scratch 会话、可选项目连接弹窗、按工作目录聚合的多会话侧栏、
  会话/时间线/Composer Agent 图标以及浏览器 Demo Adapter。
- Driver 协议告警和未知扩展仍作为脱敏 Canonical Event 保留用于诊断，但不再生成用户可见 Timeline
  条目；Timeline 读侧过滤旧版本遗留行，既有数据库无需删除或迁移，Projection rebuild 后结果一致。
- 初始对话优先选择状态为 Ready 的具体 Agent 安装（Codex → Claude → Kimi → Grok），完整保留
  `installationId`；没有 Ready 外部 Agent 时才回退 Fake Driver。
- QuotaProvider 与会话 Driver 分离；Codex 账户级 5 小时/周窗口、重置时间及按当前 native thread
  隔离的上下文/缓存命中率进入 Rust/Tauri/TS/UI。pull 15 秒缓存为权威，push 只触发失效；
  模型专属 bucket 不与账户额度混合。
- `agent-runtime` 结构化进程执行：无 shell、环境 allowlist、超时、并发 drain 与输出截断。
- `agent-registry`：版本化 TOML manifest、安全校验、PATH/平台路径发现、canonical path 去重和 semver policy。
- external-process manifest 必须声明 `version_output_markers`；只有成功退出、产品签名匹配且 semver 可解析时才进入 Installed/UnsupportedVersion，防止同名可执行文件串线。
- Codex 启动版本探针：稳定 `InstallationId`、fingerprint 缓存、Canonical Event、SQLite Projection/rebuild 和 Bootstrap snapshot。
- 桌面后台扫描内置 Codex manifest；UI 展示本机安装路径、版本、状态、耗时和安全诊断。
- 安装状态 wire/UI 已覆盖 `Installed / NotConfigured / Ready / UnsupportedVersion / Broken`。
- 成功 discovery 后按 Provider + canonical executable 计算差集；磁盘已缺失的安装写入 `agent.installation_removed`，当前 Projection 删除，Canonical Event 历史保留且 rebuild 结果一致。
- 内部探针与缺失安装协调命令强制 `SystemRecovery` actor，并校验当前 Installation 身份；Tauri UI 不能伪造系统命令。
- 根目录 `pnpm check` 一键运行 typecheck、前端测试和生产构建。
- `experiments/acp` 固定官方 Rust SDK 2.0.0 stable v1、关闭 default features，并完成 initialize、session、stream、permission-cancel、cancel 和未知扩展保真离线契约。
- `ContextPackageV1` wire 已覆盖 Handoff/Reconciliation、三档强度、确定性字段、ContextPointer、来源区间与结构化语义摘要来源。
- Context Package 显式绑定 source/target `ExecutorRef`，payload hash 因而同时约束交接方向。
- Core 从冻结的 Task Canonical Event 区间生成 payload 和 SHA-256；重复构建稳定，未知事实不编造，默认不展开 prompt、消息、终端或完整 Diff。
- SQLite `task_events` 按 Task + `(after, through]` 严格筛选并限制 10,000 条；测试证明其他 Task 和区间外事件不会进入来源。
- Tauri/TS 已暴露 `buildContextPreview`；桌面切换下一轮 Agent 前展示 Handoff Preview。
- `redaction` 默认规则集 v2 本地处理 quoted JSON secret、Bearer/Basic、URL userinfo、PEM、已知 token、assignment、高熵候选及邮箱/主目录；Core 初建、编辑重哈希与 TestReport 持久化前强制执行。重复处理不重复计数，provenance/UI 如实显示规则版本；旧 Artifact 不重写。
- `codex-driver` 已作为首个外部 Product Driver 接入：具体 Installation registry、
  initialize/account readiness、Thread create/resume、同进程复用、Context 在 Prompt 前交付、
  流式文本/工具/终态及 interrupt Port。Core/UI 已以 task/turn/run + expected version 接通停止；
  未知方法/payload 不进入 Canonical，审批只安全拒绝。
- 桌面只将 readiness 通过的安装标为 Ready 并列入下一轮选择，保留 installationId；当前 Codex、
  Claude Code、Kimi、Grok 四条 Product Driver 均已完成真实 streaming、同会话第二 Turn 和
  interrupt 验收。跨进程 resume 与 PendingInteraction 仍保持 Unknown。
- Claude Code 使用原生 headless stream-json 通道，不依赖 ACP npm adapter；本机 2.1.218 已作为
  Ready Driver 注册。详见 `docs/compatibility/claude-code.md`。
- 符合统一 ID 规范的 `ArtifactId = art_<ULID>` 与 SQLite Context Package CAS 已落地；`artifact_blobs` 按 SHA-256 去重，仓储再次校验内容、hash、字节数和 1 MiB 上限。
- `ConfirmContextPackage` 以 Task stream version 和已审阅 hash 为前置条件，由 Core 重建冻结预览；Artifact、`handoff.package_confirmed` 和 `task.preferred_executor_changed` 在同一事务提交。
- hash 不匹配时零写入；幂等 Ack 查询前置，重复确认不会因执行者已经切换而误报冲突。
- `handoff.package_confirmed` 可重建为统一时间线条目，但名称明确表示“已确认”，不冒充“已发送”。
- `AgentSessionMapSummary` 与 SQLite `agent_session_maps` Projection 已覆盖 Task、Executor、native session/turn、workspace、版本、resume capability、状态和 `lastSeenSequence`。
- `AgentDriver` 已定义 capability-gated `resume_session`；`PromptRequest.context[]` 的 Port 契约要求实现先按序交付全部 ContextDelivery，再交付用户 prompt，返回成功表示完整请求已接受。
- Core/Fake Driver 运行写入 `session.ready/resumed/resumable`；`driver.turn_started` 关联 native turn，terminal event 推进已见 sequence。Fake 明确声明 resume Supported 并恢复同一 native session；未知能力不会被提升。
- `context_transfers` Projection 跟踪 confirmed/failed/sent；sent/failed 事件记录 Artifact、payload hash、目标 Session 与固定安全失败原因，并进入统一 Timeline。
- Core 根据上一实际 `driver.turn_started` Executor 强制跨 Agent Transfer；直接绕过 Preview 修改 preferred executor 也无法把用户 prompt 发送给新 Agent。
- Driver 拒绝完整 PromptRequest 时只写 transfer failed/turn failed，Fake 证明用户 prompt 未被接受；同一确认 Artifact 保持可重试，成功后才转 sent。
- React 根据 Bootstrap SessionMap 自动路由：无可恢复 Session 使用 Handoff；切回 Supported+Resumable Session 使用 Reconciliation，并严格从 `lastSeenSequence` 构建增量。
- Fake A→B→A 集成测试验证 Handoff、B 新 Session、A 原 Session resume、Reconciliation 增量、sent 在 driver turn 前、最终 hash 以及 Projection rebuild。
- Context 编辑接口强制重建 Hub 原稿并锁定 schema/kind/strength/source/target/provenance；编辑草稿再次脱敏后才返回安全预览与新 hash，未脱敏草稿不写 Event/Artifact。
- 编辑确认在同一事务保存 Hub 原稿和最终 `art_<ULID>`，`artifact_versions` 记录 `user_edit` parent 关系；React 可编辑目标/已完成/待办并显示原稿→最终版本差异，应用后编辑框只保留安全文本。
- `OptOutReconciliation` 只接受有效的 Reconciliation 路由、已审阅 preview hash 和 Task expected version；`reconciliation.opted_out/opt_out_failed/opt_out_applied` 可重建，取消/X 不产生该决定。
- Core 将显式 opt-out 作为一次可审计的跨 Agent 运行门：仍须恢复 Supported+Resumable 目标 Session，但跳过 ContextDelivery；Driver 拒绝时保留决定重试，成功时在用户 turn 事件前标记 applied。
- Context transfer/opt-out 查询均遵循“同一目标的最新决定胜出”；较新的 sent/applied 版本会遮蔽更旧 failed 版本，不会把旧上下文再次投递。
- `WorkspaceContextProvider` 将 Core 与具体工作区采集解耦；无 Provider 时保持 unknown/空数组，不在 Core 内启动命令。
- `agent-workspace` 通过绝对 Git executable 和 `agent-runtime` 有界结构化 argv 读取 HEAD、porcelain v2 `--branch -z` status 与 staged/unstaged diff stat；不使用 shell、不读取完整 diff，3 秒、256 KiB 和 1000 路径硬上限。
- 变更路径稳定排序；≤1 MiB 普通文件写入 SHA-256，删除/符号链接/大文件不读正文。Cargo/npm/pnpm/yarn/Python/Go 清单与 lockfile 同时进入独立 `dependencyChanges` 字段。
- 工作树 fingerprint 覆盖 status、stat、路径状态和已采集内容 hash；真实临时 Git 仓库测试证明重复结果一致、内容漂移改变 snapshot。Core 确认重建时发现 workspace drift 会拒绝旧 preview hash 且零写入。
- Git 读侧集成矩阵已覆盖空仓库、clean、仅 staged/unstaged、staged+unstaged 同文件、untracked、ignored 默认排除、rename/delete、binary、symlink、executable bit、detached HEAD、linked worktree、空格/Unicode/换行路径与 1001 路径超限降级；这是设计 §22.3 的读侧增量覆盖，不等同于完整 Checkpoint/Adoption 矩阵。
- Desktop Host 自动发现系统 Git 后注入真实 Provider；Preview 显示变更文件数、依赖变化数和 base→working-tree fingerprint。
- `RecordTestResult` 是仅限 `SystemRecovery` 的采集适配器入口；命令、结果和受限报告经当前 Redactor 后序列化为版本化 `TestReportV1`，以 `application/vnd.dogcodes.test-report.v1+json` Artifact 和 `test.completed` 在同一事务提交。
- Test Artifact 强制 1 MiB 上限、内容 SHA-256/字节数复核和不可变 `art_<ULID>`；Event 只保存安全摘要与 `artifact:<id>`，报告正文不进入 Event，Context Package 确定性引用该 Artifact。
- resume 失败已细分为 `session.missing/incompatible/detached`，对应稳定 reason；Projection 能从 Event 重建。统一 `turn.failed` 不再写原始 Core/Driver 错误文本，只保存稳定 `failureKind` 和通用提示。
- `GitWorkspaceSnapshotV1` 显式记录 repo root、HEAD、branch/detached、SHA-256 worktree identity、`porcelain_v2` 和 status fingerprint；linked worktree identity 可区分，路径/状态仍受既有预算约束。
- Core 对每个已排队写入 Turn 在实际执行前后调用 Workspace Provider，写入 `workspace.snapshot_captured`；采集失败改写 `workspace.snapshot_failed`，只含 `workspace_unavailable/not_repository/timed_out/command_failed/output_truncated/path_budget_exceeded/invalid_output` 等稳定原因，不阻断 Driver。
- 起止 Snapshot Event 绑定 Task/Turn/Run/phase，并包含 changed paths、dependency changes 和 diff stat；集成测试证明顺序、失败降级、Agent 继续执行及 Projection rebuild 一致。
- SQLite `workspace_snapshots` Projection 保存成功/失败摘要；`Core::workspace_snapshots`、Tauri `core_workspace_snapshots` 和 TS Client 提供按 Task、最新优先、1–100 条有界查询，不把历史快照塞进 Bootstrap。
- `CaptureWorkspaceDiff` 只允许 `LocalUser` 且要求 Task expected version；Git Provider 用 0700 私有临时目录、独立 `GIT_INDEX_FILE` 和临时 object store，真实 object store 仅作为 alternate 读取。从 HEAD（unborn 为空树）初始化后 `add -A`，因此 staged、unstaged、删除和未跟踪内容进入同一完整 Patch。
- 所有 Git 读操作设置 `GIT_OPTIONAL_LOCKS=0`；真实仓库测试逐字节证明用户 index/status 不变，并证明真实 object store 文件数不增加。Patch 使用 `--binary --full-index --no-ext-diff --no-textconv`，前后 status fingerprint 不同则 Conflict，超过 1 MiB 则零写入拒绝。
- 成功 Patch 以 `application/vnd.dogcodes.git-diff.v1.patch` 不可变 Artifact 与 `workspace.diff_captured` 原子提交；`workspace_diff_artifacts` Projection 可重建，Timeline 显示引用。Context 仅在 Task、fingerprint 和冻结 `(after, through]` 区间同时匹配时填写 `changeSummary.artifactRef`。
- React 执行上下文区提供“固化当前完整 Diff”，运行中禁用并明确提示 Artifact 可能包含完整源码与敏感内容；该原始 Patch 不经过 redaction，metadata 以 ruleset 0 如实标识。
- 旧库升级当前为 `user_version = 2`：version 0 建旧表，version 0/1 增加 Task Artifact 归属表并完整回填 Projection，与版本更新共用 Immediate 事务；失败全部回滚，不修改 Canonical Event/CAS/dedup。未来版本拒绝迁移。
- 冻结预览的 Diff 查询已锁定 sequence 上下界；新生成的同 fingerprint Artifact、其他 Task 和空区间不会改变原预览，rebuild 前后结果相同。
- Git 在每条业务命令前只查询 clean/process filter 的配置键；含 include 的生效配置命中时返回稳定 `external_filter_configured`，不执行或回传其中的命令。所有 Git 命令统一关闭 fsmonitor/hooks/自动维护/可选 index 缓存；stat 与 Patch 均关闭 external diff/textconv。
- Git 子进程设置 `GIT_NO_LAZY_FETCH=1`、空 `GIT_ALLOW_PROTOCOL` 和 `GIT_TERMINAL_PROMPT=0`，禁止主动补拉缺失对象或交互登录；仍不继承凭据类环境变量。
- Runtime 提供 `run_bounded_bytes`，沿用同一超时/并发 drain/字节上限；原始 Patch 直接保存 bytes，非 UTF-8 内容不再被有损替换。Git 路径/status 文本严格解码，异常返回 `invalid_output`。
- 子目录项目统一限定 status/stat/Patch 的 `-- .` 范围，从 repo root 对 porcelain 路径取 hash；测试证明子目录内容漂移可见且 Patch 不含同仓库其他目录文件。HEAD 从同一 status 的 `branch.oid` 读取，失败不再被默认为 unborn。
- 浏览器 Demo 对本地 Git 固化返回明确的不可用错误，Task、sequence 和 Timeline 均不变。
- `task_artifacts` 从 Context 确认、Diff 固化和 Test 完成事件重建归属；原稿/最终稿均可列出，以 `(sourceSequence, artifactId)` 复合 cursor 分页，同事件的两份 Artifact 不会丢失。
- Core Artifact 服务只允许 LocalUser，验证 Task 归属、已审阅 hash、媒体类型、1 MiB 上限和 CAS 完整性后才返回最多 32 KiB 原始 bytes；越权、越界、损坏 metadata/blob 均无正文返回，也不改变 Task/sequence。
- Tauri Artifact 列表/读取/导出命令均校验注入的主窗口身份；请求只有强类型 ID 和声明字段，不接受任意路径。没有给 WebView 增加 dialog/fs 权限。
- React Artifact 弹窗支持主动读取、分页、分段、完整 hash 校验、只读 JSON 格式化、控制字符转义和非 UTF-8 hex 告警；正文不进入 Bootstrap/Timeline/Context。旧异步响应不会覆盖新选择。
- 原始 bytes 导出在全文校验与显式敏感内容确认后开放；Rust 原生保存对话框选择目标，同目录私有临时文件 `sync_all` 后不覆盖发布。文件写入单测已覆盖既有文件/symlink 拒绝、0600 权限、字节保真和失败清理；原生对话框交互仍待 UI 验收。
- Demo 仅内存保留本次确认的 Context Artifact，使用 ULID，ruleset 0 如实标识未脱敏，并明确拒绝导出；不冒充真实本地文件写入。接口与验收记录见 `docs/implementation/m1-s5-artifacts.md`。
- 冻结 Git Artifact 的只读结构化视图已实现：完整 hash 后显示文件导航、hunk、原/新行号、新增/删除计数和 rename/copy/mode/binary 说明；无效/不支持/非 UTF-8 回退原始/hex，最多展示 500 文件、2000 hunk、10000 行，部分计数明确标记。
- `diffView.test.ts` 以真实临时 Git 仓库验证与生产同格式的 Patch，另覆盖特殊路径、模式、symlink、恶意文本与格式/预算边界。浏览器独立内存夹具验证分段、截断、快速切换和关闭旧响应隔离；不读取项目/数据库，且不进入生产 HTML 构建入口。详见 `docs/implementation/m1-s5-diff-view.md`。

- `redaction::structured` 已提供代码内固定的 schema/path policy：未知/敏感键默认丢弃、类型/枚举/范围约束、全输入/输出结构预算，审计只记录数量，不回显未知键或正文。脱敏副本不能通过 Deserialize 冒充，落盘前再次投影验证。
- Codex M0 新增 `sanitize-fixture` 离线导出与 `check-sanitized-fixture` 只读二次检查：先验证全部有界 JSONL，再私有临时文件原子不覆盖发布；规范字节/metadata/payload 校验拒绝重复键和篡改。保留 parser/driver 版本，原生版本为 Unknown，不触发模型或改变原始审批 ID。
- 新增 14 个 Rust 回归覆盖结构策略、预算、CLI 输入不变、失败零输出、不覆盖/symlink/0600 和二次检查。实现及生产 Recorder/CI 未完成项见 `docs/implementation/m1-s5-raw-diagnostics.md`。
- 2026-09-03 评审修复（5 项，均有回归测试）：
  - `map_driver_event` 不再把 driver 原始错误串/原始 payload 写入 Canonical：`turn.failed` 只保存稳定
    `failureKind`（`driver_turn_failed`/`interrupted`）与固定提示，`VendorExtension` 只记录 `payloadBytes`，
    `ProtocolWarning` 经 redactor + 512 字符截断；driver 中途 `SessionReady` 改记 `driver.session_ready`
    诊断事件，不再撞击严格 `session.ready` 投影 schema。
  - `SubmitTurn` 强制要求 Task `expectedStreamVersion`（对齐设计 §10.1）；缺失返回 InvalidArgument，
    过期返回 Conflict；Tauri/TS/Demo 客户端全部改传 `task.streamVersion`，并发双派发被 store OCC 关闭。
  - driver 流无终止事件即结束时，由 Hub 补写 `turn.failed { failureKind: "driver_stream_ended" }`，
    active run 不再永久悬挂。
  - PATH 发现过滤空/相对条目，堵住"CWD 内同名可执行 → canonicalize 成绝对路径 → 版本探针 → 注册
    Driver"的本地劫持链。
  - 事件流 `Error{retryable}` 后：后端 pump 对存储错误做 3 次退避重试；前端对 retryable 错误自动以
    `lastSequence` 重订阅（指数退避，去重靠 cursor），订阅退出时清理 cancel 表项。
- 2026-09-03 多 Agent ACP 接入：
  - 新 crate `acp-driver`：官方 SDK stable v1（default-features 关闭）实现
    `AgentDriver`——initialize 恰好一次/连接、session/new、capability-gated `session/load` resume、
    有序 ContextDelivery→`session/prompt`、`session/update` 流式映射（AgentMessageChunk→TextDelta、
    ToolCall/Update→ToolStarted/Completed、Plan 等→VendorExtension）、`session/cancel` interrupt、
    权限请求一律 `Cancelled` 安全拒绝、审批/错误文本收敛为稳定 failureKind（复用 #1 修复的不变量）。
    env_clear + 6 项无密 allowlist；子进程 kill_on_drop + stderr 排空不保留。
  - `agent-registry` 扩展（带测试）：`identity_args`（版本输出为裸 semver 时的第二
    身份探针，marker 对合并输出校验）、`user_paths`（`~` 相对发现路径，禁 `..`/绝对）、
    `requires_executables`（生态 adapter 依赖声明）、`launch_args`（ACP 启动模式）。
  - 内置 manifest 扩至 5 个：kimi.toml / grok.toml / zcode.toml 新增。桌面扫描按 `protocol.kind`
    分派：app_server_json_rpc→Codex driver、acp→直接探针注册、claude_stream_json→Claude 原生
    driver、zcode_app_server→仅发现。
  - 本机真实事实：Kimi Code 0.40.1（`kimi acp`，loadSession=true）、Grok Build 1.0.13
    （`grok agent stdio`，loadSession=true）、Claude Code 2.1.218 + claude-agent-acp 0.73.0
    （显式 npm -g 安装，操作者要求接入）——三者 initialize 探针全部通过
    （`cargo test -p acp-driver -- --ignored`）。兼容记录见
    `docs/compatibility/kimi-code.md`、`grok-build.md`、`claude-code.md`。

## 正在修改

- M1-S4 下一步优先真实 Codex Turn/第二 Turn/真实 cancel 与 PendingInteraction；Core/UI Cancel
  已接线，实现和严格限制见 `docs/implementation/m1-s4-codex-driver.md`。旧 M0 report/生产
  Recorder 暂后移，不以诊断设施替代真实 Agent 接入。
- M0-ACP 真实 Cursor runtime 仍缺失；Codex turn/interrupt 仍等待用户明确授权模型额度，两者不阻塞确定性协作层开发。

## M0-CODEX 已取得证据

- 两份本机 Codex 均支持 `app-server generate-json-schema --out DIR` 和 `--experimental`。
- stable schema canonical SHA 已固化；`0.150.0-alpha.12.2` 与 `0.144.5` 明确不相同。
- 已识别 raw JSON schema key order 可能不稳定，兼容判断改用 canonical JSON SHA-256。
- 两版本均完成 stable initialize response + initialized notification；输出已裁剪 `codexHome` 值。
- 两版本均完成 ephemeral、read-only `thread/start`，返回 idle 且无持久化 thread path；未记录 thread id 值。
- 两版本对仍在同连接中加载的 ephemeral thread 立即 `thread/resume` 均返回 `-32600`；只保留错误码，并记录响应前 interleaved `thread/started`/MCP startup 方法。
- 隔离临时 `CODEX_HOME` 的两进程 resume 探针已完成：`0.144.5` 在固定 name update 后成功恢复零 Turn thread；`0.150.0-alpha.12.2` 在一秒内未创建 rollout，第二进程为 `-32600 / not_found`。全程未启动模型 Turn。
- 隔离实验工具仍位于 `experiments/codex-app-server`；其最小事实已用于约束桌面 Beta Driver，
  但 M0 探针本身不作为 Product Driver。本轮尚未产生模型 Turn。
- M0 有界 JSONL transport 已实现：双向单帧上限、请求前 interleaving 上限、string/number request id、stderr drain+truncate。
- Codex frame 已覆盖最小 loss-aware 归一化：Turn、文本 delta、Diff、Usage、交互请求、warning 和 VendorExtension。
- `turn-smoke` 已实现固定 no-tools prompt、两分钟期限和显式 `--allow-model-usage` 门；输出不含回复正文、thread/turn/item id。
- `interrupt-smoke` 已实现同等级额度门：`turn/start` 响应后立即发送 `turn/interrupt`，聚合 interrupt response、interleaving、文本字节数与最终状态；尚未真实执行。

## M0-ACP 已取得证据

- 官方 `agent-client-protocol 2.0.0` 以 `default-features = false` 固定，使用 `Client.builder()` stable-v1 API；Cargo.lock 解析 schema package 1.5.0。
- 内存 fake Agent 已验证 V1 initialize/capabilities、session/new、session/prompt、session/update、session/cancel。
- permission request 的 option id 完整保留；测试客户端固定返回 `Cancelled`，没有自动批准路径。
- 官方 SDK typed stable-v1 enum 会把 `_vendor/update` 解析成 `vendor/update`；`PreservedExtension` 在 typed dispatch 前保留 raw method/params，行为已由测试和脱敏 fixture 固化。
- 本机只有 Cursor IDE wrapper；`cursor agent ...` 会尝试联网安装，因此禁止用于后台版本探针。PATH 中通用 `agent` 实际是 Grok Build，不能登记为 Cursor。
- 兼容性记录位于 `docs/compatibility/cursor-acp.md`，所有当前 ACP 验证均未使用模型、凭据或用户工作区。

## 尚未实现

- Kimi/Grok/Claude（ACP）与 Codex 的真实模型 Turn、PendingInteraction 完整持久化/响应链路、
  真实 cancel 验收（全部等待用户明确授权模型额度）。
- Copilot Rust SDK Product Driver。ZCode app-server Driver 已接入 Beta（协议破解 + 免 Node
  依赖启动 + 零模型 probe；真实 Turn 等操作者 `zcode login` 后验收，见
  `docs/compatibility/zcode.md`）。
- ACP/Copilot Product Driver 的 Handoff/Reconciliation 真实发送验证；Claude/Kimi/Grok 的
  QuotaProvider、Recovery Center。
- 自动运行测试的受控采集器；当前已有 Test Artifact 接收/持久化契约，但 Core 不自行执行 package script 或测试命令。
- FTS5、通用 Artifact GC、生产 Raw Frame Recorder（结构化策略、离线副本导出与二次检查已完成；分片/压缩/保留和 CI 接线未完成）。
- Tauri 主窗口单实例、托盘生命周期和岛窗口。
- Rust→TypeScript 自动生成；当前 TS Wire 类型是受测试约束的手工镜像。

## 最近验证

当前实现基线的以下命令通过：

```text
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace                         # 158 passed, 29 ignored
pnpm check                                     # TS 26 passed + production build
cargo test -p acp-driver -- --ignored --nocapture
                                               # 真实 Turn 验收：kimi/grok 三轮全过（grok 需已登录）
cargo test -p claude-driver -- --ignored --nocapture
                                               # Claude stream-json 控制面探针 + 真实 Turn 三轮全过
cargo test -p codex-driver -- --ignored --nocapture
                                               # Codex 真实 Turn 三轮全过
cargo test -p claude-driver -- --ignored --nocapture
                                               # 本机真实 Claude stream-json 控制面探针通过
```

2026-09-03 的 `pnpm desktop:dev` 启动冒烟通过：Vite 1420 与 Tauri host 均启动，真实 app data
数据库只读检查返回 `user_version = 2`、`quick_check = ok`，task_artifacts 表存在。当前 Codex
`0.153.0-alpha.5` 的真实 initialize/account readiness 在 32 ms 内通过并进入 Ready；Claude Code
2.1.218 在 55 ms 内进入 Installed；未发任何 Agent Turn。
Browser 实测通过空列表、主动选择 Context Artifact、完整 hash、JSON 格式化及确认前后导出
按钮门槛；新增 Diff 浏览器夹具通过文件/hunk/行号、rename/binary、非 UTF-8/无效降级、
60112 B 分段/10000 行截断和延迟响应隔离。浏览器测试不构成 Tauri IPC/原生保存验收。
Computer Use 未识别未打包的开发
进程，因此原生保存/取消交互仍待验收。这是 2026-09-03 的历史环境记录，不表示目前存在可复用的开发进程。

四条 Product Driver 的真实 Turn 验收已由操作者授权完成；本轮对话首页改造没有重新运行 ignored
验收，也没有额外消费模型额度。Codex 0.153 的零 Turn resume-smoke 仍为 `-32600/not_found`。
`pnpm check` 仍提示既有 `pnpm.onlyBuiltDependencies` 配置位置弃用，不影响本轮验证通过。

2026-09-04 M1-S6 回归：`cargo fmt --all -- --check`、workspace clippy `-D warnings`、
`cargo test --workspace`（136 passed / 15 ignored）及 `pnpm check`（30 tests + production build）通过。
浏览器 Demo 已实际检查独立额度页、5 小时/周剩余进度条、Composer 圆环和 hover Popover；
`pnpm desktop:dev` 启动成功，桌面进程确实创建了专用 Codex App Server 控制面连接。另以
`initialize → account/rateLimits/read` 验证本机 0.153.0 返回账户级 300/10080 分钟窗口，全程未
创建 thread/turn、未消耗模型额度。

## 已知限制

- `Installed` 只表示 executable + version probe 成功，不等于认证完成或 `Ready`。
- `NotConfigured` 已有稳定 wire/UI 表达；`--version` 不推断认证，Codex 由独立无凭据
  initialize/account-readiness 产生 Ready/NotConfigured，其他 Provider 仍需各自事实探针。
- fingerprint 缓存只覆盖版本探针；未来认证/capability 状态需独立 TTL 与失效策略。
- 磁盘路径消失表示 Installation 不再存在，因此从当前 Projection 移除；`NotConfigured` 专指 executable 存在但 Agent 配置/认证未就绪。
- Codex manifest 已进入 `v2_minimal_beta`，真实三轮 Turn 已验收，但不能据此宣称 Production；
  未验证的跨进程 resume/approval 能力仍不得提升为 Supported。
- 四条 Product Driver 的 streaming、同会话复用和 interrupt 已 runtime verified；Codex Usage
  监控已扩展 Claude/Kimi/Grok；Claude 依赖推送，Kimi/Grok 上下文只观察 UsageUpdate，
  无观察不主动发送 Prompt，缺失数据保持 Unknown。
- ACP Product Driver 已在 Kimi/Grok runtime 验收；Cursor 仍没有 runtime 事实，不得据此宣称 Cursor 可执行。
- ACP SDK stable-v1 的未知扩展前导 `_` 丢失必须经 raw envelope 规避，进入 Product Driver 前仍需真实 runtime 回归。
- 当前编辑器开放 objective、completed work 和 pending work；其余确定性字段仍只读。每次“应用编辑”会重新脱敏/hash，但本切片在最终确认时持久化 Hub 原稿与最终版本，不保存未确认的中间草稿。
- 显式 Reconciliation opt-out 已可审计和重试；Recovery Center 尚未提供对悬空 opt-out 的人工恢复入口。
- Git facts、Snapshot、按需 Diff Artifact、受限查看/导出与只读结构化 Diff 已完成；原生保存/取消 UI 尚待验收。当前查看器不等于完整 ReviewRun/Findings、Checkpoint 或 Adoption；binary payload 不解码/应用，Windows 未实测。
- Test Artifact 已创建并校验，但尚无受控测试 Runner/外部 Product Driver 采集器；`SystemRecovery` 调用者必须提供已有结果，Core 不执行传入的测试命令。
- Git 集成场景已覆盖当前读侧可表达的主要路径与状态，显式 Snapshot 元数据已进入 Canonical Event；目标漂移合并、候选历史、dirty submodule、Freeze/Adoption 崩溃仍属于后续 Checkpoint/Adoption/Recovery 工作包。
- 当前 Patch 是 HEAD→所选工作目录最终内容的净差异，不是 staged/unstaged 双层 Checkpoint；同一文件先暂存再在工作树还原 HEAD 时，最终 Patch 可以为空。大文件/符号链接目前不计算正文 hash，不能将 status fingerprint 宣称为完整 tree hash。
- Git 防护已覆盖受测 clean/process、include、fsmonitor、hook 和 textconv；并发修改 `.git/config`、子模块自有配置、用户全局 Git 转换语义及 Windows 路径/权限仍需专门验证，不能把本切片当作执行恶意仓库的沙箱保证。
- Fake Driver 证明 Port 语义，不代表 Codex/Cursor 等外部产品已支持 resume 或 ContextDelivery；各 Product Driver 必须通过自己的 M0 事实证据声明 capability。
- Redaction v2 高熵检测是候选启发式：正文 hash/长标识可能误报，短值/低熵/其他编码可能漏报。新增结构化 allowlist 已覆盖转义字段名、非字符串 secret 与未知嵌套字段；目前仅供离线诊断副本，不启用生产 Raw Recorder。Codex 策略不放行自由文本，旧 M0 summary report/历史 fixture 尚未迁入新策略，不能宣称全部诊断出口已统一保护。Canonical 用户输入仍不擅自改写，原始 Patch ruleset 0。
# 上下文交接最新增量（2026-09-09）

非 Git 有界采集、Git/非 Git 起点持久化、宿主独占锁下的启动恢复，以及真实 Codex→Kimi→Codex 对话接力已落地。容量、恢复归因及验收范围见 [实现记录](implementation/m1-s5-turn-changes.md)。


## 2026-09-11：灵动岛首版

已启动 M1.5 灵动岛交付：独立顶部浮窗、CompanionProjection 聚合、审批入口与主窗口定位、开启偏好、界面缩放。通过前端/Store/窗口定位测试、生产构建和隔离原生 IPC 冒烟。尚不标记 M1.5 全项完成：刘海融合、完整多屏/全屏适配与 Delegation 等细分投影仍待后续。详见 [调研与实现](implementation/island.md)。
