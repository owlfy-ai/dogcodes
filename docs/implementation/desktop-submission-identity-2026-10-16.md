# 普通发送与新建会话：固定身份、回执及安全作废基础

日期：2026-10-16。对应 DT-09。本记录描述发送恢复的协议、客户端适配和后台作废基础。后续 App.submit 的持久分阶段流程及恢复界面已经接通，当前证据与剩余范围见[发送恢复记录](desktop-submission-recovery-2026-10-16.md)。

## 固定请求

`createTask` 允许显式传入 `TaskCreationIdentity {taskId, operationId}`，以原 taskId、项目、标题、目标 Agent 和版本零执行。省略身份的既有调用仍创建新身份；App 主发送入口现已通过持久流程传入固定身份。新增 `createTaskReceipt` 只读核实原创建；查询返回受理不代表该会话现在仍存在，后续流程必须单独确认资源可用，不能重建已被删除的会话。

`submitTurn` 允许传入固定 operationId，新增 `TurnSubmission` 和 `submitTurnReceipt`。发送、查询与作废复用同一个载荷构造器，完整包含原 taskId、streamVersion、正文、模型、思考强度、权限模式及附件引用。Executor 显式使用原 TaskSummary.preferredExecutor，避免把用户选定的目标留给后续当前状态解释；既有自动上下文流程在提交前返回目标执行者的实际快照。null 仍表示既有默认/自动选择语义，不能把它宣称为已固定供应商内部默认模型。

## 显式作废未受理的阶段

增加 `Core::invalidate_pending_command`，通过 daemon 和 Tauri `core_invalidate_pending_command` 提供。只支持 CreateTask、SubmitTurn、ConfirmContextPackage；必须是本机用户，且带原版本条件；创建只能使用版本零。其他命令拒绝，不开放通用执行入口。桌面 RPC 固定本机调用者身份。

先使用原完整 envelope 查询受理。已有受理返回原 Ack，绝不取消已经执行的操作。未受理时，对后台观察到的当前任务版本写入 `task.command_invalidated`，使此前快照中的原请求无法再通过版本条件。尚未创建的 taskId 只留下版本零失效的流记录，不创建任务投影；迟到的原 CreateTask 会被拒绝，不能复活为一条空会话。

作废使用从原 idempotencyKey 派生的稳定标识，绑定完整原命令指纹。作废回复丢失后重试读取已有作废回执，不再次提高任务版本，不妨碍后续新意图。原命令与作废竞争时，原命令若先受理就返回其 Ack；其他并发变更产生冲突时要求核实，不猜测成功。元数据记录没有提示词、标题、上下文正文或附件内容，不调用驱动、不生成上下文、不启动任务，也不删除已创建会话。任务流版本变化可能使其他并发写入产生版本冲突，各入口须使用各自的原意图恢复规则。

客户端分别提供 `invalidateTaskCreation`、`invalidateTurnSubmission` 和 `invalidateContextPackage`，都要求原身份及载荷。查询接口不调用作废；恢复 UI 仅在用户明确放弃原发送流程时调用，不在挂载、超时或断连后自动作废。

没有数据库迁移或清库；未修改签名/公证凭据配置。注册清单现为 115 项（113 项生产入口），CoreCommand 仍为原 18 个变体。

## 验证

- `cargo test -p agent-store --test command_invalidation --test command_identity`：12 项通过。覆盖未创建资源的流屏障、数据库重开、原创建与作废竞争、原创建受理后被删除、发送不被作废接口启动、上下文不被该接口切换、作废重试不影响后续版本、载荷变化拒绝，以及已有指纹/回执回归。
- `cargo test -p desktop --lib pending_command_invalidation`：1 项 loopback RPC 测试通过，覆盖本机身份归一化、非允许命令拒绝、只读查询无写入、重复作废幂等、迟到创建拒绝和参数变化拒绝。
- `commandReceiptClient.test.ts` 9 项、`automaticContext.test.ts` 4 项，共 13 项前端用例通过。发送/查询/作废的实际 envelope 一致，固定 taskId、原版本及所有发送参数；附件不拼入正文，上下文失败不发送。
- `pnpm typecheck`、新文件格式和变更空白检查通过。原生累计 13 项；相关验证进程均已收取 exit 0。没有运行全套测试、整套构建、安装包或真实 Agent。

日志：`/tmp/dogcodes-submission-identity-{native,rpc,frontend,types}.log`。

## 持久发送流程与界面：后续已接入

以下为实现约束，落实与验收详见[发送恢复记录](desktop-submission-recovery-2026-10-16.md)。需要直接替换 App.submit 的“prepare attachments → createTask → submitWithAutomaticContext”调用链。不能只给 submitTurn 填一个 ref 中的 ID，或在失败后把正文放回输入框就视为恢复完成。

1. 在第一次业务变更前保存原消息、目标 Agent、模型、思考强度、权限、已准备的附件引用，以及各阶段固定身份。现有会话绑定原 taskId；新草稿生成稳定 taskId，并区分草稿身份与真正的会话身份。不同窗口使用独立恢复记录，不覆盖彼此；普通未发送草稿仍各端独立。
2. 新建会话的受理需要核对原 task.created 事件；上下文阶段需保存原 request/包 hash/源版本，并通过原回执的最后事件、causationId、目标 Agent 和 streamVersion 建立下一阶段边界。最终 SubmitTurn 的原快照必须成功持久化后才发送。不能读取任意最新快照来替代该边界。
3. 重开和挂载只核实回执，用户明确继续才发送原请求。已经受理的最终提交不能再发；上下文受理与消息提交是不同阶段，不能混淆。
4. 放弃流程必须覆盖正在途中的阶段。创建/上下文已受理时，也要核实或作废其确定边界上的原最终 SubmitTurn，不能仅删除本地 checkpoint，让另一个窗口迟到的继续操作仍提交消息。保留已经创建的空会话或已经确认的上下文，并明确说明这些既有结果不被撤销。
5. 输入恢复需处理发送途中切换会话、较晚的新草稿及模型选择，避免旧失败回调覆盖新输入；成功清理附件也不能清掉用户后来添加的附件。乐观消息与规范历史的对齐应使用原命令/轮次身份，不能仅比较相同正文。
6. Demo/测试客户端需要对应的内存回执与固定身份行为，避免用无条件成功的 mock 掩盖恢复状态机。附件上传本身的固定身份、完整性与文件回收仍属独立核对项，不把“保存了路径”说成这些问题已经解决。

之后继续其他旁路 API、取消实际退出、Fork 中间阶段、共享状态及三平台候选验收；本工作块不是 GitHub 发布完成。
