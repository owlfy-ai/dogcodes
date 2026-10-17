# 审查启动请求：跨窗口核实与恢复

日期：2026-10-16。对应 DT-09，覆盖项目审查、逐轮审查及修复后复审的启动入口。

## 问题与实现

三个入口原来都只在组件 ref 中保存待确认 runId。卸载后重试会生成新标识；切换 Agent、Diff 快照、审查类型或复审轮次也可能替换原请求。后台已经按完整 ReviewRequest 比较并返回已有 run，但客户端丢失身份后无法利用这层保护。

新增 reviewStartOperations 与共享 ReviewStartRecovery。按 taskId 和入口范围持久保存完整请求，保存失败不调用后台。启动回包丢失时通过现有 turnReviews 查询原 runId，并比较 taskId、turnId、kind、executor、retryOf、sourceSnapshot 和 recheckOf 后才解除待核实状态。可选字段的 null 与省略按同一语义比较；不同载荷的回执不会被当作原操作结果。

关闭重开后显示待核实项，不自动重发。原请求未找到时提供显式重发，保留原 Agent、范围、来源和标识；不读取新的选择来替换请求。原快照已不可用且没有已受理记录时会保留错误，由用户核实后结束本机跟踪再创建新审查。结束跟踪不取消后台运行。已受理的运行、失败或中断结果都按真实状态显示，不因查到结果改成完成。

新请求与重试按钮在存在同入口待核实项时禁用；恢复按钮不依赖当前 Diff 或 Agent 列表，因此切换范围/当前 Agent 不会丢掉原操作。三个入口共用本机存储通知，不覆盖其他 task/scope 的记录；迟到回执只移除对应 runId，保留新的待核实请求。

后台路径经代码复核：Core start_review 先按 runId 找历史、比较完整 ReviewRequest，再检查驱动；项目审查服务在已有 runId 时跳过重新取得 Diff 材料，让 Core 核实原请求。本工作块复用这两条路径，没有改变原生代码或添加 handler。

## 验证

集中执行相关前端验证，未调用 Rust 编译、模型或打包。

- 最终 113 个不同用例通过：TurnActions 9、ProjectReviewActions 10、ReviewResult 14、ReviewPanel 70、reviewStartOperations 10。
- 新覆盖关闭/重开、Diff 刷新后按原范围恢复、创建后回包丢失、修复后复审保留原 Agent 和关联轮次、存储失败不发送、回执载荷冲突和迟到回执保护。
- 首轮 110 通过、3 失败：一个新复审夹具缺少让修复控件显示的 submitReviewFix 方法；两个既有 Git 面板用例仍模拟旧的直接 push 路径，未提供当前 gitOperations.advance。补齐夹具，并按当前后台组合工作流断言接续调用后，复验相关 94 项全部通过。没有把旧模拟路径当成当前产品行为。
- 初次类型检查发现新测试省略了类型中必填的 installationId；修正普通夹具，并仅在明确模拟历史线上省略字段时构造原始 JSON。最终类型检查及 diff 空白检查通过。
- ReviewResult 的既有“主 Agent 选择”测试仍有 React act 警告，断言通过；这是测试夹具异步收尾的待整理项，不把无失败等同于测试输出完全干净。
- 日志：`/tmp/dogcodes-review-start-recovery-frontend.log`、`/tmp/dogcodes-review-start-recovery-frontend-final.log`、`/tmp/dogcodes-review-start-recovery-types.log`、`/tmp/dogcodes-review-start-recovery-types-final.log`。相关进程均已收取退出状态。

## 未完成与下一步

“开始修复”会在主会话提交新的 SubmitTurn，并可能先切换主 Agent/准备上下文；它仍使用组件内 operation ref，不能直接复用独立审查的回执语义。下一步须持久保存完整修复意图并核对 CoreCommand 的原请求绑定和状态查询。此次检查发现 Core.execute 命中 command_ack 后未统一校验完整命令载荷；同日后续工作块已补齐指纹及事务内校验，见[命令身份记录](desktop-command-identity-2026-10-16.md)。修复提交的客户端恢复仍须继续完成，不能只靠客户端保证。

审查状态查询仍为已有列表/轮询，长历史开销和统一订阅待收敛。独立审查的真实后台退出/重启、跨平台模型异常和主会话/审查并发验收也未由本次前端测试覆盖。Fork 的多仓库中间阶段恢复、问答跨入口准入及其他发布门槛继续按总清单推进；DT-09 不标完成。handler 数仍为 110（108 个生产入口）。
