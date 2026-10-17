# Antigravity CLI 兼容记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Antigravity 协议细节与历史证据。

更新日期：2026-09-07

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
