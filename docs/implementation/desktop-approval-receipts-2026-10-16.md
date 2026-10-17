# 审批来源绑定与只读投递回执

日期：2026-10-16。对应 DT-09。本工作块交付审批回复的来源校验、后台查询协议和客户端适配；后续已接入[跨窗口恢复界面与显式作废](desktop-approval-recovery-2026-10-16.md)，其中包含本记录之后的验证。

## 实现

`RespondToApproval` 必须带原 `turnId` 和 `requestedAtSequence`。Core 在准入时同时核对当前待决请求的 ID、轮次和请求事件序号，再使用原版本条件进行事务写入。UI 从用户正在操作的审批卡片取来源，不能用新任务快照替换原审批来源。供应商在新轮次或同轮次复用审批 ID 时，旧按钮即使取得最新 task.streamVersion 也不能回答新请求。成功后仅移除同一来源的卡片，保留可能刚到达的同 ID 新请求。原生样例、运行中完全访问及验收脚本同步传递来源；旧调用方缺字段会被拒绝，不猜测来源。

审批已受理后，投递仍核对活动轮次及执行所有权，并检查受理来源之后已记录的同 ID 新请求；已发现替换时记录失败，不投递旧决定。此检查不是对供应商内部异步状态的原子锁。最终驱动调用仍依赖各驱动自身的会话/交互归属约束，后续实机验收需覆盖取消和驱动退出的竞态。

增加 `approval.delivery_sent`；与 `approval.delivery_failed` 一起记录原受理 commandId、taskId、turnId、approvalId、requestedAtSequence。成功表示驱动调用已返回成功，不表示获批操作或任务已完成；驱动错误不把供应商正文写入新回执事件。存储查询或投递记录写入失败时保留未知，不伪造成功，也不自动重新投递。

增加 Core/daemon/Tauri 接口：

- `prepare_approval_response` / `core_prepare_approval_response`：对原类型化命令计算既有规范指纹，返回无答案/无选项的查询信息。校验 schema、本机调用者、原版本及来源，不准入、持久化或执行；本机 RPC 与 Tauri 统一调用者身份。
- `approval_receipt` / `core_approval_receipt`：只读已有 Ack，核对指纹、原受理事件及来源，再读取固定事件边界内的投递记录。返回受理 Ack 和 `unknown` / `sent` / `failed`。找不到回执返回 null；指纹、来源或投递记录冲突明确报错，绝不调用 execute 或驱动。

TypeScript 的准备与发送复用同一载荷构造器，保留原 task 版本、来源、答案与 operationId；回执查询只传查询信息。回答正文仍按既有产品规则保存在原规范事件中，本次未新增答案副本，也未把答案写入 localStorage。指纹用于一致性核实，不是加密或访问授权令牌。没有数据库迁移、清库或新增签名/公证凭据。

## 验证

- `cargo test -p agent-core --lib approval`：5 项通过，包含受理后轮次替换、正在停止、失去执行所有权及同轮次请求替换检查。
- `cargo test -p agent-store --lib approval`：4 项通过，包含原轮次/同轮次复用 ID、真实 fixture 驱动只投递一次、自主运行审批归属及过期处理。
- `approval_receipts` 集成测试 3 项、`running_permissions` 5 项、`multi_root_history` 3 项、`questions_persist` 1 项通过，覆盖 SQLite 重开后只读查询、投递未知/成功/失败、指纹/来源冲突、不返回答案、运行中权限和取消恢复。
- `cargo test -p desktop --lib approval_receipt`：1 项 loopback RPC 测试通过；准备时身份归一化、拒绝其他命令、查询不产生事件和回执。
- `commandReceiptClient` 5 项、`AppStreaming` 9 项、`ApprovalComposer` 3 项，共 17 项不同前端用例通过；最终 `pnpm typecheck` 通过。首次类型检查暴露既有 PendingApproval.turnId 使用 string，已收紧为 TurnId；首次 Store 测试编译暴露一个恢复测试缺 TurnId 导入，已修正并复验。

`cargo check -p agent-store --example live_multi_root_history --example live_shared_project` 通过，日志 `/tmp/dogcodes-approval-source-examples.log`。所有本轮验证进程已收取终态，最终验证链 exit 0；变更文件空白检查通过。

累计 22 项不同原生测试通过。没有运行全套测试、整套构建、打包或真实 Agent 调用。日志为 `/tmp/dogcodes-approval-source-{core,store-final,integration,question}.log`、`/tmp/dogcodes-approval-receipts-{rpc,frontend-final,types-final}.log`。

## 后续

下述持久恢复 UI 已由[后续工作块](desktop-approval-recovery-2026-10-16.md)交付，保留这里的设计约束：发送前保存固定操作身份、原来源/版本和无答案查询信息；重开只查询，绝不自动重新批准。受理已确认时不得再次投递。无回执仅表示当前未找到，不能用新 ID/新版本重发。敏感或自由文本回答保留在当前界面；重开后如需显式继续，重新输入并按原完整命令指纹核对，不能把不同答案冒充原操作。准备接口存在额外往返，需处理期间版本变化，不能自动用最新版本覆盖原意图。

随后继续普通主会话发送/创建、取消实际投递和退出、Fork 中间阶段、其他旁路 API、共享业务状态及三平台候选验收。DT-08/09/10 仍是桌面首发前置条件。
