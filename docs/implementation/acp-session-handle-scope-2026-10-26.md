# ACP 会话句柄与 Core 原运行归属

> 后续进展：Core 准备/派发阶段的取消控制已补齐并本地验证，见 [准备阶段取消](core-preparing-cancellation-2026-10-26.md)。其余 Driver、异常清理和真实环境边界继续保留。

日期：2026-10-26。工作包 2 的一部分；应用尚未发布，不增加旧协议兼容或数据迁移。

## 问题与处理

原 `SessionHandle` 只有原生 session ID。Agent 可以在新进程中重复返回 `s-1`，同一连接内的恢复也可以使用相同 ID。即使 Resident 已换新，旧句柄仍可能通过字符串匹配，向新会话发送任务、取消任务、回答新审批或关闭会话。Core 取消和审批原先也从数据库映射或 run ID 重新构造句柄，无法保留运行时归属。

- `SessionHandle` 增加私有运行时 scope。`scoped` 每次创建新归属，clone 保留归属；原生 ID 仍单独保存。scope 不进入 serde 数据，持久化/远程数据反序列化后不能获得本机 Resident 的操作权限。显式恢复会话必须取得新句柄，没有从磁盘补写 scope 的迁移。
- ACP 每次成功 new/load/本机已配置会话复用都建立新的 Resident 句柄。任务、取消、审批和问答在生命周期锁内核对原 scope、原生 ID、物理退出标记及连接是否已要求重建。原生 ID 相同也不代表原句柄仍有效。
- prompt 只使用已经建立的原连接；不能用旧句柄触发自动启动新进程。原进程退出后，显式恢复才能建立新连接并取得新句柄。
- 关闭也校验原归属。尚在关闭的同一 Resident 继续使用原回执；已完成关闭保存该 ID 最新一份完成句柄，允许原关闭重试。新 Resident 出现后，旧关闭不能退休或强杀它。保存完成句柄不保留原进程、Turn 或任务对象。
- `session/new` 错误返回同一物理连接中已拥有的 ID 时，拒绝覆盖原 Resident，也不对碰撞 ID 发 close。原生配置已无法确定，连接标为需要重建，后续显式恢复建立新物理作用域；不继续在不确定的配置上执行任务。
- Core 为每个自有 Run 保留实际取得的 SessionHandle。取消、权限审批和用户问答使用这份原句柄，不再从 session map 或 run ID 推造。运行所有权释放时一并释放句柄；没有句柄或 Run 已不由本 Core 管理时明确拒绝。自主续轮也绑定自己的原句柄。
- 选择变更失败后的重建清理，在恢复前读取并保留实际当前句柄；不会在失败后用同名字符串指向一个可能已替换的 Resident。`current_session_handle` 是只读快照，不启动或恢复 Agent。

其它 Driver 的结构体构造已改为构造器，以适应私有运行时字段；本轮没有宣称 Codex/Claude/ZCode 等适配器已经全部执行 ACP 相同的 scope 校验。它们当前的原生生命周期仍需分别核对。独立真实历史 MCP 验证示例也改用实际当前句柄清理，不能用持久化 ID 伪造 ACP 句柄。

## 验证

新增五项 Unix 私有 ACP 进程测试：

1. 同连接恢复后的旧句柄、仅含 ID 的句柄、反序列化句柄及另造 scope，均不能派发任务、取消、回答问答或关闭当前会话；当前问答仍可完成，另一个会话与原物理连接保留。
2. 新物理连接中相同 session ID 的当前审批，旧句柄不能回答且不能消费请求；当前句柄可以回答。
3. 新进程再次返回相同 session ID 时，旧句柄不能取消或关闭新任务；当前句柄可正常取消及重试已完成关闭。
4. 原进程意外退出、旧 Resident 还在本机表中时，用旧句柄发送也不能启动进程；显式恢复后才有第二次 initialize。
5. 同连接 new 返回碰撞 ID 时，原 Resident 保留，没有发 close/任务；显式恢复重建原物理作用域，旧句柄不能关闭新归属。

另新增一项 Driver API 测试验证 clone/ID 变更/序列化的归属边界，一项 Core 行为测试在没有数据库 session map、注册 Driver 已替换的条件下实际执行取消、审批、问答，核实仍使用原 Driver 与原 scope；释放 Run 后不能继续操作。原恢复测试改用恢复返回的句柄，保留原有进程退出、审批请求 ID 和辅助查询隔离断言。被放弃的恢复不再重新授权先前句柄。

- macOS 全工作区库测试通过，包含 Core **93 项**、desktop **347 项**、Driver API **29 项**及其它 Driver/存储/远控库；加入重复 ID 回归后，ACP 最终复核为 **135 项通过、21 项按需单元测试未启用**，六项真实集成测试未启用。
- 存储层 `cargo test -p agent-store --tests --locked` **146 项通过**，包含 72 项单元测试及 74 项集成测试，覆盖真实 Core 路径下的审批回执、运行权限、任务生命周期、Fork 和轮次审查。
- Linux ACP **135 项**、Core **93 项**、Driver API **29 项**通过；三包 all-targets 严格 Clippy 通过。
- macOS 全工作区 all-targets 严格 Clippy 通过；示例调整后单独严格检查通过。Windows GNU 的上述三包 all-targets 严格交叉检查通过，不能据此推断 Windows 实机通过。

证据日志：`/tmp/dogcodes-session-scopes-workspace-macos.log`（全库测试通过；随后的布尔表达式 lint 已修复，最终严格结果见下一文件）、`/tmp/dogcodes-session-scopes-final-macos.log`、`/tmp/dogcodes-session-scopes-linux.log`、`/tmp/dogcodes-session-scopes-windows.log`、`/tmp/dogcodes-session-scopes-example-check.log`。

未执行已安装 Agent、真实模型轮次、用户桌面/键鼠/剪贴板、手机模拟器或日常 Core 切换。私有进程的退出与请求日志用于核实实际副作用；Windows 仍只有静态验证。没有提交或推送 Git。

## 继续范围

本轮解决 ACP 句柄跨 Resident/物理连接的调用权限及 Core 原句柄传递，没有解决所有未标识的原生回调。同一物理连接中，Agent 只给出相同 session ID、没有原生执行代次的迟到事件，仍不能仅靠字符串完整归属；需继续核对 SDK 回调与会话设置时序。其它 Driver 的持久连接、Core 强退/重启、失败设置的原生资源清理，以及准备阶段取消/操作恢复也继续。

视频实机恢复、HDR/原生光标、GPU 路径、手机/PC 功能对齐及网络/部署验收，仍按 CURRENT_WORK 的八包范围推进，不以本轮通过替代整体完成。
