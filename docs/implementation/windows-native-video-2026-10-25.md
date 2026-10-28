# Windows 原生视频编码接入（2026-10-25）

Windows 视频从仅 OpenH264 软件编码扩展为硬件 Media Foundation 优先、OpenH264 接管，二者都不行时沿用已有 JPEG 降级。系统截图/CPU 缩放仍复用现有来源，没有将本阶段当作 D3D11 零拷贝采集、Linux 加速或完整远控目标完成。

## 实现边界

- 只枚举 hardware MFT 的 NV12 → H.264 候选，最多尝试 8 个。COM 初始化、异步解锁、格式设置、NeedInput/HaveOutput 事件、处理和关闭全部在同一个独立工作线程，不跨线程传递 COM 接口。
- 基于 [Microsoft 异步 MFT 模型](https://learn.microsoft.com/en-us/windows/win32/medfound/asynchronous-mfts) 驱动输入/输出；不把硬件 MFT 当作同步 OpenH264 调用。事件等待有期限，流变化只重设输出后重试同一输出，不重复消费输入/事件凭据。
- 当前原生编码也限定 Constrained Baseline 3.1、30 fps、目标码率至多 9 Mb/s；不借接入硬件扩大手机既有协商预算。设置 baseline/level 之后还核实实际 SPS，并用源构建 OpenH264 解码合成首帧，确认尺寸后才就绪。低延迟/B 帧数属性按 [官方 H.264 属性支持范围](https://learn.microsoft.com/en-ca/windows/win32/medfound/h-264-video-encoder) 尝试；实际单帧输出期限、时间戳和解码验证是最终准入依据。
- 预热合成图不发送给手机。首次真实输入、帧率变化重建之后的首次输入都强制自己的 IDR，不能让手机收到引用预热图的 P 帧。码率修改通过原生属性确认；不支持或出错就接管为软件编码。
- NV12 使用现有 RGBA → BT.601 YUV 转换，按 native stream info 申请有界、对齐的系统内存；样本时间戳须与该输入相符。原生 sample/events、锁定映射、activation 及运行时使用 RAII，失败的接口查询/配置和正常结束都收尾。
- [MF 序列头](https://learn.microsoft.com/en-us/windows/win32/medfound/mf-mt-mpeg-sequence-header-attribute)与样本按 NAL 处理；缓存 SPS/PPS，每个实际 IDR 携带四字节起始码的参数集。单帧最多 8 MiB、参数集/序列头及 NAL 数量有界；这些是单条内存预算，不是累计传输阈值，不恢复 64 MiB 断线设计。

## 有界失败与接管

`video/worker.rs` 的 channel 至多一个请求，全进程最多两个 native codec worker。初始化和处理调用方最长各等待 2 秒；MFT 事件循环自身通常在 800 ms 内结算。挂起 OS 调用不阻塞调用方无限等待，线程名额保持占用直到真正退出，不能通过反复重连无限创建线程。

worker 只返回独立的 EncodedFrame，从来不持有网络/显示 sink。等待超时后丢弃原回复、停用原 worker，软件编码器重新预热并输出自己的 SPS/PPS/IDR；迟到硬件结果即使返回也不能进入新画面。软件接管是当前 stream 内的单向切换，不反复探测 GPU。原生不可用但软件就绪时不返回伪造的视频失败，软件也失败才进入原图片回退。

Windows 在编码返回后、实际发布前重新检查锁屏与停止状态；不因硬件处理延迟而沿用编码开始前的检查。合成编码测试绕过 VideoStream，不读取用户桌面状态。

MF 入口只从固定 System32 mfplat.dll 动态解析，未增加启动时的 MF DLL 导入，缺组件可以在运行时选择软件路径。应用不改注册表、不下载安装系统组件。两平台原有编码和采集权限保持；P2P/TURN RTP 与独立二进制中继仍共用 Annex-B sink，无协议/手机/服务端生产改动。

## 验证证据

新增 8 项普通回归：

1. 不能 Send 的 native 状态在同一线程创建、使用、释放。
2. 处理超时后关闭不 join 挂起的 OS 调用，迟到结果被丢弃。
3. 初始化等待放弃后仍保留 worker 名额，两项挂起期间第三项不能再启动线程。
4. 三字节参数集/四字节长度样本规范化，按实际 IDR 补参数集。
5. 缺参数、截断长度及超出协商的 SPS 被拒绝。
6. 初始化失败直接走真实 OpenH264，输出可实际解码的 IDR。
7. 模拟硬件先成功后失败，两套真实编码器切换后的首帧均为 IDR，同一解码器可持续解码，后续动态码率/帧率仍工作。
8. 模拟 OS 编码超时后软件接管，原线程返回的旧帧不能再发布。

macOS remote-desktop 完整 **27 项通过**，Core **92 项**、桌面 **320 项**通过（7 项桌面按需用例跳过），两 crate 的 all-targets 严格 Clippy 通过。

Windows GNU all-targets 严格 Clippy 通过，video_bench 已实际编译并链接为 PE，可执行文件未运行。导入表检查不含 mf.dll/mfplat.dll/mfreadwrite.dll；这证明没有新增启动时 MF 硬依赖，不等于已经在 Windows N 实机启动通过，也不是 MSVC 安装包验收。新增一项 Windows 真实硬件编码测试默认忽略，需显式启用，输入完全为合成图：

```powershell
$env:DOGCODES_MF_ENCODER_FIXTURE = "synthetic-only"
cargo test -p dogcodes-remote-desktop isolated_hardware_encoder -- --ignored --test-threads=1 --nocapture
```

该用例直接要求 hardware MFT，不允许软件冒充通过；测试重复 IDR、动态码率和帧率重建后的独立 IDR及实际解码。Windows 普通测试不尝试真实 MFT。当前未执行此用例，没有运行已安装 Agent、模型、用户屏幕/键鼠/剪贴板或模拟器，也未切换日常 Core。

隔离 Linux remote-desktop **31 项通过**、2 项已有实机/独立采集按需测试跳过；all-targets 严格 Clippy 通过。使用独立带 init 的测试容器；首次环境缺 C++ 编译器的构建失败，在补 g++ 后复验，不作为通过记录。现有三个 server/postgres/redis dev 服务不变。

手机 iOS/Android decoder 的源码已有 SPS/PPS 内容变化时重建解码器的处理；本轮仅核对，没有运行原生手机解码。模拟硬件/软件的本地解码证据不能替代真实 MF → 手机的链路验收。

## 继续范围

Windows 各显卡/驱动的实际 MFT 支持、设备丢失/挂起、Windows N 与 MSVC 包、系统采集/硬件编码/手机硬解性能仍待验收。仅提供必须使用 D3D manager/纹理输入、或需要多帧填充才输出的实现可能回退软件；D3D11 采集/转换零拷贝、GPU 枚举与更完整的设备恢复继续。挂起线程有界不等于已经强制终止内核中的驱动调用。

Linux 原生硬件编码、Wayland DMA-BUF/NV12-only 来源、HDR/色彩及更多光标元数据、更高 codec/profile/level 协商仍未完成；原多平台/手机真机、真实 TURN 续期与弱网/地区网络、通知凭据/官方部署完整范围保留。PC 功能/操作恢复、原生 Agent 辅助事务/超时副作用、Core 强退及跨物理代次也继续，不以本阶段收口替代原目标。

后续采集进展：[Windows Desktop Duplication 与有界 GDI（2026-10-26）](windows-desktop-duplication-2026-10-26.md)。本记录的原生编码实现保留；新采集路径已接入相同编码与传输，真实 GPU/MSVC/驱动验收及 GPU 直达编码仍待完成。

## 2026-10-27：直接写入 MF 输入缓冲

原路径先把已有 I420 平面打包进持久 NV12 Vec，再整帧复制到 MF 的系统内存输入缓冲。现在在原编码线程锁定新分配的输入缓冲后，直接写入 Y 平面和交错 UV，随后按原路径解锁、设置有效长度并附到 sample。720p 每个编码器减少 1,382,400 字节（约 1.32 MiB）的持久暂存，以及每帧一次同等大小的复制；没有改变颜色转换、输入时间戳、异步凭据或输出处理。这仍有 CPU 转换/打包及 GPU 上传，不是 GPU 零拷贝，也不据此宣称整条视频的性能提升。

复用 OpenH264 的紧密 YUVBuffer，新增内部 `video/nv12.rs`；尺寸、算术及目标长度检查在写入前完成，不引入通用像素抽象。映射只属于尚未交给 sample 的新缓冲，在同一原生线程使用，指针不逃逸。依据 [NV12 布局](https://learn.microsoft.com/en-us/windows/win32/medfound/recommended-8-bit-yuv-formats-for-video-rendering)及 [IMFMediaBuffer::Lock](https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfmediabuffer-lock)，沿用原有配对 Unlock 和 SetCurrentLength；Lock 本身不作为跨线程同步。

两项默认回归验证已知 I420 像素的 Y/UV 顺序、分配尾部及源数据不变，以及短缓冲/空源在任何写入前拒绝。macOS 两项均通过、原生 all-targets 严格 Clippy 通过；隔离 Linux 原生 162 项通过、11 项按需跳过，严格 Clippy 通过；Windows GNU 原生与 desktop 两包 all-targets 严格 Clippy 通过，未运行 Windows MFT。证据分别为 `/tmp/dogcodes-mf-direct-input-layout-macos.log`、`/tmp/dogcodes-mf-direct-input-strict-macos.log`、`/tmp/dogcodes-mf-direct-input-linux.log` 和 `/tmp/dogcodes-mf-direct-input-windows.log`。

本轮 macOS 完整原生测试没有通过：首个既有 VideoToolbox 初始化失败导致共享测试锁中毒，随后用例连带失败（93 通过、22 失败、1 跳过）。单独复核仍在原初始化期限内未获确认；独立合成编码测试临时定位到 `VTCompressionSession::create` 进入后未返回，5.01 秒由原保护报错。临时诊断已移除，没有延长期限、重启日常 Core 或操作系统媒体服务；当前证据不足以确定系统侧根因。Windows 生产改动不进入 macOS 编码路径，但不能把本轮 macOS 全套记录为通过。诊断日志为 `/tmp/dogcodes-mf-direct-input-macos.log`、`/tmp/dogcodes-mf-direct-input-macos-recheck.log` 和 `/tmp/dogcodes-mf-direct-input-vt-diagnostic.log`；恢复原生创建后的复验仍待完成。
