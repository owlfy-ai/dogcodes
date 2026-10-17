# Fork 原请求、只读回执和跨窗口恢复

日期：2026-10-16。对应 DT-09；不代表独立审查或全部 Fork 中间状态已完成验收。

## 确认的问题

TurnForkAction 原来仅在组件 ref 中保存请求；关闭窗口或卸载组件后原 newTaskId 丢失，后续点击可能重新创建会话。切换环境也会生成另一条请求。后台只在最终 task.created/task.forked 事务完成后验证源轮次与环境，创建 Worktree/注册项目之后、最终会话事务之前没有持久原请求绑定。

## 改动

- 客户端调用前保存完整 taskId/turnId/newTaskId/environment。待核实期间不新建另一目标或更换环境；窗口重开只展示恢复入口，不自动重发。
- 新增 Core fork_receipt、daemon RPC 和有源会话窗口范围校验的 core_fork_receipt。根据持久 command_ack 的事件范围读取原创建事实，核对来源/轮次/环境/目标。查询不创建会话或 Worktree，也不靠 bootstrap 中名称相同推断成功。
- 后台在第一次外部创建或会话创建之前，以 `fork:<newTaskId>:intent` 写入 task.fork_requested，持久绑定完整原请求。后续即使未完成最终事务也先核对该意图；复用标识改来源、轮次或环境会拒绝。旧版本已完成的 Fork 仍可从最终事件查询；旧版本只有中间项目而无原请求时明确拒绝猜测恢复。
- 回执区分 completed 与 taskAvailable。只有意图但没有最终创建事实时返回 completed=false，不能显示创建成功。null 也不代表磁盘上没有部分 Worktree。已有成功事实但目标不可用时不会重新创建。
- 成功结果先交给界面打开，打开失败或原视图已关闭时保留本地跟踪。之后核实原回执可打开同一会话。结束跟踪只移除本机记录，不取消后台执行或删除 Worktree。迟到的旧操作不会删除另一个目标的待核实记录。
- 恢复按钮加入菜单键盘导航；原创建选项被禁用时焦点进入可用的核实入口。

## 验证

本工作块集中验证，没有打包、全量构建或真实模型调用：

- `cargo test -p agent-store --test task_fork`：6 项通过。包含真实 Git Worktree 的轮末文件恢复、源文件/分支不被修改、完整历史继承、重复请求、SQLite 文件重新打开后只读查询/重放不追加事件，以及未完成请求在后台重建后不能改环境复用。
- `cargo test -p desktop --lib fork_receipt`：1 项实际 loopback RPC 测试通过，验证完整请求、源会话隔离、查不到结果与重复创建不增加事件。
- `TurnForkAction.test.tsx` 11 项与 `forkOperations.test.ts` 6 项通过，共 17 项。覆盖关闭重开、回包丢失、创建成功但导航失败、视图关闭后迟到响应、本地存储失败、回执冲突、目标不可用及未完成操作的显式同标识重试。后续调整恢复按钮的键盘角色后仅复验 11 项组件用例，全部通过。
- 桌面类型检查与 diff 空白检查通过。运行日志在 `/tmp/dogcodes-fork-recovery-store.log`、`/tmp/dogcodes-fork-recovery-rpc.log`、`/tmp/dogcodes-fork-recovery-frontend.log`、`/tmp/dogcodes-fork-recovery-frontend-final.log`、`/tmp/dogcodes-fork-recovery-types.log`，进程均已收取退出状态。

## 剩余验收与下一工作块

回执 completed=false 包含正在执行、失败或中断的未完成状态，目前不区分这几者。完整阶段进度、持久错误及多仓库部分创建的恢复/人工处置仍需补齐。现有 Worktree 恢复只复用完成标记匹配的目录，不重置已存在目录中的用户文件；尚无完成标记的半成品会保留并报错。本次没有用成功的单仓库测试覆盖进程恰好在各 Git 步骤之间退出的情况。

原请求已持久化，但完整恢复计划（冻结源项目元数据、历史边界、快照引用、各仓库阶段和实际结果）尚未统一存储；服务重启仍依赖原历史和快照可用。后续须核对源项目变化、源会话永久删除、中间项目修改、目录/分支被移动、损坏恢复标记及多仓库失败，不自动清除半成品。

独立项目审查/逐轮审查/修复后复审及开始修复按钮仍存在仅用组件 ref 保存待确认请求的入口，下一步按相同原则逐一收口，不直接复制 Fork 的结果语义。问答订阅、共享 Agent 准入和三平台实机验收继续在总清单维护。当前注册 110 项 handler，其中 108 项为生产入口。
