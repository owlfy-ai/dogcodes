# 子智能体可观测性调研

> 后续已完成七个 Agent 的真实委派探测，见 [实际协议探测报告](subagent-observability-live-2026-09-11.md)。下文保留静态调研记录；Cursor、Grok 等结论以实测报告为准。

日期：2026-09-11。范围：dogcodes 当前集成的 Codex、Claude Code、ZCode、Cursor、Kimi Code、Grok Build、Antigravity。

## 结论与证据标准

不能把“Agent 能派生子智能体”直接等同于“当前接入能显示完整子任务树”。分别判断关系、生命周期、内部事件、历史读取、实时订阅。

本次完成官方文档核对、本机程序版本读取、Codex schema 生成、安装包静态代码/符号检查、项目驱动检查。没有发送模型 prompt、创建子智能体或读取用户历史对话。本报告中的“支持”表示已找到协议或实现证据，不表示完成了真实 Turn 验收。

- **强证据**：本机生成 schema，或可读安装包中的 schema、路由和映射实现。
- **文档证据**：官方明确描述对外方法/事件。
- **线索**：编译二进制的字符串和符号；能证明存在相关实现材料，不能证明确切 wire 形状、发送条件和权限。
- **未确认**不等于不支持；**未映射**不等于供应商不提供。

| Agent / 本机版本 | 当前接入 | 父子关系 | 子任务状态 | 内部过程 | 最强证据 |
|---|---|---|---|---|---|
| Codex 0.153.4 | app-server | 明确支持 | 明确支持 | 子 thread 历史可读；实时子流订阅需验收 | 本机 schema + 官方文档 |
| Claude Code 2.1.266 | stream-json | 明确支持父 tool 关联 | 有开始/进度/结束消息 | 可关联子消息、工具事件；后台/嵌套覆盖需验收 | 官方文档 + 本机符号 |
| ZCode 0.16.5 | 原生 app-server | 明确支持 | 明确支持 | 子工具映射字段、子会话消息/事件接口存在；转发覆盖需验收 | 本机 schema + handler |
| Cursor 2026.09.08-6caf4ff | ACP + cursor 扩展 | 委派调用可识别；稳定 child ID 不完整 | 委派结果/耗时可得；后台生命周期不足 | 当前 ACP 映射未找到内部子流，TUI 内部拥有 | 官方文档 + 本机 ACP 映射 |
| Kimi Code 0.41.0 | ACP | Agent 调用可识别；无直接子树映射 | 委派工具状态/结果可得 | 本机 ACP 只订阅 main，未直接暴露子过程 | 本机可读 ACP 实现 |
| Grok Build 1.0.25 (f7e67d6988e2) | ACP + x.ai 扩展 | 有明确字段线索，待 wire 确认 | 有 spawned/progress/finished 线索 | 统计有线索，完整子流/历史未确认 | 本机二进制字符串 |
| Antigravity 1.2.0 | headless stream-json | 文档明确 parent step → child conversation | 有步骤状态；独立 child 生命周期覆盖未确认 | 返回 log_uri；主流完整转发未获证明 | 官方 headless 文档 |

版本均为本次本机读取；Cursor 使用当前 symlink 的目标目录。它们与仓库部分早期兼容性记录不同，不应使用旧记录推断当前协议。

## Codex

本次用 `/Applications/ChatGPT.app/Contents/Resources/codex app-server generate-json-schema --out ...` 生成安装版本 schema，未启动模型。

`ThreadReadResponse` 引用的 `ThreadItem` 明确包含：

- `collabAgentToolCall`：`tool`、`senderThreadId`、`receiverThreadIds`、`agentsStates`、`status`、可选 prompt/model/reasoningEffort。schema 明确说明 spawn 时 receiver 是新建子 thread。
- `subAgentActivity`：`agentThreadId`、`agentPath`、`kind`。它是活动标记，不能把它当成子 Agent 全部日志。
- `CollabAgentStatus`：pendingInit / running / interrupted / completed / errored / shutdown / notFound。

获取路径：以 spawn 操作建立父子边，使用 agent 状态更新卡片；通过 `thread/read(includeTurns:true)` 读取已持久化子 thread 的内部过程。官方明确 thread/read **不会订阅**事件。实时展示必须另行验证子 thread 的加载/订阅方式和父连接是否自动收到子事件，不能通过反复 resume 猜测。

新官方文档还有 experimental parentThreadId/ancestorThreadId 筛选；它们不是所有安装版本的稳定保证。普通 forkedFromId 也不能无条件当委派父子关系。

项目差距：`codex-driver/src/lib.rs::map_tool_item` 已认识这两种 item，但只保留工具摘要。必须保留 sender/receiver/agent status；当前将 subAgentActivity 当 ToolStarted/Completed 的语义也需要重新审视。

来源：[官方 App Server 文档](https://learn.chatgpt.com/docs/app-server)。本机生成 schema 的字段结论优先于在线文档的命名变化。

## Claude Code

官方明确：`tool_use.name` 为 Agent（旧版 Task）时代表调用子 Agent；来自子上下文的消息包含 `parent_tool_use_id`，可关联父委派调用。因此可以归属子正文、工具请求和结果，而不是只显示一条“执行子任务”。[子智能体检测](https://code.claude.com/docs/en/agent-sdk/subagents)。

生命周期接口包括 `system/task_started`、`system/task_progress`、`system/task_notification`，另有 `tool_progress`。task_started 可带 task_type，必须排除 local_bash 等普通后台任务；不要把所有后台任务当子 Agent。task_progress 可报告 task_id、tool_use_id、耗时、工具次数、last_tool_name，摘要是有条件字段。[SDK 类型参考](https://code.claude.com/docs/en/agent-sdk/typescript)。

本机 2.1.266 程序中能找到上述事件名与 parent_tool_use_id。官方 headless 变更记录也明确 -p 模式会发子任务消息。[官方变更记录](https://platform.claude.com/docs/en/release-notes/claude-code)。这支持继续使用当前 stream-json，无需仅为渲染改走 SDK；仍需针对本机流确认事件包装位置和后台覆盖。

父调用 ID 与持久 agent ID 不同。官方说明某些 Agent 结果文本会包含 agentId，但 Explore/Plan 不保证提供可恢复 ID。第一版可以按 parent_tool_use_id 建立“子执行”，不依赖所有子 Agent 都有持久 session ID。

项目差距：当前 driver 未把 parent_tool_use_id、task 生命周期、子工具进度带到统一事件，多个文本流使用固定 message ID，存在后续接入时父子内容混排的风险。

## ZCode

这是对前次“未确认”结论的更新：本机 `zcode.cjs` 中不仅有字段，还有路由处理函数。

1. `session/subagents`：参数 sessionId、endedCursor、endedLimit；结果 revision、childSessionIds、running、ended 分页。
2. 子任务记录：childSessionId、agentId、toolCallId、subagentType、title、summary、startedAt、endedAt。
3. 活跃状态：running / waiting / blocked；终态：success / failed / cancelled / lost。
4. session schema：parentSessionId；sessionKind 包含 subagent_child，能避免把普通 fork 一律当子 Agent。
5. tool.updated 公共字段：parentToolCallId、source=subagent、agentId、agentType、childSessionId、childToolCallId、description；事件种类覆盖 scheduled/started/progress/result/error。
6. progress 可含 elapsedMs、stdoutTail、stderrTail、字节计数；result 带 result 与 duration。
7. `session/messages`、`session/events`、`session/subscribe` 路由存在。subscribe 支持 afterSeq 和 includeSnapshot。

静态处理路径：session/subagents handler 从父会话消息/事件找子会话，检查 taskType=subagent_child，读取子消息与运行中 projection 后生成列表；不是空 schema。工具映射辅助函数 `G0n` 保留父/子调用与会话字段。

边界：session/subscribe 通过活动 session 查找函数获取上下文，不能假定任意历史 childSessionId 直接订阅成功；需检查子会话是否已加载、如何安全附着。父流是否转发全部子工具和文本、后台子任务是否持续推送，都还需要真实 wire 验收。

项目差距：当前只抽取 toolCallId/toolName/kind，丢掉关系与进度；没有接 session/subagents。可先做子任务列表+状态，再接按需内部详情。

本机证据文件：`/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs`；定位词 `sessionSubagents`、`IEt`、`pKi`、`QB`、`mkn`、`function c3e`、`function Zpn`、`function G0n`。这些是本版本压缩符号，不应硬编码到产品逻辑。

## Cursor

官方 ACP 扩展 `cursor/task` 描述委派任务，含 toolCallId、description、prompt、subagentType、可选 model/agentId/durationMs。[官方 ACP 文档](https://cursor.com/docs/cli/acp)。

本机实际映射比字段列表更有约束：

- ACP `taskToolCall` 的 rawInput 有 `_toolName:task`、prompt、description、subagentType。
- rawOutput 在成功时只映射 durationMs、isBackground；失败映射 error。
- cursor/task 扩展的 agentId 取自调用参数，它可能是“恢复哪个子 Agent”的输入，不能假设是新建子 Agent 的结果 ID。
- 通用 toolCallId 能与主 ACP session 建立委派边，但不是独立 childSessionId。
- 官方称该扩展为 notification；本机 `sendNonBlockingExtensionNotification` 实际调用 connection.extMethod 并异步 catch。驱动需要按收到的 wire 是否带 id 正确应答，不能只凭文档标题忽略反向请求。

内部过程：同一安装包的 TUI 路径确有 onSessionCreated / onInteractionUpdate / onSessionCompleted，保存 parentToolCallId、子会话 uiState、状态。这证明 Cursor 自己可见，不证明 ACP 暴露。当前 ACP 模块没有找到该子 interaction 流的转发和关联字段；暂判**委派可见，完整内部过程不可直接依赖**。

后台特别注意：isBackground=true 的 task tool 成功，只代表后台任务已启动，不代表子 Agent 已完成。

本机定位：`~/.local/share/cursor-agent/versions/2026.09.08-6caf4ff/7578.index.js`（ACP），`990.index.js`（TUI 子会话），`190.index.js`（transcript path）。官方提及子 transcript 也不等于当前 ACP 支持读取；本次未读取用户 transcript。

## Kimi Code

本机 0.41.0 是包含可读 JS 的可执行包。运行时内部信息十分完整：subagent.spawned 带 subagentId、parentToolCallId、可选 parentToolCallUuid/parentAgentId/callerAgentId、runInBackground/model/taskId；还有 started/suspended/completed/failed。内部 roster 和逐 Agent 事件支持构建父子结构。

但 **当前 ACP 外层明确只接 main**：AcpSession 构造函数使用 `this.session.agent("main")`；init 订阅 `this.agent.events` 的 assistant/thinking/tool 事件。源码注释明确子任务 terminal 可能存在，但由于此 session 只跟随主 Agent 事件，不会附着到对应子工具。

ACP 能显示主 Agent 的 Agent 委派调用、参数、结果及通用工具状态；没有发现把 subagent.spawned 等生命周期或子正文/工具转成专门 ACP 更新。子 ID 如果只存在于结果文本，也不能据此宣称拥有结构化父子树。

可选后续路径：调研其原生会话服务/逐 Agent 事件接口，用独立可观测性通道增强 ACP；本次发现内部能力，不把内部类方法当成已稳定公开的服务 API。标准 session/load 加载主会话也不自动解决子流问题。

来源：[官方 ACP 入口](https://moonshotai.github.io/kimi-code/en/reference/kimi-acp)、[官方 Agent 工具说明](https://moonshotai.github.io/kimi-code/en/reference/tools.html)。核心通道限制依据本机 `~/.kimi-code/bin/kimi` 中 AcpSession、toolCallStartToSessionUpdate 等实现。

## Grok Build

官方确认子 Agent 是独立 child session、结束后向父会话返回摘要；官方支持 ACP，但未在查阅页面给出完整子任务扩展契约。[子智能体](https://docs.x.ai/build/features/subagents)、[ACP/headless](https://docs.x.ai/build/cli/headless-scripting)。

本机 1.0.25 二进制发现更丰富的线索：

- `x.ai/session/update` 与 XaiUpdate 类型。
- subagent_spawned / subagent_progress / subagent_finished。
- parent_session_id / child_session_id / subagent_id / attempt_id。
- turn_count / tool_call_count / context_window_tokens / tools_used / error_count。
- task 扩展相关字段 parentSessionId / childSessionId / failureError / cancelReason。
- 独立 `x.ai/task_backgrounded`、`x.ai/task_completed` 及接收端解析提示。

因此应该优先核验其 **供应商 ACP 扩展**，不能继续笼统归为“ACP 只有普通工具”。但这是编译符号证据；本次没有拿到可验证 schema 或真实 frame，字段所属事件、嵌套位置、下划线前缀、前后台分支都暂不能宣称确定。

内部过程目前只找到统计/状态线索；toolCalls 字段名不足以证明完整输入输出，更不足以证明逐 token 子流。标准 session/load 是否能附着 child_session_id、是否触发恢复而改变正在运行的会话，需要专门验收。

当前项目 ACP driver 对未知扩展基本未转成产品事件；实现时应在 typed enum 解析之前接供应商通知，以免未知 sessionUpdate 丢失或解析失败。

## Antigravity

官方 headless 文档明确：派生步骤用 subagent_info，其 subagents 数组包含 type_name、role、conversation_id、log_uri、workspace_uris。用外层父 conversation_id + step_index 对应子 conversation_id，可以建立一对多委派关系。[官方 headless 文档](https://antigravity.google/docs/cli/headless/)。

步骤有 state、duration，但文档列出的 subagent_info 字段没有证明每个 child 都有独立结束推送。主派生 step DONE 不能无条件视为所有异步 child 完成。

官方 TUI 的子任务面板能看到完整对话、步骤和工具日志；这仅证明产品内部拥有数据。[官方 CLI 功能](https://antigravity.google/docs/cli-features)。headless 返回 log_uri，说明有详情定位信息，但它是否是本地文件、读取权限、格式、增量刷新与保留期限仍需核验。不能假定主 stdout 会混入全部子事件。

当前 `antigravity-driver` 的 classify 明确忽略 subagent/checkpoint 内容。第一步先保存关系及 log_uri 作为内部资源引用；完整子流能力暂标 Unknown。

## 对项目设计的影响

建议分开记录：relation、lifecycle、progressSummary、liveTranscript、historyTranscript，而不是单个 subagentSupported 布尔值。每项记录证据等级和安装版本。

接入顺序：

1. Codex、Claude、ZCode：可先设计真正的子执行实体，明确 native child ID 与 parent tool ID 的区别。
2. Cursor：先做委派卡片、类型、耗时、结果与后台标记；新建子 ID 缺失时保留调用级身份。
3. Antigravity：先接父步骤与子会话列表，再确认状态/日志通道。
4. Grok：先保留并验证 x.ai 扩展契约，已有字段线索值得优先补验。
5. Kimi：ACP 先做委派摘要；完整子过程需要额外原生可观测性通道，不能只改 UI。

所有通道都要区分：创建子任务成功、子任务正在执行、子任务执行完成、父 Agent 收到结果。多次 resume 同一个 child ID 是多次执行，不应重复创建多个子 Agent 节点。生命周期证据不足显示“状态未知”。

## 后续真实协议验收范围

本次调研未做以下运行验收，不能将其标记为完成：

- 前台 1 子任务：父工具 ID、子身份、至少 1 次读文件工具的输入与结果、最终状态。
- 后台 1 子任务：创建返回后是否持续上报，主轮结束后是否还能跟踪。
- 并行 2 子任务：事件交错时父子归属、相同工具名不串线。
- 子任务失败/停止/恢复：状态来源、重试 attempt 与 child 身份。
- 历史与断线：只读查询是否可恢复关系、有没有原始过程、订阅游标如何补齐。

验收应在独立小目录使用固定无敏感文本，记录脱敏 frame；需要实际模型调用的阶段单独执行。本次未修改产品代码。
