# Commit & Push 的后台接续身份

日期：2026-10-16。对应 DT-09/DT-10，接续[后台操作记录](desktop-operation-history-2026-10-16.md)。

## 已实现

- `GitRequest.workflow` 将组合意图写入持久原请求：初始 commit 保留 `nextOperationId`，后续 push 保留 `parentOperationId` 和下一次重试身份。普通 commit 没有 workflow，不能从“提交成功”推导出用户要求推送。
- 新增 `GitAdvance { taskId, operationId }` / `core_review_git_advance`。调用者只指定要接续的原操作，后台在 ReviewHost gate 内读取原回执、核对 task/root/父子标识，并执行固定身份的下一步。
- 初始提交必须成功且 completed=true；推送只有已知失败才允许创建下一次重试。运行中、结果未知、暂存确认未通过、提交失败均不允许接续。已存在的子操作直接返回原回执，包括失败或未知；不会因重复点击旧的父操作而分配新 push。
- 新 push 的下一次重试身份由后台生成并随原请求持久化。两端同时接续、原窗口迟到调用及重新创建服务实例均复用同一子操作。实际副作用仍通过通用 journal；回执持久化失败保持结果未知，不宣称外部 Git exactly-once。
- Commit & Push 编辑器、原恢复面板改用后台接续。回复丢失时查询预留子操作；若仍无法核实，保留原组合记录。恢复查询可从父操作发现已受理子操作。
- “操作记录”的 Git 详情可主动继续推送/重试推送；即使本端没有原 localStorage，也能依据后台回执接续。只展开或查看详情不会自动执行。调用后展示实际子操作状态，部分失败不会错误变成成功。
- 端内缓存仍用于保存尚未确认后台受理的请求及界面跟踪；它不再决定新组合工作流的后续推送身份。已有无 workflow 的本端待恢复请求仍按原路径核实，不自动补造组合意图。
- 当前注册入口 106 个，其中生产 104 个；CoreCommand 数量不变。

## 验证

- 前端 `gitOperations`、`GitOperationRecovery`、`ReviewOperationHistory`、`ReviewGitActions` 共 52 项通过；类型检查通过。日志 `/tmp/dogcodes-git-workflow-frontend.log`、`/tmp/dogcodes-git-workflow-types.log`。包含空缓存接续、迟到原调用者、编辑器关闭后的失败推送恢复，以及既有暂存确认/错误提示回归。
- 原生 Review 专项 43 项通过，exit 0；日志 `/tmp/dogcodes-git-workflow-native.log`。新增真实 RPC/本地 Git 仓库用例覆盖两个客户端并发接续、失败推送、新服务实例从磁盘恢复关联、配置本地 bare remote 后重试、重复回执、提交计数不增加，以及未知/运行中/未确认/失败提交/普通提交/错误会话的拒绝。
- 新增实际丢弃接续 RPC 回复连接的路径后，仅复验两个工作流原生用例，2 项通过，exit 0；日志 `/tmp/dogcodes-git-workflow-disconnect.log`。本轮所有句柄已收取终态，空白检查通过。未跑前端全量、真实模型或安装包。

## 剩余发布门槛

- 后续已补共享结束状态及与接续的并发仲裁，见[结束状态记录](desktop-git-workflow-close-2026-10-16.md)。关闭保留原执行结果；仓库目标绑定仍是下一项。
- 后续已补组合推送目标绑定，见[推送目标记录](desktop-git-push-target-2026-10-16.md)。原提交 OID/分支/上游目标变化后拒绝旧接续；跨平台真实 Git hooks、代理/认证与并发外部配置变更仍需验收。
- UI 缓存写入失败、长链/长记录、三平台真实关窗/服务重启和磁盘异常需候选验收。当前 API 的新服务实例验证不等同于安装版真实进程崩溃恢复。
- 其他业务域的可靠性及共享状态仍按入口矩阵推进；DT-08/09/10 未整体完成。
