# ACP 模型与权限目录探测的独立作用域

> 后续进展：业务 session/close 已按 initialize 的实际声明接入并完成本地验证，见 [原生关闭](acp-native-close-2026-10-26.md)。本文关于原生关闭待接入的描述是当时状态，其它边界继续保留。

> 账户查询已复用独立作用域，上下文/费用读取已增加原会话与轮次归属，见 [辅助读取](acp-auxiliary-scope-2026-10-26.md)。下文这些调用待处理的描述同样是当时状态。

日期：2026-10-26。工作包 2 的一部分；不据此关闭 Core 强退、物理执行代次或其它辅助调用的剩余范围。

## 问题与处理

旧版 `model_catalog_in_workspace` / `permission_catalog` 把探测放入业务 Driver 的同一条 SDK 命令队列，使用 `session/new`，模型思考强度探测还会调用 `session/set_config_option`。调用方取消不会使这些原生请求撤销；命令循环等待探测时，排队的 prompt/cancel/本机会话退休也可能被挡住。探测创建的原生会话并未作为业务 Resident 登记，不能把本机 SessionStates 没有记录当成其资源已经卸载。

现在目录查询在自己的临时 AcpDriver/原生进程作用域内完成，继承安装身份、启动参数、版本与所需工作目录，使用独立更新、用量和会话状态。原有 NativeProcess/OwnedChild supervisor 负责原进程及任务组退出证明。未调用模型轮次、不带历史 MCP；不把 helper 的 native sessionId 当成业务会话身份，也不共享业务审批/问答路由。CodeBuddy 的启动目录使用本次查询工作目录。Cursor 的静态权限目录仍不启动进程。

- 每个原 Driver 的目录查询串行准入；等待准入最多 45 秒，可取消的等待者不会留下稍后执行的请求。
- 请求派发后，独立后台任务持有准入和 helper，调用方消失也不丢清理。真正执行查询前优先检查接收者是否已取消。
- 查询本身有 30 秒期限，不只依赖 Core 的 30 秒等待或 UI 取消。超时/取消只停止原 helper，不中断任何业务 Driver。
- 成功或失败都先请求 helper 关闭，再等完整 finished 证明，确认后才返回原结果和释放下一项探测准入。不是把 EOF、丢弃接收者或 CLI 关闭请求当成已退出。
- 10 秒内退出仍未确认时报告未确认，后台任务继续持有准入，原 supervisor 继续清理；后续查询不能越过旧作用域启动。只在实际 finished 后释放。
- 活动轮次期间可以查询模型/权限目录，不再用业务 `turn_active` 提前拒绝这些独立读取。不会因此修改当前业务会话的模型、权限或正在发送的任务。

这是一项资源隔离策略，不是对所有 Agent 协议能力的否定。当前锁定的 SDK 2.1.0 / schema 1.7.0 的 **v1 已包含可选 `session/close`**，由 `agentCapabilities.sessionCapabilities.close` 声明；遗漏/null 不声明支持，`{}` 表示支持。此前文档称“稳定 v1 没有通用 close”不准确，已订正。目录探测关闭整条临时连接，不依赖该可选接口；业务共享会话按声明发送原生 close 仍须接入，当前共享退休仍是本机缓存/路由和最后 owner 物理退出的回退实现。

## 回归与证据

全部使用内存协议夹具或私有 Python ACP 进程及其临时目录；未调用安装的 Agent、模型、真实凭据、手机模拟器或用户桌面。没有切换日常 Core，没有改手机/服务端生产代码。

新增 8 项 Unix 默认回归，前 3 项先在旧业务路径上得到对应失败：

1. 探测使用不同于业务进程的 PID；返回目录前 helper 已实际退出，业务 PID 仍存活。延迟 EOF 退出夹具排除“发关闭就算完成”的竞态。
2. 目录 `session/new` 被故意阻塞时，业务 prompt 和 cancel 仍能派发、原 Turn 正常结算。
3. 取消正在等待配置响应的目录查询，不提前释放下一项查询准入；新查询开始时原 helper 已退出，业务进程保留。
4. 查询未来已创建但尚未派发时取消，不启动任何原生进程，也不留请求待执行。
5. 队列中的查询取消后，前项完成也不会再启动它。
6. Native 不主动解除阻塞时，取消查询仍通过实际作用域终止结束 helper，保留业务进程；不靠测试方先解除阻塞才宣称清理成功。
7. 直接调用 Driver、不依赖 Core/调用方超时，查询自身期限仍回收 helper，确认退出后报告超时。
8. 正在运行任务时可读两种目录；helper 与业务 native sessionId 可以相同，其更新不进入业务回答流，业务连接没有额外探测 session/new。

原有两个排队取消/共享退休用例保留，改为在私有命令边界直接设置 SDK 背压；它们继续核实取消早于 prompt 派发时不发任务、取消退休等待后本机 ForgetSession 仍须获证。这些用例不再要求公共目录查询占用业务连接。

- macOS/Linux ACP 完整各 **109 项通过**，21 项原有真实/按需用例未启用；两端 all-targets 严格 Clippy 通过。
- macOS Core **92 项**、desktop **338 项**通过，7 项桌面按需用例未启用；desktop all-targets 严格检查通过。
- Windows GNU all-targets 严格交叉检查通过；没有执行 Windows 二进制或取得真实 Windows Job 验收。
- 三个独立只读源码副本分别移除退出证明、提前释放准入、忽略调用方取消，均在对应行为断言失败：返回结果时 PID 仍在、原 helper 未退出却已放行、取消后仍创建 native 请求。每次清理私有 proof target 的 acp-driver 包并核实旧测试二进制不存在，最后恢复源码后完整 109 项与严格检查通过。

日志：`/tmp/dogcodes-acp-metadata-{before,acceptance,final-linux,final-windows,core-desktop,proof}.log`；反例副本路径/摘要 `/tmp/dogcodes-acp-metadata-witnesses.json`，结果 `/tmp/dogcodes-acp-metadata-witness-results.json`。当前行为源码与恢复通过的副本一致；之后只订正了 optional close 的模块注释与文档。

## 继续范围

独立进程增加启动、初始化和退出成本。真实 Cursor/Kiro/CodeBuddy 等客户端的冷启动、模型账户能力和 30 秒 Core 预算需按版本实测；不能把私有协议服务通过当成所有真实 Provider 的速度与兼容性已验收。后续优化必须保留隔离与获证回收，不回到业务连接里不断创建无法确认释放的探测会话。

本阶段不改变 `connect_ready` 已认证连接的拥有方式，也没有把所有额度/上下文/模板读取替换为临时进程。业务可选 session/close、其它辅助与无 sessionId 回调、原生 ID 重用、持续输出背压、Core 强退/重启、跨物理代次与 PC/手机功能恢复继续。视频设备恢复、HDR/光标/GPU 路径及全部实机/网络/凭据/部署也继续保留在 CURRENT_WORK。
