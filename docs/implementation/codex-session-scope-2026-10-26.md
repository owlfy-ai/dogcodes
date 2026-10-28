# Codex 会话归属与原进程退出观察

日期：2026-10-26。当前代码按尚未发布的软件处理，不提供旧句柄兼容或数据迁移。本记录只收口 Codex 的这一阶段，不代表全部 Driver、恢复路径或远控目标已完成。

## 问题与结果

此前 Codex 只按原生 thread ID 查找 actor。即使 Core 已保存原始句柄，该句柄仍可能命中同名恢复或热复用后的 actor；提前取得的命令发送器也可能把取消、审批或关闭留在队列中，随后作用于新任务。关闭时记录不存在也曾直接视为成功，缺少原资源的确认。

现在创建、冷恢复和每次空闲热复用都会产生新的运行期 scope。热复用在 actor 确认空闲的同一处理步骤中更新归属；活动轮次拒绝复用。没有接收者的排队健康检查不会改变归属。模型/思考强度的现有配置命令使用这次返回的句柄。

Prompt、取消、审批、问答、配置、健康检查和关闭都携带原 scope。入口校验之外，actor 在取出每一项排队命令时再次核对，拒绝旧归属。句柄的序列化内容不包含运行期授权，单凭原生 ID 构造的句柄不能操作会话；这不是旧版本兼容分支。

`current_session_handle` 只读取当前已有归属，不创建或恢复进程。关闭完成后保留该归属的内存回执，以支持同一已确认关闭的重试；同名新归属存在时，旧关闭仍然被拒绝。迟到的旧关闭确认按原命令通道核对，不移除替换 actor。

## 退出证明

`shutdown_proof` 在关闭前捕获原 `NativeProcess`，只观察其 supervisor 的退出结果，不查找后来的同名会话，不发送原生控制请求，也不启动恢复。观察超时不会取消原关闭或重发关闭。

临时清理错误、命令通道关闭、监督通道丢失均不算退出；只有原进程作用域和原输出收尾确认完成，证明才会完成。Core 已有的单次关闭观察因此可在 Codex 关闭返回错误或等待超时后，继续保留原 Run/工作区占用，等待原资源的真实结果。

## 本地验证

五项新增回归覆盖：持有旧发送器后热复用且新任务已开始，旧的七类命令全部拒绝；取消排队的复用不更换归属；原退出证明跨冷恢复仍只观察原进程；临时错误不会提前完成证明；监督通道丢失不伪造退出。已有冷恢复夹具改用实际返回的新句柄，不再使用旧句柄继续新 actor。PID 记录使用写完后的原子发布。

使用私有 Python app-server、内存控制通道和临时目录；没有运行已安装的 Agent/真实模型，也没有切换日常 Core 或操作真实桌面输入。未提交或推送。

- macOS/Linux：Codex 各 **52 项**通过，三项需明确授权的测试未启用。
- macOS：Core **98 项**、desktop **347 项**与相关存储集成 **26 项**通过；全工作区 all-targets 严格 Clippy、格式和差异检查通过。
- Linux Codex all-targets 严格 Clippy 及 Windows GNU all-targets 严格交叉检查通过。Windows 的首次检查发现仅 Unix 测试使用的辅助方法缺少平台条件，补齐条件后通过，未关闭警告。

验证日志：macOS Codex `/tmp/dogcodes-codex-scope-final-macos.log`，Core/桌面/相关存储集成 `/tmp/dogcodes-codex-scope-core-macos.log`，严格检查 `/tmp/dogcodes-codex-scope-clippy-final-macos.log`；Linux `/tmp/dogcodes-codex-scope-linux.log`；Windows GNU `/tmp/dogcodes-codex-scope-final-windows.log`。Windows 是交叉静态检查，未运行 Windows 二进制。

## 仍需继续

Claude 及其余 Driver 的会话归属、排队请求和原生退出合同仍需逐个核对；没有为它们添加原生 ID 兼容绕过。原生启动阶段尚未得到句柄就被中断、缺少身份的回调、自动续轮/其它临时事务及 Core 强退/重启恢复仍未整体收口。

视频实机/弱网验证、HDR/光标/GPU 路径、PC/手机功能对齐、推送和部署验收继续按 `CURRENT_WORK.md` 的八包范围推进。

## 2026-10-27：模型目录初始化与原退出确认

Core 通过已注册 Driver Arc 查询模型目录，原 Codex 实现每次直接启动独立 app-server，没有目录准入。私有脚本阻塞第一项 model/list 后，第二项仍在其存活时完成；旧代码失败证据 `/tmp/dogcodes-codex-metadata-before.log`。

现在每个 CodexDriver 只准入一项目录探测，排队取消不创建后台任务。将原 Connection::spawn 拆为同步 create 与异步 initialize；普通调用仍由 spawn 依次完成两者，保留原握手内容、单请求十秒期限及退出行为。目录监督在 initialize 前就捕获原 NativeProcess：调用方取消、三十秒预算或查询 panic 都能停止原初始化/读取，先关闭原 stdin，再 request_close 并等待同一个 native.confirmed。准入、结果和失败均不越过物理退出证明；原生两秒退出宽限不缩短，不重启探测、不发送 thread/start 或 turn/start、不碰普通任务。模型分页/重复 cursor 防护及解析规则保持。

复用 workspace 已有 futures 的 catch_unwind，将该包的 futures 从测试依赖移到运行时依赖；未引入新依赖包、协议、存储格式或迁移。新增五项真实私有进程回归：并发探测不重叠及替代启动时旧 PID 消失；初始化期间取消不会继续 model/list；失败获证后可再查；排队取消不留迟到启动；另一普通 turn 运行中目录仍能读取且原进程不被配置/停止。两页目录的默认模型、思考选项与完整条目一并验证。固定 Python fixture 用私有数据目录中的硬链接运行，避免可执行文件写入/并行 fork，并已忽略临时目录。

macOS/Linux Codex 各 57 项普通库用例和各 6 项私有产品流程通过；三项真实环境库用例及一项真实审批集成未启用。macOS desktop 350 项通过，8 项按需未启用；Linux desktop 最终 350 项通过，11 项未启用。两端 Codex/desktop all-targets 严格 Clippy、Windows GNU 同两包静态严格检查均有成功输出；未运行 Windows 二进制。

验证中的环境/夹具问题单独记录：Linux 首轮桌面一项 Kiro 假更新命令无法启动，未保留 OS 错误码，不能据此认定产品更新器或具体 errno 有问题。仅将该测试改为固定 hard-link shell、独立 index/beta 数据文件，保留只读检查、70 KiB 索引、非交互更新标记和测试频道拒绝断言；Kiro 生产逻辑未改，最终 Linux 全量通过。macOS 首轮复核遭磁盘满而编译失败；确认没有活动 cargo/rustc 后，仅删除本任务 target 下三个旧 desktop 增量缓存，保留源码、可执行文件和全部日志，再次完整 350 项及严格检查通过。缓存记录 `/tmp/dogcodes-codex-metadata-cache-cleanup.log`。

Linux 上一次运行虽有 Cargo 成功输出，Docker 收尾元数据库 I/O 错误使运行器退出 125，该次记录仍保留为失败。用户随后明确授权重启 Docker Desktop；官方 restart 的停止阶段超时后，只结束已核实路径仍属于 Docker.app 的残留进程，再用官方 start 恢复。没有删除或重建容器/数据卷；原 server、PostgreSQL、Redis 三个容器保留，启动原数据库容器后全部运行，PostgreSQL/Redis healthy，服务 /healthz 返回 200。原测试容器已不存在。恢复后重新运行 Codex 全包测试、desktop 库测试及两包 all-targets 严格检查，整条 Docker 运行器退出 0：Codex 57 项库测试及 6 项私有产品流程、desktop 350 项通过。该轮仍有磁盘满遗留的 Rust 增量缓存告警，针对旧零长度文件的检查未发现残留，但 Cargo 缓存仍重放旧诊断；随后仅清理隔离 Linux target 的 desktop/codex-driver 两包构建产物并重跑严格检查，退出 0 且无告警。没有改源码来掩盖告警。

证据：`/tmp/dogcodes-codex-metadata-{after-macos,suite-macos,linux,windows,recovered-macos,strict-recovered-macos,final-linux,final-windows}.log`。恢复日志 `/tmp/dogcodes-codex-metadata-recovery-linux.log`，最终无告警严格检查 `/tmp/dogcodes-codex-metadata-clean-strict-linux.log`，Docker 残留进程处理记录 `/tmp/dogcodes-docker-recovery-20261027.log`。失败的初次 Kiro/磁盘/运行器记录均保留，没有用后一次输出覆盖失败。未运行实际 Codex/账号/模型、屏幕/设备或切换日常 Core，未提交推送。其它辅助事务、未知身份回调及原八包实机/网络/部署范围继续。


## 2026-10-27：子会话历史读取的辅助进程

Core 已按原轮次/安装定位历史；运行中的会话继续使用原 Driver。本轮检查的是该 Driver 内部的 thread/read helper，而不是改变历史归属。原实现直接启动新的 app-server；私有阻塞读取期间再次查询，第二 helper 会在第一棵树仍存在时启动并完成。反例先回收自己的进程再断言，旧实现确实记录两个 helper，日志 `/tmp/dogcodes-codex-history-before-macos.log`。

现在子会话历史按每 Driver 独立准入，和模型目录使用不同的锁。复用已有目录查询的受管执行函数：初始化前保留原 NativeProcess；排队取消不启动；开始后取消、超时、查询错误/panic 均关闭原 stdin 并等待原树退出确认，才释放该类查询准入或交付结果。不重发 thread/read，也不发送 thread/start/turn/start。原模型分页和历史条目/末轮状态解析保持；普通会话映射、Core 归属与原生两秒退出宽限不变。没有新增依赖、协议或迁移格式。

新增四项实际私有进程回归：读取期间取消及下一查询不重叠、初始化取消不继续读/重放、明确失败后的原树退出与下一读取、排队取消且模型查询仍独立；既有活跃 peer 测试增加读取历史，核实普通任务仍运行，辅助进程没有创建/配置普通会话。只使用固定 Python 夹具、私有目录与合成回复，没有运行已安装的 Codex/真实模型或切换日常 Core。

macOS/Linux Codex 各 61 项库用例及各 6 项私有产品流程通过，三项真实环境库用例和一项真实审批集成未启用；两端 desktop 各 350 项通过，分别 8/11 项按需未启用。两端两包 all-targets 严格检查通过；macOS 原 Core/SQLite 子会话历史归属集成 1 项通过。Windows GNU 同两包 all-targets 严格交叉检查通过，未运行 Windows 二进制。原八包及真实设备/工具链、macOS 原生创建超时继续，不能由 helper 回归推定完整远控或操作恢复已验收。

日志 `/tmp/dogcodes-codex-history-{after-macos,suite-macos,desktop-macos,routing-macos,strict-macos,linux,windows}.log`。Linux Docker 完整命令退出 0，自有验收容器退出清理，原三个开发服务保持运行。未提交推送。
