# Agent 接手指南

> 2026-10-17 用户已取消依赖许可证归档、生成校验及应用内许可证展示功能，相关实现与旧任务已移除。接续工作不要重新生成归档或恢复展示入口；发布说明与空白签名配置继续保留。


> 2026-10-17 最新范围调整：用户要求移除侧边栏 Chat；前端及专属 Tauri 桥已删除，保留主会话/审查共享机制和内部 Core/daemon。后文及交接快照的问答 UI 开发/验收项已退出桌面范围，不要重新添加。操作记录浏览 UI 也已移除，后台回执恢复仍保留。


更新日期：2026-10-17。

**本次换 Agent 请先读 [桌面首发接续清单（2026-10-17）](implementation/desktop-release-handoff-2026-10-17.md)**：包含当前工作区风险、已完成范围、下一块明确代码入口、剩余发布门槛、验证证据及用户约束。下方多轮“最新/下一步”是历史记录；状态以 `CURRENT_WORK.md` 为准，避免重复执行旧任务。

## 先读与下一项

最新基础工作块为[主会话安装就绪准入](implementation/desktop-primary-ready-admission-2026-10-16.md)：SubmitTurn 已复用 require_available_executor，明确安装 ID/provider/Ready/当前选择；仍在同一 agent_admission 读锁内绑定驱动，新请求无效不持久受理，原回执先返回，已受理运行保持原实例。FakeDriver 新增 with_executor，store 测试用合法固定身份+标准探测事件或实际 Core 探测命令；五个原生验收 examples 建立隔离安装身份，history MCP 旧审批补原 turn/request sequence。224 项原生测试（84 Core、138 store、2 desktop RPC）和全部 store examples check 通过；最终 98239 已 exit 0，无运行中命令。87117 会话中断后确认进程已消失才继续；80975 仅两处夹具 move 错误，修后 81375 store 全过，后续示例缺旧审批字段修正后只重查示例/桌面，未重跑 Core/store。日志 `/tmp/dogcodes-primary-ready-{core,store-final,examples-final,desktop}.log`。没有新依赖/迁移/协议命令/前端构建/打包，签名配置未变、忽略规则确认。**下一步提交说明及项目分析准入**：两处 Core 方法仍直接查 drivers；尤其 project_runs 在保存 running 回执后，worker 才逐目录等待/扫描/调用分析，需一起处理受理与驱动归属，不能只加执行时检查。长期释放/状态、其他恢复、R2 来源素材及三平台候选仍未完成；DT-08/09/10 和整体发布均保持实施中。下方“最新/下一步”为历史状态。

最新基础工作块为[运行驱动归属](implementation/desktop-run-driver-ownership-2026-10-16.md)：owned_runs 改为 run ID→原 driver，SubmitTurn 在准入锁内按将要落盘的 executor 取实例，持久受理后/发布前绑定，RunOwnership 收尾释放。worker/取消/审批/运行中权限目录用原实例，自主续跑绑定观察流 driver；无注册驱动的新请求拒绝且无回执。167 项原生通过（84 Core + 67 store lib + 9 running_permissions + 6 approval_receipts + 1 desktop primary_admission RPC）；29529 首轮仅旧版本冲突夹具缺驱动失败，修夹具并补拒绝断言，55833 余下 store/desktop 已 exit 0。日志 `/tmp/dogcodes-run-driver-{core,store-final,desktop}.log`，无运行中命令。前序主会话禁用准入 57709 也已 exit 0，180 项通过，文档已补齐。入口计数仍122/120、CoreCommand18，无新依赖/迁移/前端构建/打包，签名文件未变且 check-ignore 确认。**下一步主会话 Ready/明确安装身份/当前选择准入仍必做**，补现有 FakeDriver/隔离验收夹具的安装事实，不能设生产例外；提交说明和项目分析等入口、空闲原生会话/自主流的长期释放、长期目录/素材来源/三平台候选验收继续。DT-08/09/10 和整体发布均未完成。以下较早工作块中的“最新/下一步”仅为当时状态。

最新基础工作块为[审查准入](implementation/desktop-review-admission-2026-10-16.md)：提取 execution_admission，侧边问答/审查共用启用和Ready/明确installation ID/当前安装校验；审查候选也过滤，CoreClient/RPC传递错误。已有请求先读回执，不因禁用/移除而重执行或取消；新受理持agent_admission读锁至保存/spawn。51项原生（17Core问答+22store审查+12desktop review_rpc）及live_review_workflow示例check通过；测试夹具与隔离示例补安装身份，不增加生产例外。首轮66792仅helper &Core误用execute编译失败，改&Arc后40859已exit0；日志 `/tmp/dogcodes-review-admission-{core,store-final,desktop,example}.log`。验证后仅同步注释/文档，无运行中命令。入口计数不变122/120、CoreCommand18，无新依赖/迁移/前端构建/打包/凭据改动。下一步主会话SubmitTurn、提交说明Core仍未调用共享准入，项目分析也需核对；长记录/恢复、来源素材及三平台候选门槛继续。不将整体发布或DT-08/09/10标完成。

最新工作块为[问答恢复记录清理](implementation/desktop-sidebar-chat-retirement-2026-10-16.md)：Core `recovery::TaskDeletion`/`task_deletion` 复用 `submission_task_deletion` 验证永久标记；daemon + Tauri `core_task_deletion` + TS CoreClient 可选接口接通。`sidebarChatOperations` 按证据/原字节清理两个本地键空间并识别手动核实/重发的 TaskDeleted；App `useSidebarChatCleanup` 在启动/序号刷新/聚焦/storage/30 秒补漏时发现不在列表内的任务记录，解绑后旧证明不生效。24 原生（17 Core+7 desktop，93357 exit 0）、36 不同前端用例（33专项+3App导航）、最终类型检查通过（18826 exit 0，首轮仅新测试漏 installationId，补 null）；日志 `/tmp/dogcodes-sidebar-retirement-{native,ui,app,types-final}.log`。原生曾等待其他活跃 Cargo 构建锁，原句柄现已正常结束；本轮无活跃命令。入口122/120、CoreCommand18，无新依赖/迁移/生产打包/签名改动。后续继续核对长记录开销、其他业务恢复/准入及三平台门槛，勿标整体完成。

最新工作块为[问答删除边界](implementation/desktop-sidebar-chat-deletion-2026-10-16.md)：Core DeleteTask/UpdateProject 在审查锁之后取得问答锁，锁内验证与持久化；running 包含取消收尾，阻止删除及更改所属项目目录。删除成功才移除内存 ChatEntry、关闭通知源，失败保留；全部问答入口在取得锁后再 require_task，防止等待期间删除后重建缓存。17 Core + 7 desktop 问答专项、4 store task_lifecycle 回归通过，33833 exit 0，日志 `/tmp/dogcodes-sidebar-deletion-{native,store}.log`。无新依赖、协议、前端功能或签名变更。下一步核对客户端待核实问答/清空记录在永久删除后的处理：`sidebarChatOperations.ts` 当前仅手动 forget，App 删除/刷新未清理这两类本地记录；可复用 `Core::submission_task_deletion` 的持久 tombstone 证据，不能仅凭 NotFound/断网丢弃记录。另有长事件扫描开销、其他入口准入及三平台验收。所有本轮进程终态，勿重复已通过且未改变的测试。

最新工作块为[问答状态订阅](implementation/desktop-sidebar-chat-watch-2026-10-16.md)：Core 按会话 coalesced watch，64 个全局名额，初次刷新/15 秒心跳；启动/可见文本/取消/结束/清空均通知，clear 保持同一通知源。daemon 复用 `review_watch` 转发并释放请求门禁；Tauri `sidebar_chat/watch.rs` 复用窗口桥；前端 `SidebarChatApi.subscribe` + 既有重连机制，查询 dirty 合并，聚焦和 15 秒补漏，隐藏只 detach。16 Core + 6 desktop 专项通过（40738 exit 0），26 前端专项通过；首轮类型检查仅新测试联合类型访问错误，修正后 86317 exit 0。日志 `/tmp/dogcodes-sidebar-watch-{native,ui,types-final}.log`。入口清单更新为 121/119，CoreCommand 18；没有新依赖、生产构建、打包或签名变更。下一步长记录/删除与资源回收、其他执行入口准入及三平台验收；本轮全部命令终态，不重复已通过且未改变的检查。

最新工作块为[问答安装准入](implementation/desktop-sidebar-chat-readiness-2026-10-16.md)：Tauri 的 Ready 筛选已移到 Core 候选列表和新问答受理，检查 provider/installation ID、当前选择、Ready 及注册驱动。安装探测/移除、选择和驱动注册与问答受理共享读写门禁；原回执和已受理任务停止仍可用。80 项 Core + 5 项桌面问答专项通过，62403 exit 0，日志 `/tmp/dogcodes-sidebar-readiness-{core,desktop}.log`。MemoryRepository 测试夹具补安装事件投影及身份查询，RPC 测试使用持久安装记录，无测试驱动生产例外。无新依赖、协议命令、前端构建或签名变更。下一步继续其他执行入口的准入、问答统一订阅/长期记录/删除边界及三平台验收；所有本轮进程已终态，不重复已通过且未改变的检查。

最新工作块为[问答 Agent 准入](implementation/desktop-sidebar-chat-admission-2026-10-16.md)：Core 候选列表/新问答读取共享禁用设置，读写门禁将设置更新与受理排序，原请求回执优先，禁用不取消已受理任务。17 项原生专项通过（13 Core + 4 desktop），77743 exit 0，日志 `/tmp/dogcodes-sidebar-admission-native.log`。Local/daemon 均正确传播候选查询错误，无新依赖、前端/协议命令或签名改动。下一步核对安装就绪状态在 Core/daemon 的一致准入及其他执行路径、统一订阅/长期记录/删除边界；DT-08/09/10 和三平台验收继续。所有命令已终态，勿重复已通过且未改变的专项。

最新工作块为[问答空闲退出门禁](implementation/desktop-sidebar-chat-lifecycle-2026-10-16.md)：Core `has_active_sidebar_chats()` 已接入 daemon `retire_if_idle`，避免仅有侧边问答时服务在桌面版本切换中提前退出。15 项原生专项通过（12 Core + 3 desktop），55315 exit 0，日志 `/tmp/dogcodes-sidebar-lifecycle-native.log`；真实 RPC 覆盖丢原回复、另一客户端查询/原请求重放、启动/取消/收尾和最终退休。测试新增 async-trait dev-dependency，外部依赖集合不变，锁文件及 notices 已同步、静态检查通过。未打包或改签名文件。继续问答统一订阅、跨入口 Agent 准入、其他恢复/删除边界及三平台验收；不把 DT-08/09/10 标完成。

最新工作块为[启动维护时序](implementation/desktop-startup-maintenance-2026-10-16.md)：必要 Core 恢复/宿主初始化仍在 endpoint 发布前；发送清理和初始 Agent 扫描由服务跟踪、后台运行，不挡接收循环，完成前阻止空闲退出。RPC fixture 复用生产接收循环。一次原生 65 项通过，句柄已收取 exit 0，日志 `/tmp/dogcodes-startup-maintenance-native.log`；随后仅格式与提示文案调整。没有前端变化、真实 Agent 调用或打包。发送域 gate 仍可能等待长扫描，其他工作流/回收/三平台验收等门槛未完成。

最新工作块为[普通发送后台订阅](implementation/desktop-submission-watch-2026-10-16.md)：公共 `state_changes` 和前端 `invalidationWatch` 机制由 Diff/发送恢复复用，新增 2 个主窗口 watch/unwatch 入口；共 119 handler / 117 生产入口。登记/结束/删除清理只发提示，重连读持久记录，查询中再来提示会补查；保留 15 秒轮询。82 前端、23 原生、最终类型检查通过，所有句柄终态；首轮类型检查仅测试动态 mock 访问方式有错，已修正。日志 `/tmp/dogcodes-submission-watch-{frontend,native,types-final}.log`。下一步继续长目录/启动时机、其他恢复工作流和三平台验收；没有打包或修改私有签名配置。

最新功能工作块：普通发送恢复的单条异常隔离已实现，详见[故障隔离记录](implementation/desktop-submission-isolation-2026-10-16.md)。List 保留不可读身份并报告不明条目；桌面保留可读恢复操作与错误并列，新发送仍严格阻止；删除清理处理所有可验证记录后报告部分失败。专项 75 前端、20 原生及类型检查通过，三个进程句柄均已收取 exit 0；日志 `/tmp/dogcodes-submission-isolation-{frontend,native,types}.log`。未改 daemon 启动时机，未实现长期索引/统一订阅，也未打包。下一步继续这些剩余基础项和其他工作流/三平台验收，勿重复已通过且未变化的检查。

最新源码审阅块：9 份历史 JSON 摘要的 34 个内部身份已一致别名化、四处临时路径已占位，9 份文档的个人路径已规范；原文和映射备份在被忽略的 `release/private/evidence-publication-d4wudb1k/`。两个历史探针改用 PATH/显式环境变量，离线检查通过，未运行 Agent。最新启发式报告 `release/private/source-audit-jca99thw.json` 中工作区个人目录命中仅剩测试/脱敏规则，但旧索引和历史仍有原始值；未暂存这些编辑，尚非发布候选。见[发布准备记录](implementation/desktop-release-preparation-2026-10-16.md)。继续完成 DT-08/09/10、来源/图片审阅、历史策略和三平台验收，勿反复编译打包。

源码发布整理新增：386 个本地实验输出/设计预览已精确移出索引，383 个现存文件摘要不变，3 个原缺失文件保持缺失；原索引对象和清单在被忽略的 `release/private/source-curation-1K3wnT/`。两个实验 crate 的源码及四个协议固定样例保留；发布输入检查通过，公共模板仍 0/2 入索引，12 项发布脚本测试已有通过终态。见[源码证据政策](implementation/source-evidence.md)。下一步继续审阅其余文档摘要/截图、源码个人路径及可达历史；没有改写历史或公开仓库，不能把当前 index 视为完整候选。DT-08/09/10 等功能与平台门槛仍未全部完成。

最新工作块已接通后台附件上传/查询/读取、固定身份与 SHA-256/大小校验，以及重开后的导入恢复。前端累计 66 项不同用例及最终类型检查通过；原生专项 16 项通过，唯一验证链已收取 exit 0，无仍需等待的进程。下一步补上传孤立文件/崩溃临时目录的安全回收、发送意图后台共享发现及其他旁路入口；详见[附件恢复记录](implementation/desktop-attachment-recovery-2026-10-16.md)。注册清单现为 116 项（114 项生产入口）。

最新已补 CoreCommand 幂等载荷校验：指纹包含完整命令、actor 和版本条件，写入既有 Ack JSON，Core 快速路径和 SQLite 事务内均检查，无数据库迁移。86 项原生测试通过，唯一验证进程链已收取 exit 0，见[命令身份记录](implementation/desktop-command-identity-2026-10-16.md)。只读命令回执现已接通 Core/daemon/Tauri 及修复/上下文客户端接口，12 项原生测试和相关前端验证通过，见[回执查询记录](implementation/desktop-command-receipt-2026-10-16.md)。本轮还修复订阅 Channel 同步初始化错误导致界面渲染失败的问题，相关 31 项前端复验及类型检查通过。“开始修复”的完整意图持久化及分阶段恢复界面现已接通，累计 38 项不同前端用例及类型检查通过，未运行原生编译/打包，见[修复恢复记录](implementation/desktop-review-repair-recovery-2026-10-16.md)。主会话停止请求现已保存原 task/turn/run/版本并接通只读核实和显式恢复，累计 47 项不同前端用例及类型检查通过，见[停止恢复记录](implementation/desktop-cancel-recovery-2026-10-16.md)。审批来源绑定与无答案回执基础现已接通 Core/daemon/Tauri/client，22 项不同原生、17 项不同前端用例及类型检查通过，见[审批回执记录](implementation/desktop-approval-receipts-2026-10-16.md)；审批持久恢复界面及显式作废未受理回复现已接通，本轮 7 项原生、49 项不同前端用例及最终类型检查通过，全部最终进程 exit 0，见[审批恢复](implementation/desktop-approval-recovery-2026-10-16.md)。恢复记录不保存答案/选项/表单，收到后台任务版本变化只读刷新投递状态；每个操作独立存储，不覆盖其他窗口记录。普通发送/创建的固定身份、只读回执与显式作废阶段接口现已补齐，13 项原生、13 项前端及类型检查通过，最终验证进程均 exit 0，见[发送身份基础及后续流程要求](implementation/desktop-submission-identity-2026-10-16.md)。App.submit 现已接入创建/上下文/发送的原意图持久化和恢复界面，固定各阶段身份与版本，核实/继续/安全放弃均已接通；累计 136 个不同前端用例及类型检查通过，见[发送恢复](implementation/desktop-submission-recovery-2026-10-16.md)。附件原身份与内容校验现已由上述工作块接通；下一工作块核对安全回收、普通发送恢复意图的后台共享发现和跨入口准入；本次本地记录不等于手机协议已完成。审批的真实供应商退出/断连竞争仍留候选验收；旧回执缺指纹继续拒绝猜测，不清库。

项目审查、逐轮审查和修复后复审启动的持久原请求/跨窗口核实已接通，113 项不同前端用例及类型检查通过，见[审查恢复](implementation/desktop-review-start-recovery-2026-10-16.md)。其中对 Core.execute 未校验载荷的发现已由上述工作块修复，主会话修复提交的客户端恢复也已由最新工作块接通；普通发送恢复现已接通，其他命令和跨入口恢复仍待继续。

此前已补 [Fork 请求与回执](implementation/desktop-fork-recovery-2026-10-16.md)（7 项原生、17 项前端）及[问答启动/清空恢复](implementation/desktop-sidebar-chat-recovery-2026-10-16.md)（13 项原生、21 项前端）。Fork 多仓库中间阶段/完整恢复计划、问答统一订阅和共享 Agent 准入尚未收口。当前注册清单为 116 项 handler（114 项生产入口）。

先读 [当前工作清单](CURRENT_WORK.md)，再读该项链接的实现/验收记录；需求和技术边界分别见 [需求文档](agent-hub-requirements-v1.1.md) 与 [详细设计](detailed-design-v0.1.md)。

当前正在按域推进 DT-08/09/10；终端和项目运行配置已迁入后台，分析/编辑的持久回执和结构化拒绝已接通。原启动请求的持久绑定与重放已补齐，见[运行配置记录](implementation/desktop-project-run-service-2026-10-16.md#项目启动的原请求绑定)。文件查询/监听已迁入共享服务并补齐断线清理与系统监听迟缓降级，见[文件服务记录](implementation/desktop-workspace-service-2026-10-16.md)。Diff 已补齐独立阅读者租约与请求乱序保护，专项 5 项原生、78 项前端及类型检查通过，见[阅读租约记录](implementation/desktop-review-leases-2026-10-16.md)。Diff 快照、关联文件/Git 操作、逐轮撤销、审查材料和删除清理已迁入同一后台；项目范围修改与操作门禁协调，专项累计 33 个不同原生用例最终通过，见[Diff 服务记录](implementation/desktop-review-service-2026-10-16.md)。提交/推送/拉取/同步已补持久回执及桌面恢复，包含失败推送跨界面恢复，累计 122 个不同前端用例、4 项原生及类型检查通过，见[Git 回执记录](implementation/desktop-git-operation-recovery-2026-10-16.md)。暂存/取消暂存、丢弃/忽略及批量文件修改已复用通用持久记录并接入文件恢复界面，保留部分结果；专项 39 项原生、累计 95 个不同前端用例及最终类型检查通过，见[文件回执记录](implementation/desktop-file-operation-recovery-2026-10-16.md)。逐轮撤销已复用文件持久回执；撤销卡片和 Git/文件恢复面板接入后台失效订阅，专项 40 项原生、21 项前端及类型检查通过，见[撤销/订阅记录](implementation/desktop-undo-state-subscription-2026-10-16.md)。下一步按[入口矩阵](implementation/desktop-operation-inventory-2026-10-16.md#文件监听与快照的下一工作块)推进 CoreCommand 之外的审查/侧边问答/Fork API 的身份、载荷绑定及恢复，多入口状态及其他业务域核对，后续继续核对项目资源展示、退出后台语义及其他副作用域。DT-01 的三平台环境与发布基线仍需收口。用户最新目标是先完善桌面，再在 GitHub 公开源码并提供 Windows/macOS/Linux 安装包；执行 [桌面发布拆分](implementation/desktop-github-release-work-breakdown-2026-10-16.md)，不再从 RA-01A/RA-02A 开始。

后台操作记录分页查询与只读详情已接通，空本地缓存仍可发现回执；同时修复正常快照发布遇配额锁短暂占用就失败的问题，见[操作记录与验证](implementation/desktop-operation-history-2026-10-16.md)。组合 Git 工作流的意图/后续身份及跨客户端接续现已接入后台，见[工作流记录](implementation/desktop-git-workflow-2026-10-16.md)。共享结束标记及接续仲裁也已接入后台，见[结束状态记录](implementation/desktop-git-workflow-close-2026-10-16.md)。组合推送的仓库/分支/原提交/上游地址绑定也已接入，原生专项 50 项通过，见[推送目标记录](implementation/desktop-git-push-target-2026-10-16.md)。继续旁路 API 核对与三平台候选验收，不把本机专项视为整体可靠性完成。

手机、官方服务和远控需求及 [原拆分](implementation/remote-access-work-breakdown-2026-10-16.md) 保留。RA-02 的本机基础已前移为发布前必做 DT-08/09/10：生命周期脱离窗口、现有全部功能的操作可靠性、后台共享业务状态。DT-01 后先明确这三项契约并按域实施，不能只修 UI 后绕过基础验收；配对/跨网/Dart/媒体仍后续。用户确认跨平台设备可提供，具体 OS/架构和接入方式在 DT-01 登记。

R1–R3 调整为三平台安装包、源码发布工程和 GitHub 首发。V1 默认后台服务验收已有历史证据，见 [记录](implementation/conversation-deletion-daemon-2026-09-25.md)；其中删除/恢复等旧语义需按当前永久删除行为重新判断。不要重复开发已完成功能，不要将历史通过或本次计划当成当前发布验收通过。

## 当前产品与实现约束

- 悬浮岛已按用户要求完全移除，F3 扩展退出范围；不要恢复其窗口、入口或专属 IPC。桌面与手机 App 可复用的 Core 命令、后台服务通信、审批及 Companion 投影保留，见[移除记录](implementation/island-removal-2026-10-16.md)。项目启动、调试、构建方式自动分析与一键运行已实现，补录为 D10，见[功能核对记录](implementation/project-run-capabilities-2026-10-14.md)，不要重复开发。
- 同项目允许不同 Agent 或同一 Agent 多个对话共享目录并发写入；不增加项目级单写者锁或强制 worktree。单个会话仍隔离轮次、审批与取消。
- 审查只保留代码、安全、性能三类。逐轮及项目 Diff 共用入口组件、已有 Agent 和额度菜单；Agent 自行读取代码/Git 差异，工作区允许变化；历史 Git 比较仍按所选对象。非 Git 撤销和 Diff 的原始内容记录保留。
- 审查使用独立 Session/Run、原生完全权限，提示词要求只审查不修改；不切换主 Agent；重试沿用范围并重新读取，修复后复审检查当前实现；旧冻结报告仅保留查看。普通审查成功不等于所有问题已解决。已复用主对话流式回复/工具组件；问题详情默认折叠、文件名链接定位 Diff，界面清理内部机制和通用范围声明；失败、取消保留部分输出，见 [流式审查](implementation/review-streaming-2026-09-27.md)。
- 已归档会话的删除为永久删除，移除最近删除入口；清理历史、报告、Diff、基线与快照，共享内容按引用保留。主任务或审查活动时拒绝；项目源码和 Agent 原生历史不删除。参见 [实施记录](implementation/conversation-permanent-deletion-2026-09-27.md)。旧版软删除记录不在本次自动批量清空。
- Core 不依赖 UI/Tauri；规范事件、投影和幂等 Ack 同事务；单次追加只涉及一个流，`sequence` 为全局序号，`stream_version` 用于该流并发校验。执行命令先检查幂等结果。
- active Run 的 executor 固定，preferred executor 修改仅影响下一轮。订阅先接 live 再 replay，重复按序号去重，掉队必须从持久事件补齐。
- Artifact 保留原始 bytes、归属和内容哈希；历史材料不能被展示解码覆盖。额度不发送 Prompt，不凭套餐推算读数。
- 数据库按当前未发布策略只初始化空库，不新增旧版本迁移/兼容回填；规范事件的投影重建继续保留。见 [数据库策略](implementation/database-initialization.md)。
- Copilot 和完整会话导出暂缓；Claude Key 过期是最近一次记录，重新调用前需确认条件已恢复。其他暂缓/退出范围以当前清单为准。

## 主要代码入口

| 区域 | 路径 |
| --- | --- |
| Wire 类型 | `crates/protocol/src/lib.rs`、`packages/protocol/src/index.ts` |
| Core / 会话命令 | `crates/core/src/lib.rs` |
| 审查编排 | `crates/core/src/turn_review.rs`、`crates/core/src/turn_review/`；`review_analysis/` 仅旧类型/来源读取 |
| SQLite / 会话删除 | `crates/store/src/lib.rs`、`crates/store/src/task_lifecycle.rs` |
| 桌面适配及后台服务 | `apps/desktop/src-tauri/src/`，先读 [服务记录](implementation/core-daemon.md) |
| 会话界面 | `apps/desktop/src/App.tsx`、`ConversationHistory.tsx`、`DeleteConversation.tsx`（后两者同目录） |
| 客户端桥接 | `apps/desktop/src/tauriClient.ts` |

更详细的历史架构导航、各供应商接入过程与旧验收数字保留在 [交接历史归档](HANDOFF_HISTORY_2026-09-25.md)。其中旧数据库迁移说明和旧“当前接手点”已失效，不作为指令执行。

普通发送原请求现已接入 daemon 持久登记、分页发现及有 Core 证据的共享结束，本工作块 97 项不同前端、20 项原生及类型检查通过；桌面在发送前登记，并可在空缓存下只读找回记录，见[共享发送记录](implementation/desktop-shared-submission-intents-2026-10-16.md)。普通发送的永久删除边界已补齐，70 项前端、22 项原生及类型检查通过。下一步继续统一失效订阅/长记录开销、清理故障隔离与其他恢复工作流的删除边界、附件安全回收和其余业务域；不将本工作块视为全部 DT-09/10 完成。

发布输入检查已补工作区/暂存区双重模板校验、严格签名字段及大写私密扩展名保护，11 项脚本测试和实际检查通过。两个模板仍未跟踪，当前 index 不是发布候选。已完成工作区/index/全部本地 refs 的启发式隐私盘点，报告在忽略的 release/private/，尚需整理实验原始运行资料/截图和可达历史，见[发布准备记录](implementation/desktop-release-preparation-2026-10-16.md#发布输入复核2026-10-16后续工作块)。没有修改本地凭据、提交或推送。

## 验证与环境收尾

用户要求过程中不要频繁编译、测试、打包。先完成一个完整工作块，再按修改范围集中验证；已通过且无新改动或疑点的检查不重复，全量回归和打包留到发布候选阶段。可选检查为 `pnpm typecheck`、相关前端/Rust 测试、格式及严格 Clippy；不要为纯文档整理重新调用真实模型或完整供应商矩阵。已启动的进程收取同一句柄的终态，不因观察超时重复启动。

桌面验收使用隔离实例、数据目录和可识别进程，不操作用户生产会话。结束后停止本次启动的应用及开发服务，保留证据；只结束已确认归属本次验收的进程。历史进程号、端口和账号状态不视为当前事实。

工作区已核实有 `.git`，当前仍有大量既有未提交修改及重命名；不要覆盖这些成果或将它们归因于本轮。发布准备已将 468 个 node_modules/dist 文件从索引移除并保留本地副本，尚未提交。完成后直接更新当前清单，并新增或补充对应实施记录。

审查最新语义及验证见 [2026-09-27 简化记录](implementation/review-simplification-2026-09-27.md)，不重新加入宿主分组、冻结检索或强制二次复核。
