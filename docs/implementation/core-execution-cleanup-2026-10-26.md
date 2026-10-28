# 执行异常后的原会话清理与收尾

> 后续进展：自动续轮已复用 Execution、原资源退出证明及失败收尾；自动观察者仅在终态持久化后归还会话，Claude ACK 按完整句柄 scope 核实。自动续轮已共用持久化变更检查点；Core 强退等边界仍待，见 [自动续轮](autonomous-lifecycle-2026-10-27.md)。

> 后续进展：项目运行配置分析与提交总结共用临时会话监督，观察超时不丢弃原创建，目录/租约跟随同一原清理；Core 空闲退休纳入临时操作保护，见 [临时会话接续](core-auxiliary-session-2026-10-26.md)。其余辅助事务与整体强退/重启继续。

> 后续进展：替代创建前先持久化原源退休；原生准入前取消/准备失败保留未触及会话及原历史游标，原退出证明在重配置前捕获，见 [会话状态修复](core-session-state-2026-10-26.md)。跨 Core 强退、辅助事务与未知启动继续。

> 后续进展：主任务启动现在由独立工作者持有，迟到身份可接回原清理；原退出证明也可先于关闭 RPC 回复生效，见 [主任务启动接续](core-session-startup-2026-10-26.md)。跨 Core 强退、无身份的未知启动及辅助事务仍待继续。

日期：2026-10-26。工作包 2 的代码与本地验证进展；应用尚未发布，不增加旧版迁移或旧协议兼容。

## 2026-10-27：思考闭合记录不覆盖原终态

已复现普通任务收到原生完成后，保存最后的思考状态失败会从流处理返回错误，并开始关闭已经成功的会话。自动续轮同样会因思考闭合保存失败丢掉原完成；若用量/变更保存先等待，原思考 durationMs 还会把本地等待算进去。

普通任务将仅在终态边界生成的思考闭合加入既有收尾记录，在原生资源退休/移交后复用同一原 run 补写；仍先保存这些状态再保存最终完成。收到终态即进入 Settling，避免后续等待原输出 EOF 时仍向已结束的执行投递审批/停止。非终态的实时输出保持原处理方式。自动续轮在观察到终态时冻结思考状态与耗时，后续保存只补同一元数据；不丢原观察者、不重发任务、不把保存等待算入思考时间。

原有内容策略不变：普通任务保持原先已发布且脱敏的思考文本，原始 delta 字段不进入持久事件；自动续轮仍不持久化收到的 thought delta。没有为此增加新协议、窗口状态、迁移、缓存或恢复队列。

新增一项真实 SQLite/Core 回归，组合普通/自动续轮和思考状态/用量写入故障共四种情况。旧实现下普通思考结束故障确实调用一次不应发生的关闭；修复后保存恢复且完成不变失败，思考状态早于完成、耗时不含补写等待、任务仍只发送一次，自动 delta 仍未保存。Core 崩溃策略仍待用户选择，本节未改整体退出行为。

最终 macOS/Linux 各 Core 120 项、完整存储 195 项通过；macOS 全工作区 all-targets 严格 Clippy、Linux Core/存储/工作区三包严格 Clippy、Windows GNU 同三包 all-targets 严格静态交叉检查及格式/差异检查通过。证据：`/tmp/dogcodes-thought-finalization-before.log`、`/tmp/dogcodes-thought-finalization-after.log`、`/tmp/dogcodes-thought-finalization-macos.log`、`/tmp/dogcodes-thought-finalization-strict-macos.log`、`/tmp/dogcodes-thought-finalization-linux.log`、`/tmp/dogcodes-thought-finalization-windows.log`。只使用私有 SQLite/Driver/CLI/临时 Git；无真实 Agent、模型、桌面输入、手机或日常 Core 切换，原三个开发服务容器不变，未提交推送。

## 2026-10-27：用量补写与执行时间

原实时用量/首字事件保存失败后被忽略，收尾只保存最终摘要，不能补齐已经观察到但丢失的请求事实或首字延迟；最终摘要自身保存失败也会直接丢弃。现在 TurnMeter 仅记住未落盘的请求 ID 与首字状态，沿用已有报告数量上限；收尾把修正后的请求事实、首字延迟、最终用量冻结为现有事件，再复用原 run 的本地补写原语。成功保存且没有修正的实时请求不重复追加；重试不会重新查询会话总量，也不会查询账户额度。

普通任务在原资源退休/移交后补写，避免因用量存储故障拖住原生关闭。自动续轮先冻结当前变更，避免等待统计保存期间的外部修改混入原结果，观察者仍按原责任接收后续根轮次。终态保留已经冻结的 completedAt/failedAt 作为事件 occurred_at，recorded_at 仍表示真正落盘时间；本轮执行耗时不因本地补写等待而增加。自动续轮在收尾一开始冻结终态时间，不在等待后重新生成。

新增实际 SQLite/Core 故障回归分别拒绝请求事实、首字事件和用量摘要，旧实现下明确因提前完成而失败。恢复后验证原请求/模型/成本/生成速度、首字值、修正事实及最终摘要均先于完成保存；无重复计数，原生会话用量只读前后两次，任务只发送一次，投影重建后结果一致。扩展自动续轮用量回归，在中间轮次注入首字保存故障，恢复后第三轮继续，统计与执行时间仍各属原轮次。

最终 macOS 与 Linux 各 Core 120 项、完整存储 194 项通过；macOS 全工作区 all-targets 严格 Clippy、Linux Core/存储/工作区三包严格 Clippy，以及 Windows GNU 同三包 all-targets 严格静态交叉检查通过，格式/差异检查通过。严格检查指出新增测试的一个冗余闭包，按建议改为方法引用后七项统计回归和严格检查再次通过，没有放宽断言或关闭 lint。证据：`/tmp/dogcodes-usage-persistence-before.log`、`/tmp/dogcodes-usage-final-macos.log`、`/tmp/dogcodes-usage-linux.log`、`/tmp/dogcodes-usage-final-style-tests.log`、`/tmp/dogcodes-usage-strict2-macos.log`、`/tmp/dogcodes-usage-windows2.log`。仅运行私有 SQLite/Driver/CLI/临时 Git 夹具，无真实 Agent、模型或屏幕输入；既有三个开发容器保持运行，没有日常 Core 切换、手机设备启动或 Git 提交推送。

本节覆盖已经结束且进入正常收尾的轮次；完整运行时被杀、事件接收/工作者异常前仅留在内存中的事实，以及其它思考/工具/Fork 写入，不能由本节外推完成。未增加协议、存储格式、迁移或恢复队列。

## 2026-10-27：保留原终态与已采集变更

普通任务收到完成后，原代码把终态保存错误返回为执行错误，随后写 `turn.failed`；已采集的文件变更保存失败则丢弃原值，只尝试写失败诊断。修复后普通任务及自动续轮都保留原终态和冻结的工作区结果，复用原 run 身份检查与版本 CAS，每秒只补写本地记录。变更先于终态保存，Driver 来源和原失败原因保留，不重发 prompt，也不重新采集补写期间发生的外部编辑。

普通任务在等待这些写入前完成原生退休或后台/空闲所有权移交；自动续轮的观察者自己持有未来根轮次，继续保留原资源责任，直到终态保存并能继续消费。真正的流/工作者异常仍走原退出确认。工作区结果超出既有 artifact 上限或无效时保存对应诊断，不把永久的数据约束当成临时存储错误无限重试；原任务消失/换成后继运行时不再补写原变更。

Control 增加仅限本地的 Settling 阶段，仍持有运行与工作区保护，但不向已结束的原生工作发送停止或审批。它不改变客户端协议，也不把尚未落盘的结果提前标为完成。审批回归夹具改用真实 own_run 控制记录，并补收尾阶段拒绝交互；原来关于轮次替换、未知 owner 和审批 ID 替换的断言保留。

原 SQLite 完成故障回归在旧代码下明确失败，修复后覆盖：存储恢复后仍为完成、迟到停止不触及空闲会话、Driver 来源保留且只发送过一次任务。自动续轮对应回归确认完成不变失败、同一观察者继续接收后续根轮次。另新增两个实际 SQLite/Core 用例，验证冻结变更不混入后来的外部编辑，以及过大变更记录诊断后正常完成；原真实 Git/异常退出/准备取消回归继续保留。

最终 macOS Core 120 项、全部存储 193 项通过，macOS 全工作区 all-targets 严格 Clippy 通过；Linux Core/异常清理/准备取消/运行审批/安装检查点/审查/多目录/自动续轮及两个进程中断恢复相关 205 项通过，Core/存储/工作区三包严格检查通过；Windows GNU 同三包 all-targets 严格静态交叉检查通过，未运行 Windows 二进制。格式与差异检查通过。证据：`/tmp/dogcodes-completion-before.log`、`/tmp/dogcodes-completion-final-macos.log`、`/tmp/dogcodes-completion-strict-macos.log`、`/tmp/dogcodes-completion-linux.log`、`/tmp/dogcodes-completion-windows.log`。全部使用私有内存/SQLite/CLI/临时 Git 夹具，没有调用实际 Agent/模型、操作桌面/手机或切换日常 Core；原三个开发容器保持运行，未提交推送。

请求级用量与首字状态的后续补齐见上节；思考/子工具片段、Fork/快照等其它辅助写入仍按原范围核对。跨 Core 强退时的原生资源处置和实机验证也不由本地补写回归证明。

## 2026-10-27：异常收尾的存储故障

继续核对启动恢复时确认：异常工作者的原资源虽然已退出，随后恢复工作区或写最终失败的错误仍被忽略，Execution 会直接 finish/drop，造成数据库保留活动运行、Core 却失去收尾 owner；可能一直显示未结束，直到重启恢复。

现有独立收尾任务在同一 owner 内分两步完成本地记录：先恢复/保存该轮工作区变更，再保存原运行失败。任一步失败保留原运行及工作区保护，每秒观察重试；已经完成的工作区步骤不会因终态写入失败而重新执行。原生清理仍在重试循环外，只等待同一原关闭/退出证明，不重发任务、关闭或中断命令，也不重新解析当前注册 Driver。原任务被删除或被后来运行替代时，原 run 身份检查使迟到收尾退出，不修改后继运行。

工作区恢复的 repository 查询错误现在传播到收尾任务，不再当作“没有工作区”；scope 使用已核实原 run 的同一份 task execution state。没有基线的正常诊断仍可持久化并收尾，未增加迁移、协议或通用恢复队列。

新增 Core 故障回归分别拒绝工作区诊断写入与终态写入，验证故障期间 owner/保护不丢失、恢复后按顺序且仅记录一次，替换 Driver 不被关闭。旧代码在首种故障下明确失败，修复后两种通过。另新增实际 SQLite trigger 回归：原关闭确认后持续拒绝变更 artifact，观察到再次恢复但无终态；移除触发器后无需重启便补完变更和失败，原关闭仍只有一次。

最终 macOS Core 120 项、全部存储 191 项通过；Linux Core/准备取消/异常清理/安装检查点/审查及两个实际进程中断恢复相关 185 项通过。两平台 Core/存储/工作区 all-targets 严格 Clippy 及 Windows GNU 同三包严格静态交叉检查通过。格式/差异检查通过。证据：`/tmp/dogcodes-finalization-before.log`、`/tmp/dogcodes-finalization-after.log`、`/tmp/dogcodes-finalization-macos.log`、`/tmp/dogcodes-finalization-strict-macos.log`、`/tmp/dogcodes-finalization-linux.log`、`/tmp/dogcodes-finalization-windows.log`。仅运行私有内存/SQLite/CLI 与临时 Git 夹具；没有运行真实 Agent/模型、桌面输入、手机设备或切换日常 Core；原三个开发容器未改动，未提交推送。

范围仍是现有 Core 生命周期内的异常 Execution 收尾。普通轮次终态与变更的后续补齐见上节；其余业务操作、整进程强退后的原生资源及真实系统验收继续按 CURRENT_WORK，不能由本回归外推整体完成。

## 问题

原生任务已经启动后，`session.ready`、`history.native_prompt_accepted` 或回答事件的存储失败都可能提前结束 Core 流程，留下仍在运行的原生工作。另有两个相关问题：支持会话常驻时，失败或缺少终态的回答流仍可能跳过关闭；原生关闭失败/超时后再调用一次 shutdown，不能保证它只是观察旧请求，尤其不能把“本机记录已移除”当成原物理退出。

现在原运行保留 Driver、实际 SessionHandle、停止控制和工作区保护；清理与观察调用方分离，使用同一份操作状态。

## 实现

- `run_cleanup::Cleanup` 只发起一次原 Driver.shutdown，独立任务继续驱动同一个未来。10 秒（分析为 15 秒）只是报告“尚待确认”的观察预算，不丢弃未来、不重发关闭。成功、失败、调用方退出都不改指向当前注册的新 Driver 或同名新会话。
- Driver 可在关闭前提供 `SessionShutdownProof`。这是捕获原资源的被动证明，不能发送请求、重新按 session ID 查找当前会话或创建新会话。ACP 从原 Resident 捕获关闭回执/物理退出代次：关闭返回暂时错误后，仍能等同一回执的迟到成功或原连接真正退出。没有被动证明且关闭失败时保持未知，不伪造成功；原 Driver/句柄继续保留。
- 前台 SubmitTurn 的 `Execution` 在工作者 panic 或未来被丢弃时，把原 RunOwnership 转交给独立清理任务。已经开始的清理被加入观察，不另建关闭请求。正常工作和后台/闲置所有权移交已完成时，迟到记账错误不再关闭已移交的会话。
- 准备阶段已经返回的错误由 Adapter 按原启动合同保留/清理其资源，Core 不猜测一个新句柄；但正在启动的未来被丢弃、尚无句柄时，不能据此证明结束，保留待核实状态。这一类启动仍需继续完善。
- 存储或模型执行异常时，Core 在原资源确认结束后才记录最终失败。失败、额度耗尽、中止或没有终态的流，即使支持常驻复用也显式退休；正常成功并移交所有权的会话继续复用。原生退休/移交在最终终态写入前完成，避免用户开启下一轮后旧关闭才到达。
- 清理未确认时保持原运行及工作区读保护，沿用“正在停止”状态，不允许对同一会话重新发送新任务。其他运行仍可继续。状态/失败事件按原 Run ID 检查并以任务流版本 CAS 写入，迟到观察不能停止或标错后来运行。
- 关闭确认后，原活动 session map 按原 Driver 恢复能力转为 resumable/detached；不会留作虚假的可直接复用会话。存储故障消失后，后续用户发送能在替换的 Driver 上继续，不重放前项任务。
- 意外结束的工作者在原资源退休后，用原轮次的持久化检查点恢复变更记录，再写最终失败。与启动恢复共用检查点读取、SHA-256 验证及恢复逻辑。没有检查点则记录不可用诊断，不伪造可恢复内容；不自动修改项目文件或回滚代码。
- 项目运行配置分析复用同一清理操作。取消时只尝试一次 interrupt，等待它的五秒预算不丢弃该任务；原关闭获证后才停止仍未结束的中断观察者并释放临时目录/安装退休 lease。关闭未知时报告待确认并保留这些资产，不循环发送 shutdown。

`Control` 的 `native_owned` 标记表示这份原运行仍负有清理责任；显式完成/移交后清除，不能用它冒充物理进程退出证明。未声明原生 session/close 的第三方 Agent 仍遵循已记录的本机退休/最后 owner 物理退出合同，本阶段不宣称它的全部内部会话缓存均被释放。

## 回归与证据

新增四项 Core 测试：观察预算到期及观察者丢弃不取消/重复原关闭；关闭失败后只等已捕获的结束证明；缺少证明不把失败变成功；清理等待者被 abort 后仍保留原 Run/工作区保护，替换注册 Driver 或出现后续运行也不会被原清理波及。

新增五项实际 SqliteStore/Core 集成测试，包含三个故障点（会话登记、prompt 接受记账、回答写入）、工作者 panic、失败/缺终态流，以及移交后的终态存储失败。SQLite trigger 只作用于临时数据库；故障恢复后实际再次发送，验证能继续且不会关闭替换 Driver。持久化检查点回归让私有夹具在项目临时目录写入文件再 panic，确认原资源退出前不恢复/不释放工作区，退出后先补变更再记录失败。

ACP 新增被动证明测试：观察不发送 close；明确错误后不会提前完成，也不重发；最后 peer 的原物理退出能够完成已捕获的原证明。准备取消、运行配置分析及 desktop 的实际关闭错误夹具改为由独立退出信号提供证明，并断言仅关闭一次，未放宽其目录/退休 lease/批处理停止断言。

- macOS：Core **98 项**、ACP **136 项**、Driver API **29 项**、存储 **159 项**、desktop **347 项**通过；ACP 21 项及 desktop 八项按需单元测试未启用。全工作区 all-targets 严格 Clippy 通过。
- Linux：Core **98 项**、ACP **136 项**、Driver API **29 项**，以及准备取消/异常清理/历史 MCP/运行权限/任务生命周期的 **28 项集成测试**通过；四包 all-targets 严格 Clippy 通过。
- Windows GNU：Core/存储/ACP/Driver API 四包 all-targets 严格交叉检查通过，没有执行 Windows 二进制。现有真实临时 Git 仓库检查点回归另通过一项，确认 capture 私有对象清理后仍能由持久化检查点恢复。

Linux 最终复核发现私有夹具的 PID 文件在写完之前已被读者看见，现改为关闭文件后原子发布；没有放宽进程退出断言。修复后完整 ACP 复核及 macOS ACP/严格检查通过。

最终 macOS 日志 `/tmp/dogcodes-run-cleanup-final-macos.log`，夹具修复复核 `/tmp/dogcodes-run-cleanup-acp-final-macos.log`；Linux `/tmp/dogcodes-run-cleanup-final-linux.log`；Windows `/tmp/dogcodes-run-cleanup-final-windows.log`；现有 Git 自包含检查点复核 `/tmp/dogcodes-run-cleanup-checkpoint.log`。更早日志中故意的 panic 是私有故障夹具；最终退出码及 test result 用于判断通过，不能把该行当作真实 Agent 故障。

没有调用安装的 Agent/模型、用户桌面/输入/剪贴板、手机模拟器或日常 Core 切换。只使用私有内存/SQLite/Python ACP 夹具和临时文件；现有服务/数据库开发容器保持运行。未提交或推送 Git。

## 仍须继续

其它 Driver 的 scope、被动退出证明及 shutdown 真实确认合同仍须逐个接入/验证。没有证明的失败会停留待确认，不以循环重试或强杀共享 peer 换取表面完成；管理与强制恢复、原生启动中断、自动续轮和其余临时事务需继续统一。

持久化失败的完整重试/退出记账、Core 运行时已经停止/进程被强杀时的 OS 行为、真实 Provider/Windows/设备网络验证仍未收口。项目分析的原生启动预算与晚到资源也继续核对，不能因关闭逻辑改好就认定启动全部解决。

视频实机恢复、HDR/光标/GPU 路径、PC/手机功能对齐、推送和部署验收仍按 CURRENT_WORK 的八包范围推进，不以本轮关闭该整体目标。

## 2026-10-27：用量读取固定原运行

确认原生任务仍使用接受时的 Driver，但本轮开始/结束的会话用量通过 ExecutorRef 重新查询当前注册 Provider。运行期间重新检测/替换注册项后，结束边界可能读取另一实例的数据，继而将错误用量持久化；运行中会话的额度/上下文查询也有同类问题。

- SubmitTurn 在现有接受锁内同时捕获当前用量 Provider，随原 Run Control 保留，开始/结束读取不再查注册表。接受时没有 Provider 就保持未知，不从后来注册项补造原轮次用量。Provider 登记使用同一接受锁，不改变已经接受的 Run。
- 原生自动续轮的观察者保留原会话 Provider，并传给每个原续轮的 Control；续轮的最终统计仍只来自自己的原生请求事件，不用会话累计快照推算本轮。
- 带 taskId 的运行中额度/上下文查询使用该 Run 的原 Provider；全局账户查询和后续新任务使用当前注册项。Driver 与 Provider 仍可独立注册、独立替换，本地日志读取等不被误清除。
- 沿用原两秒用量读取预算和冻结结果后的持久化补写。没有增加持久格式、恢复队列、协议、迁移或自动重发任务，也不修改各 Agent 的账户获取方式。

新增 SQLite/Core 集成覆盖原基线读取期间替换同 ExecutorRef 的 Driver/Provider、全局与运行中会话查询、原生自动续轮、随后新任务和完整投影重建。初始实现实际只对原 Provider 读取一次，新增断言失败；修复后原轮次使用原模型/用量，自动续轮保留自己的请求事实，新轮次采用替代 Provider。全量回归还保留本地消息日志在 Driver 更换后继续补充原生调试用量的既有行为。


最终验证：macOS/Linux 各 Core 120 项、完整存储 196 项（含集成）通过；两端 desktop 各 349 项通过，分别 8/11 项按需用例未启用。两端 Core/存储/desktop all-targets 严格 Clippy 和 Windows GNU Core/存储 all-targets 严格静态检查通过，格式/空白检查通过。日志 `/tmp/dogcodes-run-usage-before.log`、`/tmp/dogcodes-run-usage-auto2.log`、`/tmp/dogcodes-run-usage-verified-{macos,desktop-macos,strict-macos}.log`、`/tmp/dogcodes-run-usage-current-{linux,windows}.log`。

桌面测试沿用 CI 的 `RUST_MIN_STACK=16777216`；首次未带该配置出现栈溢出。一次 macOS 私有 Antigravity 启动超过原八秒观察预算；未放宽超时或退出断言，单项复核与随后完整回归通过。Windows 只做静态检查；未执行真实 Agent、账户查询、用户桌面/输入/剪贴板、设备或日常 Core，原三个服务容器保持运行，未提交推送。其余无身份启动、未标识回调和原八包实机/凭据边界继续保留。

## 2026-10-27：子 Agent 历史定位原执行实例

子 Agent 卡片的历史查询原本只按 provider 名称筛选当前驱动：同类 Agent 安装多个实例就被判为不明确，替换 Driver 后也会查询新实例；当原安装不可用但只剩另一个同名安装时，甚至会把后者当成历史来源。

现在从该时间线条目的原 turnId 查找已持久化的 `turn.queued` 或 `turn.autonomous.started`，取得原 ExecutorRef 和 RunId，并复核条目 provider 一致。元数据按既有有界分页读取，固定在该条目的序号之前；不使用后来切换的首选 Agent，也不猜测唯一同名安装。原 Run 仍被 Core 持有时使用它的原 Driver；已经结束的历史由该安装当前的只读 Reader 获取。缺少原始身份或相应安装不可用时明确报错，不改投其它安装。

继续核对原任务/条目、subagent 类型、原 childId 和允许读取历史的能力标记，返回内容仍经既有脱敏。PC 与手机复用原 `subagent_history`，没有改动协议、存储格式、数据迁移或模型任务派发。

新增私有 SQLite/内存 Driver 集成：多安装、运行期间 Driver 替换、改变首选安装、普通/原生自动续轮、历史读取、重建投影及新 Core 重开；确认只有无关安装可用时零误读。旧实现实际在多安装断言得到 `subagent installation unavailable or ambiguous`；修复后的专项通过。首个夹具因保留输入 Sender 而未结束输出流，已按真实终态后 EOF 约定修正，没有放宽观察期限。


最终 macOS/Linux 各 Core 120 项、完整存储 197 项（含新增集成）、desktop 349 项通过；desktop 分别 8/11 项按需测试未启用。两端三包 all-targets 严格 Clippy、Windows GNU Core/存储 all-targets 严格静态检查、格式与差异检查通过。本轮首次 macOS 全量遇到既有私有 Antigravity 夹具启动观察超时，随后完整复核通过，未改动其八秒期限；新测试的文档格式 lint 已按规则修正。没有执行 Windows 二进制、真实 Agent/账户、用户桌面/输入/剪贴板、设备或日常 Core。原三个开发服务容器保持，未提交推送。

证据日志：旧实现 `/tmp/dogcodes-subagent-history-before.log`；专项 `/tmp/dogcodes-subagent-history-after2.log`；最终 macOS `/tmp/dogcodes-subagent-history-final-{macos,desktop-macos,strict-macos}.log`；Linux 测试 `/tmp/dogcodes-subagent-history-linux.log` 与最终严格检查 `/tmp/dogcodes-subagent-history-final-strict-linux.log`；Windows `/tmp/dogcodes-subagent-history-final-windows.log`。手机产品代码和传输没有改动，未重跑无改动的手机/服务端套件。其余无身份启动、未标识回调、视频与既定实机/凭据工作继续。
