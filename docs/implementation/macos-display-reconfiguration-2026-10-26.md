# macOS 原生显示器配置变化

参考本机 RustDesk HEAD `53996e3c4` 的 `src/server/video_service.rs::try_broadcast_display_changed`（通知变化后重建）和 `src/platform/macos.mm::DisplayReconfigurationCallback`（回调内不注销自身、短临界区、与原生清理分离）。本项目使用 ScreenCaptureKit 和既有视频代次/ready/IDR 协议，独立实现。

## 当前实现

- 在取得 SCShareableContent/原生几何前注册显示器观察，覆盖初始化期间的模式变化。状态属于本次采集；BeginConfiguration 暂停编码与最终发布，完成通知将本流永久标为需重建。不能把完成当作旧流重新获准发布。
- 视频 owner 等配置完成才推进 epoch、退役旧流、重读原显示器并重建。等待最多十秒；重复 begin 不延长期限。配置中可能出现的 SCStream 停止先封住旧输出，不在系统提交新配置前直接结束租约。原租约取消仍能退出等待。
- 通知仅分发显示器 ID、begin/commit 和原子状态，不读取屏幕、不创建/停止采集、不调编码器、不从回调注销自身。只匹配原显示器，不暗中换到其它屏幕。
- 进程共享一个注册；每次原生采集保留自己的订阅，沿用各两项采集/codec 原生配额。末订阅退出尝试注销；失败时保留一个静态、NULL context 的空注册，之后复用，避免重复注册或访问已释放的 userInfo。全局只存弱引用，晚到事件不能访问已删除的消费者。
- 为无 Cocoa 事件循环的后台 Core 增加原生 worker 的每秒核对：实际 bounds、物理像素、rotation、online/active。查询和 FPS 更新都继续受现有两秒运行期限约束；FPS 不变或配置尚未提交时不反复发送 SCStream 配置。已收到 begin 时，探针不读取/提交中间几何；未收到通知时检测到变化接入同一代次重建；显示器断开/停用给出具体原因。未用像素代替输入逻辑坐标。

回调时序依据：[Apple 显示器配置通知](https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/QuartzDisplayServicesConceptual/Articles/Notification.html)。系统变更前通知 begin，完成后显示器属性才保证已更新；不能将 begin 中的临时状态当作最终配置。注销使用相同函数和 NULL 指针，参照 [Apple 注销接口](https://developer.apple.com/documentation/coregraphics/cgdisplayremovereconfigurationcallback(_:_:))。

## 验证与边界

验证使用假的注册/通知和合成像素，没有注册真实系统观察、修改显示器或采集用户屏幕。覆盖共享注册/末订阅注销、失败注销的复用、旧订阅隔离、同步回调不死锁、重复 begin 不延期、只影响选中显示器、授权检查中的 begin 丢弃旧 codec 输出，以及每秒探针补漏与断开错误。真实 SDK 的绑定与 ScreenCaptureKit 路径经过编译/严格检查。

最终当前源码检查已通过：macOS 原生 79 项（1 项 opt-in 忽略）、较新 Linux 原生 95 项（7 项忽略），两端完整 desktop 各 336 项（Mac 7 项/Linux 10 项忽略）；原生 all-targets 及 desktop lib/tests 严格 Clippy 通过。较新 Linux 三项私有 Source/codec/累计 ACK/尺寸接续验收显式通过。Windows GNU 原生 all-targets 严格静态检查通过，未执行 Windows 程序。本轮未重跑旧 Linux 或手机测试；手机代码/协议格式没有改动。

日志：`/tmp/dogcodes-mac-display-final-{checks,linux,windows}.log`、`/tmp/dogcodes-mac-display-format.log`；检查进程均已确认退出 0，自有测试容器已退出，原有三个 server-dev 服务保留。没有注册真实系统观察、采集用户屏幕/键鼠/剪贴板、切换日常 Core、运行真实 Agent/模型或模拟器，未提交/推送。

没有以假通知证明真实显示器转屏、DPI、热插拔、休眠或 GPU 设备恢复；这些仍待实机。

## Wayland 动态输入的后续设计

后续实现已接入 [原授权 EIS 后端与可信区域](wayland-eis-input-2026-10-26.md)，包括真实 EI socket 和独立 Portal 链路验证。下文为设计时依据；当前直接使用 reis 0.7.1 的 text 能力，Enigo 原有 reis 0.5 仍保留。无 ei_text 后端的任意 Unicode/混合批次和实际 compositor 仍继续，不能由本轮假服务验证外推完成。

本轮检查了 RustDesk `libs/scrap/src/wayland/pipewire.rs::try_fix_logical_size`。它也会核对来源物理尺寸和输出区域，遇到 Full Workspace 不匹配不会盲用区域；本项目应保留同样的边界，不仅凭相同分辨率猜某台显示器。

[Portal RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)规定 ConnectToEIS 后所有输入必须走 EIS，不能保留 NotifyKeyboardKeysym 再只替换鼠标。[ScreenCast mapping_id](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)才是流与 EI absolute region 的正式关联，node ID 不能代替它。当前传递依赖 reis 0.5.0，不能假定它已支持新协议的 text 能力。

后续整体迁移需覆盖：原授权 FD（不调用 Enigo 自行申请另一授权）、映射身份、设备增删/暂停/恢复与区域代次、整批输入冻结和执行前复核、跨区域/旧设备事件拒绝、键盘布局/组合键/Unicode、释放已按下键鼠、撤销和 socket 结束；ConnectToEIS 一旦成功，失败应退役该授权，不能在原会话混用 Notify。老 Portal 可在建立 EIS 前选择现有 Notify 路径。保留当前坐标不确定时的拒绝，直到整体实现并经隔离协议服务验证。

完整目标还包括原生硬编/采集、GPU/进程恢复、P2P/TURN/WSS 实际弱网/争用，以及剩余 PC 功能与操作恢复。此实现不关闭这些项目。
