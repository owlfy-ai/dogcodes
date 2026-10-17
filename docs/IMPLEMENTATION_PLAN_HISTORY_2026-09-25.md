> 历史归档（截至 2026-09-25）。以下保留整理前全文，其中“当前”“下一步”“尚未实现”等描述只代表当时阶段，不能作为现行待办或实施约束。现行范围、状态和顺序以 [当前工作清单](CURRENT_WORK.md) 为准。

# dogcodes 实施计划

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

- 同一项目允许不同 Agent，或同一 Agent 的多个独立对话，同时修改共享目录代码；不实施项目级单写者门禁，不强制普通对话排队或创建 worktree。单个对话的活动轮次及审批/取消隔离仍保留。
- Git 提交备注“AI 总结”的独立执行留痕退出需求和 M1 收尾清单；不新增该功能的 ExecutionRun、输入/输出 Artifact 或审计记录。
- 稳定性工作包已完成服务锁释放修复、格式和严格 Clippy 检查；全工作区回归与前端检查通过，见 [修复与验证记录](implementation/m1-stability-2026-09-19.md)。并发验收覆盖不同 Agent、同一 Agent 多对话，以及 Session、消息、审批和取消的隔离，不以“没有项目锁”代替验收通过。
- Copilot SDK（或正式替代 SDK 切片）暂缓；只读委派以已完成轮次的一键审查为首批交付入口，剩余 M1 验收继续保留。

> 2026-09-12：已移除异常断连后的人工状态确认门槛。旧本地轮次自动关闭并释放占用，用户可直接继续；结果未知仍显示连接中断，不伪造完成。94 项 Rust、19 项前端专项及 Kimi 真正服务崩溃后的 GUI 直接继续验收通过，见 [状态一致性](implementation/execution-state-consistency.md)。

> 2026-09-12：已接通独立 Core 后台服务，默认桌面改走本地 IPC；服务持有 Driver、数据库锁和历史 MCP。桌面 Rust 31 项（含 6 项服务回归）、Codex/Kimi RPC 客户端硬杀后的真实完成验收、Kimi 桌面/灵动岛两次硬杀恢复、前端 198 项与类型检查已通过，见 [实施记录](implementation/core-daemon.md)。服务异常与桌面退出分开处理，不宣称八家 GUI 全矩阵或硬件链路通过。


> 数据库存储策略（2026-09-11）：项目尚未发布，不支持旧数据迁移或数据库升级。现已删除版本分支、补列、启动回填及旧宿主失败事件兼容。下文迁移记录仅为历史，不是当前要求。实现与验证见 [数据库策略](implementation/database-initialization.md)。

## 当前状态与下一步（2026-09-12）

本节是当前工作入口；后文较早批次的测试数字和失败记录仅说明当时结果，不作为当前待办。Driver 验收、原生 GUI 验收与发布版验收分别计数，不相互替代。

### 已完成

- [x] 历史增强验证：Rust 67 项、前端 198 项、类型检查、前端构建和桌面 cargo check 通过；覆盖超过 16 MiB 的 SQLite 历史、分块确认、投影重建和取消。严格 Clippy 有既有依赖警告；八家真实 Agent/GUI 本次未重验，见 [本次验证记录](implementation/history-query.md)。

- [x] 五个历史查询工具改为 stdio MCP，八家 Driver 带外注入、按任务隔离和按轮次撤销；用户消息不再拼接历史 CLI 说明。见 [历史查询](implementation/history-query.md)。

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


## Claude 订阅额度（2026-09-11）

- [x] 独立 OAuth usage 查询、60 秒缓存、系统代理、推送补充。
- [x] 白名单诊断及监控面板/圆环提示，不再只显示未知。
- [x] 本机授权类型核实：当前是自定义 API 服务，无默认官方 OAuth 凭证。
- [ ] 使用有效官方订阅 OAuth 验证真实五小时/周额度；当前本机配置不具备此条件。自定义服务余额需另行确认该服务 API，不属于官方额度接口。

## 额度加载优化（2026-09-11）

- [x] 各 provider 并发初始化，注册完成通知额度刷新。
- [x] 按安装独立请求、独立展示，保留 30 秒轮询和原有缓存。
- [x] 请求去重、会话切换响应隔离及回归测试。
- [ ] 真实桌面冷启动观察：快 Agent 不受慢 Agent 影响，后台陆续注册的 Agent 无需等待下次 30 秒轮询。

## Review / Diff

查看核心及普通暂存、丢弃/撤销已完成，进入实际使用反馈阶段。UI 打磨与 Windows/Linux 实机验收仍待安排。

范围以 [当前实现](implementation/review-panel.md) 为准，验证以 [交付审计](implementation/review-delivery-audit.md) 为准。高级文件系统恢复及其历次验收待办已移除。

## 当前：多目录项目

- [x] 创建项目弹窗与多目录数据链路。
- [x] 原生多目录传参与目录提示兼容模式。
- [x] 自动化回归及浏览器 Demo 两目录项目创建。
- [x] Codex/Kimi 真实跨目录写入、单次审批及 A→B→A 文件交接；双目录强制退出恢复自动化通过。
- [ ] 其他供应商取消/恢复矩阵及原生桌面验收，详见 [实施记录](./implementation/multi-root-projects.md)。

更新日期：2026-09-09

- [x] Antigravity 聊天框及 Agent 菜单圆环固定展示 Gemini Models 5 小时剩余额度。

- [x] 所有 Coding Agent CLI 统一应用代理环境：继承大小写代理变量，缺失项通过 macOS SystemConfiguration、Windows WinINet/WinHTTP/注册表、Linux GNOME/KDE/portal 原生配置源补齐，不执行系统命令；覆盖会话/额度/模型目录。macOS 原生读取及 Grok Ready + billing 真实只读链路分别完成验证。
- [x] 额度 stale 快照静默保留：移除周期出现的“数据待刷新”文案和圆环透明度变化，采集时间仍可判断读数新旧。
- [x] 额度总览使用 Agent 版本替代内部 Installation ID；Agent 下拉菜单移除版本文字，并为每个选项展示对应额度圆环。
- [x] Antigravity 图标替换为用户提供的 SVG，并验证开发/生产构建资源输出。
- [x] Claude Code 图标替换为用户提供的路径 SVG，并复用现有小尺寸图标布局。
- [x] ZCode 图标替换为用户提供的三段路径 SVG，并适配浅色黑色/深色白色主题。
- [x] Cursor 图标替换为用户提供的六边形路径 SVG，并复用深色主题布局。
- [x] Grok Build 图标替换为用户提供的图形 SVG，并适配浅色黑色/深色白色主题。
- [x] Kimi 图标从官网公开品牌资源提取并固化为本地 PNG，替换原 `◐` 占位字符并验证构建产物。
- [x] ChatGPT/Codex 图标从官方 sprite `#55180d` 提取为本地 SVG，替换原 `◎` 占位字符并适配浅色黑色/深色白色主题。
- [x] Agent 图标统一尺寸并设置品牌图标 2px 内边距，small 20px、medium 26px，保持 SVG/PNG 比例填充。
- [x] 品牌图标统一恢复 2px 视觉留白；Cursor 纳入主题自适应 Logo 渲染，浅色黑色、深色白色。
- [x] Agent 选择器隐藏内部 Fake，并对 Claude Code、Grok 等 CLI 多路径发现结果按可用状态/版本统一去重；Codex 桌面与 CLI Installation 继续独立呈现。

### 本轮验收更新

- [x] Antigravity 显式 Gemini/Claude 双模型真实短 turn：57.20 秒通过；修复 stdin 未实际关闭造成模型间交接挂起。
- [x] 真实桌面核心交互：System Events 权限解阻；原生模型目录、Gemini/Claude 圆环切换、详情、提交锁定、重启恢复并续聊均通过。
- [x] 时间线空白修复：重复点击同 task 时不再清空；请求代次隔离旧响应，加载失败可重试；44 项前端测试、真实桌面重复点击/快速切换各 5 次通过。
- [x] 确定性额度异常矩阵：耗尽/reset/TTL/未知/非法数值/模型组/展示 scope；修复取整误封禁与过 reset 旧零值，补本地时间边界更新。
- [ ] 桌面 hover/布局验收；真实耗尽账户和真实网络故障注入本轮未覆盖。

## 额度来源落地（进行中）

- [x] Antigravity 原生 /usage Provider 与桌面注册；分组窗口展示，未知模型不误封禁。
- [x] 本地 workspace check、Antigravity clippy、前端 38 tests/typecheck。
- [x] 新 Provider 真实只读重验（2026-09-08 通过，含缓存一致性）。
- [x] 当前模型与额度组可靠关联：原生目录、SubmitTurn.model、--model、会话持久化及圆环映射；真实双模型及桌面核心验收通过。
- [x] Cursor macOS 原生 /usage 自动采集及真实验收；整数读数仅展示，原生月日保留文本，不伪造时间戳。
- [x] ZCode 用户明确授权复用原有凭证后，当前 CLI provider 凭证 + 官方额度 GET 实装；BigModel 真实验收通过。
- [ ] Z.ai 账号真实验收（当前本机未登录）。

## 当前推进：Codex 桌面发现与额度展示回归修复

2026-09-08 已修复两条交付回归：Codex 仍是单一 Provider，桌面 App 内置 Runtime 与独立 CLI
作为不同 Installation 探测并按 canonical path 去重；macOS 系统/用户 Applications 路径不再依赖
启动终端的 `PATH`。安装差集只有在已记录的绝对 executable 确实消失时才发
`agent.installation_removed`，避免环境差异误删 Ready 安装。本机已重新发现并注册
`/Applications/ChatGPT.app/Contents/Resources/codex` 0.153.4 为 Ready。

同时修复 `QuotaSnapshotView` Rust wire 缺失 camelCase，前端此前使用 `providerId + installationId`
无法匹配任何额度快照，导致切换全部 Agent 都显示“暂无订阅额度读数”。新增序列化契约、桌面差集
和 manifest 回归测试。

## 上一推进：额度耗尽状态闭环

2026-09-08 已完成结构化 `TurnQuotaExhausted`、安全 Canonical `quota_exhausted`、可见时间线提示、
额度圆环/总览“已用尽”和新鲜供应商读数发送门禁。ZCode 1308 已真实验证；其他 Driver 使用已验证
code 或保守的明确耗尽措辞，普通 429/瞬时 rate limit 不误判。实现与边界见
[`m1-s6-quota-monitoring.md`](./implementation/m1-s6-quota-monitoring.md)。

## 上一推进：Beta Agent 事实收敛与重复验收

2026-09-07 已完成：① 修正 ZCode context/临时会话/真实终态；② 用
[`beta-agent-acceptance.md`](./implementation/beta-agent-acceptance.md) 统一所有 Beta Agent 的
验收口径和状态；③ 重跑真实 Stream/Cancel/Approval。七条 Driver 的本轮真实 stream/cancel
均通过；Codex、Claude、Kimi、Grok、Cursor 的 deny/allow 通过。Antigravity 的 headless 通道
当时未接审批 Hook，旧 Unsupported 结论已被后续真实复验取代；ZCode allow 落盘和 resume 回忆也已通过，见当前状态。

## 上一工作包：M1-S7 诊断与启动保护

恢复与诊断页、只读状态快照、原生诊断摘要导出已接通；停止操作增加本次 Core 的 Run
归属校验。启动数据库完整性/版本检查和只读恢复说明入口已接通，失败时不启动 Core/Agent。
启动故障诊断导出、会话重建与审批失效协调已实现；代表性 GUI 已验收，剩余供应商与异常边界见当前清单。
范围及验收见 [M1-S7](./implementation/m1-s7-recovery.md)。
当前真实 Driver 共七条：Codex、Claude、Kimi、Grok、Cursor、Antigravity、ZCode；下方早期
“四条 Driver”章节是阶段记录。Claude/Kimi/Grok/Cursor/Antigravity 跨进程恢复已有真实
验收记录；Codex 0.153.4 跨进程恢复和 deny/allow 审批也已取得真实证据。本轮统一命令复验
已经完成，Codex 审批已接通，不再是待实现项。

本文把需求基线和详细设计转换为可执行工作包。状态事实以
[`IMPLEMENTATION_STATUS.md`](./IMPLEMENTATION_STATUS.md) 为准，交接入口以
[`HANDOFF.md`](./HANDOFF.md) 为准。

## 当前上下文交接工作（2026-09-09）

自动交接取代下文历史记录中的 Handoff Preview/人工确认流程；发送前冻结原始问答并交付，见 [自动交接](./implementation/automatic-context-handoff.md)。

M1-S5B 已接入逐轮私有 Git tree 比较、变更文件版本与净 patch Artifact、上下文注入及结束时序保护，见 [Turn ChangeSet](./implementation/m1-s5-turn-changes.md)。尚不是完整备份或可恢复 Checkpoint。历史浏览/全文检索入口与真实 Codex/Kimi 跨目录文件 A→B→A 已通过；后续为逐事实交付覆盖及其他供应商取消/恢复矩阵。

## 实施原则

- 按可运行纵向切片交付，不提前创建无实现的硬件、daemon 或 CLI crate。
- `agent-domain`、`agent-protocol`、`agent-core` 不依赖 Tauri。
- Canonical Event 是事实源；SQLite Projection 可删除并重建。
- Agent 认证和权限保持外部自治，不增加 API Key 或凭据存储入口。
- 每个工作包完成时同时更新状态、验证结果、已知限制和下一接手点。

## 工作包

| ID | 工作包 | 主要产物 | 验收门槛 | 状态 |
|---|---|---|---|---|
| M1-S1 | Project/Task/Fake Driver 纵向切片 | Domain、Protocol、Core、SQLite、Tauri Host、三栏 UI | 重启恢复、流式时间线、CompanionProjection、测试通过 | 已完成 |
| M1-S2 | 有序 UI 事件流 | Tauri Channel、replay/live 去重、sequence 缺口补偿、前端 cursor | 不轮询；重载从 sequence 恢复；Lagged 可补齐 | 已完成 |
| M0-CODEX | Codex App Server 事实探针 | `experiments/codex-app-server`、脱敏 fixture、兼容记录 | handshake、thread/start、turn/start、stream、approval、cancel 实测 | 已完成；0.153.4 本轮复验通过 |
| M0-ACP | ACP stable v1 事实探针 | `experiments/acp`、fixture、能力矩阵 | initialize、session、prompt、cancel、approval、未知扩展 | 已完成；Kimi/Grok/Cursor 本轮复验通过 |
| M0-ZCODE | ZCode app-server 事实探针 | 自带 Electron 启动、订阅/终态、临时会话清理、兼容记录 | 零模型生命周期 + 真实 stream/cancel/context | 已完成；0.16.5 本轮复验通过 |
| M1-S3 | Agent Registry、安装探测与差集协调 | manifest schema、结构化 argv、版本/状态探针、缺失安装事件 | 多安装去重、超时/输出上限、无凭据读取、删除后可重建 | 已完成 |
| M1-S4 | Beta Product Drivers | Driver、Session Map、审批、取消、Diff | 按统一矩阵完成真实闭环 | Beta 已接入；2026-09-07 重验完成，能力缺口如实降级 |
| M1-S5 | Handoff/Reconciliation | 确定性 Context Package、hash、预览 UI | A→B→A 连续时间线且可追溯 | 进行中 |
| M1-S6 | Quota 监控 | Codex/Claude/Kimi/Grok、Unknown/Stale UI | 只读、有界、session 隔离、真实来源 | 已交付（供应商缺失字段边界见 M1-S6） |
| M1-S7 | Recovery/Diagnostics | Recovery Center、脱敏诊断包 | 中断状态可解释且不误杀外部进程 | 进行中：诊断、归属保护及存储启动门禁 |
| M1-S8 | 对话优先主界面 | 两栏对话工作台、scratch 会话、Ready Agent 默认选择 | 无项目可立即对话；保留精确 Installation；旧 Task 不被静默改写 | 已完成 |

## 最近完成工作包 M1-S8

启动页已从强制项目引导改为对话优先工作台。Desktop Host 在没有 Task 时创建应用管理的 scratch
工作区，并优先选择已探测 Ready 的具体 Agent 安装；项目连接保留为可选动作。实现、边界和接手点见
[`m1-s8-conversation-home.md`](./implementation/m1-s8-conversation-home.md)。
Codex 实际 Turn 暴露的兼容性诊断刷屏已在 Store Projection 边界修复：诊断事件继续留存，
但新旧数据库的 Timeline 均不向聊天 UI 返回这些内部记录。

## 已完成工作包 M1-S4：四条真实 Product Driver

1. `codex-driver` 按具体 Installation 运行有界 App Server JSONL，完成
   initialize/account readiness、Thread create/resume、Context+Prompt、stream、工具终态和
   interrupt Port；Core/Tauri/React 已接 `CancelTurn` 与停止按钮。（已完成）
2. Core Driver registry 与 UI 路由改为完整 `ExecutorRef`，多版本不会按 Provider 覆盖；只有
   account readiness 通过的安装显示 Ready。（已完成）
3. Claude 2.1.218、Kimi 0.40.1、Codex 0.153.0-alpha.5、Grok 1.0.13 已在操作者授权后完成
   真实 streaming、同会话第二 Turn 和 interrupt 验收。（已完成）
4. ZCode app-server M0 的 stream/cancel/context/cleanup、PendingInteraction 审批透传和
   跨进程 resume 控制面均已完成；剩余门槛仅是供应商额度恢复后的 allow 落盘与上下文回忆
   两个模型闭环复验。详见
   [`m1-s4-codex-driver.md`](./implementation/m1-s4-codex-driver.md)。

当前回归结果以 HANDOFF 置顶交付记录为准。

## 工作包 M1-S6：订阅额度监控

Codex 已通过独立 QuotaProvider 接入 App Server `account/rateLimits/read`：账户级 5 小时/周
窗口、重置时间、按当前 thread 隔离的上下文占用与缓存命中率均已进入 Composer Popover；侧栏
“额度监控”打开独立总览页。Claude 推送、Kimi loopback、Grok billing 已接入；缺失字段保持
Unknown，不按套餐推算。2026-09-06 收尾修复包含超时/缓存/进程回收、session 隔离和 stale 展示。
实现边界、事实验证和下一 Provider 接手点见
[`m1-s6-quota-monitoring.md`](./implementation/m1-s6-quota-monitoring.md)。

## 实际 Agent 接入队列

1. Codex：App Server Product Driver Beta；0.153.4 的 stream/live reuse/cancel/deny/allow 本轮通过。
2. Claude Code：原生 headless stream-json Product Driver Beta，零额外 adapter；2.1.218 的
   stream/live reuse/cancel/deny/allow 本轮通过。
3. Kimi/Grok/Cursor：ACP stable-v1 Product Driver Beta；三者 stream/live reuse/cancel/deny/allow
   本轮通过。Grok 强制 `--permission-mode default`，避免静默执行写工具。
4. Antigravity：原生 stream-json Product Driver Beta；stream/live reuse/cancel 通过，真实写场景
   未产生审批且没有落盘，审批能力明确为 Unsupported。
5. ZCode：app-server Driver 已接桌面；subscription/event/stop/context/cleanup 代码已校准，
   零模型 lifecycle、真实 stream/context/cancel 通过；PendingInteraction 与跨进程 resume 控制面
   已接通。供应商当前返回 1308，allow 落盘和 resume 回忆闭环待恢复后重跑，capability 保持 Unknown。

统一门槛、命令和当日结果只在
[`beta-agent-acceptance.md`](./implementation/beta-agent-acceptance.md) 更新。

## 已完成协作层工作包 M1-S5

1. 定义 transport-neutral `ContextPackageV1`、Handoff/Reconciliation kind、三档强度、
   provenance、ContextPointer、结构化语义摘要来源与 `BuiltContextPackage`。（已完成）
2. 从冻结的 `(afterSequence, throughSequence]` Task Canonical Event 区间确定性构建预览；
   同一事实区间产生相同 SHA-256，最多读取 10,000 条事件。（已完成）
3. SQLite 只读取目标 Task/区间，其他 Task 或更晚事件不进入 package；未知事实保留
   `unknown`/空数组，不调用隐藏模型。（已完成）
4. Tauri/TS `buildContextPreview` 已接入；切换下一轮 Agent 时先显示强度、来源区间、
   payload hash 和数据最小化说明。（已完成）
5. `redaction` 已实现规则集 v1，并在 Context Package 初建及编辑重哈希的
   强制路径上先脱敏再计算 payload hash；provenance 记录规则版本与累计命中数。（已完成）
6. 确认预览时由 Core 按冻结参数重建并校验 hash；package 作为 `art_<ULID>` 不可变
   Artifact，payload blob 按 SHA-256 内容寻址，与 `handoff.package_confirmed`、preferred
   executor 变更在同一 SQLite 事务提交。
   错误 hash 零写入，命令重试返回原 Ack。（已完成）
7. Task×Agent SessionMap 与 capability-gated resume 已实现：`session.ready/resumed/resumable`
   记录 native session/turn，terminal event 推进 `lastSeenSequence`，Projection 可重建；未知
   能力仍保持 Unknown，只有明确 Supported 才恢复。（已完成）
8. 已确认 Artifact 进入 `context_transfers` Projection；Driver `PromptRequest` 约束按序先交付
   ContextDelivery 再交付用户 prompt，并记录 `handoff/reconciliation.sent|failed`。Core 会阻止
   绕过确认的跨 Agent prompt；失败 Artifact 可安全重试。（已完成）
9. React 根据目标 SessionMap 自动选择 Handoff 或 Reconciliation；后者从目标
   `lastSeenSequence` 起只构建增量。Fake A→B→A 已验证恢复同一 A Session、对账先于 prompt、
   payload hash 与已确认 Artifact 一致，且 Projection rebuild 一致。（已完成）
10. Context Package 编辑已接入 Core/Tauri/React：不可变路由与 provenance 由 Core 校验，编辑
    草稿重新脱敏/hash；确认事务同时保存 Hub 原稿、最终 Artifact 和 `artifact_versions`
    parent 关系，UI 显示原稿→最终版本字段差异。（已完成）
11. 显式 Reconciliation opt-out 已接入命令、Canonical Event、Projection 和 UI；弹窗取消/X
    仍不产生 opt-out。opt-out 恢复旧 Session 后跳过 ContextDelivery，成功接受 prompt 前写
    `opt_out_applied`，拒绝时写 failed 并可重试。（已完成）
12. 新增可注入 `WorkspaceContextProvider` 与 `agent-workspace`：桌面使用绝对 Git
    executable、结构化 argv、3 秒/256 KiB/1000 路径上限，只读取 porcelain status、diff stat
    和 HEAD；≤1 MiB 普通变更文件记录内容 hash，依赖清单单独进入 package。工作树漂移会使
    确认重建 hash 变化并零写入拒绝。（Git/dependency 首切片已完成）
13. `RecordTestResult` 已作为仅限 `SystemRecovery` 的采集适配器契约：Core 先脱敏再生成
    `application/vnd.dogcodes.test-report.v1+json` Artifact，并与 `test.completed` 引用原子
    提交；报告正文不进入 Event，Context Package 只消费 Artifact 引用。（已完成）
14. Git 读侧矩阵已补 staged+unstaged 同文件、rename/delete、binary、symlink、Unicode 与超过
    1000 路径显式降级；resume failure 已映射为 missing/incompatible/detached，失败 Event 和
    `turn.failed` 只保存稳定原因码，Projection rebuild 保真。（已完成）
15. Git 读侧又补 clean、仅 staged/unstaged、ignored 默认排除、executable bit、空仓库、
    detached HEAD、linked worktree 与含空格/换行路径的真实仓库测试。（已完成）
16. `GitWorkspaceSnapshotV1` 已显式记录 repo root、HEAD、branch/detached、哈希化 worktree
    identity、porcelain v2 与 status fingerprint；Core 在每个已排队写入 Turn 起止追加
    `workspace.snapshot_captured|failed`，失败只记录稳定诊断且不阻断执行。（已完成）
17. `workspace_snapshots` Projection 已保存成功/失败摘要，支持按 Task 最新优先、1–100 条有界
    查询；Core、Tauri 与 TS Client 已接通，rebuild 后结果一致且不膨胀 Bootstrap。（已完成）
18. 本机用户可显式 `CaptureWorkspaceDiff`：私有临时 index/object store 从 HEAD/空树初始化后
    `add -A`，生成 binary/full-index Patch；私有 index/object store 和 `GIT_OPTIONAL_LOCKS=0`
    共同避免修改真实 index/object store。≤1 MiB Patch
    作为不可变 Artifact 原子提交，漂移/超限零写入，只有 fingerprint 一致时 Context 才引用；
    UI 已提供带敏感内容提示的固化按钮。（已完成）
19. 在开放 Artifact 查看前收紧基础边界：旧库原子迁移/Projection 回填、冻结 sequence 内的
    Diff 引用、Git 外部程序防护和原始 Patch 字节保真；补充失败零副作用回归、子目录范围一致性
    与 Demo 不伪报成功。（已完成；Rust 80 passed + 1 ignored，TS 12 passed）
20. 受限 Artifact 查看/导出：v2 Task 归属 Projection/原子迁移、复合 cursor 分页、hash 绑定的
    32 KiB 分段读取、只读文本/JSON/非 UTF-8 预览、主窗口限定与原始字节不覆盖导出。
    （代码与自动回归已完成；Rust 85 passed + 1 ignored，TS 17 passed；原生保存/取消 UI 待验收）
21. 已有受限读取接口上实现只读结构化 Diff：文件/hunk/原新行号、二进制/模式/rename 说明、
    无效 Patch 降级与展示预算；真实 Git 输出回归和独立浏览器测试入口，不增加应用补丁。
    （已完成；Rust 85 passed + 1 ignored，TS 25 passed；不是 M2 的 ReviewRun/Findings 系统）
22. Redaction v2：quoted JSON secret、Basic/URL userinfo/PEM、高熵候选整数阈值、重复处理
    不重复计数；冻结历史引用结构校验、Context 编辑/hash 与 Test Artifact 落盘回归。
    （已完成；Rust 92 passed + 1 ignored，TS 25 passed；启发式不等于完整敏感字段审计）
23. Raw Frame 结构化 allowlist、有界离线诊断导出与二次检查命令；严格字段路径、类型/深度/
    字节预算、未知字段默认丢弃、版本/来源标记、原子不覆盖及篡改拒绝回归。
    （已完成离线首切片；Rust 106 passed + 1 ignored，TS 25 passed；不等于生产 Recorder）
24. 原生保存/取消 UI、生产 Recorder 分片/保留与 CI 集成仍待验收/实现。
    dirty submodule、Checkpoint/Adoption/崩溃场景留在对应 M2/M2.5/M1-S7 工作包。
    Codex Beta Product Driver 已开始实际接入；真实 Turn/交互仍受对应 M0 事实门约束，ACP/
    Copilot 等其他 Product Driver 尚未接入。

实施细节与下一接手点见 `docs/implementation/m1-s5-handoff.md`，Artifact 接口、安全边界和
UI 验收记录见 [`m1-s5-artifacts.md`](./implementation/m1-s5-artifacts.md)，只读 Diff 的格式/预算/
可复现 UI 验收见 [`m1-s5-diff-view.md`](./implementation/m1-s5-diff-view.md)。Fake Driver 已完成实际
ContextDelivery、编辑版本链、显式 opt-out、Git/dependency facts 和 Test Artifact 首切片闭环；
外部 Product Driver 与设计 §22.3 的完整 Git/Checkpoint/Adoption 场景矩阵尚未完成。

Redaction 当前规则、兼容性与下一安全接手点见
[`m1-s5-redaction-v2.md`](./implementation/m1-s5-redaction-v2.md)。
结构化策略、离线命令、预算与生产 Recorder/CI 接手点见
[`m1-s5-raw-diagnostics.md`](./implementation/m1-s5-raw-diagnostics.md)。

## 并行事实包 M0-ACP

1. 官方 Rust SDK 固定为 `agent-client-protocol 2.0.0`，关闭 default features，使用
   `Client.builder()` stable-v1 API。（已完成）
2. 使用内存 fake Agent 验证 initialize/capabilities、session/new、prompt/update、
   permission option 保真和 session/cancel。（已完成）
3. 在 typed dispatch 前保留未知扩展的原始 method/params；SDK v1 丢失前导 `_` 的行为
   已锁定为兼容性证据。（已完成）
4. 本机 Cursor IDE wrapper 会在 `cursor agent` 调用时尝试安装，且通用 `agent` 实际是
   Grok Build；在真实 Cursor Agent 到位前不创建 Cursor manifest。（受外部 runtime 阻塞）
5. 真实 runtime 到位后验证安全版本签名、`agent acp` initialize/认证、session/load、
   prompt/update、permission、cancel、shutdown，再决定 ACP Product Driver 边界。

离线证据和后续门槛见 `docs/compatibility/cursor-acp.md`。任何会触发安装、登录、模型
额度或工作区写入的探针均不得作为后台扫描动作。

## 并行事实包 M0-CODEX

1. 在 `experiments/codex-app-server/` 建立与 Product Driver 隔离的探针程序。
2. 对每个已发现 Codex 版本运行 `app-server generate-json-schema`，保存 schema hash 和脱敏差异。
3. 用结构化 stdio 子进程完成 initialize/handshake，并捕获脱敏事实 fixture。（已完成）
4. 在用户现有认证上下文中验证 ephemeral `thread/start`、`turn/start`、流式完成与 `thread/resume`。（thread/start 已完成；`0.144.5` 零 Turn 跨进程 resume 通过；`0.150.0-alpha.12.2` 的 name update 不产生 rollout，需在获准真实 Turn 后复测）
5. 分别验证 approval、提问、Diff、cancel/interrupt；缺少证据的能力保持 Unknown。
6. 协议方法/字段适配结论已开始进入 `docs/compatibility/codex.md` 与 M1-S4 Beta Driver；继续
   以真实 Turn/interaction/cancel 事实决定 capability，不从合成测试提升支持等级。

当前 `turn-smoke` 与 `interrupt-smoke` harness 已完成：有界 JSONL transport、response/notification/server-request 分类、interleaving 保留、未知扩展保留、流式事实聚合和显式 `--allow-model-usage` 门。真实执行仍待操作者确认额度消耗。

M1-S3 已完成内容与剩余限制见 `IMPLEMENTATION_STATUS.md`。其扫描只在 discovery
成功后计算差集；缺失路径由仅限 `SystemRecovery` 的命令写入
`agent.installation_removed`，删除当前 Projection 而不删除 Canonical Event 历史。
`NotConfigured` 已进入 Rust/TS wire 与 UI，但在独立认证事实探针完成前不会由
`--version` 推断。M0 探针不得直接进入桌面 Product Driver，也不得读取或复制认证文件。

## 通用完成定义

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `pnpm typecheck`
- `pnpm test`
- `pnpm build`
- 对可执行桌面改动运行 `pnpm desktop:dev` 冒烟测试。
# 上下文交接计划更新（2026-09-09）

已实现非 Git 有界采集、Git/非 Git 持久化起点、启动崩溃恢复和真实 Codex→Kimi→Codex 对话接力验收。容量与验收边界详见 [实现记录](implementation/m1-s5-turn-changes.md)。后续方向为历史查询入口及未覆盖供应商验收；已有 Codex/Claude 文件修改证据见 Review 交付审计，完整大项目备份不属于 Diff 交付范围；不增加控制台 LLM 总结或用户交接确认界面。
