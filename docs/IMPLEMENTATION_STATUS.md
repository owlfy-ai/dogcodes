# dogcodes 实施状态

更新日期：2026-10-16。当前状态统一维护在 [当前工作清单](CURRENT_WORK.md)，本页不再叠加不同日期的待办。

- 已实现范围：清单 D1–D6、D8–D10；D7 悬浮岛已移除，共享通信与状态能力保留给桌面和手机 App，见[移除记录](implementation/island-removal-2026-10-16.md)。D10 为补录的项目运行方式自动分析与一键运行。
- 已实现但仍待验收：V2–V5。
- 当前主线：DT-01 至 DT-10 桌面完善与验证，其中 DT-08/09/10 为发布前基础改造；R1–R3 GitHub 源码及 Windows/macOS/Linux 安装包首发。已开始 DT-01 与 R1/R2，见[发布准备记录](implementation/desktop-release-preparation-2026-10-16.md)；计划与完成标准仍见[桌面发布拆分](implementation/desktop-github-release-work-breakdown-2026-10-16.md)。
- 后续开发：F1–F2、E1–E2（F3 已退出范围），不自动成为桌面首发条件。
- 远程访问后移：RA-00 需求与拆分已完成；RA-02 本机基础由本期 DT-08/09/10 提前交付，其余远程能力留待桌面发布后推进，均未实施。[原规格](implementation/remote-access-work-breakdown-2026-10-16.md) 保留，状态以工作清单为准。
- 暂缓及退出范围：见清单对应章节，不能从原里程碑直接恢复为待办。

2026-09-27 删除行为已改为 [永久删除并清理历史 Diff/快照](implementation/conversation-permanent-deletion-2026-09-27.md)，移除最近删除界面；本轮验证范围见新记录。下面 2026-09-25 的删除/恢复验收属于旧版语义，不代表新版永久删除的桌面验收。

最近交付的 [会话删除与恢复](implementation/conversation-deletion-2026-09-25.md) 已通过自动化与隔离 macOS 桌面验证；默认后台服务 V1 也已通过，见 [服务验收记录](implementation/conversation-deletion-daemon-2026-09-25.md)。当前没有标记软件首版发布完成。

整理前的阶段状态和测试记录完整保存在 [历史归档](IMPLEMENTATION_STATUS_HISTORY_2026-09-25.md)。历史数字不替代新的验收，旧未完成项不直接代表现在仍未完成。

2026-09-27 审查改为 Agent 自主读取项目和 Git 差异；取消审查专用冻结编排，保留非 Git 撤销。实现与本轮验证见 [审查简化](implementation/review-simplification-2026-09-27.md)。
