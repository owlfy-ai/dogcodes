# 原生 Agent 进程清理：第一阶段（2026-10-23）

## 2026-10-27：Core 整体退出策略与实现

改动前先核对现有边界。使用改动前的 `process-scope::OwnedChild` 构建仓库外的私有 Rust 宿主，启动独立组中的测试 shell 与 sleep 子进程；另启动无关兄弟进程作隔离对照。不是安装的 Agent、真实任务或日常 Core。

macOS 和带 init 的隔离 Linux 上分别完成三种观察：正常返回、执行 OwnedChild Drop 后，两级受管进程均结束；宿主直接 `process::exit(73)` 或被 SIGKILL 时，两级进程仍存活。因此已有 kill-on-drop/组清理不能证明 Unix Core 整体异常退出时已清理原任务，之前这一项确实尚未完成。Windows 当前路径使用私有 KILL_ON_JOB_CLOSE Job；本轮只核对源码，没有 Windows 实机证据。

所有残留都仅在私有测试组内，观察后按仍存活的原成员核实组 ID 再清理，确认主/子进程消失；无关兄弟在各次组清理后仍存活，最后由测试自行回收。临时容器已退出，原三个开发服务容器保持不变。最终 macOS 结果 `/tmp/dogcodes-core-exit-audit-macos.log`，Linux 构建及结果 `/tmp/dogcodes-core-exit-audit-linux-final.log`；临时审计工程和锁文件保留在 `/var/folders/8v/v4z1j8v93c3fkv64hsmxx83w0000gn/T/dogcodes-core-exit-audit-20261027-gmnoqepp`，未加入产品测试或发布脚本。

用户已确认：Core 崩溃或被强制结束时，结束受管 Agent CLI 和普通工具进程；重启恢复会话，任务不自动重发。关闭主窗口后独立 Core 继续工作，保持原行为。

实现复用 `process-scope::OwnedChild`：Unix 在原私有进程组内增加一个持有私有生命周期管道的 guard，Core 端句柄关闭后由 guard 结束自身所在组。它不按历史 PID 查找目标；原生 CLI 正常退出、TERM 宽限、取消清理仍走原有路径。guard 通过固定 `/bin/sh` 程序等待，不读取用户命令或环境；双重 fork 后由系统接管，避免成为 CLI 的 wait-all-children 等待对象。就绪握手有固定期限并正确重试 SIGCHLD 的 EINTR，fork 后只使用栈和异步信号安全的 OS 操作，立即 exec 释放继承地址空间。参考 [Linux fork](https://man7.org/linux/man-pages/man2/fork.2.html)、[信号安全约束](https://man7.org/linux/man-pages/man7/signal-safety.7.html)、[进程组信号](https://man7.org/linux/man-pages/man2/kill.2.html)。

Windows 沿用 suspended 创建、私有 KILL_ON_JOB_CLOSE Job 接入后恢复的现有路径。Agent Registry 探测、ZCode 账户、Cursor TUI 额度、Claude 额度/凭据辅助命令、Antigravity 本地元数据查询也接入受管边界；通用 Git 命令与外部终端启动器保持原执行方式。辅助输出同时读取并限制大小，敏感输入仍只经 stdin。

新增进程级回归覆盖宿主直接退出、SIGKILL、无关兄弟进程隔离、CLI 的 wait-all-children 不受 guard 阻塞，以及有界输出超限清理。macOS 和带 init 的 Linux 都已通过；仓库外审计工程复测三种退出方式，原 shell 与 sleep 均消失，无关兄弟保持存活。改后证据 `/tmp/dogcodes-parent-audit-after-macos.log`、`/tmp/dogcodes-parent-audit-after-linux.log`。相关驱动/运行时/Registry/作用域测试 macOS 432 项、Linux 430 项通过（其中各有一个无环境时不执行的 fixture 入口）；两端严格 Clippy 和 Windows GNU all-targets 静态严格检查通过。安装 Agent 和真实账户/钥匙串未运行。 Core 与完整存储回归 macOS/Linux 各 315 项通过，macOS desktop 348 项通过（8 项按需跳过）。证据 `/tmp/dogcodes-parent-core-store-macos.log`、`/tmp/dogcodes-parent-core-store-linux-ci.log`、`/tmp/dogcodes-parent-desktop-macos-ci.log`。验证环境修正：Linux 最初并行链接被系统终止，随后串行运行；精简 ACP 容器没有 Git，恢复测试在具备 Git 的既有 desktop fixture 中通过。desktop 沿用 CI 的 `RUST_MIN_STACK=16777216`，默认测试线程栈不足的首次运行未计为通过。

启动恢复继续使用现有 `turn.connection_lost`：保留历史、到期旧审批、恢复工作区检查点，不重放 prompt；没有把中断任务标为成功。受管普通后代沿用继承的进程组/Job，主动守护化、离组或外部服务工作不据此承诺回收；这不是系统沙箱。Windows 真实 Job 行为、各 Agent 实际工具链和跨平台用户环境仍待实机验收。

## 2026-10-23 第一阶段记录

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


ACP 停止与审批继续补齐：等待原轮次真实 prompt 终态，取消等待和重复停止复用同一次结果，派发前取消不发送；审批/问答核实活动会话及停止状态，独立身份防止连接重建编号复用。11 项新回归含 6 项先复现失败，Mac ACP 89 项/Core 92 项/桌面 320 项及严格检查、Windows GNU all-targets 静态/严格检查通过，Linux ACP 89 项及严格检查于 10 月 25 日补验通过；见 [ACP 停止与审批](acp-stop-approval-2026-10-24.md)。逐 session 关闭、metadata 事务、通知归属、顶层强退与跨物理代次仍未完成，原视频/实机/网络/推送/部署范围继续。


ACP 已继续补齐共享 owner 的本机路由/缓存退休、原 Turn 停止、最后物理退出与独立配置结算：一个会话关闭/停止超时不会杀其它开放 owner，调用方取消不会丢掉原配置/关闭任务，配置失败停用原 handle，迟到旧 interrupt 不改变退休或重新配置后会话的连接标志。Mac/Linux ACP 各 101 项、Mac Core 92 项/桌面 320 项及严格检查，Windows GNU all-targets 静态/严格检查通过；见 [ACP 会话退休](acp-session-retirement-2026-10-25.md)。原生 Agent 自有后台资源通用单会话卸载、辅助事务/超时副作用、无 sessionId 回调/旧别名、Core 强退/跨物理代次及原视频/实机/网络/推送/部署范围继续保留。

## 2026-10-27：有界输出读取的原作用域收尾

核对 Claude 只读额度/凭据与 Antigravity 会话用量的辅助命令，发现共享 `OwnedChild::wait_with_output` 在管道超限/读取错误时直接传播错误，随后只靠 Drop 请求 kill；调用方消失也没有持有原对象的异步收尾。新增私有 shell 回归在旧实现下确认“输出失败已返回，原 leader 尚未回收”，证据 `/tmp/dogcodes-owned-output-before2.log`；不是凭进程名推测残留。

在原方法中将已开始的收集交给 Tokio 监督任务，stdout/stderr 及原 wait 同时有界读取。观察者取消会停止本次读取，继续对同一个 OwnedChild 终止和确认；读取错误也先完成该收尾，才交付原错误，正常状态码/两路输出保持。确认暂时失败时只延迟重查同一组/Job，保留原身份，不重新 spawn 或重放命令。意外监督丢失返回明确的未获证错误，不能作为成功退出；Core 强退仍由已有 Unix 生命周期 guard/Windows Job 承担。未开始轮询的 future 仍走既有 Drop 兜底，不能把它当成已受理并获证的收集。

新增两项普通进程回归覆盖 stdout/stderr 各自超限后返回时 leader 与继承管道后代都已消失，以及取消收集后原树继续退出、无关进程存活。旧有非零退出码、正常等待/幂等、取消终止接续、先退出 leader 与 Drop 兜底均继续验证；macOS 作用域库七项、宿主退出三项已通过。相关库回归如下，两端没有失败；按需真实环境用例未启用：

| 范围 | macOS | 隔离 Linux |
| --- | ---: | ---: |
| Core | 103 | 103 |
| Runtime | 14 | 12 |
| Store 库 | 74 | 74 |
| Antigravity | 45 | 45 |
| Claude | 72 | 72 |
| desktop | 349 | 349 |
| process-scope 库 | 7 | 7 |

两端宿主退出测试目标也通过（两项外部进程测试，另有一个无配置时直接返回的夹具入口）。这不是本轮运行了全部 Store 集成测试，也不计桌面测试递归输出为额外用例。日志 `/tmp/dogcodes-owned-output-macos.log`、`/tmp/dogcodes-owned-output-suite-macos.log`、`/tmp/dogcodes-owned-output-linux.log`；两端上述七包（包含 process-scope）的 all-targets 严格 Clippy、Windows GNU 同七包的静态严格检查通过，分别记录在 `/tmp/dogcodes-owned-output-strict-macos.log`、`/tmp/dogcodes-owned-output-linux.log`、`/tmp/dogcodes-owned-output-windows.log`。没有执行 Windows 程序。

本项只收口共享输出读取的所有权和结果交付，不替代各 Driver 的业务准入或账号/网络语义，也不宣称全部临时辅助事务完成。没有新 API、依赖、存储格式/迁移或任务重试；私有测试不访问用户 CLI、凭据、屏幕或输入，日常 Core 未切换。Windows 真实 Job 与原八包实机/网络/部署验收继续保留。


## 2026-10-27：临时终端不再隔着包装器管理

进一步核对 Cursor 额度 TUI，确认 `script` 会把实际 CLI 放到外层受管组之外。现有 `OwnedChild` 增加直接 PTY 启动方式，CLI 自身成为受管会话/进程组 leader，原宿主 guard 同样覆盖终端中的普通后代；没有把 PTY 挂断当成退出证明。宿主直接退出/SIGKILL 的回归现同时覆盖管道和 PTY，忽略 HUP/TERM 的后代均退出，无关兄弟继续运行。调用方取消时，原查询许可和临时目录也由同一工作者保留到真实退出，详情见 [Cursor 额度终端归属](acp-auxiliary-scope-2026-10-26.md#2026-10-27cursor-临时终端的真实进程归属)。主动再次 setsid/换组逃离的工具仍超出普通进程组保证，真实 CLI/Windows 验收继续保留。
