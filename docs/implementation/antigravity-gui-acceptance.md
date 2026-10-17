# Antigravity 原生 GUI 验收（2026-09-11）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

本轮使用 Antigravity CLI 1.2.1、默认模型，真实桌面与灵动岛按钮操作。独立 review-acceptance 新库，初始 scratch 与 `/tmp/dogcodes-ag-gui` 项目；未修改正式用户数据库。证据时间部分为 UTC 2026-09-12，对应本地 9 月 11 日。

## 结果

| 项目 | 结果 |
| --- | --- |
| 桌面批准 | 批准响应真实执行。首轮相对文件误写入 Hook 目录，不能算工作区验收通过；明确目录后的桌面批准落盘、文件变更采集通过。 |
| 工作目录修复复测 | 全新项目会话，用户未提供目录提示。Driver 注入主目录和附加目录说明，区分私有 Hook 目录；原生 Cwd 正确，ag-fixed-allow.txt 精确为 AG_FIXED_ALLOW。此修复是模型上下文约束，不是文件系统沙箱。 |
| 桌面拒绝 | 原生 Hook 拒绝，无 ag-desktop-deny.txt，无重试。 |
| 灵动岛批准 | 快捷允许一次通过；文件准确落盘，桌面同步显示完成及 Diff。 |
| 灵动岛拒绝 | 拒绝通过；ag-island-deny.txt 不存在，桌面显示原生拒绝。 |
| 脱敏审批回退 | 初始长临时路径被脱敏，岛要求主窗口处理，无快捷批准按钮；保留此限制，未降低审批校验。 |
| 待审批取消 | 审批消失，Hook 阻止执行，ag-cancel.txt 不存在；重启后无旧审批、岛待命。 |
| 待审批取消后重启续聊 | 正确回忆 AG-GUI-ORCHID-517；事件为新会话和 session.rebuild_context_prepared，是历史恢复。 |
| 正常重启原生续接 | 首次发现 agy_ 临时句柄错误持久化，原生 UUID 变化且无法直接回忆。修复后新会话前后 UUID 为 5738127f-3814-46cf-9b55-e5c12e1d4b84，不调用工具即回忆 AG-NATIVE-CEDAR-839。 |
| 运行中取消 | 记录 cancel_requested / interrupt_sent，最终 interrupted；但已启动的 30 秒 shell 继续写入 END。仅取消请求链路通过，不能宣称原生命令停止。未更改外部进程终止策略。 |
| 运行中 SIGKILL 宿主 | 第二个用例检测 START 且 END 未出现后精确杀独立宿主。重启三处显示待确认、工具结果未确认、停止禁用且保留占用，记录 turn.connection_lost。shell 后续写入 END，没有伪造本轮完成。第一个 25 秒用例自然完成，保护检查未发信号，不计入硬杀通过。 |
| 原生项目目录选择 | 创建项目对话框及 macOS 文件夹选择器通过。 |

## 修复

1. Antigravity 新建/续接进程保存工作区说明，每轮输入明确主目录、附加目录与 Hook 基础设施目录的区别。进程 fixture 验证实际输入包含说明且用户消息不变。
2. Store 对 driver.session_ready 更新当前轮次、准确安装所对应的 active session native ID；旧轮结束后迟到的 ID 不覆盖映射。不执行数据库升级或历史回填。

验证：Driver 14 passed、4 ignored（本轮 GUI 实测另计），Core 24 passed、Store 37 passed，迟到 ID / 重建专项通过；独立目标目录桌面构建通过。

## 边界与后续

本段仅记录 Antigravity 的当次范围；当前其他供应商结果见 [全 Agent GUI 验收](all-agents-gui-acceptance.md)。Antigravity 的运行取消不保证终止已经启动的 shell，需单独明确产品语义和原生能力；硬杀后的未知状态按既定方案保留。未在本轮点击“确认外部已结束”，未制造新的迟到 GUI 点击或待审批硬杀。浅深主题、发布包、其他供应商矩阵仍待补齐。

构建最初与用户开发构建共用 target，曾误启动到锁失败入口，未进入业务。后改为 `/tmp/agent-gui-target` 独立构建。验收应用及本轮 1431 服务已退出，未停止用户 1420 服务。

证据：antigravity-gui（本地证据：`experiments/recovery-analysis/2026-09-11/antigravity-gui`），包含 Canonical Events、文件内容、硬杀触发证据、汇总、构建与测试日志。初始误写 Hook 文件的内容已单独留证，属于本轮隔离临时数据。
