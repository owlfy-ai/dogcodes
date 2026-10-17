# 主会话安装就绪准入

日期：2026-10-16。对应 DT-09/DT-10，接续主会话禁用检查和[运行驱动归属](desktop-run-driver-ownership-2026-10-16.md)。

## 行为

`Core::execute(SubmitTurn)` 新受理复用 `require_available_executor`：实际 executor override 优先，否则使用任务 preferred executor；需要共享启用状态、明确 installation ID、相同 provider 的 Ready 安装，且安装仍在当前选择后的可用投影中。随后仍在同一准入读锁内解析将持久化的 executor 并绑定注册驱动。安装探测、移除、选择与驱动注册通过同一写锁排序。

已受理请求在准入检查前核对原回执和完整指纹，安装变更不会导致重发或阻断查询原回执。已受理运行继续持有原驱动；审批、取消和运行中权限操作不重新应用新任务准入。不更换用户选择，不自动安装或登录。

## 夹具和隔离验收

过去不少主会话夹具只有 provider，没有安装身份或探测事实。生产检查不为这些夹具放行：

- `FakeDriver::with_executor` 允许离线驱动使用夹具提供的明确身份，不负责声明安装 Ready。
- store 单元夹具使用确定性合法安装 ID，并单独写入标准 `agent.probe_completed` 事件；重建投影后安装事实仍存在。辅助模块仅在测试编译，不进入生产 store。
- 可使用生成 ID 的集成夹具通过实际 Core 系统探测命令获得安装身份，再创建驱动。
- 原生隔离验收 examples 在其私有 Core 中建立 Installed 身份，构造驱动后登记该身份的 Ready 事实。说明字段明确标识隔离验收，这不替代桌面的真实发现、认证或就绪探测。未运行真实模型。

## 验证记录

最终通过 **224 项原生测试**：Core lib 84、store 138（67 lib、71 integration）、desktop primary_admission RPC 2。`cargo check -p agent-store --examples --locked` 也通过。最终命令 98239 已收取 exit 0，无本工作块遗留验证进程。日志 `/tmp/dogcodes-primary-ready-{core,store-final,examples-final,desktop}.log`；`git diff --check` 通过，两个私有签名配置路径的 `git check-ignore` 仍通过。

验证过程中，87117 先等待编辑器检查释放锁，Core 通过后遇到会话中断；确认句柄失效且无 Cargo/rustc 存活，才继续未完成阶段。80975 发现两处恢复夹具使用已 move 的 store，补 Arc 克隆并清理未使用 import 后，81375 的全部 store 测试通过。examples check 又发现既有 `live_history_mcp` 审批还缺当前协议的 turn_id/requested_at_sequence，已从原审批记录补齐；98239 仅重查 examples 并继续尚未运行的桌面 RPC，限制两个编译任务并发，没有重跑已通过的 Core/store。初始日志保留为 store/store-resume/examples。

新增 RPC 用例覆盖四种非 Ready 状态、缺失/不匹配身份、provider 不匹配、移除/选择其他安装后迟到的旧驱动注册；拒绝时无 turn.queued、无原请求回执，也没有调用原生 prompt。原 RPC 用例继续覆盖 Ready 受理、准备期间切换、禁用后回执恢复及实际 override。既有运行归属、审批、取消、自主续跑和恢复回归一并验证。

没有数据库迁移、依赖新增、客户端协议变更、前端构建或生产打包；签名/公证配置仍不修改。此项完成后还须继续提交说明、项目分析等执行入口准入，以及长期状态/回收、其他恢复边界、素材来源和三平台发布候选验收，不将 DT-08/09/10 或整体发布标完成。

## 后续入口核对发现

本轮静态核对 `crates/core/src/commit_summary.rs::summarize_commit` 和 `crates/core/src/project_commands.rs::analyze_project_commands`，两者仍直接查询 drivers，没有共用准入检查。项目运行分析的宿主 `project_runs.rs::analyze` 在保存 running 回执后启动后台 worker，各目录等待 gate 并扫描后才调用 Core 分析；下一步需要明确受理时与实际执行时的边界，保证接受后禁用/切换不会无意失去原驱动，也不能让排队中的新工作绕过共享设置。不能只给最终 Core 方法加一次检查就声称该组合流程闭环。
