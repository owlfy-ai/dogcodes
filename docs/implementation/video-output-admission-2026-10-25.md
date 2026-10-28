# 编码前余量、最新画面与恢复归属

日期：2026-10-25。接续 [ACK/QoS](video-relay-qos-2026-10-25.md)，参考本机 RustDesk `src/server/video_service.rs` 的取帧/消费反馈思路，独立实现本项目的入口与恢复协议；手机及服务端生产协议不变。

## 编码前拒绝，而不只在发送时丢弃

原先原生编码完成后才调用 `VideoOut::push`。窗口/队列已满时，编码工作仍继续；每次拒绝又设置 lost，owner 会反复请求无法发送的 IDR。静止时的刷新预算不能约束这种额外恢复请求。

新增 `EncodeAdmission::{Wait, Frame, Keyframe}` 与 `VideoStream::start_from_with_admission`。既有 `start/start_from` 保持正常可编码的默认行为，桌面实际视频流通过当前 `Output` 查询下游：

- 二进制路径检查 ready、发送 channel 余量、八项在途窗口和原有字节预算。没有余量时停止实际输入的颜色转换/编码，仍更新单一最新未提交画面。
- RTP 路径检查真实本地写队列的余量与 writer 退休状态；链路码率继续由 WebRTC GCC 决定。这里没有用本地队列余量替代网络拥塞估计。
- 查询是有界的建议性余量，不预约容量；另一个生产者或已接受的异步回调可能抢先用掉位置。最终 `push` 仍非阻塞并保留原来的帧数、字节及大恢复帧硬上限。
- 编码器创建/能力准备仍属于启动就绪校验；背压拦截的是实际桌面输入，不跳过必要的能力校验，也不泄漏合成预热画面。

## IDR 请求属于当前输出

`Output`/`RelayVideo` 自己保留恢复状态，直到真实 IDR 被当前输出接受。发送失败后不再由另一个旧 lost 标记反复重新请求恢复。显式请求立即进入该输出的恢复状态；不再经由 Session/Relay 的独立旧请求位延后重开恢复。ready 重试保持幂等，profile 重新确认也在当前 Output 内建立恢复。

二进制恢复先等待旧累计 ACK 排空，再给出 Keyframe 余量，允许沿用单独大恢复帧的例外；部分 ACK 不丢掉请求。手机现有 `RelayVideoReceiver` 在失败/主动跳过时仍推进累计 ACK，因此没有要求“全部旧帧成功解码”才恢复。连接长期没有 ACK 的超时/码率制动机制继续运行，没有把未知状态当成已接收。

gate 捕获的是可替换的 Output，不冻结旧 wire。换路之后仍须新 ready、正确 codec profile 和新输出余量；旧输出恢复信用不能带到替换对象。跨编码器重建、原 lease/代次及授权边界保持。

## 暂停前的迟到输出不能成为恢复画面

新增 `video/flow.rs`，按暂停/恢复时的本机输入时间建立边界。`EncodedFrame.captured` 保留原 codec 输入时间，macOS 异步回调、Windows MF、软件编码均传递同一值；RTP 不再用回调时刻减延迟近似重建它。这不是物理显示器采样时钟，也没有新增 wire 字段。

边界之前的输出计作真实 codec 进度，但不能显示、也不能核销新的 IDR 请求。最新缓存重新提交时强制从 IDR 接续；恢复边界使用同一次输入时间，避免将第一张新输入误判为过期。原 stop/OS/portal 复核仍在最终输出之外，时间边界不提供或延长授权。

未准入的缓存输入不启动编码超时、不消耗静态刷新预算。暂停时重新保留 IDR 意图，等实际提交才开始其新投递期限；已提交 native job 的无输出期限继续生效，不能用网络背压掩盖真正的编码挂起。

## 原生在途任务

macOS 实际 Shared 管线只在 VideoToolbox 没有在途输入时提交下一项。计数从提交前开始，直到完整输出回调返回才归还，包括 dropped/error 回调；同步拒绝也归还计数。软件及 Windows 同步 worker/fallback 保持各自的同步信用。

测试按生产顺序先完成 encoder 初始化，再开始计算输出等待期限；冷启动和输出超时分开验证。真实合成 H.264 已验证一个输入无需后续输入或销毁 encoder 即可输出，阻塞回调时信用不提前归还。本轮没有保留试验性的缓冲扩容或不支持的延迟属性设置。

这约束 native 输入数量，不使 GPU 系统调用可中断；`encode_frame` 等原生调用本身若挂起，仍可能占住 encoder 锁；macOS 调用隔离、设备恢复和 teardown 的实机行为仍是后续工作，不能把异步回调期限当成此类系统调用的可中断保证。

后续已补 [macOS codec 的线程隔离](macos-codec-isolation-2026-10-25.md)：原生创建/调参/输入/销毁移入有界 worker，callback 不再直接发布，外层调用有期限且丢弃迟到结果；初始化和帧输出期限分开。上段描述的是此更新前的直接调用，ScreenCaptureKit 自身调用及真正设备/进程恢复仍未完成。

## 本地验证

- macOS remote-desktop 55 项通过，1 个质量测量 opt-in 默认忽略；包含真实 VideoToolbox 回调信用、缓存画面恢复/实际 H.264 解码及静态刷新不回归。
- Ubuntu 22.04/PipeWire 0.3.48、较新 Linux/PipeWire 0.3.65 各 74 项通过，7 个 opt-in 默认忽略。
- remote-transport 各 7 项通过，实际 mpsc 写队列信用、本地 Baseline/H.264/PLI 和可靠 DataChannel 用例通过；TURN 实例验收仍是单独 opt-in。
- macOS/Linux 完整 desktop 各 331 项通过；分别 7/9 个 opt-in 默认忽略。Linux 独立 login1 下两项验收已单独执行：跨 Auto/CB3.1 编码接续，以及实际 Source→codec→RelayVideo→累计 ACK 的满窗恢复。后者核对 ready、八帧在途、显式请求/部分 ACK 期间无 IDR churn、源在暂停时变亮、完整 ACK 后第九帧为当前画面的可独立解码 IDR，随后不重复重开恢复，并等待原 source 释放。旧/新 Linux 的私有 NV12 损坏/授权/收尾通过，旧 Linux 另验证裁剪旋转来源。
- native/transport、desktop 严格检查通过；Windows GNU 的 native/transport all-targets 严格静态检查通过，没有执行 Windows MFT 或 GUI。
- 只读文件覆盖的两个对照：移除编码前准入，缓存测试在“被拒输入进入 codec”的断言失败；取消时间边界，旧回调错误核销新 IDR 的断言失败。联调期间还复现旧激活请求位晚于首个 IDR 被消费、重开恢复并卡在一帧的竞态，修复后同一真实管线通过。工作区未用回退源码做两个文件覆盖对照，自有 target 之后以当前源码清理重编译。

最终日志：`/tmp/dogcodes-admission-final-{native-mac,native-clippy,desktop-mac,desktop-clippy,windows,linux,ubuntu22}.log`；对照为 `/tmp/dogcodes-admission-no-{gate,fence}.log`；对照后的当前源码复编/私有来源记录为 `/tmp/dogcodes-admission-restored-linux.log`。有早期失败/调试记录，不作为完成证据，以上最终日志及 `/tmp/dogcodes-admission-owner-final-{mac,mac-clippy,linux}.log` 为准。初次激活竞态记录为 `/tmp/dogcodes-admission-activation-before.log`，不计通过。

## 继续范围

简化链路中恢复偏慢与锯齿仍待调校，实际 P2P/TURN/WSS 的吞吐、抖动、控制/附件争用和 decoder 消费反馈待量化。动态最终几何、Linux 原生硬编、Windows 原生采集/零拷贝、GPU 挂起/设备恢复、硬件文字/色彩与平台/手机真机继续；原 PC 功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次、通知和官方部署范围全部保留。

未调用真实模型、切换日常 Core、启动模拟器或操作实际屏幕/键鼠/剪贴板，原有三个 Docker dev 服务保留。
