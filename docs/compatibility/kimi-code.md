# Kimi Code 兼容记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Kimi 协议细节与历史证据。

更新日期：2026-09-19

## 本机事实（实测）

- 安装：`~/.kimi-code/bin/kimi`，`kimi --version` → `0.41.0`（裸 semver，无产品名）。
- 产品身份：`kimi acp --help` 输出 "Run kimi-code as an Agent Client Protocol (ACP) server over
  stdio."，故 manifest 使用 `identity_args = ["acp", "--help"]` 承载 marker，`--version` 承载 semver。
- ACP：`kimi acp` 原生提供 stable v1 stdio server。initialize 实测返回
  `loadSession=true`（session/list、resume、close、delete、fork、additionalDirectories）、
  promptCapabilities `image=true, embeddedContext=true`、authMethods 为终端设备码登录。
- 登录态：`~/.kimi`（旧 kimi-cli）与 `~/.kimi-code`（当前 CLI）均存在；Hub 不读取其中内容。
- 真实就绪探针：`cargo test -p acp-driver -- --ignored` 中
  `probes_the_current_users_kimi_installation` 通过（resume=Supported；本轮版本 0.41.0）。

## 接入形态

- manifest：`drivers/manifests/kimi.toml`（protocol kind `acp`，`launch_args = ["acp"]`）。
- Driver：`acp-driver`（官方 SDK stable v1，`default-features = false`）。
- 审批：PendingInteraction 按供应商选项透传，由用户决定；不自动批准。
- 已实现跨进程 session/load；真实验收记录见下文。

## 真实 Turn 验收（2026-09-03，操作者明确授权后执行）

`cargo test -p acp-driver -- --ignored --nocapture real_turn_acceptance_kimi`：
- 轮 1（固定 no-tools prompt）：流式 TextDelta + EndTurn → `turn.completed` ✓
- 轮 2（同一 Session）：live 复用 ✓
- 轮 3（长任务 + 2s 后 session/cancel）：`TurnInterrupted` ✓

验收中发现并修复两个真实缺陷：
1. CLI 冷启动时内部 auth 子系统未就绪会出现一次瞬态 `Authentication required`；此后 session/new
   稳定成功。ACP probe 因此加入 session/new 验证，把真正的未登录映射为 NotConfigured。
2. ACP driver 的 connection loop 在 await Prompt 期间不轮询 cancel 通道，session/cancel 会等
   本轮跑完才发出；已改为 Prompt 在独立任务执行，cancel 实时生效。

## 跨进程 resume 验收（2026-09-04 通过）

`cargo test -p acp-driver -- --ignored --nocapture real_resume_acceptance_kimi`：
turn 1 → 销毁实例 → 全新实例 `session/load`（Hub 传入项目路径）→ turn 2 回忆上下文成功，
session id 不变。`resume_session` 的 `working_directory` 参数由 Hub 提供项目路径。

## 订阅额度接入（2026-09-06 交付）

- 2026-09-19：兼容 Kimi Code 2.0.1 的 `data.quota.usages.limit5h / limit7d`，
  `usedRatio` 转换为已用百分比，`resetAt` 保留原始重置时间；同时保留旧版
  `data.summary / limits` 的解析。缺失或非法窗口不生成额度，接近 100% 时不提前舍入为用尽。
  新版 HTTP 200 中的 `kind: error` 单独处理，401 提示重新登录，其他上游错误保留安全状态码。
  仍使用本机官方额度接口，不创建模型 Turn；钱包与未知额度类型不推算为订阅窗口。

- 官方本机管理服务：kimi web --port 0 --no-open → GET /api/v1/oauth/usage。
- summary 与 limits 映射周/5h 窗口；保留供应商 reset_at，缺失字段不推算。
- 临时 token 仅内存使用；loopback 请求不走代理、不跟重定向，校验状态码与 body 上限。
- 独立于 ACP 对话连接，single-flight + 30 秒缓存/失败退避；超时与取消回收子进程。
- 上下文只观察已有 ACP UsageUpdate，按 session 隔离；不再向会话发送 /usage。
  旧实现文档所说“刷新必定同时取得上下文”不成立，未收到推送时保持未知。
- 本机只读验收 real_kimi_subscription_read_only 通过：包含 5h 窗口和已用百分比；
  重复读取命中缓存，没有 ACP 会话与模型 Turn。
- HTTP fixture 覆盖 chunked、错误状态、重定向和响应上限。完整验证见 M1-S6 / HANDOFF。
- 2026-09-08：任一新鲜 5h/周窗口剩余 0% 时 UI 显示“已用尽”并禁用发送；ACP prompt
  明确报告 usage/quota/credits 耗尽时也生成安全时间线提示。普通 429 保持普通失败。
