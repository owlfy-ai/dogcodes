# 原生模型与思考强度选择

模型目录由已注册的 Agent Driver 读取，包含原生模型标识、显示名称、每个模型支持的思考强度和默认强度。前端不维护模型名称白名单。

| Agent | 目录来源 | 配置方式 |
| --- | --- | --- |
| Codex | App Server `model/list`，遍历分页 | `thread/start` / `thread/resume` 的 `model` 与 `config.model_reasoning_effort` |
| Claude Code | stream-json `initialize` 的 `models`，保留第三方后端别名 | `--model`、`--effort`、会话 `--settings` 的 `alwaysThinkingEnabled` |
| Grok / Kimi / Cursor | ACP initialize 元数据或无 Prompt 的 `session/new`，保留完整 `models` / `configOptions` | 优先 `session/set_config_option`；否则原生 `session/set_model` |
| Pi | RPC `get_available_models` / `get_available_thinking_levels` | `set_model` / `set_thinking_level`，随后读取状态确认 |
| OpenCode / DeepSeek Harness | ACP `session/new` 的分组或平铺 configOptions | 原样使用 `session/set_config_option` |
| Kiro | 登录后的 `_kiro/config/template` 异步目录及 session configOptions | `session/set_config_option`；权限另用 autopilot |
| Antigravity | `agy models` 的原生 ID 与显示名称 | 界面拆分 low/medium/high 后缀，启动时还原已报告的完整模型 ID |
| ZCode | `workspace/readState` 的 `settings.model.available` 与 reasoning levels | `session/setModel` 与 `session/setThoughtLevel` |

Claude Code 的 off/auto/effort 语义参照 codex-host Claude adapter：off 关闭 thinking，auto 保留自适应默认，其余值使用 CLI 原生 effort。ZCode 模型标识无损序列化原生 provider/model/variant 引用，界面仅展示 label。

Core 保留携带 task 读取项目目录的接口；主界面的模型菜单现采用下述启动预加载与全局缓存，不随任务工作目录重新探测。ACP 部分 Agent 仅提供当前模型的思考选项；缓存目录未报告可选强度时不生成通用强度菜单，也不从 Cursor 模型标识的方括号参数推测枚举。

模型控件紧邻 Agent 图标左侧，模型和思考强度分别打开菜单。直接列出模型，不提供搜索框。模型列表加载失败可重试；切换任务、Agent 或模型后，旧请求结果不会覆盖当前目录；运行中的任务不由选择控件触发目录探测，启动预加载独立进行。

`SubmitTurn.model` 与 `SubmitTurn.thinking` 独立传递至 Driver，并写入会话映射。数据库迁移到版本 7，旧记录 thinking 默认为空，事件重建保留两项选择。沿用原有会话约束：已有会话保持其模型与强度，更换请新建对话；旧 Antigravity 完整模型 ID 继续兼容。新会话发送前校验原生目录及模型对应的强度，拒绝不支持的组合。

验收覆盖：目录解析、每模型强度、前端菜单交互、非法组合拒绝、选择传递、存储与事件重建。本机已通过七种 Agent 的目录读取，及 Kimi/Grok 无 Prompt 的模型与强度配置检查；这些验收不发送模型 Turn。真实模型回复质量不在这组测试范围内。

## 启动预加载与运行期缓存

主界面通过 core_model_ready_executors 确认当前进程已注册驱动后，后台预加载模型目录；持久化的 Ready 状态不代表本次运行初始化完成。监听驱动扫描完成事件，并每 5 秒轻量查询注册列表以兜底事件遗漏（不重复读取模型目录）；优先当前 Agent，最多两个并发查询。以 providerId、installationId 和 version 为缓存键，不带 Task/工作目录；同一安装实例跨会话复用，合并正在进行的请求。结果在当前主界面生命周期内保留；不持久化账号相关列表。切换时同步使用已缓存结果，避免闪现加载状态。

模型菜单不展示刷新或重试按钮；后台首次失败后分别等待 1.5 秒、4 秒再尝试，最多读取三次；等待重试不占并发槽位。下拉列表与预加载共享完整重试请求，恢复后自动显示结果。全部尝试失败后才显示暂不可用，下次打开菜单时静默重新尝试，不进行无限重试。安装实例或版本变化使用新缓存；账号变化尚无统一通知，重启应用可重新加载。每个会话的 model/thinking 选择继续独立保存，实际发送仍走原生校验。目录未报告的模型专属思考选项不推测生成。
