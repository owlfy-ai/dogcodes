# Antigravity CLI 兼容记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Antigravity 协议细节与历史证据。

更新日期：2026-09-07

## 2026-11-07 Windows 欢迎页探测闪出终端

最初仅按窗口标题和时间，将闪窗归因于 `agy models` 身份探测，并补上
`AGY_CLI_NONINTERACTIVE_HEADLESS=1`。用户重打安装包后仍复现；仅凭时间关联不足以定位
CLI 内部派生的进程，非交互认证开关也不控制自动更新。

后续通过窗口显示事件和同一时刻的进程命令行取得直接证据（12:53:12 UTC）：
`desktop.exe`（PID 39108）→ `agy.exe --input-format stream-json --output-format stream-json`
（PID 41180）→ `agy.exe --bg-updater --app_data_dir=antigravity-cli --gemini_dir=.gemini`
（PID 14800）。可见终端标题为 `agy.exe`。剩余闪窗来自 readiness 时原生 CLI 的后台更新链；
dog.codes 的 `CREATE_NO_WINDOW` 无法约束原生 CLI 自行创建的控制台。

manifest 的发现探测和 driver 的公共命令准备均设置
`AGY_CLI_DISABLE_AUTO_UPDATE=true`，覆盖版本、readiness、会话、模型、额度与 Hook 检查。
这是子进程环境配置，不修改用户的全局环境或 CLI 配置。值必须是字面量 `true`，不能用 `1`。
依据：[官方排错文档](https://antigravity.google/docs/cli-troubleshooting)、
[上游问题 #1046](https://github.com/google-antigravity/antigravity-cli/issues/1046)。原生更新器有
15 分钟检查间隔，因此连续复测不闪窗不能单独作为修复证据。

安装身份探测改为 `agy models --help`，通过本地帮助中的 `Usage: agy[.exe] models [flags]`
签名识别 CLI，再用 `--version` 读取版本。这个阶段不再请求需要登录的模型列表；实际模型
目录仍由 driver 在就绪后读取。非交互开关继续保留；此改动避免身份探测依赖认证和网络，
但单独使用不能解决后台更新器的闪窗。

进程级回归同时支持 Windows 和 Unix：即使已经设置 headless，任何不带 `--help` 的模型
请求都视为失败，只有本地帮助和版本探测允许成功。Windows 本机验证使用 `DETACHED_PROCESS`
宿主和不可用的 HTTP(S) 代理，检查无父控制台、网络不可用时也能识别安装和版本。

原生 CLI 单独复验：相同的 stream-json 启动方式、关闭 stdin、不发送 prompt，显式传入
上述自动更新开关。CLI 自身日志确认 `Auto-update disabled via environment variable
AGY_CLI_DISABLE_AUTO_UPDATE`，且没有 `Spawned background update process`；不能只依靠
检查间隔内未闪窗的结果。driver 回归同时拦截本机和远端的实际启动请求，要求字面量 `true`。

本地 NSIS 包复验：完整后台扫描和 12 个 Agent 的额度查询流程结束；110 秒窗口事件监听
捕获到的可见终端事件为 0。安装包提取出的 `desktop.exe` 与扫描时程序的 SHA256 一致
（`BB74026EE1C2CD8C8620E6204B035046C4DE2525923205612B4E36AEA9D05868`）。

## 2026-11-04 后台查询误弹 OAuth 登录页

`agy --print=/usage` 仍可能启动交互式登录：即使钥匙串 token 尚未过期，静默认证的网络请求
超过 10 秒也会进入 `Print mode: triggering interactive OAuth`。本机 03:27 的日志记录了
该分支；04:22 的后续查询则静默认证成功并正常执行 `/usage`，因此弹窗不等于登录失效。

DogCodes 的额度查询、模型目录、readiness/会话和 Hook 验证子进程现在显式设置
`AGY_CLI_NONINTERACTIVE_HEADLESS=1`，使用 CLI 的非交互认证分支，避免后台查询自动打开
浏览器。用户主动打开原生客户端登录的入口不经过这些子进程配置。

查询失败沿用现有缓存策略：临时网络错误保留原额度及采集时间，手动刷新返回错误；明确的
账户拒绝仍使缓存失效。进程级回归覆盖认证超时无浏览器副作用、原缓存保留及后续查询恢复。

## 2026-09-11 审批实现修复

参考本机 `codex-host/docs/antigravity-tool-approval.md` 和 `packages/adapters/antigravity/src/question-bridge.ts`，桌面审批采用私有 PreToolUse Hook，不是 stdin 原生审批响应。旧版“无交互审批面”的描述仅适用于原生 headless 通道。

定位到 Rust Bridge 的临时目录生命周期错误：异步任务按字段捕获 listener/token，没有持有 TempDir，启动审批服务器时即删除 Hook 配置。`/hooks` 检查发生在删除前，因此不能发现此错误。修复后任务显式持有目录，直到原生进程结束才清理，并加入文件存续/清理回归测试。

`--new-project` 与独立 Hook 目录的原生拒绝对照实验均成功，保留项目隔离。桌面审批仍需进程级 skip 参数配合 Hook，缺失验证不得启动；不能去掉 Hook 单独使用 skip。完整验收结果见 [审批验收记录](approval-acceptance-2026-09-11.md)。

## 当前结论（历史记录）

- 本机安装：`~/.local/bin/agy`（真实二进制，非 symlink），`agy --version` → `1.1.26`。
  注意 `~/.antigravity/antigravity/bin/agy` 的 symlink 已悬空（app bundle 内无该路径），不以它为发现来源。
- **官方不支持 ACP**（github.com/google-antigravity/antigravity-cli Issue #31 为开放 feature
  request；社区有第三方 agy-acp 桥，但 Hub 不引入非官方依赖）。主通道因此是官方 headless
  stream-json——与需求 §9.1 指定一致。
- 官方协议（antigravity.google/docs/cli/headless，已实测确认）：
  - 输入：每行一个 NDJSON `{"event":"user","message":{"content":[...]}}`；content 为
    `{type:"text"}` block 数组（或纯字符串）；非 text block 会导致 exit 1。
  - 输出：`init`（cwd/tools/permission_mode）→ `step_update`（step_type: user_input/
    agent_response/tool/checkpoint；agent_response 携带增量 `text_delta`；tool 步骤有
    tool_name/tool_info）→ `result`（conversation_id/status/response/usage）。
  - status 取值：SUCCESS/ERROR/CANCELED/INTERRUPTED/INVALID/WAITING/RUNNING。
  - 多轮：保持 stdin 开启，同一 conversation_id 跨轮复用；关闭 stdin 结束会话。
  - **不要传 `-p/--print`**：stream-json 模式只从 stdin 读 prompt，flag prompt 会被丢弃且
    Go flag 语法会吞掉后续参数。

## 真实验收（2026-09-04，操作者登录正常账号后执行，全部通过）

- 真实 Turn 三轮（`real_turn_acceptance`，21.6s）：流式 text_delta + SUCCESS → `TurnCompleted`；
  同会话第二轮复用；长任务 + 关闭 stdin 中断 → `TurnInterrupted`。
- 跨进程 resume（`real_resume_acceptance`，36.1s）：turn 1 → 销毁实例 → 全新实例
  `--conversation <uuid>` 重挂载（conversation_id 不变）并正确回忆上下文（"dogcodes"）。
  **桌面 capability 已提升 Supported。**
- 事件细节实证：`conversation_id` 位于事件顶层（不在 init 内部），首个 init 事件即携带；
  `step_update` 的 usage（input/output/thinking/cache_read tokens）可用于用量统计。

## 接入形态

- Driver：`antigravity-driver`（NDJSON stream-json，同 Claude driver 模式）。
  - `init` → TurnStarted；`agent_response` 的 `text_delta` → TextDelta；`tool` step 的
    ACTIVE/DONE → ToolStarted/ToolCompleted；`result` 按 status 映射终态（INTERRUPTED/
    CANCELED → TurnInterrupted）。
  - 中断：关闭 stdin（官方"关闭 stdin 在当前轮后结束"语义），终态重映射为 TurnInterrupted。
  - resume：`--conversation <ID>` 已通过跨进程真实验收，capability 为 Supported。
  - 审批：该通道无交互审批面（permission_mode 默认 request-review，无 can_use_tool 类
    控制请求文档化）；PendingInteraction 透传不适用于本驱动，后续以官方新增能力为准。
- manifest：`drivers/manifests/antigravity.toml`（protocol `antigravity_stream_json`）。
  身份探针用 `agy models`（输出 Gemini 模型列表=产品签名；该命令可能非零退出但 stdout
  有效，registry 已放宽 identity 探针对退出码的要求）。
- 桌面扫描分派 `antigravity_stream_json` → probe_ready（事件面探针，不消耗模型额度；
  地区/登录失败不误标 Broken，保留 Ready 待首轮真实 Turn 暴露）。

## 关键坑：共享认证状态 + 钥匙串静默换号（2026-09-04 事故记录）

agy 的认证是链式的（活动 token 文件 → **macOS 钥匙串** → 浏览器），且 IDE/CLI/TUI 共享
**单一活动账号**。事故时间线（日志证据 `cli-20260904_121151.log`）：

1. 并发/被中断的 agy 进程把活动 token 文件的刷新竞态打断 → CLI 状态变为 "not authenticated"；
2. 后续进程走 `Print mode: not authenticated, trying silent auth` → 从钥匙串捞到
   **旧账号 token**（本机为 guijijike@gmail.com）→ `applyAuthResult` 静默切换账号；
3. 新账号有地区限制 → `Eligibility check failed`；用户交互式 TUI 与 headless 来回踩，
   活动账号在两个身份间振荡。

**已落实的 driver 防护**（本次代码完善）：
- 进程级 spawn 互斥（`AGY_SPAWN_GUARD`）：agy 进程整个生命周期内独占 spawn 槽，杜绝并发
  token 刷新竞态；
- 永不中途 kill：probe 与会话结束一律 `wait()` 回收（EOF/关闭 stdin 后 CLI 自行退出），
  `kill_on_drop` 仅作进程泄漏的最后保险；
- 认证/资格失败指数退避（60s 起、上限 4 次幂）：`result.error` 含 Eligibility/authentication
  即计入 `AuthFailure`，退避期内 `probe_ready` 直接返回 NotConfigured，不重击凭据链。

**用户侧修复**：`agy` 重新登录目标账号；并建议在"钥匙串访问"删除旧账号
（guijijike@gmail.com）的 antigravity/gemini 相关通用密码条目，根治静默回退。

**通用教训**（已同步应用到 Claude/ACP driver）：probe 的 kill-on-timeout 会打断 agent 的
凭据持久化；这些 CLI 与交互式会话共享凭据存储，Hub 必须"优雅退出 + 串行 spawn"。

## 当前边界

- 无主动订阅额度读取面；2026-09-08 起，仅当 result error 含明确 usage/quota/credits 耗尽措辞
  时生成安全的 `TurnQuotaExhausted` 时间线提示。普通 429/资格错误仍按各自语义处理。
- 审批行为已用临时目录真实写场景复验：Turn 正常终止、出现 4 个工具事件，但
  `ApprovalRequested=0` 且目标文件不存在。因此 headless stream-json 的交互审批能力记为
  **Unsupported**，不能记为通过或 Unknown；不可用 `--dangerously-skip-permissions` 绕过。
- 账号资格稳定性：2026-09-04 验收通过后约 1 小时，资格检查再次间歇性失败
  （`Eligibility check failed ... not currently available in your location`，driver 与探针路径
  一致）。疑似当日多轮真实测试触发服务端频率/资格限制；等窗口重置后复测。
