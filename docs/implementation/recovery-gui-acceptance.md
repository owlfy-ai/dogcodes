# 原生 GUI 恢复与审批验收（2026-09-11）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

使用隔离的 `review-acceptance` 桌面实例、独立数据库与 scratch，通过原生桌面 / 灵动岛按钮操作；ZCode CLI 0.16.5、GLM-5.3-Flash、low。真实调用本机 Driver 与原生 Agent，以界面、Canonical Events 和目标文件三方核对。没有操作用户正式数据库。

| 路径 | 结果与证据 |
| --- | --- |
| 正常完成 → 退出应用 → 重启 → 原会话续聊 | 通过。同一原生 session ID，正确回答 GUI-ORCHID-942，模型和 low 保留。 |
| 桌面批准一次 | 通过。gui-approval-allow.txt 精确为 GUI_APPROVAL_OK，工具和回合完成。 |
| 灵动岛批准一次 | 通过。gui-island-allow.txt 精确为 GUI_ISLAND_OK，桌面同步完成。 |
| 灵动岛拒绝 | 通过。gui-approval-deny.txt 不存在；返回桌面显示拒绝并停止。 |
| 桌面拒绝 | 通过。gui-desktop-deny.txt 不存在；回合完成，未重试。 |
| 桌面待审批停止 → 灵动岛同步 | 通过。卡片消失、灵动岛随时待命、gui-cancel.txt 不存在；回合 interrupted。 |
| 待审批取消 → 退出 → 重启 → 新消息 | 历史恢复通过。实际走新原生会话与 session.rebuild_context_prepared，正确回答标记，没有补执行取消命令。不能记为同一原生 session ID 续接。 |
| 恢复中心会话失效 → 重建并保留历史 | 通过。仅对隔离 DB 的 session map 注入 missing；点击按钮后 rebuild_pending，turn.queued 仍为 7、session.ready 仍为 2；发送新消息后才建新会话和历史包，回答标记。没有删除真实原生会话。 |
| 正常诊断导出 Save / Cancel | 通过。原生保存对话框与成功/取消提示，JSON 只含 schemaVersion、generatedAt、lastSequence、truncated、items。 |
| 损坏 DB 启动 → 导出 Save / Cancel | 通过。独立损坏测试文件触发恢复页，无业务入口；文件字节未变、无 scratch。摘要 code=database_check_failed，含版本/时间/automaticReplay=false，不含路径和对话。 |
| 宿主锁故障导出取消 | 通过。锁失败页显示固定提示，原生保存取消后提示已取消导出。 |

共提交 9 条新消息，8 条完成、1 条主动中断；4 次审批决议。重建产生 2 份历史包（取消后恢复与显式重建各一次）。四次审批决议对应四个不同请求。文件内容核验不单独证明底层命令执行次数；未做并发竞态压力验收。

## 验收中修正

- 没有工具终态的历史回合不再显示“等待中/进行中”，改为“已结束 · 结果未确认”；不凭回合结束伪造工具成功或失败。
- 没有任何可展示字段的工具通知不再生成空白工具行。已通过真实历史页面复验，正常工具结果保留。
- 加载提示字号使用界面缩放变量，修复既有 windowChrome 回归失败。
- 隔离验收构建支持显式 DOGCODES_RECOVERY_GUI 开启真实 Agent 探测，不改变默认验收夹具行为。

最终 typecheck 通过，前端 45 个测试文件 / 184 项通过。

## 后续异常中断验收

运行/待审批 SIGKILL 的 ZCode GUI 恢复及迟到审批后端测试已补验，发现已批准 shell 继续运行的边界，见 [异常中断验收](interruption-late-approval-acceptance.md)。以下是本报告原批次的覆盖范围。

## 覆盖边界

本次是 ZCode 代表性 GUI 全链路及通用恢复/导出验收；此前七家 Agent 的 Driver 验收不能等同于七家 GUI 全量通过。未覆盖其余六家逐个 GUI 矩阵、运行中硬杀宿主、并发迟到审批竞争、深浅主题矩阵、所有启动错误类别及打包发布版。会话 missing 为受控故障注入，真实重建与模型回答未模拟。

证据：事件（本地证据：`experiments/recovery-analysis/2026-09-11/gui-e2e/events.json`）、文件核验（本地证据：`experiments/recovery-analysis/2026-09-11/gui-e2e/files.json`）、前端回归（本地证据：`experiments/recovery-analysis/2026-09-11/gui-e2e/frontend-tests.log`）、正常诊断（本地证据：`experiments/recovery-analysis/2026-09-11/gui-e2e/diagnostics.json`）、启动诊断（本地证据：`experiments/recovery-analysis/2026-09-11/gui-e2e/startup-diagnostics.json`）。事件证据含本次隔离测试提示及原生会话 ID，与可分享的脱敏诊断摘要不是同一类产物。
