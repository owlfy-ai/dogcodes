# Claude 会话归属、原进程退休与后台观察者接续

日期：2026-10-26。本阶段在 `remote-control` 工作区完成代码和私有本地验证；软件按尚未发布处理，没有添加旧句柄兼容、历史数据迁移或原生 ID 绕过。

## 解决的问题

Claude 原先按持久化 session ID 接受 prompt、取消、审批、问答和关闭。冷恢复与热复用后，旧句柄都能命中新任务；异步入口即使已查到进程，等待写入期间也可能跨过复用。原有关闭按进程号调用 Windows `taskkill`，并在最后按 ID 删除记录，不能保证退休的是原进程树，也可能移除替换记录。

现在每次创建、冷恢复和成功的空闲热复用都有新的运行期 scope。异步写入/审批/配置持有准入保护并重新校验；Begin/Cancel 命令带着原 scope 排队，实际处理时再核对。旧句柄和只有 session ID 的句柄都被拒绝，prompt 不通过它们自动恢复进程。

停止和关闭不等待被阻塞的 prompt 写入释放准入锁：它们按原 scope 核对，关闭原子地禁止后续复用，并让独立监督者关闭原 OS 作用域。否则，恰恰需要停止的那次写入会阻塞停止本身。迟到的原关闭确认只移除相同进程实例；已确认退休的内存回执只允许同一 scope 重试，不把记录缺失当成退出证明。

## 原生作用域和退出证明

CLI 在启动时接入已有 `OwnedChild`，使用 Unix 私有进程组或 Windows 私有 Job。删除了 Claude 自己按 PID 关闭进程树的实现。就绪探测/模型辅助 CLI 同样使用 OS 作用域，清理失败不再作为成功返回，但它们的异步退休与并发准入仍待统一。

常驻会话的独立监督者持有原 `OwnedChild`、有界 stdout 管道和 stderr 排空任务。Driver Drop、实际 EOF、主进程退出、明确关闭及部分写入取消都能启动同一原资源的退休。观察者结束或协议 actor 卡在写入时不会丢掉 OS 资源。OS 清理暂时失败时保留原对象继续同一次退休，不向 CLI 重放控制请求，也不按后来的进程号查找对象。

`shutdown_proof` 在关闭前捕获原 `SessionProcess`：先等原 OS 作用域与管道收尾，再等协议/后台输出及待决响应收尾。它不发消息、不恢复连接、不查询同名替换记录；监督通道失联不算成功。关闭超时只报告尚未确认，Core 可以继续保留原 Run/工作区占用并观察该证明。

配置读写、初始选择验证、prompt 准入和部分行写入都有取消保护。已经开始原生操作却被调用者放弃时，会禁止并退休原进程，避免半条消息或未知配置进入下一轮。已验证的配置回滚保留原会话；只在配置成功且仍为空闲时交接新句柄。如果配置期间出现后台执行，不再把它作为空闲复用交接；已发配置的冲突会退休原作用域。

## 后台续答和旧输出

每次成功热复用也更换根续答事件通道，Core 随本次实际句柄取得新观察者。否则，Core 会一直用旧句柄处理新续答，导致审批/取消失效。启用过后台观察后，即使 Core 尚未取走新 receiver，新续答也先进入本次通道；关闭中的进程不再开启新续答。

后台工具保留其原输出发送器，所以旧 Bash 输出仍送往原轮次。已验证回滚的配置失败不更换句柄或事件通道，避免没有成功返回新句柄却提前丢掉原观察者。

这只解决热复用交接的归属，不代表自动续轮的异常清理、工作者失效或所有缺少原生身份的回调已经收口。

## 验证范围

十四项新增 Driver 回归覆盖旧句柄/排队取消、等待准入后的重检、原进程被动证明、监督者失联、阻塞写入下的 Driver Drop/公开停止/公开关闭、取消部分写入和配置、初始化中断、实际 EOF/退出 leader 的子进程、迟到关闭与替换实例，以及热复用/配置回滚后的后台观察者。旧进程树单测的覆盖移入独立监督路径；原真实 CLI 验证仍需显式启用。

新增 `agent-store` 集成测试通过真实 Core、SQLite 和私有 Python CLI，连续完成两轮前台任务后验证后台续答审批与取消。它使用实际返回的新句柄，确认 canonical `turn.failed` 的 `failureKind=interrupted`，不把取消当成普通完成。其它相关存储回归未放宽。

最终 macOS Claude **68 项**、Core **98 项**、desktop **347 项**及全部存储 **160 项**通过；Claude 八项、desktop 八项需另行授权/环境的单元测试未启用。Linux Claude **68 项**、Core **98 项**及相关存储集成 **27 项**通过。macOS 全工作区与 Linux 四包 all-targets 严格 Clippy、Windows GNU 四包 all-targets 严格交叉检查通过，格式/差异检查通过。Windows 未执行实机二进制。

日志为：

- macOS：`/tmp/dogcodes-claude-scope-accepted-macos.log`，严格检查 `/tmp/dogcodes-claude-scope-clippy-accepted-macos.log`。
- Linux：`/tmp/dogcodes-claude-scope-accepted-linux.log`。
- Windows GNU：`/tmp/dogcodes-claude-scope-accepted-windows.log`，属于交叉静态检查，没有运行 Windows 二进制。

没有调用已安装 Agent/真实模型，没有操作真实桌面/输入/剪贴板、手机模拟器或日常 Core；开发服务和数据库容器保持运行。未提交或推送。

## 仍需继续

其余 Driver 的 scope/退出合同、无原生身份的回调、全部临时辅助事务的取消与准入、原生启动未返回句柄时 Core 的晚到结果、协议 actor 本身失效、自动续轮的异常执行与 Core 强退/重启恢复继续。当前证明不把这些未知情况伪造成完成。

原视频的实机/弱网、HDR/光标/GPU 路径，PC/手机功能对齐及推送/部署仍按 `CURRENT_WORK.md` 八个工作包推进，不因本阶段结束缩减总目标。

## 2026-10-27：模型与权限目录辅助进程

核对发现模型目录和权限目录各自直接启动临时 CLI，没有共同准入；取消依赖 OwnedChild 的 Drop 兜底，不能等待原进程组退出后再放行下一查询。原路径也没有读取 stderr，CLI 在初始化中输出较多诊断时会堵住，目录即使正常也无法交付。私有 stdio 夹具复现并发第二查询在第一查询仍存活时完成；修复前证据为 `/tmp/dogcodes-claude-catalog-before2.log`。

复用已有 NativeProcess 监督与原 OS 作用域。每个具体 Driver 的目录查询共享一把独立准入锁，既不占用普通任务连接，也不改变运行中的会话；模型、工作区模型和权限目录三个入口统一接入。排队者取消不启动，已启动查询的原观察者消失会关闭自己的 helper；受管 worker 保留原准入，直到 native.confirmed 取得原进程树退出证明。读取失败、十秒查询期限及 panic 都进入同一收尾，不因观察者放弃而启动替代进程或重发控制。成功结果也在退出获证后交付，去掉原本额外等待三秒自然退出的流程。

stdout 复用有界 duplex，沿用原目录 1 MiB 读取上限；stderr 由已有监督持续排空，不积存诊断内容。请求仍只发送 initialize 控制，目录解析、模型/思考选项及 supportsAutoMode 规则保持，不发送模型任务，不新增协议/依赖/兼容或迁移。

新增五项真实私有进程回归：大 stderr 的成功目录与退出证明；取消查询后权限探测须等原树退出、且普通会话存活；模型/权限并发不重叠；空目录失败后获证再查；排队取消不留迟到启动。夹具沿用固定可执行文件及私有工作区数据，测试不会调用已安装 CLI/账号。专项五项及 macOS/Linux 的 Claude 各 72 项、desktop 各 349 项已通过；完整检查证据另见当前工作清单的本轮最终记录。

这关闭本 Driver 的目录辅助进程缺口，不代表其余额度/辅助事务都已审完；实际 CLI 目录能力、Windows 进程树、跨平台实机与原八包范围保留。未重启日常 Core、运行模拟器或提交推送。


## 2026-10-27：额度 HTTP 与凭据辅助命令

继续核对账户额度入口，发现 Cache 的互斥只覆盖调用者等待。调用者取消后，原 `wait_with_output` 的工作者仍在退休 curl 进程组，下一次强制刷新却可立即启动。私有完整 Driver 反例 `/tmp/dogcodes-claude-quota-before.log` 中，第二次假 curl 的启动记录明确包含仍存活的前次 leader 和后代；这不是仅凭锁状态推断。命令 stdin 写入阶段此前也只由调用方持有，取消时没有同一工作者等待原树退出。

现在 Cache 保留一把原 helper 准入，macOS 凭据辅助命令和额度 HTTP 使用同一许可。局部独立工作者持有原 OwnedChild、stdin 写入、有界 stdout 与许可，调用方取消时结束原读取/写入并清理；自然退出、读取失败、输出超限也在原进程及普通后代获证结束后才返回。准入等待者取消不启动；退出暂未获证时仅继续同一次退休，不重放 HTTP 或凭据命令。复用原组/Job 和宿主 guard，没有新增抽象层、协议、依赖版本或迁移。

仍只读取本机已有 OAuth，不刷新或写回凭据，不发送模型任务。令牌仅通过 stdin，不放入进程参数或错误；原 Keychain 五秒、整体二十秒和 curl 十二秒/响应大小限制保留。缓存、强刷、429 Retry-After/退避与登录失效清空语义保持，取消结果不写缓存。

新增四项私有进程回归及一个隔离子测试入口：完整 Driver 取消后再强刷必须等待原树、正常结果与缓存不重启、429 强刷也遵守退避及 401 清缓存；部分 stdin 阻塞取消后不重叠启动；输出超限失败和无 stdin 的凭据型成功均先确认后代退出；排队取消不留下迟到启动。完整 Driver 测试在单独进程设置私有 CLAUDE_CONFIG_DIR 和假 curl 路径，不修改测试宿主环境，也不触碰 HOME、真实凭据/Keychain、安装的 CLI 或网络服务。其余测试只启动固定私有 Python 夹具。

本地专项通过，日志 `/tmp/dogcodes-claude-quota-scopes-macos.log`。真实 Keychain 权限/账号、Windows Job 及平台实机验收继续；其它未知启动、未标识回调、临时事务和原八包范围保持，未操作日常 Core、提交或推送。


最终 macOS/Linux Claude 各 **77 项**、desktop 各 **350 项**通过；Claude 八项真实/按需单元测试以及 macOS/Linux 分别 8/11 项桌面按需测试未启用。两端两包 all-targets 严格 Clippy 与 Windows GNU 两包 all-targets 静态交叉检查通过，Windows 未执行程序。fmt/diff 检查及 Cargo.lock 本轮前版本比较通过，原开发服务保持运行。日志 `/tmp/dogcodes-claude-quota-suite-macos.log`、`/tmp/dogcodes-claude-quota-final-strict-macos.log`、`/tmp/dogcodes-claude-quota-linux.log`、`/tmp/dogcodes-claude-quota-windows.log`。改动限于 Claude 额度辅助入口，没有改动普通会话、手机、服务端或视频实现。


## 2026-10-27：就绪探测的诊断排空和自然退出

Claude 的 `probe_ready` 仍自行持有 OwnedChild，只读 stdout，不排空 stderr。私有 CLI 在初始化前写较多诊断时会堵住，无法交付合法握手，正常安装可能被误标为不可用；原始断言失败见 `/tmp/dogcodes-claude-ready-stderr-before.log`。取消原探测还会直接进入 Drop 的 kill 兜底，跳过原本为 CLI 凭据落盘保留的五秒自然退出窗口。另一个私有用例在已经收到 stdin EOF 后取消，再允许 CLI 完成，原实现没有完成标记；证据 `/tmp/dogcodes-claude-ready-before.log`，不使用真实账号文件模拟。

现在静态探测复用现有 NativeProcess 和观察者监督：有界转发 stdout、持续排空 stderr 到 sink，独立工作者持有初始化读取和退出证明。外层取消或查询结束会关闭自己的原资源，保留五秒自然退出，超出后沿用原组/Job 退休并确认；正常返回和协议失败也等待实际进程及普通后代退出。不向用户任务派发消息、不共享业务连接、不按历史 PID 寻找进程。NativeProcess 的普通 start 仍使用原 300 ms 宽限，只有探测指定原有五秒值。

仍只发送一次 initialize 控制，校验成功响应及原 request_id，不发送模型 prompt。保留十秒握手期限和 2 MiB 行上限，并在实际 read_line 期间限长；超大无换行输出不再等无限扩张后才判断。没有新增管理层、依赖版本、协议、兼容或迁移。

新增三项私有回归覆盖较多 stderr 下的成功握手和延迟自然退出；取消后仍完成 EOF 收尾、普通 peer 不受影响；错误 request_id 和超大无换行数据拒绝且先确认原树退出。所有进程均为固定假 CLI 和私有目录/符号链接，未调用实际 CLI/Keychain、账号、模型或设备。修复初轮有一次四秒外层观察超时，日志 `/tmp/dogcodes-claude-ready-current.log`；当时观察混合了启动与处理，不能据此认定具体系统原因。最终测试先确认假 CLI 实际收到 initialize，再以同样四秒验证处理和真实退出；启动另由既有 wait_for 的界限约束，没有放宽产品或处理期限。原负例已停止私有运行进程，首次原始 Drop 负例中死 leader 的 orphan-reap 观察失败也原样保留，不计成功证据。

专项三项及严格检查已通过，日志 `/tmp/dogcodes-claude-ready-scopes-macos.log`、`/tmp/dogcodes-claude-ready-strict-macos.log`。实际安装的终端行为/凭据落盘、Windows Job、平台和其余就绪探测仍待验收；日常 Core、原开发服务及未提交修改保持，未提交或推送。


最终 macOS/Linux Claude 各 **80 项**、desktop 各 **350 项**通过，Claude 八项以及桌面 macOS/Linux 分别 8/11 项真实/按需测试未启用。两端两包 all-targets 严格 Clippy 与 Windows GNU 两包 all-targets 静态交叉检查通过，Windows 未执行程序。Python 语法、fmt/diff 与 Cargo.lock 本轮前版本比较通过，原三项开发服务正常。日志 `/tmp/dogcodes-claude-ready-suite-macos.log`、`/tmp/dogcodes-claude-ready-final-strict-macos.log`、`/tmp/dogcodes-claude-ready-linux.log`、`/tmp/dogcodes-claude-ready-windows.log`。未调用真实模型/账号、设备或日常 Core，未提交推送；其它原工作包仍按 CURRENT_WORK 保留。
