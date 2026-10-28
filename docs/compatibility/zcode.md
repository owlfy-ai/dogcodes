# ZCode（智谱）兼容记录

> 跨 Agent 的当日验收状态、统一门槛和可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只维护 ZCode 协议事实与能力边界。

更新日期：2026-10-28（新版原生登录态额度监控）

## 2026-10-28 新版个人项目认证

个人 Coding Plan 不再要求旧版持久化 API key：该槽缺失且原生个人连接已选中时，
从当前 OAuth 账号读取原生项目位置，核对它属于当前账号的非团队项目，并以原生
`POST .../api_keys/<id>/access_tokens` 获取短期项目 Token。没有位置缓存时按原生规则
发现个人项目，并只复用名为 `zcode-api-key` 的已有 Key；缓存 Key 已删除时重新查询一次。
尚未在 ZCode 初始化个人 Key 的账号须先在原生客户端完成连接，本实现不创建持久 Key。

模型检测用项目 Token 查询有效订阅，再将权益交给原生 CLI 生成模型目录。运行时每次
凭据回调都取得新 Token，支持原生拒绝后的有范围重试；返回 `apiKeyId` 和与原生一致的
账号 scope。Token、项目 Key 身份不进入界面、事件或持久化配置。账号、登录 Token、
Start JWT、原生项目位置或连接选择变化时拒绝旧授权；远端项目/Key 变化也不能静默
跨越已有授权。旧版个人 Key 路径继续保留，团队项目不放行，个人套餐不触发验证码。

本机 BigModel 已通过真实原生就绪检测、完整目录和运行时凭据回调，读到 GLM-5.3 与
GLM-5.3-Flash；没有发送真实模型输入。Z.ai 的项目认证分支使用隔离 fixture 验证，
尚未用真实 Z.ai 账号验收。Node 33 项、Rust Driver 76 项回归通过。
桌面 debug App 已重新构建并加载，真实新对话的 Agent 列表可选 ZCode，模型菜单可选
GLM-5.3 与 GLM-5.3-Flash。未通过隐藏未就绪状态或强制放行来恢复菜单。

## 2026-10-28 额度监控兼容修复

新版 Desktop 的个人套餐项目 Token 不再写入旧版个人 API key 槽，原实现因此把已登录
账号误判为 `quota_login_required`。额度查询在该槽缺失时改用当前账号 OAuth access token，
以 Bearer 认证向该账号所属业务域的 `/api/monitor/usage/quota/limit` 发起只读 GET；
旧版个人 API key 查询仍保留。不签发项目 Token、不修改原生凭据，不把 OAuth 登录 Token
用于模型请求。账号、登录 Token 或连接选择发生变化时丢弃查询结果及旧身份缓存。

本机 BigModel 真实只读查询已取得 5 小时和每周窗口；Z.ai 分支通过隔离 fixture 验证，
尚未用真实 Z.ai 账号验证。Node 回归 23 项、Rust Driver 回归 76 项及
`quota::tests::real_native_credentials_quota` 本机只读验收通过。
源码更新后需重新构建并重启后台 Core 才能加载内嵌的查询脚本。

管理页补充修复：原先只有模型就绪后才注册额度 Provider，Core 查询也仅枚举可执行
Driver；因此原生模型目录不可用时，额度接口返回空列表，卡片仍无读数。现在发现
ZCode 安装后独立注册账号监控，Core 合并额度 Provider 与执行 Driver 的安装身份并去重。
模型未就绪仍保持原状态，不影响个人账号额度读取，也不因此开放模型执行。
回归覆盖未就绪时的桌面额度 RPC、安装筛选/去重/移除，以及未配置卡片的双窗口展示。

## 原生账号与套餐

当前版本读取安装包内置 Provider 注册表、ZCode Desktop 的本地账号、所选连接和套餐权益，
通过 `provider/updateAccountConfig` 交给原生 CLI。模型列表来自原生
`settings.model.available`，不在 dog.codes 中维护固定模型名单。

- 个人 Coding Plan：支持 BigModel 与 Z.AI；使用对应账号的个人套餐密钥或短期项目 Token 查询订阅，
  仅当前选择的个人连接可以响应 `interaction/requestProviderRuntimeHeaders`。
  不借用其他账号、团队套餐或自定义 Provider 的密钥，不触发 Start Plan 验证。
- Start Plan：按官方 balance 的有效套餐和模型能力提供权益；运行时使用原生 JWT。
  官方配置要求验证时打开默认浏览器的临时本机验证页，使用官方 SDK，用户完成后继续任务。
  该页不含 JWT/API key；取消任务会关闭等待中的验证服务。
- 团队 Coding Plan 尚未接入，不推断其授权。
- 模型查询会重新核实套餐；其他会话操作短暂复用 30 秒结果，凭据回调再次检查账号、
  密钥和当前连接。读取过程跨越账号切换时丢弃结果。超时不会当作已授权。
- 原生账号、配置文件均只读；账号凭据仅在本机子进程管道间传递，不进入 UI、远程网关或日志。
  已移除旧 CLI 配置的转换、加载和运行时回填；只读取客户端原生 Provider 注册表及账号凭据。
  额度查询同样使用当前原生个人套餐连接，账号变化时丢弃旧账号的额度缓存。
- 新版 CLI 使用 `persistence: deferred` 草稿查询模型，关闭时核实 deferred 状态，
  不为查询模型生成持久会话。需重新启动后台 Core 才能加载更新后的 Rust Driver。

本机验证已发现个人套餐的 GLM-5.3、GLM-5.3-Flash，并确认每个模型只出现一次，通过原生模型选择和凭据回调；
没有调用真实模型。Start Plan 的权益、验证回调和取消路径使用隔离测试验证，
尚未用实际 Start Plan 账号完成付费模型任务或真实浏览器验证。

回归命令：`node --test crates/zcode-driver/src/*.test.cjs`、
`cargo test -p zcode-driver --lib --locked`。本机只读账号验收：
`cargo test -p zcode-driver account::tests::local_account_model_catalog -- --ignored --nocapture`。

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
- **CLI 协议无订阅窗口 API**：5h/周窗口及重置时间由上文的原生凭据监控 GET 获取；
  无法取得订阅快照时不以本地 token 统计推算余额。
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
- resume：0.16.9 原生跨进程恢复已通过隔离验收，标记 Supported；其他版本仍为 Unknown。
  请求带原生 workspace 和历史 MCP，恢复后核对会话身份、规范化工作目录及空闲状态，
  再设置模型/思考强度并订阅。并发恢复复用同一结果，不接管原生正在运行的任务。
  完整模型目录保留自 resume 快照；session/read 只含当前模型时不覆盖目录。
  只有明确的原生 session_not_found 才映射为会话不存在，连接/协议异常保留原分类。
  不恢复旧 CLI 配置转换或运行时回填，也不自动迁移已被旧 Core 标记 Detached 的映射。

## 2026-10-20 原生冷恢复验收

`cargo test -p zcode-driver --lib resume_tests::installed_native_cold_resume --locked -- --ignored --nocapture`
在 macOS 安装的 0.16.9 上运行：使用临时 HOME、原生 v2 测试 Provider 和 loopback 模型服务，
完成第一轮后销毁 Driver，新进程并发恢复同一个 session，再发第二轮。
本地模型服务实际收到第一轮的用户与 Assistant 历史；不存在的会话返回 SessionMissing。
无真实账号/配置改动，无付费模型调用。此验收不代替 Windows 或实际套餐模型的验收。

## 未完成

- 审批 deny/allow 最终闭环与实际套餐模型的跨进程上下文回忆仍需单独复验。早期验收时供应商在
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
