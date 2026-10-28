# macOS 采集控制、取消启动与退出确认

日期：2026-10-25。接续 [codec 隔离](macos-codec-isolation-2026-10-25.md)，本轮将 ScreenCaptureKit 的控制与对象生命周期移入原生 worker。没有修改手机/服务端协议，也没有在宿主机采集实际屏幕、调用模型、启动模拟器或切换日常 Core。

## 参考与问题

本机 RustDesk HEAD `53996e3c4` 的 `libs/scrap/src/quartz/capturer.rs` 在停止后等待退出信号，再释放 stream 和 queue；`libs/scrap/src/common/quartz.rs` 将采集器与缓存画面消费分开。这里借鉴退出确认和所有权边界，独立实现 ScreenCaptureKit 的 actor、生命周期与错误处理，没有复制 CGDisplayStream 代码或协议。

此前 `Capture` 在调用方直接创建/配置/停止 SCStream；等待 completion 的五秒期限不能限制进入 API 本身或对象释放时的阻塞，而且 `Capture::error()` 总返回空。采集结束时，已进入编码器的旧图也没有来自原 SCStream 的直接失效信号。

## 实现

- `video/worker.rs` 分开采集与 codec 配额，各最多两项、每个队列最多一项；健康管线同时使用两个不同池，不会让采集占满编码器的恢复名额。配额跟随实际原生上下文直到销毁完成，外层句柄退出不提前返还。
- 新 `video/capture_worker.rs` 管理普通 Rust 的 facade、可撤销帧回调、尺寸元数据和生命周期。SCStream、Output/delegate、configuration 与 DispatchQueue 的创建、使用和释放留在同一原生线程；调用方不拿这些对象。
- 采集启动保留十秒总预算，调参确认限两秒。原生线程可继续等待系统真实回复，但外层有期限；不会把等待时间误当成初始化已成功、停止已确认或帧已输出。尺寸查询读取缓存，不调用原生 API。
- 读取 shareable content 前后及录屏启动前后都核实原请求；迟到 content 不能启动已经取消的录屏。`VideoStream::start_controlled` 接入原会话的 `Publication.stop`，取消检查不只依赖十秒超时。
- 原会话取消标志与当前 VideoStream 的局部停止标志分开。尺寸/profile 重建只结束旧流，不会将整条租约误关；现有跨编码与 ACK 验收继续覆盖这个边界。
- SCStream callback 通过 `Delivery` 找到当前帧消费者。退休会独立清掉真实回调，旧 native context 即使仍等待系统也不持有该发布消费者；不持锁等待正在执行的消费者结束。
- 新增 `SCStreamDelegate::stream:didStopWithError:`，系统停止/撤销录屏时记录首个错误并退休。退休钩子直接设置原 Shared 的停止标志与首错误，既有发布守卫会拒绝已经进入编码器的旧图，无需等 owner 下一次轮询。
- `set_fps` 使用真实 configuration completion，错误或超时进入相同退休路径。外层 stop/drop 不 join 原生线程；成功的 stop completion 或 delegate 才确认物理停止。停止报错但没有实际退出确认时，保守保留原生对象和名额。
- Capture 创建返回失败时，`VideoStream` 也设置同一停止/首错误状态；就绪阶段已有的失败流复活保护保持。

已经进入的系统调用不能被 Rust 强行中断。取消能阻止后续阶段与帧发布，不能承诺录屏指示器或物理资源立即消失。启动/更新/停止永不回复时，原线程持有上下文和配额；两个名额都挂住会拒绝新采集。进程边界与真正设备恢复仍需实现。

## 验证范围

新增六条可跨平台运行的采集控制器回归，并增加一条取消启动早于 OS 检查的 VideoStream 回归，覆盖：

1. `!Send` 原生状态在同线程创建、使用、销毁，尺寸元数据不重新调用驱动。
2. 取消/超时后的 content 回复不执行下一录屏阶段，不保留真实消费者；原生名额直到初始化实际返回才释放。
3. 采集和两个 codec 名额可同时占用，两个采集上下文占满后不再无限创建线程。
4. 配置卡住后，外层有界返回并清掉消费者；迟到帧不发布，原生销毁等待实际 stop 确认期间仍占名额；后续错误不覆盖首错误。
5. 系统停止、失败启动、迟到成功不复活消费者。回调消费者已在执行时，退休不等它，真正发布守卫仍拒绝其旧图。
6. 原租约在准备期间取消，同样不进入新的录屏阶段；入口已经取消时不触碰 OS 采集检查。

这些运行测试使用私有 Rust 采集夹具。真实 ScreenCaptureKit 创建、权限对话框、delegate 事件及驱动故障尚未执行，需要后续实机；不是用一般线程测试宣称 OS 行为已验收。实际 VideoToolbox 合成编码仍运行，测试不采集用户屏幕。

- 当前 macOS 原生库 66 项、1 项 opt-in 默认忽略；旧/新 Linux 原生库各 82 项、7 项 opt-in 默认忽略，通过。原生 all-targets 严格 Clippy 通过，Windows GNU all-targets 严格静态检查通过，没有执行 Windows GUI/MFT。
- macOS/较新 Linux 完整 desktop 各 336 项通过，7/9 个 opt-in 默认忽略，desktop lib/tests 严格 Clippy 通过。
- 较新 Linux 单独执行两项私有 login1 验收，跨编码接续和实际 Source→codec→RelayVideo→累计 ACK 满窗恢复通过；旧 Linux 单独运行 NV12、裁剪旋转两个来源，通过。未重跑旧 Linux 完整 desktop，其它 opt-in 未计通过。
- 四个独立源码副本分别移除准备后的取消检查、直接发布失效钩子、原生名额持有边界和父租约取消检查，对应单项测试均在预期断言失败。

## 构建缓存问题与最终证据

复验发现，多个源码副本共用 target 且复制时保留文件时间戳，可能读到正常版本产物。早先共用目录的反例日志不计入最终证据，不能仅看命令退出码认定加载了正确源码。

已使用 APFS 独立复制的构建目录，确认不是 live cache 的硬链接。**每个反例之前只在这个私有目录清理 remote-desktop 包**，核实原生测试产物已移除，再强制编译指定副本，记录编译路径、源文件摘要及预期失败用例；从未清理工作区的 live target。

本轮四项和上一轮 codec 四项共八项均已按此方式重编并复现失败。之后再次清理私有原生包、编译当前工作区，macOS 原生 66 项、完整 desktop 336 项及两者严格检查全部通过。上一轮 codec 的结论已有新的独立证据，不依赖旧缓存记录。

- 最终反例汇总 `/tmp/dogcodes-isolation-clean-witnesses.log`，分项 `...-clean-witness-{capture-late-prepare,capture-publication-hook,capture-native-slot,capture-parent-lease,codec-phase-deadline,codec-callback-credit,codec-native-lifetime,codec-readiness}.log`。
- 当前源码清理重编 `/tmp/dogcodes-isolation-clean-positive-{setup,native,native-clippy,desktop,desktop-clippy}.log`；私有 target 路径记录在 `/tmp/dogcodes-isolation-proof-cache-path.txt`。
- 平台复验 `/tmp/dogcodes-capture-isolation-{native-final,native-clippy,desktop,desktop-clippy,windows,linux,old-linux,final-controller}.log`。其中末次控制器复验还调整了失败夹具的通道关闭处理，防止断言失败时析构再次 panic；原行为断言与数量断言保持。

## 后续

优先补原生退役配额的有界等待与快速重建压力测试：当前配额满时直接拒绝，即使旧上下文即将退出；尤其已有一个挂住上下文时，另一个健康流重建也可能碰到这个边界。随后继续动态最终几何、进程隔离/真正设备恢复、Linux 原生硬编和 Windows 原生采集、真实手机/系统/弱网及共享链路争用。这里没有完成每一个原生调用或 CV buffer/OS 状态查询的隔离，也没有把采集线程隔离等同于驱动可中断。

原目标中的剩余 PC 功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次、通知与官方部署全部保留。原有三个 Docker dev 服务保留，没有创建 Git 提交或推送，也没有切换日常 Core。

后续：[退役配额等待与快速重建](native-retirement-grace-2026-10-26.md)已处理名额即将归还仍立即拒绝的问题，等待计入原初始化绝对期限、最多两秒宽限，配额上限保持；两个池各 24 次连续重建及实际 VideoToolbox 八次重建/独立解码通过。上文“配额满时直接拒绝”为该更新前状态，永久挂住/真实设备与其它范围继续。
