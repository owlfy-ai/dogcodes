# 主会话停止请求的原目标与断连恢复

日期：2026-10-16。对应 DT-09，复用已有 CoreCommand 身份校验与只读回执。原生实现未改动，注册入口仍为 111 项（109 项生产入口）。

## 已核对的后台语义

Core.execute 在同一持久追加中写入取消受理事件与 Ack，非重复命令才启动 cancel_turn。cancel_turn_inner 再核对原 task/turn/run，随后调用对应原生 session 的 interrupt；投递结果通过 turn.cancel_sent 或 turn.cancel_failed 记录。最终退出继续由运行终态表述。审批也先写 approval.resolved 再异步通知驱动，并有 approval.delivery_failed 路径；客户端不能把这两类受理回执当作执行成功证明。

本轮只改造停止入口。审批/回答及普通发送的恢复仍是后续项。

## 实现

- CoreClient.cancelTurn 接受可选 operationId，新增可选 cancelTurnReceipt。Tauri 使用同一 cancelTurnCommand 构造发送/查询载荷，明确固定 taskId、turnId、runId 及原 expectedStreamVersion。复用 core_command_receipt，不新增另一套后台状态。
- cancelOperations 在发送前将原 TaskSummary、命令 ID 和请求时间保存到本机恢复记录；按 task/run 分开，后来的轮次不会覆盖前一轮记录。首次新请求保存成功后直接发送，避免流式输出推进版本时增加一次不必要查询。回包丢失先查原回执；显式恢复先查询，必要时仅发送同一身份、载荷及版本。不 bootstrap 后重新选取当前运行。
- 存储失败阻止发送；查询失败或无可核实指纹不自动重发。匹配 Ack 只结束对应受理跟踪，未找到 Ack 不推定失败。迟到回包不能删除更新的记录。
- App 保留最新已收到的流版本用于首次停止，并以同步 guard 防重复点击。原结果未知时，当前轮次停止按钮进入“停止结果待核实”，新轮次仍有独立停止入口。
- CancelRecovery 独立于普通输入框/审批面板显示。重开只展示保存记录，不自动执行。提供核实、重发原请求和显式结束跟踪；说明受理后仍需等任务退出，结束跟踪不撤回后台指令。回执让恢复行卸载后，仍可刷新当前任务；切换到其他任务后不调用旧行的刷新回调。
- 抽取 clientOperationLock，复用“开始修复”既有同 JS 队列及可用时的同源 Web Locks，未改变其受理规则。浏览器 Demo 提供仅内存的取消回执模拟及原目标检查，不能当作生产持久化证据。

不增加提示词、回答或凭据副本；没有数据库变更，也未改动发布签名配置。

## 验证

最终累计 47 个不同前端用例通过：cancelOperations 9、CancelRecovery 3、commandReceiptClient 4、AppStreaming 9、ApprovalComposer 3、demoClient 6、reviewRepairOperations 13。覆盖存储失败、丢失回包、重开、旧目标/原版本保持、多轮次独立记录、迟到回包、重复点击、首次发送不增加查询往返、恢复挂载不执行，以及高速流版本在 UI 批量更新前被用于首次取消。既有审批面板和修复恢复相关测试也通过。

第一批 46 项及类型检查全部通过。随后核对并修复恢复行因本地记录删除提前卸载导致刷新遗漏，只复验相关 3 项；再针对首次停止多一次查询造成额外版本漂移的问题调整发送顺序、增加 1 项用例，只复验受影响 21 项及类型检查，全部通过。diff 空白检查通过。

日志：`/tmp/dogcodes-cancel-recovery-frontend.log`、`/tmp/dogcodes-cancel-recovery-ui-final.log`、`/tmp/dogcodes-cancel-recovery-send-final.log`、`/tmp/dogcodes-cancel-recovery-types-send-final.log`。所有启动的验证进程均已收取终态，最后 exit 0。本工作块没有运行 Rust 编译、全套测试、打包或真实 Agent 验收。

## 仍需推进

此处解决客户端原停止请求的受理恢复，不保证各供应商 interrupt 必然成功，也不把后台服务重启或驱动失败后的处理等同于重新发送原命令。高频真实流中的版本冲突、已受理但投递失败后的用户反馈、跨平台原生退出仍须候选实机验证；不能为了绕过冲突自动换版本/换 run。

普通 submitTurn、respondApproval 仍未提供完整的客户端持久意图/恢复链路，自动上下文发送仍需复用分阶段规则。审批/回答需特别核对原 turn 归属与敏感回答的存储边界：QuestionSchema.writeOnly 已被 UI 用于密码输入，不能直接照搬普通客户端记录，将这些答案额外复制到 localStorage。下一步先核对审批受理/投递/终态及查询所需的最小恢复信息，再实现安全恢复。

Fork 多阶段副作用、其他共享状态与全域可靠性、三平台安装/退出/重启和源码发布审核仍未完成，不据本轮专项通过标记 GitHub 首发已就绪。
