# 原生 Agent 进程清理：第一阶段（2026-10-23）

本节补齐 Antigravity 的进程退出确认，并建立其它原生驱动可复用的 `process-scope::OwnedChild`。其它驱动尚未接入，不能把本节视为全部原生驱动清理已经完成。只做代码及本地隔离验证，跨平台实机联调后置。

## 行为与所有权

- EOF 只说明输出流结束，不能证明 CLI、工具子进程和审批连接已经退出；actor 同时独立监测进程退出，后代持有 stdout 也不会挡住原组清理。shutdown、空闲会话回收不再提前移除会话或标记 finished。
- 独立会话 actor 持有原进程作用域、认证启动互斥、stderr 任务及审批桥任务。调用方取消等待或超时不取消清理；未确认时保留会话，按原对象重试，后台继续清理并退避。
- 等待认证启动互斥有 8 秒上限，提示等待上个进程清理，不无限挂起。启动互斥属于 actor；清理完成后即使旧 Arc 仍被引用，也能启动下一会话。
- 空闲回收与开始新任务共用短生命周期锁。实际关闭前再次检查 active，防止旧空闲快照误关新任务；关闭开始后拒绝新的 stdin 写入。
- 关闭时继续有限度地排空 stdout；主动关闭的活动轮次发 TurnInterrupted，异常 EOF 发 TurnFailed。native scope、reader、stderr、审批桥服务及请求任务全部收尾后才发布清理完成。
- readiness probe 也由独立 actor 持有原作用域和启动互斥；取消探测调用方不能让下一 CLI 提前启动。原进程退出尚未确认时，由后台重试继续持有互斥。
- shutdown 从 map 删除时比较 Arc 身份；已经开始等待的旧关闭不能删除替换后的对象。

## OS 边界

Unix 每次启动创建独立进程组。使用 waitid(WNOWAIT) 观察直接子进程，先保留其未回收身份，再对原组请求 TERM/KILL。回收直接子进程后只探测，不再按旧数字发送信号；只有组返回 ESRCH 才确认退出，EPERM 不当作已停止。清理 future 被取消后，下一次继续原终止状态。

Windows 先挂起创建进程，纳入私有 Job Object 后再恢复初始线程。Job 设置 KILL_ON_JOB_CLOSE；主动关闭使用原 Job 句柄，查询 ActiveProcesses 为零并回收直接子进程后确认。没有按进程名或迟到 PID 运行 taskkill。FFI 单独隔离在 process-scope；runtime/驱动仍禁止 unsafe。

平台设计依据：[POSIX wait/WNOWAIT](https://pubs.opengroup.org/onlinepubs/9699919799/functions/wait.html)、[Microsoft Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)、[先挂起再关联 Job 的原因](https://devblogs.microsoft.com/oldnewthing/20131209-00/?p=2433)。

Unix 进程组不是沙箱：主动 setsid/换组逃离的后代不在本阶段保证内，也不能保证 Core 被 SIGKILL 或直接进程退出时有机会完成异步收尾。Drop 只请求尽力清理，不能充当退出回执。直接子进程须由 OwnedChild 独占回收。跨 Core 代次与共享驱动进程的退出仍需继续审计。

## 本地验证

- macOS：process-scope 4 项、Antigravity 27 项、runtime 13 项通过；5 项原有按需/真实 Agent 用例未启用。
- 隔离 Linux：上述三组分别 4、27、11 项通过；5 项按需用例未启用。容器使用独立 init 回收测试孤儿后代，不接触开发者实际 Agent 或桌面。
- 回归用私有 shell CLI 与原审批 Hook 夹具；覆盖 TERM 不响应、直接进程先退出但后代持有管道（进程作用域和真实 actor 两层，活动轮次须报告失败而非完成/主动中断）、取消关闭后重试、无关兄弟进程不被杀、旧引用不占用启动互斥、回收时新轮次活跃、Driver Drop、取消 readiness 后仍保留认证互斥，以及主动关闭不误报任务完成。
- process-scope/runtime/Antigravity 的 macOS、Linux 严格 Clippy 通过；Antigravity（含新 Windows native scope）的 x86_64-pc-windows-msvc 交叉检查通过。Windows native scope 的 all-targets 严格 Clippy 也通过，但没有实际运行验证。
- 桌面完整库 331 项通过/7 项按需跳过，Core 92 项通过；桌面 all-targets 严格 Clippy 通过。手机与服务端没有生产代码变化；不重启日常 Core，不操作实际屏幕、输入或剪贴板，不运行付费模型用例。

## 继续工作

1. Pi、ZCode、ACP、Codex 的正常退出、强退、等待取消、共享连接和工具/审批任务收尾；不能直接把每会话 shutdown 都改为杀共享进程。
2. 其它探测/辅助子进程及顶层 Core 退出、异常退出路径；旧 native session 别名在冷恢复后与新物理实例的代次隔离。
3. 视频硬件加速、特殊采集来源与更高编码能力；本阶段未修改视频管线。
4. 已保留的 Windows/macOS/Linux X11/Wayland、iOS/Android 实机、真实硬解、TURN 续期、弱网和地区网络、推送凭据及官方服务部署验收继续保留。

## 后续进展

Pi 的当前 Core 生命周期、关闭及中断等待恢复已完成代码和隔离验证，见 [Pi 退出与中断恢复](pi-process-cleanup-2026-10-23.md)。上文“其它驱动尚未接入”描述的是本阶段落地时的范围；Pi 的跨 Core/物理代次边界仍保留，其余驱动及视频/实机等完整范围继续推进。

ZCode 的当前 Core 会话关闭/停止和私有共享 CLI 已接入原作用域清理，见 [ZCode 停止、关闭与私有进程清理](zcode-process-cleanup-2026-10-23.md)。不把单个会话关闭升级为杀共享进程；stop ACK、空列表和本地循环结束均不作为实际停止证明。临时元数据事务、辅助进程和跨 Core 边界仍保留，ACP/Codex 及完整后续范围不缩减。

继续补齐 Pi/Antigravity 的通信取消和 Antigravity 停止确认：半帧取消在锁内使原管线失效；Pi 控制请求等待取消和发送前轮次占用不会留下不确定状态；Antigravity 无换行输出有界，停止等待原作用域真实退出。Mac/Linux Pi 各 22 项、Antigravity 各 30 项与严格检查通过，Windows all-targets 静态检查通过，见 [通信取消与停止确认](agent-input-cancellation-2026-10-23.md)。ACP/Codex、临时/辅助事务和 Core 顶层异常退出仍是下一审计范围。

Codex 的 Thread 与额度辅助进程现已接入原作用域、独立 supervisor 和退出回执；读取半帧取消、写入/请求取消、停止只确认 ACK、额度缓存引用环均已修复，见 [Codex 清理与停止确认](codex-process-cleanup-2026-10-23.md)。Mac/Linux 各 46 项、Windows all-targets 静态检查、最新桌面 331 项/Core 92 项及严格检查通过。ACP、其它临时/辅助事务、背压流控、顶层 Core 强退及跨物理代次，以及视频和全部实机等原完整范围仍保留。

ACP 进程/后台任务第一阶段也已接入原作用域和独立清理，见 [ACP 清理第一阶段](acp-process-cleanup-2026-10-23.md)。Mac/Linux 各 78 项、Windows GNU all-targets 及严格检查通过，当前工作区桌面 320 项/Core 92 项及严格检查通过。MSVC 的 C 依赖构建缺 Windows 头，未视为通过；Windows 实际 Job 待验收。ACP interrupt 终态、逐 session 退休和其它元数据/辅助事务、跨 Core/物理代次仍保持开放，不能把五种驱动已接 scope 当作全部生命周期已经完成。
