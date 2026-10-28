# Wayland EIS 输入与可信动态区域

日期：2026-10-26。接续工作包 3；本轮完成原授权 EIS 后端与隔离验证，未关闭整个工作包，也未缩减视频、PC 功能或操作恢复目标。

后续更新：[混合快捷键与文字已接通](wayland-eis-mixed-input-2026-10-26.md)，含布局/锁定状态预演与同步边界，最新 Linux 原生 126 项及 EIS 19 项通过。下文保留首轮证据；其中“先快捷键再输入文字”的拒绝限制已由后续实现解除。任意 Unicode 的旧后端回退与实机仍未收口。

再后续：[不读取/不自动恢复的剪贴板回退](wayland-clipboard-paste-2026-10-26.md)已接入，最新 Linux 原生 130 项及完整 Portal→EI→Clipboard FD 路径通过。终端/布局/旧能力与真实 compositor 范围继续，首轮“无 ei_text 全部拒绝”的边界不再代表当前全部实现。

## 实现依据与范围

参考本机 RustDesk（`53996e3c4`）`libs/scrap/src/wayland/remote_desktop_portal.rs::connect_to_eis` 及 `pipewire.rs::try_fix_logical_size` 的授权 FD 和物理/逻辑区域分离思路，不复制实现。协议以官方资料为准：

- [RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)：Start 后取得原会话 ConnectToEIS FD；建立后所有输入只能走 EIS，断线应结束该会话。
- [ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)：`mapping_id` 将视频资源与 EI region 关联；PipeWire node、名字、相同分辨率不能代替它。
- [EI device](https://libinput.pages.freedesktop.org/libei/interfaces/ei_device/index.html)、[keyboard](https://libinput.pages.freedesktop.org/libei/interfaces/ei_keyboard/index.html)、[text](https://libinput.pages.freedesktop.org/libei/interfaces/ei_text/index.html)：设备配置在生命周期内固定；paused 会清空按下状态；物理键码、独立输入帧、CLOCK_MONOTONIC 微秒、键盘反馈及文字长度都有独立约束。

Linux 新增直接依赖 reis 0.7.1（支持 ei_text）和已由 Enigo 间接使用的 xkbcommon 0.9。保留 Enigo 原有 reis 0.5 依赖，EIS 会话不调用其环境 socket 或二次授权逻辑。未改网络协议、手机或服务端。

## 已落地

- RemoteDesktop v2+ 且每个已授权 stream 有独立有效 mapping_id 时，在原 Portal session 上选择 EIS。缺失映射的旧 Portal 在连接 EIS **之前**保留 Notify；收到 EIS FD 后，握手失败/EOF/协议错误均关闭原授权，不回退 Notify、不暗中重建会话。仅观看模式不接入 EIS。
- 同一个 Portal actor 处理 Session.Closed、EIS 事件及输入命令。等待设备实际 resumed/ready 后才发布就绪；握手和设备就绪各十秒，已有命令三秒及所属连接取消继续生效。每个 native 输入动作之间让出调度，撤销能中断长批次。
- 只用选中 stream 的 mapping_id 找 virtual absolute region，校验边界、唯一性及数值，按归一化坐标映射到其逻辑区域。设备替换后使用新 region；不将捕获帧的物理尺寸写成输入逻辑尺寸。EIS 可信区域可继续控制；旧 Notify 在像素几何改变后仍拒绝不确定的指针操作。
- 输入在入队时捕获代次；设备增删、暂停/恢复或外部键盘状态变化使旧批次失效。完整批次先准备，再在每次 OS 效果前排空已到达 EIS 事件、复核原代次/授权/设备。执行途中失败结束授权并禁止重放。
- 记录真实按下的键码及其原设备；重复 press 不发送第二次按下；已按下时拒绝 click，避免把已有拖拽/修饰键意外释放；release 使用原设备。paused/removed 清理本地按下状态，不向已停用设备补发释放；resume 不重放旧按下。正常显式 release 通过 EIS 发送。
- 快捷键优先用服务端 keymap 映射 evdev 键码，布局组参与解析；文字优先 ei_text UTF-8，按最多 254 字节和字符边界分块，每块独立 frame。鼠标、文字、快捷键不会混用 Notify。滚轮单位转换为 EI 的 120 分之一格。
- 缺少 ei_text 时，仅用实际 keymap 映射可用字符，处理 Shift/AltGr/Caps Lock；不可映射的后缀会使整批在执行前失败。键盘 FD 限制大小并用 read_at 读取普通文件，拒绝短文件、socket/pipe 和错误编码，不 mmap 越界。
- 修饰键反馈按实际物理按键预测，并通过 EI sync 等待直接反馈；自己的 Ctrl/Shift 状态不会误废弃健康授权。未预测的外部变化仍使旧批次失效。输入帧使用单调时钟；同一按键 press/release、UTF-8 块各有独立 frame。

## 验证与证据

没有访问用户桌面、系统 Portal、键鼠或剪贴板。EI 测试使用自建 Unix socket pair 和真实 reis 双端协议；Portal 验收运行在容器内独立 D-Bus、模拟 login1 及合成 PipeWire 来源中。

- Linux 原生完整单元测试：120 通过、7 个 opt-in 默认忽略；其中新增 13 项 EIS/布局回归无跳过通过。覆盖中文/emoji、多字节分块、布局/组合键、区域身份、暂停/恢复/替换、EOF/取消、外部修饰键、短 FD，以及无前缀副作用的整批拒绝。
- 原有私有 Portal 验收扩展后通过：原授权 FD → EI 握手/设备 ready → 移动/中文/Ctrl → release；断言 Notify 调用数不增加。EOF 和握手失败不新建隐藏会话，不回退 Notify。原有合成 PipeWire 捕获及旧 Portal 授权测试继续通过。
- macOS 原生 91 通过、1 个 opt-in 忽略；macOS/Linux desktop 各 338 通过，分别 7/10 个 opt-in 忽略。原生 all-targets 及 desktop lib/tests 严格 Clippy 通过；Windows GNU 原生 all-targets 严格静态检查通过，未运行 Windows 程序。
- 独立源码副本分别移除每次输入前的代次保护、移除 mapping_id 匹配、把自己的修饰键反馈当外部变化，对应真实 EI 回归均退出 101（测试断言失败而非编译失败）；还原副本后 12 项通过；随后增加按下期间 click/重复 press 的真实协议回归，最终当前工作区源码在同一私有 target 清理重编后全部 120 项通过（含 13 项 EIS），严格检查及私有 Portal 链路再次通过。使用独立 proof target，每次 clean 原生包并确认旧测试产物已清除，未污染工作区 target。

日志：最终当前源码以 `/tmp/dogcodes-eis-current-source-final.log` 为准；此前 `/tmp/dogcodes-eis-linux-validation.log`（完整单元结果）、`/tmp/dogcodes-eis-linux-final.log`（严格检查/Portal）、`/tmp/dogcodes-eis-{macos-tests,macos-clippy,desktop-macos,desktop-macos-clippy,desktop-linux,windows-clippy}.log`。反例副本路径/摘要在 `/tmp/dogcodes-eis-witnesses.json`，结果在 `/tmp/dogcodes-eis-witness-results.json`，分项日志为 `/tmp/dogcodes-eis-witness-*.log`。

## 未完成边界与后续

1. 未提供 ei_text 的 compositor 不能凭 keymap 输入任意中文/emoji；目前明确拒绝，不借用用户剪贴板，也不伪装成功。该类后端的完整 Unicode 方案仍需补齐，不能据本轮将“文字输入全面覆盖”标完成。
2. 缺少 ei_text 时，若同一批次先执行快捷键再输入文字，快捷键可能切布局/锁定状态，当前在整批执行前拒绝并要求分开发送；完整混合批次的规划仍待补齐。
3. 真实 GNOME/KDE 等 compositor 的分设备布局、多显示器、热插拔/缩放/转屏、实际 Portal 授权与撤销仍需实机验收。假服务不证明厂商实现符合协议；也未证明 GPU/视频性能或真实弱网。
4. Linux 硬编、HDR/原生光标/GPU 少拷贝、设备/进程恢复，以及 PC 功能对齐、ACP/Core 恢复、真机/链路、推送/部署保持原范围；八个工作包未整体关闭。
