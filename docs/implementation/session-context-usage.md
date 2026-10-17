# 会话上下文与用量（2026-10-01）

主对话输入区圆环展示当前 Agent / installation / native session 的上下文已用百分比。
账户剩余额度仍在 Agent 选择菜单与额度管理页；上下文占用不参与订阅额度发送门禁。
悬停或键盘聚焦使用共享 `Tooltips` 数据面板，Escape 关闭。没有有效上下文占用百分比时隐藏圆环，收到读数后显示；真实的 0% 正常显示。

## 数据来源

| Agent | 上下文 | 其他用量 |
| --- | --- | --- |
| Codex | `thread/tokenUsage/updated`: `last.totalTokens / modelContextWindow` | `total` 输入、输出、缓存读取、推理、总 tokens；最新请求缓存命中率 |
| Claude Code | 最后 assistant 消息的输入 + 缓存读取 + 缓存写入，配合 result 中当前模型容量 | result 本轮输入/输出/缓存；单独标注会话累计费用 |
| ZCode | `session` snapshot 的 `projection.contextUsed / contextWindow` | 原生消息缓存命中率（不是 token 缓存命中率） |
| Pi | 现有 RPC 连接只读 `get_session_stats.contextUsage` | 会话输入/输出/缓存/总 tokens/费用；`message_end` 最新缓存命中 |
| Antigravity | 自有进程日志定位本地 HTTPS Language Server，读取当前 conversation 的 context metadata；也接受流中显式容量 | `step_update` / `result.usage` 的输入、输出、推理、总 tokens |
| Grok | ACP `usage_update` 与精确 session 路径下 `signals.json` | 原生 prompt response `usage`（保留 vendor 字段，包括费用 ticks） |
| Kimi | ACP `usage_update.used / size` | 原生 prompt response `usage` 中实际返回的字段 |
| Kiro | `_meta.kiro.context_usage`、空闲时只读 `_kiro/session/context` 的百分比 | 按原生 requestIds / executionId 去重的已记录 Credits（当前连接观察范围）；不反推 token 数或容量 |
| CodeBuddy | ACP `usage_update`（需原生实际发送） | 精确工作区/session JSONL 活动分支，按 messageId 去重的输入/输出/缓存/推理/总 tokens/Credits |
| Cursor / OpenCode / DeepSeek Harness | 接收 ACP `usage_update`，缺失就显示未知 | 接收 ACP prompt response `usage`，只展示原生实际返回字段 |

CodeBuddy 历史汇总标注为“当前分支”，Kiro Credits 标注为“已记录”，避免把恢复连接后尚未观察到的历史消耗算作零。
ACP 的 token 统计标注为“原生统计”，因为不同实现对 prompt response 的统计范围存在差异。
接入 ACP 采集不等于该 Agent 当前版本必定上报；不把 codex-host 的另一种原生 transport 能力假定为本项目 ACP 能力。
Antigravity 仅在原生元数据提供容量时计算百分比，不使用按模型名称猜测的默认容量。
CodeBuddy 的完整历史受 16 MiB 读取上限约束，超限、分支不完整或会话/工作区不匹配时忽略该可选来源。

## 实现边界

- `QuotaProvider.read_session_usage` 与订阅额度读取分离；Core 即使订阅读取失败，仍返回已有会话数据。
- SessionUsageSnapshot 的上下文、费用与 token 字段均可缺省；缺少字段不补零。
- 不用累计消耗计算窗口占用，不把多个时间段/不同模型窗口累加。
- Pi 压缩后原生 `tokens: null` 保持未知，等待下一次原生估算；不回退到旧窗口占用。
- 所有读数按 native session 分区；任务切换清空视图，迟到响应被丢弃。
- 沿用每轮完成/30 秒刷新；活动会话增加每 5 秒的当前 installation 刷新，请求复用并发中的读取。
- 查询不提交模型 prompt；Antigravity 仅访问自有进程暴露的 loopback 地址，限制时间/体积且禁用代理与 curl 用户配置。
- push-only 会话在重启或恢复后需要原生再次上报；无读数时不显示别的会话的数据。

参考：本地 codex-host 的 Codex native usage、Pi session stats、Kiro usage、CodeBuddy history 和 Antigravity context metadata 采集方式。

## 验证

离线 parser / transport 测试覆盖累计消耗与窗口分离、最新缓存比率、百分比-only、压缩后未知值、重复 Credits 去重、CodeBuddy 活动分支去重、ACP 通知与 prompt response 实际传输、账户失败 fallback 和任务隔离。
前端测试覆盖百分比圆环、可选字段、共享 Tooltip 动态更新/键盘/Escape、订阅额度原有行为与刷新去重。开发预览 `rendering-preview.html` 有固定示例数据；已经人工核对面板排版。
未为验证主动发起真实模型请求，未宣称所有 Agent 原生版本都实际产生了完整字段。

## Antigravity 实机补查（2026-10-01）

当前 agy 内嵌 Language Server 1.2.14 的本地元数据接口返回 HTTP 401 `missing CSRF token`；已有端口发现不足以完成读取。当前 driver 尚未取得该进程的 CSRF 凭据，因此不能声称本地上下文读数已在该版本可用。

官方 headless 文档明确 `result.usage` 是会话累计值，而 `step_update.usage` 是单次生成的统计。已删除将 `input_tokens` 回退为上下文已用量的逻辑，补充缓存读取并区分统计范围。只有显式上下文字段或成功获取的元数据才进入上下文圆环；不按模型名称硬填容量。

来源：https://antigravity.google/docs/cli/headless （Read the results）。
