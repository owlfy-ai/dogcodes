# 原租约内的视频运行异常恢复

> 后续进展：下文发现的原生旧显示器尺寸与手机旧画面输入问题已完成代码/本地修复，见 [输入几何](input-geometry-2026-10-26.md)；真实设备与 RTP 逐帧身份边界仍保留。

## 问题与参考

之前 Windows 桌面复制已有有限重建/GDI 回退，Windows/Linux 编码器也已有软件接管，但上层 VideoStream 报运行错误后，desktop 会直接结束视频，手机只剩图片回退。参考本地 RustDesk src/server/video_service.rs 的采集丢失后有限重建、达到窗口内次数上限后回退的思路，本轮补齐自己的租约内管线监督，没有复制实现或协议。

## 当前实现

- 对已经启动的管线运行错误，在同一显示器、同一租约和原 Source 授权内重建，不重新创建 Portal 会话。每次尝试前后校验停止标记、连接、锁屏及原 Source；macOS 额外使用不会弹窗的屏幕录制权限预检。
- 最近 30 秒最多三次尝试，延时 250/500/1000 ms；短暂成功不会清空这个窗口。恢复开始到新画面就绪共用 20 秒状态期限；失败后重复 warmup 不续期。底层原生调用仍由已有的有界 worker 持有并清理，20 秒不是对内核驱动退出的保证。
- 先推进 Publication 代次，再释放原生对象，阻断 teardown 回调；每次失败的初始化也推进独立代次。重新创建的流仍使用原 Source、最新清晰度请求和本次实际编码 profile。
- 输出 reset 后等待新的 decoder generation 与实际 codec 就绪；仅在接受新 IDR 后计为恢复成功。新源尺寸回写同一 resize 实例，即使用户清晰度序号不变，手机仍按 generation 重建。
- RPC 返回 recovery 的 restarting/warming/ready 状态和尝试/完成数。恢复期间不以普通十秒清晰度调整超时提前终止；切换输出的准入等待这轮恢复完成。
- 手机保留最后一帧和原租约，显示“电脑画面正在恢复”，在正确代次准备解码器；RTP 原轨道也确认新 ready。它有独立的 20 秒本地状态期限，重复状态不延长，超时或明确失败回到图片模式。新图片解码前不替换旧纹理。

没有加入旧版本兼容、数据迁移、用户粘贴方式选择或剪贴板恢复。

## 验证证据

- desktop 新增八项默认回归：等待匹配 ready/真实 IDR；先 fence 后 Drop；失败初始化的迟到帧隔离；三次上限；重建前后授权失效；等待/启动中租约退休；跨 warmup 的总期限；成功后仍保留最近故障窗口。
- 新增显式运行的合成原生编码专项：640×400 原流重建为 320×200，未 ready 时不出画面，ready 后输出新 SPS/PPS/IDR，随后调码率并继续输出。新流用全新的 OpenH264 解码器验证尺寸与帧。macOS VideoToolbox 与 Linux 后端均单独运行通过；此测试不采集屏幕，不注入键鼠，不调用安装的 Agent。
- macOS desktop 347 项通过、八项需显式环境的测试跳过；Linux desktop 347 项通过、十一项跳过，合成编码专项另行通过。macOS/Linux 严格检查及 Windows GNU all-targets 静态严格检查通过。顺带把 Unix 专用的 seed_run 测试 fixture 标记为 Unix，避免 Windows 测试构建的 dead-code 失败。
- 原生库 macOS 91 项、Linux 131 项通过（对应平台需显式环境的项跳过）。Windows 只做静态编译检查，未据此声称实际屏幕或 GPU 通过。
- 手机增加三个回归：状态不能重置期限；保留租约/旧纹理直到替代图片解码；恢复卡住后有限等待再回退。完整 Flutter 394 项通过、无跳过；三个修改文件 analyze 无问题。

本地证据 /tmp/dogcodes-video-recovery-{tests,mac-strict,acceptance,final-macos,final-linux,flutter-full,flutter-analyze}.log。最终日志覆盖早期编码和 fixture 等待问题修复后的结果；早期失败不计为成功证据。没有重启日常 Core，三个开发服务容器保持运行，也没有改动 RustDesk 参考仓库。

## 尚未完成

本轮覆盖已启动管线的 runtime error。初次授权/启动失败、显示器持续变化、重新协商失败不会在这里擅自重取权限或无限重建。真实设备移除/驱动崩溃、睡眠唤醒、各平台权限撤销、手机后台/P2P/TURN/WSS 换路仍需实机验收；合成故障不等于真实设备恢复验收。

审计发现 NativeController 的键鼠映射仍可能使用 Capturer 创建时保存的 Display 尺寸，原生显示器移动/缩放后需刷新；应在后续输入/几何对齐中修复，并核对最后一帧与用户输入的归属。Wayland 自身的动态 EIS 区域逻辑继续保留。HDR/原生光标、GPU 少拷贝和其它执行恢复/PC 功能工作也继续，整体目标未收口。


## 2026-10-27：手机丢帧后等待有效的新关键帧

核对本地 RustDesk 的 `src/client/io_loop.rs`，其接收循环对关键帧与普通帧分开处理，队列覆盖时请求刷新；本项目继续使用已有的有界队列与累计 ACK，不照搬它的协议或线程结构。发现手机 RelayVideoReceiver 有实际异步竞态：旧 IDR 正在解码时，队列溢出丢掉了后续参考帧；旧 IDR 完成后直接清除 needKey，使剩余依赖缺失参考帧的 delta 继续进入解码器。原测试仅检查 ACK，注释甚至把这些 delta 继续解码视为正确，未覆盖参考链是否仍然有效。

现在使用一个 receiver 内的参考状态代次。请求新关键帧或丢帧后保留队列中新 IDR 时推进代次；正在解码的关键帧只有在其间没有再丢参考帧时，才可恢复 live。保留的新 IDR 必须实际解码完成；有该 IDR 时不额外请求。被丢弃/等待期间跳过的帧仍累计确认，保持原发送窗口、ACK 重试、队列上限和半秒关键帧请求限速。

只修改手机 `lib/features/computers/relay_video.dart` 与对应回归：原积压用例补查 delta 不进入解码器、ACK 继续推进、后续新 IDR/delta 恢复；新增保留 IDR 尚在解码期间不能提前 live 的用例。两项在未改生产源码时分别以“调用数 3 而非 1”和“live 提前为 true”失败，修复后通过。相关恢复/接收共 17 项、完整手机 **446 项通过、9 项原生 opt-in 未启用**，全项目 analyze 和格式/差异检查通过。

日志 `/tmp/dogcodes-relay-key-recovery-{before,after,suite,analyze,format}.log`。这是可控异步解码/控制回执测试，未声称在真实手机上复现花屏或验收硬件解码。二进制 video 经 WSS 中继或 DataChannel 时共用此 receiver；独立 RTP 轨道由 WebRTC 解码，不经过本次修改。没有改 PC/服务端协议或持久格式，没有旧版迁移、真实设备/桌面/模型、日常 Core 操作或提交推送。八包范围及真实平台/弱网/背景恢复仍保留。


## 2026-10-27：RTP 统计读取不能无限阻塞视频恢复

复核控制反馈时确认：RelayVideo 的序号在 reset/rebind 后继续递增，旧累计 ACK 不能清除新代次帧，不为该处另改协议。发现另一实际缺口：手机稳态 RTP 循环直接等待 WebRTC getStats，没有期限；原生桥一直不回复时，后续续租、停帧检测、恢复都停在这次读取。二进制解码统计和 RTP 配置准备路径已有三秒期限，稳态入口漏接。

现在稳态 RTP 统计也限三秒；当前 peer 读取失败/超时按已有图片回退路径关闭原视频租约/纹理，保留业务连接。读取完成和失败均核对捕获的原 DesktopConnection；若换路已替代它，不把旧统计或旧错误用于新 peer。Future.timeout 不取消原生调用，迟到结果由原 Future 处理，不重发原调用，也不操作替代连接。没有新增超时配置、协议或迁移。

增加一个页面回归：真实页面、私有 Peer/独立链路及方法通道夹具，挂起 getStats 后在原期限和既有 renderer 清理期限内进入非视频新租约，继续发图片请求；核对业务连接保持、业务查询仍能回答，以及原统计迟到报错不关闭/重启替代连接。旧源码反例在“应创建第二连接、实际仍只有第一连接”失败；新源码对应资源四项通过。RTC 假服务同时改为尊重 open 的 video 参数，避免在明确请求图片时仍返回视频。

验证的两次修正单独记录：首版回归在 finally 内直接 await 广播流关闭，使 fakeAsync 没有机会继续调度，已改为与原有夹具一致的 unawaited + pump；只中断了核实身份的自有测试 runner。随后保留 renderer 已有三秒清理预算，并按实际协议断言图片请求不含 video=true（该字段为可选而非必须 false）。这些测试夹具修正没有修改生产恢复期限。原反例 `/tmp/dogcodes-rtp-stats-before-final.log`；夹具迭代 `/tmp/dogcodes-rtp-stats-{before,after,after-final}.log`；当前资源回归 `/tmp/dogcodes-rtp-stats-current.log`。完整手机 447 项通过、9 项原生 opt-in 未启用，全项目 analyze 无问题，格式和差异检查通过；日志 `/tmp/dogcodes-rtp-stats-{suite,analyze,format}.log`。

只改变手机页面与对应测试，没有运行真实 WebRTC/手机/屏幕或日常 Core、Agent。这里只验证可控的原生桥超时与现有恢复机制，不等于实际硬件解码、TURN/弱网和背景恢复验收；原八包继续，未提交推送。


## 2026-10-27：首次 RTP 渲染器创建与图片回退

首次 _startVideo 直接等待 RTCVideoRenderer.initialize，无期限；超时/取消归属只在换路候选渲染器路径覆盖。页面退出期间返回的 renderer，还会先等待并绑定旧屏幕流，再核对 session。此外，首次 RTP 渲染失败只是开始拉图片，没有结束已有 PC 视频租约，可能继续无用编码。

将候选已有的 ownedResource + 三秒创建期限提取为页面内共享方法，首次和候选使用同一实现，不引入新层级或配置。创建期间捕获原 session/DesktopConnection；失效或超时后返回的 renderer 只走已有一次性释放函数，不进入屏幕绑定。取得屏幕流后、绑定前再次核对原 owner，绑定完成后仍复核。RTP 启动失败走原 _fallback，先结束原视频租约，再以 video 未开启的新租约拉图片；不再次尝试 renderer，也不关闭业务 RPC。原五秒等 stream、三秒绑定与现有释放期限不变。

新增两个页面/方法通道用例，在改生产代码前分别复现创建永远不返回时没有图片替代连接、退出后旧 stream 仍被绑定。修复后验证旧视频租约结束、图片请求继续、迟到 texture 80 恰好销毁一次且从未绑定、无第二次创建/旧结果重启以及业务连接保持。测试需要先排空原生事件 listen/cancel 后再检查销毁回执；首轮方法通道/事件调度夹具没有完成取消，未把空 disposed 列表当作已释放。现用标准 MockStreamHandler 和一次 10 ms runAsync 排空平台事件，未延长生产期限。失败及夹具迭代 `/tmp/dogcodes-rtp-startup-{before,after,current,disposal-diagnostic}.log`；两个新回归通过记录 `/tmp/dogcodes-rtp-startup-event-drain.log`。

完整手机 449 项通过、9 项原生 opt-in 未启用，全项目 analyze 无问题，格式/差异检查通过。完整回归 `/tmp/dogcodes-rtp-startup-suite.log`，最终分析 `/tmp/dogcodes-rtp-startup-analyze-final.log`，格式 `/tmp/dogcodes-rtp-startup-format.log`。首轮分析仅在新测试两处 if 提示补花括号，已修正，没有改变行为或关闭规则；首轮分析日志保留。本轮仅修改手机页面与新增 rtp_startup_test.dart；复用已有受管资源与图片回退，不改 PC/服务协议、持久格式或旧版迁移。没有运行真实屏幕/键鼠、手机/模拟器、WebRTC 原生解码或日常 Core/Agent，未提交推送。真实设备/网络和原八包范围继续。

## 2026-10-27：手机本地解码恢复期间的输入

已有输入保护只采用 PC 管线恢复状态。手机二进制视频解码失败、丢帧等待新关键帧时，接收器虽然已经 `live=false`，页面仍允许点击旧纹理；PC 的正常状态轮询还会继续放行输入。私有页面/原生通道反例确认：一次已知失败后，点击使发送记录从 2 项增至 4 项。

接收器现在仅在实际 live 状态改变时通知原页面。页面把 PC 恢复状态与当前接收器 live 一起交给已有 `RemoteInputQueue.setViewReady`，新接收器的首帧也须获证后才接受输入。失去参考链时立即丢弃尚未发出的旧画面意图，并释放本队列已发送/在途的按住状态；普通 PC ready 不能解除本机解码保护。只有当前参考状态的新 IDR 实际解码成功才恢复，旧 IDR 的迟到结果仍遵守已有代次检查，已销毁接收器不再通知。

页面显示“画面正在恢复”，同时停止旧画面上的点击、文字和光标预测；缩放/平移以及必要的按键释放仍沿用原规则。恢复不重放丢弃的点击、不新建租约、不改变输入序号或半秒关键帧请求节奏。监听回调核对原 session/屏幕代次和接收器对象，不能解冻替代会话。换路成功后同步当前输入状态，保留原输入队列与传输归属处理。

新增页面回归覆盖实际通道 decode 错误、立即释放拖拽按键且只释放一次、健康 PC 轮询不解冻、IDR 尚未输出时继续拒绝点击、成功后接受新输入且不重放。新增接收器回归确认销毁后的迟到输出不发布 ready，原错误恢复回归加强状态转换断言。扩展拖拽测试初版误以为最后一项必为按下，实际还有随后的移动；改为核对最后的按钮事件后通过，没有修改生产手势顺序。

完整手机 **451 项通过、9 项原生按需测试未启用**，全项目 analyze、四文件格式和差异检查通过。反例 `/tmp/dogcodes-relay-picture-input-before.log`，专项 `/tmp/dogcodes-relay-picture-input-current.log`，最终完整验证 `/tmp/dogcodes-relay-picture-input-suite-final.log`、`/tmp/dogcodes-relay-picture-input-analyze-final.log`、`/tmp/dogcodes-relay-picture-input-format.log`；首轮完整套件的拖拽断言失败保留在 `/tmp/dogcodes-relay-picture-input-suite.log`。

本次只改手机二进制视频接收与页面；DataChannel 和 WSS 二进制传输共用该接收器，独立 RTP 轨道继续使用原 WebRTC 恢复。没有改 PC/服务端协议或持久格式，没有旧版迁移，也未执行真实键鼠、原生硬件解码、手机/模拟器、真实屏幕/Agent 或日常 Core。真实设备/弱网验收与原八包继续，未提交推送。

## 2026-10-27：RTP 重置清理应用层旧帧

核对换路时确认，原租约的输入序号准入已有实现与回归，正常同源换路允许继续操作，不把它改成新会话或重发输入。本轮确认的缺口是 PC 输出：`VideoSender` 未覆写 `VideoOut::reset/retire`，因此管线重建、切屏或换路时，应用层八帧队列仍可能继续发送旧内容。底层 `TrackLocalStaticRTP::write_rtp` 还会等待 WebRTC driver 的有界事件通道，旧帧的一部分可能停在等待容量的 Future 内。

现在每个应用层排队帧带发送器内的代次，重置以已有 Tokio watch 通知同一发送任务：丢弃旧代次队列，优先停止尚未完成的旧包入队，并清理分包器缓存的 SPS/PPS。同步请求新关键帧；SSRC、序号和采集时间戳时钟继续，不把新流伪装成一次序号重新开始。PC 的 reset 与 retire 都接入这个处理。原八帧上限、GCC/NACK/TWCC、编码器和线上的 RTP 格式不变，没有新增协议层、依赖或迁移。

发送循环仅提取原有写入调用作为私有测试入口；生产仍使用同一个 `TrackLocalStaticRTP`。两个新回归直接执行该队列与 H.264 分包代码，验证任务开始前排队的旧帧被清掉、等待中的旧分片在 reset 后取消、同队列后续旧帧被跳过、新帧可发且 RTP 序号/时钟继续。既有默认 H.264 和 Baseline-only 两个实际 WebRTC 本机回环用例加强检查：两个排队旧帧的专用标记未出现在接收端，新代次标记到达，原媒体反馈与有序数据通道继续通过。测试数据为合成 NAL，不把收到 RTP 当成硬件解码验收。

macOS/Linux 传输各 9 项、desktop 各 350 项通过；分别保留一项 coturn 按需验收及 8/11 项桌面按需测试未启用。两端两包 all-targets 严格检查及 Windows GNU 两包静态严格检查通过，Windows 未执行二进制。专项 `/tmp/dogcodes-rtp-output-reset-current.log`；完整 `/tmp/dogcodes-rtp-output-reset-suite-macos.log`、`/tmp/dogcodes-rtp-output-reset-desktop-macos.log` 与 `/tmp/dogcodes-rtp-output-reset-linux.log`（含 Linux 严格检查）；其余严格检查 `/tmp/dogcodes-rtp-output-reset-strict-macos.log`、`/tmp/dogcodes-rtp-output-reset-windows.log`。

本次只清理**仍由本应用持有或等待入队**的帧/包；已经交给 WebRTC driver、pacer、重传缓存或网络的包无法由此撤回，RTP 没有本项目逐帧 generation。不能据此宣称手机显示绝无旧帧或真实弱网/切屏已经验收。独立 RTP 的实际呈现/输入时序、真实设备和 TURN/弱网仍在包 5/7；手机、服务端与 WebRTC vendor 源码均未改，没有屏幕/键鼠/模型、日常 Core 操作或提交推送。

后续本机 TURN 视频轨覆盖已补齐：UDP/TCP/TLS 与分片 TCP 的实际 RTP、并行 DataChannel 和 PLI 已在 macOS/隔离 Linux 验证，见 [TURN 实施记录](turn-stream-transports-2026-10-19.md#2026-10-27强制-turn-视频轨与并行数据验证)。这是传输验收，不替代上述实际呈现/输入、手机硬解或公网弱网边界。
