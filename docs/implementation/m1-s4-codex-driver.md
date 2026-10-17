# M1-S4：Codex App Server Product Driver（Beta）

更新日期：2026-09-03

## 当前结论

Codex 已不再只是 M0 探针：桌面现在可以为通过真实 `initialize + account/read` 的本机安装
注册 `codex-driver`，并把具体 Installation 作为下一轮执行者。Adapter 已实现
Thread 创建/恢复、同进程 Session 复用、Context+Prompt 有序交付、Turn 流式事件、工具摘要、
终态和 interrupt Port；Core/Tauri/React 也已接通精确到 task/turn/run 的停止操作。当前仍是
Beta，尚未在本轮消费模型额度执行真实 Turn。

## 代码入口

| 文件 | 职责 |
|---|---|
| `crates/codex-driver/src/lib.rs` | 有界 JSONL、App Server 进程/Session actor、协议映射与 AgentDriver 实现 |
| `crates/codex-driver/tests/product_driver.rs` | 合成真实子进程的 account/Thread/Context/Turn/审批拒绝端到端回归 |
| `crates/codex-driver/src/bin/codex-driver-fixture.rs` | 只供上述测试使用的合成 stdio peer，不调用模型 |
| `crates/driver-api/src/lib.rs` | `SessionRequest.workingDirectory`、精确 Executor/版本/同进程复用契约 |
| `apps/desktop/src-tauri/src/lib.rs` | 安装扫描后就绪探针、按 Installation 注册 Driver、Ready/NotConfigured/Broken 状态 |
| `apps/desktop/src/App.tsx` | 只列出 Ready Installation，切换时保留 installationId，显示真实能力降级与额度 Unknown |

## 实现语义

- Driver 实例绑定 `ExecutorRef(providerId=codex, installationId=ins_…)`，Core registry 也以完整
  ExecutorRef 为键；两个 Codex 版本不会再按 providerId 相互覆盖或被 UI 丢掉安装身份。
- `create_session(SessionRequest)` 在发送任何 Turn 前校验绝对工作目录，并以该目录执行
  `thread/start(ephemeral=false, sandbox=workspace-write, approvalPolicy=never)`。用户在桌面显式
  提交 Prompt 才会触发；就绪扫描不会创建 Thread 或消费模型额度。
- 每个 Thread 由独立 actor 持有 stdin/stdout/stderr 和有限命令队列。JSONL 单帧 64 KiB、
  request 10 秒、interleaved 256 帧；stderr 只 drain，不读取/保存正文，也不与 stdout 混流。
- 启动只继承运行 Codex 所需的现有 PATH/home/temp 定位变量，不读取 credential/config 文件，
  不接受 API Key。`account/read(refreshToken=false)` 只判断 account 是否存在或是否无需 OpenAI
  auth，response 随即丢弃，不保存 email、plan、token 或 account payload。
- Context Artifact 按数组顺序转换为 text input，每项包含已审阅 kind/hash/content，全部位于
  用户 Prompt 之前的同一 `turn/start`。非 UTF-8 Context 或总帧超限明确拒绝。
- 只映射固定字段：agent message delta、command started/completed、Turn status。命令正文、
  warning/error 原文、未知 method/payload 和原始 Diff 不进入 Canonical Event；Diff 继续由
  Workspace Snapshot/按需 Artifact 作为事实源。
- 当前 PendingInteraction 尚未落地，所以 Thread 默认 `approvalPolicy=never`；若某版本仍发出
  command/file approval，Driver 只返回其官方 `decline`，其他 callback 返回固定 method-not-
  supported。绝不 approve-all，UI 明示“安全拒绝”。
- 已有真实证据的 `0.144.5` 声明跨进程 resume Supported。其他版本保持 Unknown，但同一桌面
  进程内保留 live Thread 供连续 Turn 使用；重启后只做有界恢复尝试，失败转为 missing/detached，
  不把尝试成功与否预先写成 capability。

## 当前证据

- 合成 App Server 真实子进程验证：initialize、account/read、thread/start、Context 先于 Prompt、
  server approval 收到 `decline`、text delta、Turn completed、Session actor 复用和 shutdown；
  另验证 interrupt response 前夹带的 interrupted 终态不会被 JSON-RPC request 丢弃。
- `CancelTurn` 要求 expected stream version 与当前 task/turn/run 完全匹配，Canonical 先记录
  requested，再调用该 Run 的具体 Installation Driver；停止中禁止重复提交。合成 Driver 验证
  interrupt 调用、终态、Projection 和从 Event rebuild 一致。
- 真实 `/Applications/ChatGPT.app/Contents/Resources/codex` 当前为 `0.153.0-alpha.5`；桌面启动
  的 initialize + account/read 在 32 ms 内通过，SQLite Installation 状态从 Broken 恢复为
  Ready，`quick_check = ok`。没有创建真实 Turn、输出 account 数据或读取凭据。
- 对该版本使用隔离临时 `CODEX_HOME` 重做零 Turn 跨进程 resume：thread/start/name-set 响应
  成功，但没有形成 rollout，第二进程返回 `-32600/not_found`。因此 resume 仍为 Unknown，
  不沿用旧 `0.144.5` 结论。
- 当前全量验证：Rust 112 passed、1 ignored；fmt/clippy 通过；前端 25 passed，typecheck/build
  通过。桌面 readiness/数据库 smoke 通过，开发进程已主动停止。

## 明确未完成

- 尚未由操作者在桌面提交一个真实 Codex Turn，因此当前版本的真实 streaming/tool/write/
  terminal 状态仍未验收；合成 peer 只证明 Adapter 契约，不证明供应商 runtime 行为。
- Core/UI cancel 已接线，但还没有对本机 `0.153.0-alpha.5` 的真实模型 Turn 做 interrupt 验收；
  session 尚未 ready 时取消会留下稳定 `turn.cancel_failed`，而不是猜测 PID 或误杀进程。
- PendingInteraction 的先落库、原生 option/version、幂等响应和过期处理未完成；在完成前所有
  提权请求只能被 runtime policy/Driver 拒绝。
- 账户级 Usage/Quota 已由独立 QuotaProvider 接通；完整 Diff notification、terminal chunk、
  server question/form 尚未映射。额度边界见 `m1-s6-quota-monitoring.md`。
- 进程组/Windows Job、ownership token、空闲回收、有限重连退避、过载 jitter 和孤儿恢复
  仍需 Runtime Supervisor；当前同进程 live Session 可能占用一个子进程直到应用退出。
- 当前只有 Codex Product Driver 进入 Beta。ACP/Cursor 与 Copilot 的真实 Adapter 仍是下一批，
  不能把 M0 fake contract 或 SDK 编译通过当成已接入。

## 下一 agent 接手

1. 在用户明确同意消耗一次 Agent 额度后，从桌面选择 Ready 的具体 Codex Installation，提交
   固定 no-tools Prompt，核验 Canonical `session.ready → driver.turn_started → message.delta →
   turn.completed`、SessionMap/native version、工作区起止 Snapshot 和第二 Turn live 复用。
2. 用上述真实 Turn 验证停止按钮产生 `cancel_requested → interrupt_sent → interrupted`；若目标
   已完成或 Task 版本变化，必须保持 Conflict/失败事件，不能直接从 WebView 调 Driver 或按 PID
   杀进程。
3. 实现 PendingInteraction 后将 approvalPolicy 调整为经版本契约验证的交互策略；必须先落库
   再展示，只回传供应商 option，未知 form 不伪造/自动批准。
4. 将通用 stdio actor/有界传输下沉为 Runtime Supervisor 后开始 ACP Product Driver；保持每个
   Agent/Installation 独立版本与 capability，不复制 Codex 的 resume/approval 结论。
