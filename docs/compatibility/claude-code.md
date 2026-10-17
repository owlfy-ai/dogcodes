# Claude Code 接入事实

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Claude 协议细节与历史证据。

更新日期：2026-09-07

## 通道决策（方案 A，已实施）

产品约束：**安装 dogcodes 后用户不需要手动安装任何额外依赖**。据此，Claude Code 的主通道
从社区 ACP 适配器改为 **原生 Rust 直连官方 headless stream-json 协议**：

- Driver：`crates/claude-driver`，spawn 用户已装的 `claude`
  （`-p --input-format stream-json --output-format stream-json --verbose
  --include-partial-messages --permission-mode manual --session-id <uuid>`）。
- 依赖模型与 Codex 一致：只要求用户已安装并登录 Claude Code 本体；无 npm、无 Node、无 adapter。
- 审批：`--permission-mode manual` 下工具使用以 `can_use_tool` control request 到达；
  PendingInteraction 透传供应商选项，由用户决定，不自动批准。
- 中断：client→claude 的 `interrupt` control request；result 事件为权威终态。
- 会话：一个 CLI 进程 = 一个原生 Session；`--session-id` 由 Hub 生成 UUID，create_session 即可
  返回稳定 native session id；进程跨 Turn 复用（live reuse = true）。
- Resume：`--resume` 已通过跨进程验收，见下文 2026-09-04 记录。

## 本机事实（2.1.218 实测）

- `claude --version` → `2.1.218 (Claude Code)`；CLI 真实路径
  `~/.local/share/claude/versions/2.1.218`（`~/.local/bin/claude` 为 symlink）。
- 控制面探针（零额度消耗）：spawn + `initialize` control request → 收到
  `control_response success`（含 slash commands 列表）。该探针已固化为
  `cargo test -p claude-driver -- --ignored`。
- `--permission-mode` 2.1.218 可选值：acceptEdits / auto / bypassPermissions / manual /
  dontAsk / plan；Hub 固定使用 `manual`。
- 初始探针未发送模型 Turn；随后真实 streaming/resume 验收见下文记录。

## ACP 适配器路径（降级为可选，当前未接线）

- `@agentclientprotocol/claude-agent-acp`（Apache-2.0，社区维护）本质是官方
  `@anthropic-ai/claude-agent-sdk` 之上的薄壳；0.73.0 的 ACP initialize 探针此前通过。
- 不作为主通道的原因：它是 Node 程序（要求用户有 Node/npm）、全局安装污染环境、随 Claude Code
  迭代有滞后风险。桌面扫描不再检测/注册该 adapter；manifest 已移除 `requires_executables`。
- 如未来需要（例如复用其额度字段），可作为"用户已有 Node 时的可选加速路径"重新引入，届时以
  独立 manifest + 用户显式安装为前提。

## 真实 Turn 验收（2026-09-03，操作者明确授权后执行）

`cargo test -p claude-driver -- --ignored --nocapture real_turn_acceptance`：
- 轮 1（固定 no-tools prompt，"Reply with exactly: OK"）：流式 TextDelta + `turn.completed` ✓
- 轮 2（同一 Session 再次 prompt）：live 复用 ✓
- 轮 3（长任务 + 2s 后 interrupt control request）：`TurnInterrupted` ✓

验收中发现并修复：Claude 中断后的 result 携带 `error_during_execution`，driver 曾把它映射为
TurnFailed；现已修正为"操作者请求过中断则任意终态都映射为 TurnInterrupted"。

## 订阅额度接入（2026-09-04，参照 codex-host 实现）

- Claude stream-json **无按需额度拉取**；订阅窗口只经 `rate_limit_event` 推送（会话级、
  可在无 Turn 时到达）。解析形状按 codex-host 实测：`rate_limit_info.unifiedWindows.
  {five_hour,seven_day}`，`utilization` 为 **0–1 小数**（SDK .d.ts 注释的 0–100 是错的），
  `resetsAt` 为 unix 秒；兼容旧扁平 `rateLimitType + utilization` 回退。>1 的值按百分比
  防御性处理。
- 上下文使用最后 assistant message 的 input/cache_read/cache_creation 之和，
  并以 result.modelUsage 中对应模型的 contextWindow 为窗口；不使用 result 多次模型调用的
  累计 token 作为上下文占用。缓存命中率 = 最后请求 cache_read / prompt tokens。
- QuotaProvider 语义：纯推送驱动（`native_push`），无窗口和上下文时 CapabilityUnavailable；
  快照 1 小时后标记 stale（推送只在 Turn 中刷新）。**本机注意**：当前 Claude Code 配置
  走自定义后端（probe 出现 `kimi-for-coding` 模型与 `inference_geo: not_available`），
  非 Claude.ai 订阅路径，可能永远不产生 `rate_limit_event` —— 属实表现，不推断。
- 注册：桌面在 Claude 安装 Ready 时同时注册 Driver + QuotaProvider。

2026-09-06 修复：上述桌面注册此前只写在文档中，现已实际接线；上下文按 native session 隔离，
快照 stale 从观察时间算起，刷新不会延长。离线测试验证 session 隔离/过期；本轮未消耗真实
Claude 模型额度，因此不声称验证了本机 Claude.ai 订阅窗口。

2026-09-08：新鲜 `rate_limit_event` 任一窗口剩余 0% 时 UI 显示“已用尽”并禁用发送；result
只有包含明确 usage/quota/credits 耗尽措辞时才产生安全的 `TurnQuotaExhausted` 时间线提示。
本机自定义后端未取得真实 Claude.ai 耗尽终态，分类由离线 fixture 锁定，普通 429 不误判。

## 未完成

- 无按需额度拉取；在首次供应商推送前显示暂无读数。
- 额度接入候选：Claude 官方 statusLine 结构化额度字段（FR-5.4，走 QuotaProvider，与本 Driver 解耦）。

## 跨进程 resume 验收（2026-09-04 通过）

`cargo test -p claude-driver -- --ignored --nocapture real_resume`：turn 1 → 销毁
driver 实例 → 全新实例 `--resume <session-id>` → turn 2 回忆上下文（"dogcodes"）全部成功，
session id 保持不变。**关键发现**：`--session-id` 不能与 `--resume` 共用（除非另加
`--fork-session`），组合会导致 CLI 启动即退出（stderr: "--session-id can only be used with
--continue or --resume if --fork-session is also specified"）。driver 的 resume 分支因此只传
`--resume`。desktop 注册 capability 已提升为 `Supported`。
