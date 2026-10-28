# Pi、OpenCode、DeepSeek Harness、Kiro 接入

2026-09-28。参考本机 codex-host 的适配器与原生协议资料，按 dogcodes 的 Rust Driver 接口实现；没有调用 codex-host 服务，也没有复制其适配器代码。

## 接入方式

| Agent | 原生入口 | 本机验证版本 | 会话恢复 |
| --- | --- | --- | --- |
| Pi | `pi --mode rpc`，新增 `pi-driver` | 0.87.1 | 原生持久会话文件；常驻进程复用 |
| OpenCode | `opencode acp`，共享 ACP 驱动 | 1.15.10 | `session/load` |
| DeepSeek Harness | `dsh --profile acp`，共享 ACP 驱动 | 0.1.7-rc.2 | 协商 `sessionCapabilities.resume`，使用 `session/resume` |
| Kiro | `kiro-cli acp --agent-engine v3 --auth-method cli`，共享 ACP 驱动 | 2.24.1 | `session/load` |

codex-host 的 Pi/Kiro 分别采用 RPC/ACP；OpenCode 采用 HTTP/SSE，DeepSeek 采用 Web Remote。本项目采用后两者已提供的原生 ACP，复用现有流式、附件、审批、取消、模型目录和会话状态管理，避免引入额外 HTTP/WebSocket 桥。并不表示 codex-host 的所有供应商扩展功能都已移植。

## 具体行为

- 新增四个内置 manifest、安装路径、原生探测和驱动注册；欢迎页、Agent 菜单、灵动岛名称/图标接入。无订阅额度接口时显示“连接就绪”，不等待或伪造额度授权读数。
- 优先终端选中的安装，再查已知路径。DeepSeek 额外识别已存在的 `~/.npm/_npx/*/node_modules/@deepseek-ai/dsh`，校验包名和原生身份；不调用 npx、不下载或改动原生安装。
- Pi 0.87+ 使用请求 ID 关联控制回复，持续读取有大小限制的 NDJSON；以 `agent_settled` 作为终态，避免把 `agent_end` 后的重试/压缩截断。显示增量正文、原生思考、工具输入/结果及扩展对话框，校验回答后转发原生响应。
- Pi prompt 发出后立即交还事件流，允许原生 preflight 在确认接收前询问用户。取消先关闭扩展询问，再发 `abort`，超时回收进程；死亡连接在下一次恢复时替换。正常关闭真正释放 stdin 句柄，交给 Pi 清理，超时才 kill。
- Pi 按 `provider/model` 区分模型；思考档位实时询问当前模型，设置后检查实际状态。文本交接/对账按顺序放在用户提示词前；图片使用原生 images，其他文件使用路径引用。
- Pi 原生不提供可选的逐工具审批或沙箱。权限菜单明确说明工具直接执行，扩展询问仍可交互；不伪造安全权限模式。Pi 未提供原生 MCP 配置，因此 Core 不为它创建历史 MCP 租约；原生上下文、显式交接和对账保留，跨 Agent 历史按需查询暂不支持。
- Kiro 的工作模式与权限分开：权限使用真实 `autopilot`，默认 off（Supervised），on 明确标为完全访问。登录后模型目录异步刷新，等 `_kiro/config/template` 提供目录再开放配置；不内置模型清单。
- ACP 恢复优先使用已协商的 `session/load`，仅有新 resume 能力时调用 `session/resume`。保留每个会话的工作目录/MCP 绑定检查。DeepSeek 分组目录与 JSON 形式模型 ID 原样保留。
- OpenCode 1.15 在原生错误时可能返回无任何模型/工具活动的 `end_turn`。这种结果明确显示失败，不伪装为任务完成。
- 新 Agent 的自动升级、供应商专用工作流、桌面 App 会话导入不在本次范围；使用安装好的 CLI 及其原生登录/配置。Kiro 账户额度已在后续补齐（见下）；其余三家尚未接入账户额度查询，不代表原生服务不提供额度。

## 验证与限制

- Pi 离线测试覆盖增量/最终正文去重、Unicode 分隔符、模型选择确认、热复用、跨进程恢复、提前提问、取消后继续、断连/畸形记录后的恢复与有序上下文。
- 三个 ACP 的本机元数据验收通过：模型目录、权限选择、创建、热复用和跨进程恢复。观测模型数：OpenCode 8、DeepSeek 3、Kiro 9；目录会随账户和原生服务变化。
- Pi 真实模型：短回复、跨进程随机标记记忆恢复、中途停止均通过。
- DeepSeek/Kiro 真实短回复通过。OpenCode 本机默认数据触发 `failed to run data migrations`；使用隔离数据目录后，免费模型服务返回“OpenCode 1.18.0 or newer is required to use the free tier”。没有更改、删除其现有数据库或升级用户 CLI。
- OpenCode 用真实 1.15.10 CLI、独立数据目录和本地 SSE 模型完成接入验收，收到 4 个正文增量；该结果证明 ACP 驱动链路，不证明本机免费模型账户可用。复现：`python3 experiments/agent-integration/opencode_local.py /absolute/path/to/opencode`。
- Rust 相关 lib 测试通过（ACP 38、Registry 11、Core 41、Desktop 86、Driver API 14、Pi 6）。新驱动与 Registry/API 的严格 Clippy 单独验证。
- 前端接入相关 33 项用例通过，类型检查与生产构建通过。全量前端测试发现两个未修改模块的失败：`ConversationDeletion.test.tsx` 永久删除按钮选择器、`DraftConversation.test.tsx` 项目选择断言。Desktop 全目标严格 Clippy 另有三个未修改位置的警告：`turn_timing.rs` 嵌套 if，`review.rs` / `review/recorded_changes.rs` 测试中的 format/collect；不将这些报告为全部通过。
- 本轮未做四家真实工具写入/非 Git 撤销、图片、独立审查的完整桌面回归；不以协议共用推断所有供应商功能均已实测。

原生资料：Pi 安装包 `docs/rpc*.md`、DeepSeek `@deepseek-ai/dsh-acp` 0.1.7-rc.2 源码、Kiro ACP 配置扩展、本机 codex-host `packages/adapters/{pi,opencode,deepseek-harness,kiro-cli}`。图标来源和许可证见 `apps/desktop/src/assets/licenses/`。

## 后台加载

构建完成后通过 `retire_if_idle` 确认旧服务空闲并替换为新构建。生产数据目录的四个新增安装均已注册、状态为 Ready：Pi 0.87.1、OpenCode 1.15.10、DeepSeek Harness 0.1.7-rc.2、Kiro 2.24.1。Ready 表示原生连接/会话探测通过，不覆盖上述 OpenCode 模型服务限制。

Pi 真实元数据验收在关闭 stdin 句柄的修复后耗时约 3.8 秒（包含探测、模型目录、创建与清理多个进程）；六个离线回归约 3.35 秒。DeepSeek ACP 当前未声明图片能力，图片附件会明确拒绝，不静默丢弃。

## Kiro 账户额度补齐（2026-09-28）

- 通过已认证的 ACP 连接调用 `_kiro/account/getUsage`，空参数即可读取账户用量；无需会话 ID，不创建对话、不发送模型请求。查询独立于 prompt 调度，沿用账户缓存、并发请求合并、30 秒有效期和强制刷新。
- `usageBreakdowns` 的 CREDIT 资源映射为“套餐内（Kiro）”当前账期剩余比例；由原生 `used / limit` 计算，避免原生整数 percentage 丢失小数。无限额度、错误或缺少有效读数不伪造成 0%/100%。
- 套餐内用尽不作为阻止对话的依据；赠送、加购以及按量付费可能继续支持调用。当前只展示套餐内比例，额外额度存在时在套餐说明中标注，不将未经验证结构的额外额度合并为总余额。
- 原生 `billingCycleReset` 只包含日期时保留日期文本，不虚构时区或重置时刻。具有时区的完整时间才转换成时间戳。
- 注册 Kiro QuotaProvider，首页重新检测、额度面板与输入框额度环接入现有刷新机制；其余未接入 Agent 的提示改为“暂未接入账户额度查询”。
- 验证：Kiro 2.24.1 真实账户读取、缓存命中、强制刷新通过；ACP lib 42 项通过，前端相关 47 项通过。覆盖异常额度、日期、附加余额、并发缓存和刷新失败保留原始过期时间。
- ACP 全目标严格 Clippy、前端类型检查/生产构建和桌面构建通过。空闲后台已加载新构建，实际服务 RPC 读到 `kiro-account-usage-v1`、`native_pull` 和有效账期额度，diagnostic 为空。

## Kiro 模型思考强度适配（2026-10-20）

- 识别原生 `effortLevel`，按模型 `_meta.kiro.hasEffort/effortLevels/defaultEffortLevel` 构建选项。
  支持字符串或对象形式的档位、去重及有效默认值检查；当前模型的实际配置优先于模板。
- 切换模型后先使用原生返回的新选项，再设置思考强度，并核对原生确认的实际值；
  不接受无效档位或把未确认写入当成功。auto 路由不展示思考强度，不支持调节的模型也不补造菜单。
- 协议回归覆盖模板/当前目录差异、模型切换、固定强度模型和原生拒绝设置。
  原生未公开可调档位时，UI 继续隐藏下拉，这是该模型的实际能力。
- 本机 Kiro 元数据验收通过（8 个模型、2 个权限模式，创建/热复用/关闭后恢复），
  未发模型任务。三家 Driver 严格 Clippy 与桌面 `cargo check -p desktop --locked` 通过。
