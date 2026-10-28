# Wayland 文字粘贴回退：不备份、不自动恢复

日期：2026-10-26。用户最新决定覆盖此前“尽力恢复”的讨论：旧 Wayland 缺少 ei_text、当前 keymap 无法输入文字时，可以用剪贴板粘贴；**不读取或备份原内容，不尝试自动恢复**。

场景仍是手机远程桌面底部“键盘”向电脑聚焦窗口输入。Agent 消息、手机原生终端、用户手动发送/获取剪贴板的独立功能保持各自接口。

## 实现

- 在原 RemoteDesktop session 的 Start 之前请求 Clipboard 扩展，订阅同一接口的有序信号流；Start 的 clipboard_enabled 决定是否保留能力。没有二次授权会话或环境 socket。缺少接口/被拒绝不冒充已授权，也不对原生 EI 文字强制使用剪贴板。[Clipboard 协议](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Clipboard.html)规定该顺序与 FD 传输方法。
- 整批输入预演中，只有 literal Text 在 keymap 无法映射且具备剪贴板权限时才规划粘贴。按原 keymap 找 Ctrl/V/Shift 键码，等待带唯一标记的本会话所有权，再通过现有 EI 动作/帧/同步及授权代次检查触发一次粘贴。
- 只发布用户本次输入的 UTF-8 文字，不调用 SelectionRead。操作结束仅释放本地事务占用，继续提供当前文本，不发第二次 SetSelection、不切回旧内容、不创建延迟恢复定时器。之后用户/其他应用成为所有者时，放弃提供本会话文字，绝不重新抢回。
- 所有权与传输使用同一个有序 D-Bus 信号流。请求固定到原 session、当前唯一标记和已授权对象；每次传输冻结字节，旧请求完成不影响新事务。近期 serial 去重，活跃请求单独保护，最多四项写入。
- Clipboard manager 在触发粘贴前的预读不算输入交付；只记录 arm 后请求的文字传输及 SelectionWriteDone 确认。超时、断开、所有权变化或未知结果不自动重发。现有输入三秒总期限及外层取消保留，不用延时假定操作成功。
- FD 只接受 pipe/socket，设置非阻塞并使用 AsyncFd；部分写入、背压及每次写前的授权复核都有边界。socket 使用 MSG_NOSIGNAL，pipe 只在当前同步 syscall 周围屏蔽/消费本次 SIGPIPE 并恢复线程信号掩码，不改变整个进程的 handler。这里的信号掩码恢复与剪贴板恢复无关。

参考本机 RustDesk `53996e3c4` 的 `src/server/input_service.rs`：它也明确避免恢复剪贴板，因为目标应用可能尚未取数，恢复会使粘贴读到旧内容。本项目使用自己的 Portal/事务/FD 实现，没有复制其代码或采用其固定等待来判定交付。

## 自动选择粘贴快捷键

用户要求不增加手动选择器。优先级为原生 ei_text、当前布局的物理文字规划、原会话已授权剪贴板回退；手机和传输协议不新增快捷键选项。

进入最后一条路径时，PC 使用可选、只读的 AT-SPI 元数据查询：只接受活动窗口内带 FOCUSED 状态的控件，terminal role 采用通常的 Ctrl+Shift+V，其他情况一次 Ctrl+V。缓存每次检查控件角色、焦点、祖先窗口活动状态及 DEFUNCT；持有剪贴板后再核对一次，已识别的焦点变化则不发送按键。不按程序名字/标题猜测，不读取控件文本，不启用系统辅助功能。GetNameOwner 要求服务已经运行，后续调用绑定唯一连接名；总查询上限 200ms，缓存复核上限 100ms，树最多 128 个节点/12 层，跳过非活动窗口。可选服务缺失或断连后最多每五秒重新尝试接入。

无法识别、不可达或查询超时只走一次普通快捷键，绝不先 Ctrl+V 再试 Ctrl+Shift+V；未知结果不自动重发。这里只识别控件类型，不能得知应用的自定义快捷键；原生 Wayland/Portal 也没有统一的焦点应用接口。[AT-SPI Accessible 定义](https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/xml/Accessible.xml)。

RustDesk 的 Shift+Insert 路径同时设置 Linux CLIPBOARD 与 PRIMARY（本机 `src/clipboard.rs`）；当前 Clipboard portal 只管理其会话的剪贴板，不能直接照搬，否则有些应用会读取旧 PRIMARY。

## 证据

未读写本机真实剪贴板，未操作桌面键鼠、模拟器或模型，未切换日常 Core。均使用容器内独立 D-Bus、模拟 login1、合成 PipeWire 及真实 EI socket 协议。

- Linux 原生完整 130 项通过、7 项 opt-in 默认忽略；新增四项 FD 回归覆盖 Unicode 大数据部分写入/背压、取消/拒绝、无读端 pipe 和非流 FD。SIGPIPE 测试在独立子进程启用默认终止行为，证明写断管不会杀死 Core 或改变线程原掩码。
- 扩展原 Portal 专项并通过：原生 ei_text 不访问剪贴板；无 ei_text 的实际 EI Ctrl+V 引发 FD 请求并交付中文/emoji；连续两次操作各仅一次 SetSelection，结束后仍提供最新文字；用户复制新内容不被写回；权限拒绝保留观看；预读而无粘贴请求报告未知；交付期间外部复制保留新内容且不重放。模拟 Clipboard 的 SelectionRead 始终拒绝并计数，最终调用数为 **0**。
- Linux desktop 338 项通过、10 项 opt-in 默认忽略；原生 all-targets 与 desktop lib/tests 严格 Clippy 通过。本轮改动在 Linux Wayland，未将之前 macOS/Windows 的结果重记为本轮验证。
- 四个独立副本分别引入“把预读当粘贴”“结束后丢弃已提供文本”“读取原剪贴板”“不屏蔽断管信号”，均得到对应断言失败，最后一项子进程实际被信号 13 终止。独立 proof target 每次清理原生包并确认无旧二进制；还原源码后完整 130 项及 Portal 专项通过。

日志 `/tmp/dogcodes-clipboard-{validation,final,acceptance,desktop}.log`；反例路径/摘要 `/tmp/dogcodes-clipboard-witnesses.json`，结果 `/tmp/dogcodes-clipboard-witness-results.json`，分项 `/tmp/dogcodes-clipboard-witness-*.log`。最终源码与还原通过的副本逐文件核对。

### 本次自动选择验证

- 容器内私有 AT-SPI 树与真实 EI 修饰键状态联动：终端只接受 Ctrl+Shift+V，普通输入框只接受 Ctrl+V；同一会话内角色改变不沿用缓存，失去活动状态的终端不被采用。
- 焦点在 SetSelection 后消失时没有任何键盘事件；元数据超时只发送一次普通快捷键，恢复后同一会话重新识别终端。原生 ei_text 不查询 AT-SPI，所有场景没有 SelectionRead 调用。
- 最终 Linux 原生 130 项（7 项 opt-in 默认忽略）、all-targets 严格 Clippy、私有 Portal 专项通过。未重复无改动手机/desktop 检查，未将旧平台结果冒充本次验证。
- 独立只读源码副本的两项反例：强制普通快捷键时终端取不到文字并触发断言失败；移除按键前焦点复核时错误地成功粘贴并触发断言失败。每项清理独立 proof target 的原生包；恢复副本后完整测试及专项通过，当前 Wayland 源码与恢复副本逐文件相同。日志 `/tmp/dogcodes-adaptive-{paste,paste-final,proof}.log`，副本及摘要 `/tmp/dogcodes-adaptive-witnesses.json`，结果 `/tmp/dogcodes-adaptive-witness-results.json`。

## 仍须推进

Clipboard 的传输确认不证明目标窗口已插入文本，协议不提供请求应用身份；arm 后的其它预读也无法被完全区分，不能据模拟服务宣称全应用输入已验收。保留新文本避免了自动恢复导致的取数竞态，但连续快速输入、clipboard manager 及应用取数仍须实机覆盖。

已增加可选焦点元数据驱动的自动快捷键选择，不增加用户设置。未暴露 terminal role 的集成终端、未运行 AT-SPI、超出查询预算的复杂控件树、自定义绑定及多个桌面会话仍须真实应用验证；输入与焦点无法做到系统级原子操作，不能宣称覆盖全部 Wayland 应用。可映射非拉丁字符的键符比较已修复，见 [当前 EIS 记录](wayland-eis-mixed-input-2026-10-26.md#2026-10-27非拉丁键符的等价字符)。非拉丁布局未必能映射拉丁 V，不能据此擅自猜测其它组的粘贴快捷键。没有 Clipboard portal/ei_text 且当前布局也无法输入时保持无副作用失败，文案不再要求用户给不存在的权限；实际后端与应用仍需验证。

原授权结束后由系统 clipboard manager 决定内容是否缓存，尚未实机证明。Linux 硬编、设备/进程恢复、HDR/光标/GPU 路径、PC 功能对齐、ACP/Core 恢复、真实链路与部署均不由本轮关闭；八个工作包继续维护在 CURRENT_WORK。
