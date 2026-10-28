# ACP 共享连接会话退休与配置结算（2026-10-25）

接续 [停止与审批](acp-stop-approval-2026-10-24.md)。本轮修复一个会话关闭误杀其它会话、配置等待取消后失去所有者，以及配置失败后旧句柄仍可发送的问题。本阶段区分本机路由/缓存退休与物理 CLI 退出，尚未接入按 Agent 声明调用的 native 关闭。

2026-10-26 校对订正：当前锁定的 SDK 2.1.0 / schema 1.7.0 的 v1 已包含可选 `session/close`，由 `agentCapabilities.sessionCapabilities.close` 声明；遗漏/null 表示未声明支持。此前“稳定 v1 没有通用 close”的表述不准确，不能据此不再推进原生关闭。业务会话按声明能力接入仍是剩余工作；目录探测已在 [独立作用域阶段](acp-metadata-scope-2026-10-26.md) 先回收整条临时连接。

## 生命周期

- 每个经新建/显式恢复确认的业务会话登记 Resident，冻结原连接 finished 身份、原 native 控制器、最后一轮 Turn 及一次关闭结果。元数据探测会话不算业务 owner。未知 native handle 不能直接触发启动/发送，须先显式恢复。
- 关闭先在 setup 准入与生命周期锁内停用原会话路由，重复/取消等待加入原关闭结果。后台任务继续拥有清理；调用方最长等待 10 秒，未获证明不能报成功。
- 若仍有其它开放业务 owner，关闭空闲会话只清理本机 SessionStates；关闭活动会话先加入原 Turn 的停止，须原 prompt 正常结算后才发本机 ForgetSession 并等确认。该内部命令不产生不存在的 native RPC。若停止/终态不确定则保持退休中，不按本地空闲或停止超时假装完成。
- 最后一个开放 owner 关闭时，才请求原进程作用域退出并保留 cleanup marker，等原 SDK/后台任务/OS 作用域 finished 证明。所有会话都已申请关闭时也可清理整个原作用域，因此一个未知停止不会让其它 owner 的显式关闭永远无法释放连接。
- stop 超时不能杀仍被其它会话持有、或正在新建/恢复确认的共享连接。请求仍报告未确认；原关闭继续等待，不重复发取消。普通 Core 业务会话使用独立 Driver，保留原生进程清理路径。
- 本机 ForgetSession 清除值、绑定与 pending revision，保留退休标记忽略迟到配置通知。显式恢复前不得复用缓存，恢复重新走 native load；setup 开始后才解除 SessionStates 的退休标记。
- 物理连接移除和 registry 移除都比较原 Arc 身份。迟到清理不能删除替换对象；清理结果发布前完成这些变更。完成的 registry 条目释放，不长期持有旧 native 控制器/通道。

## 配置与事件

- 新建、恢复、prompt 准入和关闭受同一 setup gate 排序。new/load/config 发出后由独立任务持有原响应和准入，调用方取消不会释放一半配置的会话供其它操作使用。
- 结算成功但结果无人接收时，登记后退休该原会话；配置出错时停用原目标并清理本机绑定，错误返回后旧 handle 不能继续发送。`setup_in_progress` 也保护结算期间不被迟到停止/元数据重建请求结束物理连接。错误目标的停用与解除 setup 标志在同一生命周期锁内完成；interrupt 必须仍持有当前 Resident 的原 Turn，退休或重新配置后的旧轮次不能改变连接重建标志。
- 其它业务 owner 仍持有可用连接时，不因一个会话更换 Cursor 权限启动参数、CodeBuddy 工作目录或要求重建而直接结束大家的进程。
- 带 native sessionId 的流式更新只送到匹配且未退休的当前活动会话；退休会话的迟到文字不能进入其它会话。元数据/用量仍按各自 session 观察。审批/问答继续使用上一阶段的归属校验。

手机/服务端生产代码和通信协议没有变更。没有切换日常 Core 或调用安装的真实 Agent。

## 验证

新增 12 项回归，三项共享关闭和一项配置失败先在修复前失败：

1. 两个空闲 owner 关闭一个，另一个能继续 prompt；关闭者须 native load 后恢复，最后才退出 CLI。
2. 关闭空闲 owner 不停止另一个运行中的轮次。
3. 关闭运行中的会话等待真实取消结算，保留另一个 owner。
4. SDK 元数据命令阻塞时取消关闭等待，后台仍完成本机缓存退休，期间恢复被拒绝，之后 native load 成功。
5. 取消正在执行的恢复等待，准入保留到原响应，孤立结果自行退休；另一个 owner 关闭后原进程获证退出，无遗留 registry。
6. native 忽略停止时不结束其它 owner 的连接，不重复发取消；最后一个 owner 显式关闭后全部原退休完成。
7. 退休会话的迟到 native 文字不能进入另一会话的流，当前文字正常到达。
8. SessionStates 退休删除缓存/绑定/pending，并拒绝旧响应与迟到通知；显式 setup 后才能重新 resident。
9. 配置失败后旧句柄不能以未确认设置继续 prompt，另一个 owner 不受影响。
10. 未登记 native handle 不启动 CLI、不派发 prompt。
11. 退休会话的迟到 interrupt 不能改变 CodeBuddy 连接重建/中断标志，影响剩余 owner。
12. 同一 session 已完成新的配置结算后，旧 Turn 的 interrupt 不能再使配置失效；停止同时核实当前 Resident 的原 Turn 身份。

更新原“关闭其它空闲 handle 必须拒绝”的用例为“允许退休但不能杀当前轮次”，保留活动会话/进程存活断言；CodeBuddy 可控通道夹具显式登记原已初始化会话。私有 Python/stdin/stdout/任务组测试不使用真实凭据、模型或桌面。

- macOS/Linux ACP 完整各 **101 项通过**，各 21 项原有真实/按需用例跳过，all-targets 严格 Clippy 通过。
- macOS Core **92 项**、桌面完整库 **320 项**通过，7 项桌面按需用例跳过；桌面 all-targets 严格 Clippy 通过。
- Windows GNU all-targets 交叉检查和严格 Clippy 通过；没有 Windows 实际执行或 MSVC 新验收。
- Linux 复用独立 ACP 镜像/缓存和 init，不更改现有 server/postgres/redis dev 服务。手机/服务端无生产修改，不重复其未受影响的整套测试。

## 保留的完整范围

共享 CLI 内 Agent 自己持有的会话缓存/自主后台资源，不等同于本机 SessionStates；未声明 `session/close` 的 Agent 没有已确认的逐会话卸载能力；当前业务退休实现也尚未调用该可选方法。Core 独立业务 Driver 的关闭会结束其原物理作用域；直接共用 Driver 时本轮证明的是 prompt 结算、本机路由/缓存退休、其它 owner 保留和最后退出，不能泛称通用逐 session 原生 OS 资源释放已实现。

仍需继续：metadata/model/permission 辅助事务超时后 native 迟到副作用、无 sessionId 的供应商回调和相同 nativeId 再次调用旧句柄的歧义、其它共享/辅助进程、持续输出背压、顶层 Core 强退与跨 Core/物理代次、当前 PC/手机功能和操作恢复完整核对。原视频硬件加速、特殊来源、更高编码能力及全部三平台/手机实机、TURN 续期/弱网、地区网络、推送凭据和官方部署验收范围不缩减。
