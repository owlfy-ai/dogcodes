# Computer Use 第一阶段实现与验收

日期：2026-10-31。平台：Windows x64。目标：让 agent 自行观察软件运行效果、通过 GUI 复现问题、修改代码后回到软件验证，也能完成用户指定的桌面工作。

## 已实现的使用方式

已有任务的输入栏新增电脑按钮。默认关闭，空闲时开启后从下一轮生效，设置按任务保存。运行中可以关闭，中断正在进行的 computer-use 调用。开启后 agent 根据任务自主使用工具，无需逐次授权普通点击、输入或截图。

本阶段采用用户授权的宿主机模式：允许浏览器、前台鼠标键盘和应用启动。没有引入 VM、沙箱或逐应用授权流程。computer use 开关控制本应用提供的能力；它不约束 agent 原有的 shell 能力。

## 实际架构

```text
agent 原生 MCP 客户端
    → dog.codes --computer-mcp（stdio 代理）
    → Core 任务端点（本地认证 RPC）
    → 常驻 Cua Driver 0.38.0（Windows Job 管理子进程树）
    → Windows UIA / 截图 / 鼠标键盘
```

Core 持有驱动连接，代理负责协议转换并透传 MCP 图片、结构化结果。相同连接跨步骤保留 element token。驱动超时或连接丢失后返回结果可能不确定，停止旧进程，下一次重新建立连接；不会自动重放点击。

`SessionRequest.app_mcps` 替代单一 history MCP 字段，使历史查询与 computer use 并存。Claude、Codex、ACP、Antigravity、Zcode 注入路径已迁移；Pi 仍明确不支持 MCP。本阶段只对 Codex 和 DeepSeek Harness 做了真实模型验收，不能据此宣称 12/12 已验证。

20 个工具：`list_apps`、`list_windows`、`launch_app`、`get_window_state`、`get_desktop_state`、`zoom`、`click`、`double_click`、`right_click`、`type_text`、`set_value`、`press_key`、`hotkey`、`scroll`、`drag`、`move_cursor`、`invoke_menu`、`set_window_frame`、`verify_state`、`run_steps`。

保留上游坐标、截图标识、目标和后置验证的 schema，仅裁剪未公开工具和冗长描述。工作流提示要求先观察、操作后验证，失效 token 重新读取，视觉坐标使用新截图；测试键盘事件时使用真实输入，不能仅用 set_value 代替。`run_steps` 限制为 1–32 个公开输入步骤及等待/断言。

桌面控制权在首次工具调用时获取，持有至该轮结束。其他任务仍可正常编程；竞争桌面时收到可解释的 busy 结果。取消走独立于 agent stdio 的通道。关闭、任务取消和手机端手动输入会撤销当前调用并清理驱动进程树。不能回滚已发生的输入。

每个动作记录工具名、耗时、图片数量、传输/工具错误状态；不把完整截图重复写入审计事件。关闭 Cua 遥测。Windows 打包带固定版本 driver 与 UIA helper，下载校验固定 SHA256，启动校验文件清单；附带 MIT 许可。

## 接入中解决的问题

- Codex 0.164.0：仅注入 MCP 不足以避免自带 computer-use 插件竞争。对启用本能力的子进程禁用相关内置插件，动态查询并关闭对应 skill；不修改用户全局配置。MCP 服务配置普通工具审批和 180 秒超时。
- 动态开启：MCP 描述符进入会话绑定。Codex 配置变化时重新挂载原线程；Claude 的会话复用也检查 MCP 配置。
- DeepSeek Harness：已存在活动会话直接 resume 会被拒绝。仅对声明支持 close 的 DeepSeek ACP，会话绑定变化时先关闭旧空闲会话再恢复，其他任务连接不受影响。
- 多模态：真实验收选择安装实例提供的 `["deepseek-account","deepseek-flash"]` 账号路由。用户确认 `deepseek-v4-flash` 可用；未将它当作文本模型。官方 API 路由因本机没有对应 key 没有用于本次验收。ACP 的 promptCapabilities.image=false 并未阻止 MCP 图片或 read_image 链路，实际截图识别成功。

## 真实验收结果

测试经过生产 Core、SQLite、实际 agent driver 和 MCP 代理，先运行未开启的普通轮次，再在同一任务开启，验证会话重配置。没有调用 Claude 付费模型。

| Agent | 桌面表单 | 网页复现、修复、复测 |
|---|---|---|
| Codex | 通过；5 次工具调用、3 个内联图片 | 通过；7 次工具调用，最终数量 3、总价 21 |
| DeepSeek Harness Flash | 通过；11 次工具调用、3 个内联图片 | 通过；27 次工具调用，最终数量 3、总价 21 |

桌面用例：独立 WinForms 窗口，输入项目名和中文，从绘制图片识别随机四位数字，保存报告，读取成功提示并截图。独立检查报告内容与随机码，避免只依赖模型自述。

网页用例：独立 Edge 测试配置及本地页面，单价 7、数量 3 的错误总价为 10。agent 通过界面复现，编辑 app.js 将加法改为乘法，GUI 刷新重测，点击保存得到独立记录 `{quantity:3,total:21}`，保存修复前后截图。DeepSeek 用例还明确执行了截图定位的前台像素点击；其图片主要保存到文件再用 read_image 读取，审计的内联 imageCount=0 不能解释为未使用视觉。

证据保存在仓库忽略目录 `.local/computer-use/acceptance/`：

- `codex-1793434255`、`deepseek-harness-1793434842`：桌面表单。
- `web-codex-1793434935`、`web-deepseek-harness-1793435162`：网页，含 before.png、after.png、report.json、修复后的 app.js。
- 每例的 evidence 下有 summary.json、actions.json、events.json；其中包含模型工具结果，按本地测试材料处理，不提交真实用户桌面内容。
- 桌面占用改为首次调用时获取后，再跑 Codex 桌面用例 `codex-1793435599`：通过，7 次调用；一次批处理工具错误后重新观察并恢复，独立报告与截图检查通过。

这些是端到端功能证据，不是统计可靠性结论。尚未完成重复成功率、大规模 token/延迟评测、多显示器、高 DPI、UWP 和无 UIA 应用的系统兼容矩阵。WinForms 截图在本机存在黑色边缘，基础视觉与语义操作可用，但仍需专项 DPI 测试。

## 重现与工程验证

```powershell
node scripts/prepare-computer-use.mjs
cargo build -p agent-store --example live_computer_use
./scripts/computer-use-acceptance.ps1 -Agent codex
./scripts/computer-use-acceptance.ps1 -Agent deepseek-harness
./scripts/computer-use-web-acceptance.ps1 -Agent codex
./scripts/computer-use-web-acceptance.ps1 -Agent deepseek-harness
```

要求交互式 Windows 桌面、已登录对应 CLI，网页用例需要 Edge。脚本仅关闭自身测试窗口/配置目录对应进程。桌面 fixture 使用可见 PowerShell WinForms 测试进程；产品本身不采用逐动作 PowerShell 脚本。

离线集成测试 `cargo test -p agent-store --test computer_use` 覆盖默认关闭与持久化、双 MCP 注入、图片透传、工具过滤、重复 RPC 不重复点击、取消无后续副作用、手机输入抢占、并行任务首次操作争用。使用假驱动，不操作真实桌面。另完成相关 Rust 单元测试、clippy、desktop 类型检查、前端组件测试及 Vite 构建。NSIS 安装包与其余 agent 的真实运行未列为已验证。

最终回归：相关 9 个 Rust package 的 lib/tests 共 532 项通过、61 项按原定义忽略，0 失败；忽略项主要需要本机环境或真实付费 agent。desktop 与各 driver 的 all-targets clippy（warnings 作为错误）通过；TypeScript 通过；电脑按钮及共享 tooltip 11 项测试通过；Vite 生产构建通过，保留现有大 chunk 提示。Git 清理回归原先的 5 秒等待在本机不足，调整测试等待上限至 30 秒后 15 项全部通过，未改动生产 Git 实现。

Windows 正式打包和开发启动自动准备 sidecar；生成二进制及下载缓存均不提交仓库。

## 前后台状态与桌面提醒

后续补充：Core 发布独立的瞬时状态流，区分 `observing`（读取/截图）、`background`（后台投递）、`foreground`（前台输入）和 `idle`（本次调用结束）。此状态由执行层生成，不依赖 agent 的自然语言描述。审计同时记录所选模式及 foregroundPossible。

桌面宿主在每个启动时已连接的显示器顶部显示贯通边缘的透明光带：细亮线、光晕与横向流光，中央为紧凑文字。后台/观察用绿色，文案为“dog.codes 正在后台操作电脑验证”；前台用橙色，文案为“dog.codes 正在使用键盘鼠标，请暂停键鼠操作”，旁边显示“Esc 退出”。批处理允许前台回退时保守显示“可能使用键盘鼠标”。旧矩形提示条和退出按钮已移除。

视觉修订：亮线由 2 个逻辑像素加粗到 6 个逻辑像素，并扩大、增强光晕；Esc 退出直接使用普通文字，不使用 kbd 标签、边框或按钮外观。更新后的 Windows 渲染预览为 `.local/computer-use/glow-live/glow-wide-background.png` 和 `glow-wide-foreground.png`。

中央品牌文字进一步替换为 18px 的应用图标；状态文案不再重复显示 dog.codes，图标保留品牌替代文本供辅助功能读取。最新预览为 `.local/computer-use/glow-live/glow-icon-background.png` 和 `glow-icon-foreground.png`。

图标继续使用已批准的桌面图形，通过 SVG 亮度蒙版只绘制白色轮廓，使黑色底块完全透明并融入提示区背景；不修改系统应用图标。透明版本的实际渲染预览为 `.local/computer-use/glow-live/glow-transparent-background.png` 和 `glow-transparent-foreground.png`。

MacBook 刘海屏：macOS 从对应 NSScreen.safeAreaInsets.top 读取原生安全区域，把中央文字放到该区域下方并留 8 点间距；光带仍对齐屏幕顶部。按 Cocoa 与 CoreGraphics 的坐标转换匹配多显示器，不能匹配时使用保守安全偏移；窗口高度随文字偏移增加，避免文字被窗口自身裁剪。普通屏幕偏移为零。安全区数值与前端布局有回归测试，尚未在真实 MacBook 刘海屏上验收。

窗口透明、无阴影、不激活且不获取键盘焦点，鼠标事件穿透，不遮挡顶部菜单的点击。它独立于主窗口，主窗口在后台或最小化时也可显示。Windows 额外校正透明窗口的非客户区偏移，使亮线对齐显示器的实际顶部像素。隐式 macOS brief-foreground 环境开关不继承。

桌面构建的前台调用先等待匹配 operationId 的光带在前端完成渲染帧，再额外等待 1 秒后发送输入。10 秒内提示未就绪则拒绝该次前台输入。取消时先结束驱动操作，再发布 idle，光带淡出后隐藏。服务连接丢失时保留状态未知提醒，不假定键鼠已经释放。

Windows 全局 Esc 监听只在前台操作时启用：人工 Esc 关闭该任务的 computer use 并中断正在执行的驱动调用，编程会话本身仍可继续。低级键盘钩子忽略 LLKHF_INJECTED 输入，所以 agent 自己按 Esc 关闭菜单不会触发退出；只消费对应的人工按下、连发与释放，不记录其他按键。钩子线程由宿主拥有，退出时卸载。macOS/Linux 仍待接入并实机验收全局 Esc，当前 computer-use 宿主能力只在 Windows 启用。

前后台通知机制的既有 Windows 实测（旧视觉样式）：真实桌面程序、生产 daemon RPC、Codex 与 WinForms fixture 联合验收中，收到 preparing 后约 1033 ms 才开始前台 batch，batch 约 5351 ms。整个提示准备及执行区间的 101 次前台窗口采样均属于 fixture，未出现通知窗口所属进程，验证未抢焦点。macOS/Linux 的窗口、全屏工作区与显示服务器行为仍需各平台实机验收。

新光带在实际 Windows WebView 中检查：1440×76 逻辑像素，捕获为 2880×152 PNG；背景透明，两种状态均无按钮；中文文案、前台 Esc 提示与后台隐藏 Esc 均通过检查。`.local/computer-use/glow-live/glow-preview-background.png` 和 `glow-preview-foreground.png` 为透明光带合成到中性深灰底的预览。键盘钩子安装/卸载、注入输入过滤、按键连发去重及回调只通知一次的 3 项测试通过。执行中的驱动取消与无后续输入继续由离线副作用计数器回归覆盖。

用户可见演示也已执行：生产桌面宿主中的 Codex 对独立 WinForms 测试窗口先执行后台 batch 约 13.17 秒，再执行前台 batch 约 12.47 秒，两段均成功。原生窗口检查确认客户区起点 (0,0)、逻辑尺寸 1440×76、鼠标穿透和不激活标记均生效。新视觉的实际调用证据位于 `.local/computer-use/glow-live/states.json`、`events.json` 和 `native-layout.json`。

补充回归覆盖：两种 run_steps 语法、批处理前台默认值与回退、桌面目标别名、过期通知拒绝、渲染确认与提醒提前量、通知缺失拒绝输入、取消后 idle、断线未知状态、光带颜色与 Esc 提示、无按钮，以及本地 RPC 的 host-only 分类。CLI 独立验收示例默认不连接桌面提示窗口；要求提示的配置用于实际桌面宿主。
