# Windows Desktop Duplication 与有界 GDI 回退

参考本机 RustDesk HEAD `53996e3c4` 的 `libs/scrap/src/dxgi/mod.rs`、`src/server/video_service.rs::DxgiRecoveryState`：区别没有新帧与接口失效、有限重建、同一屏幕回退，实际释放后才能归还资源。本项目独立实现 D3D11/DXGI 绑定、恢复控制器及测试，没有复制代码或协议。

## 接入

- Windows 从 GDI 逐帧截图扩展为 Desktop Duplication 优先，仍向现有 CPU 缩放/MF 或 OpenH264 编码链交付独立 RGBA 图；不是 GPU 纹理直接进入编码器。
- 枚举有界，按现有显示器句柄 ID 和系统设备名双重核实输出，在该输出所属 adapter 上创建 D3D11 device。复用一张 staging texture；成功 Acquire 才持有帧，Map/Unmap、ReleaseFrame 及 COM apartment 由 RAII 在原线程配对。
- Acquire 最长等待 20ms；WAIT_TIMEOUT 和仅鼠标变化属于正常没有新画面，不创建空帧、不重建、不做 GPU 回读。就绪后的静止画面复用最后一张不可变图，不用“没有新帧两秒”结束健康静止桌面。
- 检查格式、尺寸、RowPitch、可读长度及内存预算；按 DXGI 的 0/90/180/270 度将存储图转为呈现方向，固定不透明 alpha。BGRA8 直接转 RGBA；浮点 scRGB 已于 10 月 27 日增加按真实白电平归一化到 SDR 的路径，见后文。无法确认格式/颜色信息时继续使用既有有界回退。

API 依据：[Desktop Duplication 的方向与鼠标语义](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)、[AcquireNextFrame 的有限等待与失效](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe)、[DuplicateOutput 要求 device 属于同一 adapter](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput)。

## 恢复、期限与取消

`capture_source.rs` 控制器区分 Lost / Denied / Unavailable。AccessLost/device reset/removed 进入有限恢复：十秒窗口最多三次替代，在同一次取帧的两秒绝对预算内等待新来源第一张画面；每次重建不重新获得两秒。不能把失效前的缓存当作新来源就绪。权限/桌面访问拒绝终止，不借 GDI 绕过；不支持、调用超时、持续失效或没有第一帧才进入本流单向 GDI 模式。

DXGI 和 GDI 对象都放入 `worker.rs` 的原生采集池，每池最多两项，与编码池独立；RGBA 独立结果跨线程，COM/D3D11/monitor 对象不跨线程。外层软件采集协调线程仍有原来的两项限制。旧 native 调用/析构挂住继续占名额，不能假归还后不断创建替代线程。

GDI 回退另有最多两秒预算，首次建立 actor 的等待计入同一次 GDI 取帧预算；初始化后每次截图同样受期限约束。Windows 软件采集就绪上限调整为六秒，覆盖 DXGI 两秒和 GDI 两秒及交付，外层视频启动仍沿用十秒边界。已过期的 run 请求在发送前拒绝；实际预算还不能超过 worker 原运行期限。迟到的原生结果与关闭的接收者隔离。

恢复开始暂停编码与最终发布；新图先安装进 Shared，再恢复准入，之后沿用原 IDR/输入时间围栏。因此即使 GDI 返回的像素与旧图完全一样，也必须交付一次，不能被静态去重吞掉而永远暂停。转入 GDI 后释放多余的旧原尺寸缓存。共享不可变来源的 Arc 未变时，软件采集跳过重复缩放/比字节，但仍获取来源并复核授权/停止状态。

取消包含父租约和局部视频流两个独立标记。重建局部流不关闭父租约，但退役流不能在等待期间继续发起新的 DXGI/GDI 恢复。控制器在初始化、取帧、回退及结果交付前后复核；native actor 初始化/实际调用前也检查。已经进入 OS 的调用不能强杀，仍由原期限与真实退出名额处理。日志保留 HRESULT、超时和回退类别，不向手机添加实现按钮。

## 本地验证

新增十二条普通回归，覆盖：

- 就绪后静止不超时；替代来源必须得到当前像素；三次恢复上限及跨十秒窗口恢复。
- 同一取帧预算随初始化/重建递减；首帧缺失有界回退；权限拒绝不回退；初始化/运行不支持单向回退。
- GDI 后续失败重新封住发布；回退释放旧原尺寸图；父取消、初始化期间取消、回退期间取消及局部退役不关闭父租约。
- 实际 worker 故意挂住原生 actor，回退 actor 仍提供当前浅色图，第三项被拒绝；释放原 actor 后旧深色图不能覆盖回退，配额实际归零。
- BGRA 行填充、通道、alpha 和四种方向；短缓冲/无效 stride/方向拒绝。
- 运行预算为零无原生副作用，短预算不扩为默认两秒；字节相同且 Arc 相同的恢复图也必须在恢复发布前交付。

最终当前实现验证通过：macOS 原生 91 项、1 项 opt-in 忽略；较新 Linux 原生 107 项、7 项忽略；两端完整 desktop 各 336 项，Mac 7 项/Linux 10 项 opt-in 忽略。原生 all-targets、desktop lib/tests 严格 Clippy 均通过。Linux 三项私有 Source/H.264/ACK/尺寸接续验收显式运行通过。

Windows GNU 原生 all-targets 严格检查及测试 PE 交叉链接通过，未执行该程序。导入核对包含系统 d3d11.dll / D3D11CreateDevice、dxgi.dll / CreateDXGIFactory1，未增加 MF 可选 DLL 静态导入；GNU 测试程序不是 MSVC 安装包验证。最终日志 `/tmp/dogcodes-dxgi-scoped-{mac,linux,windows-link}.log`、`/tmp/dogcodes-dxgi-scoped-pe-imports.log`、`/tmp/dogcodes-dxgi-final-format.log`，进程均已确认退出 0。未重跑旧 Linux、手机或服务端测试，这些代码和协议格式没有改动。

没有执行 Windows 程序、访问真实屏幕/键鼠/剪贴板、调用真实 Agent、切换日常 Core、启动模拟器或提交/推送。Windows 真实驱动/多 GPU/多屏/DPI/热插拔/休眠尚未验证；没有凭交叉编译或假 actor 声称性能、画面与恢复已全平台验收。

## 继续范围

原生 pointer shape 和动态独立光标判定已于 10 月 27 日接通：仅使用有效鼠标元数据，未知状态显式保留；状态随原帧进入编码并在当前代次传输接纳后发布。相同像素的模式变化强制 IDR，恢复来源与 GDI 回退各自重置判定。实现、隔离测试及明确限制见 [原生光标](native-cursor-2026-10-27.md#2026-10-27dxgi-动态光标状态)，Windows 实机仍待。

Windows 浮点视频及 JPEG 的 SDR 归一化已有代码和合成验证；HDR 高光/广色域完整呈现、GDI 回退的 HDR 质量、dirty/move rect、GPU 纹理到硬编、实际 Windows/MSVC 安装及 GPU/断网验证仍需继续。两个原生上下文永久挂住还需要进程边界/真正设备恢复；本轮没有强杀线程或提前释放原生对象。

Wayland EIS 整体输入后端、Linux 原生加速、真实 P2P/TURN/WSS 弱网与争用，以及剩余 PC 功能、操作恢复、metadata/辅助事务、Core 强退/跨物理代次、推送与部署都保持原范围。此项没有替代或关闭完整目标。

## 2026-10-27：浮点桌面归一化到现有 SDR 视频

原视频 DXGI 路径在发现 HDR 时直接进入 GDI，没有取得原浮点桌面并校正白电平。本轮借鉴本机 RustDesk `53996e3c4` 的 `dxgi/hdr.rs` 所采用的参考白归一化思路；本项目沿用既有 CPU 读回，在映射帧上以自有查表转换生成 RGBA，没有复制其 GPU 着色器实现或协议。

现代输出使用 `DuplicateOutput1` 请求 FP16/BGRA。HDR 必须实际取得 FP16，按原 adapter LUID 和 GDI 名字寻找唯一活动目标，读取 `DISPLAYCONFIG_SDR_WHITE_LEVEL`；不能读取、为零或镜像目标不明确时不假定白电平。SDR/Advanced Color SDR 的浮点参考白为 1.0。传统 SDR 的格式请求不受支持时仍可使用原 DXGI 调用，权限拒绝仍终止，不靠回退绕过。未知色彩空间及不支持的 HDR 输出继续沿用有界 GDI 回退。

`scrgb::Converter` 为每个原采集 worker 保留一张 64 KiB 的半浮点通道表，先按参考白缩放，再转 sRGB；逐像素只读表，直接写最终 RGBA 并处理行填充与旋转。超过 SDR 白的高光及超出 SDR 色域的值裁剪，alpha 固定不透明。这保持桌面 SDR 内容的相对颜色，没有声称完整 HDR 高光压缩、广色域或 HDR 编解码透传。NaN、无穷和负分量都有确定的有界输出，零白电平拒绝且不改旧表。

仅当真实白电平改变时重建表；原目标每秒复查一次，不新建后台定时线程。每次取帧前和完成读回后检查原 DXGI factory 是否仍当前；拓扑/色彩失效进入已有有限恢复，不能沿用旧 output 描述发布新模式的像素。GPU/COM/纹理仍由原 worker 持有，Map/Unmap 与 ReleaseFrame 配对、内存限制、原租约取消和最终发布检查保持。手机、传输协议与持久格式未变，MF/OpenH264 继续接收 8-bit SDR。

官方接口依据：[FP16/scRGB 与 HDR/SDR 参考白](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/high-dynamic-range)、[DuplicateOutput1 的格式选择](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_5/nf-dxgi1_5-idxgioutput5-duplicateoutput1)、[SDRWhiteLevel 单位与原目标查询](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-displayconfig_sdr_white_level)、[旧 output 描述的刷新要求](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_6/nf-dxgi1_6-idxgioutput6-getdesc1)。

新增五项默认合成回归：真实白电平缩放与 gamma、非有限/负值和通道单调性、白电平更新/失败保留、填充/非正方形四种方向/短缓冲拒绝，以及实际 FP16 → RGBA → 原 CB3.1 编码器 → OpenH264 解码后的四块颜色/灰阶。macOS 五项通过；Linux 完整原生 167 项通过、11 项按需忽略，两端原生 all-targets 严格检查通过。Windows GNU 原生/desktop 两包 all-targets 严格检查通过，实际生产 `video_bench.exe` 交叉链接通过，PE 导入确认 DisplayConfig 三项查询和原 D3D11/DXGI 接口，没有执行 Windows 程序。

日志：`/tmp/dogcodes-scrgb-{macos,strict-macos,linux,windows,windows-link,windows-imports}.log`。只增加现有 windows crate 的 Display API feature，锁文件与本轮前相同；没有引入新第三方 crate、运行真实 GPU/用户屏幕、Agent、日常 Core 或手机，未提交推送。

这是 Windows 视频源的代码与合成输入验证，不是 Windows 实机、真实 FP16 桌面、SDR 亮度滑块/HDR 动态切换、镜像/多 GPU、实际性能或 MF 硬编画质验收。JPEG 随下节接入同一实现，GDI 仍为原截图回退；macOS 原有测试目录中的 VideoToolbox 初始化问题、跨平台 HDR、GPU 少拷贝及原八包剩余范围继续。

## 2026-10-27：JPEG 复用同一 DXGI 控制器

Windows JPEG 原来直接在系统请求线程调用 GDI，既绕过 FP16 归一化，也没有视频采集 actor 的单次调用期限。现在把视频原有的 DXGI/有界 GDI 构造提取为同一个拉取控制器；视频与 JPEG 各自保留原流状态，共用同一个最多两项的原生采集池，没有另起池或不断抓屏的图片线程。JPEG 首次拉取才创建来源，复用最新图；五秒不拉图片后由既有 housekeeping 释放，下一次需要图片时惰性重建。实际原生析构尚未退出时仍占原名额。

JPEG 与视频因此采用同一个格式校验、FP16 白电平转换、恢复及 GDI 回退实现。每个图片来源有自己的取消标记，空闲/销毁先取消原标记再释放来源；新标记不能解冻旧来源。Windows `Capturer::new_owned` 绑定原远控租约，取消检查同时观察原租约，覆盖初始化前后、实际取帧和 JPEG 发布；发现原 owner 失效后即使进入 idle 也不能恢复该 facade。显示器几何变化沿用原重新读取/重建处理。原 Windows GDI 不支持 HDR 归一化的边界保留。

JPEG 的 `cursorEmbedded` 直接保留来源的 true/false/unknown，包括 unchanged 回复。只有明确 false 才读独立光标，避免未知 DXGI 状态叠加第二个指针。手机已有 nullable 模式与 unchanged 更新路径，本轮只增加该场景的客户端回归，没有改手机生产代码或做旧版兼容。

缩略图现在借用共享原图，再产生目标尺寸图；Windows 原始尺寸缓存和 Wayland 连续来源都不必先克隆整张 RGBA。采用原图像库的同一缩略图算法，普通/竖屏/极窄图及放大的结果逐字节对比原路径通过；没有按移除这一次复制的大小外推整体性能。

验证：四项新图片生命周期/真实 JPEG 回归及一项原缩略图等价回归通过，覆盖惰性复用、空闲与旧取消、准备中撤销、取帧中撤销、拒绝重新启用失效 owner，以及 FP16 → 归一化 → 借用缩放 → JPEG 解码灰阶。手机原光标文件共 9 项通过。macOS 同一当前测试二进制的私有硬链接路径完整原生 125 项通过、1 项按需忽略，desktop 350 项通过、8 项按需忽略，两包严格检查通过；原测试目录的 VideoToolbox 创建问题未因此关闭。Linux 完整原生 172 项、desktop 350 项、私有 Portal/cursor/JPEG/软件视频专项通过，分别 11/11 项按需测试默认忽略，两包严格检查通过。Windows GNU 原生/desktop 两包 all-targets 最终严格检查、生产视频示例 PE 交叉链接及系统 API 导入核对通过，没有执行 Windows 程序，不能当作实机通过。

日志：`/tmp/dogcodes-windows-picture-{macos,thumbnail,native-suite-macos,desktop-macos,strict-macos,phone,final-linux,final-windows,windows-link,windows-imports}.log`。没有新原生库/协议/数据格式或永久验收脚本；没有访问用户屏幕、输入、剪贴板、真实 Agent/账号或日常 Core，未提交推送。真实 Windows JPEG/视频切换、HDR/DPI/多屏、GPU 和原八包其它范围继续。
