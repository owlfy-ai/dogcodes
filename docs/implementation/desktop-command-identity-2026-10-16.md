# CoreCommand 幂等请求的载荷绑定

日期：2026-10-16。对应 DT-09；这是命令受理身份的基础改造，不等于所有命令的执行结果恢复已完成。

## 问题与实现

Core.execute 原来命中 command_ack 后直接返回。两个不同命令、不同正文/审批选项或不同版本条件如果复用了同一 idempotencyKey，后来的请求可能拿到前一个操作的成功回执。只在 Core 层加预查还不足以处理两个请求同时查不到回执的情况。

现在将 schemaVersion、actor、expectedStreamVersion 和完整类型化 CoreCommand 载荷规范化成稳定 JSON，再计算版本化 SHA-256 指纹。对象键排序，数组保持原顺序。commandId 和 issuedAt 属于投递信息，不参与指纹；idempotencyKey 仍作为查找键。每次重试必须保留原命令内容和版本条件，不能用新的前置版本冒充原请求。

AppendRequest 增加可选 request_fingerprint，Core.execute 的唯一受理追加入口传入指纹；后台内部事件追加显式传 None，保留其既有独立协议。CommandAck 增加兼容旧数据的可选 requestFingerprint，直接随现有 command_dedup.ack_json 落盘，无数据库结构变更、迁移或数据清理。仅保存摘要，不新增原始提示词、审批正文或凭据副本。

Core 的快速回执路径先比较指纹；SQLite 的 append 在同一事务中再次比较，然后才返回 duplicate。因此并发请求不能绕过内容校验。绑定命令也不能碰撞到没有指纹的内部操作。不同指纹明确拒绝，不执行新命令。两边都没有指纹的后台内部追加仍使用其既有处理，不被此处宣称为已统一绑定。

旧回执无指纹时可继续读取，但 Core 拒绝猜测它是否属于本次重发，提示先核实原操作结果；不会补造原载荷或清库。永久删除后的新请求回执也保留指纹，且删除清理失败后的同请求重试仍可继续完成清理。

## 验证范围

本工作块集中验证 Core、SQLite 和 loopback RPC，没有打包或真实模型调用。已启动的原生验证串行使用同一构建目录；曾等待另一个存活的工作区 cargo check 释放锁，未重复启动，也未终止该外部检查。

最终共 86 项原生用例通过：

- `cargo test -p agent-core --lib`：76 项通过。
- `cargo test -p agent-store --test command_identity --test task_lifecycle`：4 项身份测试及 4 项永久删除/清理测试通过。
- `cargo test -p agent-store --lib deduplicates_commands_and_enforces_stream_versions`：1 项通过。
- `cargo test -p desktop --lib command_identity`：1 项 loopback RPC 测试通过。
- 格式和 diff 空白检查通过；修改 AppendRequest 调用点时曾误在一个测试辅助函数签名后插入字段，格式检查发现后已在启动编译前修正，未留下无效源码。

覆盖原回执跨数据库重开、不同投递信息仍可重试、载荷/actor/版本/命令类型变化被拒绝、事务内冲突只受理一次、旧回执不猜测，以及永久删除/清理重试。日志：`/tmp/dogcodes-command-identity-core.log`、`/tmp/dogcodes-command-identity-store.log`、`/tmp/dogcodes-command-identity-dedup.log`、`/tmp/dogcodes-command-identity-rpc.log`。唯一验证进程链已收取 exit 0，无待等待的构建。

## 剩余工作

这只绑定已经受理的命令，不提供完整原请求查询或执行终态查询。客户端仍需要先持久保存意图，回包丢失后核实回执；下一工作块优先处理“开始修复”的 SubmitTurn（包含选中的问题、原版本条件，以及主 Agent/上下文切换的独立步骤）。现有更新前置版本再重发的调用需要改为查询原操作，不能为了维持旧重试方式跳过指纹比较。

通过确定性核心测试不代表所有 18 个命令变体的外部副作用都具有一次执行保证。审批传给供应商、进程启动和上下文切换等仍需逐域核实。后台内部 append、Fork 多阶段恢复、统一订阅、Agent 准入和三平台实机验收继续按矩阵推进。注册 handler 数未变，仍为 110（108 个生产入口）。
