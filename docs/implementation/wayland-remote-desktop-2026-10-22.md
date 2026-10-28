# Wayland 共享授权与持续视频（2026-10-22）

## 代码范围

- PC `crates/remote-desktop/src/wayland/` 管理一个连接所有的 RemoteDesktop/ScreenCast 会话：D-Bus 授权、恢复令牌、已授权屏幕、PipeWire FD、键鼠与结束清理。
- `daemon/remote/native.rs` 继续负责配对后的连接归属、15 秒租约、输入序号及禁止重放；没有把授权判断移到手机或服务端。
- 采集、JPEG、H.264 和输入引用同一授权会话，避免为录像与 Enigo/libei 分别弹出两次授权。Wayland 路径不调用 xcap 的截图 portal 或无界 recorder。
- 手机先完成授权准备，再列出本次允许的屏幕并打开租约；输入和图像都映射到这一屏幕流。只有画面可读而无完整键鼠授权时，不宣称已获得控制权。

## 授权与生命周期

`device_status.desktopPreparationRequired` 仅在 Wayland 返回 true。手机使用同一物理连接轮询：

```json
{"method":"remote_desktop_prepare","args":{"control":true}}
```

回复为 `state: pending | ready`；待授权时显示“请在电脑上允许共享屏幕和键鼠控制”。系统授权最多等待 120 秒；手机每 500 ms 查询，准备阶段没有占住原生操作线程。连接退出、锁屏、准备阶段 15 秒无人续期都会取消。用户取消或明确授权失败后，需要手动重试，避免重复弹窗。

同一准备过程固定 owner/control/fresh；其他连接不能接续或使用其授权。准备成功后，所有权转交给远控 Lease。关闭 Lease 时立即结束 portal，即使后台仍持有截图/编码的旧引用也不能继续发输入。每个输入事件前核对连接是否仍活着；在途 D-Bus 调用也有取消监听（约 100 ms），不等待 3 秒的操作期限才结束。输入批次有 3 秒 OS 调用期限、调用方 4 秒等待期限；结果未知结束会话，不自动重放。

授权撤销监听在 Start 前安装，防止“刚批准即撤销”漏过。PC 对已结束授权回复 `authorizationEnded: true` 和原因，手机的打开、帧、输入、切屏与视频保活均处理此标记，停止并等待用户重新连接。

初次系统确认后，请求系统保存可恢复授权。控制会话的持久化只通过 RemoteDesktop.SelectDevices；只读会话通过 ScreenCast.SelectSources。恢复令牌单次使用：先从磁盘消耗旧令牌，再发请求；收到新令牌后原子保存。控制与只读分别存储，目录 0700、文件 0600，不返回手机；generation 和连接存活检查阻止旧会话覆盖后来的令牌。

手机屏幕菜单新增“重新选择共享屏幕”。它关闭旧连接，准备时固定 `fresh: true`，消耗但不使用旧恢复令牌，由 PC 系统重新选择共享来源；普通重连继续使用有效令牌。系统是否接受持久授权仍由桌面门户决定，旧版门户或用户撤销时可能再次要求 PC 确认。

## 帧流与输入

- 通过 **OpenPipeWireRemote 返回的 FD** 连接受限 PipeWire，不连接默认服务器来猜测屏幕节点。有 `pipewire-serial` 时使用 `target.object`，避免节点 ID 被复用；旧门户兼容 node ID。
- 只接受用户授权的 monitor stream；逻辑尺寸用于绝对指针，像素尺寸来自真实帧。负坐标和缩放不混进 stream-local 坐标。
- 同时仅保留最新 RGBA 帧；有界软件采集轮询复用 Arc，变化才缩放/编码。支持 RGB/BGR24、RGBA/BGRA/RGBx/BGRx 的有界转换、行 padding 和 offset 校验。拒绝损坏/不完整/超尺寸缓冲；格式不支持或流断开时结束授权。
- H.264 接入现有软件编码管线、CB3.1 预算、关键帧、码率/帧率调整及 WebRTC/P2P、TURN、WSS 发送，不新增服务端视频协议。手机解码失败时 JPEG 使用同一授权源，不能通过回退另开截图授权。
- 优先让 compositor 内嵌光标；支持独立元数据的来源则在 PC 合成后通过 `cursorEmbedded` 告知手机。只读模式不创建输入控制器，见下方后续记录。
- 输入批次在第一项效果前验证全部事件；按 monitor stream 的逻辑坐标发送绝对移动，按钮使用 evdev 码，按键/文字使用 keysym，滚动为离散轴。切屏先释放旧按钮/按键。没有混用 EIS 与 Notify 方法，也没有增加 uinput/root 服务。

## 已验证

- Linux 原生库普通测试：22 项通过、2 项 opt-in 跳过；包含像素布局/越界、RGB24、逻辑坐标、Unicode/换行/快捷键协议和恢复令牌隐私/代次/单次消耗。macOS 原生库原有 18 项通过，Windows GNU 边界交叉检查通过。
- `python3 scripts/wayland_acceptance.py` 已在隔离 Linux Docker 通过：独立 D-Bus 假门户 + **真实 PipeWire videotestsrc RGB24**，FD 交付 → 320×240 像素 → 实际 H.264 编码 → 解码。另验证恢复令牌、fresh 重新选择、只读隔离、按键释放、输入失败后的后续事件不执行、授权等待取消/拒绝、批准瞬间撤销，以及连接死亡取消在途输入。
- 上述脚本会创建自己的消息总线、运行目录、PipeWire 与 WirePlumber，退出按已知 PID 清理；仅测试子进程 PATH 的合成锁屏检查返回未锁定。没有生产代码的锁屏绕过，也不访问用户桌面/键鼠。CI 使用 WirePlumber 0.4（Ubuntu 22.04）；不是产品对 WirePlumber 的版本限制。
- Linux 完整 desktop 库已通过编译及严格 Clippy；Tauri 的生成目录使用独立 Docker volume，未改宿主生成文件。macOS PC 库 290 项通过/7 项跳过，手机 276 项普通测试通过；静态检查和普通应用构建以最终执行记录为准。

## 保留的未完成范围

- 假门户不代表 GNOME/KDE 或其它 compositor 的真实系统授权、跨重启恢复、热插拔、多屏、锁屏和混合 DPI 已验收；用户已明确实机联调后续安排。需验证真实门户对 keysym/中文与 IME 的接受程度、文字吞吐及 3 秒输入批次期限，不能把协议测试当成中文实机输入验收。
- compositor 需要提供 ScreenCast；控制还需 RemoteDesktop。缺接口时明确报错，不通过 XWayland 猜测坐标或绕过系统权限。仅 DMA-BUF/NV12 的来源、硬件加速与更完整光标元数据仍待补齐；当前公共 packed RGB 路径已接入。
- Windows/Linux 原生硬件编码、视频缩放驱动分辨率、远控自身无缝换路、真实弱网/长时/TURN/手机硬件验收仍保持待办。Wayland 原生剪贴板权限的兼容验证与剩余写操作恢复审计也没有因此关闭。

协议依据：
[ScreenCast](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)、
[RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)、
[PipeWire buffers](https://docs.pipewire.org/structspa__chunk.html)。RustDesk 仅参考会话/来源/输入共享的思路，没有复制实现。

## 2026-10-27：独立光标元数据

已补 Embedded 不可用而 Metadata 可用的路径：从同一受限 PipeWire FD 读取 SPA cursor/bitmap，按热点与预乘透明度合成，再经过原裁剪/方向和现有 JPEG/H.264。仅光标移动、隐藏和队列中先前的形状更新不会丢失；保留干净背景避免残影，不读取 X11/Enigo。格式重协商清理缓存，授权和最终发布栅栏保持。最大光标 512×512，单背景/单光标，无新网络协议或持久格式。

私有 `rgb-cursor` 流实际验证像素、纯元数据移动/隐藏和三种尺寸 H.264 编解码；原内嵌 RGB 与裁剪旋转专项也通过。Linux 原生 155 项、desktop 349 项与两包严格检查通过。只有 Hidden 模式的来源仍受系统能力限制；真实 compositor、混合 DPI、旋转、性能和原工作包验收保留，详见 [实现与验证](native-cursor-2026-10-27.md#2026-10-27portal-独立光标元数据来源)。上文早期的后端/硬件待办以 CURRENT_WORK 与各后续记录为准，不重复立项。
