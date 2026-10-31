# Computer Use 后续实施与验收

日期：2026-10-31。本文补充第一阶段报告。当前范围按用户要求：Pi 不支持也可；macOS/Linux 完成接入代码，实机验收由用户后续执行；Claude 已恢复额度，纳入 Windows 实测。

## 已实现的后续工作

- 动作进入现有会话时间线，重启后仍可查看，并通过事件流实时更新。显示工具、观察/后台/前台模式、耗时、截图数量、调用结果；传输未确认和效果未验证分别展示，不把它们标记为验证成功。记录绑定实际 turn/provider，不复制截图或完整输入到动作审计。
- 手机手动输入会中断正在执行的驱动调用、清除旧回执；手动控制期间拒绝新输入。释放手动控制后，同一任务下一次明确调用可恢复，先重新观察，不自动重放中断动作。
- 固定 Cua Driver 0.38.0 的 Windows x64/ARM64、macOS universal、Linux x64/ARM64 发布资产与 SHA256。按 Rust target 独立准备资源，避免交叉构建误打包宿主架构。Windows 包含 driver、UIA helper、cursor helper；Unix 包含 driver、cursor helper。包含 MIT 许可，不携带 Node SDK/perception 服务。
- 正式打包和开发入口自动准备资源；已在使用的相同版本 Windows exe 不再重复覆盖。宿主按平台资源目录查找，验证版本、平台、架构及文件摘要。
- macOS 使用上游 embedded/direct 集成和宿主 bundle ID；增加辅助功能、屏幕录制权限检查及本地化引导。Cua 当前二进制要求 macOS 13，低版本明确禁用此能力，应用其他功能仍保留原最低系统版本。
- Windows 保留物理 Esc 钩子；增加 macOS 原生事件 tap、Linux XInput2 物理键监听，过滤已识别的合成输入。全局 Esc 设施不可用时拒绝前台输入并提示采用后台方式。原绿/橙光带、透明图标和 Mac 刘海安全偏移保留。
- 所有新增界面文字覆盖现有 11 个具体语言包。

## Agent 接入中的实际修复

真实验收均先进行未启用 computer use 的 READY 轮，再在同一任务开启并操作窗口，覆盖动态注入，而非仅验证新任务。

- Claude/Codex：保留已有 MCP 绑定重配置；Claude 现已执行真实模型测试。
- Kimi/DeepSeek ACP：MCP 或工作区绑定变化时，仅在 peer 明确声明支持 `session/close` 时关闭该空闲会话再恢复，保留共享连接其他任务。
- Grok：识别原生 `server__tool` 形式。
- CodeBuddy：兼容原生 `call_<hex>`、`chatcmpl-tool-<hex>` 标识；当审批只带参数时，仅从同一 session、同一未完成调用且参数完全相等的记录中恢复 MCP 工具名。
- Kiro：识别 `@server/tool` 与 `server::tool` 的对应关系。
- ZCode 0.17：活动会话绑定变化时关闭该逻辑会话，再通过原 worker 的 `session/resume` 重载配置；不退出其他会话。任务级 `toolDenylist` 排除竞争的 `mcp__node_repl__js` 入口，关闭本能力时传空列表恢复。既有冷恢复版本验证规则独立保留。
- Antigravity：验收程序接入原审批 Hook 入口；启动检查遇到供应商账号拒绝时显示账号错误，不再笼统归为 Hook 损坏。本机供应商明确拒绝当前账号所在地区，真实模型测试因此受阻。

自动批准仍要求匹配本任务确切 MCP server 和公开工具；未为任意 shell/其他 MCP 增加自动批准。

`run_steps` 的公开 schema 与实际限制统一为 1–32 步，说明保留两种语法及 `observe` 用法；不支持的步骤返回可纠正的格式提示。

## 可复现的 Windows 验收

```powershell
cargo build -p agent-store --example live_computer_use --locked
./target/debug/examples/live_computer_use.exe --discover
./scripts/computer-use-matrix.ps1 -Agents codex,deepseek-harness,claude-code -Repeat 3
./scripts/computer-use-acceptance.ps1 -Agent kiro
./scripts/computer-use-web-acceptance.ps1 -Agent claude-code
./scripts/computer-use-calculator-acceptance.ps1 -Agent codex
```

要求交互式 Windows 桌面、对应原生 agent 已登录；网页测试需要 Edge。测试只使用独立 fixture/浏览器配置；计算器只操作新建的独立窗口，无法新建时拒绝操作已有窗口。GUI 测试串行运行，可能短暂使用前台键鼠。

桌面用例用绘制图片中的随机数字验证图片理解，输入项目名和精确中文并点击保存，由独立报告文件校验内容；不能只凭模型声称成功。网页用例要求通过窗口复现错误、修改本地代码、再通过 GUI 验证数量 3、总价 21，并保存前后截图。计算器用例验证 UWP/MSIX 应用、真实截图与坐标点击，并由独立 UI Automation 观察结果。

矩阵输出每次结果、耗时、动作、原生版本、原生用量和请求记录。用量缺失或归属不完整时保留 unknown/partial，不能据此填零或声称完整成本。主要 agent 的重复验收阈值为 3/3，其余为接入冒烟；少量成功不构成任意软件的可靠性保证。首次失败与修复后重测分目录保留。

本地原始证据保存在 `.local/computer-use/acceptance/`，不提交模型会话、原始截图或个人环境路径到仓库。

## 本轮 Windows 实际结果

| Agent / 原生版本 | 桌面结果 | 单轮耗时 / 实际 driver 调用数 |
|---|---|---|
| Claude Code 2.1.308 / claude-opus-5-5 | 重复 3/3 通过 | 112、95、50 秒 / 17、11、6 次 |
| Codex 0.164.0-alpha.11.1 / gpt-6.1-sol | 重复 3/3 通过 | 53、66、52 秒 / 5、7、5 次 |
| DeepSeek Harness 0.2.1-rc.5 / 账号 Flash 路由 | 重复 3/3 通过 | 108、54、161 秒 / 31、16、30 次 |
| Cursor 2026.10.22-27dcf5f | 1/1 冒烟通过 | 80 秒 / 5 次 |
| OpenCode 2.2.5 | 1/1 冒烟通过 | 160 秒 / 20 次 |
| Kimi 2.4.0 | 修复后 1/1 通过 | 64 秒 / 8 次 |
| Grok 1.0.70 | 修复后 1/1 通过 | 136 秒 / 9 次 |
| Kiro 2.36.0 | 修复后 1/1 通过 | 55 秒 / 4 次 |
| ZCode 0.17.0 | 修复后 1/1 通过 | 255 秒 / 15 次 |
| CodeBuddy 2.178.1 | 最终桌面 1/1、网页 1/1 通过；诊断阶段结果见下文 | 桌面 123 秒 / 11 次；网页 357 秒 / 27 次 |
| Antigravity 1.4.11 | 按用户要求暂缓后续测试 | 既有地区限制记录保留 |

耗时包括同一任务的 READY 轮与实际 GUI 轮，不含脚本发现/fixture 启动；调用数只统计进入 driver 的动作，不能当作全部模型请求数。CLI 版本按本机实际探测记录，不能只凭平台工具接线推算通过。Claude/Codex 用量为 partial，DeepSeek 为 unknown，未报告完整 token 成本。循环内出现过步骤错误，模型观察后恢复，最终保存结果由独立文件确认；这里的 3/3 是端到端结果，不能解释为每一步都成功。

Claude 网页闭环通过：11 次 driver 调用、85 秒，界面复现错误总价，修改 `app.js` 为乘法，刷新后界面复测并保存数量 3、总价 21，`before.png`/`after.png` 均存在。证据目录 `web-claude-final`。原先 Codex/DeepSeek 的网页闭环证据保留在第一阶段报告。

CodeBuddy 早期实测原生 CLI 对两个审批请求均收到合法 `selected/allow` 回应后，仍返回 `PERMISSION_DENIED`。绕过本项目 Core 与 Cua，用独立 ACP 客户端和仅返回固定文本的最小 MCP 服务，在新会话复现相同失败；未执行最小服务的工具回调。该阻塞现已在接入层处理，详见下节。本地原始复现证据在 `.local/computer-use/codebuddy-wire`、`.local/computer-use/codebuddy-isolation`；失败试验保留，没有与修复后成功样本混算。原生 `codebuddy.ai/errorMessage` 透传到失败信息，保留供应商具体原因。

Antigravity 在不调用模型的原生 `/hooks` 检查中明确返回 `Your current account is not eligible ... not currently available in your location`，不是额度不足或 Cua 操作失败；当前账号环境无法完成验收。

## CodeBuddy 2.178.1 接入修复

用户要求暂缓 Antigravity，继续解决 CodeBuddy。本轮没有继续对 Antigravity 进行模型/GUI 验收，也没有扩展 Pi 或 macOS/Linux 实机测试。

1. CodeBuddy 的 `DeferExecuteTool` 在嵌套 MCP 审批中可能把已允许的调用结束为拒绝。dog.codes 启动它的 ACP 进程时传入 CLI 附加设置 `--settings {"deferToolLoading":false}`，直接加载工具，绕过这层包装。没有修改安装目录、用户配置文件或权限模式；代价是工具定义提前进入上下文。
2. 直接 MCP 审批帧可能不带 title，仅在 `_meta["codebuddy.ai/toolName"]` 中提供真实工具名。接入层现在使用该原生身份，保留对本任务确切 server/tool 的匹配；不从任意工具参数猜测工具身份。
3. 原生日志证实 `session/load` 返回时新 server 已显示 connected，但模型使用的 connected-tools 缓存仍可能只有旧 server，约半秒后才提交更新。其 first-run prewait 在进程内复用，不能保障后续动态新增 MCP。仅在同一连接的 MCP 绑定确实变化后加 2 秒兼容稳定期，普通轮次和其他 agent 不增加等待。这个稳定期是经验兼容措施，不是 ACP 提供的就绪证明；原生 CLI 升级后需复验。
4. CodeBuddy 的模型消息没有完整保留 Cua `structuredContent` 中的 capture_id、截图尺寸和窗口边界。代理将这些小字段同步为普通文本，放在图片之前；PNG 另从固定长度头部提供实际像素尺寸。原始图片、结构化结果和 capture 标识均保留，不重新编码像素。这样模型能直接获得视觉坐标依据，减少额外 shell 读 PNG 尺寸的需求。

最终桌面确认通过：`codebuddy-final-confirmation`，原生版本 2.178.1，完整轮次完成，耗时 122520 ms、11 次 driver 调用。独立 `result.json` 确认项目名、随机图片码和精确中文输入一致，并保存了结果截图。调用过程中出现过 zoom/run_steps 参数错误，模型纠正后完成任务；不把端到端通过解释为每步都成功。最终配置只执行了这次 1/1 确认，没有声称达到重复 3/3 阈值。模型与 token 用量无法可靠归属，保留 unknown。

此前 `codebuddy-fixed` 三轮为 2/3；第三轮因额外通过 Python 读取 PNG 尺寸被验收器拒绝而失败。后续截图元数据兼容与验收器修正前的失败样本也保留，不与最终成功样本合并计算通过率。

真实网页闭环通过：27 次 driver 调用、357 秒，独立 `report.json` 为数量 3、总价 21，保存了前后 PNG。证据在 `web-codebuddy-direct-tools`。桌面验收继续使用随机图片码、精确中文和独立报告，先 READY 再动态开启的方式没有跳过。

诊断阶段曾出现“GUI 已保存成功，但模型额外执行 `ls -l` 检查截图目录，被原纯 GUI 验收器拒绝而终止”的情况。这与 MCP 自身审批异常不同。验收器现在只允许精确列出本次 fixture 输出目录的只读 `ls -l/-la/-lah`，以及其固定 `| head -20` 截取输出形式；读取源文件、写报告、其他路径及其余复合 shell 命令仍不批准。生产 Core 的审批策略没有因此放宽。相关反例和原失败记录保留，不能把保存了文件但模型轮次失败的样本算为通过。

离线回归：ACP 76 项通过、21 项依原定义忽略；Core computer-use 8 项、完整 store 代理集成及验收器目录检查回归通过，相关 all-targets 严格 Clippy 通过。新回归检查元数据文本兼容、原图片字节保留、真实工具名与参数区分、MCP 绑定变化判定、以及拒绝目录外/复合命令。

计算器用例未完成：保留了已有用户窗口，启动没有得到可单独控制的新窗口，因此没有向已有计算器发送输入。该用例提供了脚本，但不能列为 UWP/MSIX 已通过；高 DPI/多显示器组合也没有进行系统性枚举验收。

离线回归：前端完整 148 文件 / 1064 项通过、TypeScript 通过；新增组件/事件流/语言覆盖专项 17 项通过。发布/开发/资源脚本 38 项通过、1 项 macOS 原生签名专项按平台跳过。Core computer-use 与历史审批回归、store 完整代理集成、ACP、ZCode、Antigravity 审批、Windows Esc 相关回归及 desktop/store/相关 driver 的 all-targets 严格 Clippy 通过。没有执行 macOS/Linux 测试。

Windows x64 未分发签名的 debug NSIS 包已实际编译生成；安装脚本确实包含 driver、UIA、cursor、manifest 和 MIT 许可，暂存资源摘要校验通过。包位于 `.local/computer-use/package-target/debug/bundle/nsis/dog.codes_0.1.0_x64-setup.exe`。为保留正在运行的开发客户端，采用独立 target 目录构建。没有安装覆盖现有客户端，也没有把编译成功写成已完成干净安装/正式签名验收。

## 平台使用与后续实机事项

| 平台 | 接入与打包 | 实机状态 |
|---|---|---|
| Windows | 固定 driver/UIA/cursor，物理 Esc，绿/橙提示 | 本轮执行原生 agent/真实 GUI 验收 |
| macOS 13+ | universal driver/cursor、embedded 宿主身份、权限引导、原生 Esc、刘海安全偏移 | 按要求未运行 macOS 测试，待用户实机确认 |
| Linux X11 | 对应架构 driver/cursor、XInput2 Esc | 按要求未运行 Linux 测试，待用户实机确认 |
| Linux Wayland | 对应架构资源及上游 GNOME helper 随包提供 | 本实现不通过 XWayland 假定可监听全局 Esc；缺少可靠 Esc 时只开放后台投递 |

后台可用性取决于应用/控件/显示服务器，不应把“启用 computer use”解释为所有操作都无需前台。前台回退仍先展示橙色提示，再投递输入。

macOS 正式签名必须为 Cua helper 提供 `APPLE_SIGNING_IDENTITY` 或本地 signing config 的完整证书身份；helper 在复制入包前签名，之后计算 manifest 摘要。开发/未分发签名使用 ad hoc。签名、notarization、TCC、刘海/多屏和全屏空间行为留给真实 Mac 验收。

Linux GNOME Wayland helper 未自动安装到用户桌面，其他 compositor 不推定兼容。用户后续需按实际会话检查截图、AT-SPI、后台投递和 Esc；需要前台控制且无全局 Esc 的 Wayland 环境目前需使用 X11。没有用宿主机模拟结果代替这些平台测试。

上游固定版本资料：[Cua 0.38.0 embedding](https://github.com/trycua/cua/blob/cua-driver-rs-v0.38.0/libs/cua-driver/rust/Skills/cua-driver/EMBEDDING.md)、[Cua Driver](https://github.com/trycua/cua/tree/main/libs/cua-driver)。
