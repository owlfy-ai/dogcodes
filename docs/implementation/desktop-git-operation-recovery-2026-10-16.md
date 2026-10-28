# Git 操作回执与恢复

日期：2026-10-16。对应 DT-09，并衔接 DT-08/10。范围是 commit、push、pull、sync；不是全部 Git/文件副作用验收完成。

2026-10-28 界面调整：按用户要求移除右侧边栏的独立 Git 操作详情卡片，包括操作说明、状态、核实/重发、继续/重试推送和结束操作按钮。`GitOperationRecovery` 组件及面板挂载已删除；普通 Git 操作入口和底层请求身份、回执处理保留。下文恢复面板交互为历史实现记录，相关底层工作流回归现位于 `ReviewGitWorkflow.test.tsx`。

## 行为与服务边界

- `review/git_operations.rs` 在共享 ReviewHost 下保存 `review/git-operations/<task>/<operation>.json`。请求绑定会话、根目录、action、提交说明、暂存选择和快照；operationId 为规范 UUID。同 ID 不同请求拒绝，同请求有终态则返回原结果，不重复执行 Git。
- 先原子保存意图并同步，再执行 Git；执行结果持久化后才回复。记录损坏、保存意图失败均不启动 Git。持久化结果失败或服务新实例发现旧 Running 意图，返回结果未知，不自动重放。Git 与记录文件没有共同事务，不宣称外部操作 exactly-once。
- 原生 `core_review_git_action` 新增必需 operationId；新增只读 `core_review_git_operation`，复用同一认证 review RPC。执行在后台 gate 内完成，并与任务删除和项目范围修改协调；读取回执不等待长 Git 操作占用的 gate。删除会话清理其记录并禁止继续查询。当前注册表为 101 项，其中生产 99 项。
- 桌面 `gitOperations.ts` 在发送前按操作 ID 独立保存原请求，不用一个共享存储槽覆盖其他窗口的请求。网络回复丢失时先查回执；后台无记录仅允许用户明确重发相同 ID/载荷，Running 或 Unknown 不重发。收到成功、失败和需要暂存确认分别处理。
- `GitOperationRecovery` 在打开/重开面板或待核实操作变化时查询事实源，支持核实、明确重发原请求和人工核对仓库后结束跟踪。不自动重试副作用；操作在发送时禁用恢复按钮。查询不可达和未找到记录不能直接丢掉请求身份。
- Commit & Push 在提交前预留后续推送 ID，保存提交完成状态；原界面与恢复界面使用同一推送 ID。推送结果未知不显示成已失败，也不提供换 ID 的推送重试。
- 提交后的推送明确失败时保留回执和已完成提交标记，关窗重开仍可重试推送。重试再次核实旧操作确已失败，沿用预先保存的重试 ID；同一失败操作的多个恢复界面不会生成不同重试身份。只重试推送，不再提交。成功后清除客户端待核实项，后台回执仍保留。
- 验收脚本注入 Git operationId，并把旧暂存调用的单个 `path` 改为实际入口要求的 `paths` 数组。脚本只做语法检查，尚未运行完整 GUI 验收。

## 验证证据

1. `cargo test -p desktop --lib git_operation -- --nocapture`：4 项通过，日志 `/tmp/dogcodes-git-operations-native.log`。包括记录重开后重放、不同参数拒绝、执行取消后的 Unknown、确认/失败/存储异常，以及真实认证 RPC 丢失提交回复后核实并重放仍只有一次新提交。服务重开测试采用新 ServiceControl/embedded 实例，不等于真实进程崩溃实机验收。
2. 前端最初专项 115 通过、3 个 ReviewPanel 夹具失败：旧面板夹具直接模拟 native action，没有模拟新增回执入口，真实客户端待核实记录影响其后续用例。调整这些既有 UI 单测的服务边界 mock，专用恢复集成用例仍使用真实 helper。随后五个文件共 119 项通过，日志 `/tmp/dogcodes-git-operations-frontend-final.log`；类型检查通过。
3. 补失败推送恢复后，针对 helper、恢复面板及提交组件运行 47 项：46 通过、1 个新增用例断言失败。断言原本检查整段文本不包含“重试推送”，误命中正常错误提示；改为检查是否存在该按钮，未改产品行为。仅复验恢复面板 4 项全部通过，日志 `/tmp/dogcodes-git-push-recovery-ui-final.log`；前一轮日志 `/tmp/dogcodes-git-push-recovery.log` 保留失败证据。当前相关五个文件累计 122 个不同前端用例最终通过，不是一轮全套 122 项运行。
4. 最后类型检查 exit 0，日志 `/tmp/dogcodes-git-push-recovery-types.log`。`node --check apps/desktop/src-tauri/src/review_acceptance.js` 与 diff 空白检查通过。本工作块未构建安装包、运行全量前端套件或调用真实模型。

## 后续发布前工作

- 后续已为暂存/取消暂存、丢弃/忽略及批量文件操作补持久回执，复用提取后的 operation_journal，见[文件回执记录](desktop-file-operation-recovery-2026-10-16.md)。逐轮撤销继续收口；本记录的四个 Git action 测试数字不扩大到其他功能。
- 后台可按已知 ID 查询；客户端待核实列表目前仍由端内持久请求驱动。后续多入口需继续核对后台列表/订阅和撤销等跨进程刷新，手机接入不应依赖桌面 localStorage。
- 回执保留到会话删除，增长与保留策略待测量。旧未知结果不能因为时间过去就删记录并自动重试。底层 Git/网络失败可能已造成部分影响，用户仍需核实仓库；不承诺远端回滚。
- 三平台真实 GUI 退出、服务进程重启、磁盘/网络故障、长 Git 操作响应和安装包候选验收未完成。DT-08/09/10 及 GitHub 发布门槛仍保持未完成。
