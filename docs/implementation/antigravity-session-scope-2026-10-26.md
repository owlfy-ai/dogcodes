# Antigravity 会话归属、审批轮次和退出证明

日期：2026-10-26；工作分支 `remote-control`。软件尚未发布，只使用当前运行期格式；没有旧句柄兼容、历史数据迁移或旧配置回退。

## 结果

创建、冷恢复和成功热复用均返回新的运行期 scope。临时本地进程 ID 与原生 conversation ID 是同一次当前运行中的两个标识：首次 CLI init 后，Core 持久化原生 ID，但正在执行的 Run 保留最初获得的实际句柄；下一次热复用在同一生命周期锁内更新 scope 并把表项改为原生 ID。这是原生协议的身份交接，不是旧版兼容。

原生 ID 可以用于只读发现当前句柄；开始任务、审批、回答问题、取消和关闭均要求准确的 scope。等待输入准入后再检查归属，旧句柄、只有字符串的裸句柄或之前捕获的空闲退休对象均不能操作新所有者。表项移除也核对进程实例；已经获证的内存退出回执仍须匹配原 scope。

恢复和重配置采用单独的安装级准入门，输入采用进程级准入门；不会持有全局会话表等待进程退出或原生验证。忙碌会话拒绝配置，保留当前所有者。热复用核对工作区说明、权限和已选模型；需要冷启动时先确认原进程退休，并要求已验证的原生 resume 能力。已经退休中的原进程，即使随后发现不支持恢复，也先等原退出证明再返回设置失败，避免 Core 尚未获得新句柄就释放工作区保护。

## Hook 和轮次

每个本地 turn 有独立运行期 ID。Hook 接受连接时捕获当时的 scope/turn，读取 payload、接收用户决定和最终非阻塞写入时重新核对。UI 审批和问题使用新的随机 request ID，不以可复用的原生 step 字符串充当授权。原生 conversation/step 的重复请求不重复分发；子会话仍须先由原生父会话事件登记。

旧处理器只移除自己的请求、向自己保存的原事件接收端报告 QuestionClosed；即使旧回答/允许决定随后到达，也不会泄露给新轮次。开始发送回复前若归属失效，返回拒绝；已写入一部分时关闭不完整回复，Hook 客户端解析失败后拒绝执行。不会尝试把一条部分发出的允许回复改写成另一条合法 JSON。

回答经表单验证后，还按最终嵌套 JSON 编码长度检查 Hook 的实际帧上限。过长回答保持原问题可回答，用户可以缩短后重试，不先消费回复槽再让传输失败。

依据 [Antigravity headless 协议](https://antigravity.google/docs/cli/headless/)，init 在长驻进程中仅发送一次；本地每轮开始事件不再依赖重复 init。显式出现的原生 conversation ID 若与已绑定身份冲突，停止原进程并把当前轮次标记失败，不能覆盖成新身份。[Hooks 文档](https://antigravity.google/docs/hooks) 中的 stepIdx 是 trajectory 的索引；不按到达顺序维护简单高水位，避免把并发乱序的合法 Hook 当作旧请求。

## 进程退出与查询

被动 `shutdown_proof` 捕获原进程对象，只等待原 OS 进程组/Windows Job、输入输出和 Hook 后台任务收尾。它不发停止命令，不根据会话 ID 查找后来替换的进程；中途清理错误、监督通道丢失和表项消失均不冒充成功。

安装级 auth 锁在原进程退出及后台收尾后释放，之后才发布正面完成回执。Driver 析构通过独立 owner 信号通知原进程，不依赖能否立刻取得全局会话表。候选进程在首次等待安装表锁之前就交给独立 supervisor；调用方放弃安装仍会退休同一个 OS 作用域。

原生 Hook 验证、模型列表和额度查询同样由独立任务持有 `OwnedChild` 和原 auth 锁；调用方取消或超时不会把仍在访问共享凭据的辅助进程交给下一次启动。观测超时与物理退出分开，退出暂未获证时继续清理原作用域。全局原生 auth 串行约束保留，因为 agy 共享认证文件与钥匙串；这不是所有 Agent 的统一并发限制。

## 验证

只使用私有 shell/Python CLI 和本机 loopback Hook 夹具，未调用已安装的 Agent、真实模型、屏幕/键鼠/剪贴板、手机模拟器或日常 Core。

十五项新增回归覆盖热复用与别名交接、旧/裸句柄拒绝、忙碌恢复保留原任务、原进程被动证明及同名冷替换、错误/监督者失联不伪造完成、启动验证和候选安装被取消后的独立清理、仅一次 init 的连续轮次、外来原生身份、模型和额度查询取消，以及延迟 Hook/旧审批/旧问题与过长编码回答。

存储集成使用真实 Core、SQLite、私有长驻 CLI 和实际 Hook TCP 传输，连续四轮验证首次身份绑定、原生 ID 热复用、审批、问答与物理取消；旧 scope 拒绝不影响当前任务。

macOS/Linux Antigravity 各 **45 项**通过，4 项按需真实 CLI 测试未启用；新增 Core 集成两端通过。macOS 现有 Core **98 项**、desktop **347 项**及全部存储 **162 项**通过；Linux Core **98 项**及相关存储集成 **29 项**通过。macOS 全工作区、Linux 四包和 Windows GNU 四包 all-targets 严格检查通过，格式与差异检查通过。Windows 为交叉静态检查，没有运行 Windows 二进制。

当前代码补充验收日志：`/tmp/dogcodes-agy-scope-accepted-macos.log`、`/tmp/dogcodes-agy-scope-clippy-accepted-macos.log`、`/tmp/dogcodes-agy-scope-accepted-linux.log`、`/tmp/dogcodes-agy-scope-accepted-windows.log`。前一完整回归日志为 `/tmp/dogcodes-agy-scope-final-macos.log`、`/tmp/dogcodes-agy-scope-clippy-final-macos.log` 与 `/tmp/dogcodes-agy-scope-final-linux.log`；最后补充的额度查询取消用例在 accepted 日志中验收。

现有三个开发服务/PostgreSQL/Redis 容器保持运行。未提交或推送。

## 仍须继续

原生协议没有为全部回调提供不可混淆的 turn 身份。本次能隔离接受时已属于旧 turn 的 Hook；如果原生旧工具直到下一轮才首次建立连接，且只有同一 conversationId 和此前未见的 stepIdx，仍不能凭本地接收时间证明它属于哪轮。无标签文本/终态、独立子进程脱离 OS 作用域和 actor 崩溃不能以当前局部验证冒充已解决。

ZCode 等其它 Driver、Core 原生启动阶段晚到句柄/退出回执、其余临时事务、自动续轮以及 Core 强退/系统重启继续推进。真实各系统 Agent、远控视频/弱网/HDR/光标/GPU、手机与 PC 功能对齐、推送和部署仍按 `CURRENT_WORK.md` 八个工作包跟进。


## 2026-10-27：模型与额度查询在取消后及时清理

按辅助入口核对：权限目录是静态值，子会话历史只读一个有大小上限的本地文件，没有为这两处新增进程监督。发现模型目录与额度 helper 已独立持有 OwnedChild/auth 锁，但它们等待原有 20/45 秒查询期限才进入清理；调用方已经取消时，随后启动的普通会话仍会被该锁挡住。不能通过提前释放锁修复，否则会重新引入共享凭据竞争。

现在两个已有 helper 各自保留一个观察者通道：调用方消失时结束原读取等待，进入原 terminate 重试流程；原树退出获证后才 drop 原 SpawnSlot。worker 尚未开始时已取消则不创建 CLI。没有建立新任务框架、改共享 auth 串行约束、缩短正常读取期限或改变原错误/模型解析/额度缓存/鉴权失效规则。该阶段只处理这两个有明确复现的辅助查询，不将 readiness/Hook 启动探测或无身份的供应商回调一并宣布完成。

新增固定 Python 夹具通过私有目录的 hard-link 执行，只读写私有 started/release 文件，不访问实际 CLI、账户或用户 HOME 文件。两个真实进程测试启动一个忽略 TERM 的 helper 和后代，取消查询后等待取得同一 auth 锁，此时核对两个原 PID 均消失，并验证模型可再次成功读取。反例在五秒内仍拿不到锁时，先写私有 release、等待原树获证结束，再失败断言；不会为制造反例遗留进程。旧源码两个反例都按该断言失败，日志 `/tmp/dogcodes-agy-metadata-before-final.log`。最初测试辅助函数缺少泛型 Debug 约束的编译失败单独保存在 `/tmp/dogcodes-agy-metadata-before.log`，不作为行为证据；已改用 matches 判断 JoinError，未添加不必要的约束。

macOS Antigravity 全包 47 项普通用例通过，4 项真实环境库用例及 1 项真实审批集成未启用，见 `/tmp/dogcodes-agy-metadata-suite-macos.log`。Linux 同样 47 项通过，4 + 1 项真实环境用例未启用；两端 desktop 各 350 项通过，分别 8/11 项按需未启用。两端 Antigravity/desktop all-targets 严格检查及 Windows GNU 同两包静态严格检查通过，没有运行 Windows 二进制。原运行器因新测试 match 写法触发 single_match_else 而退出 101，改为等价 if let 后仅重跑严格检查，不关闭规则或重复无变化的全套测试。

日志 `/tmp/dogcodes-agy-metadata-desktop-macos.log`、`/tmp/dogcodes-agy-metadata-linux.log`、`/tmp/dogcodes-agy-metadata-strict-final-{macos,linux}.log`、`/tmp/dogcodes-agy-metadata-windows.log`。第一次严格检查失败保留于 `/tmp/dogcodes-agy-metadata-strict-macos.log` 和 Linux 原运行记录。最终两个严格检查运行器均退出 0；自有容器退出清理，原三个开发服务保持运行。没有操作真实模型/屏幕/设备或日常 Core，未提交推送；原八包的剩余代码、平台、真实工具链和部署范围保留。


## 2026-10-27：可用性探测的观察者取消

模型/额度查询补齐后，继续核对独立的 probe_ready_with。它关闭 stdin 后等待首行 stdout，已有独立任务与原退出保护；但 stdout 一直开着而调用方取消时，仍要等原 20 秒读取期限，继续占用同一 auth 锁。原 EOF 回归只覆盖 stdout 已关闭，不足以证明这个路径及时退出。

为该已有 owned 探测加同样的观察者通道：任务开始前已取消则不启动；已启动后取消只结束首行等待，随后仍走原两秒自然退出宽限、原 terminate 与清理失败时的后台继续观察。没有把取消、Drop 或退出错误当正面退出证明；没有缩短正常探测期限，正常 JSON/鉴权/回退规则不变。在等待首行期间取消的读取不再进入正常事件解析；若读取已完成，仍按原结果处理规则收尾。

复用固定私有 Python 夹具，新用例保持 stdout 打开、记录 helper/后代 PID，取消后在五秒内取得原 auth 锁且两个 PID 都已消失。旧源码必须等写入私有 release 后才能清理，按“取消后仍占锁”的断言失败；反例先收尾再断言，没有遗留进程。原“EOF 后取消仍保护原锁”用例继续通过。macOS 全包 48 项普通测试通过，4 项真实库用例、1 项真实审批未启用；日志 `/tmp/dogcodes-agy-readiness-before.log`、`/tmp/dogcodes-agy-readiness-suite-final-macos.log`。

首轮实现漏导入 Tokio oneshot，库及依赖桌面检查在编译阶段失败，未执行用例；已补原 Tokio 模块的 import，没有引入新依赖。失败记录 `/tmp/dogcodes-agy-readiness-{suite-macos,desktop-macos,linux}.log` 保留。最终 Linux Antigravity 同样 48 项通过，4 + 1 项真实环境用例未启用；两端 desktop 各 350 项通过，分别 8/11 项按需未启用。两端 Antigravity/desktop all-targets 严格检查及 Windows GNU 同两包静态严格检查通过，未运行 Windows 二进制。最终日志 `/tmp/dogcodes-agy-readiness-final-linux.log`、`/tmp/dogcodes-agy-readiness-desktop-final-macos.log`、`/tmp/dogcodes-agy-readiness-strict-macos.log`、`/tmp/dogcodes-agy-readiness-windows.log`，两个最终运行器均退出 0。

本阶段没有调用真实 Agent/账号、桌面/键鼠/设备或日常 Core，未提交推送。Hook 启动验证、供应商无标签事件与原八包其余范围仍按当前清单核对；不以本项通过宣称全部辅助事务或跨平台验收完成。


## 2026-10-27：启动 Hook 验证取消与缓存归属

继续核对最后这个独立启动验证 helper。Bridge::prepare 原来已由 owned 任务持有原 auth 锁并等待原树退出，但创建调用方取消后，prepare_owned 仍可等满 45 秒验证期限；成功验证还可能继续写入 VERIFIED 缓存。该路径不属于普通工具审批操作，本次没有更改审批 allow/deny 或用户回答规则。

prepare 为已有 owned 任务保留观察者通道，在开始、缓存命中返回以及启动 CLI 前检查原调用方；已启动后的取消结束原输出读取，仍沿用同一原进程 terminate/正面退出确认后再释放原 auth 锁。正常验证仍保留 45 秒、原输出大小限制、源配置核对和缓存规则。原树清理完成后，在验证结果/缓存发布前再检查观察者，已取消的结果不写缓存、不返回可继续启动会话的 Bridge。没有新增调度层、全局重试或兼容格式。

新增公开 create_session 路径的私有 CLI 回归：Hook 阶段保持输出打开、记录原 helper/后代，取消创建后在五秒内取得原锁时两个 PID 都已消失；会话表仍为空，随后正常模型读取可以成功。旧源码在原锁仍占用时失败；失败前写私有 release、完成原树清理，没有遗留进程。证据 `/tmp/dogcodes-agy-hook-cancel-before.log`；固定夹具复用前两轮 metadata.py 的 hard-link，只使用私有 CLI 和临时项目，没有调用真实账号/模型或修改用户配置。

macOS/Linux Antigravity 全包各 49 项通过，4 项真实环境库用例及 1 项真实审批集成未启用；两端 desktop 各 350 项通过，分别 8/11 项按需未启用；原 Core/SQLite 的热会话归属/审批/问答/取消集成两端各 1 项通过。两端 Antigravity/desktop all-targets 严格检查及 Windows GNU 同两包静态严格检查通过，未运行 Windows 二进制。

日志 `/tmp/dogcodes-agy-hook-cancel-{suite-macos,routing-macos,desktop-macos,strict-macos,windows,linux}.log`。两个运行器均退出 0，自有容器退出清理，原开发服务容器和数据卷保留。没有真实桌面/键鼠/设备、日常 Core 操作或提交推送。供应商无标签回调、未知启动、其它辅助事务及原八包的实机/网络/部署范围继续；不以这组取消验证外推整体完成。
