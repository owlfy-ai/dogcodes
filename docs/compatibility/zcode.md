# ZCode（智谱）兼容记录

> 跨 Agent 的当日验收状态、统一门槛和可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只维护 ZCode 协议事实与能力边界。

更新日期：2026-09-07（文本流校准完成，真实 Turn 通过）

## 当前结论：Driver 已接入 Beta；真实文本流验收通过

- 安装形态：ZCode 桌面客户端（Electron）。CLI 以 Node 脚本形式打包在 App 内：
  `/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs`（0.16.5），**无独立 PATH 命令**。
- **免 Node.js 依赖启动**：用 App 自带 Electron 以 node 模式运行 CLI——
  `ELECTRON_RUN_AS_NODE=1 <App>/Contents/MacOS/ZCode <...>/zcode.cjs <cmd>`（实测内嵌 node
  v24.14.0）。manifest 经 registry 的 `discovery.env_overrides` 声明该变量；安装了桌面
  App 即可用，与 Antigravity/Claude 同等"装了本体即可用"。
- 协议：`zcode.cjs app-server` 是 **JSON-RPC 变体**（NDJSON；有 `id/method/params/result/
  error` 与 -3260x 错误码，但**拒绝 `jsonrpc` 版本字段**——发送即 zod 校验错误）。服务端会
  **反向发起请求**，客户端必须应答否则 Turn 阻塞。

## 登录解法（实测，2026-09-07 最终版）

CLI 路径要求 `~/.zcode/cli/config.json`（显式模型供应商）；桌面 App 用**另一份**
`~/.zcode/v2/config.json` + `credentials.json`（0600，OAuth），二者默认不共享。

**直接复制 v2 → cli 不可行**：v2 provider 条目含 `enabled/source/apiKeyRequired` 等字段，
触发 strict schema 静默拒绝（整个文件被丢弃，`session/create` 仍报 Model config missing）。

**可行解法**：写**最小 config**，从 v2 provider 条目中提取必要字段（含 `options.apiKey`）：

```json
{
  "model": "bigmodel-coding-plan/GLM-5.3-Flash",
  "provider": {
    "bigmodel-coding-plan": {
      "name": "BigModel - Coding Plan",
      "kind": "anthropic",
      "options": { "apiKey": "<从 v2 复制>", "baseURL": "https://open.bigmodel.cn/api/anthropic" },
      "models": { "GLM-5.3-Flash": {} }
    }
  }
}
```

顶层 `model` 是 `"provider/model"` 格式字符串（或 `{main: "...", lite: "..."}` 对象）。
凭据 `~/.zcode/v2/credentials.json` 被 CLI 共享读取，无需额外复制。API key 只在本机
两个配置文件之间复制，Hub 不解析不外传。

## 协议事实（全部实测）

| 方法 | 结果 |
|---|---|
| `session/list` | ✅ 返回真实会话数组（sessionId/title/status/workspace/mode） |
| `session/create`（`workspace{workspacePath, workspaceKey}`） | ✅ 成功；session id 在 `result.session.sessionId`（`projection.sessionId` 恒为 `"unknown"`） |
| `session/subscribe`（`{sessionId, deliveryKind: "desktop-continuous"}`） | ✅ 返回 `{eventSeq, events, sessionId}`；之后所有通知经此订阅到达 |
| `session/send`（`{sessionId, content: string}`） | ✅ **只返回 `{accepted:true}`**，不是 Turn 终态；必须继续读事件直到终态 |
| `session/stop` | ✅ 发送取消请求 |
| `session/close` | ✅ 关闭活动会话；有消息的会话仍作为历史记录保留 |
| `session/read` | ✅ 返回 projection（contextUsed/contextWindow） |
| `usage/stats`（`range: all\|7d\|30d`） | ✅ 本地 agent-db token 统计（零模型调用）。**不是订阅额度窗口** |
| `--settings` 参数 | ❌ `app-server` 子命令不接受（Unknown option） |

## 事件流架构（2026-09-07 实测，关键发现）

**所有 Turn 内容到达时包装在 `session/event` 信封内**。`params.type` 是 0.16.5 的权威事件类型，
payload 键签名只作为旧变体缺少 type 时的回退。工具 result payload 自身也可能含 `resultType`，
因此仅凭键签名会把工具结果误判为 Turn 终态。

```json
// 一条真实的 session/event 通知
{"method": "session/event",
 "params": {
   "deliveryKind": "desktop-continuous",
   "eventId": "uuid",
   "payload": { /* 实际事件内容，形状因类型而异 */ },
   "seq": 42,
   "sessionId": "sess_...",
   "timestamp": 1788831842267,
   "traceId": "uuid",
   "turnId": "turn_..."
 }}
```

### `params.type` → 事件类型映射（实测验证）

| type / payload | 含义 | Driver 映射 |
|---|---|---|
| `model.streaming` + `{kind: "text_delta", delta}` | 流式文本片段 | `TextDelta` |
| `model.streaming` + `{kind: "reasoning_delta", delta}` | 推理片段（**不记录**，隐私纪律） | 忽略 |
| `tool.updated` + `{kind, toolCallId, toolName}` | 工具生命周期 | `ToolStarted/Completed` |
| `turn.started` | Turn 开始 | `TurnStarted` |
| `turn.completed` + `{resultType, cacheStats, ...}` | **Turn 终态**（权威） | `TurnCompleted/Interrupted/Failed` |
| `turn.failed` + `{error, turnPhase}` | Turn 失败详情 | 用于稳定失败原因 |

### 真实 Turn 事件统计（一次完整回复）

一次简单 prompt（"Reply with exactly these words: ..."）产生约 **1182 条 session/event**，
其中绝大多数是高频遥测（788x computer-use/operation-event、789x process/mcpTelemetry、
208x process/resourceSample、788x v4/telemetry/event）。实际业务事件约 12 条：
- 1x title 更新
- 1x turn 开始
- 9x 流式 delta（text_delta + reasoning_delta 混合）
- 1x 模型完成（含 stopReason、content、usage、contextWindow）
- 1x turn 终态（resultType + cacheStats + tokenCount）

### 必须应答的服务端反向请求（否则 Turn 阻塞）

| 方法 | 触发时机 | 应答要求 |
|---|---|---|
| `session/requestRuntimePreferences`（scope=runtime-materialization） | session/create 后 | 至少 `{nativeSearchEnhancementsEnabled: boolean}` |
| `session/requestRuntimePreferences`（scope=user-execution） | session/send 后 | 同上 |
| `interaction/requestOfficialMcpAuthHeaders` | Turn 中（MCP 插件需要 auth） | 空对象 `{}` 即可（表示无 auth 可提供） |
| `interaction/requestPermission` | 工具需要审批 | 挂起并透传供应商 options；响应 `{"optionId":"..."}` |
| `interaction/requestUserInput` | 用户问题 | 当前 `{"cancelled":true}` |

**关键坑**：`interaction/requestOfficialMcpAuthHeaders` 未应答会**阻塞整个 Turn**——
Agent 一直在等待 MCP 认证头，不会超时也不会报错，事件流静默停止。

## 真实 Turn 验收（2026-09-07，全部通过）

修复文本流映射后的真实测试：

| 测试 | Prompt | 返回 | 结果 |
|---|---|---|---|
| `real_turn_acceptance_streams_and_completes` | "Reply with exactly: OK" | `"OK"` | ✅ 3 事件（TurnStarted + TextDelta + TurnCompleted），7.9s |
| `usage_stats_answers_quota_reads` | N/A | 本地统计 | ✅ windows=0, source=native_pull |

消耗额度：约 35K input / 130 output tokens（三次最小测试合计）。

## 额度（QuotaProvider）

- `usage/stats`（`range: 7d`）：本地 agent-db 统计（零模型调用）——30s TTL、`native_pull`。
  包含：totalTokens（665M）、cacheReadTokens（648M，97.6% 命中）、turns/tools/activeDays。
  **不是订阅额度窗口**。
- **协议无订阅窗口 API**：没有 5h/周限额、重置时间等概念。windows 如实为空。
- 上下文：`session/read` 返回 `projection.contextUsed/contextWindow`（1M 窗口），
  Turn 终态后按 native session id 保存。真实验收断言 `used_tokens > 0`、
  `window_tokens > 0`，通过。

## Driver：`zcode-driver`

- `Connection`：spawn（env_clear + allowlist + `ELECTRON_RUN_AS_NODE=1`）、自增 id 请求、
  等待期间内联应答控制请求、通知优先按 `params.type` 分类，键签名仅作兼容回退。
- `run_turn()`：先订阅 session；`session/send` accepted 后持续读取事件直到 `resultType`
  终态。取消经独立 channel 在同一连接写 `session/stop`。
- `probe_ready`（零模型调用）：`session/list` → `session/create` 临时会话 → close → list
  确认不可见。未配置时报 NotConfigured（"run zcode login"）。
- 审批：真实 options 为 `allow_once / allow_project / deny`。Driver 原样保留 optionId，映射为
  `ApproveOnce / ApprovePersistent / Reject`，挂起反向请求直到 Hub `respond_to_interaction`；
  不接受 Hub 自造 option。MCP auth 请求仍以空对象应答（防 Turn 阻塞）。
- context：终态后 `session/read` 读取 projection。
- resume：跨进程 `session/resume` 控制面重挂载已通过；上下文回忆闭环完成前仍不提升 capability。

## 未完成

- 审批 deny/allow 最终闭环与跨进程上下文回忆尚需在供应商模型恢复后复验。本轮实现后供应商在
  首次模型请求返回稳定错误码 `1308`（官方定义为使用额度已达上限，见
  <https://docs.bigmodel.cn/cn/api/api-code>）；控制面、选项形状和 resume 重挂载不受影响。
- 2026-09-08 真实复验确认 1308 映射为 `TurnQuotaExhausted`；Core 仅持久化稳定
  `quota_exhausted`，聊天时间线显示“额度已用尽”。协议仍无订阅窗口，因此不伪造百分比或重置时间。
- Windows 路径（`ZCode.exe`）未实测。

## 2026-09-07 统一复验

- 零模型 lifecycle：`session/list → create → close → list` 通过，临时 session 不再可见。
- Stream/Context：最终复验 12.31s 返回 `OK`，真实 `TextDelta + TurnCompleted`、session 级上下文
  以及 shutdown 原生 close 回执通过。
- Cancel：长 Turn 开始后发送 `session/stop`，1.48s 内得到 `TurnInterrupted`。
