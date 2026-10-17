# 独立 Core 后台服务

2026-09-12 开始实施，依据用户确认：桌面退出或被硬杀，不应关闭 Coding Agent 的协议连接；后台服务也作为未来硬件网关的运行基础。

## 生命周期与边界

- 正常桌面通过本地 IPC 接入独立 Core 服务。Core、数据库 lifetime lease、Driver、Agent stdio、审批等待、历史 MCP 和轮次变更采集均由服务持有。
- 当前以同一个打包可执行文件的内部 `--core-service <data-directory>` 模式启动，不新增面向用户的 CLI，不依赖开发工具启动。Unix 子进程使用独立进程组和空标准输入输出；桌面不持有 kill-on-drop 子进程句柄。
- 退出桌面保留后台服务。再次打开使用同一数据目录发现并连接原服务，不重建正在运行的会话、不重新发送 Prompt，不因 UI 离线产生 `turn.connection_lost`。
- 真正的服务崩溃或系统重启仍走异常恢复；不得把“服务也被杀”描述为可无损继续。这一分支与 UI 退出不同，旧本地轮次自动释放，显示连接中断并允许直接继续；外部结果不伪造为成功，继续时由 Agent 结合历史检查现状。
- 已存在的 disconnected 记录不会因引入服务被伪造为完成。后台执行任务的完成必须来自原生轮次终态，不能仅凭一个 Shell 文件推断整轮完成。

## 本地连接

- 首版使用仅绑定 IPv4 loopback 的 TCP，长度前缀 JSON 帧、显式协议版本、随机连接令牌、64 MiB 帧上限及连接数上限。
- `service.json` 原子发布地址、PID、版本和令牌；macOS/Unix 通过私有临时文件创建，权限 0600。数据库文件锁确保同目录只有一个服务拥有者；启动竞争失败的进程不恢复或接管活动数据库。
- 桌面 RPC 是明确的方法白名单；服务端拒绝内部探测/测试写入命令，并强制普通命令使用 LocalUser。此连接面向本机同用户桌面，不作为未来硬件接口暴露。
- 已接收的命令执行不随客户端 socket 断开取消；不自动重试可能已经提交的变更。命令沿用 Core 的幂等键、流版本和审批身份校验。
- 事件订阅先回放再实时推送；序号用于去重和补齐。桌面重开重新 bootstrap/subscribe（灵动岛/悬浮岛已于 2026-10-16 移除）。
- 桌面的审阅/丢弃/恢复操作仍需 Core 的工作区排他锁。服务持有锁，客户端通过存活的 lease socket 保持占用；桌面硬杀关闭 socket 后自动释放。

## 硬件扩展

未来 Device Gateway 接入同一 Core 和 CompanionProjection，独立实现配对、scope、USB/Wi-Fi、重连与设备审批能力裁剪。桌面 IPC 的完整方法白名单和令牌不得直接提供给硬件。

## 验证与当前限制

2026-09-30 增加服务独立持有的诊断文件，记录启动阶段、就绪/退出、失败及 panic。采用脱敏、大小限制和轮转，现有问题报告可导出有界尾部；不依赖桌面保持 stderr 管道连接。详细范围与本轮验证见 [运行稳定性记录](runtime-stability-2026-09-30.md)。

- 6 项新增通信/生命周期回归通过（桌面 Rust 全部 31 项通过），覆盖丢失回包后的命令完成及幂等、事件游标重连、写入锁随连接释放、认证/版本/帧限制、单实例锁及空闲退出与工作区互斥。
- 真实 Codex/Kimi 已通过：原生单次审批后写 START、sleep 12，在 END 前硬杀前台 RPC 客户端，两家均继续写 END、返回最终回答并记录 turn.completed；服务 PID 不变，无 turn.connection_lost，待审批请求可由新客户端读取。结果记录在 `experiments/daemon-acceptance/2026-09-12`；只有 results.json 成功结果计为通过。测试前台为独立 RPC 客户端进程，不冒充 Tauri GUI 全链路。
- 旧 `review-acceptance` feature 的历史 GUI fixture 暂保留内嵌 Core，仅用于原有审阅测试；它不是后台服务验收入口。默认产品构建使用独立服务。
- 当前不注册开机启动；服务随首次启动桌面按需启动。桌面启动比较可执行文件标识，兼容协议的旧服务空闲时自动退出并启动新服务；有任务或工作区写入占用时保留原服务。尚无独立的服务管理 UI；真正服务异常恢复已取消人工确认门槛，详见 [状态一致性](execution-state-consistency.md)。

前端回归：198 项通过，TypeScript 类型检查与前端构建通过。Rust 桌面 31 项通过，包含 6 项服务回归；默认构建、workspace all-targets 与 review-acceptance all-targets 检查通过。并行历史 MCP 接口补齐后已重新验证。

## 真实桌面与灵动岛验收

使用默认产品服务架构与 `custom-protocol` 内置前端资源构建，独立 `.app`、独立数据目录与工作目录，没有启动 Vite 或占用 1420。

- Kimi 从主窗口提交一条真实命令；待审批时 SIGKILL UI PID 71036，服务 PID 71045 保持，原审批 ID/选项/轮次不变，命令尚未执行。
- 重开 UI PID 72872，灵动岛显示原审批的完整命令；人工点击允许一次。
- `gui-start.txt` 出现且 `gui-end.txt` 未出现时，精确 SIGKILL UI PID 72872。后台继续等待命令完成、采集文件变更、记录最终回答。
- 再开 UI PID 74422，灵动岛明确显示“已完成”；点击任务进入主窗口，显示 `GUI_DAEMON_CONTINUED_OK`、终端完成及两个文件的本轮变更。输入“继续”后发送按钮可用，无手动状态确认门槛；测试草稿已清空，没有发出额外轮次。
- 事件仅有一次 turn.queued，没有 turn.connection_lost；正常退出最终 UI 后服务仍存活。最后通过空闲退出接口关闭本次隔离服务，核对 UI 与服务 PID 均已退出。

证据：`experiments/daemon-acceptance/2026-09-12/gui-results.json`、`gui-events.json`、`gui-timeline.json`。这次 GUI 代表用例为 Kimi，Codex另有真实 RPC 客户端硬杀完成验收；不扩展为八家逐一 GUI 全矩阵、发布签名包或 Windows/Linux 通过。

## 会话删除/恢复补充验收（2026-09-25）

默认服务构建已通过删除/恢复的双桌面同步、主任务/独立审查门禁、应用与服务重启、历史材料保留和丢失回包后的幂等验证。未修改产品代码；完整证据与边界见 [验收记录](conversation-deletion-daemon-2026-09-25.md)。
