# Git 提交说明真实 Agent 验收（2026-09-29）

## 复现与修复

通过隔离 Core 服务的 `summarize_commit` RPC，逐个实际调用全部 12 家已注册 Agent。所有请求使用同一份示例 diff：重试次数从 1 增至 3，并同步更新断言。不操作用户项目、不提交或推送。最多两个不同 Agent 请求同时进行，以下耗时包含模型目录、会话启动、模型回复和清理，不能等同于纯模型延迟。

首轮 4 家成功、6 家误报空输出、2 家原生失败。根因是提交说明只消费旧 `TextDelta`，ACP 驱动现在通过 `Presentation(type=message)` 交付正文。已补齐文本增量、完整文本覆盖、重复事件去重和双通道去重，并排除其他会话/子 Agent/思考内容。标准化工具事件也按既有无工具约束处理；失败仍释放会话。

Claude 原生认证失败现在显示明确的登录/API Key/权限排查提示，不展示原生响应体或凭据。

## 每家结果

| Agent | 首轮 | 修复后/最终结果 | 成功耗时 |
| --- | --- | --- | --- |
| Antigravity | 成功 | 成功 | 26.39 s |
| Claude Code | Claude Code turn ended with an error result | 认证失败（403） | — |
| CodeBuddy | Agent 未返回提交备注 | 成功 | 11.55 s |
| Codex | 成功 | 成功 | 10.74 s |
| Cursor | Agent 未返回提交备注 | 成功 | 19.34 s |
| DeepSeek Harness | Agent 未返回提交备注 | 成功 | 1.39 s |
| Grok | Agent 未返回提交备注 | 成功 | 4.94 s |
| Kimi | Agent 未返回提交备注 | 成功 | 8.47 s |
| Kiro | Agent 未返回提交备注 | 成功 | 6.89 s |
| OpenCode | OpenCode 未返回任何内容或工具活动，无法确认任务完成。请在原生客户端检查运行状态和日志后重试。 | 原生数据库迁移失败 | — |
| Pi | 成功 | 成功 | 5.88 s |
| ZCode | 成功 | 成功 | 7.66 s |

共 10 家实际生成了符合示例 diff 的中文提交说明。原本成功的四家未重复消耗模型额度；六家受影响的 ACP Agent 均实际复测通过。

## 原生环境阻碍

- Claude Code：直接在临时目录调用原生 CLI，也返回 `Failed to authenticate. API Error: 403 status code (no body)`；与 UI 或 ACP 文本读取无关。需要检查原生登录、所配 API Key 或服务权限。
- OpenCode：直接调用原生 `run --format json` 退出码为 1；同轮原生日志记录 `failed to run data migrations`。未删除/迁移用户数据库、未修改认证配置，也未自动升级 CLI。

## 验证与复现

- Core 提交说明测试 5 项通过，涵盖标准化正文流、隔离/去重、长度上限、工具拒绝、模型/思考选择和会话释放。
- Claude 驱动测试 26 项通过，6 项真实验收按默认忽略；新增认证错误脱敏分类测试。
- Rust 桌面构建通过。Core/Claude Clippy 通过（保留既有 collapsible_if、duration_suboptimal_units 例外）。
- 可复用脚本：`python3 experiments/agent-integration/commit_summary.py DATA_DIR all OUTPUT_DIR`；需先启动隔离 Core 服务。脚本会消耗真实模型额度，不输出服务令牌。

已通过 `retire_if_idle` 确认生产 Core 空闲并加载新构建；生产服务的 DeepSeek Harness 提交说明 RPC 冒烟验证通过。Claude 的明确认证错误提示也已实际复测。隔离验收服务已正常退出，生产用户项目未提交或推送。
