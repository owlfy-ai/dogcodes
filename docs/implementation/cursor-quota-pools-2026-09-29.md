# Cursor 分项额度

Cursor 的订阅页分别显示 Cursor Models（Cursor Grok、Composer）和 Other Models（第三方模型）。原生 CLI 2026.09.23-86fc751 的 `/usage` 已经返回分项，但仍使用 Auto / API 旧名称。此前适配器只读取 Included，因此丢失了分项。

本机 Cursor 桌面代码 `auto-spillover-ui` 明确将 autoPercentUsed 映射为 Cursor Models，将 apiPercentUsed 映射为 Other Models；CLI 的 usage-data 和 usage-modal 使用同一对字段。

官方说明：[Models and pricing](https://cursor.com/docs/models-and-pricing)。Cursor Models 超额可能消耗 Other Models 或按量付费，因此任一分项归零不表示整个 Agent 不可用。

## 实现

- 2026-10-28 起参考本机 `codex-host/packages/adapters/cursor-cli/src/account-usage.ts`，通过 Cursor 原生 DashboardService 只读查询 `GetCurrentPeriodUsage` 和 `GetPlanInfo`，取代依赖欢迎页文案和终端布局的 `/usage` 自动操作。不启动 TUI、创建探针目录或发送模型请求。
- 复用 CLI 的 OAuth 存储：macOS 默认读取 Keychain 的 `cursor-access-token` / `cursor-user`；显式 file 模式读取 `~/.cursor/auth.json`。Linux 使用 `$XDG_CONFIG_HOME/cursor/auth.json`（默认 `~/.config/cursor/auth.json`），Windows 使用 `%APPDATA%/Cursor/auth.json`。API Key 与 memory 模式不读取已保存 OAuth。凭据仅在内存中用于官方接口，不进入快照、日志或持久化，不自动刷新或改写。
- `cursor-dashboard-usage-v1` 解析器将 `planUsage.autoPercentUsed` / `apiPercentUsed` 映射到 `cursor-models` / `cursor-other-models`，保留分组 scope 和小数精度。超过 100% 的原生使用率展示为 100%；缺失的 API 分项不补零，缺失或非法 Auto 百分比拒绝快照，不使用 spend/limit 或 Included 推算余额。
- 欢迎页、额度详情分别显示两个每月额度池；紧凑主圆环代表 Cursor Models，并明确标注名称，不计算跨组总额。
- 账期重置使用 `billingCycleEnd` 的毫秒时间戳（兼容数字字符串），转换为协议要求的 Unix 秒；缺失或非法时间保持未知。额度按模型池展示，不因单池用尽封锁整个 Agent。
- HTTPS 请求支持环境/系统代理，禁止重定向，单请求 10 秒超时、响应上限 1 MiB。401/403 使缓存额度失效并提示重新登录；套餐名称查询失败不丢弃有效额度。保留现有刷新合并和缓存策略。

## 验证

历史验证：2026-09-29 通过运行中的本地服务强制刷新，约 7 秒返回，无 diagnostic：Cursor Models 已用 20%，Other Models 已用 100%，Pro，重置 Oct 4，按量付费关闭。与用户截图一致。

当前回归覆盖原生双池百分比、精确账期、缺失/非法字段、OAuth 存储选择、API Key/memory 隔离、HTTP 请求契约及错误脱敏；前端沿用双池卡片、月度重置、圆环名称和分项用尽不封锁整个 Agent 的既有行为。

2026-10-28 新适配器本机只读实测通过，1.64 秒返回 Pro、Cursor Models 已用 19.6111%、Other Models 已用 21.4889%，两池均取得 `billingCycleEnd`。`cargo test -p acp-driver` 通过 68 项，另外单独运行 Cursor 真实额度测试通过；前端 982 项测试、ACP all-targets 严格 Clippy、工作区格式检查通过。其他需要原生客户端或消耗模型额度的测试保持忽略。
