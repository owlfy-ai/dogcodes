# Linux 硬件 H.264 与有界软件回退

日期：2026-10-26。接续远控视频目标的第 4 包。代码和隔离验证推进，不代表真实显卡、发行版安装包或手机链路已经验收；其余八包范围继续由 CURRENT_WORK 维护。

## 选择与数据路径

- X11 截图或原授权 Wayland/PipeWire 来源仍输出规范化 RGBA，不创建新的屏幕授权；JPEG、键鼠和手机协议保持原路径。P2P、TURN 与 WSS 都使用同一 EncodedFrame 输出。
- Windows 的硬件优先/软件回退控制器提取到 `video/accelerated.rs`，Linux 复用它；Windows 原 MF 实现在 `video/windows.rs`，未切换为 GStreamer。
- Linux 原生后端使用 GStreamer 1.20 及以上的安全 Rust 绑定，按有限候选 `nvh264enc`、`vah264enc`、`vaapih264enc` 探测。先要求工厂声明硬件编码，再实际创建、用合成画面试编码，核对 Annex-B、SPS/PPS、协商 profile/level 和 IDR，并用源码构建的 OpenH264 试解码核实实际尺寸。不把“插件已安装”或 API 可枚举当成可用证明。
- 运行管线：有界 appsrc RGBA → videoconvert/NV12 BT.601 SDR → 硬件编码 → h264parse → 单 AU/Annex-B → 有界 appsink。不调用 shell/ffmpeg 命令，不允许远端传入管线或插件名字，不下载运行时编解码器。
- NVENC 与 VA-API 的属性分别按对应后端设置，禁用 B 帧和不兼容的熵编码/变换，输出 Constrained Baseline；旧 VAAPI 属性名依据官方 1.20.3 源码核对。驱动不接受属性、格式、能力或关键帧时选择下一候选或软件回退。

## 生命周期与恢复

- 编码对象只在原生 worker 内创建、调用和销毁；初始化 2 秒、单次请求 500ms 上限，普通取帧最多等待 200ms。原有两个 codec permit 约束活动/退休 worker，销毁阻塞不在用户调用线程 join，也不能通过反复重连无限创建本项目 worker。
- 同时只发送一帧；appsrc/appsink 各限制一项。输入 PTS 单调递增，取回的 AU 必须匹配该请求 PTS；旧缓冲、错时间戳、空/超预算画面、缺失 IDR 都拒绝。输入 capture time 原样交回上层。
- Native 不持有发布 sink。超时请求的迟到结果只属于旧 worker/响应通道，不能进入软件接管后的画面流。外层原租约、代次、锁屏和视频发布复核继续生效。
- 调码率/FPS 时先停止旧管线，再在原 worker 上重建同一后端，重新试编解码，下一帧带真实 SPS/PPS/IDR。不假设所有驱动允许在 PLAYING 状态修改同一组属性，也不因调码率并行持有两个活动驱动实例。
- 初始化或运行失败后，本编码实例一次性切换源码构建的 OpenH264，使用当前码率并从新的参数集/IDR 开始；不在每帧重试坏驱动。重新连接或外层重新建立编码代次后可再次选择硬件。

这不是 GPU 零拷贝实现：当前仍有 RGBA 拷贝、颜色转换和上传。GPU 原生帧、HDR、原生光标、性能调优和设备/进程恢复继续保留。permit 只证明本项目 worker 有界，不代替对 SDK 内部线程、插件 scanner 子进程、Core 强退或真实 GPU 驱动退出的验收。

## 构建与分发

Linux 新增 `gstreamer`、`gstreamer-app`、`gstreamer-video` 0.25 绑定，锁文件固定解析版本，启用的 API 上限按基线使用 1.20。desktop/release 工作流安装 `libgstreamer1.0-dev` 和 `libgstreamer-plugins-base1.0-dev`。DEB 显式依赖 core/base 运行库 >= 1.20、PipeWire >= 0.3.48 以及提供默认客户端配置和协议模块的 pipewire-bin；插件和驱动是可选项，缺少时仍可软件编码。CI 另安装 base/bad 插件运行合成软件元素专项，不要求 GPU。

实际硬件需要发行版对应的 NVENC/VA-API 插件和厂商驱动；远控会话不自动安装它们。AppImage 中库/插件搜索路径、发行版打包以及真实多 GPU 选择仍需验收，不能由开发容器通过外推完成。

## 验证与证据

测试未使用真实屏幕、键鼠、剪贴板、GPU、Agent 或手机模拟器；未替换日常 Core，也未改手机/服务端生产代码。

- Linux 原生 131 项通过，9 项显式按需测试默认忽略；新增默认回归要求试编码必须能解码、尺寸正确且确为关键帧。 两个独立 ARM64 容器分别验证 Debian 12/GStreamer 1.22 与 Ubuntu 22.04/GStreamer core 1.20.3、app/video 1.20.1、GLib 2.72.4；旧库基线使用独立全新 target 构建，完整原生、严格检查和合成管线均通过。没有将 ARM64 容器结果当作 Linux x64 GPU 或安装包验收。
- 独立软件元素专项真实运行 GStreamer → OpenH264 → Annex-B → 解码：720p CB3.1、强制 IDR 与中间 P 帧、调码率/FPS 后的新 IDR、四块彩色像素、原捕获时间、旧 PTS/旧队列、缺少元素和尺寸不匹配均覆盖。该专项使用 I420 的 `openh264enc`，没有将其标成 GPU 验收；生产硬件仍输入 NV12。
- 原有共享控制器的缺失后端、运行失败、超时迟到输出、软件 IDR 接管和继续调码率回归全部保留。私有 Portal/PipeWire→视频/输入链路通过。
- Linux desktop 338 项和 3 项私有视频迁移/几何/背压专项通过，严格检查通过。macOS 原生 91 项和严格检查通过；Windows GNU all-targets 严格交叉检查通过，未执行 Windows 二进制。打包脚本 32 项及 release input 检查通过。
- 四个独立只读源码副本分别移除关键帧事件、试解码尺寸检查、PTS 检查、明确 BT.601 约束，得到对应真实断言失败；颜色反例解码出的红块为 `[215, 9, 30]`，超出像素误差预算。每项清理独立 proof target 的原生包并核实旧二进制不存在。恢复副本后完整原生、严格检查、软件管线与 Portal 专项全部通过；原生后端/控制器/worker/Annex-B 源码与恢复副本逐文件相同。

主要日志：`/tmp/dogcodes-linux-hardware-{verify,final,desktop,macos,windows,release-tests,release-inputs}.log`。反例摘要 `/tmp/dogcodes-linux-codec-witnesses.json`，结果 `/tmp/dogcodes-linux-codec-witness-results.json`，过程 `/tmp/dogcodes-linux-codec-proof.log`。Ubuntu 22.04/旧库基线完整执行结果在 `/tmp/dogcodes-linux-gst-ubuntu22.log`，不是仅据镜像构建成功认定通过。

合成软件管线命令（显式 opt-in，需已安装软件元素；不访问显示器）：

```sh
DOGCODES_LINUX_ENCODER_FIXTURE=synthetic-only cargo test -p dogcodes-remote-desktop --lib --locked synthetic_pipeline_verifies_headers_forced_idr_pts_rate_and_rejection -- --ignored --nocapture
```

真实 Linux GPU 的合成画面专项已提供，但本轮未运行；只在安排好的实机上启用：

```sh
DOGCODES_LINUX_ENCODER_FIXTURE=hardware-synthetic-only cargo test -p dogcodes-remote-desktop --lib --locked hardware_synthetic_idrs_and_live_rate_changes -- --ignored --nocapture
```

## 参考

借鉴本机 RustDesk `53996e3c4` 的 `libs/scrap/src/common/hwcodec.rs`：实际能力探测、硬件失败后的软件接管及配置结算思路；没有引入或复制其 hwcodec/FFmpeg 实现、私有协议和消息格式。

- [GStreamer VA H.264 参数与输入能力](https://gstreamer.freedesktop.org/documentation/va/vah264enc.html)
- [NVENC H.264 参数与输出能力](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264enc.html)
- [官方旧 VAAPI 1.20.3 源码归档](https://gstreamer.freedesktop.org/src/gstreamer-vaapi/gstreamer-vaapi-1.20.3.tar.xz)
- [appsrc 有界输入](https://gstreamer.freedesktop.org/documentation/app/appsrc.html)、[appsink 输出](https://gstreamer.freedesktop.org/documentation/app/appsink.html)

真实 NVENC/Intel/AMD 驱动、旧 VAAPI、多 GPU、X11/Wayland 桌面、手机解码与 P2P/TURN/WSS、弱网、发行版安装包和性能仍在包 4/5/7 验收范围。硬编路径存在不等于这些项目已完成。


## 2026-10-27：GStreamer 输入共享原采集帧

旧 `Stream::encode` 对每次输入执行 `frame.image.as_raw().clone()`，再把新 Vec 包装为 GStreamer buffer。即使尺寸已经合适、原采集帧已由 Arc 管理，仍会额外分配并复制完整 RGBA 画面。1280×720 每次是 3,686,400 字节（约 3.52 MiB）；静态细化和强制关键帧同样重复复制。

现在使用安全绑定的只读 `Buffer::from_slice`，包装一个持有原 `Arc<RgbaImage>` 的 `AsRef<[u8]>` 所有者。原像素保持到最后一个原生 memory 引用释放，不借用可能先退出的栈变量，也不新增 unsafe。buffer 的 PTS/duration 元数据仍可写，像素被标为 READONLY；下游确实请求写映射时，GStreamer 复制为自己的可写数据，不能修改共享采集/重发缓存。原单输入、PTS、背压、原 worker 退出、profile/IDR、颜色和最终发布检查保持。

参考本机 RustDesk `libs/scrap/src/common/mod.rs::EncodeInput<'a>` 的 YUV 借用与 `src/server/video_service.rs` 的同步编码输入思路：让编码器使用仍由上游持有的帧，不为接口包装无条件复制。本项目异步 GStreamer 用 Arc 持有而非栈借用；具体代码自行实现，没有复制 RustDesk 或更换其纹理协议。

新增两项普通内存回归验证原始指针相同、READONLY、采集所有者先退出后仍有效、最后原生引用退出后释放、元数据可写及实际写映射 COW 不改原图。共享断言在旧复制路径下按预期失败，证明该处原来确实复制，并非声称旧复制本身不安全。新增显式 CPU 微基准同时测旧 Vec 克隆和新包装，不用耗时阈值作为易波动的测试断言。

验证：Linux 原生 160 项、desktop 349 项通过，分别 11/11 项按需专项默认未启用；两包 all-targets 严格检查通过。另显式执行真实 GStreamer/OpenH264 合成管线，原颜色、实际解码、PTS、IDR、调码率与拒绝检查通过。Ubuntu 22.04、GStreamer core 1.20.3/app 1.20.1、GLib 2.72.4 的独立原生 target 上，两项新内存回归、同一合成管线及原生 all-targets 严格检查通过。未增加依赖/API 下限、变更协议、改手机/服务端或重跑没有改动的 macOS/Windows 套件。

同一容器/进程、720p、各 300 次（各先预热 20 次）的输入包装平均耗时：旧复制 88.519µs，新共享 222ns。这里只量输入包装与读映射，不包含采集、颜色转换、GPU 上传、编码、网络或解码；不能按该比值声称整个视频提速。没有声称 GPU 零拷贝或真实 NVENC/VA-API 驱动验收，GPU 上传/HDR/真实设备性能及原八包继续。

日志：`/tmp/dogcodes-linux-frame-input-before.log`（旧路径共享断言失败）、`/tmp/dogcodes-linux-frame-input-after.log`（完整回归、软件管线、微基准和严格检查）、`/tmp/dogcodes-linux-frame-input-ubuntu22.log`（旧库基线）。只使用合成画面和已存在的独立测试镜像，没有访问用户屏幕/输入/剪贴板、真实 GPU、Agent 或手机，未切换日常 Core、提交或推送。

## 2026-10-27：DEB 补齐直接链接的 PipeWire 运行库

已有 DEB 声明列了 GStreamer，但遗漏 `libpipewire-0.3-0`。实际非测试 desktop 二进制直接链接 `libpipewire-0.3.so.0`，因此没有安装该库时，程序在选择 X11/Wayland 或软件编码回退之前就无法加载。配置增加 `libpipewire-0.3-0 (>= 0.3.48)`，沿用已有 Ubuntu 22.04 基线；没有升级 Rust 依赖或强制安装可选硬编插件。

核对锁定的 tauri-cli 2.11.4 官方源码：[Linux 默认依赖](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-cli/src/interface/rust.rs#L1289-L1347) 仅在配置之外补 GTK、WebKit 和选用的托盘库；[DEB control 生成](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/linux/debian.rs#L187-L199) 不会替应用分析全部 ELF 依赖。当前 Debian 依赖图已由这些基础包间接带入 GBM、EGL、Wayland、XKB 等链接库，只有 PipeWire 缺失，故没有重复扩展声明。

本地验证在独立 ARM64 Debian 12 容器完成：离线构建实际 `desktop` 非测试 debug 二进制，使用 `readelf`、`ldd` 和已安装包的 Depends/Pre-Depends 元数据计算声明的依赖闭包。把该闭包提供的 ELF 库复制到临时私有根目录，以动态加载器 `--list` 验证，不启动应用。原配置缺少 PipeWire，加载器 exit 127；仅补 PipeWire 后，同一二进制所有链接库解析通过、exit 0。修改真实配置后再次执行也通过。既有打包脚本 32 项及 release input 检查通过。

诊断脚本仅放 `/tmp`，没有新增长期打包工具或镜像实现断言的配置测试。日志：`/tmp/dogcodes-linux-runtime-production-link.log`、`/tmp/dogcodes-linux-runtime-closure-before-final.log`、`/tmp/dogcodes-linux-runtime-closure-after.log`、`/tmp/dogcodes-linux-runtime-release-tests.log` 和 `/tmp/dogcodes-linux-runtime-release-inputs.log`。

这里验证的是当前 Debian ARM64 二进制的动态库声明，不是 DEB/AppImage 构建或安装验收，也不是 Ubuntu 22.04/x64、真实 GPU/桌面或插件搜索路径验收；这些继续留在原包 4/5/7。没有操作日常 Core、用户屏幕/输入/剪贴板或真实 Agent，没有提交推送。

## 2026-10-27：DEB 实际构建与干净环境安装

在前述依赖声明验证之后，重新构建当前前端，再在隔离 ARM64 Debian 12 中以 `custom-protocol` 构建实际 desktop debug 二进制，嵌入当前前端资源。用锁定的 tauri-cli 2.11.4 执行 `bundle --debug --bundles deb --features custom-protocol --ci`，生成 `dog.codes_0.1.0_arm64.deb`，包名为 `dog-codes`。只在临时 Docker 工具镜像中安装对应 CLI，没有新增仓库脚本、修改锁文件或读取本机签名设置。

随后在全新 `debian:bookworm-slim` 容器中安装该包；初始不存在 PipeWire、GStreamer、GTK 和 WebKit 运行包，使用 `apt-get install --no-install-recommends` 从实际 DEB 的 Depends 安装所需运行库。安装完成后，`ldd /usr/bin/desktop` 全部解析，没有 `not found`；实际 control 包含显式 PipeWire/GStreamer 及 Tauri 补入的 GTK/WebKit 依赖。可选 `gstreamer1.0-plugins-bad` 未安装，不影响此动态链接验证。

桌面入口的 Exec/Icon 均能定位已安装的可执行文件和图标。三个图标、项目 LICENSE 及 agent-icons 下全部许可证资源与当前源码逐字节相同。安装与检查最终 exit 0；未运行安装后的 GUI/Core，不把动态库可解析当作实际软件编码或硬件插件可用的证明。

构建日志：`/tmp/dogcodes-deb-frontend-build.log`、`/tmp/dogcodes-deb-binary-build.log`、`/tmp/dogcodes-deb-bundle.log`；包内容与 SHA-256：`/tmp/dogcodes-deb-inspect.log`；实际干净环境安装与断言：`/tmp/dogcodes-deb-clean-install.log`。产物保存在独立验证卷 `dogcodes-video-native-target-20261025` 的 `/native-target/debug/bundle/deb/`，没有上传或作为正式发行产物。

这补齐当前 Debian 12 ARM64 调试包的实际构建、安装、依赖及资源检查，不代表 release 优化构建、Ubuntu 22.04/x64、AppImage、真实桌面/GPU、驱动或跨平台发布验收通过。原开发容器与数据卷保持，Docker 接口已正常，无需再次重启；没有访问用户屏幕/输入/剪贴板、真实 Agent/账号或日常 Core，未提交推送。

## 2026-10-27：AppImage 的 PipeWire 客户端模块

实际生成 AppImage 后确认：ELF 扫描打入了 `libpipewire-0.3.so.0`，但没有动态加载的 SPA 支持模块。在不装系统 PipeWire 的干净容器中，原 AppDir 连 `pw_main_loop_new` 都失败，报告 `can't make support.system handle: No such file or directory`，尚未走到 Portal 或屏幕授权。仅检查 `ldd` 不能证明这条路径可用。

现于原 `package-desktop.mjs` 的 Linux AppImage 分支，用本机 `pkg-config` 的 `libdir`/`moduledir` 定位 `libspa-support.so`、`libpipewire-module-protocol-native.so` 和 `libpipewire-module-client-node.so`；缺失时在构建前明确报错，不生成缺模块的包。通过 Tauri 的 AppImage files 加入这三项及项目自有的最小客户端配置、启动脚本、桌面入口。没有引入旧版兼容、远程下载客户端模块、系统音频或 PipeWire 服务管理。

启动入口先设置 `SPA_PLUGIN_DIR`、`PIPEWIRE_MODULE_DIR` 和配置目录，再以 `exec` 运行原 desktop 二进制并原样传递参数，不留下额外常驻进程。配置只加载原生客户端协议和 client-node，采集仍使用 Portal 授予的原受限 FD，不连接未授权默认服务器。模块匹配包内的库版本，不依赖目标发行版恰好安装同版本的模块。锁定的 Tauri 打包器只把自定义文件里的 usr 子树复制进 AppDir，不能靠声明根级 apprun-hooks 放入钩子；因此使用包内的桌面入口和 usr/bin 启动脚本，而未修改 Tauri SDK 或在多线程 Rust 进程中改全局环境变量。参考 [Tauri 对应版本的打包实现](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/linux/appimage/linuxdeploy.rs)。

已在两个现有工作流补齐模块包与 `file` 构建工具。原 `wayland_acceptance.py` 新增显式 `--appimage` 模式：提取实际产物，在私有副本中只把入口二进制替换为当前原生测试程序，保留实际 AppRun、启动脚本、库、配置和模块；合成服务端/源仍用系统环境。桌面 CI 在生成安装包之后运行这一模式，避免只验证开发环境。没有新建单独验收脚本，也没有执行 GitHub 工作流。

验证与限制：

- 实际 ARM64 debug AppImage 构建通过。纯命令行 Debian 缺少 linuxdeploy 有意排除的基础图形/字体/音频库，不能当作桌面系统；根据实际缺失 SONAME 安装对应的 13 个基础包建立隔离基线，不安装系统 PipeWire、SPA 或 GStreamer 包。提取后把目录搬到含空格的路径，原应用与客户端模块的全部动态库解析通过；图标、许可证、启动入口和配置与源码逐字节一致。
- 在该干净基线的私有提取副本中，以小型 C 客户端探针替换入口，实际 AppRun → 启动脚本 → PipeWire loop/context 成功，带空格的参数边界保持。探针没有连接服务器、启动 GUI 或 Core；不能由此声称 GUI 已验收。
- 使用实际包内组件的私有 Portal/cursor 夹具通过，核实原受限 FD、真实 PipeWire 合成像素、JPEG 光标、软件视频及原授权/退出检查；原不带 AppImage 的模式也通过。最终两种模式均由提交的同一脚本执行，每种 1 项。可选 D-Bus/RT 支持产生警告，但所需客户端链路通过，没有为消除可选警告扩展模块范围。
- Ubuntu 22.04 / PipeWire 0.3.48 的本机 SDK 路径解析及同一最小配置创建 context 通过；这不是在 Ubuntu 上运行 Debian 产物的证明。打包回归共 33 项通过，覆盖 ARM64/x64/lib64 路径、空格、缺失模块和无效 SDK 路径。没有改 Rust 生产逻辑或重跑未受影响的跨平台全套测试。

日志：`/tmp/dogcodes-appimage-pipewire-before.log`（原模块缺失反例）、`/tmp/dogcodes-appimage-final-rebundle.log`（最终产物）、`/tmp/dogcodes-appimage-final-clean.log`（实际提取/动态库/资源/AppRun）、`/tmp/dogcodes-appimage-integrated-portal.log`（最终包内与原模式）、`/tmp/dogcodes-appimage-client-ubuntu22.log`、`/tmp/dogcodes-appimage-release-tests.log`。临时探针与诊断工具留在 `/tmp`，原开发容器/数据卷保持。

本项解决已有 AppImage 客户端漏包，不把 ARM64 debug 隔离结果外推为 x64/release 优化包、真实桌面/驱动/GPU、GStreamer 硬编插件、其他发行版或实际手机链路验收。后者及原八包范围继续；没有访问用户屏幕、输入、剪贴板、真实 Agent/账号或日常 Core，未提交推送。


## 2026-10-27：Ubuntu 22.04 实际 release 包和 DEB 客户端配置

从独立源码快照和新 target 开始，在 Ubuntu 22.04 ARM64 上运行锁定的 Tauri CLI 2.11.4 实际优化编译及 DEB/AppImage 打包。前端先完成 TypeScript/Vite 生产构建，再将新 dist 放入快照，避免使用旧嵌入页面；沿用 Cargo 默认 release 优化，没有降低优化等级。系统 PipeWire 保持 0.3.48、GStreamer core 1.20.3/base 1.20.1，SPA 仍只使用原脚本准备的构建头文件。此次不是 x64 构建或正式发布。

干净 DEB 安装发现第二个实际运行依赖缺口：仅声明 libpipewire-0.3-0 能让 ELF 加载，但其推荐包在 --no-install-recommends 下不会安装默认 client.conf 和原生协议模块。私有 C 客户端使用与生产相同的默认 Context 创建方式，返回 `can't load config client.conf: No such file or directory`、exit 2。Ubuntu 22.04 和 Debian 12 的本机包元数据均确认 client.conf 由 pipewire-bin 提供，该包直接依赖匹配版本的 libpipewire-0.3-modules。因此 DEB 增加 `pipewire-bin (>= 0.3.48)`，不增加自有启动器或服务管理。依赖修改后使用原优化二进制重打 DEB，只调整打包元数据；反复打包产生的 Tauri bundle type 提示不作为新的优化编译结果，当前未使用 Tauri updater。

实际验证结果：

- 全新 Ubuntu 22.04 容器从该 DEB 使用 --no-install-recommends 安装，最终完整脚本 exit 0。desktop 的动态库全部解析，许可证和三种尺寸图标与快照逐字节核对，桌面入口存在，默认 PipeWire Context 创建成功，client.conf 和协议/client-node 模块存在，未安装可选 gstreamer1.0-plugins-bad 包。临时检查脚本的高分辨率图标映射及 dpkg 未安装条目判断曾出错，均已修正后重新完整执行，不把这些错误算成产品缺陷。
- 实际 release AppImage 放在只提供 13 个基础图形/字体/音频包的干净 Ubuntu 22.04 基线中，没有安装系统 PipeWire/SPA/GStreamer。提取并搬到含空格目录后，真实 desktop 与模块依赖、资源字节、AppRun/启动脚本和参数边界均通过；以私有 C 入口验证包内客户端 Context，未启动 GUI/Core 或连接系统服务。
- 原 wayland_acceptance.py 增加 --release，选择实际优化原生测试二进制。使用实际 release AppImage 的 AppRun、库、配置和模块，原私有 Portal/rgb-cursor 专项 1 项通过，验证受限 FD → 320×240 真实合成像素、光标、软件视频及原授权/退出路径。合成服务端仍使用隔离系统库，不代表真实 compositor 或 GPU 验收。
- 同一 Ubuntu 基线 release 原生普通回归 172 项通过，11 项按需测试未启用；打包脚本 18 项、发布输入、Python AST、工作流 YAML、差异检查通过，Cargo.lock 与本轮开始前一致。发布流程在上传前新增同一路径的优化包专项，并安装所需私有 fixture 工具；没有触发 GitHub Actions。

最初 AppImage 打包失败来自临时 SDK 镜像缺 dpkg-dev；补齐后成功，现有 GitHub 流程的 build-essential 已提供它，没有据此修改产品依赖。附加的小型私有 AppImage 运行时观测也未复现父入口退出后子进程资源丢失，本轮没有据该假设另加进程/资源缓存框架。

证据：`/tmp/dogcodes-linux-release-frontend.log`、`/tmp/dogcodes-ubuntu22-release-build.log`、`/tmp/dogcodes-ubuntu22-release-rebundle-fixed.log`、`/tmp/dogcodes-ubuntu22-release-deb-context-before.log`、`/tmp/dogcodes-ubuntu22-release-deb-final-proof.log`、`/tmp/dogcodes-ubuntu22-release-appimage-clean.log`、`/tmp/dogcodes-ubuntu22-release-appimage-portal.log`、`/tmp/dogcodes-ubuntu22-release-native.log`、`/tmp/dogcodes-ubuntu22-release-package-tests.log`、`/tmp/dogcodes-ubuntu22-release-inputs.log`。本次 DEB SHA-256 为 `01edc47f0d4a929157994dd35e607ad653c56ec9d70da6e67348f904988bf99d`，AppImage 为 `5bec9387c2e5ab315c284f1425176b4984f59f02cf2c930222f57d651acd7773`；它们仅是本次隔离验证产物。

本项补齐 Ubuntu 22.04 ARM64 优化构建与包内客户端证据，不能外推 Linux x64、其他发行版、实际桌面/手机、硬编插件/多 GPU 或正式发行验收。原八包范围保持，原 server/Postgres/Redis 开发容器与数据卷保持；没有访问用户屏幕、输入、剪贴板、真实 Agent/账号或切换日常 Core，没有提交、推送、创建 tag 或发布。


## 2026-10-27：Linux x64 release 隔离验证（本地完成）

本机 Docker 的 amd64 容器已实际执行 uname 并返回 x86_64。独立 amd64 SDK 使用 Rust 1.97.1、Node 22.23.4、Tauri CLI 2.11.4、PipeWire 0.3.48、GStreamer 1.20.3；运行基线仍仅含原 13 个基础桌面库。当前 1009 个构建源码文件与上一轮已修正 DEB 依赖的 ARM64 快照逐字节一致，429 个前端输入也一致，因此复用上一轮已验证的新生产 dist。独立 target 卷为 `dogcodes-ubuntu22-amd64-desktop-release-target-20261027`，没有混用 ARM64 编译产物。

Rust 实际优化编译已在 56m 36s 完成，生成当前 desktop 和 amd64 DEB。首次整段 CLI 在 AppImage 阶段失败；详细诊断及独立调用确认，正确架构的静态 PIE AppImage 构建插件在当前本机模拟执行时返回 Exec format error，父工具报 subprocess exit 2。通过 Ubuntu 官方包提取的 ARM64 静态 QEMU，原插件的 offset 和 API 版本调用均成功。仅对本次私有构建缓存中的插件调用临时加执行前缀，使用同一优化二进制重新打 AppImage 成功，finally 恢复原插件；没有重编 Rust、切换 Docker 全局模拟设置或替换安装包运行时。这是本机验证工具处理，不是新增产品兼容层。

实际包检查已通过：

- `dog.codes_0.1.0_amd64.deb` 在全新 Ubuntu 22.04 中以 --no-install-recommends 安装，完整脚本 exit 0；全部动态库解析，许可证与三种图标字节核对、桌面入口、默认 PipeWire Context、client.conf 及协议/client-node 模块检查通过，未安装可选 bad 插件包。只复用下载缓存，没有预装运行库。SHA-256：`1d67acb0b4c4e491654304e392beb3baf19383f9b41179b084afe54619abe564`。
- `dog.codes_0.1.0_amd64.AppImage` 在没有系统 PipeWire/SPA/GStreamer 的基础桌面库基线中，提取并搬到含空格路径；真实 desktop 和客户端模块的动态库解析、资源、AppRun/启动脚本、参数边界与包内 PipeWire Context 均通过。仅静态运行时的提取使用隔离 QEMU；实际 AppRun 与 C 探针仍在同一 x64 环境执行，没有启动 GUI/Core。SHA-256：`633e96c0506e68a35b64aaafc489a24edb3d9ac9a99f71b2434284b165c127eb`。

优化原生普通测试实际通过 172 项、11 项按需默认忽略；随后显式执行 GStreamer 软件合成管线 1 项和私有 Xvfb/X11 捕获 1 项，均通过。后者从 1024×768 虚拟显示器取得变化画面并实际解码两帧 CB3.1 视频。真实 AppImage 的 Portal/rgb-cursor 专项在原生验证 exit 0 后执行，1 项通过，确认原受限 FD → 320×240 真实 PipeWire 合成像素、光标、软件视频及原授权/退出检查；只在运行时提取时使用同一个私有执行前缀，保留实际包内 AppRun、库、配置和模块。完整测试 stdout 已保存，不用包检查推断视频通过。

主要证据：`/tmp/dogcodes-ubuntu22-amd64-release-build.log`、`/tmp/dogcodes-ubuntu22-amd64-bundle-diagnose.log`、`/tmp/dogcodes-ubuntu22-amd64-appimage-plugin-file.log`、`/tmp/dogcodes-appimage-qemu-plugin-probe.log`、`/tmp/dogcodes-ubuntu22-amd64-appimage-qemu-bundle.log`、`/tmp/dogcodes-ubuntu22-amd64-deb-clean.log`、`/tmp/dogcodes-ubuntu22-amd64-appimage-clean.log`、`/tmp/dogcodes-ubuntu22-amd64-appimage-portal.log`、`/tmp/dogcodes-ubuntu22-amd64-native-check.log`。源码快照/校验信息在 `/tmp/dogcodes-amd64-release-source.json`。最终重新核对当前 1009 个构建输入，与验证快照无差异，Cargo.lock 与本轮开始前一致，记录 `/tmp/dogcodes-ubuntu22-amd64-final-source-check.json`。原 DEB/视频/Portal 临时容器均已结束，只保留本次下载和编译缓存；原 server/Postgres/Redis 容器及数据保持。

这里证明的是本机模拟执行下的当前 x64 优化构建、包检查和上述合成视频路径，不替代真实 x64 桌面、GPU/驱动/FUSE、手机和公网验收；没有运行真实 Agent、用户屏幕或日常 Core，没有提交、推送、创建 tag 或发布。原八个工作包保持。
