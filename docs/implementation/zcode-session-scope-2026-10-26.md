# ZCode 共享进程中的会话归属、配置事务与退出证明

日期：2026-10-26；工作分支 `remote-control`。本阶段使用当前运行期格式，没有旧句柄兼容、历史数据迁移或旧版已保存模型选项修补。原生 CLI 版本的能力验证仍保留，它与应用旧数据兼容是两件事。

## 归属与共享进程

ZCode 的一个私有 app-server 可以服务多个逻辑会话，因此不能把关闭一个会话直接等同于杀死整个进程。创建、冷恢复与成功热复用现在均返回新的 scoped handle；持久化的 native session ID 只用于发现对象，不能单独授权 prompt、审批、回答、取消或关闭。

每个逻辑会话有独立准入门，按 native ID 的恢复门防止并发冷恢复重复安装。热复用在生命周期锁内检查空闲及退出状态，再交接新 scope；忙碌时拒绝恢复，保留正在执行的任务。排队 prompt 在拿到准入和原共享连接后再次检查归属，旧句柄和旧已捕获对象不能关闭或回答恢复后的会话。

会话表改为只保存短操作的同步锁，不在它里面执行原生请求或等待退出。这样收到新会话 ID 后，可以在下一次 await 前完成归属预留，避免调用者取消订阅安装时留下无法发现的候选对象。重复 native ID 拒绝安装，不覆盖或关闭原所有者。创建与恢复都核对原生身份、工作区和空闲状态；其他工作区或仍在执行的快照不授予配置、订阅及关闭权限。

## 原会话与原进程证明

`shutdown_proof` 捕获原 `SessionState` 及原 `NativeProcess`，被动等待原逻辑会话退休或原 OS 进程退出的正面事实；不按字符串重新找对象，不发 RPC，也不因为会话表缺失、关闭错误或收到 `closed:false` 就完成。即使 Driver 退出后没有另外发送逻辑关闭 RPC，原物理进程确实退出也能满足已捕获的证明。恢复安装仍须等待原表项预留退休，不能只凭 OS 退出绕过它。逻辑关闭仍复用原有唯一关闭请求及 `session/read` 缺失证明；没有明确回执时不重发同一未知 mutation。已经确认的退休回执必须匹配原 scope，表中同名替换不会被旧清理移除。

原共享进程已经退出或被替换时，关闭原逻辑会话等待原 `NativeProcess` 的退出结果，不能停止后来进程。正面信号在原对象清理与表项退休后发布；临时错误、监督者丢失仍属于未确认。一个会话成功退休后，其他会话继续使用相同后台进程。

已获得身份的启动、订阅或配置在取消时通过独立任务退休原逻辑会话，保留原共享连接和对象，不依赖调用者继续 await。恢复看到已有退休过程时，等待原对象正面退出后再尝试重建，不借新 scope 绕过原清理责任。

原生明确返回 session/resume 的 session-not-found 回执时，保留共享进程中的其他会话；未知/未确认失败不借用这一例外。如果启动还没有拿到 native session ID 就被放弃，只能停止原私有 OS 进程组/Windows Job，不能猜测 ID 关闭其中一项。这可能使同一后台进程中其他空闲会话需要恢复；已经识别的候选不会走此路径。原生 turn 仍由共享连接串行处理，未知启动时没有同时持有该连接执行的另一轮 foreground turn。独立 OS supervisor 保留原进程直到退出获证。

## 模型与思考强度

选择模型和思考强度在同一次会话准入内执行：先读取并核对原生身份/工作区/空闲状态，应用选择，再读回验证。只有验证成功才交接新 scope。

思考强度依原生设置分为模型引用中的 reasoningLevel 和独立 thoughtLevel。独立 API 未启用时，不再额外调用它；只更新当前原生目录提供的模型选项。不补造旧版应用保存过的缺失选项。模型与思考请求都带 `persistAsWorkspaceLastUsed:false`，避免一次任务选择改变工作区默认值。

部分成功后发生拒绝时，恢复原模型与原思考强度并读回确认；获证回滚返回 `selection_rejected`，保留原句柄。无法确认的配置或回滚会退休原逻辑会话，退出证明成立后才把启动/恢复错误返回 Core。新建会话尚未交付句柄时，即使回滚成功，也先清理候选后报告失败。

## 回调、审批与轮次

审批 UI 使用新的随机请求 ID，原生 JSON-RPC 请求 ID 单独保存用于回复。即使原生连续两轮重复使用 permission ID，上一轮的 UI 答案也不能消费新请求。问答和审批在同一生命周期锁内检查 scope、活动状态和取消状态，再发布/消费请求；排队答案在派发前重新检查原 turn 是否仍接受操作。

具有明确 sessionId/turnId 的其他会话或轮次事件不会成为当前正文、审批或终态；包裹层与 payload 中的标签都要一致。已经见过的原生 turn ID 由原共享 Connection 持有，因此同一进程的热复用及冷恢复都不能把已知旧 turn.started 误认成新回答。重建物理连接时不沿用其运行期标签集合，避免把另一个进程的 ID 计数当作当前证明。

本地 worker 在结束失败轮次时先在生命周期锁里禁止复用，再释放活动状态；结束动作也核对原句柄，不能清掉新所有者的审批/取消槽。

## 验证

新增十九项 Driver 回归，全部使用私有 Python 协议夹具，覆盖旧/裸/已捕获句柄、准入后重检、同名冷替换、共享进程隔离、忙碌拒绝、并发恢复、重复 ID、启动取消、配置事务及回滚、原生 reasoning API 区分、明确关闭拒绝、重复权限 ID 和明确标识的旧轮次。

真实 Core、SQLite 和私有共享 CLI 的四轮集成验证首次创建、两次热交接、审批、问答与取消；原生 server request ID 故意跨轮次重复，旧 scope 拒绝不影响当前操作。已有按需真实 CLI 测试改用实际返回的 scoped handle，没有为旧裸句柄增加兼容分支。

macOS/Linux Driver 各 **57 项**通过（分别 12/11 项按需真实 CLI 测试未启用），新增四轮 Core 集成两端通过。macOS 现有 Core **98 项**、desktop **347 项**和全部存储 **163 项**通过；Linux Core **98 项**与相关存储集成 **30 项**通过。最后补充已确认缺失会话及原进程退出证明后，两端重新验收 **57 项** Driver 回归，以及 Core 关闭/准备取消与新增 ZCode 集成共 **14 项**，均通过。

macOS 全工作区、Linux 四包和 Windows GNU 四包 all-targets 严格检查通过，格式/差异检查通过。Windows 为交叉静态检查，没有运行 Windows 二进制。

最终补充验收日志：`/tmp/dogcodes-zcode-scope-accepted-v3-macos.log`、`/tmp/dogcodes-zcode-scope-clippy-accepted-v3-macos.log`、`/tmp/dogcodes-zcode-scope-accepted-v3-linux.log`、`/tmp/dogcodes-zcode-scope-accepted-v3-windows.log`。此前完整回归为 `/tmp/dogcodes-zcode-scope-final-macos.log`、`/tmp/dogcodes-zcode-scope-clippy-final-macos.log` 和 `/tmp/dogcodes-zcode-scope-final-linux.log`。

未运行已安装的 Agent、真实模型、屏幕/键鼠/剪贴板、模拟器或日常 Core；现有服务和数据库容器保持运行。未提交或推送。

## 仍须继续

全部没有原生轮次标签的迟到事件、此前从未见过的旧 turn.started，以及未知/自动续轮不能仅凭到达时间归属。本阶段只过滤可证的明确标签，不声称已解决所有原生流归属。Actor/外部脱离进程、Core 强退/重启与真实各平台 CLI 仍须验收。

启动前尚无句柄的取消虽然已有独立原进程清理，Core 仍需要接收原启动尝试晚到的身份与退出回执，不能把本阶段 Driver 清理等同于该接续合同已经完成。原生明确拒绝关闭时不会伪造成功；当前退休过程需保留待确认责任，后续需结合用户重试/监督恢复完成操作体验。

原生模型目录/额度/账号等其余辅助事务仍需统一核对。远控视频实机/弱网/HDR/光标/GPU、手机与 PC 日常功能对齐、推送与部署继续按 `CURRENT_WORK.md` 八个工作包推进，不以本阶段替代整体目标。


## 2026-10-27：模型目录与账号辅助进程

旧模型目录读取调用 `ensure_connection`，与运行中的 `run_turn` 持有同一连接锁；任务未结束时，打开/刷新模型菜单就可能等待到 Core 的 30 秒观察超时。目录能力探测还可能创建 deferred 会话，调用方退出后请求和迟到回执仍留在业务连接里。私有协议回归已在旧实现复现“model menu waited for the active task's connection”，先清理原任务再做断言；最初夹具缺少 workspace 的失败另行修正，不当作产品反例。

现在 `model_catalog_in_workspace` 使用同一安装的独立临时 app-server，不访问业务连接或会话表。每个 Driver 一项探测；排队取消不启动新进程，已启动探测通过原 watch 归属在观察者退出后关闭自己的进程。读取、deferred 会话关闭、错误或 panic 后都等待原 OS 作用域获证退出，再释放下一项准入并返回目录。沿用原账号 overlay 确认和原生目录能力协商，没有模型 prompt、MCP 或持久会话映射，也不增加协议/版本迁移。业务连接的账号配置不会被目录读取顺带改写。

账号 helper 的 stdin/输出及退出也由独立工作者持有。自身超时、调用方放弃及所属探测/Driver 结束会停止只属于它的读取，随后继续原进程清理，退出获证之前不返回账号数据或放行同一目录探测。探测在账号读取阶段被取消时，不再继续发 session/create；业务连接仍使用自己的授权和请求。没有重发任务、重复原生 mutation 或用同名 sessionId 寻找替代进程。

新增八项普通回归使用固定脚本、私有工作区及真实 stdio/进程组：任务流运行时读取目录且原任务不被中断、deferred 会话/进程先退出后返回、取消保留清理并让排队查询在获证后开始、排队取消不启动、失败后下次查询可用、分别确认账号 overlay 且业务连接请求序号不变、账号 helper 超时先清理、账号阶段取消时两个原作用域都先结束且不创建新会话。记录原 leader/普通子进程 PID，并在第二个 helper 实际启动时检查旧作用域均已退出。

最终 macOS/Linux ZCode 各 65 项、desktop 各 349 项通过，原 Core/SQLite/共享 ZCode 四轮归属集成两端各 1 项通过；ZCode 分别 12/11 项、desktop 8/11 项真实环境按需测试未启用。两端三包 all-targets 严格检查及 Windows GNU 三包 all-targets 严格静态检查通过；Windows 没有实际运行。格式/差异及 Python 语法检查通过。日志 `/tmp/dogcodes-zcode-metadata-{before2,after,targeted,account,final-macos,final-suite-macos,final-store-macos,final-strict-macos,final-windows,linux-final}.log`。

首轮 Linux desktop 的旧临时更新器夹具在启动成功断言失败，未显示底层错误；已改为固定脚本和私有的带空格符号链接，不再现场生成/覆盖可执行脚本，成功标记与失败退出/敏感输出断言保留，并增加失败结果说明。没有据此断言产品升级有故障，也未修改产品更新逻辑。首轮严格检查发现新夹具 PID 窄化转换及 RPC 方法被误判为扩展名比较，已改为检查转换/精确方法匹配；失败运行不计验收，以 final 日志为准。

没有读取真实 ZCode 账号、运行已安装 Agent/模型、设备或用户桌面/输入，原开发服务保持，未切换日常 Core、提交或推送。真实客户端/账号/平台和原八包其余范围继续；这次只收口已证明的目录辅助路径，不把所有未知启动/未标识回调视作完成。


## 2026-10-27：额度身份与读数 helper 的取消和 EOF

额度入口虽已有独立账号工作者和原树清理，但 Cache 的锁属于调用方：取消身份读取或读数请求后，下一次查询仍可在旧账号 helper 退休期间启动。私有完整 Driver 反例在第二 helper 的启动记录中观察到旧 PID 存活，见 `/tmp/dogcodes-zcode-quota-before.log`。同一反例还确认有效账号输出已写出且 leader 已退出时，普通后代持有 stdout 会阻止 EOF，原实现先等读完再观察 leader，直到十二秒查询期限才清理；四秒观察超时后测试仍等待原树收尾才失败。

现在每个 Cache 保留自己的查询准入，身份与读数两个 helper 共用。排队仍在调用方等待并沿用十二秒预算，取消排队不生成后台启动；拿到的 owned 许可交给现有账号工作者，取消、失败或原树退出尚未获证时持续持有。只用剩余预算读取，过期不再启动。普通会话和模型目录各用原监督路径，不额外被这把额度锁串行化。

账号工作者也改为并行观察原 leader 和有界 stdout。leader 自然退出时先由原 OwnedChild 清理同组普通后代，让输出 EOF 到达；读取错误、超过 1 MiB、非零退出、无效 JSON 或取消仍走同一实际退休流程。共享这段代码的模型/账号授权和 runtime 入口保留原超时、owner 和授权读回合同。没有新增进程管理层、依赖、协议、兼容或迁移，也不重发业务任务。

新增五项私有进程回归：分别在身份/读数阶段取消后下一查询等待原退出；leader 正常退出但后代持有 stdout 时及时返回；排队取消没有迟到启动；缓存命中仍只复核身份、切账号立即换读数、失去登录清空快照；无效/过量输出先退休再成功重试。全部复用固定假 Electron/账号夹具与临时 Resources，没有读取安装的 ZCode 或真实凭据、请求模型/供应商网络，也没有操作日常 Core。专项 `/tmp/dogcodes-zcode-quota-scopes-macos.log` 和严格检查已通过；平台/真实工具链及原八包其余范围保留。


最终 macOS/Linux ZCode 各 **70 项**、desktop 各 **350 项**通过；ZCode macOS/Linux 分别 12/11 项、desktop 分别 8/11 项真实/按需测试未启用。两端两包 all-targets 严格 Clippy 与 Windows GNU 两包 all-targets 静态交叉检查通过；没有运行 Windows 程序。Python 夹具语法、fmt/diff 及 Cargo.lock 与本轮前版本比较通过，原 server/Postgres/Redis 容器保持正常，未提交或推送。日志 `/tmp/dogcodes-zcode-quota-suite-macos.log`、`/tmp/dogcodes-zcode-quota-final-strict-macos.log`、`/tmp/dogcodes-zcode-quota-linux.log`、`/tmp/dogcodes-zcode-quota-windows.log`。其余就绪探测与无身份回调仍逐入口核对，不据这五项回归宣称整个包 2 完成。


## 2026-10-27：就绪探测只使用 deferred 会话

核对静态探测和桌面实际使用的 connect_ready，发现原 session/create 没有指定 persistence，会按普通持久会话创建。即使探测随后关闭临时会话，也不应为一次安装/重连检查留下用户历史。原 close 分支只拒绝显式 false，缺少 closed 字段时会结合空 session/list 接受成功；但列表可省略 deferred 会话，空列表不能证明关闭。

复用已有原生 deferred 合同：探测 create 明确请求 deferred，close 带 expectedPersistence=deferred，要求 closed=true；后续列表也须为合法数组并不再列出原临时会话。正常 connect_ready 继续复用同一原进程，实际用户会话仍按原方式创建；探测失败先等待原 NativeProcess 的关闭结果再返回，不重发任何创建或关闭。取消继续由原 Connection/NativeProcess 监督收尾，未返回 ID 的候选只存在于原 deferred 作用域，不增加新控制层、协议、持久格式或迁移。

新增三项私有回归：成功保留原进程而仅创建/确认关闭 deferred 候选；缺少 close ACK 或后续非法列表都拒绝就绪且先退出原进程；create 已受理但未返回 ID 时取消不会保存探测会话或发送 session/send。固定假原生后端记录真实请求，并按是否指定 deferred 模拟持久化边界。三个改前行为反例均失败，见 `/tmp/dogcodes-zcode-ready-before-final.log`；首轮 fixture 使用 unwrap_err 带来额外 Debug 编译约束，已改 matches，编译失败 `/tmp/dogcodes-zcode-ready-before.log` 不计行为证据。修复专项三项及严格检查通过，日志 `/tmp/dogcodes-zcode-ready-current.log`、`/tmp/dogcodes-zcode-ready-strict-macos.log`。

未运行实际 ZCode、账号、模型或用户数据库；上述证据证明请求和受管边界，原生客户端的实际存储行为仍须实机验证。原八包和其它未标识回调/未知启动范围保持，未操作日常 Core、提交或推送。

本轮还对照当前源代码核对已有就绪监督，未给它们新增管理机制：

| 入口 | 正常结果和取消的归属 |
| --- | --- |
| ACP initialize/session-new | spawn_agent 即接入原生命周期 Process，成功探测先等待原 kill/关闭回执；取消由原 Process Drop 请求同一 actor 退休 |
| Codex initialize/account-read | 原 Connection 生命周期从 initialize 前开始，退出关闭原 writer 并等待 NativeProcess.closed；取消不丢原监督者 |
| Pi get-state/available-models | 临时无会话 Connection，call 的取消 guard 和 Connection Drop 关闭原对象，close 等原监督者结果 |
| Claude initialize | 已复用 NativeProcess 排空输出、保留自然退出，并等待原树确认；见 Claude 实施记录的就绪章节 |
| Antigravity init | 原 readiness worker 持 auth 排他至原作用域退出，取消不把认证锁提前交给下一次启动 |
| ZCode list/create/close | 本轮接入上述 deferred/明确关闭；失败等待原进程退出，成功热复用保留同一 Connection |

这些入口没有发送模型任务；这份核对不扩展为所有第三方协议事件都可归属，也不代替真实 CLI/Windows 验收。


最终 macOS/Linux ZCode 各 **73 项**、desktop 各 **350 项**通过；ZCode macOS/Linux 分别 12/11 项、desktop 分别 8/11 项真实/按需测试未启用。上述 ACP、Pi、Codex、Antigravity 四项既有取消清理用例，两端各四项均通过，未重跑无关的全部驱动/视频测试。两端 ZCode/desktop all-targets 严格 Clippy、Windows GNU 两包 all-targets 静态交叉检查通过，Windows 未执行二进制。Python 语法、fmt/diff 与 Cargo.lock 本轮前版本比较通过，原三项开发服务正常。日志 `/tmp/dogcodes-zcode-ready-suite-macos.log`、`/tmp/dogcodes-zcode-ready-final-strict-macos.log`、`/tmp/dogcodes-zcode-ready-linux.log`、`/tmp/dogcodes-zcode-ready-windows.log`、`/tmp/dogcodes-readiness-audit-macos.log`。本轮未提交推送。

其余临时事务不按“就绪探测”笼统重复计数；下一入口是 ZCode 运行期 provider_auth/Account.runtime 的 owner 传递（额度和配置读取已经接入，runtime 仍须单独核对），需要私有凭据回调夹具验证后才能确认其取消合同。未标识的原生回调、无身份启动和真实平台/网络/部署仍见原八包。


## 2026-10-27：运行期凭据回调跟随原进程关闭

核对发现 prompt 的显式取消分支已经会放弃原授权等待，但运行期 Account.runtime 没有观察所属 NativeProcess。原进程主动关闭、Driver/查询所有者消失或实际退出时，凭据 helper 仍可等到一百五十秒上限；即使 Connection 已关闭，再调用内部凭据回调仍会启动 helper。私有反例 `/tmp/dogcodes-zcode-runtime-auth-before.log` 确认“原关闭后 helper 仍等待”和“已关闭连接返回 headersApplied=true”，正常授权对照原本通过。这不表示真实账号被读取或凭据已写入新连接。

现复用 NativeProcess 原关闭 watch：凭据配置和运行期回调都将它交给已有账号工作者，主动 close 与外层 owner 结束沿用原信号；监督者在 EOF/原 leader 退出时也发布同一关闭信号。发布方用原 NativeProcess 的 Weak 引用，不因自持 sender 改变最后所有者消失时的清理行为。已经关闭时不启动 helper，读取中关闭时停止原请求并等待其 OwnedChild/普通后代退出；结果发布前再检查原 watch，不能把已退休来源作为有效授权。原连接的发送 fence 仍保留。

没有新增资源管理器、全局锁或协议字段；额度 helper 仍按原 Cache 独立作用域读取，运行期一百五十秒和配置读取二十二秒上限保持。正常授权保留原进程，不改变实际账号/套餐验证、captcha 逻辑、令牌只经 stdin 的规则或显式任务取消分支。

新增三项私有回归：原 close、owner=true、owner sender 消失和原 leader 被动退出四种情形都结束同一 runtime helper，返回失败前普通后代已退出；正常成功只结束 helper，原 Connection 仍可用；已关闭原进程不启动或授权新的 helper。夹具复用固定假 Electron，只在临时目录模拟阻塞和私有值，没有执行真实 JS 账号逻辑、打开浏览器、访问账号或供应商网络。旧阻塞负例先放行假 helper 并确认退出才失败，不遗留进程。

本地专项三项通过，日志 `/tmp/dogcodes-zcode-runtime-auth-current.log`；首次严格检查只指出测试变量 owner/owned 过于相似，已改名且未放宽 lint，失败记录 `/tmp/dogcodes-zcode-runtime-auth-strict-macos.log`。实际 Start Plan/浏览器回调及各平台验收仍待；原八包其余工作继续，未操作日常 Core、提交或推送。


最终 macOS/Linux ZCode 各 **76 项**、desktop 各 **350 项**通过；ZCode macOS/Linux 分别 12/11 项、desktop 分别 8/11 项真实/按需测试未启用。两端两包 all-targets 严格 Clippy 与 Windows GNU 两包 all-targets 静态交叉检查通过，Windows 未执行程序。Python 语法、fmt/diff 与 Cargo.lock 本轮前版本比较通过，原开发服务正常。日志 `/tmp/dogcodes-zcode-runtime-auth-suite-macos.log`、`/tmp/dogcodes-zcode-runtime-auth-final-strict-macos.log`、`/tmp/dogcodes-zcode-runtime-auth-linux.log`、`/tmp/dogcodes-zcode-runtime-auth-windows.log`。没有改动手机、服务端、普通授权规则或视频协议，未提交推送；这不替代原八包中的未知启动、未标识回调及实机/网络/部署验收。
