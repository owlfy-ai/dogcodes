# 项目分析与提交总结的临时会话接续

> 后续：审查启动取消与关闭确认已复用同一临时会话管理，见 [有限修复记录](review-lifecycle-2026-10-26.md)。下文该项待办由此更新；其余原有范围继续，按实际证据和用户操作影响排优先级。

日期：2026-10-26。工作包 2 的进展；只修改当前实现，不做旧版本兼容或数据迁移。

## 问题与处理

项目运行配置分析原本已独立持有启动工作者，但内部仍用 30 秒 timeout 包住原生创建；预算结束会丢弃创建未来。提交说明生成则直接等待创建、prompt 和关闭，观察者取消/异常时没有完整资源保护，关闭超时也被忽略。

这两条路径现在共用 `auxiliary_session::Session`：

- 临时目录、原 Driver 和可选服务租约由独立启动工作者持有。30 秒只限制前台观察，不取消已准入的原请求，不重新查注册表或重发创建。迟到成功交给同一原资源清理，不能再派发 prompt；迟到明确拒绝才释放资产。
- 提交总结的模型目录读取也在该工作者内。观察者在目录读取期间退出时，原读取继续结算，但进入下一步创建前检查停止标记，不新建一个已经无人需要的会话。
- 获得句柄后立即捕获原 scope 的被动退出证明，在 prompt 可能退休或移除查找表之前保存。Drop、错误、panic 和正常完成都使用同一原句柄/证明；只发送一次 interrupt/关闭。
- 关闭未获证时返回明确的待确认错误，保留目录/租约并继续观察原请求。原证明可以早于关闭 RPC 返回；迟到的关闭错误不能推翻已确认退出。提交总结不再把关闭失败当作一次完整成功。
- guard 捕获创建时的 Tokio runtime，在普通线程丢弃 guard 仍能调度原清理。整体 runtime 被销毁不在本轮保证范围内。
- 原生启动 panic 且没有返回身份时，报告未知并保留资产/租约；不能猜测句柄或伪造退出。后续持久化启动监督仍需继续。

Core 增加临时操作的活动租约。`AcceptedExecution` 只保存同一 Core 的计数器，实际开始分析/总结时才占用；分析的项目保护租约通过组合回调一起保留。桌面 `retire_if_idle` 在原启动/清理仍未结算时拒绝退休，不能因为前台调用已经结束就关闭 Core。

## 本地验证

新增 9 项 Core 回归，覆盖观察超时后迟到成功/拒绝、调用方取消、启动 panic、创建后原证明不再可查询、清理结果观察者退出、普通线程丢弃 guard、总结目录读取取消、总结 prompt 取消/panic，以及正文完成但关闭未知不能声称成功。只使用私有内存 Driver 和临时目录；原有项目分析与提交文本回归继续执行。

新增桌面 Core/RPC 集成回归：分别在项目分析与提交总结创建阶段取消调用方，验证原启动期间和原关闭等待期间 `retire_if_idle` 都拒绝，原会话只创建一次、没有 prompt，确认原关闭后才允许退休。没有触碰日常 Core。

macOS/Linux Core 各 119 项、macOS desktop 348 项（8 项按需测试未启用）与全部存储 175 项、Linux 相关存储集成 42 项通过。macOS 全工作区、Linux Core/存储/Driver API 三包及 Windows GNU 对应三包 all-targets 严格检查通过，格式与 diff 空白检查通过；未执行 Windows 二进制、真实 Agent/模型、桌面输入或手机/模拟器。原三个开发服务容器保持运行，未提交或推送。

日志：`/tmp/dogcodes-auxiliary-session-final-macos.log`、`/tmp/dogcodes-auxiliary-session-strict-macos.log`、`/tmp/dogcodes-auxiliary-session-final-linux.log`、`/tmp/dogcodes-auxiliary-session-final-windows.log`。

## 后续范围

同一 Core 存续期间的原生创建/清理接续不等于 Core 强退、OS 重启后的持久化恢复。没有身份的未知启动仍保留待核实，不通过资源计数清零冒充退出。

已核对下一处具体缺口：`turn_review/execution.rs` 仍在创建时直接 select 取消/timeout，并忽略 5 秒 shutdown 的结果，尚未接入本轮临时会话监督，下一阶段应处理审查的原启动和关闭确认。其余辅助目录/账号/上下文等调用、尚存的后台临时通道、自动续轮与无轮次标识回调继续核对。功能对齐、视频 HDR/光标/GPU、Wayland 特殊输入、实机/弱网、推送和部署仍按八个工作包保留。


## 2026-10-27：移除退役侧栏 Chat 的辅助通道

按已确认的产品范围，侧栏 Chat 不再提供给用户。此次从剩余辅助事务调用点核对发现：桌面 UI、daemon（包括远控准入）、手机和服务端已无调用；Core 却仍保留完整 Chat 创建、流式问答、取消、关闭、回执和状态订阅。这条路径也仍直接 timeout/drop 创建未来并忽略关闭确认，继续为它补生命周期会维护一个已退出产品范围的功能。

删除 Core 的 `sidebar_chat` 模块、状态/订阅名额、删除会话及改项目目录时的专用锁和缓存处理。同步删除 AgentDriver 的三个专用 Chat 方法、Codex 的 `hub-chat` 内部模式、Claude 的 `chat_only` 分支及两者的专用创建实现。原有普通会话、只读运行分析、全权限审查、Git 提交总结、后台临时活动保护以及远控协议保留。Core 仍按真实主任务、Fork 和审查的状态阻止删除或换目录，文案不再提到已移除的问答。

删除三个专用模块共 1,687 行，其中 17 项 Core 用例只验证退役功能，随功能删除；另删除 Claude 专用 Chat 测试。Codex 的“调用方要求 full-access 也不能扩大临时会话权限”回归改为通过仍在使用的分析会话入口验证，夹具也改用该入口。桌面分析/提交说明生命周期夹具直接创建分析会话，不再借用 Chat 方法。退役清理本身相对本轮开始的源码净删除 1,858 行（不计后续夹具移动），没有恢复旧 UI、增加替代聊天功能、协议兼容或数据迁移，也未清空历史数据库。

仓库代码及手机/服务端调用搜索只剩远控准入测试中的 `start_sidebar_chat` 名称，用于明确断言退役方法不开放。验证：macOS/Linux 的 Core、存储（含全部集成）、Driver API、Codex、Claude 和 desktop 六包测试各 803 项通过，其中 Core 103、存储 197、Driver API 29、Codex 58（含产品夹具 6）、Claude 67、desktop 349；分别 20/23 项真实环境按需测试未启用。测试总数比此前少 18 项，因为删除 17 项退役 Core Chat 测试及 1 项 Claude Chat 测试；Codex 权限回归仍保留。两端全工作区 all-targets 严格检查和 Windows GNU 六包 all-targets 严格静态检查通过，Windows 未运行二进制。日志 `/tmp/dogcodes-retired-chat-{macos,linux-final,strict-macos,windows}.log`，初始编译 `/tmp/dogcodes-retired-chat-check.log`。

Linux 首轮在 Claude 私有进程测试的创建阶段出现 `ETXTBSY / Text file busy`，尚未进入取消断言；并发 fork 可能短暂继承其它测试刚打开的可执行脚本写描述符。夹具改用仓库内固定的 `tests/fixtures/native-process.py`，只有模式和输出在每项的私有 cwd 读写，运行期不再生成/覆盖该可执行文件。没有修改产品重试、退出断言、8 秒期限或测试并发度；修正后 Linux 六包完整测试和严格检查通过，失败首轮见 `/tmp/dogcodes-retired-chat-linux.log`，不计验收。macOS 的最终 Claude 全部 67 项也再次通过，见 `/tmp/dogcodes-retired-chat-fixture-macos.log`。此调整不改变真实 CLI 行为。

本阶段只运行合成/私有 CLI，没有运行已安装 Agent、真实账户、用户桌面/输入或设备；原开发服务保持运行，未提交推送。剩余辅助查询/未知身份边界、视频及实机/部署仍按 CURRENT_WORK 原八包继续。
