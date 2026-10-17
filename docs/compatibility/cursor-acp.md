# Cursor Agent / ACP 兼容性记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Cursor 协议细节与历史证据。

更新日期：2026-09-07

## 当前结论：已接入并通过真实验收

- 本机 runtime：`~/.local/bin/cursor-agent` → `~/.local/share/cursor-agent/versions/
  2026.09.02-c22c1a3/cursor-agent`。首次调用 `cursor agent ...` 触发的官方下载已完成并可用；
  `cursor-agent --version` 输出 `2026.09.02-c22c1a3`（registry 的前导零回退解析为 semver）。
- **直连二进制的子命令是 `acp`**（`cursor-agent acp`）；wrapper 形式 `cursor agent acp` 才带
  agent 前缀。manifest launch_args 固定为 `["acp"]`。
- 账号已登录：`session/new` 成功返回 sessionId、modes（agent/plan/ask）与完整模型列表
  （Auto/claude-opus-5/gpt-5.6/glm-5.2/kimi-k3/gemini-3.8 等约 38 个，含 configOptions）。
  未登录时该调用返回 `-32000 Authentication required`（authMethods: `cursor_login`），
  ACP probe 映射为 NotConfigured。

## 真实验收（2026-09-04，操作者授权后执行，全部通过）

- 真实 Turn 三轮（`real_turn_acceptance_cursor`，29.5s）：流式 TextDelta + EndTurn →
  `turn.completed`；同会话第二轮复用；长任务 + session/cancel → `TurnInterrupted`。
- 跨进程 resume（`real_resume_acceptance_cursor`，30.2s）：turn 1 → 销毁实例 → 全新实例
  `session/load` 重挂载并正确回忆上下文（"dogcodes"）。initialize 上报 `loadSession=true`。

## 接入形态

- manifest：`drivers/manifests/cursor.toml`（protocol kind `acp`、identity 用 `acp --help`
  的 "Agent Client Protocol" 输出、user_paths 覆盖 `~/.local/bin/cursor-agent`）。
- Driver：`acp-driver`（与 Kimi/Grok 同一 stable v1 产品路径）。
- 额度：initialize/session 响应不含订阅额度面；QuotaProvider 如实 Unknown。2026-09-08 起，若
  ACP prompt 明确报告 usage/quota/credits 耗尽，仍会产生安全的“额度已用尽”时间线提示；普通
  429 不推断为订阅耗尽，也不会因 Unknown 禁用发送。

## 历史告诫（仍有效）

- wrapper 命令 `cursor agent ...` 在 runtime 未安装时会尝试从 cursor.com 下载——不得用作
  后台探针（现已由直连 `cursor-agent` 的版本/身份探针替代，不再触发下载）。
- PATH 中的通用 `agent` 指向 Grok Build，不是 Cursor Agent；`version_output_markers`
  产品签名校验仍在防串线。

早期“未取得安全版本输出前不创建 manifest”的门槛已经满足：当前内置 manifest 只直连
`cursor-agent`，同时用 `version_output_markers` 校验产品身份，不使用 PATH 中通用 `agent`。

## 协议基线

- Cursor 官方说明的 ACP 入口为 `agent acp`，传输是 stdio 上的 JSON-RPC 2.0 JSONL：
  <https://cursor.com/docs/cli/acp>
- ACP stable v1 初始化顺序和能力协商：
  <https://agentclientprotocol.com/protocol/v1/initialization>
- 官方 Rust SDK：<https://github.com/agentclientprotocol/rust-sdk>

工作区固定 `agent-client-protocol = 2.0.0`、关闭 default features，并使用
`Client.builder()` 的 stable-v1 API。锁文件实际解析的 schema package 为 1.5.0。

## 已完成的离线契约

`experiments/acp` 使用官方 SDK 和内存双工流验证：

| 能力 | 证据 | 状态 |
|---|---|---|
| initialize + capability negotiation | 请求/响应协议版本为 V1 | 通过（离线 fake Agent） |
| session/new | session id 完整往返 | 通过（离线 fake Agent） |
| session/prompt + session/update | update 在 prompt 完成前可被客户端消费 | 通过（离线 fake Agent） |
| permission | option id 保真，客户端固定返回 Cancelled | 通过（离线 fake Agent） |
| session/cancel | Agent 收到正确 session id | 通过（离线 fake Agent） |
| 未知扩展 | raw method/params 保真 | 通过（本地适配层） |

脱敏证据：`experiments/acp/fixtures/stable-v1-offline-contract.json`。

## SDK stable-v1 已知差异

官方 SDK 2.0.0 的 stable-v1 `AgentNotification` / `ClientNotification` 枚举解析
`_vendor/update` 后，`method()` 返回 `vendor/update`，丢失 ACP 扩展方法要求的前导
下划线。实验中的 `PreservedExtension` 因此在 typed dispatch 前保存原始 method 和
opaque params。未来 Driver 必须保留该 raw interception seam，不能把 typed enum
往返当作未知扩展的唯一事实源。
