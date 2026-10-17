# Claude 后台工具、认证分类、Pi 取消与服务诊断

日期：2026-09-30。参考已拉取的 codex-host `7c960f12`，按 dogcodes 的 Driver → Core → Store 和独立服务架构实现。

## 本次交付

- **Claude 后台 Bash**：识别原生 `backgroundTaskId` 和 `task_*` 事件，保留工具身份与原始轮次。移交后台时继续显示运行中，不将工具回执当作执行完成；后续完成、失败或中断由原生通知决定。主轮结束后 Core 继续保存带有明确后台标记的工具事件，投影重建保留终态；无关联的普通工具事件仍被过滤。
- **Claude 认证分类**：采用原生 assistant `error` 和结构化结果错误码，识别 `authentication_failed` / `oauth_org_not_allowed`。普通回答提到登录或 API Key 不会变成认证失败，子任务错误不改写主轮认证状态。向上层返回固定提示，不拼接供应商原始错误；每轮终态清空错误状态。
- **Pi 取消**：等待 abort 回执、`agent_settled` 和 `get_state.isStreaming == false` 三者确认后，才发布中断终态并允许复用会话。取消过程最长等待 30 秒；未确认结束或状态矛盾时关闭连接、移除会话，不伪造取消成功。扩展问题先按原生协议取消。
- **服务诊断**：Core 服务独立写入启动阶段、就绪/退出、启动失败及 Rust panic/backtrace，桌面退出后文件仍在。既有“保存问题报告”包含这些日志的有界尾部；导出说明提醒分享前检查内容。

## 日志存储与边界

文件位于应用数据目录的 `diagnostics/core-<pid>-<uuid>.log`。Unix 目录权限为 0700、文件为 0600；使用已有确定性脱敏器，写入前和导出时各处理一次。单条详情最多约 16 KiB，单文件上限 256 KiB，轮转保留一份 `.previous`；按修改时间保留最近 8 份，活动写入文件通过锁保护，可能额外保留。报告最多读取最近两份各 32 KiB 尾部，并丢弃截断的首条记录。

本次记录的是服务自身诊断，不是所有进程 stderr 或生产 Raw Frame Recorder。日志不可写不会阻止服务启动。已有脱敏规则不能保证识别任意私人信息，导出前仍应检查。

## 验证

- Claude Driver：29 项通过，6 项需真实原生环境的测试未运行。覆盖结构化认证码、正文误判、跨轮状态清理、后台工具先回执后启动/结束及原轮次路由。
- Pi Driver：9 项通过，2 项真实原生测试未运行。包含超过旧 5 秒边界的慢回执、仅回执未结束、已结束但仍报告 streaming，以及后续轮次复用。
- Store 集成：1 项通过，分别覆盖后台子 Agent 与 Bash；验证主轮结束后的落盘、过滤无关工具以及重建投影。
- 桌面服务诊断：4 项通过，覆盖脱敏、大小限制、轮转、活动文件保留、空报告，以及子进程 panic 后仍能读取已脱敏记录。
- 启动与问题报告前端：12 项通过；TypeScript/Vite 构建通过。
- 桌面二进制构建通过。真实 `--core-service` 子进程在隔离目录读取损坏数据库时，成功留下启动阶段与失败记录；原数据库字节不变，未发布就绪连接信息。该内部入口沿用既有退出码 0 行为，失败判断以诊断记录为准。
- 首轮 Claude/Pi/Core/桌面的 all-targets Clippy 执行完成；当时严格模式受现有项目命令、运行权限、终端等模块告警阻碍。本次新增代码的告警已修正，既有告警在下述后续清理中解决。

这些是确定性回归及隔离验证，不代替真实 Claude 账号或 Pi 模型调用验收；本次没有修改用户的登录配置。

## 同日后续：既有告警清理

按用户要求修复既有模块告警：合并嵌套条件、显式列出运行权限模块依赖、移除重复错误字符串转换、规范时长和文档格式、用单个字符串缓冲区生成测试数据。终端进程 ID 改为检查范围后转换，测试读取字节也不再使用可能截断的强制转换。没有降低 lint 等级或增加忽略规则，同时修正两处既有 Rust 格式问题。

- `cargo clippy --workspace --all-targets -- -D warnings` 通过，当前 macOS 默认 feature 下全工作区零告警。
- `cargo fmt --all --check` 通过。
- `cargo test -p agent-core -p agent-store -p desktop --lib`：Core 48、Store 48、桌面 111，共 207 项通过；7 项依赖本机安装/网络的显式忽略测试未运行。覆盖项目分析/缓存、终端执行与子进程清理、安装选择、历史 Diff 以及已有存储/服务回归。

本轮没有修改前端，没有进行跨平台编译或真实模型调用验收。

## 后续范围

后台 Bash 的专用终端面板/实时输出读取、会话置顶/归档迁入 Core、消息排队/steering 和会话 watch 未包含在本次工作包中，当前状态继续以 [工作清单](../CURRENT_WORK.md) 为准。

## 2026-10-07：消息边界、ZCode 统计与 Claude 自主续答

参考 codex-host `f7e3ca0a` 的行为修复，按本项目的 Rust Driver → Core → Store 实现。

### 消息边界

Core 按工具身份识别首次出现的工具/子 Agent；首次观察也可以是完成通知。首次出现切分正文，已有工具的进度、重复开始或迟到完成不再拆开正在输出的回答。原生工具 ID 与带会话前缀的展示 ID 共同去重；子任务内部活动不切分主 Agent 正文。普通输入与自主续答使用同一分段器。

### ZCode 统计

每个活跃 Session 保留创建时的完整模型目录；冷恢复通过已有控制面目录读取获取独立目录，避免 session/read 对当前模型窗口的覆盖。按 providerId/modelId 读取实际所选模型的 contextWindow，目录缺失时保留原生 projection 回退。支持 settings.model.available 与旧 workspace/readState 的 modelCatalog.providers 结构。

优先读取 runtime.contextUsage.used；缓存读取/写入 token 和命中率来自 runtime.contextUsage.cache。移除按 cachedMessages/totalMessages 推算 token 命中率的逻辑。缺失指标保持未知，越界比例不展示；轮末指标读取失败不改写已确认的执行结果。

### Claude 自主续答

- Driver 提供一次订阅的 Session 级自主事件流。后台任务通知本身不开轮；主 Agent 的正文、工具/思考输出或审批请求开始自主轮。原生未提供轮次 ID 时保持未知，由 Core 分配独立 Turn/Run 身份。
- Core 持久化 `turn.autonomous.started`，复用运行状态、审批、取消、历史展示、上下文引用和恢复投影；不创建用户消息、不发送合成提示词。后台工具结果继续归属其原轮次。
- 新消息与自主输出使用原子状态/输出通道准入。原生 result 到达后，仍等待 Core 保存终态再放行用户输入；多个已发生的续段分别确认。若续答在命令校验后抢先开始，新消息以 `autonomous_turn_busy` 结束并提示尚未发送，不替换原生会话，不重放输入。
- 只有原生确认的中断且 Driver 支持活跃连接复用时，取消后的 Session 才保持 active；流关闭/错误不会伪造成功。孤立自主 Run 的服务重启恢复沿用 connection_lost。
- 续答的工作区变更记录覆盖 Core 开始观察到结束的区间；原生已经开始执行后才收到通知，不能据此声称捕获了精确的执行前快照。自主续答没有新建主对话历史 MCP lease，既有 lease 生命周期保持原规则。

### 验证范围

确定性用例覆盖正文迟到工具事件、工具身份别名、子任务隔离；模型窗口切换、原生缓存统计与未知值；Claude 实时续答、取消、确认终态后复用；Core/SQLite 的续答审批、竞态提交、连续续段、后台结果归属、EOF 失败、重启恢复和投影重建。

未发起真实模型调用；真实 Claude 后台续答、ZCode 已安装版本和 GUI 链路仍需环境验收。本次不修改登录配置或 Agent 安装。

验证命令与结果：

- `cargo test -p driver-api -p agent-core -p agent-store -p zcode-driver -p claude-driver --lib -- --test-threads=1`：176 项通过，16 项真实环境测试保持忽略。此前并行运行中既有 Claude 关闭进程用例出现一次 3 秒超时，单独重跑和串行全组均通过。
- `cargo check --workspace` 通过，包含桌面主进程。
- 上述五个 crate 的 `cargo clippy --all-targets` 完成；新增文件无告警。严格 `-D warnings` 受已有 `driver-api/src/usage.rs` 等模块告警阻碍，未为此降低 lint 等级或修改无关模块。
- `git diff --check` 通过。新增 Rust 文件已格式化；未把工作区既有格式差异带入本次变更。

### 同日跟进：Clippy 告警清理

补齐用量统计 API 的 `#[must_use]`、锁中毒 panic 文档与文档代码标记，统一相关测试中的长数字分隔符；合并 ACP/Antigravity 的嵌套条件；文件快照的 64 KiB 读取缓冲区改为堆分配。桌面 Git 操作从现有窗口读取托管状态，减少一个注入参数，前端命令参数保持兼容。

`cargo clippy --workspace --all-targets -- -D warnings` 已通过，消除了上文记录的严格检查阻碍；当前 macOS 默认 feature 下全工作区零告警，未新增 lint 豁免或降低规则等级。

回归验证：

- `cargo test -p driver-api -p acp-driver -p antigravity-driver -p agent-workspace --lib -- --test-threads=1`：223 项通过，34 项显式忽略测试未运行。
- `cargo test -p desktop --lib review:: -- --test-threads=1`：19 项通过，覆盖 Git 提交预览校验及审查行为。
- `git diff --check` 通过。

## 2026-10-08：Claude 后台取消与用量入口

参考 codex-host v0.13.1（`08c7d65d`），保持本项目原生 Driver → Core → Store 分层。

- Claude 的原生 Agent/Workflow 任务占用由 `task_started`/`task_progress`、后台任务列表及结构化异步工具结果跟踪；重复或迟到的终态/开始不复活已完成任务。主回复 `result` 到达后，有后台 Agent/Workflow 或子任务审批时不提前提交轮次终态。普通后台 Bash 仍保持原先的跨轮呈现语义。
- 主回复终态只关闭主回复审批，子任务审批保持可答；原生撤回、所属任务结束及用户取消都会失效并拒绝迟到回复。新增 `ApprovalClosed` 映射到已有 `approval.expired`，复用 SQLite 待审批投影与重建。Workflow 原生进度提供的 agentId 用于关联其审批归属，不新增 Ultracode 开关或 Workflow 专用界面。
- 取消对本轮已知后台任务发送 `stop_task`，对主回复发送 `interrupt`；控制请求 ACK 不等于任务结束。取消过程中新增任务继续停止，再次出现主回复则继续中断。所有本轮任务和主回复结束后观察 200 ms 静默再提交中断终态；超过 3 秒未确认则升级为关闭本会话进程组。强制关闭会终止该会话进程组内的其他后台工作，不影响其他会话的进程组。
- Unix 独立进程组先 TERM、后 KILL，并回收根进程、确认组消失；macOS 的已退出未回收进程可能返回 EPERM，回收后再次检查，不把 EPERM 本身当成关闭成功。Windows 实现使用根进程仍存活时的 `taskkill /T /F`，本轮未做 Windows 运行验收。无法确认关闭时报告失败并保留不可复用状态；确认关闭后仅通过已有原生 resume 能力重建连接。stdout 使用可取消安全的逐行读取，取消计时不会丢失半行帧。
- 用量按钮不再依赖可计算的上下文百分比：Token、费用、Credits、窗口或有效缓存指标任一已知即可展示；百分比未知显示“用量”，提示中明确“上下文已用：未知”，不绘制虚假的占用比例。仅有时间戳时不显示。ZCode 与其他 Agent 统一使用“最新 token 缓存命中”；存在原生用量快照时，不回退旧 context 的缓存率。

验证使用本地协议子进程和进程组、Core/SQLite 及前端确定性用例；未调用真实模型，未切换或启用原生 Workflows 设置。`stop_task` 的控制载荷与本机 Claude Code 2.1.289 内嵌 SDK 实现核对。

验证结果：

- `cargo test -p claude-driver -p agent-core -p agent-store --lib -- --test-threads=1`：Claude 40、Core 53、Store 54，共 147 项通过，6 项真实环境测试保持忽略。新增用例覆盖 ACK 不代表停止、取消中续答与迟到审批、强制关闭后 resume、子进程组隔离、原生撤回及审批过期的 SQLite 投影重建。
- `pnpm --filter ./apps/desktop exec vitest run src/ContextUsageRing.test.tsx src/Tooltips.test.tsx src/tooltipConvention.test.ts`：16 项通过；桌面 `typecheck` 通过。
- 全前端测试入口曾意外运行完整套件：661 项通过、5 项失败。随后在 HEAD 前端源码的独立临时副本中重跑对应五个文件，复现同样五项失败（AppStreaming、ConversationDeletion、ConversationRendering、ReviewStream、windowChrome），与本次两项修改无关；未改动这些文件或放宽断言。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过；`git diff --check` 通过。
