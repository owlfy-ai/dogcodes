# 原生模型与思考强度控制能力

核查日期：2026-09-12。范围：dogcodes 当前使用的原生传输协议，而非各产品交互式终端的全部能力。

| Agent | 核查版本 / 证据 | 模型与思考强度更新 | 当前实现 |
| --- | --- | --- | --- |
| Codex | 本机 0.154.0-alpha.6.2 导出的 v2/TurnStartParams schema；官方 App Server 文档 | turn/start 接受 model 和 effort，成为当前及后续轮次的配置 | 在原连接上保存下一轮覆盖值，随 turn/start 发送；进程和 thread ID 均保留 |
| Claude | 本机 CLI 2.1.269；官方 SDK 0.3.269 类型定义及实现；本机无推理控制探测 | apply_flag_settings 接受 model、effortLevel、alwaysThinkingEnabled；支持 null 清除/恢复默认 | 发送单个会话级控制请求，等待相同 request_id 的 success，之后才发 prompt；不修改配置文件 |
| Antigravity | 本机 agy 1.2.2；官方 headless 文档；本机无推理拒绝探测 | stream-json 明确拒绝 control_request；/model 也不支持这种输入模式 | 保留带新启动参数恢复同一原生 conversation ID 的路径，不向活动进程发送不支持的控制事件 |
| ACP（Cursor / CodeBuddy 等） | 既有原生 session/load、session/set_config_option 或 session/set_model 适配 | 配置更新依赖各 Agent 已声明的 ACP 能力 | 沿用原生配置更新路径 |
| ZCode | 既有 session/setModel、session/setThoughtLevel 适配 | 原会话配置更新 | 沿用原生配置更新路径 |

## 默认值与失败处理

- Codex 的 null/省略不能被当成清空旧会话覆盖值。恢复默认或仅指定模型时，在同一个连接上读取 model/list，解析默认模型/effort，明确下发；不为这次解析另外启动进程。
- Claude SDK 说明 model:null 和 effortLevel:null 分别恢复原生默认模型和模型默认 effort；alwaysThinkingEnabled:null 清除 flag 层覆盖。off 映射为 false，明确强度映射为 true，auto/默认清除覆盖。
- Claude 的 set_model 只能解决模型切换；set_max_thinking_tokens 的 token 预算不等同于 low/high effort，因此未用它冒充 effort 更新。
- Claude 控制响应按 request_id 匹配。拒绝、EOF、超时均失败并清理等待项，不会把未确认的配置当作已成功。
- Codex 活动轮次不接受 Configure 控制，避免改变正在执行的请求。UI 的预选在下一次提交时应用。
- 原生会话不可恢复或明确没有所需能力时，Core 才使用历史重建；普通连接/协议错误不会静默重建。

## 验证边界

- 本机 Codex：导出 schema，确认 model/effort 字段；没有为此发送真实推理请求。
- 本机 Claude：在同一 CLI 进程中执行 apply_flag_settings、get_settings，确认 effective.effortLevel=low、effective.alwaysThinkingEnabled=true，并获得恢复默认的 success。没有发送 user prompt。
- 本机 Antigravity：发送控制事件后明确报告“不支持 control_request”，退出码 2；没有发送 user prompt。
- Codex 进程模拟测试：连续切换模型/effort、恢复默认，检查实际 turn/start 参数、相同 PID 和相同 thread ID。
- Claude 进程模拟测试：连续更新、off、默认，检查相同 PID/SessionProcess；不相关确认不误匹配，拒绝和 EOF 可见且清理等待项。
- Core/Store 既有回归覆盖配置持久化、事件回放、原生会话保留及必要时的历史重建。

## 来源

- [OpenAI App Server：逐轮配置覆盖](https://learn.chatgpt.com/docs/app-server#start-a-turn)
- [Anthropic Agent SDK TypeScript](https://code.claude.com/docs/en/agent-sdk/typescript)
- [Anthropic 官方 SDK 0.3.269 发布类型定义](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.269/sdk.d.ts)：Query.applyFlagSettings、SDKControlApplyFlagSettingsRequest。
- [Google Antigravity Headless：不支持的消息](https://www.antigravity.google/docs/cli/headless/#unsupported-messages)

官方文档会随版本变化；后续新增热更新能力应以已验证的本机协议为准。
