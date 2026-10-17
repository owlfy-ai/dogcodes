# M1-S7 恢复与诊断

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

当前断连语义（数据库已取消升级迁移） 见 [状态一致性](execution-state-consistency.md)。后文旧版将宿主中断记录为 turn.failed 的行为已由此取代。

## 原生 GUI 最新验收（2026-09-11）

ZCode 代表性 GUI 路径及通用故障诊断导出已完成，详见 [GUI 验收报告](recovery-gui-acceptance.md)。正常重启复用原生会话；待审批取消后重启走新会话历史恢复，两者分别记录。七家 Driver 通过不代表七家 GUI 通过。

## ZCode 冷恢复修复（2026-09-11，覆盖下方旧失败记录）

恢复原生会话后、订阅和发送新消息前，读取对应 workspace 的当前模型定义，复用 ZCode 本地配置中同一 provider/model 的连接配置，通过原生 `session/updateRuntimeModelConfig` 恢复运行配置。保留原模型和思考强度；显式的新选择在恢复配置阶段生效。不自动选择其他 provider/model。

凭证沿用现有本地配置解码，只在子进程管道和原生 JSON-RPC 内存中传递；不落入 Canonical Events、Artifact 或诊断日志，不更改凭证或全局配置。缺失/禁用模型或不可解析配置时明确失败并关闭本次恢复的原生运行实例。`ZCODE_RUNTIME_MODEL_UNAVAILABLE` 现在映射为固定的配置错误，不再完全丢失原因。

正常结束后的跨进程续接与历史回忆已真实通过。Driver 单测 12 项通过（9 项默认忽略）；运行配置构造测试 3 项通过；桌面 cargo check 通过。双目录中断后续接、历史标记回忆和待审批取消均真实通过，目标文件未写。桌面仅将已验证 CLI 0.16.5 的 resume 能力提升为 Supported，未知版本维持 Unknown。完整证据见 zcode-resume-fixed.log（本地证据：`experiments/recovery-analysis/2026-09-11/zcode-resume-fixed.log`）。该 Driver 修复批次不含 GUI；后续 GUI 结果见上方报告。


更新：2026-09-11。状态：取消、会话重建及启动故障诊断导出已实现；Antigravity 待审批取消最新复验通过；ZCode 冷恢复本轮已修复并通过；ZCode 代表性 GUI 及通用诊断导出已验收，其余六家 GUI 与异常边界待补齐。

## 已实现

- Store 在打开既有数据库时先使用 READ_ONLY 连接执行 `quick_check(1)` 和版本检查，
  再在用于迁移的连接上复检，通过后才切 WAL/迁移。检查失败不尝试重建或删除数据库。
  检查有 SQLite VM progress handler 5 秒预算（每 1,000 条指令检查）及 5 秒锁等待上限；
  不是 OS 阻塞 I/O 的硬实时超时。检查完成/失败均移除 hook，避免影响后续 SQL。
- Tauri 存储准备失败仍保留窗口，但不注册 DesktopState/Core、不探测 Agent、不创建会话。
  完整性检查失败时不会创建 scratch；权限、较新数据库版本及存储准备错误同样关闭业务入口。
- `core_startup_status` 限主窗口，只返回固定状态，不暴露底层 SQL 或路径。
  StartupGate 在 ready 后才延迟加载 App；恢复模式无 Composer、Quota 轮询或业务订阅。
  页面说明先关闭应用、备份数据库与 WAL/SHM、核对版本/权限后重启。

- Protocol RecoverySnapshot/RecoveryItem，Core 只读检查安装就绪、Driver 注册、未知归属
  Run、停止中 Run、待审批和不可恢复 Session。最多分析前 100 个任务和安装，并标记截断。
  当前通过 bootstrap 读取投影，尚不是数据库层有界分页扫描。
- Core 记录本实例调度的 Run，RAII 在任务结束/取消/异常展开时移除归属。
  CancelTurn 必须同时满足 task/turn/run、stream version 和本实例归属。
- React 侧栏“恢复与诊断”，手动刷新、返回会话处理、停止本实例待审批 Run。
  状态变化时拒绝过期停止操作；不提供重发或清理按钮。
- Tauri `core_recovery_snapshot` 和 `core_export_diagnostics` 限主窗口。
  导出由服务端重新生成，通过原生保存对话框选择目标；复用私有临时文件、fsync、
  persist_noclobber，不能覆盖已有文件。用户取消不写入。
- 摘要仅含固定状态码、白名单 Provider、内部 task/run/turn ID、版本和时间。
  不导出对话、文件路径、工具参数、原始报错、原生会话 ID 或凭据。

## 验证

- Store 停止测试扩展：第二个 Core 读取同一数据库识别未知归属 Run；诊断零事件写入，
  序列化无项目路径/标题；第二实例停止被拒绝且不调用 Driver interrupt；原实例正常停止。
- 新增 4 项 Store 测试：损坏文件/新版本拒绝且原字节不变、已提交 WAL 可读、超时失败并移除 hook。
- 新增 2 项 Desktop 测试：损坏数据库不创建 scratch、首次启动正常建库；2 项前端恢复说明测试。
- 前端 typecheck、35 项测试和 production build 通过。
- `cargo test --workspace` 通过；`cargo clippy --workspace --all-targets -- -D warnings`
  和 `cargo fmt --all` 通过。未运行消耗模型额度的 ignored 用例。
- 上述早期自动化不等于 GUI 验收；后续已通过正常/损坏库诊断 Save/Cancel，范围见 GUI 报告。

## 本次新增实现及验收

- 宿主恢复在终结遗留轮次前记录 `approval.expired`。取消请求暂停该轮审批，`turn.cancel_failed` 恢复未处理审批，终态永久失效；审批回复的异步投递复核接受时的 turn ID 与本实例 Run 归属。
- `prepare_session_rebuild` 仅接受本地用户、空闲任务及预期流版本；记录 `session.rebuild_requested`，不调用 Agent。下一条用户消息建立替代会话，保存 `session.rebuild_context_prepared` Artifact 并附带历史。旧 Prompt 只作为历史资料，不重新提交。
- 重建包读取最近 10,000 条源事件，标明截断，不包含当前轮 Prompt，限制 1 MiB；文件证据通过当前轮只读历史查询访问。该包不是无限历史恢复，也不证明供应商已经消费每条事实。
- Store v9 重建会话状态和 Artifact 派生投影；原始事件保留。
- 原生执行前重新验证全部目录，包括已有 Review 基线的目录；缺失目录不会静默省略。
- 启动失败固定分类：数据目录、宿主锁、数据库检查、临时目录、中断历史恢复。摘要仅含阶段、状态码、应用版本和时间；故障导出不依赖 Core/数据库，不修库、不覆盖文件。

| Agent | 双目录运行取消 | 新驱动续接并回忆历史标记 | 待审批取消、目标文件未写 |
| --- | --- | --- | --- |
| Codex | 通过 | 通过 | 通过 |
| Claude Code | 通过 | 通过 | 通过 |
| Kimi | 通过 | 通过 | 通过 |
| Grok | 通过 | 通过 | 通过 |
| Cursor | 通过 | 通过 | 通过 |
| Antigravity | 通过 | 通过（使用 SessionReady 持久会话 ID） | 通过；收到审批后取消，文件未写 |
| ZCode 0.16.5 | 通过 | 通过（正常完成和中断后两种场景） | 通过；文件未写 |

探针：`crates/store/examples/live_recovery_matrix.rs`，须显式 `--allow-model-usage PROVIDER`，使用两个隔离临时目录，不自动批准工具，不更改全局权限。原生会话创建/续接与 Prompt 均携带两个目录；不将连接成功等同于上下文恢复成功。

本轮回归：Core 21、Store 36、Desktop 25、双目录集成 3 项通过；Workspace 119 项通过（10 项忽略）；前端 174 项通过（全量 172 + 新增独立导出测试 2），TypeScript 和生产构建通过。覆盖停止失败后审批恢复、附加目录变更在终态前保存、缺失目录阻止执行、重建无自动重发、过期版本拒绝、重建投影保留、真实子进程退出后审批失效且重复恢复幂等，以及启动故障分类/摘要脱敏。

## 下一接手点

1. 补齐其余六家原生 GUI：Antigravity → Codex → Claude Code → Kimi → Grok → Cursor；逐家验证桌面/灵动岛审批、取消、重启及恢复方式。
2. 宿主与原生执行状态已分离，详见 [状态一致性](execution-state-consistency.md)；后续增强为原生状态只读核验和后续变更采集，不默认结束外部 Agent。
3. 逐事实交付确认及超出有界历史快照的按需读取。

浅深主题、原生目录选择、其他启动错误类别及发布版仍待覆盖。损坏库浏览/自动修库不属于当前交付，高级文件系统恢复、adoption 恢复和 worktree 清理不在本次范围。

## 关键文件

`crates/core/src/recovery.rs`、Core `lib.rs`、Protocol Rust/TS、
`apps/desktop/src/RecoveryCenter.tsx`、`App.tsx`、`tauriClient.ts`、`demoClient.ts`、
`apps/desktop/src-tauri/src/lib.rs`。沿用 artifact_export 的文件安全边界。

## 历史：修复前失败复验（2026-09-11，已被上方通过记录取代）

重新构建当前代码后，ZCode 双目录中断后续接两次失败；首轮与运行取消通过，新驱动 resume 请求返回后，首条 Prompt 返回错误终态（`native_failure_error_result`）。独立单目录 `real_resume_reattaches_across_driver_instances` 同样在续接轮缺少 `TurnCompleted`，因此并非仅取消路径失败。暂不宣称根因已确定。

Antigravity / ZCode Driver 单元测试分别 12 / 10 项通过，另有 4 / 9 项真实用例默认忽略。本次真实续接验收单独显式执行，不能用单元测试通过覆盖上述失败。

Antigravity 复验通过：双目录运行取消、跨进程续接回忆及待审批取消全部通过，目标文件未写。验收脚本补接 `approval_bridge::run_hook_if_requested`；此前未产生审批的结果受脚本缺少 Hook 入口影响，不能视为 Driver 审批失败。

## 历史：ZCode 续接失败原因分析（2026-09-11，修复已完成）

原生 JSON-RPC `session/send` 返回 `-32031`，`data.code=ZCODE_RUNTIME_MODEL_UNAVAILABLE`，提示历史任务模型不可用。`session/resume` 本身成功；此错误发生在模型执行前，不是审批回包失败，也没有证据说明对话历史丢失。

恢复后与 workspace/readState 的模型引用均为 `bigmodel-coding-plan / GLM-5.3-Flash`。两组额外真实对照均失败：恢复后 session/setModel 选择同一可用模型；恢复前先读取 workspace 模型列表。因此不能将提示直接解释为模型下架，也不能将简单重选模型视为已验证修复。

本机 ZCode 原生实现中，冷恢复的 `s8i/createRecord` 在 resume 且没有 runtimeModel、且 `$8` 未在进程内 workspaceModelCatalogs 查到可执行 target 时设置 `restoreWarning`。`session/send` 在执行前检查该警告并返回上述错误。模型列表能显示本地模型，不代表该进程内 registry/runtimeModel 已就绪；`session/setModel` 只设置引用也不足以消除警告。

修复前，本项目 `resume_session` 只发送 sessionId；`resume_selected_session` 随后仅调用 setModel/setThoughtLevel，没有补齐冷恢复需要的 runtimeModel/provider registry。另一个缺口是 `map_request_error` 丢弃了该原生错误码，最终显示通用 TurnFailed，掩盖了原因。

当时提出的修复方向（后续实现与复验见本文顶部）：按原生协议初始化同一 workspace 的受支持模型运行配置/registry，再恢复会话；保留模型、思考强度与历史，不能通过跳过 restoreWarning 或伪造 headersApplied 修复。补原生错误码的白名单分类。修复必须再验证正常结束后续接、中断后续接和标记回忆；该分析批次仅定位原因，后续修复批次已完成真实验证。

脱敏证据：zcode-cold-resume.json（本地证据：`experiments/recovery-analysis/2026-09-11/zcode-cold-resume.json`）。临时诊断输出代码与对照性测试改动已撤回，保留原审批修复及原验收断言。
