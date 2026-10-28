# Core 替代会话与未启动任务的状态修复

日期：2026-10-26。工作包 2 的进展。应用尚未发布，本轮只修改当前事件格式，不增加旧数据迁移或旧协议兼容。

## 修复范围

改变模型/思考强度时，部分 Driver 需要关闭原会话再创建替代会话。如果替代创建返回 `selection_rejected`，原来的失败投影可能让已关闭的源映射继续保持 active。另一个问题是取消/准备失败发生在原生会话准入之前时，原本仍可用的闲置会话会被标为 detached，且历史游标推进到从未派发的输入之后。

现在的处理顺序：

- 在恢复/重配置前捕获原实际 SessionHandle 及其被动退出证明。证明是只读观察，成功恢复时丢弃它不会发起关闭。
- 不支持重配置时，先退休捕获的原资源并等待同一份正面证明，再持久化源映射的 resumable/detached 状态。之后才准备替代上下文并创建新会话。没有当前本地句柄的持久化源映射也不能在替代期间继续声称 active。
- 退休状态写入失败则不启动替代；取消已接受则不创建替代。原关闭回复尚未返回但原资源已获证退出时，可以继续收尾，不取消或重发关闭。
- 一般恢复错误也先退休捕获的原资源，再记录恢复失败。已明确验证回滚的 `selection_rejected` 和自主续轮忙保持原有处理，不关闭仍可复用的源。
- 每个 Run 增加单调的原生会话尝试标记，与可在移交/结算后清除的清理责任标记分开。只有从未进入原生会话准入的失败/中止/工作者异常才带 `nativeSessionUntouched=true`。
- 存储遇到这个明确事实时，保留整个源映射及其历史游标和最后使用时间；仍正常记录本轮失败、清除活动 Run 并推进任务事件流。不能仅保留 active 状态却推进未发送内容的游标。

退休状态事件不推进 native 历史游标，因此同一原 Run 随后登记替代会话仍使用原映射身份/游标比较和任务流 CAS；旧观察者不能替换新运行。

## 验证

新增 2 项 Core 单元测试及 8 项真实 SQLite/Core 集成测试，使用私有内存 Driver、临时目录及临时 SQLite：

1. 原退出证明在重配置后已无法重新查询，仍能被原清理接续；原关闭回复晚到且失败不能降级已获证退出。
2. 丢弃未使用的源快照不会关闭成功复用的会话。
3. 原生准入前取消保留源映射的全部字段；后续发送确实复用同一原生会话。
4. 工作区准备失败也保留未触及的映射及游标；修复工作区后能继续复用。
5. 原源已关闭、替代创建尚在等待时，退休状态已持久化；模型选择被拒不能恢复 active，随后重试成功。
6. 替代成功时退休记录先于新会话记录，身份 CAS 不冲突，实际 prompt 完成。
7. 普通 warm resume 失败退休原缓存资源，后续发送能重建。
8. 重配置移除原查询表项后，不能以“查不到”代替退出证明；仍等待原证明，且取消不新建替代。
9. 已无本地句柄的持久化源遇到替代拒绝时，也不会继续 active。
10. SQLite trigger 拒绝退休状态写入时，不创建替代；最终失败保留真实资源状态，恢复存储后可重试。

macOS/Linux Core 各 110 项、macOS desktop 347 项（8 项按需测试未启用）及全部存储 175 项通过；Linux 相关存储集成 42 项通过。macOS 全工作区、Linux Core/存储/Driver API 和 Windows GNU 对应三包 all-targets 严格检查通过。Windows 是静态交叉检查，没有执行 Windows 二进制。

证据：`/tmp/dogcodes-session-state-final-macos.log`、`/tmp/dogcodes-session-state-clippy-macos.log`、`/tmp/dogcodes-session-state-final-linux.log`、`/tmp/dogcodes-session-state-strict-linux.log`、`/tmp/dogcodes-session-state-final-windows.log`。Linux 行为日志末尾的测试夹具 default 写法 lint 已修复，严格检查最终结果以单独的 strict 日志为准。格式及差异空白检查通过。

没有运行安装的 Agent、真实模型、桌面采集/键鼠/剪贴板、手机设备/模拟器或切换日常 Core。三个已有开发服务容器保持运行；没有提交或推送。

## 仍须继续

本轮不宣称跨 Core 强退/重启恢复完成，也不把没有身份的未知启动判为已退出。原生未标识回调、自动续轮、辅助启动/临时事务、持久化启动回执及退出记账继续推进。

真实平台和设备上的视频恢复、HDR/光标/GPU、PC/手机功能与恢复路径对齐、网络/推送/部署仍按 `CURRENT_WORK.md` 八个工作包保留。

## 同一 Agent 不同安装实例的检查点隔离

随后核对重启路径发现，`turn.completed` 和失败/断线共用的 `update_active_session_checkpoint` 仅以 task/provider 过滤，能同时推进同一任务下其它安装实例的历史游标，甚至将其标为 detached。两处 SQL 现增加已有 installation_key 条件，与当前 active executor 的完整身份一致；没有新增表、格式迁移、恢复队列或协议。

新增 `crates/store/tests/session_checkpoints.rs` 四项真实 SQLite 回归，覆盖完成、失败、可复用失败及重新打开数据库后的 Core 启动恢复。每项分别验证当前 ExecutorRef 有/无安装 ID，保留同 provider 的另一安装实例、无安装 ID 实例及其它 provider 的完整映射；验证投影重建后结果一致。旧 SQL 下四项均因修改其它实例检查点而失败，修复后通过。

macOS 全部存储 184 项通过；Linux 存储 lib、准备取消、异常清理和新专项共 101 项通过。包级 all-targets 严格检查在 macOS/Linux 及 Windows GNU 交叉目标通过；Windows 没有运行二进制。Linux 首轮镜像缺 Git，两个既有恢复夹具初始化失败；补 Git 的临时容器完整重跑通过，没有更改生产代码或跳过它们。格式及差异检查通过。

日志 `/tmp/dogcodes-installation-checkpoints-{before,after,tests,clippy,linux-with-git,windows}.log`。仅使用私有数据库/目录和既有本地夹具，三个开发服务容器保持运行，没有操作日常 Core、真实 Agent/设备或提交推送。此修复不代表未知原生启动、外部进程或整体强退恢复已经完成。
