# OpenCode 默认模型下线与错误提示修复

日期：2026-10-10。

## 结论与 codex-host 对照

`codex-host` 已执行 `git pull --ff-only`，本机仍为 v0.13.1 / `08c7d65d`。其 `packages/adapters/opencode/src/versioned-adapter.ts` 按 CLI 主版本区分 1.x、2.x 驱动；2.x 通过 `v2/connection.ts` 启动独立、带认证的原生 HTTP 服务，并通过原生模型目录/默认接口和 SSE 工作。恢复会话要求主版本匹配，不迁移用户历史。`v2/catalog.ts` 等待模型相关插件就绪，再读取模型目录及默认模型；原生调用错误保留为可识别的失败。

本次实际选中的 CLI 是桌面托管的 OpenCode 2.0.18。原生 ACP `session/new` 正常返回 `configOptions`，模型默认值和候选列表中仍包含 `opencode/exo-free`。`session/prompt` 返回有效 JSON-RPC 错误：`-32603 / Internal error: Model exo-free has been deprecated.`；原生会话错误记录为 HTTP 410。故本次失败来自服务端模型下线，不是 ACP JSON 无法解析，也不是单独清除本项目缓存即可解决。codex-host 的 HTTP 版本路由值得后续按完整能力范围评估；本次没有替换整个传输层。

## 已实现

- 模型明确下线/不可用错误归类为 `model_unavailable`，UI 提示切换模型，不再错误建议更新客户端。只持久化稳定错误类别，不存原生错误正文。
- OpenCode 模型刷新时只读原生 SQLite 的近期模型/错误元数据，兼容 1.x `message` 与 2.x `session_message`。仅 HTTP 410 且明确声明模型下线的候选被排除；认证、额度、网络失败不隐藏模型；后续成功记录允许模型恢复。无硬编码下线模型名单，不修改原生数据库或全局配置。
- 原生默认 ID 保留，UI 检测到其不在可用目录时显示“默认模型不可用”并禁用该选项，提示选择其他模型。发生模型不可用错误后自动触发当前模型目录刷新，保留用户草稿选择。
- 不自动选择另一个模型或提供商。当前会话需要由用户选择可用模型后重试；已保存的历史失败消息不重写。运行中的旧后端需要重启才能载入 Rust 修复。

元数据读取每种数据库结构限制 2048 条最新 assistant 记录、100 ms 锁等待。数据库缺失、不兼容或无法读取时保留原生目录；尚未实际请求过的下线模型不能通过此补充判断预先识别。

## 验证

- Driver API / ACP 单元测试：74 项通过、21 项真实环境用例保持显式忽略。覆盖原生错误穿过终态映射、下线过滤、暂时性失败、后续成功恢复及只读失败。
- 前端相关回归：32 项通过，包括失效默认选项、失败后刷新与保留选择；TypeScript 检查通过。
- `cargo clippy -p acp-driver -p driver-api --all-targets -- -D warnings` 通过；桌面 debug 构建通过。
- 实际 OpenCode 2.0.18 模型目录验收：排除 `opencode/exo-free`、保留 `opencode/big-pickle`，原生默认值仍如实保留。
- 默认模型真实请求：失败提示为模型不可用，无伪造的零 Token 请求记录。
- 显式 `opencode/big-pickle`：连续两轮真实对话均完成，并读回有效请求用量；关闭本次测试连接成功。

实测摘要见 [verification.json](evidence/opencode-retirement-2026-10-10/verification.json)，不含提示词、正文、原生会话 ID 或本机路径。

本轮前端测试入口曾意外运行全量套件：691 项通过、5 项失败，失败文件与已有 [运行稳定性记录](runtime-stability-2026-09-30.md#2026-10-08claude-后台取消与用量入口) 中的五项一致（AppStreaming、ConversationDeletion、ConversationRendering、ReviewStream、windowChrome）。未修改这些既有失败的断言或相关功能。
