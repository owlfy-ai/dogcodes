# M1-S6：订阅额度监控

## 2026-10-30：OpenCode Go 和供应商余额

- `provider-quota` 独立注册 OpenCode、Pi、DeepSeek Harness 账户监控，并组合已有 Driver 的会话用量和上下文读取；模型就绪失败也能查询账户，不创建对话、不发送模型输入。
- OpenCode Go 使用官方 `GET https://opencode.ai/inference/go/v1/usage`，展示 rolling（5 小时）、weekly、monthly 的已用百分比和原生重置时间。月度按日历窗口标记，不假定 30 天。兼容旧 `auth.json` 的 `opencode-go`、新版 SQLite `credential` 表中的原生当前凭据，以及 `OPENCODE_GO_API_KEY` / `OPENCODE_API_KEY`。不拿 Zen 或 OAuth 凭据代替 Go Key。Go 窗口只描述 Go 供应商，不因其耗尽禁用 OpenCode 的其他供应商或额外余额。
- Pi 从 `PI_CODING_AGENT_DIR`（默认 `~/.pi/agent`）的 `auth.json` / `models.json` 获取原厂 DeepSeek Key、自定义供应商 Key 和服务地址；DeepSeek Harness 支持 `DSH_HOME/.credentials.yaml` 的 API Key 引用、DeepSeek API Key 记录及继承的 `DEEPSEEK_API_KEY`。托管引用优先于环境；浏览器会话 grant 不作为 API Key。
- DeepSeek 官方余额来自 `/user/balance`；Pi 自定义服务仅向配置的服务 origin 查询兼容 sub2api 的 `/v1/usage`。`QuotaSnapshot.balances` 单独携带供应商名称、金额、币种，不换算百分比、不合并不同钱包和币种、不推算订阅金额。零余额与未知、无限额度分别处理。
- 每请求超时 10 秒、响应上限 1 MiB、禁止重定向。最多查询 8 个不同账户，单个供应商失败不丢弃其他已确认余额。成功缓存 60 秒，失败退避 15 秒，手动刷新绕过缓存；凭据和目的地址变化使缓存失效。账户读数不进入前端按 installationId 保存的旧额度回退，避免凭据切换后显示旧账户。
- 仅复用本机已有凭据，不执行命令型 Key helper，不自动登录/续期，不记录凭据或上游原始错误。当前不覆盖任意 DSH 插件自定义的认证存储、Pi 的任意 OAuth 订阅或 OpenCode Console 的美元钱包。

隔离验证覆盖三窗口解析、币种/零余额、旧版/新版 Go 凭据选择、账户切换、缓存/手动刷新、GET-only 查询、错误隔离、有界响应与会话用量保留。真实只读 DeepSeek Harness 余额查询已通过；本机 OpenCode 未配置 Go 凭据，Pi 未配置受支持的钱包，未声称它们完成了真实余额验收。

参考：[OpenCode Go 官方接口及路径说明](https://github.com/anomalyco/opencode/issues/52227)、[DeepSeek 官方余额 API](https://api-docs.deepseek.com/zh-cn/api/get-user-balance/)。Pi 的供应商余额范围参考本地 codex-host `packages/adapters/pi/src/account-balance.ts`。

## 2026-09-11：订阅总览去重

额度卡片按 provider 合并，优先 ready、有读数及较新版本，避免 Codex 桌面运行时与 CLI 重复展示同一订阅。侧栏计数按可用 provider 统计。执行实例列表和会话绑定保持独立，仍保留各实例读数供会话使用。回归覆盖乱序返回、旧版本有读数而新版本无读数、不可用安装、卡片渲染数量与版本选择。


更新日期：2026-09-08

最新增量：用户授权 ZCode 原生凭证只读复用；已读取 CLI 当前模型 provider 并调用官方 quota GET，BigModel 新 Provider 真实验收通过，Z.ai 路径尚待已登录账号实测。5 小时/周窗口进入圆环，工具月度额度单独展示。凭证不保存、不记录、不自动刷新，401/403 后回 ZCode 登录。此为下面旧版“不读取用户认证文件”约束的明确授权例外。

最新：Cursor macOS 原生终端 /usage 已自动采集并通过真实测试，桌面已注册；账期整数用量用于圆环展示，不作为发送硬门槛，重置日期只展示原生文本。Antigravity 原生额度已真实验收。ZCode 仍未接入订阅窗口，模型组自动关联仍待显式模型选择贯通。

实施增量：Antigravity 原生 /usage QuotaProvider 已编码并注册，按模型组显示窗口；当前模型组未知时不合并成圆环数字，也不全局禁用发送。新实现真实重验尚受审批服务限额阻塞。Cursor/ZCode 读取通道未完成。前端 38 tests 与 typecheck 通过，Rust workspace check 通过。

## 交付范围

独立额度总览与 Composer 指示器均按具体 Agent Installation 查询。切换 Agent 随即切换读数；
切换 Task 清空旧上下文，迟到的响应不会写回另一个 Task。优先显示 5 小时窗口，其次最短窗口。
数字表示剩余百分比。未知用量显示 ?，仍可通过悬停或键盘聚焦查看窗口、重置时间和上下文；
无数据不等于零额度。总览显示采集时间、过期状态、当前会话上下文与缓存命中率。

| Agent | 订阅数据来源 | 上下文/缓存 | 边界 |
|---|---|---|---|
| Codex | App Server account/rateLimits/read，账户窗口 | 当前 thread token usage 推送 | 沿用既有实现；不混入模型专属 bucket |
| Claude Code | rate_limit_event 推送，5h/周窗口 | 当前 session 最后 assistant 请求 usage + result 模型窗口；缓存读占比 | 未收到推送/自定义后端可无订阅数字；不主动发 Prompt 刷新 |
| Kimi | kimi web --port 0 --no-open，GET /api/v1/oauth/usage | 仅已有 ACP UsageUpdate；无缓存命中字段 | 刷新不发送 /usage Prompt；未主动推送上下文时保持未知 |
| Grok | ACP _x.ai/billing | 已有 ACP UsageUpdate | creditUsagePercent 可选；缺失时保留窗口/reset，百分比 Unknown |
| Cursor / Antigravity | 尚未接入；Antigravity 原生 /usage 已实测，Cursor CLI /usage 已静态定位 | Cursor 仅 ACP UsageUpdate；Antigravity 无 Provider | 见 quota-source-research.md；模型组作用域需先完善 |
| ZCode | 尚未接入订阅窗口；桌面 entitlement 服务已定位，CLI usage/stats 仅为历史 token 统计 | `session/read` context | 外部只读桥接尚未验证；1308 仍映射明确额度耗尽 |

不读取用户认证文件；Kimi 管理端口的临时 token 只驻留内存，不写日志、不落库。不根据套餐价格推算。

## 实现和可靠性

- 桌面为 Codex、Claude、Kimi、Grok 同时注册 Driver 和 QuotaProvider。
- Claude 账户窗口共享，上下文以原生 session ID 隔离；stale deadline 从观察时间计算，
  轮询不延长。result 的累计多次调用 token 不用作当前上下文占用。
- Kimi 额度读取直接走独立管理进程，不经过 ACP command loop，因此不阻塞 Prompt/Cancel。
  30 秒缓存与 single-flight 防止同时启动重复进程；临时服务器使用随机 loopback 端口与 --no-open。
  有界 banner、15 秒启动超时、10 秒 HTTP 超时、1 MiB body 上限，校验 HTTP status；
  reqwest 处理 chunked，禁止重定向和本地 HTTP 代理。正常/错误路径 kill + wait，
  取消时 kill_on_drop 回收。当前采用临时进程，不常驻 Web 服务。
- Grok 优先读取 config.creditUsagePercent（0–100），兼容 used/monthlyLimit 与旧时间字段；
  套餐兼容 subscription_tier（ACP 响应）与 subscriptionTier（其他版本）。
  单请求 8 秒，失败延迟 250ms 重试一次。
- Kimi/Grok 27 秒整体刷新上限，30 秒成功缓存/失败退避；失败返回原有 stale 快照，不改原时间。
  Core 为每个 Provider 增加 29 秒上限，避免单一 Agent 永久挂起整个总览。
- ACP CLI 保留显式 HTTP(S)/ALL/NO_PROXY 环境配置，让 CLI 的供应商访问与用户终端一致；
  loopback HTTP 客户端单独 no_proxy。环境值不记录。
- ACP readiness 有 15 秒握手上限；ACP/Claude 探针优先等待正常退出，5 秒宽限后回收。
- UI 30 秒轮询避免自身重叠，查询失败保留已有读数并按时间标旧。未知/上下文-only 同样可查看详情。
- Tauri quota view wrapper 使用 camelCase wire contract；Composer 以精确
  `providerId + installationId` 选择快照。2026-09-08 修复了 wrapper 曾输出 snake_case、导致所有
  Agent 的圆环均退化为 Unknown 的回归，并增加 Rust 序列化契约测试。
- 任一仍有效、未过期且置信度为 `provider_reported` 的额度窗口剩余 0% 时，Composer 圆环显示
  红色 `!`、详情和总览显示“已用尽”，发送按钮禁用；过期读数、已到重置时间、Unknown 和 Demo
  数据均不得阻止发送。
- Driver Port 新增结构化 `TurnQuotaExhausted`。ZCode 1308、Codex 明确 usage/quota code，以及
  Claude/ACP/Antigravity 明确的 `usage limit reached`、`quota exceeded`、`insufficient credits`
  等终态映射到 `turn.failed.failureKind=quota_exhausted`。原始供应商文本不进入 Canonical；普通
  HTTP 429 / `rate limit` 可能只是瞬时节流，保持普通失败，禁止误报为订阅耗尽。
- `quota_exhausted` 投影为可见时间线提示；没有订阅额度 API 的 Agent 因而也能在供应商明确拒绝
  回合后向用户说明“额度已用尽”，并建议等待恢复或切换 Agent。

## 代码入口

- crates/acp-driver/src/lib.rs：Kimi/Grok 拉取、parser、cache、session context、验收。
- crates/claude-driver/src/lib.rs：推送观察、session context 与过期。
- crates/core/src/lib.rs：installation registry、Task/native session 映射、有界并发读取。
- apps/desktop/src-tauri/src/lib.rs：真实桌面注册。
- apps/desktop/src/App.tsx、quotaPresentation.ts：总览、指示器、刷新与展示规则。

## 验证

离线回归覆盖：Grok 新旧字段和未知百分比、非法日期、Kimi 5h/周解析、chunked/status/redirect/body
上限、失败退避、账户缓存与会话隔离、Claude session 隔离和观察时间过期、前端余量与窗口优先规则，
以及明确耗尽/瞬时节流区分、安全 Canonical 映射、Timeline Projection rebuild、发送门禁。

本机只读验收命令（不发送模型 Turn）：

```sh
cargo test -p acp-driver real_kimi_subscription_read_only -- --ignored
cargo test -p acp-driver probes_the_current_users_grok_billing_extension -- --ignored
```

Kimi 只读验收已通过（5h/周、缓存命中、无 ACP 会话）；Grok 严格只读验收已通过（窗口/reset/缓存）。
Grok 首次上游请求失败定位为被清理的本机代理环境；恢复 CLI 代理环境后通过。
旧 Grok 测试已由“错误只打印仍通过”修成严格断言。旧 Kimi 模型测试不能作为必然产生上下文的证明；
是否有 UsageUpdate 必须由实际推送决定。

浏览器 Demo 已检查圆环详情、总览与时间/上下文展示；Demo 数字不作为真实账号验收。
workspace tests 通过（149 passed / 22 ignored）；修改后的 ACP/Claude 定向测试、fmt/clippy、
前端 typecheck/33 tests/production build 通过。最终桌面构建结果见 HANDOFF 置顶。

2026-09-08 增量验收：真实 ZCode 1308 已映射为 `TurnQuotaExhausted`（5.70s、审批和工具调用前
终止）；Rust 相关 crate 离线测试、workspace clippy `-D warnings`、前端 typecheck/37 tests/
production build 通过。该真实测试按预期无法完成原 approval 用例，因为账号仍处于额度耗尽状态。

2026-09-08 wire 回归修复后再次验证：`QuotaSnapshotView` camelCase 契约测试通过；运行中的桌面已
重新注册 Codex 0.153.4 Ready 并启动专用 App Server 连接；Rust workspace 171 passed / 34
ignored、fmt/clippy 通过，前端 typecheck/37 tests/production build 通过。

## 后续维护

供应商 payload 变更时增加脱敏 fixture 和 parser version；不可把缺失百分比强制当零。
Claude 无独立拉取、Kimi/Grok 无主动上下文推送时的数据缺失属于公开边界。
真实账号 API 暂不可用时保留 stale/Unknown，不把探测成功等同于额度请求成功。
