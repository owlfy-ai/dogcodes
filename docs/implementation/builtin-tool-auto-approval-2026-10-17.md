# 内置历史工具自动通过审批

日期：2026-10-17。用户明确授权：Agent 调用本项目内置工具时不需要用户审批，自动通过。

## 行为

统一 Core 执行流在创建 `approval.requested` 记录前识别当前活动历史服务的工具请求，通过本次 Run 所持有的原驱动和 native session 回复 Agent 提供的“允许一次”选项，不创建待用户确认卡片。策略适用于使用此执行流的各 Agent，未更改全局权限或各 Agent 的外部工具策略。

允许范围复用实际 MCP 工具目录的五项：`getTurns`、`searchHistory`、`getDecisionLog`、`getChangesSinceCheckpoint`、`getArtifact`。服务仍执行原有令牌验证、当前任务/冻结历史范围、参数校验、分页预算和租约撤销；自动授权不扩大查询范围。

识别绑定活动 `HistoryLease` 的真实服务名。支持原生限定名称 `mcp__<server>__<tool>`；Cursor 的展示名称或裸工具名必须同时有结构化 `providerIdentifier` 和 `toolName` 且精确匹配。显式元数据冲突、外部同名服务、前缀相似名称、未知工具或出现在任意参数中的工具名不会触发自动通过。没有活动租约时没有免审批路径。

仅使用本条请求原生提供且语义为 ApproveOnce 的选项，不选择持久/全局授权。无法识别身份或没有本次允许选项的非标准请求保留原审批流程；回复失败沿用执行错误处理，不宣称授权成功。审查已有的用户授权完全访问流程不变。

## 验证

- Core lib 首轮 86 项通过；随后收紧 Cursor 必须同时提供结构化服务身份的条件，相关 history_bridge 5 项再次通过。
- store `approval_receipts` 6 项通过，原用户审批的受理、来源绑定与投递规则保持。
- store `history_mcp` 最终 2 项通过：五项内置工具均回复原生允许一次（即便持久允许选项排在前面），不进入用户待审批列表；外部 MCP 同名工具、终端命令、无服务身份的裸名称仍需审批。另有既有冻结历史/分页/撤销回归。
- 共 94 个不同原生用例通过。首轮新增集成夹具在发送终态后没有关闭非后台驱动的事件流，导致收尾等待超时；修复夹具后只重跑受影响组，没有重复完整 Core 或审批回执组。
- 日志：`/tmp/dogcodes-builtin-tool-core.log`、`/tmp/dogcodes-builtin-tool-core-final.log`、`/tmp/dogcodes-builtin-tool-store.log`、`/tmp/dogcodes-builtin-tool-store-final.log`。最终命令 39005 exit 0。

无前端、数据库、依赖或协议变更；没有打包、调用真实模型或重启用户运行中的后台服务。开发程序下次编译启动后加载新后端逻辑，三平台实机和供应商真实调用仍按发布候选计划验收。
