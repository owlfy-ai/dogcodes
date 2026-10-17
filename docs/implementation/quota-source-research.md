# ZCode / Cursor / Antigravity 订阅额度数据源研究

日期：2026-09-08。仅研究及只读探针，未新增产品接入。源码线索不等于真实验收。

最新实施：Cursor macOS 受控 PTY 自动采集已实现，真实自动读取/退出测试通过；读取 Included 用量，精度为原生整数，reset 月日仅文本展示。Antigravity init 零 turn 探针确认没有 model 字段，模型列表无当前标记，自动关联不能猜测。ZCode 新账号授权范围等待用户决定。

最新续记：Antigravity 新实现真实只读测试现已通过（包含缓存一致性）；Cursor 原生 TUI `/usage` 已真实验证，显示 Pro Included 15% used、Auto 16%、API 0%、Resets Oct 4、On-Demand Disabled。Cursor 自动化采集尚未实现；终端只给月日，不可伪造精确 reset。以下较早的审批阻塞记录已被本次验证覆盖。

实施续记：Antigravity QuotaProvider 现已编码并注册桌面；scope 分组 UI 已接入，当前模型组未确定时不显示猜测的全局百分比。新实现真实验收被沙箱/审批服务限额阻塞，尚未验收通过。Cursor/ZCode 尚未实现读取通道。

## Antigravity：原生命令已真实验证

- 本机 agy 1.1.27：`agy --print=/usage --output-format stream-json`。
- 返回 `command_result`，`command.name=usage`，数据位于 `command.data.groups[].buckets[]`。
- group 区分 Gemini Models 与 Claude and GPT models；bucket 含 `id`、`window`（5h/weekly）、`remaining_fraction`、`reset_time`。
- 本次 Gemini 剩余约 99.51%（5h）、99.91%（周），Claude/GPT 两窗口均 100%。这是当时快照，不是当前保证。
- 结果 SUCCESS，`num_turns=0`，所有 token 计数为 0；约 9 秒完成。沙箱首次因本地日志写入/loopback 绑定失败，批准后原命令成功。
- 参考实现：邻仓 `codex-host/packages/adapters/antigravity/src/quota.ts`。命令必须独立执行，不混入 stream-json 输入会话。
- 下一步：QuotaProvider、超时/退出回收、单飞缓存、脱敏 fixture、真实重跑、桌面注册及 UI 切换验证。

## ZCode：桌面有订阅服务，CLI 尚无已验证读取通道

- `/Applications/ZCode.app/Contents/Resources/app.asar` 静态确认：
  - `usageStatsService.getEntitlementSnapshot`，内部 channel `usage-stats`。
  - BigModel/Z.ai quota GET 路径 `/api/monitor/usage/quota/limit`。
  - 请求有 provider、organization、project 区分；认证由原生 credential service 管理。
- `/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs` 的 `usage/stats` / `session/usage` 不等于以上桌面订阅服务；未发现上述 entitlement 方法。
- 官方说明 ZCode 使用账号 Coding Plan 套餐额度：https://zcode.z.ai/cn/docs/configuration 。
- 优先研究原生桌面是否有受支持的只读桥接；**未验证外部进程可调用内部 IPC**。内部 channel 名不能直接当 app-server RPC 使用。
- 直接请求 quota endpoint 需要合适的账号认证；当前不读取用户凭证文件、不增加密钥存储的边界下，不能宣称已可直接接入。若需新增授权流程应先确认产品范围。
- 只查 quota，不调用 reset / claim / subscription mutation。失败保持 Unknown，不以历史 tokens 推算订阅余量。

## Cursor：本机 CLI 有原生 /usage，但自动化出口未验证

- 本机版本目录 `2026.09.02-c22c1a3` 的 `4347.index.js` 包含 `src/usage/usage-command.ts` 和 `usage-data.ts`。
- `/usage` 注册为本地命令，调用 `getCurrentPeriodUsage`、`getHardLimit`、`getPlanInfo`，展示 included total/auto/api 使用百分比、账期重置日期和 on-demand 状态。
- 部分套餐返回 unavailable；企业分支可能只有花费，不能当剩余百分比。
- `--help` 没列 usage 子命令；未验证 print/ACP 能执行本地 `/usage`。不要直接发 `/usage` prompt 假设它不会调用模型。
- 下一步优先验证受控原生 CLI /usage 输出；若只能 TUI，需单独评估 PTY 解析可靠性及认证边界。内部 RPC 是静态线索，不是公开稳定个人 API。
- 官方 Team Admin API 是另一条路径，但需管理员 API key，只适用团队管理，不能冒充通用个人额度接口：https://prod.cursor.com/docs/account/teams/admin-api 。

## 共同接入前置

现有 QuotaWindow 只有时长/百分比/reset，缺少模型组和计费作用域；UI 对任一新鲜 0% 窗口禁用发送。
必须先加入作用域选择：Antigravity 的 Gemini 耗尽不能封禁 Claude/GPT；Cursor included 耗尽且允许按量计费时不能一律判不可用。
圆环按当前模型适用窗口取最短限制；无法确定模型组时显示分组或 Unknown，不猜选组。
建议次序：作用域语义 → Antigravity 实装验收 → Cursor 原生命令探针 → ZCode 桌面授权桥接探针。
