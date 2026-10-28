# ACP 进程与后台任务清理：第一阶段（2026-10-23）

本阶段补齐 ACP 连接的原生作用域、独立退出与后台任务清理。**第一阶段结束时单会话退休和停止真实终态确认仍未全部完成**，见末尾；后续停止/审批进展见 [10 月 24 日记录](acp-stop-approval-2026-10-24.md)。不能将本节视为 ACP 以及整个远控目标已完成。

## 已改变的行为

- ACP stdio 和临时 Kimi Web 用量辅助进程使用 `OwnedChild`，独立 actor 持有原进程组 / Windows 私有 Job。读取或探测等待被取消后，仍由原作用域继续清理；不只依赖直接子进程的 kill_on_drop。
- 新生命周期控制器独立于 SDK foreground。SDK 输出 EOF、IO 错误、进程本身退出或 owner 关闭均会进入清理；直接进程先退出而后代仍持有管道，也能观察并终止原作用域。Windows 以原 Job 证明，Unix 保留未回收身份后信号原进程组，不按迟到 PID 重杀。
- ACP Driver 独立 owner 信号可关闭连接，即使 prompt 收尾任务持有 Shared、Shared 又保留命令发送者。Driver 释放不能因这个引用链而让 CLI 永远运行。
- stdout 包装器在读入 SDK 前限制单条记录为 8 MiB，包括一直没有换行的异常输出。错误/EOF 使原管线关闭。单条预算不是累计流量断开阈值，stderr 原有有界诊断分类继续保留。
- 问答回复、CodeBuddy 问答转换、Grok 用量查询和并行 prompt/用量命令纳入同一后台任务组。关闭后不接收新任务，已有任务 abort 并 join，清空原审批/问答 map，再等待原生进程和 stderr 收尾。SDK foreground 也被明确 abort/join；不再留下无所有者的 tokio::spawn。
- Native stdio 有 5 秒自然退出宽限，以保留原生认证/配置的正常收尾机会。临时 Kimi Web helper 没有可用的 stdin EOF 关闭协议，不空等该宽限，直接使用原作用域的 TERM/KILL 与退出证明；不会结束用户的 Kimi 客户端。

## 关闭与重建

`shutdown` 不再清空连接后立即返回。先保留不继续发送的 cleanup marker，再请求原控制器退出；实际 SDK、后台任务和进程收尾后才置 finished，外部等待有 10 秒期限。取消或超时不会取消原清理，重试继续等待原 marker。

重建也保留同一 marker，未获原退出证明不启动替代进程。迟到的关闭只在 finished Arc 仍是原对象时删除连接，不能删除替换后的注册。临时探测成功也须先取得原退出证明，不能把 kill 请求当作完成。

已明确关闭的 native handle 在当前 Driver 内被标记；后续 prompt 不会为旧 handle 擅自重启 CLI 并派发，必须显式走原生 load/恢复。新建/恢复只有原连接仍可用才解除标记。prompt 的占用与收尾使用短生命周期锁，避免新旧 sink/活动状态交错。

关闭的目标如果不是当前活动会话，返回错误而不关闭正在工作的共享连接。正常 Core 已通过 `independent_session_driver` 为各业务会话隔离 Driver；这里的保护还覆盖直接共用同一 ACP Driver 的调用方。

## 本地证据

新增 10 项私有 Python/stdin/stdout/任务组回归；不运行真实 Agent、模型、用户凭据或实际桌面：

1. 取消 initialize 探测后原进程和后代仍会退出。
2. 输出 EOF 不等于存活进程已经停止；主进程退出但后代持管道也能清理。
3. 取消 shutdown 等待后保留 marker，同一对象清理完才移除。
4. Driver Drop 时 prompt 终态任务持有 Shared，仍能结束原进程并报告失败。
5. 关闭其它句柄不能杀掉当前活动会话。
6. 审批、SDK 和后台任务清理完成后才能确认关闭。
7. 关闭后的旧 handle 不会启动/发送；显式恢复后正常工作。
8. 没有换行的超限帧在进入 SDK 无限积累前停止。
9. Kimi 临时用量 helper 启动等待取消后仍清理原作用域。
10. 后台任务组关闭等待资源 Drop，关闭后的迟到任务不能重新运行。

macOS 与隔离 Linux ACP 完整各 78 项通过，各 21 项真实/按需用例保持跳过；两平台 all-targets 严格 Clippy 通过。Windows `x86_64-pc-windows-gnu` all-targets 交叉检查及严格 Clippy 通过，使用已安装的 MinGW 工具链。MSVC 检查在 sqlite/zstd 的 C 构建阶段缺少 Windows 标准头而失败，本机未安装额外工具链；不能记为 MSVC 或 Windows 实际运行已通过。

当前工作区最新桌面完整库 320 项/Core 92 项通过，另 7 项桌面按需用例未启用，桌面 all-targets 严格 Clippy 通过。同期工作区其它修改调整了测试总数，此处按本轮实际输出记录；早前 331 等数字保留为当时证据。

没有替换日常 Core、启动模拟器或操作用户屏幕/键鼠/剪贴板；不因 Driver 改造重复运行未修改的手机/服务端全套测试。

## 仍须推进

- **停止确认**：第一阶段 `interrupt` 仍只是把 session/cancel 放进队列；后续已接入冻结原轮次、真实终态等待及独立停止结果，见 [停止与审批接续](acp-stop-approval-2026-10-24.md)，未获确认不报成功。
- **真正的单会话退休**：第一阶段仍是连接级 shutdown，只保护不同活动会话；10 月 25 日已补业务 owner 的本机路由/缓存退休、原 Turn 结算和最后物理退出，见 [会话退休接续](acp-session-retirement-2026-10-25.md)。共享 Agent 内部资源仍须按其声明的可选 `session/close` 能力接入核对，不能把本机缓存退出等同于全部逐 session 原生资源已释放；此前协议没有 close 的表述已在接续记录订正。
- 命令取消、迟到审批/问答、metadata 新建/load/config 的取消和未知结果、通知的活动 session 归属，仍需与物理连接/轮次身份一起核对。
- 整个 Core 强退、跨 Core 与相同 native session 别名的物理代次、Unix 主动 setsid/换组逃离的后代，以及其它 helper 仍保留。
- 原生视频加速、特殊来源、更高编码能力；全部平台/手机实机、真实 TURN 续期/弱网、地区网络、推送凭据与官方服务部署验收保持原完整范围。
