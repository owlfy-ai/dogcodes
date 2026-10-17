# 真实 Agent 审查验收（2026-09-24）

使用已有原生 Agent Driver 和真实服务调用，走 `Core::start_project_review`、分组、冻结检索、独立复核、SQLite 持久化及投影重建。新增显式运行的验收程序 `crates/store/examples/live_review_workflow.rs`。测试项目和数据库位于独立临时目录，未修改本项目之外的用户工程或应用主数据库。材料为人为构造的缺陷；结论不是对本项目全部代码的审计。

## 结果

| Agent / 类型 | 结果 | 完成组数 | 冻结上下文请求 | 独立复核会话 | 已知缺陷 |
| --- | --- | --- | --- | --- | --- |
| Codex / 代码审查 | 通过 | 2/2 | 4 | 2 | 移除余额检查导致负余额；取末项索引越界 |
| Codex / 安全检查 | 通过 | 1/1 | 4 | 1 | 删除所有权校验导致越权读取 |
| Kimi / 性能分析 | 修复兼容问题后通过 | 1/1 | 4 | 1 | 一次批量查询退化为 N 次远程查询 |
| Claude Code / 安全检查 | 认证受阻，未通过 | 0/1 | 0 | 0 | 认证服务返回 HTTP 403；用户确认 Key 已过期，本轮跳过 |
| Grok / 代码审查 | 通过 | 2/2 | 0 | 2 | 两个已知缺陷均确认 |
| ZCode / 代码审查 | 通过 | 2/2 | 0 | 2 | 两个已知缺陷均确认 |
| Cursor / 代码审查 | 调整启动预算后最终复验通过；此前有超时 | 2/2 | 0 | 2 | 两个已知缺陷均确认 |
| Antigravity / 代码审查 | 调整启动预算、代理继承后最终复验通过 | 2/2 | 0 | 2 | 两个已知缺陷均确认 |
| CodeBuddy / 代码审查（首次） | 部分完成，整体失败 | 1/2 | 9 | 2 | 两个初审问题保留；一组复核 JSON 无效 |
| CodeBuddy / 代码审查（重试） | 未完整通过 | 0/2 | 3 | 1 | 复核触发工具被拦截；另一组会话创建失败 |
| CodeBuddy / 代码审查（用户要求再次重试） | 完整通过 | 2/2 | 10 | 2 | 两个已知缺陷均确认；没有改代码或配置 |

版本：Codex `0.155.0-alpha.2.6`、Kimi `2.0.2`、Claude Code `2.1.278`。使用各 CLI 已配置的默认模型，未新增或替换凭据。

首批 Codex/Kimi 的三个成功运行均返回有效结构化报告，目标文件全部有检查记录，四个已知问题的位置/引用校验通过，独立 AI 复核均支持初审结论。每组初审与复核会话 ID 不同；保存证据用会话 ID 的 SHA-256 摘要保留独立性证明，不复制原生会话 ID。

冻结验证：先从实际 Git 比较及不可变对象采集 `policy.py`，再把磁盘文件替换成 `LATER_LIVE_VERSION_MUST_NOT_BE_USED`。Agent 实際引用的是原先的金额规范化、权限和数据库查询语义。三个成功运行均未改变测试文件；重建 SQLite 投影后可读回同一输入摘要和报告。Codex 另执行真实主对话一轮，正确使用返回的问题及复核状态；长报告仍按既有规则作为明确标记的摘录回流，未宣称传入全文。

## 实测发现并修复的问题

1. **新版 ACP 消息未被审查读取。** Kimi 正文通过 `Presentation(type=message, textDelta=...)` 返回，审查只接收 `TextDelta`，导致有真实回复但显示“未返回完整审查结论”。新增统一收集，支持替换消息、事件去重、主会话过滤，忽略思考和子会话正文，并避免新旧通道双份 JSON。工具/子 Agent 的 Presentation 事件也立即停止审查。
2. **Claude 审查启动误屏蔽用户认证配置。** `--setting-sources ""` 排除了用户 settings 中已有的 API 认证及模型路由。改为只读取 `user` 来源，继续排除 project/local，并保留禁用工具、严格空 MCP、禁用 Hooks、plan 权限。修复后不再提示未登录，但认证请求返回 403；未继续反复重试或修改凭据。
3. **Claude 的 error result 被误判成功。** 真实 CLI 会返回 `subtype=success` 同时 `is_error=true` 的认证失败结果。现在按失败处理，不把它当作完成。

回归：Core 收集器 2 项新增测试、Claude Driver 24 项测试（6 项原有真实调用测试仍按默认忽略）、既有审查集成 15 项通过。Core/Claude Driver 全 targets 及验收 example 严格 Clippy、格式检查、桌面编译通过。

## 证据与复跑

成功与失败摘要、结构化报告、分组过程及请求摘要保存在 [evidence/review-native-2026-09-24](evidence/review-native-2026-09-24/)；主对话回流文本见 [codex-primary-return.txt](evidence/review-native-2026-09-24/codex-primary-return.txt)。未保存凭据或账户配置。

以下命令会真实使用已登录 Agent 的额度，仅显式执行，不加入自动测试：

```sh
cargo run -p agent-store --example live_review_workflow -- codex code
cargo run -p agent-store --example live_review_workflow -- codex security
cargo run -p agent-store --example live_review_workflow -- kimi performance
cargo run -p agent-store --example live_review_workflow -- grok code
cargo run -p agent-store --example live_review_workflow -- cursor code
cargo run -p agent-store --example live_review_workflow -- codebuddy code
cargo run -p agent-store --example live_review_workflow -- antigravity code
cargo run -p agent-store --example live_review_workflow -- zcode code
```

Claude 认证恢复后再执行 `claude-code security`。程序保留临时证据目录，未完成/非结构化结果以失败退出。

本次覆盖真实 Driver/Core 的项目审查链路和主对话回流，没有逐个点击桌面 UI，没有新增逐轮入口的真实采集验收，本轮已尝试其余已接入 Agent，结果见下节。逐轮与项目入口共用的执行层已由本次实测覆盖；逐轮来源组包、取消和原材料重试仍依据既有自动化测试。已知样例通过不代表对任意项目的准确率保证。


## 其余 Agent 实测补充（2026-09-24）

用户确认 Claude Code 的 Key 已过期，本轮不再请求 Claude，也未修改任何 Agent 的凭据。对 Grok、Cursor、CodeBuddy、Antigravity、ZCode 分别发送同一套代码审查样例（2 文件、2 组、两个已知缺陷），通过产品原生 Driver/Core 执行，不是只检查版本号或端口连通。

Grok `1.0.41 (4220f3b224a6)`、ZCode 首次完成 2/2 组，各自通过 2 个独立会话复核，两个缺陷全部确认，位置校验、投影重建、测试文件未修改均通过。这两家未主动请求补充上下文（0 次），因此本轮不能据此宣称它们的冻结检索交互也已验证。

CodeBuddy `2.156.0` 两次未完整通过：首次两组均返回初审，一组复核输出非法 JSON（如 `"endLine":5"`），另一组复核成功；第二次第一组复核尝试调用工具被停止，第二组未能创建审查会话。首次 9 次、第二次 3 次冻结上下文请求；原始失败结果及初审均保留。这两次运行均按失败保留，未把无效复核当作确认，也未放宽只读限制。用户要求再次重试后的完整通过记录见下节。

启动问题和修正：

- Cursor `2026.09.18-9a7762b` 的原生 ACP 对照探测：initialize 约 0.4 秒，session/new 约 22–35 秒；原先 15 秒的探测上限不足。调整为 45 秒并记录阶段耗时。调整后仍出现过一次超时，说明存在启动时延波动，不能只凭增加预算宣称根因全部消除。
- Antigravity `1.2.10` 原生审批 Hook 冷启动实测 21.5 秒，超过原先 20 秒限制。专用 Hook 探测预算调至 45 秒，并补齐与正式会话相同的系统代理继承；审查创建会话预算调整为 1 分钟，仍受总时限约束。等待启动期间可立即取消，新增回归验证。Hook 校验本身仍必须通过，未绕过审批桥。
- 验收程序补齐 Antigravity Hook 入口，提供可选 `--session-probe`，只创建隔离分析会话、不发送模型请求，便于区分启动失败和模型输出失败。

Cursor、Antigravity 最终复验均完成 2/2 组，各自两个缺陷均经独立会话复核确认，位置校验、投影重建及文件未修改检查通过；均未请求补充上下文。Antigravity 最终四次 Hook 探测约 8–13 秒。此前启动失败证据保留，后续仍需观察不同网络/冷启动条件，单次复验通过不等于长期稳定性保证。

回归：审查集成 16 项、ACP Driver 32 项、Antigravity Driver 14 项通过（19 项默认忽略的真实测试未额外触发）；Core/ACP/Antigravity 全 targets、Store 验收 example 与审查集成测试的严格 Clippy、格式检查及桌面编译通过。顺带修正检查暴露的既有测试数字分隔与 MutexGuard 作用域问题。

本轮其余 Agent 测试均为 `code`，不能据此声称每家三个类型均已验证，也没有新增桌面点击或逐轮采集验收。失败证据均保留，不覆盖为成功记录。


## CodeBuddy 再次重试（用户提出可能存在网络波动）

在相同验收程序、相同两组预设缺陷、原有模型与认证配置下重跑，未改代码、提示词或权限限制。CodeBuddy `2.156.0` 本次完整通过：2/2 组、10 次冻结上下文请求、2 个独立复核会话，余额校验删除和索引越界两个缺陷均得到确认，代码位置与引用校验有效；2/2 文件有覆盖记录，投影重建后报告可恢复，测试文件未修改。连接初始化约 0.77 秒，探测会话约 0.82 秒完成。本次未再出现此前的复核格式错误、工具调用或会话创建失败。

因此最新状态更新为“本次真实审查复验通过”。网络波动可能解释部分连接失败，但此次通过不能单独证明此前所有失败均由网络导致。先前两次失败证据保留；这次结果见 `evidence/review-native-2026-09-24/codebuddy-code-network-retry.json` 和同名 `-report.json`。
