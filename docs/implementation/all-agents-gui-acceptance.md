# 全 Agent 原生 GUI 验收（2026-09-12）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

范围：当前产品八家 Provider，Fake 除外。ZCode、Antigravity 已有独立 GUI 证据；本批补齐 Codex、Claude Code、Kimi、Grok、Cursor、CodeBuddy。每家审批、取消、重启和断连分别记录，不把 Driver 测试当 GUI 通过。

| Agent | 当前状态 |
| --- | --- |
| ZCode | 主要矩阵已通过，见 recovery-gui-acceptance.md、interruption-late-approval-acceptance.md。 |
| Antigravity | 主要矩阵已验收，运行取消不保证 shell 终止；见 antigravity-gui-acceptance.md。 |
| Codex 0.153.4 | 本批桌面与灵动岛批准/拒绝、正常原生续接、待审批取消及重启回忆、运行取消请求、运行硬杀后的未知状态均已检查。运行取消未终止已启动 shell，不能标为命令停止通过。 |
| Claude Code 2.1.266 | 主要矩阵已检查；后台 Shell 分类已修复并通过原生 GUI 复验。 |
| Kimi 0.42.0 | 主要矩阵已验收；长命令审批摘要截断已修复，修复版桌面/岛完整详情及岛内批准落盘复验通过。 |
| Grok 1.0.30 | 两端审批、正常原生续接、待审取消后历史恢复、运行取消、硬杀未知状态均已检查；Shell 在运行取消后仍可继续。多种拒绝选项的岛内兼容已修复复验。 |
| Cursor 2026.9.10-fd3934a | 两端审批、正常原生续接、待审取消后历史恢复、运行取消、硬杀未知状态均已检查；拒绝误显示完成已修复复验。 |
| CodeBuddy 2.150.0 | 两端审批、正常原生续接、待审取消后原生续接、运行取消、硬杀未知状态均已检查。 |

## Codex

使用 `/Applications/ChatGPT.app/Contents/Resources/codex` 0.153.4，默认模型和思考配置，default 权限；所有写入均逐项原生请求审批，未全权限放行。

- 桌面批准：codex-desktop-allow.txt 内容 CODEX_DESKTOP_ALLOW；拒绝：codex-desktop-deny.txt 不存在，无重试。
- 灵动岛允许一次：codex-island-allow.txt 内容 CODEX_ISLAND_ALLOW，桌面同步完成和 Diff；拒绝：codex-island-deny.txt 不存在，无重试。
- 正常退出并重启：原生 thread ID `01a093c9-1afd-7fd1-aa17-fa1ad9317f2b` 不变，无工具回忆 CODEX-GUI-CEDAR-246。
- 待审批点击停止：卡片消失，codex-cancel.txt 不存在；再次退出重启后无审批、无旧命令重放，新请求正确回忆标记。session.rebuild_context_prepared 明确表明此分支使用历史包重建，不计为原生 UUID 不变的续接。
- 运行取消：已确认 START 且 END 不存在时点击停止，轮次结束并保留起点文件；但超过 60 秒后 END 出现。只能证明取消请求送达，不能证明终止外部 shell。
- 硬杀：第二条独立命令在 START 出现且 END 未出现时精确 SIGKILL 隔离宿主；重启后侧栏、桌面和岛均待确认，旧工具结果未确认，停止禁用、Run 仍占用，事件 turn.connection_lost。未伪造终态。
- 未在本批点击确认外部结束，保留隔离任务未知状态作为证据。

### 旧安装限制

0.128.0 CLI initialize 成功，但 thread/start 失败：本机原生配置 service_tier=default 不受该版支持（只接受 fast/flex）。未修改用户全局配置，也未切到可能改变计费的档位。该安装不能记为会话验收通过，继续使用当前 0.153.4 验证 Provider 主矩阵。

证据：all-agents-gui（本地证据：`experiments/recovery-analysis/2026-09-12/all-agents-gui`），含事件、安装版本与路径、Session Maps、文件内容、硬杀触发记录和旧版错误。独立新库地址见 fixture.txt；工作目录 `/tmp/dogcodes-all-gui`。不读取正式业务数据库。

## Claude Code 2.1.266

任务 `tsk_01M29XAE5V5DCCADJTX8Z64Z8W`，默认模型及权限。桌面与灵动岛均人工单次批准或拒绝，没有全权限放行。

- 桌面批准：claude-desktop-allow.txt 为 CLAUDE_DESKTOP_ALLOW；拒绝后 claude-desktop-deny.txt 不存在，无重试。
- 灵动岛允许一次：claude-island-allow.txt 为 CLAUDE_ISLAND_ALLOW，主窗口同步完成及 Diff；岛内拒绝后 claude-island-deny.txt 不存在，无重试。
- 正常退出重启：原生 UUID c88874ef-5017-4d7d-afff-f6860ecd6ceb 保持不变，无工具回答 CLAUDE-GUI-MAPLE-357。
- 待审批停止：审批卡消失，claude-cancel.txt 不存在；退出重启后没有旧审批或命令重放。新请求通过历史上下文包重建会话，UUID 11d95689-797e-46ba-a516-9f3e046ca182，正确无工具回忆标记。这一分支不是原生续接。
- 运行停止：START 已写且 END 未出现时点击停止；原生回传 interrupted / failed，轮次结束。起点验证后约 70 秒（超过 sleep 60）END 仍不存在，见 claude-running-cancel.json。
- 宿主硬杀：独立命令写 START 后、END 前精确 SIGKILL 本测试宿主，见 claude-hard-kill.json。重启后侧栏、正文、灵动岛均显示运行状态待确认，工具结果未确认，停止禁用，事件 turn.connection_lost；保留未知占用，未手工确认外部结束。
- **发现未通过项**：长 Bash 命令的原生后台 task_started / task_notification 被适配成 subagent，主窗口把终端操作改成“子智能体”。取消后卡片仍错误显示子智能体。证据 events.json 中 tool_DklgDODA9AmmhuHNhBBzRG31；应区分后台 Shell 与真实子 Agent，再复验。

当前运行中的验收实例沿用 fixture.txt 所指新库，独立 Vite 端口 1431。Codex、Claude 的硬杀任务保持未知状态作为证据；后续 Agent 使用独立新对话。

## Kimi 0.42.0

任务 `tsk_01M29Y29CA0V2A3RF7X0PHXD2B`，默认模型与权限，仅逐项批准一次，没有会话级放行。

- 桌面批准：kimi-desktop-allow.txt 内容 KIMI_DESKTOP_ALLOW；拒绝后 kimi-desktop-deny.txt 不存在，无重试。
- 灵动岛允许一次：kimi-island-allow.txt 内容 KIMI_ISLAND_ALLOW，主窗口同步终端完成和 Diff；岛内拒绝后 kimi-island-deny.txt 不存在，无重试。
- 正常退出重启：原生 session_3bd70657-c573-4cfa-ae8c-bcf2aa26868b 不变，无工具回忆 KIMI-GUI-PINE-468。
- 待审批点击停止：卡片消失，kimi-cancel.txt 不存在。重启后不恢复旧审批、不重放旧命令；下一条消息使用历史上下文包重建到 session_73728895-f725-4742-8618-904c26f680b6，仍正确回忆标记。此分支属于历史重建，不是原生续接。
- 运行中停止：确认 START 已写、END 尚未存在后点击停止，原生返回 Interrupted by user；超过 sleep 60 秒后 END 仍不存在，见 kimi-running-cancel.json。
- 硬杀：独立命令 START 出现、END 尚未出现时精确终止隔离宿主，见 kimi-hard-kill.json。重启后侧栏、正文和灵动岛均待确认，旧工具结果未确认，停止禁用，turn.connection_lost 落盘；未自动释放未知占用。
- **待完善项**：Kimi 原生审批请求只含截断的长命令摘要，但审批前的工具 activity 已含完整 JSON 参数。当前桌面必须额外展开终端卡才能核对完整命令；本次长命令均展开核对后才批准。审批卡标称“完整操作详情”却仍为截断摘要，需合并同工具已知输入，并检查灵动岛的完整内容门禁。短命令岛内批准/拒绝通过不能扩展为长命令详情通过。

以上命令文件及逐事件证据保存在同一 all-agents-gui 目录。当前 Kimi 硬杀任务也保留未知状态；接下来验收 Grok、Cursor、CodeBuddy，并修复复验发现的问题。

## 发现项修复进展（2026-09-12）

- Claude 后台任务：根据原始 tool_use 名称保留 Bash/普通工具类型；真实 Agent/Task 保持子 Agent 分类，后台终态仍路由到原轮次，关闭连接也不把 Shell 改为子 Agent。Driver 单测 20 通过、6 个真实测试默认忽略；GUI 复验进行中。
- ACP 审批参数：请求自身 rawInput 优先；缺失时只查同 session/tool 已收到的输入。Kimi 的 JSON replacement content 仅在完整可解析且含 command 时使用，不接受半截内容、不同会话/工具或已结束工具。ACP 单测 22 通过、14 个真实测试默认忽略；GUI 复验进行中。
- 修复版隔离桌面构建通过。日志见 claude-background-kind-tests.log、acp-approval-input-tests.log、fixes-desktop-build.log。自动测试不替代尚在进行的 GUI 复验。

Kimi 完整审批 GUI 复验通过：新的长命令在桌面和灵动岛均展示完整 command；岛内允许一次后 kimi-full-approval-details-recheck.txt 为 KIMI_FULL_APPROVAL_OK、kimi-full-approval-end.txt 为 KIMI_FULL_END，轮次正常完成。该结果覆盖上面的修复前截断问题。

Claude 后台 Shell 分类 GUI 复验通过：任务 tsk_01M29YXH88XJG4JKNT7VWWXQKP 显式请求原生 Bash run_in_background，单次批准后 task_started 仍映射为 tool/Bash，主窗口保持终端卡，没有 subagent 事件；最终 START/END 文件均正确。见 claude-background-recheck-events.json。后台 Bash 的“完成”来自原生启动回执，不等于后台进程已结束；此用例只证明分类修复，不作为后台执行状态生命周期的验收通过。

## Grok 1.0.30

安装 ~/.grok/downloads/grok-1.0.30-macos-aarch64，隔离任务（公开记录省略原 ID），默认模型/思考及权限，未持久放行。

- 桌面拒绝：首轮模型将 GROK_DESKTOP_ALLOW 拼成 GROK_DESKktop_ALLOW，核对后选择原生 reject-once，grok-desktop-allow.txt 当时不存在。模型没有自动重试；另发独立请求后命令拼写正确，批准一次，文件精确为 GROK_DESKTOP_ALLOW。
- 岛内批准：grok-island-allow.txt 精确为 GROK_ISLAND_ALLOW，桌面同步完成及 Diff。岛内选择本次拒绝 reject-once 后 grok-island-deny.txt 不存在，无重试。
- 正常退出重启：native ID 01a093f1-31d0-7132-9ff9-141b4772e83d 不变，无工具回忆 GROK-GUI-BIRCH-579。
- 待审批停止：卡片撤销，grok-cancel.txt 不存在；重启不出现旧审批、不重放旧指令，下一条消息通过历史包重建到 01a093f9-20bc-7553-8859-4375ccfd5410，回忆正确。该分支不是原生续接。
- 运行停止：START 已写且 END 未出现时停止，轮次结束；约 73 秒后 END 已出现，见 grok-running-cancel.json。因此仅取消请求/轮次终止通过，不认为外部 Shell 被停止。
- 硬杀：另一条前台命令 START 出现、END 未出现时精确 SIGKILL 隔离宿主，见 grok-hard-kill.json。重启后侧栏、正文、岛均待确认，工具结果未确认，停止禁用，turn.connection_lost 落盘，保留未知占用。
- **已修复复验**：Grok 同时提供 reject-once 和 reject-always，旧 UI 显示两个同名“拒绝”，岛内因为拒绝选项不唯一禁用。现在同角色多选项保留原生文案，两端可区分作用范围；岛仅在原生 ID 唯一、拒绝文案可区分、角色与详情完整时允许操作。未知/脱敏/歧义选项继续禁止岛内快速决策。真实岛内允许一次与本次拒绝都通过，未改变原生 optionId。
- 前端 195 项测试通过，类型检查通过；包含多种拒绝按钮提交原生 reject-once 的组件测试、重复 ID/相同文案拒绝门禁。日志 grok-island-tests.log、grok-island-typecheck.log。

当前隔离实例仍使用 1431 端口；下一步继续 Cursor、CodeBuddy，并做最终覆盖审计及必要补验。各宿主硬杀任务保留未知状态，不影响新会话测试。

## Cursor 2026.9.10-fd3934a

任务 tsk_01M2A0319C1GXD8MM9QDW6N6A2，原生会话 422b9f56-d103-41ba-a4af-8e7e637ba149，默认模型/权限，工作区 /private/tmp/dogcodes-all-gui。

- 桌面批准：cursor-desktop-allow.txt 精确为 CURSOR_DESKTOP_ALLOW，原生随后额外只读核对文件，未重复写入。
- 桌面拒绝：cursor-desktop-deny.txt 不存在，未重试；但原生发空 completed 工具更新，旧 UI 误显示“完成”。修复前证据 cursor-rejection-before-fix.json。
- **已修复**：审批响应缓存同 session/tool 的拒绝身份；收到空 completed 时将被拒绝工具保持 failed，防止协议调用关闭被误当成执行成功。其他会话或工具不受影响，ACP 23 项测试通过，构建通过。日志 cursor-rejection-tests.log、cursor-fix-build.log。
- 正常退出并加载修复版重启：原生 UUID 保持不变，无工具回忆 CURSOR-GUI-OAK-681。
- 岛内拒绝及修复复验：cursor-deny-recheck.txt 不存在，主窗口工具明确显示“失败 / Rejected by the operator in dogcodes”，没有重试。轮次正常结束与工具拒绝分别显示。
- 岛内单次批准：完整 command 可见，允许一次后 cursor-island-allow.txt 精确为 CURSOR_ISLAND_ALLOW；主窗口同步终端完成及 Diff。
- 待审批停止：cursor-cancel.txt 不存在，重启后无旧审批、无命令重放；新消息经 session.rebuild_context_prepared 历史包重建到 20e7800a-08a8-4c75-83ba-50ecf18352f5，正确无工具回忆 CURSOR-GUI-OAK-681。此分支不是原生续接。
- 运行中停止：START 已写、END 未出现时点击停止；89 秒后 END 仍不存在，见 cursor-running-cancel.json。UI 保守保留工具结果未确认，没有伪造执行成功。
- 硬杀：新历史会话 26fcf491-16fa-4bfa-be4e-b6ae457c89c3 的独立命令写 START 后、END 前，精确 SIGKILL 隔离宿主 PID 48167；重启侧栏、正文、灵动岛均待确认，工具结果未确认、停止禁用，turn.connection_lost 落盘。见 cursor-hard-kill.json；未确认外部结束或释放未知占用。

### Agent 菜单可访问性修复

原有 button 被赋予 role=option 后，原生 AX 仅暴露整组列表，没有各 Agent 操作项。改为有名称的 group + 原生 aria-pressed 选择按钮；原生 AX 已暴露九个安装项（八家 Provider，Codex 两份安装），实际通过该按钮选中了 Cursor。视觉布局未改变。ComposerFooter 既有测试、类型检查通过，见 agent-picker-tests.log、agent-picker-typecheck.log。

## CodeBuddy 2.150.0

任务 tsk_01M2A0ZSG08XX3PHD6KVY6ZKRE，原生会话 01a0940f-ea07-7c12-9c10-ece6fe89ad8c，默认模型/权限，隔离项目 /private/tmp/dogcodes-all-gui。

- 首个 CODEBUDDY_DESKTOP_ALLOW 标记文件命令被模型误判为伪造权限标记，没有工具请求；此轮不计审批通过。采用原生模型建议的普通 hello 文件后继续标准审批，无更改权限策略。
- 桌面批准：gui-approval-smoke-test.txt 精确为 hello，工具完成和 Diff 同步。
- 桌面拒绝：gui-smoke-two.txt 不存在，没有重试；工具保守显示结果未确认，未显示执行成功。
- 岛内批准：完整 command 可见，允许一次后 gui-smoke-three.txt 精确为 hello，桌面同步终端完成与文件变化。
- 岛内拒绝：gui-smoke-four.txt 不存在，没有重试，岛内已停止；主窗口工具未显示执行成功。
- 正常退出重启：原生 UUID 01a0940f-ea07-7c12-9c10-ece6fe89ad8c 保持不变，正确无工具回忆 CODEBUDDY-GUI-ELM-792。
- 待审批取消：gui-smoke-five.txt 不存在，原生工具 failed/cancelled，重启后岛显示已停止且无旧审批；新请求正确无工具回忆 CODEBUDDY-GUI-ELM-792。UUID 保持 01a0940f-ea07-7c12-9c10-ece6fe89ad8c，未产生历史包重建事件；此分支为原生续接。
- 运行中取消：START 已写且 END 未出现时停止，原生 failed/cancelled；约 72 秒后 END 仍不存在，见 codebuddy-running-cancel.json。
- 宿主硬杀：独立命令 START 出现、END 未出现时精确 SIGKILL 隔离宿主 PID 67824；turn.connection_lost 落盘。重启灵动岛、侧栏和正文均待确认，旧工具结果未确认、停止禁用；保留未知占用。见 codebuddy-hard-kill.json。
- 恢复检查期间鼠标定位一度失效（CUA windowNotFoundAtPosition）；键盘能展开岛。正常重启并重新绑定隔离应用后，实际从侧栏打开 CodeBuddy，完成正文核验。未以只读数据库结果替代 GUI 核验。

## 最终覆盖审计与验收边界

当前八家 Provider 的主要验收矩阵已完成检查：桌面和灵动岛批准/拒绝、正常退出重启的原生会话回忆、待审批取消与重启、运行取消，以及宿主断连后的恢复表现。ZCode 的原生硬杀和旧审批失效采用既有真实证据；新状态一致性 GUI 使用该隔离证据的恢复副本复验，不声称本批重新进行 ZCode 硬杀。Antigravity 与本批六家为独立真实 GUI 记录。

- 正常原生续接与取消后历史恢复分开统计。取消后 ZCode、Antigravity、Codex、Claude、Kimi、Grok、Cursor 使用历史包重建；CodeBuddy 本次保持原生 UUID。
- Antigravity、Codex、Grok 已启动 Shell 在运行取消后仍可继续；只确认取消请求和轮次状态，不认定 Shell 已停止，也不自动终止外部 Agent。Claude、Kimi、Cursor、CodeBuddy 本次 START/END 用例超过等待时长后 END 未出现，不扩展为所有工具均可停止的保证。
- 迟到/并发审批的精确调度、旧 request ID 重用和版本冲突由 Core/Store 确定性回归覆盖；代表性 ZCode GUI 检查旧审批失效。未在八家 GUI 分别人为延迟 IPC；不声称该分支全供应商 GUI 已穷举。
- Codex 旧安装 0.128.0 存在原生配置不兼容，本机该安装未通过；Provider 主矩阵基于 0.153.4。没有修改全局配置或计费档位。
- Claude 后台 Bash 分类修复通过；后台启动回执不等于执行结束，后台生命周期不纳入本次前台命令结果。
- 浅深主题全矩阵、发布打包版、Windows/Linux、其他启动故障、自动核实断连后外部执行与后续变更采集未覆盖。完成本次 Agent 验收不等于 M1 整体完成。

本批修复的回归证据：ACP 23 passed、14 ignored；Claude 20 passed、6 ignored；前端 195 项通过、类型检查通过；Core/Store 原生身份持久化与迟到更新专项通过。ignored 的真实 Driver 测试不计为本次 GUI 用例；测试数量不重复累加。原始日志均在证据目录。

收尾：已正常退出隔离桌面，精确停止本次 Vite PID 24356，核对 1431 端口释放；未触碰用户 1420 服务。隔离数据库、未知任务和证据保留，见 cleanup.json。

## Codex / Kimi 硬杀后 END 缺失追查（2026-09-12）

补查原生会话记录，而非仅根据宿主的未知状态判断。证据摘录：`experiments/recovery-analysis/2026-09-12/hard-kill-native-analysis/native-cancellation.json`。

- Codex：宿主 SIGKILL 时间 1789186405.820035；约 6 ms 后原生工具结果记录 `aborted by user after 25.0s`，约 21 ms 后记录 `turn_aborted / interrupted`。
- Kimi：宿主 SIGKILL 时间 1789187831.117663；约 6 ms 后原生记录 `turn.cancel / user_cancelled`，约 9 ms 后 Bash 返回 `Interrupted by user`，约 10 ms 后轮次 `cancelled`。
- 两条已获批命令均为写 START、sleep 30、写 END；Kimi timeout 为 60 秒。最终文件采集远晚于 30 秒，补查原工作目录仍无这两个 END，可排除仅因检查过早而漏看完成。

这证明当次原生轮次实际进入取消路径；结合文件结果，支持前台命令在 END 前被中断。当前通过 stdio 管道连接，时间关系强烈支持宿主死亡导致管道断连、原生端触发取消的解释；未取得这两个已安装原生版本的内部断连清理实现或进程退出跟踪，因此具体终止信号、Shell/sleep 的最终退出方式及 Agent 本体是否退出仍未证实。原生日志的 user_cancelled 是其取消分类，不代表用户点击了停止。

此结果不能扩展为任意后台命令、脱离进程组的任务或其他版本都会终止，也不改变宿主断连后保留未知状态的产品语义。此次为历史证据追查，未重新执行硬杀测试。
