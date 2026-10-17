# Review 当前交付审计

本轮按用户确认的 Diff 查看范围裁剪，具体边界见 [review-panel.md](review-panel.md)。此前数百批针对高级恢复系统的缺口清单已退出当前交付范围。

## 变更

移除子模块/整树恢复、目录拓扑重建、孤立候选清理、遗留锁隔离、索引续作管理和侧栏存储管理。保留查看主干、普通文件丢弃/撤销和暂存/取消暂存。Git 索引锁已简化为标准独占锁；保留文件范围、版本及发布前检查。

旧基线、普通恢复备份和高级恢复历史数据未清理；当前版本不执行旧高级恢复记录。普通文件元数据保护仍保留。

## 本轮验证

- `pnpm check`：类型检查、129 项前端测试、生产构建通过。
- `cargo test -p agent-workspace --lib`：119 通过、0 失败、10 ignored；ignored 未计入通过。
- `cargo test -p desktop --lib --features review-acceptance`：22 通过。
- `cargo clippy -p desktop --all-targets --features review-acceptance -- -D warnings`：通过。
- 原生隔离应用由当前源码构建，使用 `DOGCODES_REVIEW_MANUAL=1`，经电脑操作工具实际点击：打开 Review、非 Git 差异、左右对比、普通丢弃/撤销、子模块指针展示、Git 暂存/取消暂存；最终 Staged 显示无变化。
- 独立读盘确认非 Git 丢弃内容等于初始基线，撤销等于操作前字节；Git 暂存后的索引内容正确，取消暂存后索引差异为空、工作文件 CRLF 字节及子模块内部文件保持、index.lock 已释放。

本轮手动原生 fixture：系统临时目录下 `dogcodes-review-acceptance-8WM26H`。隔离应用已退出，数据保留。未运行真实模型调用；未重跑整套历史原生自动脚本，不把此次手动主流程冒充所有原生场景通过。

裁剪前备份：`~/Workspace/dogcodes-backups/before-diff-simplification-20260910-233314.tar.gz`。源码目录本身没有 Git 历史。

## 平台边界

Windows/Linux 实机仍待目标机器执行，见 [平台验收](review-platform-acceptance.md)。不以子模块对象库重建、完整 ACL/所有者恢复矩阵、任意故障恢复作为 Diff 交付门槛。

## 裁剪前的真实 Agent 验证

2026-09-10，macOS：Git 项目 Codex→Claude→Codex、非 Git 项目 Claude→Codex→Claude，共 6 次真实修改，7 项比较断言通过。经正式发送、交接和审批流程；Git 暂存保留第一轮内容，工作区保留第三轮内容，非 Git 始终使用会话初始基线。

随后复用产物提交并重启桌面，五种 Git 模式及非 Git 持久基线另 7 项通过，累计 14 项。报告位于系统临时目录 `dogcodes-review-acceptance-5kSRbs` 下的 `acceptance-report.json` 与 `history-report-1789056065966896000.json`。这是裁剪前证据；本轮没有重新调用模型，也不代表所有 Agent 或平台均已验证。复验入口见 [平台验收](review-platform-acceptance.md)。

## 打开性能优化（2026-09-11）

合并前端相同在途查询，避免开发版重复执行；同任务关闭重开保留冻结阅读状态并后台探测变化；后端每批最多三个目录并发读取，保留操作锁及原有文件校验。

KidsVibeCoding 三个实际 Git 目录、共三个变更文件，同一后端查询与统计路径、隔离临时缓存，两轮测量：串行 1,200/1,296 ms，并发 639/606 ms，均包含仓库发现。约减少 47%～53%。这是后端耗时，不包含窗口渲染或完整 IPC，不宣称原生点击端到端耗时。

验证：前端 144 项测试、类型检查与生产构建通过；桌面 feature 库 23 项测试通过，包含多目录同名文件、基线隔离、结果顺序和单目录错误。桌面包 `clippy --no-deps -- -D warnings` 通过；包含依赖的严格检查被另一个正在修改的 `driver-api/src/lib.rs` 的 `collapsible_if` 告警阻断，未修改该任务代码。临时项目计时测试已移除。

## Changes 界面（2026-09-11）

移除目录选择下拉框，使用可折叠目录树及 M/U/A/D/R 文件状态；目录和文件展示红绿行数，隐藏常规界面的绝对路径、基线哈希及数字文件模式。目录选择与文件选择不重复查询当前变化，历史模式仍按所选仓库请求。

浏览器隔离夹具实际检查了三目录、浅色/深色、折叠、文件选中及左右对照；不冒称原生业务端到端验收。前端 146 项测试和生产构建通过；原生验收脚本已改为目录树定位并通过语法检查，本轮未重跑原生自动脚本。

## 抽屉与独立窗口（2026-09-11）

Diff 改为主布局右列，分离时收起侧栏，原生窗口支持独立移动、缩放及回到侧栏。创建时直接传递阅读选项；修复 StrictMode 重复调用状态 updater 导致初始选中文件丢失，并加入回归。

前端 148 项、桌面 24 项测试通过，前端生产构建与桌面 feature 构建通过，桌面包严格 clippy（no-deps）通过。原生隔离 fixture `dogcodes-review-acceptance-8zp9Bm` 实际点击验证：抽屉推开聊天、分离、选中文件及左右对照保留、窗口移动/缩放操作、独立窗口暂存、回到侧栏、系统关闭按钮收回。独立读盘确认 `tracked.txt` 的索引为 `working`；收回后主侧栏提示新变化。未验证真实跨显示器拖动或 Windows/Linux。测试应用已退出，数据保留。
