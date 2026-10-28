# Pi：退出确认与中断等待恢复（2026-10-23）

延续 [原生进程清理第一阶段](native-process-cleanup-2026-10-23.md)，Pi 已接入相同的 OwnedChild 进程作用域。本节完成当前 Core 内的 Pi 生命周期及取消等待恢复；跨 Core 崩溃恢复、持久 native session 别名与物理实例代次隔离仍未完成。

## 改动

Connection 不再把 Child 和 reader 的清理放在关闭调用方的 future 内。独立 supervisor 持有原进程、输出 reader、stdin 及退出结果；Connection/Driver Drop 发起关闭，调用方中断等待后 supervisor 仍继续。

关闭先使连接不可再发送控制命令或开始新轮次。只在能立即取得 stdin 锁时关闭它，避免被阻塞的写入拖住；保留 Pi 在 EOF 时保存原生配置/授权状态的 5 秒宽限，之后对原作用域终止并核实。supervisor 同时观察输出流和直接进程退出，后代持有 stdout 不会掩盖主进程退出。实际 OS 作用域和输出任务清理完成后才发布成功；等待错误、请求强杀、EOF 均不当作成功。

close 最多等 10 秒；后台清理失败时发布未确认错误、保留原作用域并按 1/2/4…30 秒退避重试。正常 shutdown 和中断失败后的 retire 仅在 close 确认后删除会话，且比较 Arc，防止已开始等待的旧 shutdown 删除冷恢复后的对象。恢复 disconnected 会话同样先确认原实例退出，取消恢复等待不会提前移除原条目。

readiness 和模型目录查询的临时 Connection 在调用方取消时，也通过 Drop 触发独立清理；不把关闭失败忽略后再返回健康结果。

中断另有独立任务及每次操作自己的完成 watch。取消等待不会遗留永久 cancelling 状态；并发/重复中断加入同一操作，不重新发送 native abort，也不读取下一次操作的结果。成功仍须满足原有三个条件：abort 回复、agent_settled、get_state 确认 isStreaming=false。超时/拒绝/连接断开会发起退出，不能伪造 TurnInterrupted 或成功完成。关闭会清掉 pending 控制回复和对话；持有 Connection 的 prompt 回复任务不会阻止 Driver Drop 清理。

## 本地验证

- macOS Pi：19 通过、2 项原有真实 Agent/模型按需测试未运行。新增 9 个用例。
- 隔离 Linux Pi：19 通过、2 项按需跳过；容器带独立 init 回收测试后代。
- 两端 Pi 严格 Clippy 通过；Windows MSVC 库交叉检查及 all-targets 严格 Clippy 通过。Windows 未实际运行。
- 桌面完整库：331 通过、7 项原有按需跳过；Core：92 通过。桌面 all-targets 严格 Clippy 通过。
- 新用例使用原有私有 Python RPC 夹具扩展，验证：取消 shutdown 等待后保留原对象/继续清理；EOF 后进程仍活着不能确认退出；直接进程退出但后代持管道；Driver Drop 时 prompt 仍未回复；取消 readiness 后清理原组；取消 abort 等待后重试只发一次 abort；未 settlement 的中断在调用方取消后仍清理并可冷恢复；旧关闭等待者不删除新实例；2 MiB 写入阻塞时关闭不被 10 秒写超时拖住。

只处理测试自己创建的进程；没有切换或重启日常 Core，没有运行手机模拟器、真实 Agent、屏幕采集、输入或剪贴板操作。手机、服务端与视频管线本节没有生产代码变化。

## 边界及后续

OwnedChild 的 Unix 保证是原进程组；主动换组/setsid 逃离的后代不在此保证内。Windows 依赖私有 Job 的实际行为，尚需实机验证。Drop 请求退出不能代替确认；整个运行时被直接终止、Core 崩溃及旧 native session 文件别名的跨实例请求仍需审计。

Pi 的恢复配置流程仍串行持有会话表锁，尚可进一步缩短 IO 期间的全局锁范围。不能把本节的 Arc 比较描述为已解决冷恢复后所有迟到请求：它保护已取得原连接的等待者，不解决之后才按相同文件别名查找的新关闭请求。

继续推进 ZCode、ACP、Codex 的共享连接/退出确认及辅助任务审计；原生视频硬件加速、特殊来源与编码能力，以及 Windows/macOS/Linux X11/Wayland、手机真机、TURN 续期/弱网、地区网络、推送凭据和官方部署等原完整范围保留。

## 后续通信取消修复

Pi 半帧写入取消、未确认控制请求等待取消和已占轮次但尚在等待 stdin 的 prompt 取消，现已补同步失效守卫和原作用域后台清理；pending reply 未移交时不保留空注册。macOS/Linux Pi 各 22 项通过，Windows all-targets 静态检查通过，详见 [Pi / Antigravity 通信取消与停止确认](agent-input-cancellation-2026-10-23.md)。这不关闭上面的全局会话锁、跨 Core/物理代次及原完整范围。
