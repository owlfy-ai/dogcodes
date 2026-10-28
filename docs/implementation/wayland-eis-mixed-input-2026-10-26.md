# EIS 混合快捷键与文字输入

日期：2026-10-26，接续 [EIS 输入后端](wayland-eis-input-2026-10-26.md)。使用场景是手机远程桌面的“键盘”入口向电脑当前聚焦的窗口输入；不改变 Agent 任务消息或原生终端输入。

## 问题与实现

上一轮缺少 ei_text 的后端会在整批执行前拒绝“快捷键后输入文字”，防止 Caps Lock 或布局切换使后续键码失效。本轮直接用旧代码复现三项失败：Ctrl+C 后切 Caps Lock 再输入、释放上一批次 Ctrl 后输入、两次切换布局后分别输入。

- 每个涉及的键盘按需建立独立规划状态，按原按下顺序重建键计数，并保留当前锁定/布局及外部修饰键基准。keymap 可共享，XKB 可变 state 不共享；不以 xkb_state_ref 冒充复制，不在同一个 state 上混用 slave update_mask 和 master update_key。鼠标批次不创建键盘规划副本。
- 预演整批物理按下/释放，后面的文字使用当时的规划状态。因此同一批里可以松开旧 Ctrl、切 Caps Lock、按 keymap 切布局后继续输入；布局组按实际组数回绕。多字符后缀不支持时，整个准备过程失败，实际 Caps Lock、按键归属及电脑均未被改变。
- 快捷键和后续文字之间增加 EI sync 边界。符合 keymap 的反馈归入自己的预测；已报告的外部/未预测布局变化使旧代次失效，后续文字停止，已执行前缀不得整体重放。原有每次 OS 效果前的授权、代次和设备检查继续保留。
- 测试端按接收键码用真实 XKB state 解出字符，而非只比较生成的键码；覆盖 Ctrl+C、大小写、双 Shift 持有、跨批次释放、两次 US/DE 布局切换、无副作用的无效后缀，以及真实 EI 服务把布局反馈延迟到 sync 前才发送的情况。后者同时覆盖有/无 ei_text。

[EI keyboard](https://libinput.pages.freedesktop.org/libei/interfaces/ei_keyboard/index.html)保证直接反馈在 sync 回调前送达，但间接触发的布局变化仍可能在回调后到达；后续事件继续按代次处理，不能把协议同步当作对操作系统未来焦点/布局的锁定。[XKB state](https://xkbcommon.org/doc/current/group__state.html)区分 server/client 状态，本轮规划保持该边界。

## 验证

没有使用真实桌面、键鼠、模型或剪贴板，也未切换日常 Core。

- 旧代码三项新增混合输入回归均失败，日志 `/tmp/dogcodes-eis-mixed-before.log`。
- 新代码 EIS 共 19 项回归无跳过通过，其中本轮新增 6 项；Linux 原生完整 126 项通过、7 项 opt-in 默认忽略。严格 all-targets Clippy 和原有独立 D-Bus/合成 PipeWire → Portal → EIS 全链路通过。
- 独立源码副本移除规划按键状态更新，实际输出大小写错误；移除文字前的 sync 边界，延迟布局反馈后仍输入了错误布局的后缀；两项对应测试均退出 101（断言失败）。还原副本后 19 项通过。每次清理独立 proof target 的原生包、确认没有旧测试产物再编译。

证据：`/tmp/dogcodes-eis-mixed-{after,validation,final}.log`；反例路径/摘要 `/tmp/dogcodes-eis-mixed-witnesses.json`，结果 `/tmp/dogcodes-eis-mixed-witness-results.json`，分项 `/tmp/dogcodes-eis-mixed-witness-*.log`。本轮没有改跨平台公开契约或手机代码，未重做上一轮 macOS/Windows/desktop 验证，不将之前的结果当作新的实机证据。

## 后续已获授权

用户最新决定是：旧 Wayland 必要时用已授权剪贴板粘贴，**不尝试自动恢复**，覆盖此前“尽力恢复”的讨论。回退现已接入，且不读取/备份原剪贴板，见 [当前实现、证据与限制](wayland-clipboard-paste-2026-10-26.md)。不是自动同步剪贴板，也不抢回后来由用户/其它应用替换的内容。

Wayland 真实 compositor、原生 Unicode 回退、其余视频/PC 功能/操作恢复，以及八个未整体验收工作包仍保持原范围。

## 2026-10-27：非拉丁键符的等价字符

复现当前 XKB 的俄文/希腊文布局已包含所需字符，但输入被判为 Unmappable。原因是布局可用命名键符（如 Cyrillic_ef），literal Text 使用 Unicode 键符；两者数值不同，不能直接当作不同字符。[XKB 键符定义与 Unicode 转换](https://xkbcommon.org/doc/current/group__keysyms.html)明确了这两类编码，均属于当前规范，与旧版应用数据兼容无关。

只修改原布局匹配：原精确匹配保留；请求为有效的直接 Unicode 键符时，单一布局符号再按其 Unicode 字符比较。文字规划和单字符按键均复用此判断，原组别、Shift/AltGr、Caps Lock、整批预演、代次和同步检查保持。Return/Tab/修饰键等命名控制键仍按原符号严格匹配；不把多符号键当作单字符，不假定 US 布局或擅自切换组别。

三项新回归在旧代码均失败：俄文/希腊文的大小写及 Caps Lock；没有 ei_text/剪贴板能力的真实 EI wire 输入；同批 US→RU→US 切换并输入。修复后，用接收端真实 XKB state 解出的文字验证全部通过，不只核对发送键码。当前布局没有拉丁 V 时仍拒绝猜测快捷键，终端/自定义粘贴和实际 compositor 验收保留。

原生文字与剪贴板通道均不可用、当前布局也无法映射时，明确提示本次未执行，不再要求用户授予可能不存在的剪贴板权限。此处不新增输入方式或重发策略；已有 ei_text 和已授权剪贴板回退继续按原优先级工作。

最终 Linux 原生 158 项通过、10 项按需用例默认未启用，私有 Portal/实际 PipeWire/EIS 专项通过；原生与 desktop all-targets 严格检查、格式及差异检查通过。修改仅在 Linux Wayland，未重复无改动的 macOS/Windows、手机、Core/存储和服务端套件。未访问用户桌面/键鼠/剪贴板、真实 Agent、设备或日常 Core，原三个开发服务容器保持，未提交推送。证据 `/tmp/dogcodes-nonlatin-before.log`（三项失败）、`/tmp/dogcodes-nonlatin-after.log` 和 `/tmp/dogcodes-nonlatin-final.log`；真实应用的 IME、焦点/布局变化及性能仍需实机，不能据解码夹具宣称全语言/全桌面输入已验收。
