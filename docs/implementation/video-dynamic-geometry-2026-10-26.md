# 运行中画面尺寸变化与解码接续

本轮继续参考本机 RustDesk HEAD `53996e3c4`，独立实现本项目的接续逻辑，不复制其代码或协议。

## 参考和问题

- `src/server/video_service.rs::try_broadcast_display_changed`：重新核实显示器，通知接收端变化，再触发采集链重建；不能只将新尺寸塞入旧编码器。
- `src/server/wayland.rs` 的屏幕几何/输入范围处理：明确区分呈现方向下的物理像素、逻辑范围和显示器原点，不能简单除以缩放率混用。
- 本项目 Wayland 在有效新帧尺寸变化时直接关闭 Portal；软件采集也将其报告为终止错误。手机仅按用户清晰度请求序号接纳配置，不能识别同一请求序号下的来源尺寸变化。

## 实现

1. Wayland 接纳经过原有格式、范围、crop、transform、授权检查的新尺寸画面，保留原来源与授权。软件采集发现输入尺寸与本流初始尺寸不同时，设置粘性的 geometry 标记并退出该采集线程；新尺寸画面不交给旧编码器。
2. 标记同时进入编码准入与最终发布边界。最终边界在授权查询前后检查，阻止查询期间变化后才返回的旧 codec 输出；它不会把尺寸变化伪装成授权撤销。静止画面的刷新也不能继续编码旧图。
3. PC 视频 owner 先推进发布 epoch，退役旧流，重置传输视频代次，再用同一来源和最新目标宽度重建。更新实际输出尺寸及清晰度上限；不借用/增加用户的请求序号。来源变化不等待旧画面的 ACK，否则未显示过的旧配置可能永久阻塞恢复。普通用户清晰度调整仍遵循原 ACK 条件。
4. 二进制路径沿用新代次 ready 和 IDR 握手；RTP 沿用同一协商 profile 下的 WebRTC 解码路径。十秒窗口最多接续八次尺寸变化，持续抖动给出明确错误，避免无限重建。
5. 手机在相同实例、未处于 native updating、请求序号未倒退的前提下，也按更新的视频代次接纳配置。二进制解码器使用新尺寸创建，先订阅/确认 ready，得到新画面后才替换旧纹理；尚未被 PC 应用的原清晰度请求保持原身份。过期代次及其它显示器实例不能覆盖当前状态。

## Wayland 输入边界

[Portal 官方接口](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)要求指针使用流的逻辑坐标。新像素尺寸不能证明新的 `logical_size`，本轮没有用它覆盖 Start 返回的坐标依据。

同一 Portal 任一来源的像素尺寸改变后，逻辑指针坐标被标记为待重新授权。移动、按下/点击和滚动在入队前及 actor 执行前整批拒绝，给出重新连接提示；逐事件也复核。单纯拒绝整批不会关闭观看授权。松开键鼠和键盘仍可执行；已有释放通道保留。若变化发生在已部分执行的批次中，继续遵守原“执行中断结束本次远控、禁止重放”的规则。此标记保守覆盖该 Portal 的全部显示器，且只有新授权可恢复，尺寸恢复原值也不猜测坐标已可靠。

这意味着画面可接续，Wayland 鼠标几何尚不能完全自动更新。后续需引入可信的 compositor/EIS 动态区域与 stream mapping，或者显式刷新授权；不能宣称运行中转屏/DPI/布局的输入已经全部对齐。

## 本地证据

- 原生回归：软件采集尺寸变化不交付新尺寸给旧 sink、无伪终止错误并归还采集名额；授权检查期间变化不能发布旧帧；Wayland 坐标失效拒绝鼠标批次并保留释放和观看。
- 私有 login1 下的实际 Source → VideoStream → OpenH264 → RelayVideo：故意扣留全部旧 ACK，320×240 → 240×320 → 320×240 两次接续。请求序号和实例 ID 不变，代次递增；新 ready 前没有帧；旧 ready 失败；首帧 SPS/PPS/IDR 可独立解码并验证当前深/浅像素。随后撤销原来源，输出停止并释放同一来源。
- 手机页面测试：PC 自行改变尺寸/代次但 applied 序号仍为零，页面仍创建对应尺寸的新解码器；新画面出来前保留旧纹理，之后恰好释放一次。配置单测另覆盖更新中/旧代次/其它实例拒绝，以及未完成请求的保留。
- macOS 原生 73 项（1 项 opt-in 忽略）、desktop 336 项（7 项忽略）；较新 Linux 原生 89 项（7 项忽略）、desktop 336 项（10 项忽略）；三个 private codec 验收显式运行通过。原生 all-targets、desktop lib/tests 严格 Clippy 通过。Windows GNU 原生 all-targets 严格静态检查通过，未执行 Windows 程序。手机普通测试 339 项通过、9 项跳过，改动文件静态分析无问题。

日志：`/tmp/dogcodes-geometry-{native,mac-clippy,mac-desktop,mac-desktop-clippy,windows,mobile-all,mobile-analyze,mobile-widget}.log`；较新 Linux 最终记录 `/tmp/dogcodes-geometry-final-linux.log`，macOS 最终原生记录 `/tmp/dogcodes-geometry-final-mac.log`。早先 Linux 检查中的测试命名/导入 lint 已修正，最终日志为准。

没有运行真实屏幕/键鼠/剪贴板、真实 Agent 或模拟器，没有切换日常 Core，也未提交/推送；保留原有三个 Docker dev 服务。本轮未重跑旧 Linux 完整矩阵或实际 Portal 动态尺寸生产源，不能将合成 ScreenSource 等同为跨 compositor/GPU/手机实机验收。

## 后续

继续 macOS 原生动态几何、Wayland 可信输入区域更新、显示器物理断开/重接、GPU 设备丢失/进程恢复、Linux 硬编/Windows 原生采集，以及 P2P/TURN/WSS 实际弱网与控制/附件争用验证。剩余 PC 功能、操作恢复、metadata/辅助事务、Core 强退/跨物理代次、推送与官方部署继续保留，未以这项视频改动替代完整目标。
