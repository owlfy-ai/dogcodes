# Beta Coding Agent 统一验收

更新日期：2026-09-08

本文是 Beta Product Driver 的唯一验收状态表。各 `docs/compatibility/*.md` 记录协议细节，
`IMPLEMENTATION_PLAN.md` 与 `HANDOFF.md` 只引用本文，不再各自维护互相冲突的“已通过”结论。

## 验收口径

所有真实写操作只允许发生在 `tempfile::tempdir()` 创建的目录；测试结束必须调用
`driver.shutdown(session)`。真实用例默认 `#[ignore]`，仅在操作者明确允许模型额度消耗后运行。

| 门 | 通过条件 |
|---|---|
| Stream | 至少一个非空 `TextDelta`，随后收到真实供应商终态 `TurnCompleted`；accepted/queued 回执不算终态 |
| Cancel | 在进行中的真实 Turn 调用 `interrupt`，30 秒内收到 `TurnInterrupted`，不得把普通完成或失败算取消成功 |
| Approval deny | 收到供应商真实 `ApprovalRequested`，选择供应商提供的 Reject 选项，临时目录目标文件不存在 |
| Approval allow | 严格核对工具名、命令和临时工作目录后选择 ApproveOnce，目标文件内容与固定 marker 完全一致 |
| Cleanup | 探针创建的 deferred/临时原生会话关闭后再次 list 不可见；测试进程和连接可回收 |
| Context | Turn 终态后按 native session id 读取，`used_tokens > 0`、`window_tokens > 0`；不同会话不得串值 |

## 2026-09-07 本机矩阵

“历史通过”仅表示旧版本证据；“本轮”必须来自下列可重复测试的当日执行结果。

| Agent / 本机版本 | Stream | Cancel | Approval | Context/Quota | 本轮结论 |
|---|---|---|---|---|---|
| Codex 0.153.4 | 通过（14.26s，含 live reuse） | 通过（同一用例） | deny + allow 通过（35.35s） | 订阅窗口已接入 | **Beta，本轮通过** |
| Claude Code 2.1.218 | 通过（23.56s，含 live reuse） | 通过（同一用例） | deny + allow 通过（46.61s） | 上下文 + rate-limit push | **Beta，本轮通过** |
| Kimi 0.41.0 (ACP) | 通过（11.55s，含 live reuse） | 通过（同一用例） | deny + allow 通过（最终解析器复验 32.68s） | 5h/周窗口 + 上下文 | **Beta，本轮通过** |
| Grok 1.0.13 (ACP) | 通过（47.57s，含 live reuse） | 通过（同一用例） | deny + allow 通过（89.40s） | 原生 billing 周期 | **Beta，本轮通过** |
| Cursor 2026.09.02 (ACP) | 通过（33.28s，含 live reuse） | 通过（同一用例） | deny + allow 通过（52.12s） | 无订阅窗口源 | **Beta，本轮通过** |
| Antigravity（本机 bundle） | 通过（23.93s，含 live reuse） | 通过（同一用例） | **Unsupported**：真实写场景 0 次审批、0 写入 | 本地 usage/context，无订阅窗口 | **Beta；审批能力不支持** |
| ZCode 0.16.5 | 通过（text: "OK"，12.31s） | 通过（1.48s） | PendingInteraction 已接；deny 通过，最终 allow 复验被供应商 1308 阻断 | usage/stats + session 级 1M context；resume 控制面通过 | **Beta；两个模型闭环待复验** |

ZCode 2026-09-07 二次更正：CLI provider 配置恢复后真实 Turn 通过（text: "OK"）。事件流架构更正为
`session/event` 信封，0.16.5 以 `params.type` 为权威类型、payload 键签名仅作兼容回退，详见
`compatibility/zcode.md`。
dogcodes 不读取、记录或复制配置中的凭据；Ready 以协议成功和真实回合证据判定。

本轮还修正了一项真实安全缺陷：Grok 的旧启动方式 `grok agent stdio` 会在没有
`ApprovalRequested` 的情况下执行写工具。manifest 现固定为
`grok --permission-mode default agent stdio`；修正后 deny/allow 每轮均观察到一次供应商审批，
deny 不落盘，allow 仅在严格匹配临时目录命令后落盘。

## 可重复命令

从仓库根目录运行。以下命令会消耗真实 Agent 额度；一次只跑一个，失败时保留测试输出，
不得用放宽断言的方式改成绿色。

```bash
# Codex：stream + live reuse + cancel；deny + allow
cargo test -p codex-driver real_turn_acceptance_streams_reuses_and_interrupts -- --ignored --nocapture
cargo test -p codex-driver real_approval_deny_then_allow_only_in_temporary_workspace -- --ignored --nocapture

# Claude：stream + live reuse + cancel；真实审批
cargo test -p claude-driver real_turn_acceptance_streams_reuses_and_interrupts -- --ignored --nocapture
cargo test -p claude-driver real_approval_deny_then_allow_only_in_temporary_workspace -- --ignored --nocapture

# ACP：Kimi / Grok / Cursor 的 stream + reuse + cancel 与 deny + allow
cargo test -p acp-driver real_turn_acceptance_kimi -- --ignored --nocapture
cargo test -p acp-driver --test real_approval real_approval_kimi -- --ignored --nocapture
cargo test -p acp-driver real_turn_acceptance_grok -- --ignored --nocapture
cargo test -p acp-driver --test real_approval real_approval_grok -- --ignored --nocapture
cargo test -p acp-driver real_turn_acceptance_cursor -- --ignored --nocapture
cargo test -p acp-driver --test real_approval real_approval_cursor -- --ignored --nocapture

# Antigravity：stream + reuse + cancel；权限行为观察（不是审批通过声明）
cargo test -p antigravity-driver real_turn_acceptance_streams_reuses_and_interrupts -- --ignored --nocapture
cargo test -p antigravity-driver --test real_permission -- --ignored --nocapture

# ZCode：零模型生命周期；stream/context；cancel；deny+allow；跨进程 resume
cargo test -p zcode-driver probes_the_current_users_zcode_installation -- --ignored --nocapture
cargo test -p zcode-driver real_turn_acceptance_streams_and_completes -- --ignored --nocapture
cargo test -p zcode-driver real_turn_cancellation_reaches_interrupted_terminal -- --ignored --nocapture
cargo test -p zcode-driver real_approval_deny_then_allow_only_in_temporary_workspace -- --ignored --nocapture
cargo test -p zcode-driver real_resume_reattaches_across_driver_instances -- --ignored --nocapture
```

## ZCode 本轮修复

- `session/create` 的真实 ID 路径是 `/session/sessionId`；probe 现在要求 close 成功，并在第二次
  `session/list` 中确认临时会话已消失。
- `session/send` 只表示 accepted；Driver 现在等待 `session/event` 中的真实 Turn 终态。
- `interrupt` 通过 turn task 的 cancel channel 在同一 stdio 连接上发送 `session/stop`，不再被
 连接互斥锁阻塞。
- Turn 结束后 `session/read`，从 `projection.contextUsed/contextWindow` 写入按 session id 隔离的
  `ContextUsageSnapshot`；上一轮 cacheStats 映射为 cache hit rate。
- 已知的 state/telemetry/computer-use 通知不进入聊天时间线；文本和工具事件按 0.16.5 envelope
 schema 映射。
- `shutdown` 不再吞掉 `session/close` 错误，并在发布终态前完成 active-state 切换，避免 UI 收到
  终态后立即关闭时误走运行中分支。已产生消息的会话是历史记录，close 后仍可出现在 list；只有
  probe 创建的空临时会话要求从 list 消失。

## 当前验收边界

- ZCode 已把供应商 `allow_once / allow_project / deny` 原样转换为 Hub `PendingInteraction`，并
  拒绝任何自造 optionId；跨进程 resume 控制面已通过。最终 allow 落盘与 resume 上下文回忆在
  供应商模型错误 `1308`（官方定义为使用额度已达上限）恢复前仍不提升为通过，resume capability
  暂保持 `Unknown`。2026-09-08 再次单跑 allow，5.70 秒即在 0 次审批、0 次工具调用时返回
  结构化 `TurnQuotaExhausted`；测试会在断言中保留该终态，避免误判为审批映射回归。
  官方错误码：<https://docs.bigmodel.cn/cn/api/api-code>。
- Antigravity headless stream-json 在真实写场景没有交互审批请求，能力明确记为 Unsupported；
 这不是审批通过，也不得使用跳过权限参数替代。
- Cursor 无可验证的订阅窗口来源；Claude 的订阅额度仅在供应商 push 后出现。缺失读数保持
  Unknown，不从套餐、价格或模型名推算。

## 本轮回归基线

2026-09-07 在上述真实验收完成后再次执行：

- `cargo fmt --all -- --check`：通过。
- `cargo test --workspace --quiet`：通过（真实额度用例继续保持 ignored，单独按上方命令运行）。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `pnpm check`：typecheck、35 项前端测试、production build 全部通过；仅保留 pnpm 对旧
  `pnpm.onlyBuiltDependencies` 配置位置的弃用警告。
