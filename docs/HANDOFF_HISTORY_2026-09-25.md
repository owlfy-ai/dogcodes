> 历史归档（截至 2026-09-25）。以下保留整理前全文，其中“当前”“下一步”“尚未实现”等描述只代表当时阶段，不能作为现行待办或实施约束。现行范围、状态和顺序以 [当前工作清单](CURRENT_WORK.md) 为准。

# Agent 接手指南

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


## 最新产品决策（2026-09-19，优先于下方历史记录）

- 允许不同 Agent，或同一 Agent 的多个独立对话，同时修改同一项目的共享目录；不要增加项目级单写者门禁、普通对话排队或强制 worktree。单个对话的活动轮次及审批/取消隔离仍保留。
- Git 提交备注“AI 总结”不需要独立执行留痕；不要把增加 ExecutionRun、输入/输出 Artifact 或审计记录列为 M1 待办。上下文交接已有的来源记录保持原范围。
- 服务锁释放修复、格式与严格 Clippy 检查已完成；SDK 暂缓，逐轮只读委派首批已实现，继续真实 Agent 联调和剩余验收。同项目并发的不同 Agent/同 Agent 多对话及交互隔离需要独立验收，不以本次稳定性收尾代替。
- 本次全工作区 Rust 回归 436 通过、61 忽略；前端 324 项测试、类型检查与构建通过。桌面 16 线程连续 20 轮通过；验收特性严格 Clippy 和 56 项桌面测试通过。首次回归的一次 Claude 退出超时在单项、测试组连续 5 轮及最终全工作区重跑中未再出现，根因未确认。细节见 [稳定性记录](implementation/m1-stability-2026-09-19.md)。

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


## 2026-09-11：Claude 独立额度查询

`claude-driver/src/quota.rs` 增加只读官方 `/api/oauth/usage` 拉取，按 60 秒缓存/重试；复用默认 macOS Keychain、凭证文件或显式 OAuth token，curl 通过 stdin 接收授权头并应用现有系统代理环境，不修改/刷新凭证、不发模型请求、不记录密钥。HTTP utilization 按百分比解析（不能复用 stream-json 的 0–1 解析器），保留较新的推送及会话上下文。需要系统 curl；自定义配置目录目前读取凭证文件，不支持其散列命名钥匙串。

Core 只透传白名单诊断码，监控页面和选中 Agent 的圆环显示自定义服务、缺少授权、网络错误、限流等区别。默认单元/进程 fixture 禁止读取本机账户；显式真实只读测试入口：`cargo test -p claude-driver real_usage_read_only -- --ignored --nocapture`。

本机事实：默认 OAuth 钥匙串和凭证文件不存在；用户 settings 配置 ANTHROPIC_AUTH_TOKEN 和非官方 ANTHROPIC_BASE_URL。CLI auth status 的 loggedIn=true 不能作为官方订阅授权依据。真实官方额度成功验收因此未完成；不能将该自定义服务的 token 发给 Anthropic。若需要显示该服务余额，需要其独立额度 API。当前显示明确的 custom-provider 诊断，不伪造数值。

## 2026-09-11：额度首屏独立加载

启动时七个 provider 使用 JoinSet 并发探测；每个 provider 完成注册后发送 `quota-provider-ready`，前端立即重读安装列表并请求额度。`core_quota_snapshots` 新增可选 installationId，前端每个安装独立调用、独立更新，不再等待最慢 provider。保留 30 秒轮询及驱动缓存；单安装在途请求去重，切换会话/卸载后丢弃旧响应，更新已有卡片不改变其顺序。

入口：`src/quotaRefresh.ts`、`App.tsx`、`tauriClient.ts`、桌面 `scan_builtin_agents`、Core `quota_snapshots_for`。测试：前端 141 项通过（含快慢隔离、在途去重、旧响应忽略和新增安装）；TypeScript 检查及桌面 Rust cargo check 通过。尚未执行本次真实桌面冷启动计时验收；接口本身的网络延迟、缓存和认证退避仍然存在。

更新日期：2026-09-09

## Review / Diff 当前范围（已精简）

用户明确要求的是编程 Agent 的 Diff 查看功能。保留 Git 五种比较、非 Git 会话初始基线、多目录、冻结阅读、暂存/取消暂存、普通文件丢弃/撤销（含重命名配对）。子模块只展示状态并支持暂存提交指针。

已移除子模块/嵌套整树恢复、目录拓扑重建、候选目录清理、遗留 Git 锁隔离、暂存续作管理和侧栏存储管理，以及对应桌面接口、专用执行模块和验收脚本。不要重新把这些扩展为交付门槛。普通文件保护、现有基线及备份继续保留。

当前实现与验证入口见 [review-panel.md](implementation/review-panel.md) 和 [review-delivery-audit.md](implementation/review-delivery-audit.md)。旧批次记录和恢复系统的历史缺口已退出当前计划；裁剪前源码与文档有项目外备份。

## 正在实施：多目录项目

见 [多目录项目实施](./implementation/multi-root-projects.md)。创建项目、数据模型、目录提示及原生 adapter 已接线；前端 91 tests、Rust workspace lib、all-targets check、生产 build、Codex 两项进程 fixture 通过。浏览器 Demo 已验证两目录创建。开发阶段不要求旧版兼容，桌面使用新 `app-v5.db`，旧 `app.db` 原样保留；本机原生登录不变。Review 已支持多目录；逐轮交付采集已覆盖附加目录（2026-09-11）；真实 Codex/Kimi 跨目录文件交接和七家 Driver 取消/恢复已有通过记录；GUI 覆盖范围见当前清单。目录清单当前逐轮注入，尚未按会话去重。

## 上下文交接：逐轮净变化续批

自动交接已加入 Git/非 Git turn_start→turn_end 净变化、变更文件版本和不可变 Artifact。结束采集先于轮次终态，普通聊天不显示交接预览。详情及范围见 [M1-S5B](./implementation/m1-s5-turn-changes.md)。历史浏览/检索与 Agent 只读入口已接入，逐事实交付范围和超限按需读取已于 2026-09-12 完成；已有真实接力与文件修改验收见对应文档；不要将有界变更文件版本称为完整项目备份，不要将共享工作区变化归因为 Agent 独占修改。

## 最新界面与 Grok 额度诊断（2026-09-09）

- 用户指定 Antigravity 圆环固定读取 Gemini Models 的 5 小时窗口，聊天框和 Agent 下拉菜单均生效；缺失该窗口保持未知。悬浮仍显示全部窗口。仅改变圆环展示，发送门禁仍按当前明确选择模型的额度组判断。类型检查及额度相关 15 项测试通过。

- 额度总览以安装版本替换内部 Installation ID；Agent 菜单移除版本文字，每项按 provider + installation 匹配额度圆环。未知百分比继续显示问号。类型检查及额度组件 7 项测试通过。
- Grok 1.0.24 真实只读 `_x.ai/billing` 响应仅返回 config.currentPeriod、onDemandCap/onDemandUsed/prepaidBalance、isUnifiedBillingUser、billingPeriodStart/End 和 subscription_tier；缺少 creditUsagePercent 及 used/monthlyLimit。因此监控可显示账期/重置时间，但百分比保持未知。只输出字段类型，未记录凭证或原始账户响应。
- 后续已核实官方源码 `xai-grok-pager/src/app/effects/helpers.rs::credit_balance_from_config`：百分比缺失且 monthlyLimit 非正时默认已用 0%。`/usage` 与 ACP 共用 billing。真实原始 GET `/v1/billing?format=credits` 为 200，但同样缺少 creditUsagePercent/productUsage；旧 GET `/v1/billing` 为 200，used 存在但 monthlyLimit 为 0，不能作分母。凭证只在内存使用，未修改登录或发送模型请求。当前账号实际剩余比例仍不可验证，不能以原生默认值伪装剩余 100%。Grok 总览未知读数下增加原因说明。原 billing 验收只断言窗口和 reset，不代表取得百分比。

## 最新基础设施更新：所有 Coding Agent CLI 自动继承代理

- `agent-runtime::apply_proxy_environment` 统一解析并注入代理：优先采用 dogcodes 进程继承的 `http_proxy`、`https_proxy`、`all_proxy`、`no_proxy`（兼容大写），缺失项再通过原生系统接口补齐；不会启动 `scutil`、`gsettings` 等外部命令。
- macOS 直接调用 SystemConfiguration 的 `SCDynamicStoreCopyProxies`；Windows 读取当前用户 WinINet/WinHTTP/注册表有效配置；Linux 通过 GLib/GIO 读取 GNOME GSettings，兼容 KDE `kioslaverc` 与桌面沙箱 portal。
- 同一代理值同时写入大小写环境变量，覆盖 ACP（Grok/Kimi/Cursor）、Claude Code、Codex、Antigravity、ZCode 的主会话、额度读取和模型目录子进程；环境仍先 `env_clear`，不扩大其他变量暴露。
- 系统代理派生的 `NO_PROXY` 自动包含 `localhost,127.0.0.1,::1`，Kimi loopback HTTP 仍显式禁用代理。代理值不写日志、不持久化，也不硬编码本机 7890 端口。
- PAC/WPAD 无法可靠折算成单个 CLI 环境变量，因此暂不虚构固定地址；这类环境仍需在启动 dogcodes 前显式设置代理变量。运行中的 CLI 连接不会因之后修改其他 shell 环境而改变，需要重启应用或重建 Agent 连接。
- 渲染层 WebView 的网络请求可由其引擎遵循系统策略，但标准前端 API 不提供跨平台读取实际代理地址的能力；代理发现与 CLI 子进程注入由 Rust 原生运行时负责。
- 2026-09-09 在桌面用户会话权限下，原生 macOS 探针直接读到 `http://127.0.0.1:7890` 并通过；清空全部代理环境变量后，Grok ACP Ready 与 `_x.ai/billing` 真实只读探针仍通过，证明端到端使用的是原生系统代理。隔离测试环境读不到该用户设置，解释了此前命令探测结果的差异。全程未发送模型 Turn。
- 真实只读验收：本机继承 `127.0.0.1:7890` 后，Grok ACP Ready 探针与 `_x.ai/billing` 额度探针均通过；没有发送模型 Turn。

## 最新 UI 更新：品牌图标主题适配

- Codex、Grok Build、ZCode 的本地 SVG 资源统一使用浅色路径；图标外壳仍由主题控制，浅色主题通过内层图像滤镜显示黑色，深色主题显示白色。
- 三个图标改为“外壳 + 内层图像”渲染，避免反转滤镜影响圆角边框和背景；系统跟随、手动浅色和手动深色三种主题均覆盖。
- Agent 图标容器统一放大为 small 20px / medium 26px，品牌图标统一保留 2px 内边距并裁剪溢出，在放大的同时避免贴边；SVG 保持原始比例，不做拉伸变形。
- Cursor 图标改为与 Codex/Grok/ZCode 相同的主题渲染路径：资源使用浅色路径，浅色主题滤为黑色、深色主题保持白色；所有品牌图标保留 2px 内边距，避免与边框粘连。
- ZCode 不再固定使用紫色填充，因此浅色背景下不会与背景混淆；相关资源为 `apps/desktop/src/assets/zcode.svg`，渲染逻辑在 `apps/desktop/src/App.tsx`。

## 最新 UI 更新：Antigravity 品牌 SVG 图标

- 使用用户提供的官方彩色 SVG，保存为 `apps/desktop/src/assets/antigravity.svg`。
- `AgentIcon` 对 Antigravity 使用构建期资源 URL，替换侧栏、会话记录、Agent 选择器及 Composer 中的字母占位图标；其他 Agent 图标逻辑不变。
- 增加小尺寸适配样式，Vite 生产构建确认输出 `dist/assets/antigravity-*.svg`。前端 60 项测试与 typecheck 通过。

- Claude Code 同步替换为用户提供的 `claude-code.svg` 路径图标，保留既有尺寸和主题边框样式。
- ZCode 同步替换为用户提供的三段路径 `zcode.svg` 图标，使用主题自适应黑/白填充和尺寸适配。
- Cursor 同步替换为用户提供的六边形路径 `cursor.svg` 图标，使用浅色路径填充适配深色界面。
- Grok Build 使用用户提供标志左侧的图形部分，`grok.svg` 已裁剪 viewBox，仅保留图形、不显示 Grok 文字。
- Kimi 使用官网左上角同款官方品牌资源 `pwa-192.png`，已固化为 `apps/desktop/src/assets/kimi.png`；`AgentIcon` 不再回退到 `◐` 字符，所有侧栏、会话和 Composer 位置统一使用本地 PNG，不依赖运行时网络。
- Codex 使用 ChatGPT 官方 sprite 中的 `#55180d` 图形（`sprites-core-7bb22fec.svg`），已独立提取为 `apps/desktop/src/assets/chatgpt.svg`；Codex 的 `◎` 占位字符已移除，仍沿用 Codex 的深色主题边框。
- Agent 选择器不再展示内部 Fake driver；Claude Code、Grok 等 CLI 的 PATH/用户目录重复发现会按可用状态和语义版本只保留最佳一个。Codex 的桌面/CLI 多 Installation 仍保留，因为它们是不同运行时。

## 最新推进：额度异常矩阵

- 新增 quotaAcceptance.test.ts（固定时钟，无网络/真实额度消耗）：14 项场景覆盖正余额、100% 耗尽、重置边界、TTL、未知/非法数值、周额度限制、显式模型组切换、Cursor/工具/MCP 展示 scope。
- 修复 99.96% used 被显示取整误判为耗尽：门禁直接比较有限原始 usedPercent >= 100，小额正余额显示下限 0.1%，圆环标 <1。
- 已到 reset 的旧主窗口不继续显示当前 0%，改为 Unknown + 等待读取新额度；Panel 标明重置待刷新，不捏造 100%。旧失败记录本身仍可提示耗尽，不作为未知额度的硬禁发依据。
- App 按已有读数的 reset/TTL 边界安排本地重渲染，避免等待原生轮询才更新发送门禁。没有额外高频网络轮询。
- 组件回归补充重置待刷新、小额正余额。接口失败的缓存状态由固定快照验证，未注入真实供应商网络故障；真实耗尽账号、桌面鼠标 hover/缩放仍未验收。

## 最新修复：重复点击当前会话导致时间线空白

- 确定根因：侧栏 onClick 无条件 setTimeline([])，重复点击当前 task 时 ID 不变，加载 effect 不重跑。并非数据库丢失，也不能仅归为启动竞态。
- 统一 selectTask：同 ID 无操作；跨会话同步失效旧请求并清空旧审批。RequestEpoch 为时间线/审批/额度分别提供请求代次，防止旧响应、A→B→A 和重叠刷新覆盖较新数据。
- 会话加载与事件刷新复用 loadConversation；额度请求独立，不再用 Promise.all 阻塞聊天展示。时间线失败显示专用错误和重试入口，不再静默变成新会话欢迎页。
- 前端 44 tests/typecheck 通过；新增三项回归覆盖当前会话重复点击、旧空读数晚返回、A→B→A/卸载失效。
- 真实 Tauri/System Events 验收：重启后打开既有验收 task，重复点击 5 次、快速往返切换 5 次，前后均保留两组 Desktop acceptance/OK 记录。未发送新模型请求。
- 下一步：额度异常测试矩阵（耗尽/重置/过期/失败/模型组隔离），及 mouse hover/窗口布局细节。本段覆盖下方“首次切换空白待定位”的历史状态。

## 最新验收：桌面权限已解阻，核心链路通过（2026-09-08）

- 通过真实 Tauri 窗口 + System Events（非浏览器 Demo）完成：新建专用会话、原生模型目录、Gemini/Claude 选择、模型组圆环与详情。观察值 Gemini 5h 98.8%，Claude 5h 98.7%；详情含每周与重置时间。原生默认仍 Unknown。详情通过键盘聚焦验证，鼠标 hover 未单独记录。
- 专用 task `tsk_01M20TKEGR5XZQ7RDXM7T467A0`：Claude 首轮返回 OK，模型 disabled；进程重启后从侧栏重开，模型/锁定保存；再次真实发送恢复验收 prompt 返回 OK。
- 实测发现不同轮次的原生 message ID 相同导致答案拼接。Store 改用 task+turn+native item ID；DB v4 在事务中回放历史投影，事件日志不变。新增跨轮增量/迁移/回放回归，Store 30 tests 通过。真实迁移后两个 OK 分别跟随各自 prompt。
- 迁移前一致性备份：`/tmp/dogcodes-pre-v4-acceptance.db`，权限 0600。未删除既有历史，新增一个验收会话及两次最短模型请求。
- 待办：启动后首次切换时间线曾空白，切至其他会话再回来恢复，需要单独定位刷新竞态；真实耗尽账户门禁与 mouse hover 本轮未专门覆盖，不宣称全桌面无缺陷。
- 收尾：Store clippy 通过；本轮桌面与 Vite 已停止，释放 1420 端口。

## 前次验收：双模型通过，桌面权限阻塞（已解阻）

- `AGY_MODEL_EXECUTABLE="$HOME/.local/bin/agy" cargo test -p antigravity-driver explicit_models_complete_real_turns -- --ignored` 通过，57.20 秒。Gemini `gemini-3.1-pro-low` 与 Claude `claude-sonnet-4-6` 顺序真实短 turn 均出现 TurnCompleted，均无 ToolStarted。
- 首次重跑仍挂在模型间交接，已停止该执行。根因是 ChildStdin 的 shutdown 未释放管道；改为 Mutex<Option<ChildStdin>> 并 take/drop，确保 EOF 实际送达；shutdown、idle retire、interrupt 统一处理。修复后双模型通过；Driver 单元测试 7 passed。
- Vite 1420 与真实 `cargo run -p desktop` 已启动。但 CUA 按名称/路径均无法识别未打包进程；System Events 读取窗口被 macOS 拒绝：osascript is not allowed assistive access。未修改系统权限，未把浏览器 Demo 当成桌面验收。
- 待解阻：为执行自动化的宿主开启辅助功能权限，或使用可被 CUA 识别的打包应用，再验证新会话模型选择、提交后的模型锁定、重启恢复、额度圆环与悬浮模型组一致性。桌面交互仍未通过，持久化 model 的专门回放断言仍待补充。
- 收尾：补充 fake 回归，shutdown 后 2 秒内成功创建第二模型会话，测试通过；前端 41 tests/typecheck、Driver clippy 通过。本轮启动的桌面及 Vite 已停止，避免占用用户后续启动的 1420 端口。

## 前次实施记录：Antigravity 显式模型与额度组

- 新对话可通过 core_available_models 加载 agy models 原生目录，SubmitTurn.model 经 Core 校验后传给 create_session_with_model，原生启动参数包含 --model。现可为下一轮更换模型与思考强度；配置变化时优先在同一原生会话中更新：ACP/ZCode 使用原生配置接口；Codex 在下一轮 turn/start 下发 model/effort，Claude 使用 apply_flag_settings 并等待匹配确认，均保留进程；Antigravity 的 stream-json 不接受配置控制消息，仍用新配置恢复同一原生会话 ID。仅在原生会话无法恢复或明确不支持配置更新时重建并衔接历史；连接与协议错误不会触发静默重建。
- 数据库版本 3 增加 session model；session.ready 事件及 projection 保存模型，恢复时传回原模型。迁移兼容已有 model 列；Store 29 项测试通过。
- 前端按显式 gemini-/claude-/gpt- 模型与已验证组名匹配圆环；不修改原始快照，未知默认继续 Unknown。选择按 task/installation 隔离，目录异步返回防串写。
- 本地 fake 测试验证 --model 参数和原生 session ID 复用；修复 shutdown/切换空闲会话只标 finished 未关闭 stdin、进程锁不释放的问题。
- 真实 explicit_models_complete_real_turns 本轮挂起，未取得全部通过结果；已通过该执行会话 Ctrl-C 停止（exit 130）。修复后重新实测受审批服务限额阻塞，不能标完成。精确子进程检查也未获审批，残留原生子进程状态未确认。
- 原生 UI 未启动，localhost:1420 不可达，因此完整桌面验收仍待执行。不要以浏览器 Demo 或单测代替真实桌面。
- 后续：补 session model 重启/回放专门断言；重跑两模型真实短 turn；启动桌面验证新对话选择、会话中切换、额度圆环切换与耗尽门禁。此前其他额度来源状态不变。

## 最新实现：ZCode 原有凭证复用已获授权并实测通过

- 用户明确授权读取 ZCode 原有凭证（不新增登录）。优先读取 `~/.zcode/cli/config.json` 中当前 model 对应 provider 的 apiKey，不从其他 provider 猜选凭证。
- 新增 `zcode-driver/src/quota-reader.cjs` 和 `quota.rs`：借用已发现的 ZCode Electron Node 运行时，只读配置，凭证不进入 stdout、日志或数据库；只允许 HTTPS open.bigmodel.cn/api.z.ai 固定 quota GET，拒绝重定向，不刷新/改写登录。
- 当前本机 BigModel 真实调用成功；新 Provider `real_native_credentials_quota` 通过。Z.ai 分支使用同一解析契约，但没有本机已登录账号实测，不能标成真实验收通过。
- 5h、周窗口和工具月度额度已映射，工具窗口带 scope 不阻断普通对话；缺失 reset 保持 Unknown。60 秒缓存/失败退避，失败不延长旧读数 TTL，继续附带当前 session context。
- Node 3 项 parser/host/cipher 测试通过，Rust check/clippy 通过。遇到凭证失效请用户在 ZCode 重新登录；本项目不接管刷新令牌。
- 仍未完成：Antigravity 当前模型选择/回传与 scope 关联（与本次 ZCode 凭证任务独立）。

## 最新实现：Cursor macOS 自动额度采集

- `crates/acp-driver/src/cursor_quota.rs` 已接入 ACP QuotaProvider 和桌面注册。
- 使用系统 script 创建 PTY，固定 stty sane/120x40；仅信任 Driver 自建空目录，等待原生 /usage 菜单后回车，读出用量后 /quit 正常退出。不读取凭证、无模型 prompt。
- 真实自动验收通过，约 5 秒；启动目录由 Installation 的 QuotaState 复用，避免每次刷新创建目录。沿用 60 秒缓存/失败退避、27 秒外层超时及 context 合并。
- 仅采套餐 Included 整数百分比，圆环可显示但不会据此禁用发送；重置月日、计划与按量付费提示保留原生文本，不猜年份/时间。非 macOS 返回未配置。
- Antigravity 零 turn init 实测只有 cwd/tools/permission_mode，没有 model；models 列表也没有当前标记。自动关联不可从列表首项猜测，尚需贯通用户显式模型选择。
- ZCode 尚待用户确认是否接受新增 BigModel/Z.ai 授权流程；没有扩大读取原生凭证权限。

## 最新推进：原生额度重验已解阻（2026-09-08）

- Antigravity 新 Provider `real_readonly_quota` 真实通过，约 10 秒，连续读取缓存一致。覆盖上轮“审批限额阻塞”状态。
- 修复活跃会话占锁时额度拉取等待 20 秒：立即返回 stale cache/未知，不争抢原生认证进程锁。
- 修复 Ring：无关 scope/未知百分比不再覆盖真实 turnQuotaExhausted。前端 39 tests/typecheck 通过，驱动 clippy 通过。
- Cursor 原生 TUI `/usage` 真实成功：Pro Included 15% used、Auto 16%、API 0%，Resets Oct 4，On-Demand Disabled。仅零模型本地命令，已 /quit 退出。探针专用空目录 `/private/tmp/quota-probe.6BlbcH`；仅该目录获原生 workspace trust。
- 尚未自动采集 Cursor：TUI 不是 JSON，日期缺年份/时刻，禁止捏造精确 reset timestamp；需受控 PTY 采集及版本/布局回归。
- 当前 Antigravity 会话 API/事件未提供可靠当前模型字段，不能取 models 列表首项猜测；需要模型选择/回传贯通后才把 scope 用于主圆环。
- ZCode 仍只有桌面内部 service/channel 线索，未找到已验证外部额度 RPC；不要读取原生凭证或直接调用 reset。

## 当前实施：Antigravity 额度 Provider 已接入，真实重验待放行

- 新增 `antigravity-driver/src/quota.rs`，独立原生 /usage 拉取、60 秒缓存、20 秒超时、输出上限、沿用进程互斥及认证退避；桌面注册 QuotaProvider。
- QuotaWindow 新增可选 scope，前端总览/悬浮显示模型组；未解析当前模型时圆环保持 Unknown，不能将某组耗尽视作全账号耗尽。
- Cursor/ZCode 仍是研究阶段，未新增授权读取或宣称支持。后续继续按 quota-source-research.md 验证出口。
- `cargo check --workspace` 通过；Antigravity 单元测试 5 passed。真实重验沙箱内失败，沙箱外审批因审批服务使用限额拒绝，未绕过。
- 重验命令：`AGY_QUOTA_EXECUTABLE="$HOME/.local/bin/agy" cargo test -p antigravity-driver real_readonly_quota -- --ignored`。这是只读额度，不发送模型 turn。

## 最新研究：剩余三个 Agent 的额度来源

见 [数据源研究](implementation/quota-source-research.md)。Antigravity 原生 `/usage` 已真实零 turn 验证；
Cursor CLI `/usage`、ZCode 桌面 entitlement 服务已静态定位，但外部自动化读取尚未验收。
这些是研究进展，尚未注册新的 QuotaProvider。接入前先解决模型组/计费作用域，避免任一窗口耗尽误封所有模型。

## 最新接手点：Codex 发现与额度 wire 回归已修复

- `codex` 保持一个 Provider；桌面 bundle 与独立 CLI 是不同 Installation。manifest 已覆盖
  `/Applications/ChatGPT.app/Contents/Resources/codex`、`/Applications/Codex.app/...` 以及用户
  `Applications` 对应路径，发现层 canonical 去重。
- 安装差集不再把“本轮 discovery 没返回”直接等价为卸载；只有已记录 executable 的 metadata
  也不存在时才发 `agent.installation_removed`。这修复了终端 PATH 不含桌面内置 Runtime 时误删
  Codex Ready 记录的问题。
- `QuotaSnapshotView` 增加 `#[serde(rename_all = "camelCase")]`；前端现在能用
  `providerId + installationId` 匹配 Composer 圆环。此前所有 Agent 都显示无读数是 wire contract
  错误，不是所有供应商额度接口同时失败。
- 本机只读事实：Codex 桌面内置 Runtime 0.153.4 发现通过并已由运行中的桌面重新注册 Ready；
  桌面子进程已启动该 Installation 的独立 `codex app-server --listen stdio://` 连接。Kimi/Grok
  真实只读额度探针通过。最终回归：Rust workspace 171 passed / 34 ignored，fmt 与 workspace
  clippy `-D warnings` 通过；前端 typecheck、37 tests、production build 通过。

## 上一接手点：额度耗尽 UI 闭环完成

- Driver API 使用 `TurnQuotaExhausted { resets_at_unix }`，Core 仅持久化稳定
  `turn.failed.failureKind=quota_exhausted`、Executor 和可选重置时间，不保存供应商原始错误文本。
- Store 为该稳定终态生成 `quota.exhausted` 时间线项，Projection rebuild 结果一致。
- Composer 对新鲜、provider-reported 且尚未到重置时间的任一 0% 窗口显示红色 `!` 并禁用发送；
  Unknown/stale/Demo 不阻止用户。切换 Agent 会按 Installation 切换主动额度读数。
- ZCode 1308 已用真实账号验证为结构化耗尽；Codex/Claude/ACP/Antigravity 有保守明确措辞/code
  映射。不要把普通 HTTP 429 或 `rate limit` 扩大解释为订阅额度耗尽。
- 验证：相关 Rust 测试通过；workspace clippy `-D warnings` 通过；前端 typecheck、37 tests、
  production build 通过。真实 ZCode approval 用例因账户确实耗尽而按预期停在额度终态。

## 上一接手点：Beta Agent 重验完成

当前唯一验收矩阵、门槛和可重复命令：
[`docs/implementation/beta-agent-acceptance.md`](./implementation/beta-agent-acceptance.md)。

- ZCode 已修：`session/send` accepted 不再冒充完成；先 subscribe，再解包顶层
  `session/event.params.type/payload`，以 type 为权威、键签名仅作回退，等待真实终态。TextDelta 使用 `payload.delta`；终态后
  `session/read` 写按 native session id 隔离的 Context；interrupt 经 task channel 发送 stop。
- ZCode 零模型 probe 现在从 `/session/sessionId` 取 ID，要求 close 成功并再次 list 确认临时
  session 消失，2026-09-07 本机通过。
- ZCode 0.16.5 真实 stream/context（text=`OK`）和 cancel 均通过；配置元数据不能单独用于
  NotConfigured 推断，Ready 以协议错误及真实能力证据判定。
- 七条 Driver 的 stream/cancel 已按统一矩阵重跑完成；Codex、Claude、Kimi、Grok、Cursor 的
  deny/allow 通过。Antigravity 当时未接审批 Hook；后续通过记录见当前状态。
- ZCode `PendingInteraction` 已接通供应商原生 optionId，resume 控制面已在新进程重挂载成功；
  `params.type` 是权威事件类型，不能再用 payload `resultType` 抢先判终态。
- 历史阻塞：2026-09-08 的 ZCode allow 单跑在审批前返回 1308，不能视为审批协议失败。后续完整原生 response 回包及模型运行配置已修复，allow 落盘和 resume 回忆已通过，CLI 0.16.5 已提升 Supported。

## 上一接手点：M1-S7 恢复与诊断首切片

2026-09-07 续批：`SqliteStore::open` 已加既有库 READ_ONLY quick_check/版本预检，迁移前
复检；VM progress handler 限检查预算。Desktop `prepare_desktop_storage` 失败则只注册
StartupStatus，保留恢复窗口，不注册业务 Core、不启动 Agent。`StartupGate.tsx` 等待
ready 后才加载 App；否则显示备份/权限/版本处理说明。新增 Store 4 项、Desktop 2 项、
前端 2 项回归测试。降级入口不是损坏库会话浏览器，也没有自动修复功能。
本批验证：Rust workspace 155 passed / 22 ignored；前端 35 tests、typecheck、生产构建、
fmt --check 和 clippy -D warnings 通过。未使用真实用户数据库进行故障注入；原生故障启动
端到端验收当时未执行；2026-09-11 已在隔离目录通过损坏库启动及诊断 Save/Cancel。

- 已实现 `Core::recovery_snapshot`、进程内 Run RAII 归属追踪、CancelTurn 归属校验、
  Tauri 只读快照及原生保存诊断 JSON、React 恢复页和侧栏入口。
- 诊断以固定 code 和内部 ID 代替原始消息；未知 Provider 映射 other。不含 prompt、路径、
  native session、工具参数、token。导出复用私有临时文件 + 不覆盖既有目标的原子保存。
- 恢复页支持状态清单、返回会话、停止当前实例待审批 Run；后续已加入保留历史的显式会话重建，不提供自动修库或清理。
- 原生故障诊断及遗留审批失效已交付；剩余 GUI 异常中断/迟到审批边界见当前清单，禁止自动重发遗留指令。
- 实施细节、测试记录和未完成项见 [M1-S7](./implementation/m1-s7-recovery.md)。


## 当前交付记录：额度监控收尾

本节和 `docs/implementation/m1-s6-quota-monitoring.md` 为额度功能的当前事实；
下方“追加”章节保留历史，不能作为当前接线状态。

- 已修复 Claude QuotaProvider 漏注册、跨 session 上下文串用、轮询延长 stale deadline。
- Kimi 独立 loopback 读取、--no-open、single-flight/30s cache、HTTP 上限和进程回收。
- Grok 解析真实 percentage/legacy/plan 字段、单请求超时、重试、stale fallback。
- 所有 Coding Agent CLI 统一继承用户代理环境，并在缺失时通过各平台原生配置源读取静态系统代理；loopback 不走代理。
- 总览/Composer 显示未知、context-only、重置时间、采集时间与 stale；切换 Task 拒绝迟到数据。
- Kimi 上下文仅观察 UsageUpdate，已不再发送 /usage；旧文档“上下文必然完整”结论撤回。
- Kimi/Grok 真实只读验收均通过（包含缓存；不发送模型 Turn）。Grok 最初上游请求失败，
  保留本机代理配置后通过。Claude 订阅推送因本机自定义后端未做真实账号验收，
  已用离线完整 Turn 覆盖最后请求 token、会话隔离、过期与桌面注册。
- 回归：workspace tests 通过（149 passed / 22 ignored）；后续修改的 ACP/Claude 定向测试通过。
  fmt/clippy -D warnings 通过；前端 33 tests、typecheck、production build 通过。
  浏览器 Demo 验证总览与可聚焦 Popover；真实额度以 Rust 只读验收为准。
- `cargo build -p desktop` 已通过，可直接重新启动 `pnpm desktop:dev` 验收。
- 可交付边界：Claude 额度只在供应商推送后出现；Kimi/Grok 上下文只在 ACP 推送后出现；
  Grok 没有 percentage 的账号仍显示 Unknown。不得用模型 Prompt 或套餐推算补数字。

## 先读顺序

1. `agent-hub-requirements-v1.1.md` 的 §3、§7、§10、§16。
2. `detailed-design-v0.1.md` 的 §4–§8、§10、§18、§22、§25。
3. `docs/IMPLEMENTATION_PLAN.md`。
4. `docs/IMPLEMENTATION_STATUS.md`。
5. 本文“当前接手点”。

## 架构入口

| 区域 | 文件 |
|---|---|
| Domain 与状态机 | `crates/domain/src/lib.rs` |
| Wire DTO | `crates/protocol/src/lib.rs` |
| Driver Port/Fake Driver | `crates/driver-api/src/lib.rs` |
| Core/Repository Port/订阅 | `crates/core/src/lib.rs` |
| SQLite Event/Projection | `crates/store/src/lib.rs` |
| 安全进程执行 | `crates/runtime/src/lib.rs` |
| Agent manifest/发现/版本探针 | `crates/agent-registry/src/lib.rs` |
| 内置 Provider manifest | `drivers/manifests/*.toml` |
| Codex M0 探针与 fixtures | `experiments/codex-app-server/` |
| Codex Product Driver（Beta） | `crates/codex-driver/src/lib.rs` |
| QuotaProvider 与 Codex 额度实现 | `crates/driver-api/src/lib.rs`、`crates/codex-driver/src/lib.rs` |
| 额度监控实施记录 | `docs/implementation/m1-s6-quota-monitoring.md` |
| Beta Agent 统一验收矩阵 | `docs/implementation/beta-agent-acceptance.md` |
| ACP Product Driver（Kimi/Grok） | `crates/acp-driver/src/lib.rs` |
| Claude 原生 stream-json Driver（零依赖主通道） | `crates/claude-driver/src/lib.rs` |
| ACP 真实安装探针（ignored） | `cargo test -p acp-driver -- --ignored` |
| Kimi/Grok 兼容记录 | `docs/compatibility/kimi-code.md`、`docs/compatibility/grok-build.md` |
| Codex 实际接入状态与下一验收 | `docs/implementation/m1-s4-codex-driver.md` |
| ACP stable-v1 离线探针与 fixture | `experiments/acp/` |
| Cursor/ACP 兼容记录 | `docs/compatibility/cursor-acp.md` |
| Claude Code 安装/ACP adapter 事实 | `docs/compatibility/claude-code.md` |
| Context Package 构建与哈希 | `crates/core/src/context.rs` |
| Task Artifact 归属/有界读取/完整性校验 | `crates/core/src/artifacts.rs` |
| 确定性 secret/identity 脱敏 | `crates/redaction/src/lib.rs` |
| Redaction v2 边界与接手点 | `docs/implementation/m1-s5-redaction-v2.md` |
| Raw Frame 结构化策略 | `crates/redaction/src/structured.rs` |
| 离线诊断导出/二次检查与 CLI 回归 | `experiments/codex-app-server/src/diagnostics.rs`、`experiments/codex-app-server/tests/fixture_export.rs` |
| 诊断策略与 Recorder/CI 接手点 | `docs/implementation/m1-s5-raw-diagnostics.md` |
| 有界 Git/依赖 workspace facts | `crates/workspace/src/lib.rs` |
| M1-S5 实施记录 | `docs/implementation/m1-s5-handoff.md` |
| Artifact 接口与验收接手点 | `docs/implementation/m1-s5-artifacts.md` |
| Tauri Adapter | `apps/desktop/src-tauri/src/lib.rs` |
| 原始字节不覆盖导出 | `apps/desktop/src-tauri/src/artifact_export.rs` |
| TS Wire 类型 | `packages/protocol/src/index.ts` |
| 前端 Core Client | `apps/desktop/src/tauriClient.ts` |
| 主 UI | `apps/desktop/src/App.tsx` |
| 对话优先首页实施记录 | `docs/implementation/m1-s8-conversation-home.md` |
| Artifact 弹窗/安全预览 | `apps/desktop/src/ArtifactBrowser.tsx`、`apps/desktop/src/artifactView.ts` |
| 只读结构化 Diff | `apps/desktop/src/diffView.ts`、`apps/desktop/src/StructuredDiffView.tsx` |
| Diff 实施与本地 UI 复现 | `docs/implementation/m1-s5-diff-view.md`、`apps/desktop/tests/artifact-review.html` |

## 不变量

- Core crate 不得依赖 `tauri` 或 UI 类型。
- 同一 append 只写一个 stream；event append、Projection、dedup Ack 同事务提交。
- 命令业务校验前先查询 idempotency Ack；重试不得因第一次执行造成的状态变化而失败。
- Context Package Artifact ID 必须是 `art_<ULID>`；payload blob 以 SHA-256 为内容地址。Artifact、blob 与引用它的确认事件必须同事务提交且不可覆盖。
- `sequence` 是全局唯一顺序；`stream_version` 只用于单 stream 乐观并发。
- Context 中的 Diff 引用必须同时满足 Task、fingerprint 和冻结 `(after, through]` 区间，不能查询“当前最新”后污染旧预览。
- SQLite 当前 `user_version = 2`；后续 schema 变更要新增版本迁移。旧版 version 0/1 升级、Task Artifact 归属 Projection 回填和版本更新在同一事务，不能只建新表不回填。
- Artifact Patch 保持原始字节；不能经过 `String::from_utf8_lossy` 后再保存。UI 文本解码/JSON 格式化仅作展示，导出必须使用原始 bytes。
- Artifact 查询强制 LocalUser、Task 归属、已审阅 hash、媒体类型和 1 MiB 预算；每段最多 32 KiB。Tauri 限定主窗口，路径仅来自 Rust 原生保存对话框，不能开放任意路径参数或 WebView 文件权限。
- active Run executor 创建后冻结；修改 preferred executor 只影响下一 Turn。
- Channel 订阅必须先取得 live receiver 再查询 replay，live 重复按 sequence 去重。
- 广播接收发生 `Lagged` 时必须从持久化 Event Store 补齐，不可直接跳过。
- 不增加第三方 API Key 表单、凭据读取或 shell 字符串执行。
- QuotaProvider 必须与 AgentDriver 分离；额度刷新不得发送 Prompt。没有 provider-reported 来源时
  必须显示 Unknown，不得从套餐价格推算；上下文用量必须按当前 Task/native session 隔离。
- `Installed` 不能由 UI 或 `--version` 升级为 `Ready`；Ready 必须来自独立认证/capability 事实探针。
- `SubmitTurn` 与其他写命令一样必须携带 Task `expectedStreamVersion`；缺失 → InvalidArgument，过期 → Conflict。
- Driver 提供的任何文本（失败原因、warning、vendor payload）进入 Canonical 前必须收敛为稳定 failureKind、
  固定提示、redact+截断文本或纯字节数；driver 中途 `SessionReady` 只能记 `driver.session_ready` 诊断事件。
- `driver.protocol_warning` / `driver.unknown_event_observed` 是诊断事实，不是 Agent 回复；必须留在
  Canonical Event，但不得投影到 Timeline。读侧保留对旧版脏行的过滤，除非有显式数据库迁移替代。
- driver 流结束时若未观察到终止事件，Core 必须补写 `turn.failed { failureKind: "driver_stream_ended" }`。
- Agent 发现只使用绝对 PATH 条目；空/相对条目必须丢弃（防 CWD 劫持）。
- UI 事件流收到 `Error{retryable}` 后必须以 `lastSequence` 重订阅；后端 pump 对存储错误先做有限退避重试。
- Claude Code 主通道是原生 stream-json（零额外依赖）；不得重新引入 npm adapter 作为必需依赖。
  PendingInteraction 已落地且真实 deny/allow 通过，不得退回无条件 deny。
- Provider manifest 的环境 allowlist 不得包含 secret-like 变量；进程启动必须先 `env_clear`。
- External Provider 必须用 `version_output_markers` 验证产品身份；同名或通用 basename 不能单独作为 Provider 证据。
- Agent 扫描写入 Core 时必须使用 `SystemRecovery` actor；UI adapter 必须拒绝该内部命令。
- 只有 discovery 成功后才能协调缺失安装；扫描失败/manifest 失败不能删除既有 Projection。
- 缺失 executable 使用 `agent.installation_removed` 删除当前 Projection，但不得删除历史 Event；`NotConfigured` 只表示 executable 仍存在而认证/配置未就绪。

## 最近完成：对话优先主界面（2026-09-03）

- 启动不再显示强制项目表单，直接进入两栏对话工作台；项目连接是侧栏可选动作。
- 空数据库由 Desktop Host 创建 app-data `scratch/`、“快速对话”项目和“新对话” Task。
- 自动执行者只从 Ready 安装选择，顺序为 Codex、Claude、Kimi、Grok、其他，并保留精确
  `installationId`；无 Ready 外部 Agent 时使用 Fake。
- 侧栏按 `project.path` 聚合工作目录，同目录可显示多会话；每条会话和 Composer Agent 菜单均显示
  provider 图标。聚合逻辑及回归测试位于 `workspaceGroups.ts` / `App.test.ts`。
- 接手前先读 `docs/implementation/m1-s8-conversation-home.md`，不要把 scratch 改成空路径，也不要
  静默修改已有 Task 的 preferred executor。

## 最近完成：安装差集协调

- Rust protocol、Agent Registry、TS wire 和 UI 已覆盖完整五态安装状态。
- `ReconcileMissingInstallation` 仅接受 `SystemRecovery` actor，并校验
  `installationId + providerId + executable` 与当前 Projection 完全匹配。
- Tauri 后台扫描在成功 discovery 后按 Provider 计算 canonical path 差集；未变化路径不重复发事件，也不会误删其他 Provider。
- SQLite 处理 `agent.installation_removed` 时只删除 `installations` Projection；从头 replay 后缺失安装仍不会复活。
- 对应测试位于 Core、Store、Desktop 和 Protocol crate；当前全 workspace 基线见下方当前接手点。

## 最近完成：四驱动真实 Turn 验收（2026-09-03，操作者授权后执行）

- 统一验收形态（每驱动三轮，固定 no-tools prompt，`--ignored` 测试）：流式 TextDelta +
  `turn.completed` → 同会话第二轮 live 复用 → 长任务 + interrupt → `TurnInterrupted`。
- 结果：**Claude 2.1.218、Kimi 0.40.1、Codex 0.153.0-alpha.5、Grok Build 1.0.13 全部三轮通过**。
- 验收驱动出的三个真实修复：① Claude 中断后的 error_during_execution 终态映射为
  TurnInterrupted；② ACP connection loop 在 Prompt await 期间不轮询 cancel 通道（Prompt 已改为
  独立任务执行）；③ ACP probe 加入 session/new 验证，未登录（Grok 登录前状态）如实报
  NotConfigured，不再假 Ready。
- 未验收保持 Unknown：跨进程 `--resume`/`session/load`、PendingInteraction 审批透传。

## 最近完成：Claude Code 方案 A——原生 stream-json 零依赖通道（2026-09-03）

- 产品约束落地：Claude Code 不再要求用户安装任何 npm adapter/Node。新 crate
  `claude-driver` 直连官方 headless 协议（`-p --input-format stream-json
  --output-format stream-json --permission-mode manual --session-id <uuid>`），manifest 改为
  `claude_stream_json`，ACP adapter 路径与 `requires_executables` 检测已从桌面移除（如需可再引入）。
- 审批 = `can_use_tool` control request 一律 `deny`（Beta 边界）；中断 = `interrupt` control
  request；thinking delta 不记录；resume 在真实 Turn 验证前保持 Unknown。
- 真实探针（零额度消耗）：2.1.218 控制面 initialize 握手通过
  （`cargo test -p claude-driver -- --ignored`）；离线 fake CLI 契约测试覆盖
  classify、完整 Turn、审批 deny 与中断。

## 最近完成：多 Agent ACP Product Driver 接入（2026-09-03）

- 新 crate `acp-driver`（官方 SDK stable v1）：session/new、capability-gated
  `session/load` resume、有序 ContextDelivery→`session/prompt`、`session/update` 流式映射、
  `session/cancel` interrupt；审批一律 `Cancelled` 安全拒绝；driver 侧错误文本收敛为稳定
  failureKind，不进入 Canonical 原文。
- registry 扩展：`identity_args`（Kimi 裸 semver 的第二身份探针）、`user_paths`（`~` 相对发现，
  禁 `..`）、`requires_executables`（Claude 的 adapter 依赖）、`launch_args`（`kimi acp` /
  `grok agent stdio`）。
- 内置 manifest 5 个；桌面按 `protocol.kind` 分派注册。本机 initialize 级真实探针通过：
  Kimi 0.40.1、Grok Build 1.0.13、claude-agent-acp 0.73.0（后者为操作者要求接入后显式
  `npm i -g` 安装，非隐式 npx）。ZCode 0.16.5 完成发现（`/Applications/ZCode.app/.../zcode.cjs`，
  自定义 app-server 协议，非 JSON-RPC/ACP，Driver 属独立工作包）。
- 基线：Rust `130 passed, 10 ignored`（ignored 含四驱动真实 Turn 验收）、
  clippy/fmt 全绿；TS `25 passed` + build。

## 最近完成：评审修复（2026-09-03）

- 代码评审发现的 5 个重大问题已全部修复并有回归测试：driver 原始错误串/payload 不再进入 Canonical
  事件；`SubmitTurn` 强制 `expectedStreamVersion`（TS/Demo 客户端同步修改）；driver 流静默结束由 Hub
  补写 `driver_stream_ended` 失败；PATH 空/相对条目不再参与 Agent 发现；事件流出错后后端退避重试、
  前端自动重订阅。基线更新为 Rust `116 passed, 1 ignored`、TS `25 passed`，clippy/fmt/check 全绿。

## 当前接手点

工作包：M1-S6 订阅额度监控进行中；Codex 首切片已完成，后续 Provider 仍需逐个事实探针。

预期落点：

- `ContextPackageV1`、source/target executor、三档强度、来源区间、ContextPointer、结构化 semantic summary provenance 和 payload SHA-256 已进入 Rust/TS wire。
- `Core::build_context_preview` 只从冻结的目标 Task `(after, through]` 事件构建；through 必须不超过 Canonical Head，10,000 条上限防止无界展开。同一范围重复构建 hash 一致。
- 当前 builder 不展开 prompt、message、terminal、完整 diff；它输出完成 Turn 引用、显式 decision/test 事实和一个带 hash 的 Canonical History pointer。缺数据保持 `unknown`/空数组。
- Tauri query 与 React Handoff Preview 已接通。确认走 `ConfirmContextPackage`：Core 重建同一冻结包并比对已审阅 hash，随后在单事务写入 `art_<ULID>` Artifact、SHA-256 blob、`handoff.package_confirmed` 和 preferred executor 变更。它仍不代表 package 已发送给 Agent。
- 默认 Redaction 已升级 v2：quoted JSON secret、Bearer/Basic、URL userinfo、PEM 和整数阈值高熵候选在 Context/TestReport hash/落盘前屏蔽。重复处理不重复计数；只有结构校验通过的冻结历史 locator 保留，任意 URL 仍脱敏。旧 CAS 不重写，原始 Patch 仍 ruleset 0；误报/漏报边界见 v2 实施记录。
- 四条 Beta Product Driver 已进入桌面并完成真实三轮验收：Codex App Server、Claude 原生
  stream-json、Kimi/Grok ACP。完整 ExecutorRef 绑定 Installation，Core/UI 以 task/turn/run +
  expected version 接通停止。跨进程 resume 与 PendingInteraction 仍待验收。
- 对话首页默认从 Ready 安装中选择 Codex → Claude → Kimi → Grok，并保留 installationId；空库使用
  app-data scratch 创建“无项目”对话。实现与边界见 `m1-s8-conversation-home.md`。
- QuotaProvider registry 已独立于 Driver registry。Codex 使用 `account/rateLimits/read`，5 小时
  窗口优先；Composer 圆环随 preferred executor 切换，Popover 显示周窗口/重置及当前 thread
  上下文，独立总览页列出各 Installation。下一 Agent 先读 `m1-s6-quota-monitoring.md`，不得把
  `rateLimitsByLimitId` 的模型 bucket 合并到账户额度，也不得显示其他 thread 的 token usage。
- Codex Turn 曾将兼容性 warning/unknown event JSON 重复显示为助手消息；Store 已在写侧停止生成此类
  Timeline 条目，并在读侧隐藏旧库遗留条目。回归测试同时覆盖 Canonical 保留、旧行过滤与 rebuild。
- 已确认的确定性版本使用内容地址且不可覆盖；hash 错误零写入，重复命令返回原 Ack。`handoff.package_confirmed` 只表示用户确认，不得改名或解释为 sent。
- SessionMap 已进入 Rust/TS Bootstrap 与 SQLite Projection：`session.ready/resumed/resumable`、native turn 和 terminal `lastSeenSequence` 均来自 Canonical Event，rebuild 结果一致。
- Driver Port 已有 capability-gated resume 与有序 ContextDelivery；只有明确 Supported+Resumable 的映射可以走 Reconciliation，Unknown/Detached 仍不可恢复。
- `context_transfers` 跟踪 confirmed/failed/sent。Core 在运行态再次校验 Artifact 完整性，并保证 ContextDelivery 在用户 prompt 之前由 Driver 接受；直接绕过 UI 的跨 Agent prompt 也会失败。
- React 切回可恢复 Session 时自动从该映射 `lastSeenSequence` 构建 Reconciliation 增量。Fake A→B→A、失败后重试、hash/Session/事件顺序与 Projection rebuild 已验证。
- Preview 编辑已接入：UI 当前开放 objective/completed/pending，Core 锁定路由与来源并重新脱敏/hash；最终确认同时保存 Hub 原稿、最终 Artifact 与 `artifact_versions(user_edit)` parent 关系，UI 显示安全的字段 diff。
- Reconciliation 有独立“明确关闭对账并切换”动作；取消/X 不产生决定。opt-out 仍恢复旧 Session，但不发送对账内容；failed 可重试，applied 必须先于 driver turn，Projection 可重建。
- `WorkspaceContextProvider` 与真实 `agent-workspace` 已接入桌面：绝对 Git executable、结构化 argv、3 秒/256 KiB/1000 路径上限，只采 HEAD/status/diff stat；普通小文件只记录 hash，依赖清单进入 `dependencyChanges`，工作树漂移会使确认 hash 失效。
- 版本化 Test Artifact 已接通：仅 `SystemRecovery` 可提交已有测试结果，Core 对命令/结果/报告脱敏后原子写入受限 `TestReportV1` Artifact 和 `test.completed` 引用；不要把传入的 command 当作脚本执行。
- Git 读侧矩阵已覆盖空仓库、clean、仅 staged/unstaged、staged+unstaged、untracked、ignored 默认排除、rename/delete、binary、symlink、executable bit、detached HEAD、linked worktree、空格/Unicode/换行路径和路径预算降级。
- 设计 §10.4 的 `GitWorkspaceSnapshotV1` 已落地：porcelain v2、repo root、HEAD、branch/detached、哈希化 worktree identity 和 status fingerprint。每个已排队写入 Turn 起止产生绑定 Task/Turn/Run 的 `workspace.snapshot_captured`；稳定 `workspace.snapshot_failed` 诊断不会中断 Agent。
- Snapshot 已有可重建 SQLite Projection，并通过 Core/Tauri/TS Client 提供按 Task 最新优先、1–100 条查询；它不进入 Bootstrap，避免长期任务启动数据无界增长。
- 按需完整 Diff Artifact 已接通：只允许本机用户显式触发，以私有临时 index/object store 从 HEAD/空树生成包含 staged/unstaged/untracked/binary 的 ≤1 MiB Patch；所有读侧 Git 设置 `GIT_OPTIONAL_LOCKS=0`，真实 index 字节与 object store 均不变。
- `workspace.diff_captured`、Artifact 和 `workspace_diff_artifacts` Projection 原子提交并可重建；Context 只引用同 Task、同 fingerprint 且位于冻结事件区间内的 Patch。React 已有固化按钮和原始源码/敏感内容提示。
- 2026-09-03 基础边界修复：旧库 version 0→1 原子回填且失败整事务回滚；未来版本拒绝迁移；Canonical Event、CAS、dedup Ack 保留。后续相同 fingerprint 的 Artifact 不会改变既有冻结预览，rebuild 后仍一致。
- Git 每条业务命令前有界检查生效配置的 clean/process filter（包含 include），发现后返回 `external_filter_configured`；所有命令关闭 fsmonitor/hooks/自动维护/可选 index 缓存，stat 也禁用 textconv。该防护不是恶意 `.git` 的通用沙箱，未验证边界见实施记录。
- `run_bounded_bytes` 已接入 Git：Patch 保留非 UTF-8 原始字节，路径/status 等文本严格 UTF-8 解码。子目录项目的 status 与 diff 同按 `-- .` 限定，porcelain 的 repo-relative 路径从 repo root 取 hash，不越出所选子目录采集其他文件。
- Demo 不能访问本地 Git 时明确拒绝固化，不生成虚假的成功状态或事件。
- 受限 Artifact 查看/导出代码已实现：v2 `task_artifacts` 可重建，复合 cursor 防止同事件原稿/最终稿翻页丢失；读取前验证 Task/hash/媒体/预算/完整内容，UI 主动展开、全文 hash 校验和敏感内容确认后才可导出。
- 原生导出使用同目录私有临时文件与 `persist_noclobber`，不覆盖既有文件或 symlink；单测覆盖字节保真、权限和失败清理。Browser 实测后补 JSON 可读预览；未增加 HTML/ANSI 执行或远程内容加载。
- 只读结构化 Diff 已完成：完整 hash 后显示文件/hunk/原新行号、模式/rename/binary；最多 500 文件/2000 hunk/10000 行，超预算明确标记部分，无效或不支持格式回退原始/hex。真实 Git 输出回归覆盖空格、Unicode、特殊路径和 symlink/mode；不解释 binary payload，也不应用补丁。
- Browser 测试入口为 `apps/desktop/tests/artifact-review.html`；覆盖分段门、截断、快速切换/关闭的旧响应隔离，不连接用户项目/数据库。实现与复现步骤见 `docs/implementation/m1-s5-diff-view.md`。
- Raw Frame 结构化 allowlist 与离线诊断副本已完成：字段按完整路径/类型筛选、未知及敏感键默认丢弃、全输入/输出预算；新增 `sanitize-fixture` / `check-sanitized-fixture`，校验版本/来源与规范 JSONL，拒绝覆盖、损坏或篡改。不启动 runtime，不改写 live request ID，不声称原生版本已验证。
- 下一接手点：审批透传剩余工作是把 Codex `requestApproval` 按 Claude/ACP 同模式接入
  （挂起 + respond），随后是跨进程 resume 验证（Claude `--resume`、Kimi/Grok `session/load`、
  Codex thread resume）与真实审批场景的额度授权验收。ZCode 需要先做其自定义 app-server 协议的
  M0 探针。Raw Recorder/CI、原生保存 UI、ReviewRun/Checkpoint 仍保留在后续工作包。
- resume 失败已映射为 missing/incompatible/detached；`session.*` 与 `turn.failed` 仅持久化稳定原因码和通用提示，不能重新写入 Driver 原始错误字符串。异步测试已锁定 rebuild 与无泄漏行为。
- `experiments/acp` 离线契约已收口；SDK extension 前导 `_` 问题和 Cursor runtime 缺失见 `docs/compatibility/cursor-acp.md`。
- 真实 Cursor Agent 由操作者独立安装后再验证；不要调用会触发安装的 `cursor agent` 作为 probe。
- `experiments/codex-app-server` 的 schema、initialize、thread、resume 和受额度门保护的 turn/interrupt harness 已完成；不得在没有用户明确授权时运行 `turn-smoke` 或 `interrupt-smoke --allow-model-usage`。
- 所有子进程继续使用结构化 argv、期限、输出上限和明确环境；fixture 删除用户路径、账号、session id、消息正文等个人数据。
- 未经真实 runtime 验证的能力保持 Unknown，不根据协议文档或 fake Agent 结果宣称产品支持。
- 不要把磁盘缺失映射成 `NotConfigured`；后续认证探针只能在 executable 确实存在时产生该状态。

完成后，把本节改为下一个工作包，不能只在聊天中说明。

当前验证基线：Rust workspace `136 passed, 15 ignored`；前端 `30 passed`，fmt、clippy、typecheck 与
production build 通过。当前本机 Codex 0.153.0-alpha.5、Claude 2.1.218、Kimi 0.40.1、Grok 1.0.13
均为 Ready；本次首页改造未额外执行真实模型 Turn。原生保存交互仍待验收。

## 本地命令

```bash
pnpm install
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm check
pnpm desktop:dev
```

历史验收时目录尚未初始化为 Git 仓库；不要假设可使用 git diff/status。已有文件均视为用户工作成果。

## 最近完成：Codex 审批接入 + 真实审批验收；跨进程 resume 验收未通过（2026-09-04）

- Codex driver：`item/commandExecution|fileChange/requestApproval` 不再自动 decline，改为上报
  `ApprovalRequested`（挂起 jsonrpc id）并由 `respond_to_interaction` 回
  `{"decision":"allow"|"decline"}`；fixture 集成测试已更新为"应答 decline"。
- Claude 启动参数加入 `--permission-prompt-tool stdio`（Agent SDK 同款隐藏 flag）——没有它
  `manual` 模式不产生 `can_use_tool`。真实审批验收通过（写操作触发：allow → 工具真执行 →
  轮次完成）。边界：只读工具（如 `ls`）按 Claude 自身策略自动放行，不产生审批请求。
- 驱动 port 修正：`resume_session` 增加 `working_directory` 参数（ACP `session/load` 必需，
  由 Hub 传项目路径，driver 不再保存本地内存映射）。
- 跨进程 resume 验收（2026-09-04）：**Claude/Kimi/Grok 全部通过**（新实例重挂载 + 上下文
  回忆）。关键发现：Claude 的 `--session-id` 不能与 `--resume` 共用（否则启动即退出），resume
  分支已改为只传 `--resume`；Grok 首跑有 turn 1 超时瞬态，重试通过。Claude desktop capability
  已提升 Supported。Codex 0.153 因额度限流待重跑（thread/resume 对 0.144.5 本就 Supported）。

- 基线：Rust `133 passed, 15 ignored`、clippy 0、fmt ok、前端 `27 passed` + build。

## 最近完成：M1-S6 Codex 订阅额度监控首切片（2026-09-04）

- 独立 QuotaProvider registry 已落地；Codex 官方 pull、push 失效、stale fallback、账户 bucket
  过滤和 Task/native thread 上下文隔离均已实现。
- Composer 圆环优先显示 5 小时剩余；hover/focus 显示周额度、重置、上下文和缓存命中率；侧栏
  “额度监控”进入独立总览页。无官方来源的 Agent 继续 Unknown。
- 接手前读 `docs/implementation/m1-s6-quota-monitoring.md`。下一步逐个验证 Claude statusLine、
  Kimi/Grok 官方 usage 面；任何刷新不得发送 Prompt 或从套餐表推算。
- 基线：Rust `136 passed, 15 ignored`，TS `30 passed`，fmt/clippy/typecheck/build、Browser visual 和
  `pnpm desktop:dev` smoke 通过；Codex 控制面实测未创建 Turn。

## 追加：Grok 订阅额度接入完成（2026-09-04）

- acp-driver 编译收尾并接通 Grok billing：扩展请求经 `ClientRequest::ExtMethodRequest`
  走 `_x.ai/billing`（wire 方法保留 `_` 前缀），响应为原始 JSON Value。
- 真实 payload 形状（已实测）：`config.currentPeriod.{type,start,end}`（周窗口 ISO-8601）、
  `subscription_tier`；**无用量百分比字段**——按不伪造原则 `used_percent` 改为 Option，
  Grok 只上报窗口与重置时间，UI 对 null 显示"用量未知"。
- 桌面已为 grok 注册 QuotaProvider；`real_quota_probe` ignored 测试验证真实读取。
- 编译收尾与瞬态处理（2026-09-04 接手续完）：扩展请求改走
  `ClientRequest::ExtMethodRequest`（wire 方法保留 `_` 前缀，Response 为原始 JSON Value）；
  Grok 新连接首次扩展调用有瞬态失败（auth/billing 子系统预热），`read_quota` 已加单次重试。
  真实 payload 解析落地：`config.currentPeriod`（周窗口 ISO-8601 起止）+ `subscription_tier`，
  无用量字段 → `used_percent: Option`（None 时 UI 显示"用量未知"）。桌面已为 grok 注册
  QuotaProvider；`real_quota_probe` 验证真实读取成功。
- 基线：Rust `136 passed, 16 ignored`、clippy 0、fmt ok、前端 `30 passed` + build。

## 追加：Antigravity + Cursor 接入（2026-09-04）

- 调研结论：Antigravity 官方**不支持 ACP**（Issue #31 开放中）；主通道为官方 headless
  stream-json。新 crate `antigravity-driver`：init/step_update(text_delta,tool)/
  result 全事件映射、stdin 关闭中断、`--conversation` resume 保持 Unknown、无交互审批面。
  真实链路已验证通畅，但本机账号地域限制（Eligibility check failed）+ 未登录，真实 Turn
  验收待操作者解决后进行。兼容记录：`docs/compatibility/antigravity.md`。
- Cursor：`cursor agent acp` 触发的下载已完成，`~/.local/bin/cursor-agent`（2026.09.02）
  真实可用；ACP initialize 握手通过（loadSession=true），session/new 返回 Authentication
  required（未登录 → NotConfigured 语义）。manifest `cursor.toml` 已建（identity 用
  `acp --help`；注意 cursor-agent 的子命令是 `acp`，wrapper `cursor agent acp` 才带 agent
  前缀），走 acp driver 路线；registry 版本解析新增前导零回退（`2026.09.02` → semver）。
- registry：identity 探针放宽退出码（stdout/stderr 有有效输出即采信，`agy models` 场景）。
- 追加（同日，操作者登录后）：Antigravity 真实 Turn 三轮 + `--conversation` 跨进程 resume
  全部验收通过；conversation_id 经 SessionReady 事件上报为 native session id，桌面 capability
  提升 Supported。基线：Rust `140 passed, 18 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：Cursor 真实验收通过（2026-09-04）

- 操作者在 Cursor 完成登录后：真实 Turn 三轮（29.5s）+ `session/load` 跨进程 resume
  （30.2s）全部通过，走 `acp-driver` 现成路径，零新代码（仅 ignored 测试）。
  `cursor-agent acp` 是直连子命令（wrapper 才带 agent 前缀）；session/new 返回完整
  模型/模式矩阵。manifest capability 已标注 runtime verified。
- 基线：Rust `140 passed, 20 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：Antigravity 资格间歇性失败记录（2026-09-04）

- 真实验收通过约 1 小时后，资格检查再次间歇性返回 `Eligibility check failed`（driver 与
  探针一致）。疑似当日多轮真实测试触发服务端限制；`real_turn_acceptance` 可能因此在窗口期
  失败，非 driver 回归。权限行为（headless 无审批面）待资格恢复后实测，已在
  `docs/compatibility/antigravity.md` 记录验证方法。

## 追加：spawn 安全防护落地（2026-09-04，账号被换事故的修复）

- 事故复盘（日志证据）：agy 共享认证状态（token 文件 + macOS 钥匙串），并发/被中断的进程
  刷新竞态使其变为 "not authenticated"，silent auth 从钥匙串静默捞到旧账号（guijijike@）
  并切换活动账号 → 地区资格失败。触发者为 Hub 测试的 kill 密集 spawn 模式。
- antigravity driver 三层防护：①进程级 `AGY_SPAWN_GUARD` spawn 互斥（槽位持有整个进程
  生命周期，会话进程通过 `_spawn_slot` 字段持有）；②永不中途 kill——probe 与会话结束一律
  `wait()` 回收，`kill_on_drop` 仅作泄漏保险；③认证/资格失败指数退避（`AuthFailure`
  分类 → 60s 起步、上限 4 次幂；probe 首事件即报账号问题也会计入；成功事件清零退避）。
- Claude/ACP driver 同步审计：probe 的 `kill()` 已全部替换为 `wait()`（这些 CLI 同样与
  交互式会话共享凭据存储）。Codex 保持 `kill_on_drop` + 显式 shutdown 流程（其
  app-server 生命周期独立，无共享 token 文件问题）。
- 基线：Rust `140 passed, 20 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：Claude 额度 QuotaProvider 接入（2026-09-04，参照 codex-host）

- 调研结论：stream-json 无按需拉取；`rate_limit_event` 推送是唯一订阅窗口来源（codex-host
  实测形状：utilization 0–1 小数 + resetsAt unix 秒）。`result` 事件携带上下文/成本
  （usage tokens + modelUsage.contextWindow + total_cost_usd）。
- claude-driver：`Classified::PlanLimit`/`Terminal(_, context)` 分类 + `ClaudeQuotaState`
  共享存储（plan_windows + context，所有会话进程写入）；`impl QuotaProvider` 纯推送驱动、
  无观察时 CapabilityUnavailable、1 小时 stale。11 个 lib 测试（新增 3 个形状测试）。
- 桌面：Claude Ready 时注册 Driver + QuotaProvider。本机 Claude 走自定义后端
  （kimi-for-coding），订阅窗口事件可能不出现——UI 将如实只显示上下文。
- 基线：Rust `143 passed, 20 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：Kimi 额度 QuotaProvider 接入（2026-09-04，真实验收通过）

- 发现官方零成本通道：Kimi ACP `available_commands_update` 列出本地命令 `/usage`——向
  session 发送 `/usage` prompt 文本，0.6s 本地返回（"no LLM calls yet"）并先触发结构化
  `usage_update` 推送。简单 Turn 不会主动推送 usage。
- 实现：`Command::ReadKimiUsage` + `kimi_usage_quota`（5s 内新鲜读数跳过命令往返；
  source=native_pull）；非 grok/kimi 的 ACP Agent 走 `context_only_quota`（有真实
  UsageUpdate 观察才应答）。桌面为 kimi 注册 QuotaProvider。
- 真实验收：turn 前 unavailable → turn 后 `used=22252 window=262144`。
- 基线：Rust `143 passed, 21 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：Kimi 订阅额度完整接入（2026-09-06，真实验收通过）

- 补齐订阅窗口通道：`kimi web` 本机管理服务 `GET /api/v1/oauth/usage`（token 从启动 banner
  解析，`--port 0` 随机端口，Drop 时关闭进程——该服务无凭据持久化可安全 kill）。解析
  summary（周窗口）+ limits[]（5h 窗口）：hour/day/week→分钟、used/limit→百分比、
  ISO-8601 reset_at。移除被取代的 `Command::ReadKimiUsage`（/usage 会话命令路径）。
- `read_quota(kimi)`：web 窗口 + session 上下文合并（native_pull，30s TTL），web 不可用
  回退 context-only。真实验收：周窗口 3%/5h 窗口 13% + 重置时间 + context 22313/262144。
- 基线：Rust `144 passed, 21 ignored`、clippy 0、fmt ok、前端 `30 passed`。

## 追加：ZCode Driver 接入（2026-09-07 初版，已由本文置顶和统一矩阵取代）

- 免依赖启动落地：registry 新增 `discovery.env_overrides`，manifest 以
  `ELECTRON_RUN_AS_NODE=1` + App 自带 Electron 运行打包 CLI（无系统 Node 要求）。发现探针
  Installed + 0.16.5（真实通过）。
- 新 crate `zcode-driver`：JSON-RPC 变体（无 jsonrpc 字段）+ 服务端反向请求
  （本段记录初版的 runtimePreferences 内联应答与审批安全 deny；当前审批已由置顶记录取代）；零模型 `probe_ready`（session/list +
  session/create 的 "Model config is missing" → NotConfigured，真实探针验证）；prompt 走
  spawn 任务持连接锁 + 终态映射；resume 保持 Unknown。
- 登录解法（实测）：`app-server` 不接受 `--settings`；正解是操作者跑一次 `zcode login`
  （bundle 文档：login 把最终 API key 写入 `~/.zcode/cli/config.json`；与桌面 `~/.zcode/v2`
  不共享）。Hub 不接触凭据。
- 兼容记录：`docs/compatibility/zcode.md`（协议事实、方法表、未完成项）。
- 基线：Rust `158 passed, 29 ignored`、clippy 0、fmt ok、前端 `35 passed`。

## 追加：ZCode 文本流早期校准记录（2026-09-07，事件分类结论已由本文置顶取代）

- **登录解法（最终版）**：直接复制 v2 → cli 不可行（strict schema 静默拒绝含
  `enabled/source/apiKeyRequired` 的条目）。可行解法是从 v2 provider 提取最小 config（含
  `options.apiKey`）+ 顶层 `model: "provider/model"` 字符串。凭据
  `~/.zcode/v2/credentials.json` CLI 共享读取。API key 只在本机两个配置文件之间复制。
- **事件流架构（早期误判，已纠正）**：所有 Turn 内容包装在 `session/event` 信封内；0.16.5
  实际包含权威 `params.type`。payload 键签名只能兼容缺失 type 的旧变体，不能用于抢先判终态。
- **Turn 阻塞坑**：`interaction/requestOfficialMcpAuthHeaders` 未应答会**永久阻塞 Turn**
  （Agent 等 MCP auth 头，不超时）。必须空对象应答。
- 真实 Turn 验收通过：`text: "OK"`（3 事件，7.9s）。额度：`usage/stats` 本地统计（无订阅
  窗口 API），context 从 `session/read` projection（1M 窗口）。
- Driver 重构：`run_turn()` 独立方法（send accepted 后持续读事件直到权威 Turn 终态）；
  当前 `event_method()` 优先使用 `params.type`，键签名仅作回退。兼容记录见 `compatibility/zcode.md`。
- 基线：Rust `161 passed, 31 ignored`、clippy 0、fmt ok、前端 `35 passed`。
# 最新交接增量（2026-09-09）

非 Git 有界快照、持久化起点和宿主独占锁下的崩溃恢复已实现，真实 Codex→Kimi→Codex 对话接力通过。以 [实现与验收边界](implementation/m1-s5-turn-changes.md) 为准：其他供应商矩阵尚未逐一验证；Codex/Claude 文件修改验收见 Review 交付审计。较早阶段的非 Git 待办已被此实现取代，完整项目备份不属于 Diff 范围。

## 验收环境收尾约定（2026-09-11）

测试使用隔离实例、数据目录与可识别的进程。结束时退出测试应用，并停止本次创建的 Vite/其他开发服务器，核对测试端口已释放；只结束已确认属于本次验收的进程，不结束用户随后启动的新实例。不把历史“开发进程保持运行”说明当作当前事实。本次 1420 残留占用已处理。
