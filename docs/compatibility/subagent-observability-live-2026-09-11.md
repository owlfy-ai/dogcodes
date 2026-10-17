# 子智能体可观测性：实际协议探测

执行时间：2026-09-10 23:20 起（America/New_York），UTC 2026-09-11。这是实际发送模型请求、派生子智能体后的结果，不是文档能力推断。

## 方法与结论

通过本机七个 Agent 当前集成使用的原生接口执行同一类任务：主智能体派生一个子智能体，由子智能体读取隔离目录中的随机 marker 文件并计算 19+23，主智能体等待结果。marker 不出现在初始 prompt 中。记录双向 JSON 帧；仅查询本次创建的子会话历史或协议返回的本次日志路径。没有改动产品代码。

六个 Agent 返回了正确 marker 和 42。Antigravity 创建了子智能体，但文件读取被权限机制拒绝，未完成结果验收。结果只代表本机版本、本次单子任务和本次协议连接；没有覆盖多子任务并发、后台运行、多层嵌套、重连或异常恢复。

| Agent / 本机版本 | 实际接口 | 父子关系 | 状态 | 实际拿到的内部过程 |
|---|---|---|---|---|
| Codex 0.153.4 | app-server | 明确：子 thread ID；历史含 parentThreadId | 活动事件、子 turn 及工具完成状态 | **历史查询可得**：实际 cat 命令、输出、退出码、子回复；本次未验证自动实时转发子工具流 |
| Claude Code 2.1.266 | stream-json | 明确：task_id、tool_use_id、parent_tool_use_id | started / progress / completed | **实时可得**：子 Read 调用、参数、tool_result、marker |
| ZCode 0.16.5 | app-server | 明确：agentId、subagentSessionId、parentToolCallId 等 | 子工具 scheduled / started / result；主 turn 成功 | **实时可得**：带 source=subagent 的 Read 参数与成功输出 |
| Cursor 2026.09.08-6caf4ff | ACP + cursor/task | 明确：委派 toolCallId 对应非空 agentId | 委派 pending / in_progress / completed、耗时 | **本次只有委派与结果**：未收到子 Read 工具步骤 |
| Kimi Code 0.41.0 | ACP | 委派可识别；agent_id 从结果文本解析 | 委派工具结束及结果文本 completed | **部分可见**：客户端收到文件读取 RPC，但无明确子归属；没有子工具事件流 |
| Grok Build 1.0.25 | ACP + _x.ai/session_notification | 明确：parent_session_id、child_session_id、subagent_id | spawned / progress / finished | **实时可得**：独立子 sessionId 的 Read 调用与输出，还有进度统计 |
| Antigravity 1.2.0 | headless stream-json | 明确：父 step → child conversation_id + log_uri | 派生步骤状态可见；本次未验证子执行成功 | **日志部分可得**：读到了子 view_file 请求；读取被拒绝，无成功工具输出 |

“内部过程”指协议公开的工具调用、输出、消息和状态，不保证完整内部推理。原生运行时版本与底层模型应分开记录：例如本次 Claude Code 子消息报告模型为 kimi-for-coding，测试证明的是 Claude Code 传输与编排能力。

## 关键实际证据

原始采集目录：`experiments/subagent-observability/runs/20260910-232031/`。每个 Agent 包含 fixture.txt、wire.jsonl、summary.json、evidence.json。evidence.json 保留精选事件及 wireLine，可定位到原始帧。原始记录只作本地调试材料，不直接嵌入产品界面。

### Codex

父 thread：`01a08e7b-2bbe-7b23-9575-39bdaef6db74`；子 thread：`01a08e7b-5bd4-7d53-8c05-fa5dc132a0e5`。

`subAgentActivity.agentThreadId` 返回了子 ID，agentPath 为 `/root/read_fixture`。本次 wait 的 `collabAgentToolCall.receiverThreadIds` 为空，因此不能只靠 receiverThreadIds 建树。

另起 app-server，仅调用 `thread/read`，传子 thread ID 和 `includeTurns:true`，成功读取一个子 turn，见 `codex/child-history.json`：

- `parentThreadId` 及 `source.subAgent.thread_spawn.parent_thread_id` 指向父任务。
- `commandExecution` 含读取 fixture 的 cat 命令，commandActions 分类 read，status=completed，exitCode=0。
- aggregatedOutput 含正确 marker；agentMessage 含 marker 和 42。

没有通过 resume 触发模型继续运行。该实验验证了历史读取；不能据此宣称已验证子工具实时订阅。`item/completed` 携带 `subAgentActivity.kind=started` 时，完成的是活动条目，不代表子智能体完成。

### Claude Code

wire 第 170 行 `task_started`：task_id=`a39373663c5728174`，tool_use_id=`tool_KfbwGbmjsXKdf9ol3rQfd4r8`，spawn_depth=1，is_backgrounded=false。

子 assistant Read 调用和 user tool_result 都携带 parent_tool_use_id，可以明确挂到该委派调用；tool_result 包含 fixture marker。第 178 行 `task_notification` status=completed，tool_uses=1，输出 marker 和 42。中间 `task_progress` 给出 last_tool_name=Read。

### ZCode

父 session：`sess_a01a8d18-df84-4938-852a-25edc9956079`；子 session：`sess_subagent_agent_f1f0fdb7-f1a6-46a0-86b0-07326a9afda5`。

`session/event` 的 tool.updated 携带 `source=subagent`，以及父调用、子会话和子工具关联字段；采到 Read 的输入、开始及成功结果，内容包含 marker。第 553 行，38.136 秒，原生 `turn.completed` 的 resultType=success，返回 marker 和 42。

**探针自身问题：**运行中的初版脚本错误地在 payload.type 检查结束类型，实际应检查 params.type，因此原始 summary.json 240 秒后记录 TimeoutError。原始记录保持不变，evidence.json 已注明；这是探针等待超时，不是 ZCode 任务失败。脚本已修复。本轮未完成 session/subagents 额外查询，不能把静态支持当成已验收。

### Cursor

wire 第 36 行实际收到带 id=0 的反向 RPC `cursor/task`，本次不是无 id 的通知。它给出 agentId=`64d24aa4-b591-42eb-a5b6-e6a9e50cb840`，与委派 toolCallId 关联，durationMs=13952。

ACP 更新中只有一个 Task 工具实体，状态完成；没有子 Read 实体。最终文本是多个增量拼接后才出现完整 marker，校验正确，sum=42。该结果修正静态调研中“child ID 不完整”的判断：**本次非空 child ID 已实证**，但独立子过程仍未从此连接拿到。

### Kimi Code

ACP 只有一个 Agent 工具实体；大量增量是同一调用参数更新，不能渲染成多个工具。

客户端收到真实 `fs/read_text_file` 请求并返回 fixture 内容，但其 sessionId 仍为主会话，缺少 child ID 或 parent tool 关联。只有一个子任务时可推断用途，多任务时不能据此可靠归属。

委派结果文本包含 agent_id=agent-0、actual_subagent_type=explore、status=completed、stop_reason=completed、marker、sum=42。这提供应用层解析线索，但不是统一 ACP 父子关系字段。可以展示委派卡和结果；不要把无归属文件 RPC 当成完整子时间线。

### Grok Build

wire 第 90 行 `_x.ai/session_notification` / subagent_spawned 明确连接父会话 `01a08e7c-7cb8-78d0-afb0-ad2c5397a307` 与子会话 `01a08e7c-b8ae-7733-af56-0c1eaf8328d6`。

第 129、131、133 行标准 `session/update` 的 sessionId 为子会话，依次包含 read_file 调用、路径参数和 completed 输出，marker 正确。

第 197 行 subagent_finished：status=completed、tool_calls=1、turns=1、duration_ms=3752，输出 marker 和 42。subagent_progress 另含 tools_used、tool_call_count、tokens_used、error_count。

因此本次确认它不只是提供统计，还实时发送子工具流。实现时需要同时处理厂商扩展与按 sessionId 分流标准事件。

### Antigravity

父会话 `6577a760-449f-4beb-b484-cb6779c1ee96` 的 step_index=2 / invoke_subagent 返回子会话 `c696e225-4a71-453a-9929-927894c80b9e` 和本次 log_uri。

读取该精确路径成功，副本见 `antigravity/child-transcript.jsonl`，其中有子智能体请求 view_file 读取 fixture 的记录。但父 result 的 denied_actions 包含 read_file / ViewFile，没有 marker 和子工具成功结果。

父 result.status=SUCCESS 仅说明父轮次返回；invoke_subagent 的 DONE 也仅说明派生调用结束。两者都不能作为子任务成功依据。本次没有绕过权限重试，因此“成功执行后的完整日志”和“子状态持续订阅”仍待验证。

## 对 dogcodes 渲染的直接影响

1. 将关系、生命周期、实时工具流、历史读取四项能力分开声明，避免简单 supportsSubagents 布尔值。
2. Claude、ZCode、Grok 可以优先实现可展开子任务卡，内部复用现有读文件、终端、搜索等分类。Codex 增加按子 thread 查询历史的详情入口，并明确实时能力尚待验证。
3. Cursor 本次可提供子任务名称、执行状态、耗时和最终结果；Kimi 先提供委派状态与结果。没有内部事件时明确显示“当前接口未提供执行详情”。
4. Antigravity 使用返回的子日志建立详情来源，但派生调用 DONE 和父 SUCCESS 都不能折算为子完成；读权限失败应单独显示。
5. 工具状态按 provider + sessionId + toolCallId 更新，同一调用的参数增量不新增行。父工具关联、子会话关联应独立保存。
6. 耗时优先用供应商子任务 duration；用本地时间测量时标识为界面观测耗时。不要把父任务等待时间、派生工具耗时、模型思考时长混为一谈。

## 复现材料与验证边界

- `experiments/subagent-observability/probe.py`：原生 JSON 协议探针；再次运行会实际调用模型并创建新任务。
- `read_codex.py`：只读取 probe wire 发现的子 thread 历史。
- `collect_evidence.py`：离线提取事件与 marker 校验；没有模型调用。
- 三个脚本通过 Python 语法编译；本轮真实 wire 验证如上。修正后的探针没有重新对七个模型重复运行。

本次测试没有经过 dogcodes 的归一化驱动，所以证明的是供应商输出可用性。产品是否正确保留这些字段、重连是否重复渲染、历史加载是否完整，需要下一阶段驱动接入测试。
