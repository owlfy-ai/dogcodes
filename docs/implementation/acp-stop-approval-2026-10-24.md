# ACP 停止确认与审批归属（2026-10-24）

本阶段接续 [ACP 清理第一阶段](acp-process-cleanup-2026-10-23.md)，补齐一次 prompt 的停止确认和审批/问答回复隔离。不是整个 ACP、多会话共享连接或远控视频目标的完成声明。

## 停止行为

- 每次 prompt 持有自己的 Turn，冻结原 native session、物理连接 finished 身份、控制器、派发状态和终态接收端。终态发送端由独立 prompt 收尾任务持有，避免对象自己持有完成信号而永远等不到通道关闭。
- interrupt 先在生命周期锁内核实目标属于原轮次，再设置停止状态；错误会话的停止不修改全局中断/重建标志，也不清掉当前会话的审批。
- 一次停止由独立任务持有，后续等待复用原结果。取消调用方、重复点击不重置期限或重复发 session/cancel。生产等待 prompt 终态上限为 10 秒，超时明确失败并请求原 native 作用域后台关闭；不把排队、ACK、turn_active=false 或管道关闭当成停止成功。失败结果保留，显式恢复完成后才重新发送。
- SDK 请求入队和 sent 标记在同一 dispatch 锁内完成。取消在 prompt 尚未入队时到达则阻止派发；晚到的 cancel 检查原终态，在原任务结束后不能针对下一轮重新发送。停止未结算时拒绝新建/恢复/发送；prompt 的占用、标志重置和终态发布与生命周期锁排序。
- prompt 返回错误后要求连接恢复，不继续复用原连接中不确定的会话设置。CodeBuddy 原有取消后重建规则保留。

## 审批与问答

原实现有两个通过私有运行时复现的问题：respond_to_interaction 忽略传入会话句柄；每个连接的审批计数从 1 开始，旧连接 acp-perm-1 可命中新连接 acp-perm-1。

- 审批/问答采用独立生成的随机 ULID 身份，不随连接或进程重置计数。
- 回复与停止/结束按同一生命周期锁排序。只能回复当前活动、未关闭、未停止的会话；审批还核实 pending 中的原 native session。
- 会话、选项和表单校验在取走 pending 之前完成；错误调用保留合法请求，合法回复只送一次。不再先 remove 再 insert，避免错误选项造成短暂缺失或与停止竞争复活。
- 真实连接的审批回调持有 Shared 的弱引用。与当前活动会话不符、已关闭或停止后到达的请求，直接回复 native Cancelled，不插入 pending、不展示为当前任务的新审批。问答同样核实；供应商缺少 sessionId 的问答暂绑定唯一活动轮次。
- 停止只取消目标会话的权限等待与问答桥，不生成允许或用户答案。正常审批和合法结构化问答仍可完成。

没有改变 Core/手机通信协议，也没有新增 UI。手机与服务端生产代码未修改；日常 Core 未替换。

## 验证

本轮较上一阶段新增 11 项私有运行时回归。四项停止用例及两项审批归属用例先在旧代码上失败：

1. 两个已取消的停止等待加入同一次取消，并等待真实 prompt 响应。
2. native 忽略取消时不能报停止成功或任务完成。
3. 错误会话不能停止当前轮次或改变其标志。
4. metadata 命令阻塞时，取消先于排队 prompt，原 prompt 不得发给 native。
5. 其它会话的句柄不能回答当前审批，原审批保留。
6. 重建/恢复后的审批身份不能与旧请求相同，旧回答不能消耗新审批。
7. 错误审批选项保留请求，合法回答与重复回答只产生一次 selected 响应。
8. native 提供非活动 session 的审批被取消，不进入当前 UI。
9. 停止后才到达的 native 审批不再展示或遗留 pending。
10. 显式 session / 供应商无 session 问答均验证当前调用方和表单，错误答案保留，合法答案只回复一次。
11. 停止问答会取消 native 桥，迟到答案被拒绝，不误报完成。

macOS ACP 完整 89 项通过，21 项原有真实/按需用例不启用；all-targets 严格 Clippy 通过。Core 92 项、桌面完整 320 项通过，7 项桌面按需测试跳过；桌面 all-targets 严格 Clippy 通过。Windows GNU all-targets 交叉检查与严格 Clippy 通过；这不是 Windows 实际运行或 MSVC 环境验收。

10 月 25 日补齐隔离 Linux 验证：ACP 完整 89 项通过，21 项真实/按需用例未启用，all-targets 严格 Clippy 通过。原 Linux 测试镜像/缓存已不存在，本次从本机 rust:1.97.1-slim-bookworm 重建独立环境，补 Python、procps、GLib 开发依赖及 Clippy；没有更改三项现有 dev 服务。最初缺 GLib 的构建失败在补依赖后完成复验，不把先前失败作为通过证据。

本轮不运行真实 Agent、模型、模拟器，不操作用户屏幕、键鼠或剪贴板。

## 仍须继续

- **逐会话关闭**：本阶段结束时 shutdown 仍为连接级，共用多个空闲业务会话时关闭一个仍可能结束其它连接。10 月 25 日已补本机路由/缓存退休、保留其它 owner、最后物理退出和配置结算，见 [接续记录](acp-session-retirement-2026-10-25.md)；当前业务退休未接入可选的 `session/close`，不能将本机退休或 Core 独立实例当作 native 单会话关闭证明；SDK 已包含该能力的订正及后续接入见上述接续记录。
- metadata 新建/load/config 的取消和未知结果、通知与活动轮次归属、缺 sessionId 的供应商迟到回调，仍需结合逐会话关闭与物理实例身份继续审计。
- Core 顶层强退、其它辅助事务、持续输出背压、跨 Core / 同 native session 别名与物理实例代次、Pi 全局会话表锁边界仍保留。
- PC/手机功能和操作恢复完整核对按当前协议进行；不直接用历史终端/取消/电源 RPC 记录证明当前行为。
- 原视频硬件加速、特殊采集来源、更高编码能力、三平台/手机实机、真实 TURN 续期/弱网、地区网络、推送凭据与官方部署验收保持完整范围。


## 2026-10-27：CodeBuddy 问答桥接固定原轮次

日常触发场景：用户回答 AskUserQuestion 后，CodeBuddy 的原 prompt 可能已结束；异步桥接尚未消费答案或还在等待 `_codebuddy.ai/resolveInterruption` 确认，而同一个原生会话已经开始下一轮。原实现没有在派发时复核原 Turn，并在请求失败后读取共享 updates 通道，导致旧答案可晚发、旧失败提示/QuestionClosed 可进入下一轮。

现在桥接只捕获原 Shared 的弱引用与既有 Turn 对象，不增加持久状态。派发在原生命周期 gate 内复核物理 epoch、原 Turn、当前会话、停止/退休/配置状态，并同步放入 SDK outbox；后续等待原响应不占 gate、不重发。迟到失败再次核对原轮次，不能发布到新输出通道。原 RPC responder 仍按其独立 ID 结束，不因失效桥接向新轮次发送额外 deny。也不长期持有原 updates sender，以免等待原生确认拖住已完成时间线的关闭。

五项私有 Python ACP 回归覆盖：正常回答及用户取消的原生 payload、派发前已结束的旧答案、派发后迟到拒绝、新轮次问答可继续、停止后不发送第二个原生动作、原轮次内的真实投递失败仍提示。旧生产实现下两个迟到场景实际失败，其中一项捕获到新流里的旧 ProtocolWarning/QuestionClosed；修复后五项通过。测试故意复用相同 native session/tool ID，并以文件屏障控制原 prompt 结束和原回复，不靠长时间睡眠假定事件已处理。

macOS/Linux ACP 各 141 项通过，各 21 项按需单元测试及六项真实集成测试未启用；两平台严格 Clippy 和 Windows GNU all-targets 静态严格检查通过。macOS Core/存储集成 59 项、desktop 348 项通过（8 项桌面按需测试未启用），沿用 CI 的测试线程栈配置。证据 `/tmp/dogcodes-codebuddy-bridge-before3.log`、`/tmp/dogcodes-codebuddy-bridge-after.log`、`/tmp/dogcodes-codebuddy-bridge-macos.log`、`/tmp/dogcodes-codebuddy-bridge-strict-macos.log`、`/tmp/dogcodes-codebuddy-bridge-windows.log`；Linux 和依赖回归 `/tmp/dogcodes-codebuddy-bridge-linux.log`、`/tmp/dogcodes-codebuddy-bridge-core-integration.log`、`/tmp/dogcodes-codebuddy-bridge-desktop.log`。没有调用已安装 Agent、真实模型或日常 Core，也没有提交/推送。

这是明确带身份的问答桥接修复；缺少原生会话/轮次身份的供应商回调、真实工具链与设备验收仍按当前工作清单继续。本轮未改 PC/手机协议、兼容旧版、迁移数据或增加新的工作包。
