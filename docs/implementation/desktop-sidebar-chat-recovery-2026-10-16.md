# 侧边问答：原请求绑定与断连恢复

日期：2026-10-16。对应 DT-09，覆盖独立侧边问答的启动请求，其他操作仍按入口矩阵逐项验收。

后续已修复问答未参与后台空闲退出判断的问题，15 项原生专项通过，见[生命周期补充](desktop-sidebar-chat-lifecycle-2026-10-16.md)。统一订阅和跨入口准入等下面列出的剩余项仍未完成。

## 问题和实现

原实现只识别当前问答的 requestId，未比较正文与执行 Agent；后续提问或清空聊天后，旧标识可能再次启动回答。驱动可用性检查又早于去重，导致驱动离线时无法取回已受理的结果。

Core 现在在启动原生会话前，将完整 ChatRequest 与受理状态一起保存到现有 sidebar.chat.saved 事件。重发时按 taskId/requestId 查询历史，比较规范化正文与完整 executor，匹配才返回原状态；不同载荷拒绝，不启动第二次回答。查询先于驱动准入，支持驱动离线及清空当前聊天后核实原请求。终态继续使用既有事件持久化，不新增数据库表或迁移。

新增 sidebar_chat_receipt 共享服务查询，经本机 RPC 与 core_sidebar_chat_receipt 提供给窗口。记录分批读取；旧数据没有原始请求时仍可查询，但拒绝凭消息文本猜测请求并重发。服务重新加载发现运行中的历史记录时标中断，不自动执行；当前仍在运行的请求返回内存中的最新流式状态。返回受理回执不代表回答已经成功结束。

客户端先持久保存完整原请求，再调用后台。窗口重开后保留待核实项，新发送及清空被暂时阻止；用户可查询原结果，后台未发现记录时可明确重发相同标识、正文和 Agent。不会自动重发，修改后的草稿不会取代原请求。结束本机跟踪不取消后台执行，界面明确说明。存储写入失败时不发送；迟到的旧回执不删除新请求。

## 验证

本次集中验证，无全量打包、真实模型调用或安装包验收：

- `cargo test -p agent-core -p desktop --lib sidebar_chat`：9 项 Core、2 项桌面原生用例通过。涵盖旧请求跨后续提问/清空恢复、载荷冲突、驱动缺失、重启标中断、SQLite 文件关闭重开和实际 loopback RPC。
- `SidebarChat.test.tsx`：8 项通过，包括关闭/重开窗口后显式恢复、保留修改后的草稿、回包丢失后的查询恢复。
- `sidebarChatOperations.test.ts`：5 项最终通过，涵盖本地写入失败、不同载荷拒绝、回执冲突和迟到回执保护。首次执行因遗漏 jsdom 测试环境声明失败，补声明后仅复验这 5 项；没有据此改动业务逻辑。
- `pnpm --filter @dogcodes/desktop typecheck`：通过；`git diff --check`：通过。

临时日志：`/tmp/dogcodes-sidebar-recovery-native.log`、`/tmp/dogcodes-sidebar-recovery-frontend.log`、`/tmp/dogcodes-sidebar-recovery-frontend-final.log`、`/tmp/dogcodes-sidebar-recovery-types.log`。进程均已收取退出状态；日志不是仓库中的持久验收附件。

## 尚未完成

- 清空聊天的版本条件、持久回执与客户端恢复已在下面的后续工作块补齐。版本冲突等未受理拒绝仍通过错误反馈并保留本地原请求，用户核实后可结束跟踪；未来错误协议统一时需要进一步区分结构化拒绝和传输结果未知。
- 问答仍轮询状态，统一失效订阅、跨入口禁用 Agent 的准入和长历史性能需要继续收敛。回执查询使用每页 256 条的扫描，但历史总扫描及全局问答锁持有时间没有完成性能验收。
- 问答终态持久化失败目前会在内存状态报告保存错误；后台重启后按已持久化受理记录显示中断。此处不承诺恢复未保存的流式输出。
- 独立审查/Fork 请求恢复、其他业务域及三平台真实退出/重启验收仍未完成；本专项通过不代表 DT-08/09/10 或桌面发布已达标。

## 清空聊天：版本条件与持久回执（同日后续工作块）

原来的 clear(taskId) 改为 ChatClearRequest：taskId、operationId、expectedRevision。旧的不带版本调用不再接受。ChatState 增加 revision；新问题受理、回答终态及清空后更新，既有缺少版本的记录用最后保存事件的序号生成稳定版本，不迁移数据库。流式输出期间禁止清空，终态另换版本。

Core 在同一个问答互斥范围内查询原清空回执、比较完整原请求、检查当前版本并写入事件。清空结果和原请求在同一个 sidebar.chat.saved 事件中落盘，没有“已写意图但尚未清空”的中间阶段。同一 operationId 的成功请求只返回原回执，不覆盖当前新聊天；改 expectedRevision 的复用被拒绝；另一操作对同一旧版本的迟到请求被拒绝。新增 sidebar_chat_clear_receipt，经 daemon RPC 与有窗口范围检查的 core_sidebar_chat_clear_receipt 查询。

客户端在打开确认框时冻结所见版本，发送前保存原操作；关闭重开窗口后仍可核实，明确重试时保留相同标识和版本。问答启动与清空的本地待核实记录互相阻止新操作，后台版本校验负责跨客户端的并发保护。查询到成功回执后重新读取当前聊天，不用历史空结果覆盖新内容。结束跟踪不会撤销已经发送的请求，界面明确提示。

这一工作块集中验证结果：

- `cargo test -p agent-core -p desktop --lib sidebar_chat`：11 项 Core、2 项桌面原生用例通过。新增旧版本清空/不同载荷拒绝、成功清空后出现新问答再重放、并发清空仅一个成功；扩充 SQLite 文件重开与 RPC 回执测试。首次编译因新测试误写 DriverEvent 的变体名失败，改用实际 TurnCompleted 后复验通过。
- `SidebarChat.test.tsx` 的 11 项及 `sidebarChatOperations.test.ts` 的 10 项全部通过；包括确认时冻结版本、丢包后重新打开、只重试原请求、历史回执不遮住新问答、存储失败不发送、回执载荷不符及迟到回执保护。
- 桌面类型检查及 diff 空白检查通过，无全量编译/打包或真实模型调用。
- 日志：`/tmp/dogcodes-sidebar-clear-native.log`（首次编译失败）、`/tmp/dogcodes-sidebar-clear-native-final.log`、`/tmp/dogcodes-sidebar-clear-frontend.log`、`/tmp/dogcodes-sidebar-clear-types.log`。对应进程均已收取退出状态。

注册入口现为 109 项（107 项生产入口）。长历史性能、统一订阅、跨入口 Agent 准入、独立审查/Fork 及其他业务域仍按 DT-09 清单推进，三平台真实进程/窗口验收尚未完成。
