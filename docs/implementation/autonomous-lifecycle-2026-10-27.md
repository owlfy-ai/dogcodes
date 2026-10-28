# 自动续轮的异常清理与确认归属

日期：2026-10-27。工作包 2 的有限修复，复用现有运行保护与原资源清理，不增加协议、迁移或恢复队列。

## 问题与改动

Agent 在用户轮次结束后自行启动的根轮次走 `autonomous_turns`。原流程没有前台任务使用的 `Execution` 保护：流失败、缺少终态或存储异常可能在原生工作退出前结算；panic 则可能留下活动运行与工作区占用。

现在自动续轮准入后立即持有 `Execution`，工作区读保护归入同一运行；消费事件前捕获原 Driver、实际句柄及被动退出证明。失败/EOF/异常沿用一次原关闭，等原资源确认退出后再收尾；原表项即使被移除，仍等待已捕获的证明，不重新查询或关闭替换 Driver。未准入的原会话由同一清理组件退休，等待期间计入 Core 活动保护。

成功终态及支持常驻复用的中断保持原有复用行为。自动观察者还负责接收未来的续轮，因此仅在终态持久化成功后归还会话；终态/冻结变更保存失败时，原观察者保留责任并补写同一结果，恢复后继续接收后续根轮次。真正的流错误或工作者退出仍由原运行保护退休该会话；已经收到的部分回答保留。普通任务也共用这一补写原语，见 [终态与变更](core-execution-cleanup-2026-10-26.md#2026-10-27保留原终态与已采集变更)。

Claude 的自动续轮 ACK 同时改为接收完整 SessionHandle，并通过原 scope 查找资源。同一个 native session ID 热复用后，旧句柄的迟到 ACK 不再减少新句柄的待接续计数。

## 持久化工作区基线补齐

后续核对确认自动续轮只持有内存 capture，panic 后无法从中恢复变更。现在普通任务与自动续轮共用 `record_turn_checkpoint`，将同一份基线保存为已有的工作区 artifact；没有新增存储格式或恢复分支。已观察到的续轮在准入、获得工作区保护及开始 capture 后保存基线，再消费回答事件。

异常收尾仍先等待原资源退休，再用该轮持久化基线补回变更、记录失败并释放保护。基线保存失败保持原有的明确诊断，不借用上一轮基线或伪造恢复；不重发任务，也不修改用户文件或 Git 暂存区。此处记录的仍是现有 `workspace_interval` 范围，不声称覆盖 Core 观察续轮之前原生工作已经完成的写入。

新增回归用不同于前台轮次的文件内容建立续轮基线，panic 后确认退出前不恢复、退出后读取重新打开数据库中的变更 artifact，验证该轮修改前后的字节与终态顺序。另用实际私有 Git 仓库验证未暂存内容作为基线、原 capture 丢弃后恢复，以及用户 Git index 字节保持不变。

基线回归在旧实现下因未调用恢复而失败，修复后通过。最终 macOS Core 119 项、全部存储 190 项及现有 Git 持久化恢复 1 项通过；Linux 对应 Core/自动续轮/异常清理/多目录/实际 Git 相关 140 项通过。两平台 Core/存储/工作区 all-targets 严格 Clippy 及 Windows GNU 同三包严格静态交叉检查通过。证据：`/tmp/dogcodes-auto-checkpoint-before.log`、`/tmp/dogcodes-auto-checkpoint-macos.log`、`/tmp/dogcodes-auto-checkpoint-strict-macos.log`、`/tmp/dogcodes-auto-checkpoint-linux.log`、`/tmp/dogcodes-auto-checkpoint-windows.log`。Linux 只在本轮临时验证容器内安装 Git，容器退出即删除，原三个开发服务容器未改动。

## 本地验证

- 四项新增 SQLite/Core 回归覆盖失败事件、传输错误、EOF、panic、回答存储故障、关闭报错后等待原证明，以及终态存储故障。确认原资源退出前运行/工作区仍被保护，关闭只调用一次，替换 Driver 不受影响。
- 扩展 Claude 私有热复用测试，确认原句柄 ACK 不影响后继 scope；旧实现下该断言失败，修复后通过。
- macOS：Core 119、Driver API 29、Claude 68、全部存储 188 项通过；Claude 原有 8 项按需测试未启用。全工作区 all-targets 严格 Clippy 通过。
- Linux：同三包 216 项，存储自动续轮/准备取消/异常清理/审查相关 62 项通过，四包严格 Clippy 通过。
- Windows GNU：同四包 all-targets 严格静态交叉检查通过，未执行 Windows 二进制。

最终日志：`/tmp/dogcodes-autonomous-final2-macos.log`、`/tmp/dogcodes-autonomous-strict2-macos.log`、`/tmp/dogcodes-autonomous-final2-linux.log`、`/tmp/dogcodes-autonomous-final2-windows.log`。测试仅使用私有流、SQLite 及进程夹具，没有调用安装的 Agent/模型、操作实际桌面、启动手机设备或切换日常 Core。未提交推送。

## 边界

本次保证的是同一 Core 生命周期内已获身份的原运行；自动续轮已补齐现有持久化工作区检查点，基线本身不可用或无法保存时仍明确报告变更不可恢复。无身份未知启动、未标识回调、其余辅助事务、Core 强退/重启及持久化故障的完整退出记账仍按 CURRENT_WORK 继续，不将本轮视为整个工作包完成。
