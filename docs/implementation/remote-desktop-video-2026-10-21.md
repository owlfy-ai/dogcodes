# 远程桌面：对照 RustDesk 的改进（2026-10-21）

范围：PC `crates/remote-desktop` + `daemon/remote/native.rs`；手机 `dogcodes-app/lib/features/computers/`。
参考：`~/Workspace/opensource/rustdesk`（只读）。

## 现状与 RustDesk 对照

| 环节 | DogCodes 改动前 | RustDesk | 本次采纳 |
|---|---|---|---|
| 采集 | 每帧 `xcap::Monitor::all()` 重新枚举并重新采集 | `scrap`：长期持有 capturer（mac Quartz/CGDisplayStream），只在有变化时出帧 | 租约内长期持有 `Capturer`；显示器切换或采集失败时重建 |
| 编码 | 每帧 JPEG（`image` crate）+ base64 + JSON | VP8/VP9/AV1/H.264/H.265（vpx/aom/硬件），关键帧 + 增量帧 | **未换编解码器**（见剩余阶段 S3）；画面无变化时不编码（`knownFrameId` → `unchanged`） |
| 传输 | 拉取，最多 5 fps，每帧 base64 JSON | 推流 + `VideoReceived` 回执做流控 | 仍为一次一帧的拉取（天然自限速），上限提到 25 fps |
| QoS | 固定 250 ms 定时器、固定 1600 px/q60 | `video_qos.rs`：以超出基线的延迟判断排队；启动期翻倍；连续 3 次坏样本才降，先降码率后降帧率；1 s 超时立即减半；好样本回升 | 手机端 `FrameRateControl` 按同一规则调 fps 与 JPEG 质量；分辨率随缩放变化（`wantedWidth`） |
| 锁屏检查 | 每帧 2 次、每个输入 1 次启动 `ioreg` 进程（约 28 ms/次） | — | macOS 直接读 `CGSessionCopyCurrentDictionary`（约 1 ms），失败时回退 ioreg |
| 输入 | 每个事件一次 RPC；并发请求可能乱序到达 worker → 触发"序号冲突"终止租约；每次移动重新枚举显示器 | 有序可靠通道，鼠标事件带绝对坐标 | 手机 `RemoteInputQueue`：一次只有一个请求在途、移动合并、滚动累加；PC 接受 `events` 批量（一个序号覆盖一批，全部解析通过才执行，最多 64 条）；坐标映射缓存显示器几何 |
| 手势 | 点按、长按、双指上下滚动；无拖动、无缩放 | `gestures.dart`/`remote_input.dart`：触摸模式下单指拖动=左键拖动，长按=右键，双指缩放/平移画布 | `RemoteGestureRecognizer`：点按=左键（双击位置吸附）、长按=右键、双指轻点=右键、单指按住移动=左键拖动/框选、双指同向移动=滚动、捏合=本地缩放（焦点移动同时平移）；"移动画面"模式下单指平移视图 |
| 缩放 | `BoxFit.contain`，无缩放 | CanvasModel：scale + 平移，焦点不动 | `RemoteViewport`：适应屏幕、1–6 倍、焦点不动、无空边、旋转时保持中心点、拖动时跟随光标 |
| Retina | 归一化坐标映射到 xcap bounds（点），本身正确 | 按显示器缩放换算 | 保持 0..65535 归一化：手机不关心帧的像素尺寸，PC 用点（logical points）换算；帧回传 `pointWidth/scale` 供分辨率决策 |
| 光标 | 截图不含光标，手机看不到 | 单独发送光标位置与形状 | 帧响应带 `cursor`（归一化位置），手机绘制箭头；本地操作后 400 ms 内以本地预测为准 |
| 键盘 | 隐藏输入框；空框退格无效；无修饰键 | 完整键盘映射 | 输入框常驻零宽哨兵字符（空框退格可用）、拼音组字完成后才发送；⌘⌃⌥⇧ 粘滞修饰键作用于下一次按键/点击/拖动；增加 Del、Space、F1–F12、CapsLock 键名 |

## 测量（本机 MacBook，1728×1117 pt @2x，release，`cargo run --release -p dogcodes-remote-desktop --example frame_bench`）

| | 改动前 | 改动后 |
|---|---|---|
| 锁屏检查 | 28.5 ms | 1.0 ms |
| 单帧采集+缩放+编码（1600 px q60） | 87.7 ms | 27.6 ms（变化帧约 33 ms） |
| 静止画面 | 每帧 47 KB | 9/10 帧 `unchanged`，平均 5 KB/帧 |
| PC 端可达帧率上限 | ~11 fps（另受 5 fps 限制） | ~30 fps（网关上限 25 fps） |
| 每个输入事件的 PC 端开销 | ≥28 ms（锁屏检查）+ 显示器枚举 | 一批一次锁屏检查（约 1 ms） |

注：当前终端进程可能没有屏幕录制权限，xcap 采到的可能只有桌面背景，因此 JPEG 体积偏小、静止帧比例偏高；时间数据有效。端到端（手机→中继/直连→PC）的帧率与延迟没有真机测量；手机端状态栏会显示实时 `fps · 往返 ms · KB/s`，可在真机上读取。

## 第三阶段：H.264 视频管线（已实现）

直连或 TURN 能通时（WebRTC 可达），远程桌面页另开一条 `purpose: "desktop"` 的直连。远控 RPC 走这条连接的 DataChannel，所以租约随连接结束；画面走同一 PeerConnection 上的 H.264 视频轨。WebRTC 不通（只剩 WebSocket 中继）时，自动回到 JPEG 拉取。

| 环节 | 实现 | 位置 |
|---|---|---|
| 采集 | ScreenCaptureKit `SCStream`：NV12（`420v`）、由 WindowServer 缩放到编码尺寸（≤1920 宽、偶数）、只在内容变化时回调、`showsCursor` 带真实光标、`queueDepth 6` | `crates/remote-desktop/src/video.rs` `platform::Capture` |
| 编码 | VideoToolbox：`EnableLowLatencyRateControl`、`RealTime`、Constrained Baseline、无 B 帧、`AverageBitRate` + 1 秒 1.5 倍 `DataRateLimits`、`MaxAllowedFrameQP 51`（码率不够时降画质而不是丢帧）、20 s 兜底 IDR；AVCC→Annex-B，关键帧前置 SPS/PPS | `platform::Encoder` |
| 关键帧 | 开始、切换显示器、接收端 PLI/FIR、发送队列满（丢了被引用的帧）、手机首帧超时请求；画面静止时用保留的最后一帧重编码 | `Shared::tick`、`KeyframeRequests` 拦截器 |
| 静态刷新 | 2026-10-25 更新：运动静止 400ms 后有界普通补编码，按实际输出/IDR 确认、FPS/尝试/输出/时间上限收尾；测量与限制见 [静态刷新](video-static-refresh-2026-10-25.md) | `Shared::tick`、`video/refresh.rs` |
| 锁屏 | 采集回调每 200 ms 查一次锁屏状态，锁定或状态未知时不编码；worker 每秒检查，锁屏时结束租约 | `Shared::unlocked` |
| 传输 | 视频轨 + 自己打包 RTP：时间戳取采集时刻（90 kHz），保证"静止 5 秒后的第一帧"时间戳也正确；协商只注册 H.264（42e01f/42001f/4d001f/640c1f，pm=1，level-asymmetry-allowed） | `crates/remote-transport/src/video.rs` |
| 拥塞控制 | TWCC 反馈 → GCC 估计（`rtc-interceptor`），以 2.5 倍目标码率节奏发包（同 libwebrtc），NACK 重传，RTCP SR/RR；GCC 估计值发布给编码器 | `video::engine`、`PublishedGcc` |
| 码率策略 | 编码码率 = 估计 × 0.85，并受画面尺寸上限约束；估计 <600 kbit/s 时 12 fps，<1.2 Mbit/s 时 20 fps，否则 30 fps | `daemon/remote/video.rs` `rate_policy` |
| 守护进程 | `direct_open(purpose=desktop)` 用 `answer_with_video`，应答带 `video: true/false`；`remote_desktop_open {video:true, videoMaxWidth}` 启动视频会话并在响应里返回 `video{codec,width,height}` 或 `videoError`；新增 `remote_desktop_video {leaseId, keyframe}` 保活并返回统计 | `remote/direct.rs`、`remote/native.rs`、`remote/video.rs` |
| 手机 | `connectDesktopRtc`：只收视频的 transceiver（不需要摄像头/麦克风权限）+ DataChannel；`RTCVideoView` 放进 `RemoteViewport` 计算的矩形，缩放、平移、手势和坐标映射与 JPEG 模式完全相同；每秒保活，首帧 2 秒未到就请求关键帧；用 `getStats()` 在状态栏显示 `H.264 · fps · RTT · KB/s`；任何一步失败都回到 JPEG 并给出提示 | `lib/core/rtc_transport.dart`、`remote_desktop_page.dart`、`video_stats.dart` |

### 测量（本机，负载均值约 10）

`cargo run --release -p dogcodes-remote-desktop --example video_bench -- --synthetic`：合成桌面画面，因为当前终端没有屏幕录制权限，ScreenCaptureKit 返回 TCC 拒绝。

| | 结果 |
|---|---|
| 编码延迟（1920×1240，30 fps，4 Mbit/s） | 平均 8.5 ms，p95 9.5 ms |
| 码率命中 | 目标 4000 → 实际 3995 kbit/s；目标 1000 → 1002 kbit/s |
| 输出 | Constrained Baseline、无 B 帧，`ffprobe`/`ffmpeg` 解码无错误 |
| `MaxAllowedFrameQP 51` 的作用 | 2 Mbit/s 下 90 帧输出 86 帧（之前 48 帧）；1 Mbit/s 下 60 帧输出 57 帧（之前 33 帧） |

`cargo run --release -p dogcodes-remote-transport --example video_loopback`：真实 VideoToolbox 编码 → 视频轨 → 用 libwebrtc 默认编解码器和反馈配置的接收端，回环 10 秒。

| | 结果 |
|---|---|
| 采集→接收端收齐一帧 | p50 约 9 ms，p95 约 11 ms（之前 JPEG：PC 端单帧 28 ms，加 base64/JSON 和一次往返） |
| 节奏器修正 | 2.5 倍节奏前是 p50 127 ms、p95 417 ms（关键帧在 1 倍目标码率后面排队） |
| GCC | 10 秒内 2.0 → 约 4.5 Mbit/s 平滑上升，没有丢包 |
| 最坏内容（整窗文本每帧滚动 8 px） | 1.7–3.9 Mbit/s 下编码器输出约 65% 的帧；发送队列从未拒绝；码率不足时由编码器取舍 |
| 收到的码流 | 拼回 Annex-B 后 `ffmpeg` 解码无错误 |

没有真机测量：手机硬解、实际网络、中继 TURN 下的帧率和延迟都需要在 iPhone/Android 上读状态栏。

## 第四阶段：仅中继时的视频（已实现）

WebRTC 完全不通（只剩 WebSocket 中继）时，同一个 H.264 码流改走已认证连接本身的二进制消息，手机用平台硬件解码器解码到 Flutter Texture。

| 环节 | 实现 | 位置 |
|---|---|---|
| 二进制帧 | 长度字最高位置 1 表示二进制：`[u32 长度|0x80000000][u32 头长度][头 JSON][原始字节]`；JSON 帧上限 8 MB，不会用到这一位；PC 只为明确声明 `videoTransport: "binary"` 的手机发送，所以旧手机永远收不到 | PC `remote.rs` `binary_frame`/`send_framed`；手机 `lib/core/frame_reader.dart` |
| 手机缓冲 | `SecureHostClient` 原来的 `List<int>` 缓冲（逐字节 `addAll`/`removeRange`）换成可增长的 `Uint8List`，从前端消费，一个大消息只复制一次进、一次出 | `FrameReader` |
| PC 发送 | `RelayVideo`：帧编号后作为 `{"type":"video","leaseId","seq","key"}` 二进制消息推送；未确认的帧最多 8 个且不超过约 0.4 秒的数据，超出就丢弃并等下一个 IDR（同时请求 IDR）；连接的二进制通道容量为 8，满了也按丢帧处理 | `daemon/remote/video.rs` |
| 流控/码率 | `RelayQos`（RustDesk 的视频回执做法）：以确认延迟超出最近最小值的部分判断排队；10 次干净确认 +8%，连续 3 次超出 150 ms ×0.75，超出 1 秒 ×0.5，两次下调至少间隔 500 ms，范围 0.2–8 Mbit/s；2 秒没有确认按卡顿计。码率经同一个 `rate_policy` 决定编码码率和帧率 | `RelayQos` |
| 手机接收 | `RelayVideoReceiver`：按顺序逐帧交给解码器，累计确认（最多一个在途，≥100 ms 合并一次）；解码出错或积压超过 6 帧时跳过增量帧（仍确认，PC 窗口照常前移）直到关键帧，并随确认请求 IDR（最多每 500 ms 一次） | `lib/features/computers/relay_video.dart` |
| 平台解码 | 本地插件 `packages/dogcodes_video`（SwiftPM + Kotlin）：iOS 用 VideoToolbox 异步解码出 BGRA IOSurface 给 `FlutterTexture`；Android 用 MediaCodec 异步模式，csd-0/csd-1 取自关键帧，Android 11+ 开 `KEY_LOW_LATENCY`，输出直接渲染到 `SurfaceTexture`；参数集变化时重建会话，后台失效时要求关键帧 | `packages/dogcodes_video` |
| 页面 | 没有直连且手机是 iOS/Android 时，`remote_desktop_open` 带 `video: true, videoTransport: "binary"`；响应 `video.transport == "binary"` 时用 `Texture` 显示，缩放、手势和坐标与其他模式相同；状态栏显示 `H.264 中继 · fps · 确认往返 ms · KB/s`；平台没有解码器时关闭租约、不带视频重开，回到 JPEG | `remote_desktop_page.dart` |

### 验证与测量

- PC：`binary_frame` 帧格式、`RelayVideo` 窗口和"丢帧后等 IDR"、`RelayQos` 升降速各有单元测试（`cargo test -p desktop` 共 269 个）。
- 手机：`FrameReader`（任意分块下 JSON 和 300 KB 二进制交错、缓冲复用、非法长度）、`RelayVideoReceiver`（顺序、累计合并确认、错误后跳帧和请求 IDR、积压丢弃）、页面级测试（模拟平台解码器：按顺序解码、确认到最新序号、显示 Texture、状态栏"H.264 中继"）。
- **真实解码器**：`integration_test/h264_decoder_test.dart` 用 PC 端 VideoToolbox 编出的 480×320、20 帧码流（`video_bench -- --sample`）在 **iOS 模拟器**（iPhone 18 Pro，iOS 27）和 **Android 模拟器**（API 37）上都通过：解码出至少 18 帧；从增量帧开始会抛出 `NeedKeyframe`；关键帧之后恢复。
- 带宽（`video_bench -- --compare`，同一段 1920×1240、30 fps 的桌面动画）：JPEG q60 每秒 5419 KB（旧的 base64 JSON 路径 7226 KB），H.264 每秒 287 KB，约为后者的 1/25。

没有在真实中继上测：真实 TURN 不可用、只能走中继时，手机上的帧率、确认往返和码率收敛需要真机读状态栏。

## 剩余阶段

- **真机验收（最重要）**：给 DogCodes 授权屏幕录制后在 iPhone/Android 上测 `desktop` 直连、首帧时间、静止/滚动/视频播放三种画面的 fps 与码率、TURN 下的表现；确认 libwebrtc 的 offer（RTX、扩展头）与 PC 端协商结果，以及 Android 设备都提供 H.264 硬解（不提供时 PC 会应答无视频，手机回到 JPEG）。
- **中继视频真机验收**：真实中继服务的限速、确认往返对 `RelayQos` 的影响、手机解码器在后台/前台切换后的恢复。
- **独立远控连接的无缝换路**：2026-10-22 已隔离普通中继远控和主业务连接，主业务升级不再关闭远控租约；远控当前稳定保留自己的中继连接，远控自身中继→P2P 的无缝接续仍待补齐。
- **JPEG 回退本身仍是 base64 + JSON**：二进制帧已就绪，JPEG 帧也可以改走它（少 1/4 流量），只是视频覆盖了绝大多数情况，优先级低。
- **JPEG 回退的采集与输入分线程**：视频模式下采集在 ScreenCaptureKit 队列上，不再阻塞输入；JPEG 回退时两者仍共用 native worker，输入最多要等一帧（约 25–35 ms）。
- **按缩放调整编码分辨率**：视频分辨率在打开时按手机屏幕决定（≤1920），放大很多时不会提高分辨率；可以用 `SCStream.updateConfiguration` 加编码器重建（会插一个 IDR）来实现。
- **Windows/Linux 视频**：Windows 与 Linux X11 的有界采集及 OpenH264 软件回退已补齐（见文末）。Windows DXGI/Media Foundation、Linux PipeWire/VA-API 等加速后端、Wayland 持续 ScreenCast 会话仍待实现；xcap 在 Windows 上的坐标单位需要实机核实。
- **光标**：macOS 视频内含真实光标；Windows/X11 软件视频与 JPEG 仍画通用箭头。软件视频已通过保活回复更新有控制权限的光标位置（约 1 Hz）；只读软件视频的独立原生光标读取、光标形状与高频独立更新仍待实现。
- 手势手感：滚动步长 18 px/行、长按 500 ms、捏合阈值 24 px 需要真机调整。

## 2026-10-22 可靠性补齐

- `RelayVideo` 允许不超过 4 MiB 的单个关键帧越过常规字节窗口，但必须是空窗口；增量帧继续受到原有窗口限制，防止大 IDR 被无限拒绝。切屏先停止旧编码线程，再清空窗口、增加视频代次，旧排队消息不会显示成新屏幕。
- `VideoStream::start` 在声明可用前初始化编码器；初始化失败返回 JPEG 回退原因。运行编码失败也保留错误，通过视频统计返回。手机基于原生解码帧数进行首帧和停滞检测，失败重开 JPEG，静止画面没有新帧时不误报停滞。
- 手机普通中继与 WebRTC 均使用独立远控连接；主业务的换路不会退役它。前后台处理、新租约连接恢复、TURN 到期前重连、迟到的 native dial 资源释放及停止后的输入队列丢弃已接入。不自动重放键鼠。
- 手机累计确认仅在 RPC 成功后推进，失败保留累计高水位并退避重试；不依赖下一帧触发。切屏用 PC 新尺寸重建手机解码器和显示比例。保活、收包、平台解码统计仍需真实弱网及长时测试。
- 当前代码证据：PC 视频相关 5 项测试、PC 全量库测试 273 项通过/7 项跳过、手机全量 224 项测试通过。iOS/Android 本机构建和严格 Clippy 通过；不代替真实 TURN/手机/跨平台 PC 验收。
- 当时未完成：Windows/Linux H.264（Windows/X11 软件回退现已在下节补齐）、远控自身无缝换路、分片调度的真实弱网验证和窗口自适应、缩放驱动分辨率、完整真实设备和大陆/海外链路验收。

## 2026-10-22 原生解码与切屏补齐

- iOS 和 Android 以帧 ticket/PTS 绑定解码完成回调，提交与在途帧总数上限 3；专属工作线程负责 codec 创建、重建、送帧和停止，UI 线程处理 Flutter texture 和完成通知。旧 codec 输出/错误不影响替换实例。释放或 3 秒超时结束 Dart 等待，避免阻塞消费和遗留计时器。
- 成功消费在实际解码输出后才确认；失败帧和依赖它的丢弃帧仍需确认释放窗口，同时请求 IDR，因此 ACK 不是“所有帧都已显示”的承诺。
- 切屏暂停输入、清理本机修饰键和输入法；PC 替换输入控制器以释放原先按住的键/鼠标。JPEG 拒绝旧屏代次的响应和错误；WebRTC 用新 peer/租约清空旧 RTP 缓冲，中继重建解码器并过滤视频代次。Renderer.dispose 出错仍继续关闭其它连接。
- 两平台各 2 项独立模拟器原生测试通过：PC 编码的 20 帧样本全部输出，队列上限及等待帧释放。测试使用独立新模拟器/临时 AVD，不覆盖日常设备数据。PC 273 项库测试通过、7 项跳过，手机 224 项测试通过。实际采屏、键鼠、多设备弱网与长时压测依然需要实机验证。


## 2026-10-22 H.264 协商边界

- 移除发送端实际不输出的 Main/High 声明；WebRTC 当前提供 Baseline/Constrained Baseline 3.1。检查原始 offer 的接收等级，低等级、缺失明确等级或不支持 packetization-mode=1 时使用 JPEG，不依赖底层库忽略等级的匹配行为。
- [RFC 6184 §8.2.2](https://www.rfc-editor.org/rfc/rfc6184.html#section-8.2.2) 的等级非对称允许两个方向等级不同，不能超过对端接收能力。WebRTC 采集按完整宏块预算缩放（每帧不超过 3,600 宏块、30 fps、平均 9 Mb/s 上限），同时限制极端显示比例；中继仍保留自动等级。
- VideoToolbox 只有 Constrained Baseline 自动等级，固定 Baseline 3.1 实测不保证 Constrained Baseline。因而同时校验实际 SPS 的子集/等级，越界或参数缺失不发送错误码流，而是通过编码错误触发 JPEG 回退。编码回调和读取参数失败也能被手机发现。
- 新验证：remote-desktop 8 项（包含真实硬件编码的 SPS 与极端比例）、remote-transport 6 项通过/1 项 TURN 外部环境测试跳过；标准 codec、仅 Baseline 接收端实际收帧，以及低等级接收端回退均通过。desktop 库 273 项通过/7 项跳过，严格 Clippy 和 video_bench 示例编译通过。上述不代替真实屏幕、手机与公网网络验收。
- 后续更高分辨率应扩展为双方能力驱动的等级/尺寸/帧率协商，再创建编码器，不能只提高 SDP 中的等级。


## 2026-10-22 中继分片、控制优先与取消

- 已认证连接拆为独立读写任务，TCP 发送背压不再阻塞接收输入、续租、撤销检查。Noise 的加解密状态仅在同步加解密时加锁，不跨网络 await。
- 手机打开远控时声明 `chunkedMessages: true`；通用客户端也可通过 `transport_options({fragmentedMessages:true})` 协商。旧客户端仍接收旧格式；服务器继续只转发密文，不解析桌面内容。
- 大消息拆为独立认证的 8 KiB 片段，二进制头为 `type: fragment`、32 个十六进制字符的 `id`、`offset`、`total`，内容是原始完整长度前缀消息的一部分。短控制回复优先于大回复，大回复优先于视频。
- 接收端通过不占 RPC 待回复槽的 `fragment_ack` 累计确认每个消息的连续偏移。发送端最多保留 32 KiB 未确认大块数据，避免一次把大关键帧灌入系统缓冲；短控制回复不消耗此窗口。这里限制的是未确认字节，不是累计传输流量。解码完成后的帧 ACK 仍独立保留。
- 手机最多同时重组两个消息（一个视频、一个大回复），每条逻辑消息仍沿用 8 MiB 上限；验证偏移、总长、标识和禁止嵌套，连接关闭释放未完成缓冲。PC 队列和确认元数据也有界，拒绝伪造超过已发送偏移的确认。
- 视频租约/显示器使用连接级单调代次。切屏或关闭会取消尚未传完的旧视频，并发 `fragment_cancel` 让手机释放残片；旧队列帧不再占用新屏带宽，迟到的旧确认不能恢复它。
- 验证：真实 TCP 发送阻塞期间接收模拟松键消息；控制回复穿过未完成的大帧/大回复；窗口耗尽仍可回复输入；取消旧帧回收窗口；非法/重复分片和关闭清理。PC 库 280 项通过/7 项跳过，手机 231 项通过，严格 Clippy、静态分析及两端普通应用构建通过。真实加密 Core 网关的大响应分片及原有配对/审批/流式/操作恢复链路通过，未调用真实模型或真实键鼠。
- 首帧/切屏就绪握手已在下节补齐；大体积剪贴板上行应独立于输入连接或使用双向分片，避免上行完整请求序列化影响确认。真实弱网/设备及跨平台视频后端范围保持不变。


分片确认是普通长度前缀 JSON，使用同一已认证 Noise 连接和发送序列，不创建 `requestId`/待回复槽：

```json
{"type":"fragment_ack","version":1,"hostId":"<paired-host>","acks":[{"id":"<32-hex-characters>","offset":8192}]}
```

隔离网关追加长传输验证：`DOGCODES_TEST_LONG_STREAM=1` 连续读取 200 次约 384 KB 的多字节文本，单连接累计超过 64 MiB，45.4 秒完成且后续原有业务验证全部通过。确认窗口正常循环释放，不会演变为累计流量断开限制；测试数据库与 PC/Git 目录均为独立临时实例，完成后已清理。


## 2026-10-22 中继解码器就绪握手

- 新手机打开视频声明 `deferVideo: true`。PC 初始化/验证编码器但暂不发送中继视频，`video.clientReadyRequired` 和 `generation` 指明当前握手；旧手机不声明该能力，保持原有行为。
- 手机先创建解码器、安装按租约/代次筛选的监听，再发送 `remote_desktop_video` 的 `ready: true` 与原 `generation`。PC 仅激活匹配的代次，并从 IDR 恢复；重复就绪不清掉已经使用的帧窗口。确认回复前到达的第一帧也已有接收者。
- 切屏重置为未就绪，迟到的旧解码器不能激活新屏。手机验证 PC 的当前代次/ready 状态，不将“RPC 有回复”当成“当前画面已就绪”。WebRTC 的接收器由 SDP/媒体轨创建，本握手只用于二进制中继解码器。
- 创建解码器最多等待 3 秒，超时回退 JPEG；页面退出/替换或超时后才创建出的 texture 仍被释放，不激活 PC。就绪 RPC 使用 5 秒回复期限，未知结果按原租约关闭并恢复连接，不重放输入。
- 证据：PC 门控/大关键帧/旧代次/重复就绪用例，手机初次及切屏监听先于第一帧、离页迟到纹理及创建超时用例。当前 PC 库 281 项通过/7 项跳过，手机 243 项通过，严格检查和 iOS/Android 普通应用构建通过。实际屏幕/手机硬件就绪时间仍需实机验证。


## 2026-10-22 Windows/X11 软件视频回退与停止栅栏

### 实现边界

- `video/software.rs`：Windows 和 Linux X11 使用长期持有的 xcap Monitor，独立线程按目标帧率采集，缩放后比较内容，不使用 xcap 无界 recorder 队列。显示器配置变化/采集失败保留明确错误。Windows 原生 Monitor 不跨线程移动，在采集线程内构造。
- OpenH264 从 Cargo 依赖源码构建，无运行时下载；输出 Constrained Baseline 3.1、无 B 帧的 Annex-B。所有传输路径使用相同的软件预算（≤3600 宏块/帧、≤30 fps、平均码率≤9 Mbps），实际尺寸按屏幕比例缩小，不宣称已实现高分辨率原生加速。
- 启动先实际初始化编码器并验证 SPS，私有 warmup 不对外发布；首个屏幕帧强制新 IDR。关键帧请求、静止画面刷新和真实运行时码率/帧率调整可用；编码器丢帧不误记成成功输出，也不丢掉静止画面的关键帧请求。
- Wayland 明确拒绝以截图 portal 轮询冒充持续视频，暂保留既有 JPEG 路径及其平台限制。持续 ScreenCast/RemoteDesktop portal、restore token、PipeWire 视频、原生硬件加速仍是代码待办，不属于这次软件回退完成范围。
- 采集启动有期限，停止不在输入线程 join 阻塞中的 OS 调用。独立原生视频 worker 和截图 worker 各最多 2 个（当前及退出中的旧任务），重复重连不能无限增加卡住的原生线程。
- 关闭/启动超时先关闭发布栅栏，切屏随后 reset 传输代次；旧采集或编码即使稍后完成，也无法把旧屏幕内容标成新代次。清理在资源所属线程继续，输入和关闭不等待它。
- 描述新增 `cursorEmbedded`。macOS 视频内嵌光标，手机仅显示 400 ms 的本地预测并主动移除，避免永久双箭头；软件视频/JPEG 保留独立箭头，软件视频保活回复带有控制权限的 PC 光标位置。只读软件视频的指针读取与原生形状仍待补齐。

### 本地证据与验收边界

- macOS 原生库 18 项通过，覆盖真实软件编码/解码、强制 IDR、读取原生 codec 参数确认动态码率/帧率、启动超时的线程上限、静止抑制及停止后的迟到帧。没有采集用户桌面。
- Windows GNU 目标 `cargo check -p dogcodes-remote-desktop --target x86_64-pc-windows-gnu --locked` 通过，包括原生 C++ 编码器；不等同 MSVC 安装包或 Windows 运行验收。
- Linux ARM64 隔离 Docker 普通原生库测试 16 项通过/1 项 opt-in 跳过；独立 Xvfb opt-in 用例通过：实际 1024×768 桌面采集、改变背景、软件编码并解码两个变化帧。Linux all-targets 严格 Clippy 通过。
- 桌面 CI 安装 `libgbm-dev`、NASM 与测试用 Xvfb，执行相同隔离采集用例。Xvfb 使用 `-noreset`，避免最后一个客户端关闭后虚拟桌面自动重置；没有对用户显示器或输入做操作。
- 同一软件编码器生成的 20 帧合成桌面，独立 FFmpeg 解码成功；iOS 27 和 Android API 37 隔离模拟器各 4 项原生插件测试通过，分别覆盖 VideoToolbox 与 OpenH264 输出、拒绝从增量帧开始、IDR 恢复、有界入队和退出收尾。模拟器通过不代表手机真机硬件解码已验收。
- PC 最终完整库 290 项通过/7 项跳过（降低测试并发后，首次并行全量中 5 项进程/Git 超时未复现）；远程模块 52 项通过。手机光标协议及页面计时器 15 项针对测试通过，含原生身份/加密 FFI 的完整普通测试 271 项通过。PC/native all-targets 严格 Clippy、Flutter analyze 与正常入口的 iOS 模拟器/Android ARM64 调试构建均通过。
- 本轮不改服务端协议，不重启日常 PC Core，不提交或推送。用户已确认先做代码/本地验证，Windows、X11/Wayland 和 iPhone/Android 实机联调后续安排；原生加速、Wayland、无缝换路、缩放分辨率和其余操作恢复审计保持开放。


## Wayland 后续（2026-10-22）

共享 RemoteDesktop/ScreenCast 授权、恢复令牌、受限 FD 的持续 PipeWire RGB 采集、JPEG/H.264 与键鼠现已接入；详情与验收边界见 [Wayland 实现说明](wayland-remote-desktop-2026-10-22.md)。隔离假门户 + 真实 PipeWire 合成视频的授权 → 采集 → 编码 → 解码及取消/撤销测试通过，不等同真实 compositor、中文输入或系统授权已验收。本文此前“Wayland 持续会话待实现”的记录为当时进度；硬件加速、仅 DMA-BUF 来源及其它后续范围仍开放。

## 视频缩放分辨率（2026-10-22 后续）

已补手机缩放/布局驱动的分辨率请求、PC 视频线程异步重建、发布代次栅栏、relay 解码器 ready/首帧交接和失败回退。尺寸遵守协商上限，调整不阻塞键鼠。详情和本地证据见 [视频分辨率实现说明](video-resolution-2026-10-22.md)；更高 codec level/硬件上限与跨设备实测仍开放，本文此前的待实现记录对应当时进度。

## 换路基础（2026-10-22 后续）

PC 已补同一手机的新认证连接接续二进制视频输出和控制租约、版本/序号栅栏，以及可切换的 Wayland 来源活性。手机输入队列增加可取消的迁移屏障。后续已接入手机二进制 WSS→P2P 迁移、首帧交接、未知结果核实/栅栏与二进制凭据续期；随后已补同编码预算 RTP→RTP 原位迁移、手机续期及实际首帧交接；随后补齐二进制/RTP 的编码预算重建接续及手机旧纹理保留；更高 RTP 等级协商、真实 TURN 续期/弱网验收仍开放。契约和接续步骤见 [物理连接迁移说明](desktop-handoff-2026-10-22.md)。


## 2026-10-27：图片回退沿用实际光标来源

发现 Wayland Portal 画面已由 compositor 内嵌系统光标，但 JPEG 响应没有暴露该事实。手机切到图片模式后默认保留独立箭头，在操作后可能与画面中的系统光标重叠并持续残留；不能从 H.264/JPEG 编码类型推断光标是否内嵌。

`Capturer::frame` 现在在完整 JPEG 和 `unchanged` 回复都携带 `cursorEmbedded`：Portal 使用原授权 ScreenSource 的实际值，原生 xcap 截图为 false。手机在每次图片回复中采用这一元数据，来源改变时清理旧叠加和预测；内嵌来源仍只显示 400 ms 本地操作预测，随后由既有定时器移除。没有新 RPC、权限请求或独立光标服务。此前缺字段默认当 macOS 视频处理的旧版判断已删除，符合尚未发布、不做兼容的约定。

验证：修改前新增手机用例实际出现重复箭头，两个状态用例及一个真实页面用例失败；私有 Wayland/PipeWire 验证在 JPEG 的 `cursorEmbedded` 断言得到 Null。修复后，完整 JPEG 与相同画面的无 bytes 回复均返回 true，原授权结束与锁屏仍拒绝返回图像。手机 433 项普通测试通过，9 项 FFI 按需测试未启用；三文件 analyze 无问题。macOS 原生库 101 项、Linux 原生库 142 项及独立 Portal 专项通过，两平台严格 Clippy、Windows GNU all-targets 静态严格检查通过。没有访问用户屏幕、键鼠、剪贴板、安装 Agent 或日常 Core。

日志 `/tmp/dogcodes-jpeg-cursor-before-flutter.log`、`/tmp/dogcodes-jpeg-cursor-before-linux.log`、`/tmp/dogcodes-jpeg-cursor-after-linux.log`、`/tmp/dogcodes-jpeg-cursor-macos.log`、`/tmp/dogcodes-jpeg-cursor-all-flutter.log`、`/tmp/dogcodes-jpeg-cursor-analyze.log` 及两端严格/Windows 静态日志。原生光标形状、Windows 个别采集源的内嵌判定、只读软件视频位置和真实设备验收仍保留；本次没有据元数据修复宣称已完成原生光标传输。
