# 原生光标形状与只读位置

## 行为

Windows、Linux X11 的独立光标可通过现有远控回复传到手机，显示原生文本光标、手形、箭头等形状及热点。macOS 视频和已内嵌光标的 Portal 来源继续直接显示画面中的光标；macOS JPEG 本轮只提供位置与通用箭头。

参考本机 RustDesk `src/server/input_service.rs` 的位置/形状分离、客户端形状缓存，以及 `src/platform/{windows,linux}.rs` 的原生数据读取思路。使用本项目自己的读取、PNG 数据、租约与请求协议，没有复制实现。

## 接入

- 每个桌面租约持有独立 `cursor::Reader`。构造时不访问 OS，读取不创建 Enigo 或请求输入权限；只读连接也能获得当前所选显示器的位置。Portal 来源不会进入未授权的 X11/系统读取路径。
- Windows 在原生线程 DPI 上下文内读 CURSORINFO，复制当前图标并读取自有 mask/color 位图；GetDC、位图和图标全部按原线程释放。检查尺寸、平面和实际位图范围后请求 top-down 32bpp DIB。透明、半透明与单色 AND/XOR 分开处理；反色像素采用对比描边表示，避免依赖读取目标窗口背景。Windows API 的所有权和格式依据：[GetIconInfo](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-geticoninfo)、[GetDIBits](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-getdibits)。
- X11 读取原 DISPLAY 的 QueryPointer/XFixes 图像，不注入输入；像素转为 PNG straight alpha。每次读取当前内容，资源 ID 重用也不会卡在旧图片。扩展不可用时保留位置与通用箭头。
- 原生位置使用当前显示器几何进行归一化，离开所选屏幕后返回 null。返回前复核锁屏、租约/连接仍有效；图片在光标读取后再做最终检查。
- 手机保留一个当前形状；本地移动预测期间仍可更新形状，热点始终对准点击目标。显示器/来源切换清空形状确认；离屏/隐藏只清位置。原生 PNG 按缩放显示，普通 32px 指针在缩小桌面时保持可辨识大小。图片错误可退回通用箭头。

## 当前协议

复用 `remote_desktop_frame` 与 `remote_desktop_video`，P2P/TURN 数据通道和 WSS 继续走相同请求。

| 字段 | 用途 |
| --- | --- |
| 请求 `knownCursorShapeId` | 手机当前确认持有的形状；缺少或不相同即再次返回图片 |
| video 请求 `includeCursor: true` | 初次就绪及既有状态轮询读取光标；普通高频 ACK 不读取、不发送光标图片 |
| 回复 `cursor` | 位置 x/y、当前 displayWidth/displayHeight、shapeId；不在所选屏幕或隐藏时为 null |
| `cursor.shape` | 仅在需要时返回 id、width/height、hotX/hotY 和 PNG base64 bytes |

形状 ID 来自像素、尺寸和热点，不传原生句柄。PC/手机各只缓存一个形状；最大边长 512、PNG 128 KiB，base64 回复可放入现有数据帧预算。手机在图片解码前核实 PNG 头、尺寸、热点、ID 和大小。没有新服务、持久格式、迁移或旧版回退。

## 本地证据

- 四项新原生默认回归：形状确认/丢缓存重取、热点/尺寸拒绝、单色 mask、alpha/颜色 mask。
- 私有 Xvfb 真实 XFixes 回归：8×8 自定义红/蓝光标、指定热点和位置，同一 X 资源 ID 换图后重新发送，确认后不再带图片，丢确认可重取。仅修改容器内虚拟根窗口，未触碰用户桌面。
- 六项新手机回归：形状缓存/来源切换、缩放热点、本地预测期间更新形状、数据边界、JPEG 页面、视频页面；页面核对实际绘制与后续形状确认，图片只发送一次。
- 手机普通测试 439 项通过、9 项原生 FFI 按需测试未启用；五个修改文件 analyze 无问题。macOS 原生 105 项、desktop 348 项通过；Linux 原生 146 项及独立 Xvfb 专项通过（10 项按需用例默认未启用）。macOS/Linux 原生与 desktop all-targets 严格检查、Windows GNU 原生/desktop all-targets 严格静态检查通过。日志 `/tmp/dogcodes-native-cursor-{macos,desktop-final,flutter-final,flutter-targeted2,analyze-final,windows-final,windows-flags,desktop-windows-final}.log`。Linux 最终证据见同名前缀的 `linux-final.log`。

最初隔离镜像缺 Xvfb，已在单独测试镜像补齐并等待实际连接就绪；初次失败不计为验收。原开发服务未重启，未运行真实 Agent、手机设备、用户屏幕/键鼠/剪贴板，未提交推送。

## 保留边界

Windows GDI/真实 cursor 格式、触控隐藏、DPI/多屏和真实应用仍待 Windows 实机。Desktop Duplication 的动态独立光标判定已按下节接通，不能据代码与隔离测试宣称全部双光标场景已实机验收。macOS JPEG 原生形状仍保留；Portal 独立光标元数据来源已在下节后续接通，只有 Hidden 模式的来源不能凭共享画面读取系统光标。HDR、GPU 少拷贝、真实设备/弱网与其余既定工作包继续。

## 2026-10-27：DXGI 动态光标状态

DXGI 的 PointerPosition 仅在 LastMouseUpdateTime 非零时有效；没有鼠标更新保留上次有效判定。Visible 为 true 表示可绘制独立指针；false 时禁止独立叠加，可能是隐藏、离开输出或指针已在像素中，不能一律解释为画面里必有可见光标。依据：[帧信息有效性](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_frame_info)、[独立指针可见性](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_pointer_position)。

- 新建或重建 DXGI 来源从未知状态开始，不复用旧来源判定，也不把未初始化的 Visible 当作有效信息。仅鼠标更新仍不做 GPU 回读，元数据随缓存画面交付；成功 GDI 回退明确恢复独立光标。
- 状态随原采集帧经过缩放、软件/硬件编码；相同像素但状态改变仍强制一次 IDR。最终发布在原代次的门内确认传输接纳后才更新状态，丢弃帧、旧代次和停止后的回调不能覆盖它。
- 复用现有 `video.cursorEmbedded`：false 允许独立光标，true 禁止叠加，null 表示尚未确定。手机在 true/null 时只显示操作后的短时预测；模式切换清理旧位置、形状和预测。未知状态不会触发独立原生位置查询。
- 这里记录的是最后被传输接纳的帧状态，不是手机已解码某一 RTP 帧的证明。状态轮询与画面播放仍存在时间差；未新增 SEI、帧确认服务或兼容协议。

新增回归覆盖无鼠标更新、仅鼠标更新、重建/回退、相同像素的缩放/编码/强制 IDR，以及最终发布拒绝与旧代次隔离。手机新增未知状态与预测过期用例。macOS 原生 109 项、desktop 349 项和手机普通 441 项通过；手机 9 项原生 FFI 按需测试未启用。macOS 与 Windows GNU all-targets 严格检查、手机相关文件 analyze 通过。Linux 原生 151 项、desktop 349 项和两包 all-targets 严格检查通过（分别 10/11 项按需测试未启用）。最初检查发现一项 Linux 专用测试夹具缺少新增字段，补齐后全量验证通过；失败检查不计作验收。日志 `/tmp/dogcodes-cursor-mode-{macos,strict-macos2,windows2,flutter-all,analyze-final,linux-final}.log`。没有触及日常 Core、真实屏幕/键鼠/剪贴板、安装 Agent 或设备；未提交推送。

## 2026-10-27：Portal 独立光标元数据来源

原实现优先请求 Embedded，没有此模式就选 Hidden；支持 Metadata 的 compositor 因而也看不到真实光标。现在选择顺序为 Embedded、Metadata、Hidden。Metadata 模式只在原授权 PipeWire 缓冲内读取光标，先在 PC 合成进画面，再复用 JPEG/H.264 和现有 `cursorEmbedded: true`；不增加手机 RPC、系统权限、服务端能力或独立光标队列。

- 请求有界 SPA Cursor 元数据，最大 512×512；校验描述符、位图相对偏移、行跨度、像素格式、热点及完整范围。整数 POD 允许未对齐读取，复制后不保留原缓冲指针。RGBA/BGRA 使用预乘 alpha 混合。
- 仅保留一个原始干净背景和一个当前光标。位置更新复用原形状，明确不可见时清掉形状；`id == 0` 是无新数据，不误当隐藏。相同原生 ID 仍采用新位图。跳过排队旧画面前也读取其中的形状更新，避免最终位置更新丢失前面的图片。
- 没有新像素的纯光标更新在同一干净背景重新绘制，随后按原画面的裁剪/方向呈现，移动/隐藏不在旧叠加图上绘制。格式重协商清掉原背景和光标；锁屏、原授权失效、租约/代次、编码和最终发布沿用现有检查。
- 不支持内嵌或元数据的 Hidden 来源保持其系统能力，不绕过 Portal 读取 X11 或额外申请输入权限。真实 compositor 的位图/热点、旋转、多屏/DPI 和性能仍须实机验收；该 CPU 合成没有解决 GPU 少拷贝。

四项新普通回归通过，涵盖元数据-only 移动/隐藏、背景无残影、透明度/热点/裁剪旋转、畸形范围及损坏帧。私有 D-Bus + 真实 PipeWire `rgb-cursor` 专项验证模式选择、原始光标像素、没有新画面时的位置与隐藏、三个尺寸的实际 H.264 编解码，以及原有 JPEG、锁屏和撤销防泄漏。普通 `rgb` 与裁剪旋转专项也通过。Linux 原生 155 项、desktop 349 项通过（10/11 项按需测试默认未启用），两包 all-targets 严格检查通过；相同专项已加入桌面 CI。首轮启动器漏列新格式已修正，失败运行不计验收。日志 `/tmp/dogcodes-wayland-cursor-{linux,fixture,final,jpeg-final}.log`。本轮只改 Linux 路径，未重跑无改动的手机/macOS/Windows 测试；未触及用户桌面/输入/剪贴板、日常 Core、安装 Agent 或设备，未提交推送。

语义依据：[Portal 的三种光标模式](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)、[SPA cursor 的无新数据和相对偏移](https://docs.pipewire.org/structspa__meta__cursor.html)、[SPA bitmap 的不可见与数据偏移](https://docs.pipewire.org/structspa__meta__bitmap.html)。本机 RustDesk `libs/scrap/src/wayland/pipewire.rs` 同样优先 compositor 内嵌；独立元数据与有界合成为本项目自行实现。

## 2026-10-27：macOS 依赖下限与图片光标边界

重新核对本机 RustDesk `src/platform/macos.rs::unsafe_get_cursor_data`：它用 NSCursor.currentSystemCursor 获取形状。本机 Xcode SDK 的 NSCursor.h 已标注不建议继续使用此属性，并明确未来会始终返回 nil；currentCursor 则是本应用的光标，不能代替其它应用正在显示的光标。Apple 推荐用 ScreenCaptureKit 的 showsCursor。因而不能通过换成 currentCursor 或读取私有 cursor seed 来声称图片路径已完成。现有视频已使用 showsCursor；JPEG 原生形状仍保留待办，应复用受原租约、取消及原生名额约束的采集，不另建无监督的截图/转换循环。

同时发现部署声明低于已有依赖：当前原生测试 Mach-O 对 ScreenCaptureKit 使用 LC_LOAD_DYLIB 强链接，未打包产物的 minos 为 11.0；本机 SDK 明确 SCStream 及 showsCursor 从 12.3 可用。桌面配置未设最低系统版本，而当前 tauri-utils 2.9.3 的配置默认值为 10.13。现在统一 Tauri 配置增加 `bundle.macOS.minimumSystemVersion: "12.3"`，主 README、打包说明和原生边界同步说明。两种 Mac 架构及签名覆盖层沿用该基础配置，不引入旧版协议或数据迁移。

验证：`pnpm test:release` 32 项通过；`cargo check -p desktop --lib --features custom-protocol --locked` 通过，实际 tauri-build 输出 `MACOSX_DEPLOYMENT_TARGET=12.3`。本地 CLI 帮助确认配置覆盖层逐项合并；现有签名覆盖层不允许修改系统版本字段。未完整构建、签名、公证或安装新的 app/DMG，未把未打包测试二进制当作新安装包的 Mach-O 证明；macOS 各支持版本与两种架构仍须安装实测。日志 `/tmp/dogcodes-macos-floor-{release-tests,custom-protocol,evidence}.log`。没有访问用户屏幕、键鼠、剪贴板、真实 Agent 或日常 Core；未提交推送。图片光标、HDR/GPU、原生故障恢复和原八包继续。


## 2026-10-27：macOS JPEG 复用受管 SCK 采集

图片拉取现在延迟启动同一有界采集实现，使用 BGRA 输出和 `showsCursor: true`，由系统把当前真实光标合成进屏幕，不读 `NSCursor.currentCursor/currentSystemCursor`、私有 cursor seed 或额外输入权限。只在首次图片拉取时启动，不因构造 Capturer 或启动 H.264 而创建图片流。请求 15 FPS，与手机现有 JPEG 最大请求速率一致；只保留一张原始 buffer，JPEG 请求时才读锁并转换，BGRA 队列深度为 3，视频原来的 NV12/深度 6 保持。图片尺寸仍受现有采集/缩略图和响应字节上限约束。

- 采集复用原生工作者及原名额、10 秒启动/2 秒配置观察期限、实际停止确认和显示器变化机制。底层 Delivery 的存活条件从单一 stop 标记泛化为原归属检查；视频仍检查原 stop，JPEG 检查同一可迁移租约的 Liveness。在异步读取屏幕完成后、真正开始采集前及帧交付时都检查；停止未知继续保留原名额，不声称已退出。
- BGRA 读取先核对格式、尺寸、总数据长度和行跨度，再用配对读锁/解锁按行复制，交换 B/R；不会把 NV12 平面当 BGRA。像素转换及 JPEG/base64 后重新核对原租约、锁屏和原采集/显示器状态，未变化回复也沿用校验。每次拉取更新当前逻辑屏幕信息，避免闲置重开后沿用旧尺寸。
- 图片流无拉取五秒时由原系统工作者退休，视频接管后不会持续采集图片。闲置重开更换帧缓存和显示器标记，旧停止/显示器回调只持有旧状态。后台失败在原 Capturer 内记住，不每次拉取重建；保留 xcap 截图回退。SCK 图片回复现有 `cursorEmbedded: true`，原截图仍为 false 并使用位置/通用箭头，不把回退称为有原生形状。不新增手机协议、手动设置或兼容层。

新增四项 macOS 普通测试：真实 CoreVideo 合成 BGRA buffer（含填充行、颜色和模拟光标像素）、最新帧替换及原归属失效、显示器变化/采集失败后禁发旧帧、闲置退休/迟到回调隔离与实际 SCK 配置属性。测试不启动屏幕采集；它们证明像素读取和生命周期/配置行为，不能证明真实系统已渲染某种光标。

验证：macOS 原生 113 项、desktop 349 项通过（1/8 项按需测试未启用），Linux 原生 158 项、desktop 349 项通过（10/11 项按需测试未启用）；两端两包 all-targets 严格检查及 Windows GNU 两包 all-targets 严格静态检查通过。Windows 没有实机执行。最后调整图片目标帧率为 15、闲置期限为五秒后，macOS 原生 113 项及两包严格检查再次通过；该调整只在 macOS 编译分支。日志 `/tmp/dogcodes-macos-jpeg-{targeted2,all,linux,strict,windows,final-native,final-strict}.log`。首次编译的可见性/unsafe 声明和夹具 CF 类型错误已修正，失败运行不计验收。

没有访问用户屏幕、输入、剪贴板，未启动设备/真实 Agent、切换日常 Core、构建签名安装包或提交推送。macOS 实际光标形状/隐藏、权限/显示器切换、长时间弱网及原生故障恢复仍随原实机包验收；HDR、GPU 少拷贝与其余八包范围不据本地测试关闭。
