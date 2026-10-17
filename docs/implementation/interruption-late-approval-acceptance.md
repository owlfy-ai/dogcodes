# 异常中断与迟到审批验收（2026-09-11）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

结论修订：ZCode 代表性原生 GUI 硬杀恢复、旧审批失效和不重放通过；迟到/并发审批的确定性后端测试通过。已批准 shell 在宿主 SIGKILL 后继续运行是观察事实，不应未经退出策略约定就判为缺陷。需要区分宿主断连与任务终态，后续已实现 [状态一致性](execution-state-consistency.md)。下方保留修订前测试的实际行为。

## 原生 GUI 实测

使用 `/tmp/dogcodes Recovery Verify.app`、独立数据库/scratch、独立 Vite 1431，保留用户 1420 服务。真实 ZCode CLI 0.16.5，GLM-5.3-Flash / low。通过 UI 提交与审批，只对核对过完整路径的隔离宿主 PID 发送 SIGKILL。

| 用例 | 结果 |
| --- | --- |
| 待审批时 SIGKILL 宿主 → 重启 | 通过。旧审批卡从桌面和灵动岛消失；仅一条 approval.expired(reason=host_interrupted)，回合 host_interrupted；crash-pending.txt 始终不存在。重启本身未提交新消息。 |
| 批准后命令运行期间 SIGKILL → 重启 | 恢复通过。确认 START 已写、END 未写时终止宿主；旧回合被终结，恢复 Artifact 和桌面变更卡包含 crash-running-count.txt。 |
| 硬杀后新消息 | 通过。仅执行新请求，正确回答 GUI-ORCHID-942；实际新建原生会话并附带历史包，不计为同一原生 session 续接。 |
| 已批准子进程是否随宿主死亡 | 观察结果，不作为默认停止要求。20 秒等待后 END 文件出现，START 文件仍只有一次 START。命令继续运行而非重新提交；当前宿主 SIGKILL 不能保证终止既有 shell。 |

本轮共新增三条 turn.queued（两条故障测试、一条恢复后新请求），两条 host_interrupted、一条 completed；仅一条 approval.resolved，属于明确批准的运行中测试。没有额外的自动提交。START 追加计数为一次，仅作为该测试命令未重放的证据。

恢复 Artifact 是启动检查时的快照：包含 START，不包含稍后由存活 shell 写入的 END。不能将它描述为存活进程最终完成后的完整文件证据；当前事件已使用 workspace_recovery_interval 归因，并说明存活外部进程可能继续修改文件。

## 确定性后端回归

- concurrent_and_late_approval_replies_cannot_approve_a_new_turn：两个独立回复使用同一快照版本并发提交，仅一个成功且 Driver 只收到一次；新轮次复用 perm-1 后，旧版本回复被 Repository Conflict 拒绝，新审批保持待处理，只有新决议进入 Driver。
- accepted_approval_delivery_rechecks_turn_and_ownership：模拟已接受回复延迟到投递阶段，分别覆盖轮次替换、Interrupting、Run 失去本实例归属；均记录 turn_closed_or_replaced 并在 Driver 查找/调用前返回。
- interrupted_runtime_recovers_durable_checkpoint_once：真实测试子进程无析构退出；恢复后旧审批失效，即使使用最新流版本再次批准也被拒绝、无新增事件；重复恢复幂等，变更证据保留。原有双目录中断恢复测试也通过。

Core lib 22 passed、Store lib 37 passed；桌面隔离构建通过。并发/迟到分支使用受控测试 Driver / Repository，未声称在 GUI 中人为延迟实际 IPC 或穷举全部调度间隙。当时其余供应商 GUI 硬杀矩阵尚未覆盖；当前 Antigravity 与其余六家已补验，见 [全 Agent GUI 验收](all-agents-gui-acceptance.md)。

## 后续问题

宿主退出不等于停止 Agent。断连时先保留未知状态，禁止重放和对旧审批继续操作；核实原生状态后再协调。当前已实现保守的待确认与用户确认解除流程；自动只读核验原生运行、后续变更采集仍属于后续增强。不能按名称批量终止用户 Agent。

证据目录：interruption-late（本地证据：`experiments/recovery-analysis/2026-09-11/interruption-late`）。其中 events.json、pending-restart.json、running-kill.json、running-recovery-artifact.json、files.json 记录真实测试事实；core-tests.log、store-tests.log 是最终回归日志。证据包含隔离测试内容和内部标识，不等同于对外脱敏诊断摘要。
