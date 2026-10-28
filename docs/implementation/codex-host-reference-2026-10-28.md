# codex-host 最新代码对照：ZCode 与可借鉴改动

对照日期：2026-10-28。

参考仓库 `/Users/macbookpro/Workspace/opensource/codex-host` 已执行 `git pull --ff-only`，
从 `a8adb78b` 更新到 `79283084`，共 69 个提交、367 个文件变化。更新后工作区干净，
本地 `main` 与 `origin/main` 一致。以下结论来自当前源码及与 DogCodes 的对照；没有启动
codex-host、更新依赖或运行其原生模型测试。

## ZCode 剩余额度：参考仓库尚未提供这条实现

当前 ZCode Adapter 暴露 `usageStatistics`，没有暴露账号额度的 `accounts` 能力。
全仓未找到 ZCode 的 `/api/monitor/usage/quota/limit` 查询实现。

- `src/usage.ts`：处理单请求输入、输出、缓存、思考 token 及请求计时。
- `src/usage-statistics.ts`：只读 `~/.zcode/cli/db/db.sqlite` 的消息记录，按稳定消息 ID
  统计历史请求；数据库指纹包含 WAL，避免漏掉未 checkpoint 的提交。
- `src/account.ts` 的 Start Plan `billing/balance`：用于确定有效套餐和允许的模型，
  没有把余额转换为本项目的 5 小时/每周额度卡片。

所以不能用它的 token 统计替换订阅剩余额度。DogCodes 本轮新增的 OAuth 只读监控
是不同能力，当前 BigModel 实测可用，Z.ai 仍仅有隔离测试证据。

源码：[请求统计](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/usage.ts)、
[历史统计](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/usage-statistics.ts)、
[适配器能力](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/adapter.ts#L62)。

## ZCode 可用模型：原生目录加账号权益投影

其查询链路是：

1. 读取已安装 App 的内置 Provider 配置和当前原生账号。
2. Start Plan：用 JWT 和设备 ID 查询 `billing/balance`，过滤有效且未过期的套餐，
   从属于这些套餐的余额 capability 解析可用模型。
3. 个人 Coding Plan：读取当前家族、账号 ID、个人连接选择，以及该账号对应的
   `account-provider:coding-plan:<providerId>:account:<id>:api-key`；用此 Key GET
   业务域的 `/api/biz/subscription/list`，验证当前有效 Coding Plan。
4. 启动 App 自带 Electron Helper 的 CLI，发送 `provider/updateAccountConfig`，
   核对回执 revision，确保使用同一内置配置层。
5. 创建 `persistence: deferred` 原生草稿，从 `settings.model.available` 读取完整目录，
   立即按 deferred 语义关闭，不发送模型输入。
6. 排除带 `disabledReason` 的模型，保持 Provider/Model 身份和原生思考选项。

这些主要机制 DogCodes 已有：内置配置投影、权益核验、revision 回执、临时草稿、
原生目录及思考选项。没有必要再引入一份固定模型名单或重复实现查询层。

**它也存在本机新版认证的缺口。** 个人套餐仍要求旧版持久化 API key，缺失时直接
返回 `credential-failed`；没有新版个人项目 Token 的解析、签发或刷新实现。因此，
按其源码逻辑，本机这个缺少个人 Key 槽的账号也无法通过个人模型授权。此结论是
源码推断，没有用该 Adapter 对真实账号执行模型请求。

源码：[账号权益](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/account.ts#L170)、
[个人套餐 Key 与核验](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/personal-coding-plan.ts#L145)、
[临时目录查询](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/adapter.ts#L43)、
[模型过滤和选项](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/models.ts#L49)。

## 本次新代码的借鉴顺序

| 优先级 | 改动与证据 | 对 DogCodes 的意义及边界 |
| --- | --- | --- |
| P0 | ZCode 的 `signedIn`、`startPlanChecked` 和空目录诊断；提交 `693be7a2` | 区分未登录、已登录但无模型、权益查询失败。我们目前仍会归并为“未配置”；应增加结构化诊断，并继续兼容新版个人项目认证。不能因额度可读就判模型可执行。 |
| P1 | 原生运行中追加输入；`external-turn-steering.ts`，Claude continuation 系列修复 | 可作为后续“运行中补充指令”的参考。明确原生已接收后即使界面投影失败也不能重发；必须保留原 Turn、请求身份及取消边界。不能只添加按钮。 |
| P1 | `usage-statistics-sources.ts`；提交 `0e221235` | 读取时来源变化是可重列举的暂态，最多两次短退避，只重读变化来源。适用于今后的历史用量聚合及正在追加的日志，不能直接重试不确定的模型提交。 |
| P1 | `usage-statistics-calendar.ts`；提交 `47e0b210` | 手机和远程主机时区可能不同。将展示时区传至聚合端，从原始时间戳归日，按日历加减日期处理 DST；不能只平移已聚合小时桶。当前订阅 reset 已用绝对时间显示，主要价值在后续全局统计。 |
| P2 | `harness-installation-tasks.ts`；提交 `d6dad4fc` | 升级操作由后台拥有，提交立即返回任务 ID，页面可查询同一操作。我们已有更新锁、状态缓存和资源租约，值得补的是跨窗口查询及操作回执；参考实现只有内存 Map，不具备服务重启恢复。 |
| P2 | `composerIndicators` 与 Claude 提示词缓存倒计时；提交 `a2b33e65` | 可用声明式指标展示缓存相关提示，前端不用判断供应商。必须标注为预计时间，不能把倒计时当作实际缓存命中或订阅重置证明。 |
| 可选扩展 | 新 Copilot 官方 SDK Adapter；提交 `a95f9905` | 若增加 Agent，可参考权限、提问、后台任务与账号额度的能力清单及测试分层；其文档仍标注 Desktop/远程等验收未完成，不能按完整交付直接移植。 |

补充限制：本次 `main` 曾加入桌面调度，随后由 `a6e7e3a7` 撤回，不能将已撤回功能
算作当前可借鉴实现。Codex Desktop 私有语言服务兼容属于其宿主特性，DogCodes 的独立
Tauri 应用不需要移植这层。

参考源码：

- [ZCode 新诊断](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/zcode/src/adapter.ts#L95)
- [运行中输入交付](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/host-runtime/src/external-turn-steering.ts#L142)
- [变化来源的有限重试](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/host-runtime/src/usage-statistics-sources.ts#L16)
- [统计日历与时区](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/host-runtime/src/usage-statistics-calendar.ts)
- [后台安装任务](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/host-runtime/src/harness-installation-tasks.ts)
- [提示词缓存指标](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/claude-code/src/claude-prompt-cache.ts#L24)
- [Copilot 能力与验证边界](https://github.com/BytePioneer-AI/codex-host/blob/79283084/packages/adapters/copilot/CAPABILITIES.md)

## DogCodes 当前修复和实际验证

本轮解决了两个相互独立的原因：新版原生账号缺少旧 Key 时，额度 GET 使用当前 OAuth
登录态；模型检测失败时仍独立注册额度 Provider，Core 查询不再仅枚举可执行 Driver。

- 当前后台的真实管理页 RPC 返回 5 小时与每周两个窗口，没有 diagnostic。
- 真实 dog.codes 管理页已确认 ZCode 显示 99% 和 90%（本次检查时的快照）。
- 卡片仍标“未配置”是模型目录未就绪的状态；额度恢复不代表新版个人模型认证已经修复。
- 新增后台注册到桌面额度接口的隔离回归、Core 安装筛选/去重/移除回归，以及未配置
  状态下卡片显示双窗口的前端回归；这些均已通过。

后续适配进展：DogCodes 已接入新版个人项目短期 Token，并通过本机 BigModel 的
原生就绪检测、GLM-5.3 / GLM-5.3-Flash 模型目录及运行时凭据回调验证，未发送真实
模型输入。上文关于 codex-host 仍依赖旧 Key 的判断不变；本项目此处的认证缺口已补齐。
新构建的真实 Agent 列表与模型菜单也已验证：ZCode、GLM-5.3、GLM-5.3-Flash 均可选择。
具体边界见 [ZCode 兼容记录](../compatibility/zcode.md)。精确诊断、运行中输入和历史
统计增强仍可按上面的优先级继续评估。
