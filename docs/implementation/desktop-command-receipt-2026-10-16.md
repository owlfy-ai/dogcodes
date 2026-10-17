# CoreCommand 只读受理回执

日期：2026-10-16。对应 DT-09，延续[命令身份绑定](desktop-command-identity-2026-10-16.md)。这里只交付受理查询基础及客户端适配；“开始修复”的完整跨窗口恢复界面尚未完成。

## 实现与边界

新增 `Core::command_receipt`，经 daemon `command_receipt` 和 Tauri `core_command_receipt` 暴露。查询必须携带原 envelope：使用原 idempotencyKey 查找已有 Ack，校验 schema、非空标识和完整命令指纹（含原 actor、版本条件及载荷）。请求不匹配或旧回执无指纹时明确报错，不猜测成功。

查询只读已有持久回执，不走 execute、命令当前状态准入、驱动选择或删除清理，也不为已删除会话生成新的别名回执。即便当前会话版本已经变化，原请求仍可查原受理结果。返回 null 只表示未找到匹配记录，不证明请求已经失败、不再在途或可用新 ID 重发。返回 Ack 仅证明命令已受理，后台任务完成、取消、失败和清理结果仍须查对应业务状态。

Tauri 与 daemon 保持和执行入口相同的本机调用身份归一化及内部命令限制。后台内部 RecordTestResult、RecordInstallationProbe、ReconcileMissingInstallation 不开放给 UI 查询。当前注册 handler 为 111 项，其中生产入口 109 项。

TypeScript 增加 `CommandReceipt` 及可选 `reviewFixReceipt`、`contextPackageReceipt`；上下文确认允许传入固定 operationId。Tauri 客户端发送与查询复用同一命令载荷和 envelope 构造函数，保留原版本、问题索引、上下文范围、目标 Agent 和包哈希，避免查询与发送的载荷漂移。调用查询不会先 bootstrap、执行或自动重发。

未改变数据库结构，没有迁移、清库或增加原提示词/凭据存储。签名及公证配置未改动。

## 验证

- `cargo test -p agent-store --test command_identity --test task_lifecycle`：6 项命令身份/回执测试、4 项会话删除/清理测试通过。覆盖 SQLite 重开、当前版本变化后仍可查原回执、载荷/actor/版本不匹配、旧数据与非法参数、查询不触发清理、不生成删除别名。
- `cargo test -p desktop --lib command_identity` 及 `command_receipt`：2 项 loopback RPC 测试通过，包含原回执、未知请求、actor 归一化和全部三类内部命令拒绝。
- 新增客户端测试 3 项通过，既有自动上下文 4 项及共享设置客户端 2 项通过。
- 首次前端命令使用 `test -- 文件名`，参数未按预期过滤，意外执行全套：877 项通过、2 项失败。两项失败来自已有 `reviewWatch` 的 Channel 同步初始化错误未捕获，分别表现为测试 mock 缺 Channel 和无 Tauri 桥接时构造失败。现将同步初始化异常纳入既有有限退避重连，避免抛出 React effect、阻断界面，并新增恢复与取消测试。
- 只复验 `reviewWatch.test.ts`、`ConversationRendering.test.tsx`、`DraftConversation.test.tsx`，31 项通过；未再运行全套。`pnpm typecheck` 通过。首次 RPC 测试编译缺少测试模块的 CoreRepository trait 导入，补齐后上述 2 项通过，未重跑已通过的 Store 测试。
- 原生累计 12 项通过。两个最终验证进程均收取 exit 0。格式与 diff 空白检查通过；无打包、发布或真实 Agent 调用。

日志：`/tmp/dogcodes-command-receipt-store.log`、`/tmp/dogcodes-command-receipt-rpc-identity-final.log`、`/tmp/dogcodes-command-receipt-rpc-final.log`、`/tmp/dogcodes-command-receipt-frontend.log`、`/tmp/dogcodes-command-receipt-frontend-final.log`、`/tmp/dogcodes-command-receipt-types.log`。前端全套最初失败记录保留，不能将本轮描述为修复后全套通过。

## 下一项

`ReviewRepairControls` 仍用组件内 ref 保存操作 ID，重开丢失，重试还可能读取新的 task.streamVersion。接下来将修复原意图、主 Agent 的自动上下文确认、最终 SubmitTurn 分阶段持久保存；查询使用各阶段原 ID/载荷/版本，界面显式核实和继续，不在挂载时自动执行。需要核实上下文确认后到最终提交之间的版本边界，不能把中间步骤已受理或当前 Agent 恰好相同视作最终修复已提交。

审批、其他命令变体、Fork 多阶段恢复、共享状态及三平台实机候选验收仍按原矩阵推进；此工作块不代表 DT-09 或 GitHub 发布门槛已全部完成。
