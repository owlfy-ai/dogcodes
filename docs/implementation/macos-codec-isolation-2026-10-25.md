# macOS 原生编码与销毁隔离

日期：2026-10-25。接续 [编码前准入](video-output-admission-2026-10-25.md) 和 [码率恢复](video-relay-recovery-2026-10-25.md)。本轮处理 VideoToolbox 生命周期，不改手机/服务端协议，也不将所有采集、驱动或设备恢复视为完成。

## 原因与参考

原先 macOS 在 `Shared.encoder` 锁内直接调用 VideoToolbox 创建、调参和 `encode_frame`，`Encoder::drop` 又直接 `complete_frames`、invalidate 和释放 CF 对象。即使已有实际输出超时检查，原生调用本身不返回时，ticker、错误读取或关闭仍可能等待同一把锁。

继续核对本机 RustDesk HEAD `53996e3c4`：`libs/scrap/src/common/hwcodec.rs::start_check_process` 将硬件探测放到独立进程并限制等待，`src/server/video_service.rs` 对 DXGI access-lost 使用有限重建和后备。借鉴“原生失败不占住业务调用方、重试有边界、失败仍保留原恢复通路”的思路，复用本项目已有 Windows `video/worker.rs`，独立实现 macOS 适配。

**本轮是有界线程隔离，不是 RustDesk 的独立探测进程，也不能杀掉挂住的驱动调用。** 未来仍需处理采集原生调用、真正设备丢失和需要进程边界的恢复；没有把两个挂住的线程改名为“设备已恢复”。

## 实现

- `platform::Encoder` 只保留普通 Rust 状态、真实发布 sink 与 worker 句柄。VideoToolbox `Native` 的创建、准备、调参、输入、刷新、invalidate 及 CF 对象释放都在同一专属线程；移除旧 `Encoder` 对原生对象的 `unsafe impl Send`。
- 全库原生 codec worker 同时最多两项，每个请求队列最多一项。Native 创建、运行或析构不返回时，线程仍持有配额；旧外层对象消失不提前归还，避免重连无限创建线程。Windows 继续沿用原有两秒期限。
- macOS 初始化和逐帧运行分别限五秒、两秒。实际冷初始化夹具测到约 2.93 秒，因此没有将正常创建算进两秒的输出期限；也没有用扩大初始化期限来放宽每次输入的运行期限。
- macOS 回调只解析 Annex-B、校验协商 profile，并向容量一的完成通道交付 `Some(frame)`、dropped 的 `None` 或错误。它不持有真实 sink，不执行 OS 状态检查或网络发送。最后归还 native 输入信用后才通知专属线程，下一输入不越过仍在处理的回调。
- 外层 `encode` 只有在当前请求按时完成时才调用真实 sink。帧原始 codec 输入时间保持不变，既有 Flow/发布守卫继续核实停止、原来源与桌面状态。dropped 回调归还信用，但不伪造实际输出或 IDR。
- 运行超时后禁用该外层编码器，错误读取和 `set_rate` 只访问缓存；再次 `encode` 不重新进入旧 worker。原生调用迟到返回时，worker 检查退休状态并抛弃结果；旧原生回调上下文随线程继续存活，避免提前释放 refcon。
- 关闭外层编码器不 join 正在运行/析构的原生线程。当前正在执行的输入仍可能让其调用方等待到两秒的运行期限；这不是所有 `VideoStream::drop` 或采集 API 都立即返回的保证。

## 启动失败的归属

采集回调可以早于 owner 的就绪检查运行。若它已经遇到 native 初始化/编码失败，旧就绪分支仍可能发现 `encoder == None` 并再次创建，覆盖首个失败甚至占用另一个原生名额。

现在 `Shared::initialize` 在与采集相同的 encoder 锁内先核实停止状态；已经失败的流直接返回原错误，不新建也不复活。ticker 创建失败也会先标记停止并停止采集，再返回启动错误。后者没有通过耗尽宿主机线程来制造失败。

## 验证

- macOS 原生库当前 59 项通过、1 项质量测量 opt-in 默认忽略；覆盖实际 VideoToolbox SPS/IDR、合成像素解码、静态刷新与满窗缓存恢复。原回调信用用例仍实际阻塞回调末段，确认信用不会提前返回。
- 新增 macOS 外层超时用例：模拟原生调用卡住，超时后错误/调参/关闭有界，真实 sink 引用立即释放；旧调用仍未返回时，新 VideoToolbox 实例编码出可独立解码的 IDR，并保留输入时间；释放旧调用后确认它没有发布迟到画面。
- 新增析构阻塞用例：关闭外层后旧析构仍占名额，第二实例可运行，第三实例被拒，直到释放旧析构。另覆盖 dropped 回调不制造输出/错误，以及初始化较慢不放宽运行期限。
- 独立源码副本分别恢复共用初始化/运行期限、提前返回回调信用、提前归还原生名额，三项对应回归均失败。第四副本只移除停止状态就绪守卫，现有首错误用例在 `unwrap_err` 收到成功就绪，复现流被重新初始化。未回退工作区源码，副本没有接触实际屏幕/键鼠。
- 最终 macOS 原生库 59 项、旧/新 Linux 原生库各 75 项，以及 macOS/较新 Linux 完整 desktop 各 336 项通过；分别 1/7 个原生、7/9 个 desktop opt-in 默认忽略。原生 all-targets 和 desktop lib/tests 严格 Clippy 通过，Windows GNU 原生 all-targets 严格静态检查通过；没有执行 Windows 程序。
- 较新 Linux 私有 login1 下的两项实际编解码验收已单独执行，通过跨编码预算接续、Source→codec→RelayVideo→累计 ACK 满窗恢复；旧 Linux 另外单独执行 NV12 与裁剪旋转两个私有来源验收。没有重跑旧 Linux 的完整 desktop，也不将默认忽略的其它用例算成已验收。

测试中的阻塞是私有 Rust 夹具，真实 VideoToolbox 部分只使用合成 NV12 图，不采集用户桌面；不证明特定硬件编码器被选择，也不等于真实 GPU 挂起/恢复验收。

反例目录：本机临时 `dogcodes-codec-isolation-witness-w4hefloy`；日志 `/tmp/dogcodes-mac-isolation-witness-{phase-deadline,callback-credit,native-lifetime,readiness}.log`。首次冷初始化等待两秒导致的失败保留在 `/tmp/dogcodes-mac-isolation-initial.log`，实际 2.93 秒测量在 `...-cold-init.log`，不是生产首帧无输出的证据。

当前代码最终日志：`/tmp/dogcodes-mac-isolation-{native-final,native-final-clippy,desktop-final,desktop-final-clippy,windows-final,linux-final}.log`。未用回退工作区或忽略失败用例制造通过。

旧 Linux 的 75 项、严格检查和两种实际来源记录为 `/tmp/dogcodes-mac-isolation-old-linux.log`。Docker 任务均已完成，未保留运行中的自有验收容器。

## 仍需完成

ScreenCaptureKit 创建/配置/停止调用本身尚未采用相同隔离；其异步回调等待虽有五秒期限，不代表调用入口或 CF/Objective-C 释放可以被中断。挂起线程不能强杀，两个 native 名额都被挂住时会拒绝新 codec，进程隔离/驱动重建和真正设备恢复仍开放。快速切换时的资源退役、Linux 原生硬编/Windows 原生采集、动态最终几何、真实平台/手机/弱网与控制/附件争用也继续。

原目标的全部 PC 功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次、通知与官方部署范围保持。未切换日常 Core、调用真实 Agent、启动模拟器或操作实际桌面/键鼠/剪贴板，原有三个 Docker dev 服务保留。

后续更新：[采集控制与退出确认](macos-capture-isolation-2026-10-25.md)已将 ScreenCaptureKit 控制/对象释放移入另一有界池，接入父租约取消和 delegate 发布失效守卫；实际 OS 权限/驱动事件与进程恢复仍开放。

证据补记：早先多个源码副本共用 target，复制保留的时间戳可能导致使用正常版本缓存；上文旧反例日志不再作为独立完成证据。现已在独立 APFS 构建目录中，每次清理 remote-desktop 包、确认测试产物消失，再编译指定阶段副本，本轮四项及这里四项均在预期断言失败；随后再次清理并编译当前工作区，Mac 原生 66、完整 desktop 336 项和严格检查通过。以新记录中的 `dogcodes-isolation-clean-witness-*` 和 `dogcodes-isolation-clean-positive-*` 日志为最终证据。未清理 live target 或回退工作区。

## 2026-10-27：准备失败不能报告就绪

核对发现创建完成后忽略了 `prepare_to_encode_frames` 返回的 OSStatus。现在非零状态作为原初始化错误返回，仍由原线程拥有并销毁 Native；不报告就绪，不发布输出，也不延长初始化期限。依据 [Apple 准备编码说明](https://developer.apple.com/documentation/videotoolbox/vtcompressionsessionpreparetoencodeframes(_:))，这一步可提前分配编码资源；资源分配失败不能按成功处理。未扩大可选属性的强制要求。

macOS 原生 all-targets 严格检查通过；现有九项受管 worker 用例、首次失败不复活用例、丢帧信用用例共 11 项通过。日志为 `/tmp/dogcodes-vt-prepare-{strict,worker,readiness,callback}-macos.log`。没有新增仅镜像状态判断的测试，也未声称模拟了 OS 的资源分配失败。本机上一轮系统创建调用超时仍未解决，因此未把真实 VideoToolbox 完整回归列为通过；该超时发生在 prepare 之前，不能声称此改动修复其根因。

## 2026-10-27：环境恢复后的初始化复核

用户授权恢复 Docker 后，Linux 开发服务与验证已恢复，但独立 macOS 合成编码测试仍在约五秒时报“系统编码器初始化未获确认”。没有放宽原期限、重启系统视频服务或切换日常 Core。仅针对自己创建的合成测试子进程调用 sample；报告只有镜像列表、调用图为空，不能据此断言系统服务、驱动或某个锁是根因。该诊断不计为测试通过，未继续运行依赖它成功的全套原生测试。

日志 `/tmp/dogcodes-video-recovery-isolated-macos.log`、`/tmp/dogcodes-video-create-sampled-test-macos.log`；采样 `/tmp/dogcodes-video-create-sample-macos.txt`。本轮没有采集实际屏幕或操作键鼠/剪贴板，原生创建问题继续保留。


## 2026-10-27：生产路径与测试执行目录对照

继续复核前述创建超时，先对照本地 RustDesk 的 `libs/scrap/src/common/hwcodec.rs`：其上层通过独立 hwcodec/FFmpeg 包创建编码器，并没有可直接对照的 VTCompressionSessionCreate 调用，未据此照搬或猜改参数。私有 C pthread 分别使用本项目的低延迟配置、默认配置与允许软件配置，320×240 均在一秒内创建并准备成功；匹配本项目失败用例的 640×360 低延迟配置也成功。使用当前依赖的 objc2 包复现相同调用、加入实际回调类型、额外框架及 Rust test harness 后同样成功。16 MiB 线程栈没有解决原用例；清理本包构建产物并重新编译也仍失败。以上仅缩小边界，不等于确认操作系统根因。

当前生产构建的 `video_bench --synthetic` 无改动通过：1920×1240 的三段纯合成输入均输出编码画面，30 fps/4 Mbps 一段输出 120 帧（2 关键帧），15 fps/1 Mbps 一段输出 57 帧（2 关键帧）；码率约 3952/1007 kbit/s，编码延迟平均约 27/28 ms。这是本机合成路径测量，不包含真实采集、网络或手机消费，不用跳帧数推断屏幕丢帧。

进一步以同一 inode、同一字节的测试二进制做交替对照：原 `target/debug/deps` 路径失败；同目录缩短文件名仍失败；私有 /tmp 目录保持原文件名的硬链接通过；回到原路径再次失败。没有修改签名、测试断言、五秒初始化期限或两秒运行期限。该证据表明当前失败与执行路径相关，不能再概括为“本机生产编码不可用”；具体系统机制仍未确认。临时测试打印随后已移除并重新编译，正式对照与全套使用的都是没有诊断改动的当前源码产物。

将该当前测试二进制硬链接到私有临时目录，完整原生库 **115 项通过、1 项原质量测量 opt-in 未启用**，包含真实合成 VT 输出/独立解码、关键帧、静态刷新、背压、资源退役/重建、回调信用及图片配置的私有回归；没有跳过此前失败用例。原路径普通 cargo test 仍有环境问题，不能据临时目录通过声称原命令恢复正常。

证据：`/tmp/dogcodes-vt-create-{variants,size}-macos.log`、`/tmp/dogcodes-vt-wrapper-macos.log`、`/tmp/dogcodes-vt-callback-variants-macos.log`、`/tmp/dogcodes-vt-harness-macos.log`；原失败及排除缓存/栈记录 `/tmp/dogcodes-vt-{create-current-isolated,create-large-stack,clean-isolated}-macos.log`；生产合成 `/tmp/dogcodes-vt-production-synthetic-macos.log`；交替对照 `/tmp/dogcodes-vt-name-control-macos.log`；最终全套 `/tmp/dogcodes-vt-current-full-isolated-macos.log`。旧失败日志保留，没有用通过结果覆盖。临时硬链接/测试进程已清理，无真实屏幕、键鼠、剪贴板、Agent 或日常 Core 操作，未提交推送。

这次没有生产代码修复：保留已有 prepare 状态检查、原超时与退役保护。真实设备/权限/显示器、HDR/GPU、网络与原八包继续，不能以合成路径通过认定整体远控完成。


当前源码原生 all-targets 严格 Clippy 通过，日志 `/tmp/dogcodes-vt-current-strict-macos.log`；没有为测试目录问题增加产品回退、禁用实际 codec 测试或延长期限。

2026-10-27 补充有限对照：从当前源码重新取得测试产物，仅运行纯合成的关键帧编码用例。原可执行路径配合临时工作目录仍在约五秒初始化超时；同一 inode 的临时硬链接配合仓库工作目录通过（约 1.28 秒）；回到原可执行路径和仓库工作目录再次超时。该文件未列出扩展属性。由此排除工作目录以及文件自身扩展属性作为本次差异的解释，仍不能确认具体系统机制。编码器实际使用进程内 worker 线程，并非通过可执行文件相邻路径查找 helper；没有为此增加 helper 搜索或其它产品机制。证据 `/tmp/dogcodes-macos-vt-path-current.log` 和 `/tmp/dogcodes-macos-vt-path-current-result.json`；临时硬链接与进程均已清理。此前完整回归和生产合成的通过范围保持不变，没有重跑全套或访问真实桌面。

本机重现完整私有目录验证（在仓库根目录执行，仍运行默认全部测试；只适用于上述 macOS 环境对照）：

```python
import json, os, pathlib, subprocess, tempfile
build = subprocess.run([
    "cargo", "test", "-p", "dogcodes-remote-desktop", "--lib", "--locked",
    "--no-run", "--message-format=json",
], check=True, text=True, stdout=subprocess.PIPE)
artifacts = [json.loads(line) for line in build.stdout.splitlines()]
binaries = [item["executable"] for item in artifacts
            if item.get("reason") == "compiler-artifact" and item.get("executable")
            and item.get("profile", {}).get("test")
            and item.get("target", {}).get("name") == "dogcodes_remote_desktop"]
assert len(binaries) == 1, "需要唯一的当前源码测试产物"
binary = pathlib.Path(binaries[0])
with tempfile.TemporaryDirectory(prefix="dogcodes-vt-full-", dir="/tmp") as directory:
    executable = pathlib.Path(directory) / binary.name
    os.link(binary, executable)
    subprocess.run([str(executable)], check=True, timeout=60)
```
