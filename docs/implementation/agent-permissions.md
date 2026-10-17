# Coding Agent 权限策略与审批

实现参考本机 `codex-host` 的权限目录与 `docs/antigravity-tool-approval.md`，结合本项目现有审批持久化和原生协议通道。

## 交互与生效范围

输入框左下角提供权限策略菜单，展示策略说明、当前选中标记和危险策略颜色。支持键盘导航、Esc 和点击外部关闭。空闲时的选择按任务和 Agent 安装分别保存在本机，并随下一次 SubmitTurn 发送；事件记录包含 permissionMode。切换 Agent 不继承另一 Agent 的策略。过期或未知策略不沿用。

Cursor 支持运行中切换为完全访问：主窗口向服务提交 taskId、runId、executor 和模式，服务确认后才更新菜单与本机选择。服务将授权限定在这一轮，处理已经挂起和后续到来的工具审批；仅选择原生提供的允许一次选项，不创建永久规则，不回答用户问题，不绕过明确禁止的操作。审批决策仍先持久化再交给驱动，保留活动运行、安装身份、执行所有权和任务版本校验；并发手动答复、取消和迟到请求不能批准另一轮。

运行结束后清理运行中授权。页面重载时从服务读取当前生效模式，不用本机保存的下一轮选择代替当前策略。运行中由确认模式切换为完全访问后可以切回确认；若 Cursor 本轮以原生 `--force` 启动，则收紧权限需要先停止运行，后端会明确拒绝无法生效的切换。其他 Agent 暂不开放运行中切换。

审批卡展示工具操作内容及可用选项。一次批准仅作用于当前请求，持久批准只有原生协议提供此选项时才展示。继续沿用 Core 的当前任务、活动 turn、请求 ID、选项 ID 与乐观锁检查。取消不产生批准。

## Agent 映射

| Agent | 策略来源与执行方式 | 审批 |
| --- | --- | --- |
| Codex | 只读、默认、完全访问；每轮传入 approvalPolicy 与 sandboxPolicy | app-server 原生请求与响应 |
| Claude Code | plan / default / acceptEdits / bypassPermissions；原生模型目录明确 supportsAutoMode 时增加 auto；CLI 启动参数 | can_use_tool 控制请求；策略变化关闭旧进程后恢复原会话 |
| ZCode | 已安装协议的 build / edit / plan / yolo；session/setMode | interaction/requestPermission 原生选项 |
| Kimi | ACP session/new 返回的 availableModes；session/set_mode | session/request_permission |
| Grok | ask / auto / always-approve；session/new/load 的原生 _meta.yoloMode 与 autoMode | ACP 审批通道 |
| Cursor | 操作前确认 / 完全访问；启动时映射原生 `--force`，运行中按当前轮授权处理审批 | ACP 审批通道，运行中授权仅使用原生允许一次选项 |
| Antigravity | 配置规则、桌面审批、跳过权限检查 | 桌面审批通过验证后的 PreToolUse Hook 实现 |

## Antigravity 桌面审批

默认仍使用已配置的原生规则。要在本应用中审批，应选择“桌面审批”。原生无界面 CLI 自身无法等待交互权限回复；该模式使用进程级跳过权限参数，并由私有全工具 Hook 控制实际执行。

启动任何模型输入前，运行 `/hooks` 检查相同工作区与私有目录的有效配置，严格匹配启用状态、来源路径、PreToolUse、全工具 matcher 与命令。缺失或不匹配时不启动工具。Hook 调用当前桌面可执行文件的专用参数分支，不初始化 GUI；通过带随机令牌的本机回环连接等待批准。

请求必须属于当前活动父会话或原生事件登记的子会话。只有允许一次和拒绝；超时、取消、无连接、错误令牌、未知会话、畸形数据都拒绝。并发连接数及数据量有上限。ask_question/ask_permission 不伪装成工具授权。后台脱离当前 turn 的子任务不会获得批准。

工具错误现在保留 tool_info.error 的可显示内容。权限被拒且没有正文的 turn 标为失败，不再显示空白的“成功完成”。

## 验证与限制

- 已验证菜单浅色/深色外观、选中和关闭交互。
- 审批桥测试覆盖等待决定、允许、拒绝、取消、未知会话、错误令牌、重复答复、原生会话别名及 Hook 配置匹配。
- 前端测试覆盖策略选择、按任务隔离、过期策略、加载失败、运行中应用确认与失败回退、审批面板切换及不支持的运行中操作禁用。
- 服务与 SQLite 集成测试覆盖当前及后续审批、权限收紧、跨轮隔离、并发手动答复、取消后的迟到变更，以及不代答用户问题。
- 原有各驱动审批测试继续运行；本轮未重新执行七个 Agent 的付费端到端任务。检测到另一个独立 agy 进程，未打断它或并发启动真实 Antigravity 登录进程。
- Antigravity 的私有 Hook 接口可能随 CLI 版本变化；每次启动都会验证有效配置，不能验证时拒绝启用桌面审批。
- 同时修复 turn-changes Artifact 未列入本地读取允许类型的问题，并通过 Core 读取实际捕获记录的测试验证。
