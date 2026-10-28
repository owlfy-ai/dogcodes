# 当前屏幕坐标与恢复期间的输入

## 修复的问题

NativeController 之前直接使用 Capturer 创建时保留的 Display，屏幕移动、旋转、缩放或视频管线重建后，鼠标与光标可能仍按原范围换算。另外，本项目使用的 Enigo 0.6.1 Windows 绝对移动按主屏换算，缺少 virtual desktop 标记；xcap Linux 的显示几何会除以 Xft 缩放，而 X11 注入与鼠标位置使用根窗口像素。

参考本地 RustDesk 的 libs/enigo/src/win/win_impl.rs 对整个虚拟桌面处理多屏坐标，以及 src/platform/windows.rs 的系统定位入口。使用本项目自己的当前屏幕查询和输入生命周期，不复制实现。

## 实现

- 输入前获取当前同一显示器的一份坐标快照，整批使用同一快照；读失败、屏幕断开或身份不符时拒绝，不能回退旧范围，也不先执行同批键盘动作再发现屏幕不存在。
- macOS 直接读取当前 CoreGraphics logical bounds，和 Quartz/Enigo 点坐标一致，不反复读取所有显示器的名称、DPI 等无关信息。
- Windows 对原显示器 ID/设备名复核后，以 EnumDisplaySettingsW 获取当前物理范围。仅输入线程临时采用 per-monitor DPI awareness，结束后恢复原上下文；SetCursorPos 定位可覆盖负原点和副屏，避免 Enigo 的主屏绝对换算。
- Linux X11 使用当前 RandR output/CRTC 的根窗口像素，不应用 Xft 缩放；复用本 Controller 的连接，WarpPointer 明确指定本 DISPLAY 的根窗口。断开、端口身份变化、无效范围和 X11 坐标协议越界明确拒绝，不钳制到别的屏幕。
- 只有移动、滚轮和鼠标按下需要屏幕范围；独立键盘、文本和释放不依赖已移除的显示器。若移动和按下之间发生范围变化，保留原标准化点并重新投影；不会把另一显示器的已知位置套过来。光标报告也读取当前范围。
- 网关输入固定原 owner，受租约和两秒交互期限限制。读取范围后再次检查锁屏状态，执行后续动作前检查连接/期限；迟到查询不会变成已失效连接上的新输入。已经提交给系统的效果仍遵循原有“不重放、错误后结束租约”语义，不把未知效果改成成功。
- Wayland 保留原 Portal/EIS 授权及动态区域逻辑，不能回退成 X11 输入。

## 手机行为

运行恢复与普通几何/编码器重建都会暂停新输入。队列丢弃尚未发出的旧画面动作，释放本队列已经发送/在途的按键和鼠标按住状态；只释放自己跟踪的状态，不为冻结期间被丢弃的点击发送无关释放。传输换路的所有权等待独立保留；输入结果未知时不补发释放或重放原动作。

PC 报 ready 仅证明发送了画面，不能解除手机的输入暂停；需观察本机解码进展。中继路径保留旧纹理直到新 decoder 输出，RTP 在 ready 前记录已有解码计数并等待后续进展。重复旧恢复状态不反复冻结；新几何改变会重新进入等待。20 秒本地状态期限保持，超时仍回退图片。不会恢复被丢弃的点击或文字，之后只执行新操作。

## 验证

- 十项跨平台 native 回归：移动/旋转后单快照换算、物理/逻辑单位与视频尺寸独立、断屏不回退/不部分执行、无屏释放/键盘可用、无效范围、移动到按下间重新投影、不同屏幕快照拒绝、输入失败停止余项、迟到查询原 owner 失效、批次中 owner 退休。
- Linux 另有真实 x11rb/RandR/WarpPointer 协议专项：私有 Unix socket 模拟服务，验证原输出 ID、3840×2160 根像素、改为旋转后的 1080×1920、目标 root、断开与越界不移动。没有使用用户 DISPLAY、Xauthority、Enigo，也没有连接实际 X server 或用户鼠标。
- macOS native 101 项、desktop 347 项通过；Linux native 142 项、desktop 347 项通过；相应真实设备专项继续跳过。macOS/Linux all-targets 严格检查、Windows GNU all-targets 静态严格检查通过。
- 手机新增六项队列与一项几何等待回归，并加强原运行恢复/清晰度页面测试：旧图片点击不发送、解码后也不重放、新点击可以发送。完整 Flutter 401 项通过、无跳过，六个修改 Dart 文件 analyze 无问题。

证据日志 /tmp/dogcodes-input-geometry-{acceptance-final,acceptance-linux,wire-linux,mobile-full,mobile-analyze}.log。早期 fixture 包头/等待和旧测试语义失败均已修正，以最后完整运行作证据。未操作用户桌面、键鼠或剪贴板，未重启日常 Core；服务端开发容器保持。

## 边界

没有宣称完成 Windows/macOS 实际显示器、X11/Wayland compositor、混合 DPI/旋转/热插拔、手机真机和 P2P/TURN/WSS 的实机验收。私有 X11 服务证明请求与坐标单位，不替代真实系统注入验收；RTP 解码进展也不等同逐像素或逐 RTP 时间戳的端到端画面身份验收。原生系统服务本身的停顿/驱动恢复、HDR/原生光标、GPU 少拷贝、其余 PC 功能及执行恢复继续。没有为未发布的软件保留旧数据迁移。
