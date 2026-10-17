# 灵动岛：参考调研与首版实现

> 历史记录：2026-10-16 已移除悬浮岛窗口及专属实现。共享 Core/后台服务接口、Companion 状态投影与会话重命名保留；当前范围见[移除记录](island-removal-2026-10-16.md)。下文仅记录当时的实现和验收。


更新：2026-09-11。对应需求 FR-6、详细设计 §18 / M1.5。

## 三个参考项目

| 项目 | 已公开的主要功能 | 本项目借鉴方向 |
| --- | --- | --- |
| [Open Island](https://github.com/Octane0411/open-vibe-island) | macOS 刘海/顶部状态、会话聚合、审批、精确跳回终端/IDE、额度视图、通知声音；不同 Agent 的交互覆盖不同，README 明确 Grok 目前没有审批回传 | 清楚标识任务来源、跨任务聚合、快速定位；仅参考行为，不引入其 GPL 源码 |
| [Ping Island](https://github.com/erha19/ping-island) | 需要注意时突出请求、审批与问答、SSH 远程会话、主题与声音、可分离的动画伙伴；Antigravity 插件保留原生权限提示 | 安静的折叠态、待处理事项优先、未来按当前会话抑制重复提醒 |
| [CodeIsland](https://github.com/wxtsky/CodeIsland) | 实时状态、审批/问答、像素角色、具体终端标签/窗格级提醒抑制、多显示器、iPhone/Apple Watch 伴侣 | 多屏/DPI 定位、运行与待交互分离、未来复用 CompanionProjection 接硬件 |

来源是各项目当前 README，不把功能列表视为所有 Agent 均支持的兼容承诺。未安装上述软件、未改写其 Hooks，也未复制它们的代码或美术资源。

## 当前实现

- 主窗口右上角新增“打开灵动岛”。独立透明、无边框、置顶窗口，默认不抢焦点；关闭浮岛可从主窗口再次打开，开启偏好跨重启保存。
- 折叠胶囊优先显示待审批，其次输入/错误/连接/运行状态；展开查看各任务、项目、实际运行 Agent、时长。最近完成/失败显示 60 秒，空闲停止动画与计时。
- 展开后按需获取完整脱敏审批 DTO，显示原生选项；不靠选项文字推断授权语义。缺少摘要、脱敏导致信息不足、未知选项等转主窗口处理。
- 审批复用 Core 的版本校验、命令幂等和 Driver 回传；单次点击期间禁用重复操作。另一窗口处理或请求过期后刷新移除按钮。没有新增自动批准策略。
- “打开对话”定位主窗口对应 Task；失焦或 Esc 收起。
- 只消费轻量 CompanionProjection，不向浮岛发送完整 Bootstrap、Timeline、Diff 或原始协议帧。待审批数复用同一持久化事件归约函数；最近终态也从 Canonical Event 派生。
- Core 独立广播刷新提示；主窗口最小化不影响更新。逐 token 的 message.delta 不触发浮岛刷新；其他事件合并通知，界面按 revision 防止旧响应覆盖。
- 位于主屏工作区顶部，避开菜单栏/物理刘海；使用显示器物理坐标及缩放系数，支持负坐标和空间不足时限制尺寸。界面缩放与主窗口偏好一致。

## 当前边界

本次是顶部浮岛的首个可用版本，未宣称 M1.5 全项完成：

- 尚未实现 AppKit safe-area 驱动的刘海左右融合、显示器选择、Space/全屏/热插拔的完整适配；当前为工作区顶部居中回退布局。Windows 仅共用 Tauri 定位代码，未做真机验收。
- 运行阶段使用现有投影状态，不猜测未投影的思考/工具子阶段；Delegation、Review、候选分列待相应投影完整接入。
- 未接外部终端会话、声音/宠物、SSH、手机/手表；未新增额度轮询。现有额度页和 Agent 权限策略保持独立职责。
- 原生窗口测试使用隔离临时数据库与现有验收构建，不启动真实 Agent。各 Agent 原生审批正确性沿用已完成的真实验收，本次重点验证新呈现入口与同一 Core 的连接。

## 验证入口

- `apps/desktop/src/Island.test.tsx`：单次提交、原生 optionId、跨窗口处理后移除、失焦收起、信息不足回退。
- Store 的 `approvals_persist_redacted_and_reach_the_driver_once`：投影待审批数与同一请求归约一致。
- Desktop `island::tests`：DPI、负坐标与小屏工作区。
- `island-preview.html`：仅开发视觉样例，模拟 IPC，不调用 Core/Agent。
- `review-acceptance` 构建加 `DOGCODES_ISLAND_ACCEPTANCE=1`：隔离原生窗口与 IPC 冒烟测试，报告 `ISLAND_ACCEPTANCE` 后退出。

## 本次验收结果

- 新增前端交互/状态测试 5 项通过；之前的全量前端运行中其余 169 项通过，新增 blur 测试修正事件名称后定向通过。
- Store 36 项测试通过；包含审批数归约、请求幂等及终态清理。
- 原生位置单元测试通过，桌面 Cargo 检查与前端生产构建通过。
- 隔离 macOS WebView 冒烟报告：`ok=true`、`visible=true`、`topmost=true`、`decorated=false`；投影 IPC、展开调整尺寸、定位 Task 的 IPC 均成功。
- 浏览器视觉样例：控制台无错误；70%/100%/150% 缩放无容器横向/纵向溢出，浮岛填满对应窗口。长列表与长命令使用内部滚动。

证据位于 `experiments/island/2026-09-11/`，包含日志与截图。浏览器样例中的任务和审批是测试数据，不是一次真实 Agent 调用。


## 刘海融合增量

按用户要求改为屏幕顶边连续纯黑外形，顶部直边、底部圆角。macOS 从 AppKit NSScreen.safeAreaInsets 与 auxiliaryTopLeftArea / auxiliaryTopRightArea 读取物理刘海对应的逻辑尺寸，不按机型猜测；摄像头区域预留空白，任务与审批内容位于其下方。窗口提升至菜单栏层级，定位使用完整屏幕而非菜单栏下方工作区。Windows / 无刘海屏使用 180 × 32 逻辑点模拟刘海区域，共用同一黑色外形。文字缩放不改变原生刘海预留高度。

此增量替代上文“工作区顶部居中回退”的实现描述；仍固定主显示器，完整多屏热插拔、Space / 全屏覆盖及 Windows 真机效果尚待专项验收。

依据：[Apple safeAreaInsets](https://developer.apple.com/documentation/appkit/nsscreen/safeareainsets)、[auxiliaryTopLeftArea](https://developer.apple.com/documentation/AppKit/NSScreen/auxiliaryTopLeftArea-uglc)。

本次验证：Rust check、几何测试、TypeScript 检查及 5 项前端测试通过；70% / 100% / 150% 视觉样例无溢出。隔离 macOS 原生窗口报告 nativeNotch=true、160×28 逻辑点、atDisplayTop=true、y=0，展开与定位任务 IPC 成功。窗口使用菜单栏级别 25，Tauri 的 is_always_on_top（比较其标准浮窗级别）因此报告 false，不代表该窗口处于普通窗口层级。证据：`experiments/island/2026-09-11/notch/`。

## 刘海两侧展开（取代下置状态行）

按最新交互要求：待机窗口严格等于原生刘海宽高，不在下方显示状态；存在任务、审批、短暂完成提示或同步异常时，左右各扩展 150 逻辑点（随 UI 缩放），高度保持原生刘海高度。中间区域在原生刘海屏始终留空；Windows/无刘海屏显示 Logo 与 dogcodes。左右分别呈现状态和 Agent / 数量，点击才向下打开详情。摄像头宽高独立于 UI 缩放；任务结束提示过期后自动恢复待机宽度。此段替代前一增量的“内容位于其下方”描述。

## 紧凑视觉与主动交互

重设计上下两种形态：收起时使用状态灯和去重 Agent 图标组（最多 4 个及溢出数量），保留中间摄像头区域；展开面板使用低对比背景、紧凑任务摘要、优先审批卡片和中文批准/拒绝文案。只解析 JSON 字符串的外层编码以避免命令显示转义引号，不删减实际命令。展开高度由 480 减至 360 逻辑点，长内容内部滚动。

新的待审批或 waiting_for_input 状态自动展开并选中对应任务；处理完后收起，同一交互手动收起后不因一般状态刷新再次弹开。等待回答时提供返回对话入口。鼠标进入调用受窗口身份限制的 island_focus 设置原生焦点；不会自动替用户审批。新增行为测试共 7 项通过；原生几何测试通过。浏览器视觉与构建证据保存在本次 redesign 目录，尚未对新移入行为做原生鼠标真机复验。

## 显示器变化后的定位恢复（2026-09-28）

原实现只监听 `ScaleFactorChanged`，同 DPI 的分辨率变化、主屏切换和热插拔可能留下旧坐标。现在每 500 毫秒在主线程读取主屏和实际窗口尺寸、位置，仅在不匹配时重新定位到完整屏幕顶边中央；重复核对也覆盖系统在显示器重配置期间再次移动窗口的情况。保留展开、任务活跃及文字缩放状态，隐藏窗口不主动显示，再次打开时立即定位。

macOS 跨不同 DPI 屏幕使用全局逻辑坐标，避免将目标屏幕的物理像素按窗口原屏幕的缩放解释。刘海尺寸变化通过 `island-geometry-changed` 同步前端，切换无刘海屏时同步显示模拟刘海内容。仍固定跟随主显示器。

验证：前端 413 项测试、灵动岛 Rust 14 项测试、类型检查及生产构建通过。隔离 macOS 原生验收将窗口移至错误坐标，在没有显式重新定位请求的情况下自动恢复，报告 `atDisplayTop=true`、`centered=true`。实际外接屏插拔、硬件分辨率切换及 Windows 真机尚未复验。
