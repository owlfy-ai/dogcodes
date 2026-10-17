# Grok Build 兼容记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Grok 协议细节与历史证据。

更新日期：2026-09-07

## 本机事实（实测）

- 安装：`~/.local/bin/grok`（symlink → `~/.grok/bin/grok`），另有 `agent` → `~/.grok/bin/agent`。
  `grok --version` → `grok 1.0.13 (5e9a58528b76)`（含产品名 + semver，marker 直接取 "grok"）。
- ACP：`grok --permission-mode default agent stdio` 原生提供 stable v1 stdio server。initialize 实测返回
  `loadSession=true`（sessionCapabilities list/resume/close）、`embeddedContext=true`、
  `mcpCapabilities http/sse`、authMethods `grok.com`、availableCommands（compact/deep-research/
  workflow/goal 等）、`currentModelId=grok-4.6`。
- 真实就绪探针：`probes_the_current_users_grok_installation` 通过（resume=Supported）。

## 接入形态

- manifest：`drivers/manifests/grok.toml`（protocol kind `acp`；1.0.13 必须显式
  `launch_args = ["--permission-mode", "default", "agent", "stdio"]`，否则 ACP 会继承自动执行
  策略，写工具不发 `requestPermission`）。
- Driver：`acp-driver`，与 Kimi 共用同一条 stable v1 产品路径。

## 真实 Turn 验收（2026-09-03，操作者完成 grok 登录并授权后执行）

`cargo test -p acp-driver -- --ignored --nocapture real_turn_acceptance_grok`：
- 轮 1（固定 no-tools prompt）：流式 TextDelta + EndTurn → `turn.completed` ✓
- 轮 2（同一 Session）：live 复用 ✓
- 轮 3（长任务 + 2s 后 session/cancel）：`TurnInterrupted` ✓

登录状态变化也被正确表达：未登录时 ACP probe（initialize + session/new）返回
`Authentication required`，driver 映射为 NotConfigured 并拒绝注册 Ready——登录后同一探针转为
Ready 并完成上述验收。

## 跨进程 resume 验收（2026-09-04 通过）

`cargo test -p acp-driver -- --ignored --nocapture real_resume_acceptance_grok`：
turn 1 → 销毁实例 → 全新实例 `session/load` → turn 2 回忆上下文成功（22s）。首跑曾遇 turn 1
超时（疑 leader 进程残留/冷启动），重试即通过——瞬态待观察。

## 已知限制 / 未完成

- initialize 响应的 `_meta` 含 hostname/CWD/modelState 等供应商扩展；Hub 只保留 capability 判定
  所需字段，扩展值不进入 Canonical。
- `agentVersion` 经 `_meta` 上报但未放在 `agentInfo`，因此 driver 的 `native_agent_version` 为
  None；不据此伪造版本。
- 审批已经 PendingInteraction 透传，由用户选择供应商选项；不自动批准。

## 订阅额度接入（2026-09-04）

Grok 经 ACP 扩展 `_x.ai/billing`（`ClientRequest::ExtMethodRequest`，wire 方法保留 `_` 前缀）
读取订阅额度，`read_quota`（QuotaProvider）带 30s TTL 与瞬态单次重试。真实 payload 形状：
`config.currentPeriod.{type,start,end}`（USAGE_PERIOD_TYPE_WEEKLY 周窗口 ISO-8601 起止）+
`subscription_tier`（ACP 响应；兼容 subscriptionTier）。官方 config.creditUsagePercent 是可选
百分比字段；缺失时尝试旧 used/monthlyLimit，仍不可读则保留 Unknown，不以 onDemand 代替订阅。
兼容 billingPeriodStart/End，日期使用 RFC3339 parser。

2026-09-06 收尾：单请求 8 秒超时、重试、single-flight/缓存、失败保留 stale。
严格只读验收 probes_the_current_users_grok_billing_extension 已通过（真实窗口/reset 与缓存）。
初次验收失败定位为 CLI 环境清理丢失 HTTP(S)_PROXY，导致上游连接失败；保留用户代理配置后通过。
测试已经改成读取失败必须 fail，不再打印错误后宣称成功。无模型 Turn、无原始账号读数落文档。

2026-09-09：代理处理已从 ACP 私有 allowlist 提升为共享运行时能力。所有 Coding Agent CLI 都会
优先继承 `http_proxy`/`https_proxy`/`all_proxy`/`no_proxy`（同步大小写），缺失项通过原生系统配置源
补齐，不执行 `scutil`/`gsettings` 等命令：macOS SystemConfiguration、Windows WinINet/WinHTTP/
注册表、Linux GNOME GLib/GIO、KDE 配置或桌面沙箱 portal。macOS 原生探针在桌面用户会话已直接
读到 `http://127.0.0.1:7890`；清空全部代理环境变量后，Grok ACP Ready 和 `_x.ai/billing` 真实
只读探针仍均通过，证明端到端使用原生系统代理。无模型 Turn。PAC/WPAD 暂不能可靠转换成单个
CLI 环境变量，运行中修改其他 shell 的 `export` 也不会改变已运行进程。

2026-09-08：`creditUsagePercent`/兼容窗口明确达到 100% 时 UI 显示“已用尽”并禁用发送；ACP
prompt 的明确 usage/quota/credits 耗尽错误生成安全时间线提示。缺失百分比仍为 Unknown。
