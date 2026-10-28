# ACP 额度、上下文与费用读取的归属

> 后续进展：ACP 操作句柄及 Core 原运行句柄传递也已接入，见 [句柄归属](acp-session-handle-scope-2026-10-26.md)。本文对这些范围尚待处理的描述为当时状态，其余边界仍保留。

日期：2026-10-26。工作包 2 的代码与本地验证进展；不表示物理执行代次、所有无会话回调或 Core 强退已经收口。应用仍按当前格式开发，不增加旧版兼容或数据迁移。

## 问题与处理

账户额度读取原先复用业务连接。查询等待、超时或调用方取消时，原生请求可能仍在执行；其未标明会话的输出也可能被当成当前任务输出。会话上下文和费用查询则必须使用原生会话，不能通过创建另一个同名会话读取，也不能把调用方放弃等待当成原生查询已经结束。

- Kiro `_kiro/account/getUsage`、Grok `_x.ai/billing` 使用与模型/权限目录相同的独立查询作用域，继承安装和启动配置，不加载业务会话。查询准入一直持有到 helper 物理退出得到证明；成功、失败、取消与超时均只清理 helper。保留账户缓存、并发合并、强制刷新、暂时失败保留有效缓存及鉴权失效清除缓存的规则。
- Kiro `_kiro/session/context` 只在已有连接且无活动轮次/配置过程时派发，固定原 Resident、原物理连接与原 Turn。调用方最多等待两秒；派发前取消则不发送，派发后取消或超时仍持有原请求，直到原回复、声明支持的原生关闭 ACK 或原物理退出。没有收到真实结束证明时，不重发同一读取。
- Grok `_x.ai/session/usage` 在原 `response_completed` 上取得会话/轮次归属，并在后台等待原请求。两秒仅是结果可发布期限，不提前释放原生所有权。已过期、会话重载或已开始下一轮的费用结果不合并。
- 每个原 Resident 最多一个未结束的上述会话读取，每个 Driver 总计最多八个。即使未支持原生 close 的 Agent 反复本地退休/重载，也不会无限增加悬挂读取。最后所有者退出原物理作用域后，这些读取一起获证结束。
- 上述会话读取悬而未决时，缺少会话身份的回调/交互不猜测归属；明确带当前会话身份的输出和审批/问答仍可用，已有其它会话仍可发送任务。Cursor 的 `cursor/task` 也先核对活动会话，避免另一个会话复用相同 toolCallId 时消费当前工具状态。
- 文件用量读取固定原工作目录、Resident 与 Turn；返回前再核对身份。退休、物理退出、同 ID 重载、进入下一轮或目录变化后，原读取不能覆盖当前用量。读取期间已有新用量进入缓存时，旧文件结果不合并。没有活动 Resident 的历史用量仍可以只读获取，不为此启动 Agent。

以上涉及真实 Agent/SDK 的能力差异，不是为本项目旧客户端保留兼容路径。没有重新引入探测用业务 `session/new`，也没有对未声明能力的 Agent 伪造关闭完成。

## 本地验证

新增十二项 Unix 私有进程回归和一项可移植文件归属回归。原来假定账户读取复用业务命令通道的假接口测试改为缓存不启动进程测试，原缓存并发、强刷、失败和鉴权语义由私有进程回归验证。

覆盖 helper/业务 PID 隔离及实际退出、账户输出不混入任务、调用方取消及派发前取消、上下文超时不释放请求/不重发、明确带会话身份的问答继续可用、同 native ID 重载与下一轮结果隔离、原生关闭结束未回复读取、费用超时仍保留所有权、八项上限及物理退出清理，以及异会话 Cursor task 拒绝。

所有进程都是临时 Python 协议夹具；未调用已安装 Agent、真实模型、用户桌面/输入/剪贴板，也未切换日常 Core。测试中的两秒超时实际经过，不依赖预先释放阻塞来模拟完成。

- macOS/Linux ACP 各 **130 项通过**；21 项按需单元测试及六项真实集成测试未启用，两端 all-targets 严格 Clippy 通过。
- macOS Core **92 项**、desktop **347 项**通过，八项桌面按需测试未启用；两包 all-targets 严格 Clippy 通过。
- Windows GNU ACP all-targets 严格静态交叉检查通过；没有运行 Windows 二进制，不等同 Windows 实机验收。

验证日志：`/tmp/dogcodes-acp-auxiliary-final-macos.log`、`/tmp/dogcodes-acp-auxiliary-linux.log`、`/tmp/dogcodes-acp-auxiliary-dependents.log`。

## 后续边界

仍须处理旧 SessionHandle 跨物理连接使用、原生 ID 重用下不能仅靠字符串区分的迟到事件、Core 强退/重启和所有无会话回调。原生查询完成后的未标识延迟通知也不能由本阶段的 pending 计数完整归属；不能据此宣称所有协议事件已隔离。当前只防止辅助结果跨原 Resident/Turn 发布并限制其资源占用。

真实 Kiro/Grok 的账户冷启动耗时、权限配置和客户端版本需后续实测；Windows 此阶段只有静态交叉检查。视频实机/网络恢复、PC/手机功能对齐及八包其余范围继续见 CURRENT_WORK。

## 2026-10-27：Kimi 额度服务取消与查询准入

Kimi 的只读额度查询使用临时 `kimi web --port 0 --no-open` 服务。原实现取消后已有独立进程清理，但调用者持有的刷新锁随取消立即释放，没有覆盖原服务退出确认；之后强制刷新可能在旧服务仍结束时再次启动。此路径未复用 ACP 模型/账户查询已有的准入保护。

现在先取得原 Driver 的 `shared.metadata` 准入，再由独立工作者持有同一许可、原 helper 及结果通道。调用者取消或外层 27 秒期限结束时，观察者关闭会停止原 banner/HTTP 读取，并请求既有 `lifecycle::Process` 清理。八秒退出观察失败只继续等待原清理，不释放许可或再次启动服务；实际进程组/Job 获证结束后才放行。结果同样在退出确认后交回调用者，取消的结果不会更新账户缓存。

仍使用原 loopback 地址/令牌检查、15 秒 banner 期限、10 秒 HTTP 期限、大小上限与禁止代理/重定向规则。没有发送模型 prompt、创建业务会话、新增进程抽象/依赖或修改当前格式协议；正常账户缓存、强刷与失败语义保持。

新增三个真实私有进程回归：取消时持有准入直至原 helper/后代退出；正常 HTTP 额度返回前已退出且后续缓存命中不重启；排队等待准入时取消不会稍后启动。原取消 helper 清理回归继续保留。新取消用例在原源码上失败，修复后通过。HTTP 只访问临时 Python 夹具的 127.0.0.1 服务，不访问已安装 Agent、真实账户或模型。

macOS 与 Linux ACP 各 144 项、desktop 各 350 项通过；21 项真实/按需驱动单元测试、六项真实集成测试及 macOS/Linux 分别 8/11 项桌面按需测试未启用。两端两包 all-targets 严格 Clippy 和 Windows GNU 两包 all-targets 严格静态检查通过；Windows 未执行二进制。严格检查先发现新 wrapper 使用 `&PathBuf`，改成等价的 `&Path`/`to_path_buf` 后通过，没有放宽 lint 或重复无行为变化的全套测试。

原源码反例日志 `/tmp/dogcodes-kimi-quota-before-macos.log`，修复专项 `/tmp/dogcodes-kimi-quota-current-macos.log`，完整结果 `/tmp/dogcodes-kimi-quota-suite-macos.log`、`/tmp/dogcodes-kimi-quota-desktop-macos.log` 与 `/tmp/dogcodes-kimi-quota-linux.log`；最终严格检查 `/tmp/dogcodes-kimi-quota-strict-final-{macos,linux}.log` 及 `/tmp/dogcodes-kimi-quota-windows.log`。其余辅助事务、未标识回调、视频/平台实机和部署继续按原八包推进；未操作日常 Core、提交或推送。


## 2026-10-27：Cursor 临时终端的真实进程归属

Cursor `/usage` 的临时终端原先通过 macOS `script` 启动。私有复现确认：`script` 为实际 CLI 建立另一会话和进程组，结束外层原组之后，忽略 HUP/TERM 的 CLI 和普通后代仍存活。因此只把外层包装器接入 `OwnedChild`，不能证明终端任务已结束；观察者取消也会先释放原刷新锁，允许下一次查询重叠启动。

现由现有 `process-scope::OwnedChild` 直接创建 PTY，让实际 CLI 成为可保留身份的会话/进程组 leader，并沿用原宿主生命周期 guard。终端尺寸仍为 120×40，标准输入/输出/错误都连接同一终端；额外描述符设为 exec 时关闭。没有新增进程池、协议或依赖版本。非 macOS 生产额度入口仍返回原不支持提示，Linux 仅用私有夹具验证共享 Unix 边界。

额度读取复用原 Driver 的 metadata 准入，由独立工作者持有许可、临时目录和原 `OwnedChild`，直到原组获证退出；取消只结束观察，清理未确认时继续保留原对象，不重发命令。自然退出同时观察 leader 和终端输出，避免后代持有 slave 描述符导致 EOF 永久等不到。PTY 挂断的 EIO 只当作输入结束，成功仍要求完整本地菜单交互、有效额度和真实成功退出；取消结果不更新缓存。

新私有 TUI 夹具验证取消/后代退出前不准入下一查询、正常及非零退出的原树收尾、排队取消不启动、macOS 实际 Driver 缓存命中不重启。夹具明确检查实际 CLI 自身 PID=进程组=会话 ID、前台终端归属及尺寸，记录的输入仅为 `/usage`、回车、Esc、`/quit`、回车；没有模型请求。原宿主退出测试扩展到管道/PTY 两种方式，各验证直接退出与 SIGKILL，并保留无关兄弟进程。

改前反例 `/tmp/dogcodes-cursor-terminal-before.log`；macOS 专项 `/tmp/dogcodes-cursor-terminal-quota-macos.log`，两端完整进程/ACP/runtime 回归 `/tmp/dogcodes-cursor-terminal-regression-macos.log`、`/tmp/dogcodes-cursor-terminal-linux.log`。真实 Cursor 终端布局、账号/安装版本、Windows Job 和用户设备仍待验收；其余未标识回调、未知启动及原八包范围保持。


本轮最终验证：macOS ACP 148 项/runtime 14 项、Linux ACP 147 项/runtime 12 项通过；两端 process-scope 各 7 项单元及 3 项集成测试、desktop 各 350 项通过。原有真实/按需用例未启用；桌面按需 macOS 8 项/Linux 11 项未启用。两端 process-scope、runtime、ACP、desktop 的 all-targets 严格 Clippy，以及 Windows GNU 相应 all-targets 静态检查通过；Windows 未执行程序。严格检查要求将新单模式 match 改为等价 let…else，已改并通过，没有放宽 lint。最终日志 `/tmp/dogcodes-cursor-terminal-strict-final-macos.log`、`/tmp/dogcodes-cursor-terminal-final-linux.log`、`/tmp/dogcodes-cursor-terminal-windows.log`、`/tmp/dogcodes-cursor-terminal-desktop-windows.log`。fmt/diff 检查通过，Cargo.lock 与本轮前保存版本一致；原 server/Postgres/Redis 容器正常，未操作实际 Agent/账号、日常 Core、提交或推送。
