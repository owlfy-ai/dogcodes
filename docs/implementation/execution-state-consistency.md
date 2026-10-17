# 执行连接中断与直接继续（2026-09-12）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

当前产品不再要求用户判断外部进程是否结束。桌面退出由独立 Core 服务承接执行；真正服务异常恢复时，旧本地轮次自动关闭并解除占用，用户可以直接发送下一条消息。

- `turn.connection_lost` 记录外部结果 unknown，匹配 task/turn/run 后将 Session 标为 Detached、清除活动 Run，并写入“连接已中断，可以直接发送消息继续”的会话提示；不生成 turn.completed 或成功结果。
- Companion 的最近结束状态使用 disconnected（显示“连接中断”），不作为活动任务持续占用。侧栏不再一直显示运行中；恢复页的人工核实复选框及确认接口已删除。
- 待审批请求在服务异常恢复时失效，旧请求和迟到事件不能影响后续轮次。投影重建保持该行为；已经处于 disconnected 的遗留活动轮次也在启动恢复时释放，不增加数据库迁移或升级分支。
- 用户继续时按现有逻辑建立可用的执行会话并附带历史。在最近轮次连接中断的替代会话中附加检查提示：先核实当前文件/执行结果，不假设旧命令成功或停止，不盲目重放有副作用的操作。新用户消息保持原文。
- 解除占用仅结束应用内旧轮次，不保证残留 Shell 已停止，也不等于自动核实所有供应商的外部执行。后续检查依赖原生 Agent 能力；不增加自动杀进程、自动重发或自动批准。

验证：单目录与多目录真实测试子进程无析构退出后，自动释放、旧审批拒绝、重复恢复幂等、投影重建保持一致；直接提交“继续”成功，Driver 收到原文与检查提示，旧轮次没有被标成成功，迟到旧事件不会覆盖新轮次结果。详细日志见 `experiments/recovery-analysis/2026-09-12/continue-without-confirmation`。

## 本次真实 GUI 验收

隔离 Kimi 任务待审批时，精确 SIGKILL Core 服务 PID 87790 和测试 UI；重新打开后新服务 PID 89600 自动释放旧 Run、失效旧审批。灵动岛显示“连接中断”，主窗口显示可直接继续的提示，没有人工确认门槛。实际从主窗口发送继续消息，原生回复 `RECOVERY_CONTINUE_OK`，旧 `old-pending.txt` 始终不存在，旧轮次没有被标为 completed，总共仅两条用户轮次。替代会话包含检查副作用前核实当前状态的提示。

自动验证：Core 26、Desktop 31、Store 37，共 94 项 Rust 测试通过；前端恢复/灵动岛/侧栏 19 项通过，前端类型检查与构建、workspace all-targets、默认桌面构建通过。隔离 UI 与服务已退出，未修改用户数据库或占用 1420。

证据：gui-results.json（本地证据：`experiments/recovery-analysis/2026-09-12/continue-without-confirmation/gui-results.json`）、同目录事件、时间线和测试日志。GUI 代表用例为 Kimi。

## 其余七家真实 Driver 补充验收

2026-09-12：Codex、Claude Code、ZCode、Antigravity、Grok、Cursor、CodeBuddy 全部通过。每家独立数据目录与工作区，通过桌面生产 IPC 调用真实 Driver：请求写入 `pending.txt`，保持待审批，SIGKILL 隔离 Core 服务，重启后直接发送继续消息。

七家均验证旧 Run 自动释放、旧审批失效、新消息直接受理且原生回复 `RECOVERY_CONTINUE_OK`，只有新轮次产生 completed；旧文件始终不存在，总共两次 turn.queued。所有测试服务已退出，未使用用户数据库或 1420。权限模式读取各 Agent 的默认策略，不自动批准。

本轮是七家真实 IPC/Driver 验收，不是七家 GUI 重验；与前述 Kimi GUI 一起覆盖八家断连后直接继续。此用例杀的是 Core 服务且旧命令尚未获批，不据此宣称八家已执行中的 Shell 在桌面退出后均完成。

ZCode 首次脚本固定传入 `default` 导致权限策略不支持，尚未进入硬杀分支；改为读取原生默认值 `build` 后完整通过，无产品代码修改。

证据：results.json（本地证据：`experiments/recovery-analysis/2026-09-12/other-agents-continue/results.json`）、audit.json（本地证据：`experiments/recovery-analysis/2026-09-12/other-agents-continue/audit.json`）、同目录逐家 pending/events/timeline 与可重复运行脚本。

---

以下为修订前历史记录，人工确认流程已被上述方案取代。

# 宿主与原生执行状态一致性（2026-09-11）

> 2026-09-12：默认产品已改为独立 Core 服务。以下为原内嵌宿主断连的历史实现；桌面 UI 退出不再触发该分支，真正的服务异常仍需保守恢复。详见 [Core 服务](core-daemon.md)。

宿主退出或连接丢失不等于 Coding Agent / shell 已停止。外部工具继续执行是可能发生的正常生命周期行为，不能仅凭宿主退出就把任务判为失败、停止或成功。本次不新增终止外部进程的行为。

## 当前实现

- 启动恢复为遗留 Run 记录 `turn.connection_lost`，保留 `active_primary_run`，状态使用已有 `disconnected`，表示原生执行状态未知。旧审批失效，恢复不会自动发送 Prompt。已记录断连的 Run 再次启动不重复采集或写事件。
- 未确认 Run 保留会话占用，Core 拒绝新轮次；旧审批仍不能投递。桌面显示“连接中断，原 Agent 或工具可能仍在执行”，停止按钮禁用；侧栏和灵动岛统一显示“运行状态待确认”。状态不会因完成提示超时而消失。
- 工具缺少原生终态时仅显示“结果未确认”，不再断言“已结束”。恢复文件卡标注为检查时的恢复快照，外部进程后续写入不冒充已包含。
- 恢复页增加独立的“我已在原 Agent 中确认任务和工具均已结束”复选框，勾选后才能“确认结束，解除会话占用”。
- 新接口 `acknowledge_external_run_ended` 仅主窗口 / LocalUser 可调用，要求未归属本实例且 disconnected 的 Run，并以流版本做原子冲突检查。只记录 `turn.external_end_acknowledged`（result=unknown），释放本应用占用并保持 Session Detached；不调用 Driver stop/shutdown、不中断外部进程、不提交或重放 Prompt，也不伪造执行结果。后续会话重建沿用原流程。
- 数据库仅初始化空库，不执行版本升级或旧事件兼容；断连使用当前 `turn.connection_lost` 事件。显式投影重建保留，前一轮终态不会冒充当前轮原生终态。

## 修订前验证记录（迁移相关测试现已移除）

- Store 37 项通过：真实测试子进程无析构退出，单/双目录文件证据保留，断连状态跨投影重建持久化，重复恢复幂等；旧审批与新提交被拒绝；系统身份/过期版本/重复确认拒绝，用户确认解除占用不产生终态或重放。
- 历史 v9 迁移回归通过（现已移除该兼容逻辑及测试）：legacy host_interrupted 变为 disconnected 投影，Canonical payload 原样保留，灵动岛无伪终态。
- Core 22 项通过；前端 190 项通过（含确认勾选门槛、不调用取消或发送）；typecheck、隔离桌面构建通过。
- 原生 GUI 使用历史硬杀证据的独立 v9 数据库副本，截取到故障发生时，未修改正式用户数据库。主窗口、侧栏、灵动岛显示待确认；再次重启仍保留。恢复页未勾选时确认按钮禁用；点击后仅新增一次确认事件，turn.queued 仍为 11，灵动岛恢复待命。此 GUI 检查验证升级和交互，不声称本轮再次启动真实模型进行硬杀。

证据：state-consistency（本地证据：`experiments/recovery-analysis/2026-09-11/state-consistency`），包含最终测试/构建日志及 gui-confirmation.json。

## 边界

目前不能自动核验所有供应商原生任务是否仍在执行，使用明确的用户核实解除流程。用户确认是声明，不是原生完成证据。后续增强为供应商只读状态核验、重新订阅及后续变更采集；不能将“刷新检查”描述为已查询原生状态。未统一修改正常退出时各 Driver 的现有生命周期策略。
