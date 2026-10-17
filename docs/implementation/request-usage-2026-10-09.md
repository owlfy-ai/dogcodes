# 请求级用量接入与真实验证（2026-10-09）

## 交付范围

在既有 `SessionUsageSnapshot` 上增加 `requestUsage`，保存已观察到的模型请求身份、原生模型（可缺省）、输入/输出/缓存/推理 token 及原生费用或 Credits。输入统一包含缓存、输出包含推理；不同原生口径在各驱动内转换。按请求身份替换重复观察，不把轮次/会话累计快照相加。所有记录均为“已取得的请求”，不宣称完整会话覆盖；每会话最多保留 10,000 条，达到上限明确标记部分记录。

输入框底部既有“上下文与用量”浮窗新增“已记录模型请求”和请求输入/输出汇总。只有每条记录都提供某指标时才展示该指标汇总；缺字段不补零，原生本轮/会话读数与请求汇总分别展示。没有逐请求来源的 Agent 继续展示其原生摘要。

本次未加入公开价格表估价、请求速度/首 token 计时、全局统计页面、完整历史回放或独立明细页面。实时采集器重启后从新的观测范围开始；文件/数据库读取器只读当前绑定会话。并非每个 Agent 的所有后台请求都能被观测。

## 真实测试结果

在 macOS 当前安装版本上，通过本项目生产驱动，在独立临时目录发送两次最小提示，要求仅回复 `USAGE_OK_1` / `USAGE_OK_2`，不调用工具。每轮结束后重复读取两次用量。使用用户已有原生配置；未修改登录、API Key 或供应商设置。Coding Agent 与底层模型不是同一概念：本机 Claude Code/Pi 实际返回的模型为 `kimi-for-coding`，不能将测试解释为 Anthropic 官方计费验证。

| Agent / 版本 | 实际结果 | 边界 |
| --- | --- | --- |
| Codex 0.160.0 | 两轮成功；请求数 1 → 2；重复读不增量 | 原生通知没有请求 ID，只有累计增量严格等于 `last` 时认定一次请求，以 turn + 累计检查点去重。模型、缓存写入未提供，保持未知。普通会话首轮建立超时，使用已有 `hub-analysis` 隔离模式复测成功；不等于普通会话启动问题已修复。 |
| Claude Code 2.1.289 | 两轮成功；请求数 1 → 2；重复读不增量 | 使用 `message_start` 身份、最终 `message_delta.usage`、`message_stop`，排除起始占位值和 synthetic 消息；按父工具区分并发子流。本次一次关闭返回“process shutdown remains unconfirmed”；最终解析器再次执行两轮实测，记录数 1 → 2、重复读稳定且关闭成功，见 claude-recheck.json。保留首次异常，不把复测成功当作已修复所有关闭条件。 |
| Pi 1.0.2 | 两轮成功；请求数 1 → 2；重复读不增量 | assistant `message_end`，优先 responseId，否则原生时间；错误/中断占位消息不入账；模型及原生费用可用。 |
| CodeBuddy 2.161.2 | 两轮成功；请求数 1 → 2；重复读不增量 | 精确工作区及会话 JSONL 当前活动分支，按 providerData.messageId 去重；有效 Credits 为 0 仍保留。实测确认快取来自 prompt_tokens_details.cached_tokens，而不是一直为 0 的 Anthropic 风格字段，已修正。 |
| ZCode 3.14.4 | 两轮成功；请求数 1 → 2；重复读不增量 | session/read 已完成 assistant 消息；校验 input + output = total，排除合成消息及无法确认口径的记录。 |
| DeepSeek Harness 0.2.0-rc.2 | 两轮成功；请求数 1 → 2；重复读不增量 | 读取精确会话 v4 JSONL/zstd 的 assistant/message，校验会话 ID、工作区，模型及缓存拆分可用。支持有体积上限的多帧解压。 |
| Grok 1.0.50 | 两轮成功；请求数 1 → 2；重复读不增量 | 原生 turn_completed 为摘要；仅 modelUsage 明确 modelCalls=1 时转换该模型的一次请求。多次模型调用的摘要不拆分、不冒充单次请求；费用来自原生 ticks。 |
| Antigravity 1.3.2 | 两轮成功；请求数 1 → 2；重复读不增量 | DONE agent_response 的 conversation_id + step_index 和 step usage；不把 result 累计再计一次。模型及缓存写入未提供。 |
| Kimi 2.1.1 | 两轮成功，有输入/输出/缓存数据；请求数保持 0 | 新增精确会话 main/wire.jsonl 摘要读取；原生明确 usageScope=turn，保留为本轮摘要，不制造请求身份。 |
| Kiro 2.28.0 | 两轮成功，有 Credits 及上下文比例；请求数保持 0 | 未获得逐请求 token；原生请求 ID 集合或 executionId 的 Credits 摘要沿用原有去重。 |
| Cursor 2026.10.01-e373342 | 两轮成功，没有有效用量字段 | 明确返回暂不可用；不以账户额度或回复字数推测 token。 |
| OpenCode 1.15.10 | 原生阻碍，未取得有效请求用量 | 原数据库迁移失败并报 session_message.seq 的 NOT NULL 冲突。临时数据库绕开该问题后，免费模型 API 返回 HTTP 426，要求 CLI >=1.18.0。本机仅列出免费模型，未升级用户 CLI、修复用户数据库或更换账户。 |

OpenCode 首次测试采用独立 CLI 1.15.10，未通过真实模型验收；该入口不同于应用实际优先选择的桌面配套 2.0.18，不能将此失败推广为应用接入失败。补测结果见下节。OpenCode 已实现只读当前会话的 v1/v2 数据库采集。实测发现失败记录包含全零 token；已排除此占位数据，并对同一真实失败记录执行只读复查，不将其报告为零费用的有效请求。

逐 Agent 的真实 token 数、模型、去重结果及清理状态见 [脱敏实测记录](evidence/request-usage-2026-10-09/live-results.json)。完整临时报告位于 `/tmp/agent-request-usage-final-20261009/`；首次对照位于 `/tmp/agent-request-usage-20261009/`。临时目录不保证长期保留。

## 同日 OpenCode 修复与正确入口补测

本机并存独立 CLI 1.15.10 和桌面配套 CLI 2.0.18。应用已有 macOS 兼容选择逻辑，优先使用桌面配套 2.x；原实测脚本硬编码独立 CLI，导致测试结论与应用实际入口不一致。脚本现要求 OpenCode 显式传入应用所选 executable，并支持第四个参数指定模型，报告记录覆盖参数。

- 使用官方 `opencode upgrade --method curl` 将独立 CLI 升级到 1.18.35，解决免费模型 HTTP 426 最低版本限制。
- 原库由桌面 2.x 迁移过，独立 1.x 报 `workspace.project_id` 缺列。先用 SQLite backup 和旧二进制建立备份，再在副本上验证向空 workspace 表添加 nullable `project_id` 的兼容修复；副本与原库各两轮真实调用通过。所有原 session/session_v2/message/session_message ID 保留，quick_check=ok。此本机兼容措施不是通用迁移实现，不在产品中自动执行。
- 应用实际使用的 2.0.18 能正常启动，但 ACP 新会话默认 `opencode/exo-free`，模型调用返回 HTTP 410 `Model exo-free has been deprecated.`。重新启动原生 ACP 后目录仍包含该模型并将其作为默认项，单纯刷新本项目缓存不能消除上游目录与服务不一致。
- 显式通过生产驱动选择 `opencode/big-pickle` 后，两轮对话成功，请求数 1 → 2，重复读取不增量，模型/输入/输出/缓存/推理及原生免费费用均可用，进程关闭成功。归一后两轮输入 11474 / 11593、输出 84 / 6。见 [补测证据](evidence/request-usage-2026-10-09/opencode-recheck.json)。有效请求用量的 Agent 因此增至 9 家。
- 尝试原生默认模型配置与 reload 未改变 2.0.18 ACP 的默认选择，已恢复原配置；没有硬编码替用户静默切换模型。应用内应在模型下拉框明确选择 Big Pickle，已有绑定 Exo Free 的会话同样需要换模型。
- 前端 `ModelCatalogCache` 按 provider/installation/version 缓存在当前应用运行期，无 TTL。失败时自动退避重试及手动重试，成功时没有手动刷新入口；版本变化或页面重新加载会重建相应缓存。当前也没有根据模型下线错误标记列表不可用的机制。本次未实现该独立改进。

测试脚本重新构建和定向严格 Clippy 通过；缺少 OpenCode executable 时提前拒绝，验证没有创建测试目录或调用模型。

本机备份：`~/.opencode/backups/20261009-030524/`，含原数据库、修复前数据库、旧 CLI 及原配置。短对话验证不替代历史会话冷恢复、工具执行或提交摘要验收。

## 验证与复现

- 生产驱动真实调用与合成协议回归分开记录；实测 8 家得到非零请求 token，3 家只得到摘要或无数据，OpenCode 受原生环境限制。
- 回归覆盖请求去重、会话隔离、累计快照不覆盖请求账本、缓存/推理归一、原生多调用摘要拒绝、失败零占位排除、Claude 最终用量与子流隔离、Antigravity 累计 result 不重复入账、前端缺值与统计范围。
- 7 个相关驱动/API crate 的库回归：174 项通过，45 项需单独启用的原生测试忽略；随后 Claude 最终流解析器的 2 项定向回归通过（包括新增子流隔离）。
- DeepSeek、Grok、Kimi、OpenCode 的同一批真实会话只读复查：4 家通过预期记录数断言；OpenCode 失败零占位修正后为 0 条。
- 前端 `ContextUsageRing` / `Tooltips` / `tooltipConvention`：17 项通过；桌面 TypeScript 类型检查通过。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过；真实验收 example 构建通过；`git diff --check` 通过。
- 未重新执行全前端测试、GUI 或 Windows 验收。此前全前端 5 项既有失败的基线记录不在本次范围内。

实际调用（消耗真实模型额度）：

```sh
cargo run -p agent-store --example live_request_usage -- pi /tmp/dogcodes-usage-pi
# 支持全部 12 家 provider ID；第三个参数指定原生 executable（OpenCode 必填）。
# 第四个参数可指定模型，例如 opencode/big-pickle。
# Codex 使用已有 hub-analysis 会话模式，其他 Agent 使用原生默认配置。
```

只读已绑定的原生会话（不调用模型）：

```sh
DOGCODES_USAGE_PROVIDER=deepseek-harness \
DOGCODES_USAGE_CWD=/absolute/test/workspace \
DOGCODES_USAGE_SESSION=actual-native-session-id \
DOGCODES_USAGE_EXPECTED_REQUESTS=2 \
cargo test -p acp-driver native_usage_for_bound_session -- --ignored --nocapture
```

原生读取尊重 `GROK_HOME` / `CODEBUDDY_CONFIG_DIR` / `DSH_HOME` / `OPENCODE_DB` / `XDG_DATA_HOME`；相关目录变量也传递给 ACP 子进程，保证采集器和原生进程指向同一份数据。读取限于已绑定会话，不扫描其他用户账号。Windows 尚未实际验证。
