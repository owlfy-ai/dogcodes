# dogcodes 需求说明文档

当前实施范围、完成状态和排程统一见 [当前工作清单](CURRENT_WORK.md)（2026-10-14）。本文的远期方案不等于当前已承诺交付；下列带日期的更新仅保留决策过程，不作为执行队列。

最新交付顺序（2026-10-16）：先完善现有桌面产品，再向 GitHub 公开源码并同时提供 Windows/macOS/Linux 安装包，见 [桌面发布拆分](implementation/desktop-github-release-work-breakdown-2026-10-16.md)。手机 App、官方服务和远控专项后续再做；以下已确认远程需求完整保留，其“首版”指未来远程访问首版，不作为本次桌面发布前置条件。

桌面发布前基础要求：DT-08 执行生命周期脱离窗口、DT-09 现有全部功能的操作可靠性、DT-10 后台共享业务状态必须先完成并验收。后台持有执行资源，操作有明确终态与恢复语义，共享业务事实不只存当前界面；主题布局、未发送草稿及待发送参数仍各端独立。该基础供后续手机复用，不要求先实现手机、配对或官方服务。

当前发布范围（2026-10-16）：悬浮岛窗口、启用入口及专属实现已移除；保留桌面与手机 App 可复用的 Core 命令、后台服务通信与 Companion 状态投影。项目启动、调试、构建方式自动分析与一键运行已实现，补录为 FR-3.10 / D10，见[功能核对记录](implementation/project-run-capabilities-2026-10-14.md)。

远程访问需求补充（2026-10-16，方案阶段）：手机需覆盖 PC 日常业务操作，并支持 Windows/macOS/Linux 屏幕查看和键鼠控制；Linux X11 与 Wayland 均已确定纳入远程访问首版，参考 RustDesk 的实现思路，独立实现及验收。无需注册或登录服务端账号，采用设备身份及 PC 二维码扫码配对方向，覆盖移动网络连接内网 PC。通信、配对、权限撤销、断线恢复、X11 增量估算及验收用例见[远程访问评估](implementation/remote-access-assessment-2026-10-16.md)。具体技术选型及支持的桌面最低版本尚未冻结；本条不表示功能已实现，也不改变 FR-7 首代硬件的独立范围。

远程访问讨论补充：手机 App 尚未开发，采用 Flutter 方向并参考 RustDesk 移动端思路，首版同时支持 iOS 和 Android。面向个人使用，自己的手机连接自己的多台电脑；一次扫码绑定授予全部产品功能，后续无需 PC 再确认，可随时撤销，但不替代 Agent 原生操作审批及操作系统授权。首版仅提供官方接入与中继服务；支持 PC 已登录后无人值守、关闭主窗口后后台继续工作及锁屏后的 Agent 操作，远程解锁后续再做。

手机原生界面范围补充：支持 App 内项目网页预览并保留远控 PC 浏览器；文件界面只浏览项目文件和上传任务附件，修改通过 Agent 或远控完成；Agent 页面只显示安装状态和额度，安装、升级、配置和登录通过远控完成，会话内 Agent/模型/思考强度选择保持可用。

远控交互范围补充：首版提供用户手动触发的双向文本和图片剪贴板传输；支持多显示器切换，一次查看和控制一个屏幕。不做自动剪贴板同步、多屏拼接总览、PC 系统声音传输或手机麦克风回传。

通知与草稿范围补充：待审批、待回答、任务完成和执行失败均提供后台通知，支持按电脑或会话静音；离线新任务只保留手机本地草稿，重连后由用户点击发送，不做自动发送队列。手机和 PC 的未发送文字、附件选择、待发送模型/思考强度各端独立保存，已发送内容、执行状态与审批结果共享。

服务与交付范围补充：官方服务首版覆盖中国大陆和海外；iOS/Android 先内测分发，验证稳定后再正式上架。首批供个人及少量用户试用，具体规模和预算后续确定，不预设大规模容量或无限中继流量承诺。

项目与离线使用补充：手机可选择 PC 已有目录添加项目，首版不提供原生创建空目录或克隆仓库入口；离线可查看已缓存的最近会话，显示离线状态和最后同步时间，并可清除缓存。任务执行或远控期间默认阻止 PC 自动睡眠，用户可关闭，不改变系统锁屏设置，也不提供远程唤醒。上述首版产品范围已完成本轮讨论，技术选型、兼容版本与实际部署预算按远程访问方案继续细化。

上述远程访问范围已拆分为 [37 个实施子任务](implementation/remote-access-work-breakdown-2026-10-16.md)，包含依赖、代码范围、交付与 30 项验收映射；工作包状态统一见当前工作清单。本轮完成规划，未开始功能实现。

### 历史决策更新

> 2026-09-27 审查简化：三类入口与既有 Agent 保留；Agent 在项目目录自行读取代码和 Git 差异，取消审查专用冻结材料、分组检索与强制二次复核。逐轮入口检查当前实现，Git 历史/分支按所选对象比较。非 Git 撤销及 Diff 所需原始内容记录保留。替代下方历史审查编排方案，见 [审查简化记录](implementation/review-simplification-2026-09-27.md)。

> 2026-09-27 会话永久删除：删除入口仅位于已归档会话；确认后不可恢复，清理消息、审查报告、历史 Diff 及会话快照，移除“最近删除”界面。共享内容仍有其他会话引用时保留；项目源码和 Agent 原生历史不删除。替代 2026-09-25 的可恢复删除方案。见 [实施记录](implementation/conversation-permanent-deletion-2026-09-27.md)。

> 2026-09-25 会话删除：已实现可恢复删除、最近删除列表和恢复；普通/置顶/归档会话复用侧栏入口，确认后执行。主任务或审查排队/执行/取消中禁止删除。删除状态持久化，保留消息、审查、历史 Diff 与原生会话；恢复原列表位置，不自动清空，不删除项目代码。具体设计与验证见 [实施记录](implementation/conversation-deletion-2026-09-25.md)。本条更新下方历史“删除未实现”的状态。

> 2026-09-24 更新：完整会话导出暂缓；已实现会话手动重命名并持久保存，后续消息不覆盖。灵动岛接入独立审查的类型、Agent、状态、分组进度，审批优先，终态展示 60 秒；不改变主 Agent 与取消隔离。详见 [实现记录](implementation/conversation-island-2026-09-24.md)。

> 2026-09-24 审查修复闭环：逐轮与项目 Diff 共用问题选择；发回原对话主 Agent 修复，记录原报告、选择范围和修复轮次。复审独立使用修复轮次的新冻结版本及既有审查 Agent；原材料重试保持原语义，不自动标记问题已解决。实现、真实并发和桌面验收边界见 [实施记录](implementation/review-closure-2026-09-24.md)。

> 2026-09-24 审查编排升级：实现按目录/大小分组、冻结材料的上下文检索、独立 Session 的 AI 二次复核；逐轮与项目 Diff 共用三个原入口和既有 Agent。展示各组进度、材料缺口和复核结论，失败/取消保留初审；旧快照不读取当前文件补历史。实现边界见 [审查编排记录](implementation/review-workflow-2026-09-24.md)，更新下方此前“后续实施”的状态。


> 2026-09-23 审查质量升级：在代码审查、安全检查、性能分析及逐轮/项目 Diff 双入口上，增加结构化问题、历史位置校验与跳转、基础规则和逐文件覆盖；继续复用已有 Agent 和 UI。覆盖自报与程序位置校验不等于缺陷已确认。分组、同版本上下文检索和独立 AI 复核后续实施。详见 [实现与验证记录](implementation/review-quality-2026-09-23.md)。

> 2026-09-22 审查入口决策：保留逐轮历史 Diff 与三个审查入口；项目 Diff 面板同时提供代码审查、安全检查、性能分析及独立 Agent 选择。项目审查采用当前比较范围的固定快照，支持原材料重试，结果明确关联比较来源。实现见 [项目审查记录](implementation/project-review-2026-09-22.md)。


> 2026-09-19 逐轮协作决策：每个已完成 Turn 底部，左侧提供复制、Fork 和完成日期时间，右侧提供历史 Diff、代码审查、安全检查、性能分析（2026-09-21 收敛为这三类），并独立选择审查 Agent；复用全部已接入且就绪的 Agent，每个品牌只显示一个可用安装，不展示内部安装 ID。一键发起独立只读会话，不切换主 Agent；支持单独取消/重试，结果附原轮次并带来源回流。审查和 Diff 使用该轮冻结材料，缺失时明确提示，禁止拿当前工作区替代；重试固定原材料。同项目并发的变更按时间段采集，须说明归因限制。Fork 继承截至选中轮次的对话历史、沿用项目目录、不还原文件。Copilot 暂缓接入。首批实现及验收边界见 [实施记录](implementation/turn-review-2026-09-19.md)，本条优先于较早进度描述。

> 2026-09-19 产品决策修订：同一项目允许不同 Agent，或同一个 Agent 的多个独立对话，同时修改共享工作目录中的代码；不设项目级单写者门禁，也不要求普通对话为此创建 worktree。单个对话仍保持轮次、审批和取消的隔离。Git 提交备注的“AI 总结”不要求独立执行留痕，不作为 M1 收尾缺口。

> 2026-09-12 需求执行修订：历史查询采用冻结序号上界下的按需分页，取消全历史 16 MiB 快照上限；交付确认按 SessionMap、源事实哈希和字节范围记录。执行游标不能作为历史已交付证明；截断、引用和部分读取不能确认全文。确认仅表示 Driver 接受/工具输出写出，不推断模型理解。M1 核心主链路已落地，Copilot SDK、Hub 只读 Delegation 与剩余发布/平台验收未完成；灵动岛与历史检索提前交付不代表 M1.5/M2 整体完成。


> 2026-09-11 恢复范围补充：取消必须隔离轮次，停止失败恢复有效待审批，终态/宿主中断使旧审批失效；目录缺失不得向 Agent 静默派发。原生会话丢失可显式准备重建，在下一条新消息中交付历史，不自动重发旧指令。启动故障可导出不含对话、路径和凭据的摘要，不依赖健康数据库。验收状态见 [恢复与诊断](implementation/m1-s7-recovery.md)。

> 2026-09-11 执行口径修订：当前继续完成 M1 的多目录文件证据与按需历史查询。模型目录来自各 Agent 原生接口，模型与思考强度分开选择，模型菜单不加搜索；历史全文检索不受此限制。ZCode 不采集工具调用额度。自动交接不要求用户逐次预览确认。Review 高级文件系统恢复、worktree 清理不在当前交付范围。Copilot SDK、Hub Delegation 仍属未完成事项，不能用原生子 Agent 观察替代验收。具体实现边界见 [历史按需查询](implementation/history-query.md) 和 [多目录项目](implementation/multi-root-projects.md)。

## 多 Coding Agent 中控 · 统一 GUI + 统一会话史 + 统一额度视图

| | |
|---|---|
| 版本 | **v1.1（产品需求基线）** |
| 日期 | 2026-09-02 基线；2026-09-19 执行修订 |
| 状态 | 产品边界、协作需求与 Companion Hardware 边界已形成基线；供应商协议、额度来源和兼容版本仍须经 M0 实测后写入独立兼容矩阵 |
| 目标平台 | macOS（14+，首发）、Windows 10/11 x64 |
| 配套硬件 | H1 阶段；首版软件从 M1 预留 Core/Protocol 语义，不提前实现设备链路 |
| 技术方向 | Rust + Tauri 2 + React/TypeScript |
| 开源协议 | Apache-2.0 |
| 命名 | 产品：dogcodes；CLI：`dogcodes`；npm 包与 GitHub 仓库名：`dogcodes`；官网域名：`dog.codes` |

## 0. 决策摘要

**产品与战略**

1. 产品自建主界面，不嵌入、不修改、不依赖任何第三方 Agent 客户端 UI；悬浮岛已移除；桌面与手机 App 复用共享状态及控制能力。
2. 引擎与壳分离：Rust Core 从第一天不依赖 WebView，UI 只消费版本化事件协议。
3. 认证外部自治：Hub 不提供任何 API Key 输入/存储界面（含无订阅 agent）；不读取/提取/托管凭据；不代用户选择认证方式；用户在对应 agent 内自行完成全部登录与配置，Hub 只探测状态并引导官方登录/配置流程。
4. Claude Code 按普通官方 CLI/Harness 集成，不设额外授权阻断。
5. 权限与沙箱由 agent 自管：Hub 只透传审批与提问；"总是允许"仅写入 agent 原生配置。
6. 产品特点：本地优先、不绑定任何云生态、订阅额度视图、每轮 Handoff；提供项目运行方式自动分析与一键运行。悬浮岛已退出产品范围。

**技术**

7. 四档 Provider SPI；通道选择以"复用用户已配置好的官方 Agent"为第一谓词；会话 Driver 与 QuotaProvider 解耦；PTY 只用于登录与最后兜底。
8. **独立执行服务**：Rust Core 运行于独立后台进程；Tauri UI 通过版本化本地 IPC 连接，命令与事件保持传输无关。桌面退出或被硬杀不关闭 Agent 连接，重新打开后补齐执行状态。未来硬件网关接入同一 Core。
   异常恢复不得要求用户核实外部进程状态才能继续会话：关闭旧本地轮次并释放占用，保留连接中断及未知结果事实，允许用户直接发送下一条消息；只有获得原生成功终态才显示已完成。
9. MVP 纵向切片：Native RPC、ACP、官方 SDK 三类 Driver 各验证一个闭环；两批接入清单是优先队列，不是 M1 完成范围。
10. 并行候选从完整 Checkpoint 派生独立执行空间；默认并发 2、全局上限 4；三种采纳方式；未采纳候选先持久化后清理。
11. 额度中心只展示供应商真实窗口，来源与可信度分级；不伪造统一成本、不把上下文占用误报为订阅余额。

**协作**

12. 多 Agent 协作五模式：同线程切换（Handoff/Reconciliation）、@Delegation、指定评审、并行比较（Parallel Compare）、交叉评审。
13. **同线程切换为默认交互**：切换 agent 不产生任务分支，统一时间线内以 agent 标记区分；"从第 N 轮分叉"留作 P2 可选入口；多路尝试的需求由 Parallel Compare（工作区分叉）覆盖。
14. 外部 agent 会话观察为用户显式开启的可选能力，外部任务默认只读。

**发布**

15. **开源协议 Apache-2.0（Q6 已决）**；参考 GPL 项目只参考行为不复用代码；发布时附第三方许可证清单。
16. **命名规则**：产品名 dogcodes；CLI 使用 `dogcodes`；npm 包、GitHub 仓库名统一使用 `dogcodes`；官网域名使用 `dog.codes`。发布前仍须完成人工商标、域名、包名和仓库名占用检查，不宣称绝对无同名或无近似名称。
17. **macOS 分发**：签名公证 DMG + Homebrew 双通道，不进 App Store；Windows 提供签名安装包与自动更新。
18. **快捷审批共享原生交互语义**：首版由主界面响应 Agent 原生 `PendingInteraction`；悬浮岛已移除；手机 App 可复用同一 Core 审批语义。未来硬件沿用同一交互对象，在待审批状态下以单击批准、长按拒绝/取消本次请求。Hub 不自行创造审批选项。

**硬件扩展**

19. **Companion Hardware 是 dogcodes 的受限控制端，不是新的 Agent 客户端**：设备只连接 dogcodes，不直连 ACP、App Server 或供应商账号，不保存第三方 Agent 凭据。
20. **首代交互已决**：查看 Agent/工作目录/运行状态/额度；单击开始录音、再次单击结束转写、显示文字、第三次单击确认发送；长按选择下一轮 Agent；待审批时单击批准、长按拒绝本次请求并覆盖普通按键语义。
21. **软件首版必须预留硬件所依赖的 Core 语义**：区分当前 Run 与下一轮默认 Agent，提供版本化低带宽状态投影、统一审批对象、Actor、幂等和乐观并发；USB/Wi-Fi、配对、音频和固件在 H1 实现。

**修订记录**

| 版本 | 要点 |
|---|---|
| v0.1–v0.4 | 初稿 → 战略定位/Rich 直连/Delegation/M1.5 岛 → 两批清单 → 订阅优先 |
| v0.5 | 产品决断落地；纵向切片；权限去矛盾；事件分层；指标量化 |
| v0.6 | 评审模式、对比运行、交叉评审、Checkpoint |
| v0.7 | 合并统一稿：协作模型细则、Checkpoint 完整性、Candidate Workspace 三态、采纳三分法、评审二维安全模型、验收标准 |
| v1.0 | 协议/命名/分发/引擎内嵌/同线程切换等决议并入；命名与发布章节新增 |
| v1.1 | 修正命名与 CLI；删除 Roo Code；恢复 Agent 批次；校正 Claude/ACP/额度事实；补充传输边界、Handoff 生成、Canonical History 检索、完整 Checkpoint 物化与增量采纳算法；纳入 Companion Hardware 完整需求、软件首版架构预留与多端快捷审批；更新参考项目 |

---

## 1. 背景与目标

开发者同时装有多个 coding agent（Claude Code、Codex、Cursor、Kimi 等），界面割裂、订阅额度缺乏统一视图、上下文无法跨 agent 延续。

**产品目标**：dogcodes 是一个开源、本地优先的多 Coding Agent 中控，通过桌面 GUI 统一管理多个项目、任务与 Agent，提供每轮切换、结果评审与额度视图。提供项目启动、调试、构建方式自动分析与一键运行；通用子任务委派、并行比较及 Companion Hardware 属后续范围。悬浮岛已移除，共享通信能力保留。

## 2. 产品定位与战略

一句话：**面向个人开发者的本地优先多 Coding Agent 中控——统一 GUI + 统一会话史 + 统一额度视图。** 目标用户：同时使用两个以上 agent、多项目并行、想按专长与剩余额度分配任务的个人开发者。Apache-2.0 开源，无 Hub 云账号。

### 2.1 战略定位

- **不采用注入宿主客户端路线**（codex-host 已占据该生态位，仅作额度采集与协议实现的研究样本）。
- **产品特点**：本地优先、多家原生 CLI Agent 接入、订阅额度窗口视图、每轮 Handoff；提供项目运行方式自动分析与一键运行。悬浮岛已移除，Companion Hardware 为后续可选能力。
- **引擎与壳分离**：Core 不依赖 WebView，统一历史/Handoff/额度层不依赖自有 UI；长期保留"他人的壳 + 本引擎"组合。
- **ACP 是公共底座不是天花板**：提供多数核心交互的标准表达，不保证逐家实现与语义保真；以逐 agent capability 矩阵（附录 A + 运行时探测）为准。

### 2.2 多 Agent 协作模式总览

| 模式 | 目的 | 替换主 Agent | 默认代码权限 | 结果去向 | 阶段 |
|---|---|---|---|---|---|
| Switch | 下一轮由另一 agent 继续同一任务 | 是 | 与目标 agent 当前模式一致 | 主线程 | P0 |
| Delegation | 另一 agent 完成独立子任务 | 否 | 默认只读；写任务需隔离 | 摘要/产物回流 | P1 只读 / P2 写 |
| Review | 对当前结果给出意见 | 否 | 只读意图 | 评审卡片进主线程 | P1 |
| Parallel Compare | 多 agent 对同一提示独立产出候选 | 否（直到用户采纳） | 按工作目录类型隔离 | 候选集合进比较视图 | P1/P2 |
| Cross Review | 候选互评或指定统一评审者 | 否 | 只读 | 评价附着到候选 | P2 |

## 3. 产品原则

### 3.1 认证外部自治

Hub 可以：扫描可执行文件与版本；运行官方 login/auth/doctor/状态命令；在外部终端或受控内嵌 PTY 承载官方交互式登录；打开官方安装、登录与额度页面；显示 agent 自报的登录状态、账号标签、计划与额度。

Hub 不得：提供 API Key 输入框；读取/复制/上传 agent 的 OAuth Token、Cookie、API Key；在 Hub 数据库或 keyring 保存第三方凭据；通过环境变量替用户注入 API Key；替用户选择订阅还是 API 认证；认证失败时切换认证方式（只提供诊断与重新登录）。

即使某 agent 只能使用 API Key，也由用户在其官方命令/配置文件/客户端中自行配置，Hub 只检测是否达到可运行状态。

### 3.2 本地优先

无 Hub 账号；项目、任务、会话映射、事件、协作关系与额度快照保存在本地；未经明确启用不上传代码/会话/额度数据；Hub UI 离线可用，agent 执行是否联网由 agent 决定。

### 3.3 能力驱动

每个 Driver 连接时报告 capabilities；UI 按"原生呈现、注明降级、明确不支持、保留未知事件"四级策略渲染；供应商特有能力经扩展字段保留，不因统一而静默丢失。

### 3.4 权限自治

Agent 决定权限、工具执行、文件/网络访问与沙箱；Hub 不建跨 agent 危险操作判定，不把工作目录宣传为沙箱；只显示 agent 提供的审批选项并原样返回；"总是允许"仅在 agent 原生支持时由 agent 自己保存。

2026-10-17 用户授权例外：本项目为当前会话注入的内置历史工具自动通过，不要求逐次用户审批。限定当前活动服务身份与实际工具目录，回复原生“允许一次”；外部工具及终端命令继续遵循所选 Agent 权限模式，不因此授予全局自动执行权限。

### 3.5 协作可追溯

每个输出可追溯到 agent、模型、原生 session、工作目录、代码基线与用户输入；并行候选从可识别的共同基线开始；用户选择、评审意见、采纳结果与未采纳候选均留档；不把不同供应商额度换算成虚假的统一"N 倍成本"，不以自动评分掩盖原始差异。

## 4. 术语表

| 概念 | 定义 |
|---|---|
| Project / Task / Turn | 本地目录或 Git 仓库 / 项目内统一工作线程 / 一次用户输入及对应主执行 |
| Agent Provider / Installation / Account Ref / Instance / Session | agent 类型 / 本机安装路径与版本 / agent 自报账号计划元数据（不含凭据）/ 运行中进程或连接 / agent 内部原生会话 |
| Canonical History | Hub 保存的跨 agent 统一任务历史 |
| Handoff Package / Reconciliation Package | 切换到新 agent 的结构化交接包 / 切回旧 agent 时的离开期间增量对账 |
| Delegation Task | 另一 agent 独立完成并回流主任务的子任务 |
| Checkpoint | 并行/评审前冻结且可重建的完整工作区基线：基线提交、index/staged、unstaged、untracked、文件模式/符号链接、子模块状态及必要内容；校验值只用于验证，不能替代可恢复内容 |
| Review Run / Review Report | 评审运行过程 / 其产出的版本化结构化报告 |
| Comparison Run / Candidate Run | 同一提示的一组并行候选执行 / 其中某 agent 的独立执行 |
| Candidate Workspace | 候选的执行空间：worktree、临时目录副本或无目录 |
| Candidate Result Commit | 候选完成时冻结的不可变结果提交（仅存于临时引用，不入用户分支历史） |
| Common Task Package | 所有候选共享的同一版本任务包：提示、验收标准、上下文基线、相关文件、测试要求 |
| Cross Review / Artifact | 候选完成后对其他候选的评审 / 文档、Diff、测试报告、日志等可比较产物 |
| Quota Snapshot | 某时点额度信息、来源、可信度与额度作用域（interactive/headless/agent-sdk/extra-usage/unknown） |
| Companion Hardware / Device Gateway | dogcodes 配套状态与快捷控制硬件 / 桌面端面向已配对设备的受限连接入口 |
| Device Focus | 硬件当前选中的 Project/Task 工作目录；语音、Agent 选择和审批均只作用于该焦点 |
| Preferred Executor / Active Executor | Task 下一轮默认 Agent / 已启动且不可被改投的当前 Run Agent |

## 5. 目标用户与典型场景

**场景 A 同线程切换**：Claude 分析 → Codex 实现 → 切回 Claude 检查；切换发 Handoff，切回发 Reconciliation。
**场景 B 子任务委派**：主 agent 实现中，`@Agent` 让另一 agent 调研库或分析测试失败，结论回流。
**场景 C 指定评审**：当前 agent 完成修改后，指定另一 agent 只读评审需求、Diff 与测试，意见发回作者修复并复审。
**场景 D 并行文档设计**：同一架构需求给三个 agent 独立产出设计方案，并列比较、互评合理性，择优或人工组合。
**场景 E 并行实现/Debug**：同一需求/Bug 交给两个 agent，从同一基线各建 worktree，独立执行测试，比较后择优采纳。
**场景 F 后台与外部**：主界面隐藏时后台 Core 继续执行；手机 App 可复用任务状态与控制语义。外部终端 Agent 观察仍单独规划，不依赖悬浮岛。
**场景 G 硬件伴侣**：用户离开主界面后，通过桌面硬件查看各 Agent、额度和工作目录；选择一个 Task 后以三次单击完成语音 Prompt，长按改变下一轮 Agent；待审批时在对应目录直接单击批准或长按拒绝。

## 6. 核心领域模型

见 §4 术语表；补充关系：一个 Task 关联多个 Agent Session；每个 Turn 原则上一个主执行 agent；Comparison Run 含 N 个 Candidate Run，各自绑定 Candidate Workspace 与基线 Checkpoint；当前 Review Run 绑定审查类型、目录和 Git 比较范围/轮次线索；未来候选评审才绑定冻结候选版本。同线程切换不产生分支：Task 是单一线性时间线，分支化需求由 Parallel Compare 在工作区层满足。

## 7. 总体架构与技术栈

```text
Tauri 2 Desktop（主窗口可隐藏至托盘，任务继续运行）
├── React 主窗口
├── Platform Shell / Overlay
│   ├── macOS：WebviewWindow + objc2-app-kit（必要时 Swift NSPanel helper）
│   └── Windows：WebviewWindow + windows-rs
└── Rust Host
    ├── Versioned UI Contract Adapter（Tauri command/event）
    └── Rust Core（独立 crate，不依赖 WebView 或平台窗口类型）
        ├── Project / Task Service
        ├── Agent Registry & Runtime Supervisor（进程监督/故障隔离/孤儿清理）
        ├── Capability Registry
        ├── Canonical Event Bus（append-only logical stream）
        ├── Collaboration Orchestrator
        │   ├── Handoff / Reconciliation / Delegation / Review / Parallel Compare / Cross Review
        ├── Checkpoint & Candidate Workspace Manager
        │   ├── Git Worktree · 非 Git 目录副本(CoW) · Candidate Delta/Result Commit/Patch
        ├── Quota Center（QuotaProvider 与会话 Driver 解耦）
        ├── Companion Projection（桌面、手机 App 与未来硬件共享的版本化裁剪状态）
        └── SQLite Projection Store · Canonical Event Store · Raw Event Recorder（脱敏、可回放）

Future Companion Adapter（H1）
├── Device Gateway（配对、scope、幂等与版本校验）
├── USB / Wi-Fi Transport（共享同一逻辑协议）
└── Transcription Provider（音频转写；不进入 Agent Driver）

Agent Runtime Drivers
├── NativeRpcDriver（Codex App Server、OpenCode Server、DeepSeek 协议）
├── RustSdkDriver（GitHub Copilot）
├── AcpDriver（Cursor/Kimi/Kiro/Trae/Goose/Augment/Cline/…）
├── SupplementalSdkDriver（Qoder Credits、各 SDK 额度/缺口补充）
└── HeadlessJsonDriver（无更稳协议时）
```

技术栈：Tauri 2 + React/TS；官方 ACP Rust SDK；SQLite WAL+FTS5；tokio + portable-pty（登录终端/PTY 兜底）；无第三方 Agent 凭据存储组件。Core、版本化 UI 契约和平台 Overlay 分属不同 crate；TS/Python SDK 经统一可选 sidecar host 按需加载，仅用于额度采集或补齐 ACP 明确缺失能力。

版本化契约必须与 Tauri API 解耦，至少包含 `schema_version`、单调递增事件序号、稳定实体 ID、幂等键、错误类别和未知字段保留策略。首版 Core 与桌面 UI 分属独立进程，本地 IPC 必须保留幂等键、流版本、事件重放和权限边界。

## 8. Driver 与能力模型

**通道选择谓词（按序）**：① 能否复用用户已配置好的官方 Agent；② 是否覆盖主会话承诺能力；③ 是否官方稳定接口；④ 是否保留审批/Diff/提问/终端/计划等原生语义；⑤ Rust/Tauri 接入与维护成本。PTY 只用于官方登录或最后受限兼容。

**推荐**：官方语言无关富协议且稳定 → Rust 直连；官方 Rust SDK 且复用原配置 → Rust SDK；官方 ACP 满足主会话 → ACP 默认；SDK 提供额度或缺口补充 → Supplemental Driver；PTY 只作登录/兜底。

**Capability 模型**：streaming_text、session_resume、session_fork、cancellation、model_selection、modes、tool_events、approvals、user_questions、structured_diff、terminal_stream、plan_updates、todo_updates、subagent_events、read_only_mode、usage_events、quota_remaining、custom_extensions。每项能力除布尔可用性外，还应包含 `support_level`（native/adapter/polyfill/prompt-only/unsupported）、来源版本和已知限制。**只读能力必须说明保证来源**（agent 原生保证 / 沙箱保证 / 仅提示约束），仅提示约束时 UI 必须显示风险。

## 9. Agent 集成范围与路线

### 9.1 第一批（优先队列；M1 纵向切片 + M2 铺开）

| Agent | 默认主会话通道 | 补充通道 | 认证（用户在 agent 侧完成） | 状态目标 |
|---|---|---|---|---|
| Claude Code | ACP 生态适配器（`@agentclientprotocol/claude-agent-acp`，非 Anthropic 原生 ACP） | Agent SDK 仅补已验证缺口 | Claude Code 内登录 | Beta→Production |
| Codex | **App Server JSON-RPC（M1 切片）** | ACP 兼容 | Codex 内登录/配置 | Production |
| Cursor | **原生 ACP（M1 切片候选）** | SDK Bridge 仅补能力 | Cursor CLI 登录 | Production |
| Antigravity CLI | 官方 Headless `stream-json` / `input-format stream-json` | Python SDK 仅研究，不作已安装 CLI 的默认替代 | Antigravity 内登录 | Beta |
| Grok Build | 原生 ACP（beta） | headless/原生扩展 | Grok Build 登录 | Beta |
| Kimi Code | 原生 ACP（切片候选） | `kimi web` Server API（M0 核实） | `kimi login` | Production |
| ZCode | 待稳定官方接口（主动对接 Z.ai） | 实验 Adapter | ZCode 内登录 | Experimental |
| OpenCode | Server API/OpenAPI/SSE | ACP 兼容 | OpenCode 内配置 provider | Production |
| DeepSeek Harness | 官方 JSON-RPC/SDK 协议（Developer Preview） | ACP 仅在能力更合适时使用 | Harness 内自行配置 | Beta |

表格顺序代表产品接入优先级；M1 为验证不同 Driver 类型而选择 Codex、Cursor、Copilot 三条纵向切片，不改变上述首批队列的总体优先级。

### 9.2 第二批（M2~M3）

| Agent | 默认主会话通道 | 补充通道 | 状态目标 |
|---|---|---|---|
| TraeCode | 原生 ACP | 供应商扩展 | Production |
| CodeBuddy | 原生 ACP / HTTP API（M0 核实） | SDK 补能力 | Production |
| Qoder | 原生 ACP | SDK Credits 额度（复用现有登录态成功时为 Provider-Reported） | Production |
| GitHub Copilot | **官方 Rust SDK（连接用户已有 CLI）** | 原生 ACP | Production |
| Goose | ACP | 自定义请求 | Beta |
| Kiro CLI | 原生 ACP + `_kiro.dev/*` | 无 | Production |
| Augment / Auggie | 原生 ACP | SDK 补能力 | Production |
| Cline | 原生 ACP | SDK 仅补能力 | Beta |

### 9.3 集成状态分级

Production / Beta / Experimental / **Waiting for Vendor**（缺公开接口或上游支持）/ Unsupported。产品支持状态由兼容矩阵决定，不因某 Agent 出现在 ACP Registry 就自动标记 Production。

### 9.4 M0 待核实清单

Kimi `kimi web`/Server API 的稳定性与认证复用 · CodeBuddy HTTP ACP · Grok Build ACP 能力与订阅额度形态 · ZCode headless/ACP 规格（对接 Z.ai）· Claude ACP 适配器的会话能力与实际额度池 · Qoder SDK 是否可在 Hub 不接触 Token 的前提下复用现有 CLI 登录态 · DeepSeek JSON-RPC 的取消/审批/版本兼容降级 · Codex App Server/Cursor ACP/Copilot Rust SDK 最小探针。

已核实但仍须固定版本验证：官方 ACP Rust SDK存在；Antigravity CLI 支持结构化持续输入/输出；Copilot 官方 Rust SDK存在；Qoder 提供 ACP 和 Credits SDK；DeepSeek Harness 提供官方 JSON-RPC SDK 协议。Copilot Rust SDK首版必须关闭默认 runtime 捆绑或证明其只连接用户已有 CLI，避免把另一份 Copilot runtime 当作 Hub 自带 Agent重新分发。

## 10. 功能需求

### 10.1 集成与生命周期

- **FR-1.1 (P0) Provider SPI**：discover / getConfigurationStatus / getAuthStatus / openOfficialLogin / runDoctor / getCapabilities / listSessions / createSession / resumeSession / prompt 流式 / interrupt / respondToPermission / respondToUserQuestion / listModels / setModel / listModes / setMode；四档 Driver。Hub 只承载官方登录流程，不实现或托管第三方认证。
- **FR-1.2 (P0) 接入优先队列**：见 §9。
- **FR-1.3 (P0) 注册表与能力探测**：manifest 驱动（发现/版本/启动命令/认证形态/capability），启动扫描状态矩阵；矩阵可更新不写死。
- **FR-1.4 (P1) 安装/升级引导**：官方渠道；不重分发专有软件。
- **FR-1.5 (P0) 认证——外部自治（§3.1）**：无凭据表单；未配置 agent 只提供"打开官方登录/配置流程"；受控登录 PTY 完成即销毁、输出不入普通日志；探测用无敏感信息状态命令。
- **FR-1.6 (P2) 社区 Driver/Collector 插件机制**。
- **FR-1.7 (P1) Native Rich**：Codex App Server（历史/rate-limit/usage）、OpenCode Server（历史）、Copilot Rust SDK；Claude 官方 statusLine 结构化额度字段与 Qoder Credits SDK 作 QuotaProvider 候选。所有补充通道必须验证复用现有 Agent 配置且不要求 Hub接触凭据。

### 10.2 权限与安全

- **FR-2.1 (P0) 审批透传**：只透传 agent 提供的选项。
- **FR-2.2 (P0) 信任档位**：映射各 agent 原生模式，标注"由 agent 权限机制执行，非 Hub 安全边界"；不提供跨 agent 统一语义承诺。
- **FR-2.3 (P0) 进程边界**：工作目录为上下文约束（非沙箱宣传）；本机回环；结构化 argv 不拼 shell。
- **FR-2.4 (P1) "总是允许"透传**：仅写入 agent 原生配置。
- **FR-2.5 (P0) 内嵌登录终端**：portable-pty 仅承载交互式登录，完成即销毁；输出不持久化/脱敏、不记录令牌、不自动解析凭据；浏览器 OAuth 优先。
- **FR-2.6 (P1) 配置变更防护**：写 agent 配置/hooks 前展示 diff、备份、可撤销。

### 10.3 项目、会话与 Checkpoint

- **FR-3.1 (P0) 多项目**：注册/列表/搜索/置顶；分支与工作区摘要；每项目独立任务列表、默认 agent、最大并发设置；移除只删引用。
- **FR-3.2 (P0) 任务与会话**：创建/重命名/归档/永久删除；仅已归档会话展示删除入口，确认后不可恢复，清理历史、报告、Diff、基线与快照，移除最近删除界面；共享内容按引用保留，不删除项目文件与 Agent 原生历史；活动主任务或审查期间拒绝删除。双层会话模型及重启恢复已实现；置顶/归档仍保存在 localStorage。历史搜索已实现，FTS5 属后续工程增强。
- **FR-3.3 (P0) Git 读侧**：每轮 diff 与变更文件；会话记录 branch/worktree 归属。
- **FR-3.4 (P0/P1) 并发写策略**：P0 允许不同 Agent，或同一个 Agent 的多个独立对话，同时修改同一项目的共享工作目录；不以项目正在执行为由阻止另一对话发送，不强制排队或创建 worktree。单个对话仍只允许一个活动主执行，Session、时间线、审批及取消按对话和轮次隔离。共享目录的变更不归因为某个 Agent 独占修改，也不承诺自动消除并发编辑冲突。P1 worktree 管理服务于用户明确选择的 Parallel Compare/写委派，不替代普通对话的共享目录并发。
- **FR-3.5 (P1) 会话导入**：未实现、待排期；**FR-3.6 (P2) 导出 Markdown/JSON**：按用户决定暂缓。
- **FR-3.7 (P1) Checkpoint**：完整捕获并可重建基线 commit、staged/index、unstaged、untracked、文件模式/符号链接和子模块状态；不改用户 HEAD、index 或分支历史；作为评审与 Parallel Compare 的共同基线。
- **FR-3.8 (P1) Checkpoint 物化与校验**：普通 worktree 不能代表 dirty 工作区。Hub 必须使用临时 index/Git object 或等价机制创建不进入用户分支历史的 `BaselineTree`，另存 staged/unstaged 分层信息和 untracked 内容；每个候选从同一 BaselineTree 物化，恢复完成后逐文件校验。默认不复制 ignored 文件，排除清单必须可见；任务显式需要时由用户选择纳入。dirty 子模块必须递归快照，或阻止写入型并行并说明原因，不能只记录 Hash 后继续。
- **FR-3.9 (P1) Candidate Delta 不变量**：候选可采纳内容始终定义为 `CandidateFinalTree - BaselineTree`。用户在 Checkpoint 前已有的修改属于基线，不得在采纳时二次写回；隐藏 BaselineTree/Checkpoint commit 不得被 merge、cherry-pick 或显示为用户提交历史。

- **FR-3.10（已实现，D10）项目运行方式自动分析与一键运行**：扫描项目说明、配置、锁文件线索和脚本，由所选 Agent 分析适合当前平台的启动、调试、构建、测试及部署命令；按目录分组展示名称、命令、工作目录和来源。点击运行项后在内嵌终端执行，支持多个输出标签、停止和结束后再次运行。结果按项目及平台持久化；支持重命名、固定、删除和重新分析，重新分析保留固定项，覆盖用户修改前确认。调试指执行已有调试命令，不表示内置断点调试器。

### 10.4 统一对话界面

- **FR-4.1 (P0) 三栏主界面**：项目/任务 ｜ 统一时间线/候选比较 ｜ Agent/额度面板。时间线展示消息、计划、工具、终端、Diff、测试、审批、提问、评审、候选与错误；每事件标注 agent、模型、session、turn、工作目录与协作模式。
- **FR-4.2 (P0) 对话流**：流式、工具卡片、diff 审阅、审批弹窗、取消；降级卡片 + 脱敏诊断视图。
- **FR-4.3 (P0) 发送区协作模式选择器**：`普通发送 | 切换 Agent | @委派 | 请求评审 | 并行比较`；显示本轮 agent、模型、模式、工作目录与认证状态。
- **FR-4.4 (P0) 每轮切换（同线程）**：统一时间线内切换执行者，不产生任务分支；同 agent 原生 resume；跨 agent Handoff；切回 Reconciliation（细则见 §11）。
- **FR-4.5 (P1) 上下文预算**：交接强度精简/标准/完整三档，可预览编辑。
- **FR-4.6 (P2) 通知、快捷键、命令面板**。
- **FR-4.7 (P2) @Delegation 完整形态**（只读版 P1 随 M1 交付，见 §11.3）。
- **FR-4.8 (P1) Canonical History 按需检索**：当前 Agent 可在用户可见、可审计的上下文通道中按 Turn、决策、Artifact、文件或 Checkpoint 检索统一历史；实现可选 Driver 注入、临时上下文文件或 Hub 内置只读 MCP，但不得要求供应商 Agent 原生共享 Session。
- **FR-4.9 (P0) 无隐式 Hub 模型调用**：Hub 确定性生成结构化上下文；若请求某个 Agent 生成上下文语义摘要、比较摘要或综合方案，UI 必须显示执行 Agent、额度作用域和任务来源，并按普通 Agent Turn 记录。Git 提交备注的“AI 总结”由用户主动触发并使用用户选择的 Agent，不要求另建 Turn/ExecutionRun、保存输入差异、输出 Artifact 或执行审计记录；此功能不纳入摘要执行留痕要求。

### 10.5 额度中心（只读）

- **FR-5.1 (P0) 数据模型**：provider、account_label、plan_name、billing_surface/quota_scope（interactive/headless/agent-sdk/extra-usage/unknown）、window_kind（5h/周/月/credits/premium requests/模型专项）、used_ratio、remaining_ratio、remaining_units+unit、model_scope、resets_at、source、confidence（ProviderReported/Derived/Estimated/Unknown）、parser_version、collected_at、stale_after。原始供应商值同时保留，归一化字段不得覆盖原始语义。
- **FR-5.2 (P0) Collector**：与 Driver 解耦；来源分级（官方 API/SDK > App Server/RPC > 官方 CLI/状态行结构化状态 > 本地结构化状态 > 本地估算 > 不可用）；刷新不发模型 Prompt；失败不影响会话。只能打开但不能结构化读取的官方额度页面属于外部链接，不得冒充 Quota Snapshot 数据源。
- **FR-5.3 (P0) 额度卡片**：账号/计划/额度作用域/窗口/进度/倒计时；Estimated 显著标注且不与 ProviderReported 混排；不把 session token、上下文占用、美元成本当订阅余额；百分比不伪化精确条数；过期快照显示“上次已知”而非当前值；**Parallel Compare 启动前显示并发 agent 数量与各自额度快照（不阻止用户，不伪造统一成本）**。
- **FR-5.4 (P1) 首批适配**：Codex（App Server `account/rateLimits/read`）、Claude Code（官方 statusLine `rate_limits` 结构化字段，首次模型响应前可能不可用）、Qoder（仅在不要求 Hub接触凭据且能复用 CLI 登录态时使用 SDK Credits）；codex-host 采集逻辑逐字段审计后参考。
- **FR-5.5 (P1) 阈值提醒/趋势**；**FR-5.6 (P2) 额度感知推荐（用户确认）**。

### 10.6 悬浮岛（已移除）

2026-10-16：按用户要求删除独立悬浮窗口、主窗口启用入口及其专属代码，不再安排 FR-6 的窗口、多屏、快捷审批或平台扩展。历史实现与验收记录不再作为开发要求。桌面和手机 App 可复用的 Core 命令、后台服务通信、任务/审批状态投影继续保留，见[移除记录](implementation/island-removal-2026-10-16.md)。

### 10.7 Companion Hardware（产品需求；设备实现属于 H1）

#### 10.7.1 定位与边界

- **FR-7.1 (H1/P0) 受限控制端**：硬件是 dogcodes 的状态与快捷交互终端，不是独立 Coding Agent、通用远程桌面或 Agent 协议客户端。设备只连接 dogcodes Device Gateway，不直连 ACP、Codex App Server、Agent CLI 或供应商服务。
- **FR-7.2 (H1/P0) Agent 自治不变**：所有 Prompt、Agent 切换和审批最终仍由 Core 调用既有 Driver；权限决定由 Agent 执行。硬件不得持有、读取或迁移 Agent Token、Cookie、API Key、原生会话库或安装配置。
- **FR-7.3 (H1/P0) 无独立事实源**：主界面、手机 App 与硬件消费同一 Canonical Event 派生状态；设备缓存只用于显示，不成为 Task、审批、额度或会话事实源。

#### 10.7.2 状态、项目与额度显示

- **FR-7.4 (H1/P0) Agent 总览**：显示已安装/已配置 Agent 的名称与状态，至少区分未配置、空闲、启动、排队、思考/生成、工具执行、等输入、等审批、完成、失败、断开和额度信息过期。
- **FR-7.5 (H1/P0) 工作目录总览**：按 Project/Task 显示用户可辨识的工作目录短名、分支（用户可关闭）、当前运行 Agent、下一轮默认 Agent、运行时长、次级 Run 数及待交互数。当前运行与下一轮默认不同必须明确显示，例如“运行：A；下一轮：B”，不得统称“当前 Agent”。
- **FR-7.6 (H1/P0) Device Focus**：语音、切换和审批只作用于设备当前明确聚焦的 Project/Task。设备必须提供可理解的焦点切换与确认反馈；具体旋钮、按键或触控结构由硬件设计阶段决定。
- **FR-7.7 (H1/P0) 额度显示**：复用 Quota Center 的只读 Snapshot，显示计划/额度窗口、剩余量或比例、重置时间、可信度和 stale 状态；不得直接向供应商取凭据、触发模型 Prompt、把估算伪装成供应商精确值或把不同额度池合并。
- **FR-7.8 (H1/P1) 低注意力提示**：待审批、失败、断开和额度阈值可用颜色、图标、动画或设备支持的提示方式区分；勿扰与亮度策略可配置，不得用无法关闭的声音持续打扰。

#### 10.7.3 语音 Prompt

- **FR-7.9 (H1/P0) 三次单击流程**：无待审批冲突时，第一次单击开始录音；第二次单击结束录音并开始转写；硬件显示转写文字；第三次单击确认发送。录音、转写、待确认、发送、成功、失败与取消必须有明确状态反馈。
- **FR-7.10 (H1/P0) 发送前确认**：未经第三次单击不得产生 Agent Turn。转写文字支持滚动/分页，不能只显示截断片段后允许盲发；失败、超时或用户取消均不得发送。
- **FR-7.11 (H1/P0) 目标冻结与复核**：录音开始时记录 Device Focus、Task 版本和目标 Agent；发送前重新校验。期间焦点、Task 或 Agent 已变化时必须重新显示目标并要求确认，不得静默发往另一个 Agent。
- **FR-7.12 (H1/P0) 正常 Turn 语义**：确认后的 Transcript 作为普通 User Prompt 进入 Canonical History和既有排队/执行流程；若目标与可恢复 Session 不同，由 Core 正常执行 Handoff。硬件不得绕过 Handoff 或把 Prompt 注入 Agent stdin。
- **FR-7.13 (H1/P1) 音频与 Transcript 隐私**：界面明确显示转写发生位置和是否会离开本机；默认不得因使用硬件而引入新的第三方 API Key。原始音频默认在转写成功或失败后删除，不进入 Canonical History；取消的 Transcript 不作为用户消息保存。

#### 10.7.4 下一轮 Agent 选择

- **FR-7.14 (H1/P0) 长按选择**：无待审批冲突时，长按进入 Agent 选择；只列出已安装、已配置且对当前 Task 可用的 Agent，不可用项显示原因。
- **FR-7.15 (H1/P0) 只影响下一轮**：选择只修改 Task 的 `preferred_executor`。当前已启动 Run 的执行 Agent被冻结，不被中断、重启或改投；设备明确显示“下一轮：Agent B”。下一轮跨 Agent 时由 Core执行 Handoff，切回旧 Agent 时执行 Reconciliation。

#### 10.7.5 快捷审批与输入优先级

- **FR-7.16 (H1/P0) 待审批显示**：硬件在对应工作目录区域显示高优先级 `waiting_for_approval`，至少包含 Agent、目录短名、审批类型、经过脱敏的命令/工具摘要、有效/过期状态以及“单击批准 / 长按拒绝”提示；长内容支持滚动/分页。
- **FR-7.17 (H1/P0) 单击批准、长按拒绝**：对可以安全映射为唯一正负选项的有效请求，单击提交 Agent 原生 `approve_once` option，长按提交 Agent 原生 `reject/cancel` option。动作只解决该次 `PendingInteraction`，不等于取消整个 Run；`approve_persistent/总是允许` 永不作为硬件单击默认。
- **FR-7.18 (H1/P0) 审批优先级**：当前焦点存在可快捷处理的审批时，审批覆盖普通按键语义——单击不得开始录音，长按不得打开 Agent 选择。请求被解决、过期或撤销后立即恢复普通按键语义。
- **FR-7.19 (H1/P0) 防误批**：多个工作目录或多个审批必须先聚焦具体请求，禁止批量批准。复杂多选、结构化表单、摘要不足、没有明确正负 option 或设备离线时只显示“需桌面端审批”，任何单击/长按均不得发送决定。
- **FR-7.20 (H1/P0) 一次且可竞争**：响应携带 interaction ID、Agent 原生 option ID、幂等键、expected interaction version 和 Task version；主界面、手机 App、硬件或 Agent任一端先处理后，其他端的迟到输入显示已处理/已失效，不得重复送达或串到下一请求。

#### 10.7.6 连接、离线与配对

- **FR-7.21 (H1/P0) 第一代连接**：主连接支持 USB 有线或 Wi-Fi，二者复用同一逻辑 Companion Protocol 和命令语义；具体有线设备类别、编码和发现机制属于设计决策。BLE 不作为第一代语音/控制主链路。
- **FR-7.22 (H1/P0) 运行条件**：Core 后台服务运行时硬件可继续使用，桌面 UI 退出不影响 Agent 或硬件连接；服务不可用时设备明确显示 Offline。Core 服务独立持有执行与状态，硬件仍经 Device Gateway 接入。
- **FR-7.23 (H1/P0) 离线安全**：离线可以显示带 `stale` 和最后更新时间的缓存状态，但不得审批、发送 Prompt 或修改 Agent。设备不本地排队待发送 Prompt/审批，重连后不得自动补发未确认、未送达或已过期命令。
- **FR-7.24 (H1/P0) 配对与撤销**：首次 USB 或 Wi-Fi 连接都需桌面端确认设备身份/短码；每台设备独立身份、独立 scope、可单独撤销。插入 USB 不等于自动获得全部权限。
- **FR-7.25 (H1/P0) 最小授权**：状态、额度、焦点、Agent 选择、音频、Prompt 和审批分别授权；审批至少要求 `interaction.read` 与 `interaction.respond`。设备不得获得 Tauri 命令全集、Event Store 查询或任意文件访问。

#### 10.7.7 数据最小化、兼容与恢复

- **FR-7.26 (H1/P0) 数据最小化**：默认不下发绝对路径、账号邮箱、完整会话、代码、Diff、终端、Raw Event 或 Agent 凭据。例外是完成当前操作必需的语音 Transcript 与脱敏审批摘要，且只发送给已配对设备。
- **FR-7.27 (H1/P0) 版本化同步**：设备状态带 revision；漏收增量时请求完整 Snapshot。缓存数据必须标记 stale，未知必需协议能力不得宽松猜测执行。
- **FR-7.28 (H1/P1) 设备管理**：桌面设置页显示设备名称、型号、固件/协议版本、在线状态、最近连接、连接方式、授权 scope，并支持重命名、撤销、重新配对和诊断。
- **FR-7.29 (H1/P1) 固件安全**：如产品提供固件升级，升级包必须验证发布者签名；设备不得向桌面上传可执行代码。

#### 10.7.8 软件首版架构要求与硬件阶段非目标

- **FR-7.30 (M1/P0) 首版必须预留**：首版 Core/Protocol 必须区分 `preferred_executor` 与 active Run executor；统一 `PendingInteraction`、原生 option ID、interaction version；统一 `CommandEnvelope` 的 Actor、幂等键与 expected version；维护供桌面、手机 App 和未来硬件消费的版本化裁剪状态 Projection；Quota Snapshot 可低带宽映射。上述内容必须进入 M1 测试，不能等 H1 再重构。
- **FR-7.31 (M1/P0) 不提前堆空实现**：M1 不要求 LAN监听、USB 驱动、设备配对、音频编码、转写模型、固件升级或硬件设置页，也不要求创建无实现的硬件 crate；这些属于 H1 Adapter，不得反向改变已固定的 Core 语义。
- **FR-7.32 (H1 非目标)**：首代不做云端设备中继、多用户远程控制、完整代码/终端浏览、设备端 Agent Runtime、设备端凭据管理、离线 Prompt 队列或“一键全部批准”。

## 11. 多 Agent 协作模型（细则）

### 11.0 Canonical History 与上下文构建

Hub 保存 Canonical History，但不假设不同供应商能迁移原生 Session。结构化上下文由 Hub 确定性构建，包括原始需求、用户约束、Turn/Artifact 引用、参与 Agent、文件清单、Git diff/stat、依赖变化、测试结果和待办。Hub 不自行调用隐藏模型。

需要语义压缩时，默认可请求离开当前任务的 Agent生成摘要；也可由用户指定其他已安装 Agent。该摘要是一个可追溯的普通执行，会消耗对应 Agent的额度，并必须保留输入范围、输出和来源。摘要失败时回退到确定性模板，不阻塞切换。

Canonical History 至少提供 `getTurns`、`searchHistory`、`getArtifact`、`getDecisionLog`、`getChangesSinceCheckpoint` 五类只读能力。M1 可由 Hub主动选择内容注入；M2 前必须提供当前 Agent按需取回历史的通道，避免所有历史都被一次性塞入 Prompt。

### 11.1 同线程切换：Handoff

同 agent 连续优先原生 resume/load。切换时新建/恢复目标原生会话；Handoff 至少含：目标、用户约束、已完成工作、技术决策、待办、相关文件、Diff 摘要、测试结果、必要最近对话及 Canonical History 引用。结构化字段由 Hub确定性产生，语义摘要来源遵循 §11.0。UI 明示发生切换、交接强度、生成来源和预估/实际额度作用域。切换不产生任务分支。

### 11.2 切回旧 Agent：Reconciliation

恢复旧会话后，先确定性计算该会话已交付事实与当前冻结历史之间的差集，作为用户指令前的上下文包：期间参与 agent、文件变化、diff 摘要、依赖变化、测试结果、新决策、未完成事项及相关 Artifact 引用。增量按事实交付范围计算，包含执行游标之前仍未交付的事实；不重复完整已交付事实。正常发送自动交接，不要求逐次弹窗预览确认；显式编辑/关闭作为单独操作记录。若使用 Agent生成语义解释，必须按 §11.0 单独记录并显示额度作用域。

### 11.3 子任务委派：Delegation

当前已交付形态是审查型只读委派：从已完成轮次或项目 Diff 发起，选择既有 Agent，在独立 Session/Run 中执行，不改变主任务 Agent。支持独立取消、按原比较范围重试和带来源的结果回流。

通用 `@Agent` 调研/分析、写入型委派及候选执行空间属于后续独立规划，未排入当前首版收尾。原生子 Agent 观察不能替代这些通用委派能力，也不能据此否定已实现的独立审查。

### 11.4 指定评审：Review

**当前入口**：仅代码审查、安全检查、性能分析；逐轮入口与项目 Diff 面板共用图标、既有 Agent 下拉和额度展示。检查测试、对照要求、评估影响三个入口已移除。盲评、自定义 Profile、指定文档或 Candidate 评审不属于当前已交付范围。

**审查范围**：Agent 在项目目录使用读取、搜索和 Git 查询工具，自行获得差异及必要上下文。项目 Diff 只传目录、模式、Git 引用和文件线索；历史提交比较选定父提交与目标提交，首次提交比较全部新增内容，分支比较沿用面板确定的 base/target。逐轮入口传本轮要求/回复摘要及文件线索，检查当前实现，允许纳入后续修改。非 Git 文件只按线索审查当前代码，没有旧内容时不得声称还原历史差异。无需保存审查专用代码基线；历史 Diff 和非 Git 撤销所需的原始内容仍保留。

**项目 Diff 范围选择**：按“目录 → 比较方式 → 提交/分支”级联。多目录默认所有目录，单目录隐藏目录选择；所有 Git 目录可汇总未提交/未暂存/已暂存，混合项目默认汇总 Git 未提交与非 Git 会话变更，暂存相关模式仅纳入 Git 并标明范围。指定 Git 目录后提供历史提交和分支比较；非 Git 目录只提供会话变更，无基线或读取失败不得表示为没有改动。切换目录清空提交/分支选择，保留适用模式；文件列表、统计及新审查输入使用同一范围。

**执行与输出**：每次一次独立原生 Session/Run，使用原生完全权限，允许跨目录读取；提示词明确只审查、不修改代码。权限请求在独立审查会话中采用本次允许，不改变主对话策略。界面不展示范围材料/权限实现说明。取消宿主分组、冻结上下文检索、强制独立 AI 二次复核及逐文件覆盖矩阵。保留结构化问题、严重度、文件行号和修复建议；位置由 Agent 报告，不冒充冻结位置校验。非结构化报告仍可显示，不因缺少 JSON 判定执行失败。结果保留来源、取消/重试和主对话回流。 审查过程复用主对话的流式回复与工具执行状态组件；完成后展示审查报告并可展开执行记录，失败、取消或连接中断保留已有输出，不将未完成工具标成成功。

**修复闭环**：用户选择问题交给原对话主 Agent 修复，记录关联轮次；复审检查当前代码与原问题，保留旧报告。工作区比较的重试重新读取当前代码；历史 Git 比较重试沿用原对象。不自动将零问题标记为所有原问题已解决。旧快照报告可查看，但不再重放旧流程，应从入口发起新审查。

实施与验证见 [审查简化记录](implementation/review-simplification-2026-09-27.md)。原分组/冻结检索/独立复核见带日期历史记录，不再作为当前要求。

### 11.5 并行比较：Comparison Run

**创建**：选 ≥2 个 agent；所有候选接收同一版本 Common Task Package；agent 专属配置/模型差异记录但不改变任务含义；可统一取消或单独取消/重试；**默认并发 2、全局上限 4（项目可更低），超出排队**；启动前提示将同时调用多个 agent 并显示各自额度快照（不伪造统一成本，不阻止选择）。

**Checkpoint 与执行空间**：Git 目录（含文档/设计任务）一律按以下流程建立候选：

1. 读取当前 HEAD、index、工作树、untracked、文件模式/符号链接与子模块状态，不修改用户 HEAD/index；
2. 使用临时 index/Git object 或等价机制生成隐藏 `BaselineTree`，同时保留 staged/unstaged 的分层快照；
3. 从同一 BaselineTree 为每个候选创建独立 worktree + 临时分支；
4. 恢复并校验任务允许的 untracked 内容；ignored 文件默认排除且清单可见；
5. 任一候选物化校验失败时停止该候选，不得退回共享可写目录。

纯聊天无目录→独立 Session 不建空间；非 Git 目录→明确只读候选可共享，任何可能写文件的候选自动复制独立临时目录（CoW/reflink 优先，排除规则可见）；**Workspace 创建失败不得回退共享可写目录**。候选完成前默认互不可见。

**文档/设计比较**：每 agent 产出独立版本化文档 Artifact；比较视图支持并列阅读、结构提取、差异摘要、用户批注；可择优、人工组合，或指定 agent 生成综合方案（必须引用来源候选）。

**代码/Debug 比较**：启动建 Checkpoint/BaselineTree → 每候选独立 worktree/临时分支 → 独立执行与测试 → 候选完成时冻结 **Candidate Result Commit**（agent 未提交的最终状态由 Hub在临时引用生成）。同时生成 `Candidate Delta = CandidateFinalTree - BaselineTree`；所有采纳方式都只处理 Candidate Delta，绝不再次引入用户开始比较前已有的 dirty 基线。**采纳方式三选一**：

| 方式 | 写回内容 | 候选提交历史 | 典型用途 |
|---|---|---|---|
| 合并候选提交 | 将 Checkpoint 之后由候选产生的有效提交按顺序移植到目标分支；不得合并隐藏 Checkpoint/BaselineTree 提交 | 保留 | 候选提交拆分清晰、目标工作区满足安全更新条件 |
| 应用完整变更 | Checkpoint→候选终态的全部变化一次应用 | 不保留（用户自行提交或经确认生成单提交） | 只要结果、主分支历史简洁 |
| 暂不采纳 | 不写回 | — | 仅比较/稍后处理 |

完整变更覆盖新增/修改/删除/重命名/文件模式、符号链接与可版本化产物；首版不做部分采纳（部分提交/hunk）。采纳前必须展示提交/Candidate Delta、目标分支漂移与预计冲突，用户确认后执行。目标相对 Checkpoint 漂移时，以 BaselineTree、CandidateFinalTree 与 TargetCurrentTree 做三方合并/应用，**冲突立即停止自动写入进入用户处理**。

“合并候选提交”仅在目标引用可安全更新时可用；目标工作区存在无法安全保留的未提交状态时，不得自动 stash、reset 或强行更新分支，UI 应禁用该方式并说明用户可先自行提交/整理，或改用“应用完整变更”。移植时只处理 Checkpoint 之后的候选提交，Candidate Result Commit 可作为最后一个结果提交；隐藏 Checkpoint/BaselineTree 永不进入用户历史。

“应用完整变更”把 Candidate Delta 应用到 TargetCurrentTree，不保留候选内部历史；现有 staged/unstaged 状态应尽可能保持，无法无损保持时必须在写入前停止并要求用户处理。未采纳候选先持久化回复、Candidate Delta/Patch、测试、评审和元数据，再自动清理 worktree、临时目录与临时分支；胜出候选采纳并持久化后同样清理。

**比较维度**：回答/设计内容、变更文件与 diff、编译/测试/静态检查、耗时、agent/模型/模式、原生 usage 与额度快照、评审意见、用户评分与采纳状态。

### 11.6 交叉评审：Cross Review

候选全部完成后由用户启动。两候选：A 评 B、B 评 A，或指定第三中立评审者；**三候选及以上由用户手动指定任意已安装 agent 作为统一 Reviewer**，完整 N×(N-1) 互评仅高级选项。可选盲评（隐藏作者与模型）。只读冻结后的候选 Artifact/快照；意见附着目标候选并与候选自身输出严格区分；Comparison Summary 必须保留原始证据链接。自动多轮辩论属 P2 且须设轮数与额度上限。

### 11.7 并发与资源控制

默认并发 2、全局上限 4，全局/项目级可配；同一主工作区一个写入者；每候选独立监督/超时/取消；单候选失败不取消其他；资源不足排队不静默减员。

## 12. 非功能需求

- 事件到 UI 增量 P95 < 500ms；长响应全增量显示。
- Core 空闲内存 < 150MB；完整应用（不含 agent 子进程）< 300MB；10 万事件全文搜索 P95 < 500ms。
- 单 agent 异常不影响应用/其他候选/任务；主窗口隐藏不暂停任务；孤儿进程与遗留 worktree 检测，不自动破坏性清理。
- 安全：日志/DB 无 Token/Cookie/Key/完整敏感环境变量；结构化 argv；原始事件落盘前脱敏；hooks 与配置变更可预览/备份/撤销；外部采集显式启用。
- 兼容：每 agent 维护最小/推荐/不兼容版本；Driver 固定依赖 + 事件 fixture + 协议契约 + capability 测试；CI 不用真实订阅凭据；能力降级显式诊断。
- 平台：macOS 14+（签名公证 DMG + Homebrew）、Windows 10/11 x64（签名安装包 + 自动更新）。

## 13. 存储与事件模型

Canonical Event Store 是 Hub 协作状态的持久化事实源，SQLite Projection 是可重建查询视图；Raw Event Recorder 保存脱敏后的供应商原始帧，用于 fixture/兼容回放，不取代 Canonical Event。事件必须含 schema version、sequence、idempotency key 与来源 Driver 版本。大体积终端输出/Diff/测试日志/Artifact 内容寻址外置，库内只存引用。

Checkpoint 必须保存可恢复的 BaselineTree、staged/unstaged 分层快照、允许纳入的 untracked 内容、文件元数据、子模块策略与校验值；Comparison Run 保存 Common Task Package、全部候选、Workspace、CandidateFinalTree、Candidate Delta、Result Commit/Patch、评审、采纳和清理状态。Review Run保存被评审版本与报告；支持 FTS5、Markdown/JSON导出；删除 Hub任务不默认删除 Agent原生会话。Raw Event与大体积日志应有可配置保留期和磁盘上限，Artifact/采纳证据不随缓存清理丢失。

## 14. 主界面信息架构

```text
┌──────────────┬──────────────────────────────┬────────────────┐
│ 项目 / 任务    │ 统一时间线 / 候选比较           │ Agent / 额度面板 │
│ 项目 A        │ 消息/工具/Diff/测试             │ 当前 Agent      │
│  ├─ 任务 1    │ Delegation/Review              │ 模型/模式        │
│  └─ 比较任务   │ Candidate A│B│C + Cross Review │ 登录/额度窗口     │
│ 项目 B        │ 采纳操作                        │ 交接摘要         │
└──────────────┴──────────────────────────────┴────────────────┘
```

发送区协作模式选择：`普通发送 | 切换 Agent | @委派 | 请求评审 | 并行比较`。比较视图支持列表/分栏/聚焦单候选、同步查看状态/回复/Diff/测试、冻结版本、单独评审或互评、标记首选/淘汰/待定/已采纳、打开候选 worktree 与外部 diff 工具、显式执行三种采纳。**采纳确认页**展示：候选 agent/模型/共同 Checkpoint、引入提交数或完整 Patch、修改文件与 Diff、目标分支漂移、预计冲突、是否保留候选提交历史、采纳后将自动清理的资源。设置页无任何第三方凭据输入，只提供安装状态、版本、可用性、官方配置入口、能力矩阵与额度状态。

## 15. 里程碑

下列周期仅作为范围控制参考，不作为承诺日期；排期需在 M0 后根据团队规模、供应商版本和真实账号探针结果重估。若以 2 名熟悉 Rust/Tauri 与 Agent 协议的全职工程师为假设，建议预留如下区间。

| 阶段 | 范围 | 验收 |
|---|---|---|
| **M0（验证，建议 2~3 周）** | 建立两批 Agent 的公开协议/SDK/登录复用/额度/许可证/版本初始矩阵；对第一批和 M1 三切片做真实账号探针；Codex App Server、Cursor ACP、Copilot Rust SDK 最小探针；Claude额度作用域、Qoder免凭据复用、DeepSeek限制探针；定义 Canonical Event/Capability/Driver/UI Contract；验证 BaselineTree、worktree、非 Git复制、Candidate Delta、Result Commit、采纳和安全清理 | 高风险假设有实测结论；三切片跑通最小会话；dirty baseline 不重复写回；ACP切片最终选择 Cursor或记录替代理由 |
| **M1（核心纵向 MVP，建议 6~10 周）** | Tauri主窗口 + Rust Core 后台服务（独立进程、版本化本地 IPC）；项目/任务/时间线/SQLite；Codex NativeRPC + Cursor ACP + Copilot Rust SDK；同 Agent恢复、跨 Agent Handoff + Reconciliation；确定性上下文包与可追溯语义摘要；审批/Diff/终端/取消/崩溃恢复；同项目多 Agent/多对话共享目录并发与会话隔离；基础只读 Delegation；一个真实 QuotaProvider；完成 FR-7.30 的硬件兼容 Core 语义与 Companion Projection，但不实现设备链路。不要求第一批全量，不绑定窗口 | 单 Agent闭环 + A→B→A切换；三类 Driver各一条可用真实账号链路并标明 Beta/限制；当前 Run/下一轮 Agent、审批版本/幂等、裁剪状态投影通过测试；无隐式 Hub模型调用，不以 M1直接宣称 Production |
| **M1.5（历史规划，悬浮岛已退出）** | FR-6 窗口及快捷审批规划已移除；外部 Hook 观察另行排期 | 不再安排悬浮岛窗口、刘海或 DPI 验收；共享审批与状态同步继续由 Core 提供 |
| **M2（Checkpoint、评审与文档比较）** | 完整 Checkpoint/BaselineTree + Candidate Workspace抽象；Canonical History按需检索通道；指定评审、作者修复、复审闭环；Git文档候选独立 worktree；非 Git写候选临时目录；无目录聊天独立 Session；文档比较视图/候选冻结/Comparison Summary；第一批按队列扩展；2~3家真实额度 Provider | 场景 C/D全流程；“执行→评审→修复→复审”闭环；当前 Agent可检索历史 Artifact/决策/变更而非只依赖一次性交接 Prompt |
| **M2.5（代码/Debug 并行与采纳）** | 代码候选 worktree 并行；Result Commit 与完整 Patch；三种采纳方式；漂移检测/三方合并/冲突停止；自动清理；默认并发 2 上限 4 排队 | 场景 E 全流程；采纳确认页信息完整 |
| **M3（第二批与增强编排）** | 第二批接入；写入型 Delegation + worktree；两候选互评与多候选统一 Reviewer；≥3候选综合方案；社区 Driver/Collector SDK；可选 daemon/远程访问；跨项目长期记忆作为独立可选能力，不与单任务 Canonical History混淆 | 场景 B写路径全流程 |
| **M4（产品化）** | 签名公证 DMG + Homebrew（含自动更新通道）；Windows 签名安装包；安装诊断；隐私说明；Apache-2.0 LICENSE + 第三方许可证清单；崩溃/诊断日志导出 | 发布工程就绪，两通道可安装可升级 |
| **H1（配套硬件）** | 实现 FR-7.1~7.29、7.32：USB/Wi-Fi Device Gateway、配对与最小 scope、Agent/工作目录/额度显示、Device Focus、语音三次单击确认、下一轮 Agent 选择、硬件快捷审批、离线/重连、设备管理与协议兼容 | 场景 G 全流程；状态与 GUI 一致；运行 A 时可把下一轮设为 B 而不产生 interrupt；未第三击不发送 Prompt；待审批单击只一次性批准、长按只拒绝本次请求；复杂请求不误批；重放/过期/断线不重复命令；撤销设备立即失权 |

## 16. 验收标准（分域）

- **认证边界**：Hub 内无第三方 Key 输入/存储界面；未配置 agent 只能打开官方流程；日志与 DB 无可复用凭据；agent 配置后无需在 Hub 重复配置。
- **切换与对账**：A→B 生成 Handoff并记录版本、构建来源和语义摘要执行者；B→A恢复会话并发增量 Reconciliation；切换不产生分支，时间线连续；事件可追溯 agent/session/turn/基线；Hub不进行未披露的模型调用，摘要失败可回退确定性结构化包。
- **Delegation**：子任务不替换主 agent；可独立取消/失败/重试；回流保留来源与原始 Artifact；写型子任务不污染主工作区。
- **Review**：可指定 agent 与对象；评审按所选比较范围由 Agent 自行读取代码；独立 Session 与原生权限，不宣称统一的操作系统级只读保证；结构化问题/严重度/依据/来源；意见可选择性发回复审。
- **Parallel Compare**：同 Common Task Package；Git候选全在独立 worktree（含文档）；非 Git写候选在临时目录副本；无目录聊天不建空间；所有 Git候选由同一可重建 BaselineTree物化并校验；单候选失败不连坐；采纳只写入 Candidate Delta，不重复写回用户原有 dirty基线；隐藏 Checkpoint不进入用户历史；漂移展示三方合并预览、冲突即停；未经显式采纳不写回原工作区；未采纳候选持久化后自动清理；采纳与未采纳均有记录。
- **Cross Review**：只读冻结候选；意见标识评审者与目标；两候选互评、多候选统一评审者；可关闭作者显示。
- **额度**：来源/可信度/更新时间/额度作用域齐备；无可靠数据显示未知；过期数据显示“上次已知”；刷新不发模型请求；并行前显示多 agent额度快照；interactive/headless/agent-sdk 等额度池不得混为同一窗口。
- **硬件架构基线（M1）**：Task 明确区分 active executor 与 preferred executor；PendingInteraction 包含原生 option 和版本；Command 有 Actor、幂等与 expected version；Companion Projection 可重建、可按 revision 恢复并通过敏感字段裁剪测试；实现不依赖实际硬件 crate。
- **硬件状态与额度（H1）**：设备展示的 Agent、Project/Task、active/preferred executor、运行状态、待交互和 Quota Snapshot 与同一 Core revision 一致；过期/离线内容明确 stale；不得通过设备刷新消耗模型额度。
- **硬件语音（H1）**：第一次单击录音、第二次结束转写、显示文字、第三次确认后才创建且只创建一个普通 Turn；失败/取消/未确认/断线均不发送；焦点或 Agent变化时重新确认；原始音频按保留策略删除。
- **硬件 Agent 切换（H1）**：长按只列可用 Agent；运行 A 期间选择 B 不产生 interrupt，界面显示“运行 A / 下一轮 B”，下一 Turn才由 B 执行并遵循 Handoff/Reconciliation。
- **硬件审批（H1）**：对应工作目录显示待审批、Agent 与脱敏摘要；待审批时单击/长按优先解释为批准/拒绝而非语音/Agent 切换；请求 ID、版本、option 与 scope 全部校验；复杂请求不产生决定；重复、过期和已解决响应不会再次送达 Agent。
- **硬件连接与授权（H1）**：USB/Wi-Fi 使用相同命令语义；首次连接需确认；设备可独立撤销；离线不执行写命令且不重放未送达输入；被撤销设备无法获取新状态或响应请求。
- **发布**：DMG 与 Homebrew 两通道安装/升级通过；许可证与第三方清单齐备。

## 17. 风险清单

| 风险 | 应对 |
|---|---|
| Agent 版本频繁变化 | 版本矩阵、固定依赖、fixture、契约测试 |
| ACP 实现深度不一 | capability 检测、扩展事件、Supplemental Driver |
| SDK sidecar 过多 | ACP 优先、统一 sidecar host、仅补明确缺口 |
| 跨 Agent 上下文有损 | Handoff + Reconciliation + Canonical History按需检索 + 用户可审阅 |
| 评审意见误导 | 证据化输出、保留原始结果、用户最终决定 |
| 并行候选多份额度消耗 | 执行前显示 agent 数量与额度快照、可单独取消 |
| Checkpoint 遗漏或无法重建未提交内容 | 临时 index/BaselineTree + staged/unstaged分层快照 + untracked内容存档 + 子模块策略 + 物化后校验；Hash不能代替内容 |
| dirty baseline 在采纳时被重复引入 | 采纳只处理 Candidate Delta；隐藏 BaselineTree不得进入用户历史；目标不满足安全更新条件时禁用“保留提交历史” |
| 非 Git 目录复制成本 | CoW/reflink 优先、可见排除规则、执行前估算 |
| 候选采纳污染主分支 | 采纳预览、显式确认、三方合并、冲突即停 |
| 自动清理丢失结果 | 先持久化后清理 |
| N² 互评成本 | >2 候选推荐统一 Review Agent |
| 额度无公开结构化接口 | 来源分级 + 显示未知/上次已知；官方页面只作为外部链接，不伪装为采集结果 |
| 同一 Agent不同执行面消耗不同额度池 | Quota Snapshot记录 billing_surface/quota_scope；Driver与QuotaProvider建立显式作用域映射并实测 |
| 外部 hooks 配置污染 | 显式开启 + diff + 备份 + 撤销 |
| 硬件丢失或局域网设备冒充 | 每设备独立配对身份与最小scope、桌面确认、可撤销、Wi-Fi加密、USB不自动全授权 |
| 单键复用导致误录音/误切Agent/误审批 | 明确状态优先级、焦点与操作提示、输入去抖、二元审批资格判定、复杂请求转桌面 |
| 断线重连重复发送Prompt或审批 | 命令幂等、expected revision/version、设备不持久化待发队列、重连不自动补发 |
| 硬件小屏泄露项目或账户信息 | Companion Snapshot最小化、目录短名与分支可配置、无绝对路径/邮箱/代码/完整会话、审批摘要脱敏 |
| 固件与桌面协议漂移 | 版本范围握手、能力协商、无共同版本即只读升级提示、协议测试向量 |
| 自研 GUI 范围过大 | 纵向切片；比较 M2/M2.5 |
| GitHub Agent HQ / codex-host 竞争 | 咬死本地优先、跨生态、额度视图；不与其争云端/宿主生态位 |
| ZCode 等缺稳定接口 | Experimental/Waiting 分级，不阻塞主线 |
| 第三方编排与厂商条款的长期解释风险 | 产品决策接受；Hub 零凭据保留应对空间 |
| dogcodes 近似名、域名或包名冲突 | 发布前做商标、域名、npm、GitHub仓库与二进制命令检查；产品用 dogcodes，CLI固定 `dogcodes` |

## 18. 实施期开放问题（不阻塞产品需求基线）

| # | 问题 | 决策时点 |
|---|---|---|
| Q8 | M1 ACP切片默认 Cursor；若真实账号探针无法满足主会话承诺能力，才在第一批内改选 Kimi，并记录偏离理由 | M0 实测后 |
| Q9 | 评审严重度命名（P0–P3 vs Critical/High/Medium/Low） | M2 设计期 |
| Q10 | 文档比较首版是否内置 Markdown 结构化 diff | M2 设计期 |
| Q11 | Cross Review 是否默认盲评 | M2 设计期（建议提供开关、默认不隐藏） |
| Q12 | 首个外部 Hook 对象（建议 Claude Code + Codex 双发） | M1.5 设计期 |
| — | `dogcodes` 品牌资产注册：npm包与 GitHub仓库名固定为 `dogcodes`，官网域名选用 `dog.codes`（实际持有状态以注册结果为准）；CLI固定 `dogcodes` | 立即，注册前人工终检 |

## 19. 命名、包与发布标识

| 对象 | 固定名称 | 说明 |
|---|---|---|
| 产品显示名 | `dogcodes` | 官网、桌面应用、README与发布物统一使用 |
| CLI命令 | `dogcodes` | 统一使用新的产品名称 |
| npm包 | `dogcodes` | 若 npm主包被占用则暂停发布并重新决策，不擅自增加含义不同的后缀 |
| GitHub仓库名 | `dogcodes` | 仓库 owner/organization 另行注册，不在需求文档中假设已获得 |
| 官网域名 | `dog.codes` | 对外使用完整域名；实际持有状态以注册结果为准 |
| Rust workspace crate 命名 | 按职责命名，不含产品名 | 目录如 `crates/core`、`crates/protocol`、`crates/codex-driver`；通用层包名使用 `agent-core`、`agent-protocol` 等，避免与标准库重名；是否发布 crates.io 由实现期决定 |

应用数据目录使用 Tauri/操作系统标准应用数据路径，桌面应用标识为 `codes.dog.desktop`。软件尚未发布，产品更名时直接更新环境变量、协议标识、存储键和应用标识，不保留旧名称兼容或数据迁移逻辑。发布前完成商标、域名、npm、GitHub仓库、crates与 macOS/Windows应用标识检查，并将实际注册结果写入发布清单而不是需求假设。

## 20. 参考项目与工程精读清单

**协议/SDK**：[ACP](https://agentclientprotocol.com/) · [ACP Registry](https://github.com/agentclientprotocol/registry) · [ACP官方 Rust SDK](https://github.com/agentclientprotocol/rust-sdk) · [Codex App Server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md) · [Codex ACP Adapter](https://github.com/agentclientprotocol/codex-acp) · [Claude Agent ACP](https://github.com/agentclientprotocol/claude-agent-acp) · [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) · [Claude statusLine额度字段](https://code.claude.com/docs/en/statusline) · [Cursor ACP](https://cursor.com/docs/cli/acp) · [Antigravity CLI](https://github.com/google-antigravity/antigravity-cli) · [Antigravity SDK](https://github.com/google-antigravity/antigravity-sdk-python) · [Grok Build](https://github.com/xai-org/grok-build) · Kimi Code文档/Server API（M0核实稳定性）· [OpenCode Server](https://dev.opencode.ai/docs/server/) · [DeepSeek Harness SDK Protocol](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/sdk/protocol/README.md) · [Qoder ACP](https://docs.qoder.com/cli/acp) · [Qoder SDK/Credits](https://docs.qoder.com/cli/sdk/cost-usage) · CodeBuddy ACP/HTTP（M0核实）· [Copilot SDK](https://github.com/github/copilot-sdk) · Kiro ACP · Auggie文档 · Cline ACP/SDK。

**优先工程精读**：[Codexia](https://github.com/milisp/codexia)（Rust/Tauri、Codex App Server、ACP、worktree）· [Berd](https://github.com/block/berd)（Tauri + ACP sidecar + 版本锁定）· [T3 Code](https://github.com/pingdotgg/t3code)（最直接产品/UX竞品）· [Zed External Agents](https://github.com/zed-industries/zed/blob/main/docs/src/ai/external-agents.md)（ACP capability UI与外部 Agent边界）· [OpenCovibe](https://github.com/AnyiWang/OpenCovibe)（工具卡片、审批、会话与额度 UI）· [cdesktop](https://github.com/cdesktop-ai/cdesktop)（Codex/Claude风格布局、多 Agent并排与 worktree）· [Verun](https://github.com/SoftwareSavants/verun)（候选、Checkpoint、消息派生与 worktree；AGPL仅参考行为/架构）。

**专项参考**：[Claude Squad](https://github.com/smtg-ai/claude-squad)（worktree并行与回收）· [Vibe Kanban](https://github.com/BloopAI/vibe-kanban)（仅研究 worktree生命周期与 Diff流程；项目已进入 sunset，不作为长期依赖）· [codex-host](https://github.com/BytePioneer-AI/codex-host)（额度采集逐字段审计后参考）· [Crystal](https://github.com/stravu/crystal) · [Paseo](https://github.com/getpaseo/paseo) · [acp-ui](https://github.com/formulahendry/acp-ui) · [CodeIsland](https://github.com/wxtsky/CodeIsland)（MIT）· [open-vibe-island](https://github.com/Octane0411/open-vibe-island)（GPL-3.0只参考行为）· [AgentDesk](https://github.com/yuanqing19920927-ship-it/AgentDesk)（MIT）· [Atoll](https://github.com/sheepbooy/Atoll)（MIT）· [agentbro](https://github.com/shirenchuang/agentbro)（Apache-2.0）。

ACP Registry用于发现、安装元数据和认证握手验证，不代表某 Agent已满足 dogcodes 的审批、提问、Diff、恢复、额度等 Production要求；每个版本仍须通过本项目 capability契约测试。
