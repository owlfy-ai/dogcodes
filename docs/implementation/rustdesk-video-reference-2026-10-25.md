# RustDesk 远控视频对照与推进顺序

日期：2026-10-25。按用户要求充分参考本机 `~/Workspace/opensource/rustdesk` 工作树，基线 HEAD `53996e3c4`。以下是实现思路与边界对照，本项目代码、H.264/WebRTC/WSS 协议及测试独立实现。

## 对照依据与实际差距

| 主题 | 本机 RustDesk 对照位置与思路 | 本项目现状及继续工作 |
|---|---|---|
| 持续采集与平台来源 | `libs/scrap/src/common/{dxgi,quartz,wayland}.rs`、`libs/scrap/src/wayland/pipewire.rs`：复用采集器，区分暂无新帧和设备错误，处理 native buffer/格式 | macOS 持续 SCStream、Wayland 受限 FD、Windows Desktop Duplication 与有界 GDI、X11 有界截图已接入；Linux 硬编、GPU 少拷贝及真实设备/进程恢复继续 |
| 驱动能力与失败回退 | `libs/scrap/src/common/{codec,hwcodec,vram}.rs`：探测实际编码器/设备能力，失败后降级并更新后续选择 | Windows MF 硬编优先、预热/解码校验及软件 IDR 接续已加入；Linux VAAPI 等原生后端尚未完成，跨驱动失败退避/恢复与真实 GPU 验证继续 |
| 静态画面清晰度 | `src/server/video_service/static_refresh.rs`：检测源是否已就绪，按编码器/质量有界重复输入；分别限制失败和无输出，重新运动后重置 | 已加入按真实输出核算的有界静态刷新，独立限制尝试/输出/时间与无输出/IDR 期限；实际 OpenH264 编解码对照显示有界普通刷新达到长重复同等最终质量，额外 IDR 对样本更差。原生合成来源已测停止/后续 IDR；硬件/彩色文字仍需实机校准，详见静态刷新记录 |
| 网络反馈与 ABR | `src/server/video_qos.rs`：区分路径基线与排队增量；确认拥塞、先调码率、保留 FPS 下限，恢复有观察期；过期探针主动减速 | 二进制已分离超时/新鲜回复、主动制动/慢速基线老化，新目标自己的 ACK 和稳定/失败水平约束恢复；模型恢复速度与波动已有前后数据。满窗编码前反馈已补，RTP 仍用 WebRTC GCC；永久低带宽延迟折中、实际弱网/争用和两管线体验继续量化 |
| 队列与解码连续性 | `src/server/video_service.rs` 的 `VideoFrameController` 与已取帧通知：消费速度参与生产，不无限积压帧 | 已将二进制窗口/字节/channel 与 RTP 本地队列反馈到编码前，满窗保留最新画面；当前 Output 持有恢复至 IDR 被接受，原输入时间过滤暂停前回调，Mac Live 流有一项在途 native 信用。macOS codec/采集与 Windows 原生采集已有有界 worker 隔离和退役配额等待；慢解码/后台切换/真实争用、进程边界及跨平台实机继续 |
| 设备变化与重建 | `src/server/video_service.rs`：区分 DXGI access-lost、显示器变化、无新帧与编码故障，有限重建/回退 | 已补 macOS 配置通知/后台探针、动态尺寸新代次、Windows DXGI 失效有限重建/GDI 回退；保留原租约，迟到输出隔离。真实 GPU 设备故障、两项原生调用同时挂起后的进程恢复仍未完成 |
| 可见区域和方向 | `libs/scrap/src/wayland/pipewire.rs` 使用 `VideoCropMeta`；`src/server/wayland.rs` 还明确记录转屏与几何匹配的边界 | 已补固定布局的 SPA crop/八种 transform、呈现方向/原 node 输入坐标及编码后原授权复核；Header/任一 plane chunk 持续坏帧有界结束，空更新保持正常。运行中像素尺寸已能重建接续；Wayland 逻辑坐标不确定时保留观看并拒绝鼠标，可信输入区域/完整 EIS 及更多光标元数据继续 |
| 锁屏/切换用户与资源 | `src/platform/linux.rs`、`libs/base/src/platform/linux.rs` 检查活动会话/LockedHint；平台与前台状态决定采集行为 | 已确认产品边界是锁屏仍能操作 Agent。新补各平台发送前状态检查、Linux 有界 login1 查询、后台 Core 的本用户图形桌面核实；详见下节，真实 GNOME/KDE/Windows/macOS 状态仍待实机验证 |

本机参考代码包含其自身的功能边界与待验收说明。对照时逐项核实实际实现，不将文件存在、注释或编译通过视为已经完成能力验证。

最新网络恢复进展：[恢复速度与拥塞试探](video-relay-recovery-2026-10-25.md)。继续参考新鲜反馈、恢复水平及失败保护；独立模型验证恢复加快、稳定目标波动收窄，同时明确永久低带宽的延迟折中、真实网络与消费反馈仍待继续。

最新原生调用进展：[macOS codec 与销毁隔离](macos-codec-isolation-2026-10-25.md)。参考硬件探测的隔离/等待边界和有限设备恢复思路，扩展本项目有界 worker 至 VideoToolbox；回调不持有发布 sink，旧 Native 实际退出前仍占名额。它与 RustDesk 的探测进程不同，采集调用和真正设备/进程恢复继续。

最新采集进展：[ScreenCaptureKit 控制与退出确认](macos-capture-isolation-2026-10-25.md)。参考 Quartz 等实际停止信号后再释放的边界，将控制及对象释放移入独立配额的 actor；接入父租约取消、可撤销消费者和 delegate 直接封住旧编码输出。真实 OS 事件仍待实机，退役配额等待/快速重建、动态几何及进程/设备恢复继续；反例已用私有缓存逐次清理重编，见该记录。

最新退役进展：[原生名额有界等待](native-retirement-grace-2026-10-26.md)已补归还通知与同一启动期限，保持上限并允许健康槽连续重建；借鉴有界恢复原则，未复制 DXGI 状态机。两个池各 24 次和实际 VideoToolbox 八次重建已验证，动态几何/设备与进程恢复继续。

最新动态尺寸进展：[来源尺寸、视频代次与手机解码接续](video-dynamic-geometry-2026-10-26.md)。参考 `try_broadcast_display_changed` 的先通知/重建边界，软件来源变化不再直接终止观看，也不等待旧 ACK；原授权、发布代次与实际 IDR 握手均有隔离验证。物理像素不替代 Portal 逻辑坐标，坐标不确定时要求重新连接恢复鼠标；可信动态输入区域、macOS 原生几何及真实 compositor 验收仍继续。

最新 macOS 显示器进展：[配置通知、后台探针与原生接续](macos-display-reconfiguration-2026-10-26.md)。参考显示变化先通知再重建及回调不清理自身的原则，begin 封住旧输出，commit 触发原显示器新代次；没有 Cocoa 事件循环的 Core 用有界 worker 每秒核实几何。共享注册及注销失败的复用、等待期限有隔离证据，真实热插拔/转屏/权限/休眠仍待实机。Wayland EIS 需要整体输入后端迁移，不能只换鼠标并混用 Notify；范围与依据见该记录。

最新 Windows 采集进展：[Desktop Duplication 与有界 GDI](windows-desktop-duplication-2026-10-26.md)。参考 DXGI 无新帧/失效区分、有限重建和同源 GDI，新增原生采集池中的 D3D11/Map/COM 对象所有权及 RGBA 交付。静止不回读，恢复/失败有期限，根租约与局部停止分别生效，假挂起 actor 验证保留名额/当前回退/迟到输出隔离。GNU 编译/PE 不等同真机：HDR、原生光标、GPU 直达编码、真实 Windows/GPU 设备及进程恢复仍继续。

## 本轮已落地：发送前复核与 Linux 查询

后续 Wayland 输入进展：[原授权 EIS 后端与可信动态区域](wayland-eis-input-2026-10-26.md)。参考 RustDesk 的 ConnectToEIS FD 和物理/逻辑区域分离，接入 mapping_id、设备代次、全部输入 EIS 路径与协议级隔离验证；无 ei_text 后端的任意 Unicode/混合批次、实际 compositor 验收仍未关闭，不把接通鼠标等同完整输入覆盖。

原生编码可能在采集检查之后才返回。原实现只有 Windows 的 VideoStream 包了一层发送检查；macOS 异步编码和 Linux 的输出仍可沿用旧检查，JPEG 最后一次核实也发生在压缩之前。

现在：

- VideoStream 在启动采集前核实桌面状态，所有平台的实际输出都经过 `video/publication.rs`。先检查已停止、再核实当前 OS 状态，查询期间如已停止则丢弃；锁定或状态未知使本流结束并保存原因。解锁不会让旧流自动复活。
- 编码前的周期检查发现锁定/未知也结束本流；停止后 ticker 不再查询 OS，迟到采集不再保留新的 last frame。SyntheticEncoder 仍只用合成数据，不触及 OS 桌面。
- JPEG 完成压缩和 base64 后再次核实，避免压缩期间的锁定被漏掉；已有 unchanged 路径继续核实状态。
- Linux 改为直接调用 login1 D-Bus：每次读取新属性，不缓存 unlocked；复用连接，最多一个 OS 查询线程、32 项队列，每次调用最多等待 250ms。结果绑定原接收者和期限，迟到的“未锁定”不会成为下一次调用的许可。超时/繁忙/错误/缺属性均拒绝放行。
- 明确 session ID 时核实该会话；无 ID 且后台 Core 不属于登录会话时，检查自身 UID 的 Display 会话。还核实 User、Active、Remote 和 Type，拒绝其它用户、已切走/远程/非图形会话。显式旧 ID 失效时不悄悄换另一个桌面；Agent 日常操作不经过此屏幕检查。
- 同时修正原生 hook 退休时的地址：注销回调收到原注册 hook，避免 SDK 按值辅助函数把栈上副本交给 native removed callback。

接口依据是 [login1 的会话/用户与属性 API](https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html)。真实桌面环境仍需核实 LockedHint 的维护、用户切换及会话退出/恢复，不能以 mock 属性推断全部桌面环境正确。

## 证据与限制

- macOS remote-desktop 40 项、all-targets 严格 Clippy 通过；Windows GNU all-targets 严格 Clippy 通过，未运行 Windows 程序。
- Ubuntu 22.04/PipeWire 0.3.48、较新 Linux/PipeWire 0.3.65 各 55 项通过，5 个 opt-in 默认忽略；严格检查及生产 lib 编译通过。
- 两套 Linux 分别执行五种实际 PipeWire 合成来源。私有 login1 的当前/后台会话、失效 ID 不改选、未知状态后重连、锁定后停止/解锁不复活及 JPEG 压缩中锁定均通过；较新 Linux 额外验证 SPA RGB 来源。
- 普通回归覆盖发送前停止/查询中停止、诊断争用时先退休输出、锁定/未知不放行、过期回复、有限队列、用户/会话属性，以及 native hook 原地址。
- 反例：另一路只读挂载仅去掉最后 JPEG 检查，明确复现 `JPEG pixels were returned after the lock transition`；只恢复 hook 按值删除，原地址回归失败。对照在自有 Docker target 内完成，最终当前源码重新编译复验，没有覆盖工作区源码。

输出：`/tmp/dogcodes-publication-{mac-final,mac-final-clippy,win-final}.log`、`/tmp/dogcodes-publication-{ubuntu22,bookworm}-final.log`、`/tmp/dogcodes-jpeg-before-witness.log`、`/tmp/dogcodes-hook-before-clean-witness.log`。

所有视频与状态变化均为私有合成环境，未接触用户屏幕/键鼠/剪贴板、未切换日常 Core、未启动模拟器、未调用真实模型；原有三个 Docker dev 服务保留。没有把这些证据当作真实显示器、GPU、手机或弱网验收。

## 接续顺序与完整目标

1. 固定布局的 crop/八种方向、原 node 输入坐标、编码后原授权以及持续坏帧恢复已补，独立 login1 的 desktop 跨模式复验也通过；继续运行中最终尺寸/逻辑几何变化的自动接续与来源重建，不能把正常静止当成来源故障。
2. 继续 Linux 原生编码、Windows 原生采集、GPU 原生调用挂起/设备恢复；每条路径保留独立解码校验与软件接续，逐步降低复制。
3. 已补静态普通刷新预算、实际输出/IDR 确认与软件编解码测量（见 [静态刷新](video-static-refresh-2026-10-25.md)）；继续硬件/文字样本校准，以及网络基线老化、无 ACK/恢复震荡、控制流争用和 ABR/解码消费反馈在 P2P、TURN、WSS 实际链路上的量化。
4. 补色彩/编码能力与平台、手机真机、多屏/DPI/权限/锁屏、弱网/地区网络验收；服务推送凭据与官方部署仍需完成。
5. 原目标中的剩余 PC 功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次持续保留。不能以视频局部改进替代这些交付。

后续进展：[Wayland 呈现与原授权](wayland-presentation-grants-2026-10-25.md)、[持续坏帧与 desktop 跨模式验收](video-corruption-codec-acceptance-2026-10-25.md)。固定布局 crop/八种方向、原 portal 活性、显式坏帧恢复和新私有 login1 跨模式入口已有当前源码证据；运行中最终尺寸/逻辑几何自动接续及其它原范围继续。

最新静态刷新进展及反例：[静态补编码、真实输出与关键帧恢复](video-static-refresh-2026-10-25.md)。传输协议不变，未将合成测量作为全部后端/真实网络验收。

最新 ACK/QoS 进展：[二进制视频基线与恢复](video-relay-qos-2026-10-25.md)。满窗 IDR 重试与实际弱网/消费背压仍继续，未将模块回归视为所有网络场景验收。

最新生产端余量接续：[编码前准入、最新画面与恢复归属](video-output-admission-2026-10-25.md)。满窗 IDR 重试已接入实际入口；码率恢复偏慢/锯齿与实际网络/decoder 量化仍继续。

准入的后续联调已覆盖实际 Source→codec→RelayVideo→累计 ACK：满窗/部分 ACK 无 IDR churn，排空后为最新画面的独立 IDR，同代次 ready 重试不重开恢复。保留型恢复已统一到当前 Output，默认 private launcher 同时执行两项 owned 编码/窗口验收。详见同一准入记录。
