# 主会话受理与 Agent 禁用设置

后续进展：主会话安装 Ready/明确身份/当前选择检查已在[就绪准入记录](desktop-primary-ready-admission-2026-10-16.md)收口；下文保留本工作块当时的范围和发现。

对应 DT-09/DT-10。本工作块只收口共享禁用状态与新主会话受理；安装 Ready/当前选择及已受理驱动归属仍是后续必做范围，不代表主会话准入已全部完成。

## 本次变更

`Core::execute(SubmitTurn)` 在原回执快速路径之后取得 `agent_admission` 读锁，与 Agent 设置写入排序。取得锁后再次核对回执，覆盖等待期间同一请求已经受理的情况；只对新工作验证启用状态。验证使用实际 executor override，没有 override 才使用任务 preferred executor，避免禁用某 Agent 后通过旧会话或另一客户端绕过。

共享 `require_enabled_executor` 从问答/审查准入中提取，原有安装检查继续执行。主会话锁保持到校验、持久受理和 worker 派发结束；运行本身不持有此锁。禁用后新请求被拒绝，不写 turn.queued 或受理回执；旧请求仍核对完整指纹后返回原回执，不重发任务。审批、运行中权限修改及取消不应用新工作准入，禁用不自动结束已经受理的轮次。

未改变 CoreCommand、客户端 JSON 或数据库结构；没有自动换 Agent，没有清空用户设置。

## 验证范围与当前状态

新增确定性 Core 用例将设置写入排在新请求前，覆盖 preferred 和 executor override，并确认无 turn.queued。新增真实 loopback RPC 用例覆盖另一客户端的禁用拒绝、启用后受理、准备阶段阻塞时仍可禁用设置、禁用后原请求恢复/运行继续/指纹冲突及 override 拒绝。SQLite 运行中交互用例覆盖禁用后的手动审批、完整权限更新及取消回执重放。

Core **83 项**、store lib **67 项**、运行中交互 **7 项**和审查集成 **22 项**均已通过（共 179 项）。桌面 primary_admission RPC 1 项也已通过，命令 57709 已收取 exit 0，共 180 项通过。最初等待其他活跃工作区 check（PID 95287）释放 Cargo 锁，没有终止或重复启动进程。

首轮 81050 的测试访问 MemoryRepository 私有 events 字段导致编译失败，改用公开 sync_events；后续 99586 的 Core 已通过，但新取消测试的 task_id 变量名错误，修正后仅继续 store/desktop（57709），没有重复已通过的 Core。日志 `/tmp/dogcodes-primary-enabled-{core-final,store-final,desktop}.log`；`git diff --check` 通过。无新依赖、协议命令、数据库迁移、前端构建或生产打包。


## 安装与驱动归属的后续工作块

运行驱动归属随后已进入[独立修复与验证](desktop-run-driver-ownership-2026-10-16.md)；以下是本工作块结束时发现的问题与完整验收范围，安装 Ready/当前选择仍未收口。

本次读代码确认，`run_turn_inner`、`cancel_turn_inner` 和 `deliver_approval_response` 仍会从当前 drivers 表寻找 active executor；`select_installation` 切换时会移除相应驱动。即使只在 SubmitTurn 增加 Ready 检查，受理后、真正执行前或交互期间发生安装切换，仍可能找不到原驱动。因此下一步需要一起处理：

1. 新受理按明确 installation ID、Ready、当前选择和注册驱动核实可用性，与探测/选择写入排序；回执恢复优先。
2. 受理后按 run ID 持有执行所需的原 driver，worker、取消、审批以及运行中权限操作均使用同一归属，不能重新借用同 provider 的替换实例。
3. 与 RunOwnership 生命周期一起释放引用；崩溃恢复不能伪造仍存活的 driver。存量原会话恢复与新任务准入分别定义。
4. 用“接受→准备阻塞→切换安装→执行/审批/停止”实证覆盖边界，并更新目前没有安装身份/探测事实的测试与隔离验收夹具；不加入生产测试特例。

提交说明、项目分析、长记录开销与其他恢复仍须继续。DT-08/09/10、素材来源、三平台实际安装与候选验收保持未完成；签名及公证配置未修改，本轮不打包。
