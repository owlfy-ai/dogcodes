# 持续坏帧恢复与桌面跨模式验收

日期：2026-10-25。接续 [RustDesk 对照](rustdesk-video-reference-2026-10-25.md) 与 [呈现/原授权](wayland-presentation-grants-2026-10-25.md)。代码独立实现，传输和手机协议不变。

## 持续坏帧不能无限显示旧画面

参考本机 RustDesk 的持续采集与错误分类思路，区分没有新像素、暂时坏帧和来源失效。静止桌面允许没有新 buffer，也允许空的 metadata-only 更新，不能以静止时长判定断流。

- Wayland 的 Header 或任一 plane 的 chunk 标记 CORRUPTED 时，丢弃整帧，并从首次损坏开始保留 3 秒恢复期限。连续坏帧不会延长期限。
- chunk 标记在 CPU payload 读取、mmap 和 GPU 导入之前检查，包括双平面 NV12 的第二平面。DMA-BUF 的零 chunk size 本身不是损坏；空更新不启动新期限，也不能清除已有的损坏期限。
- 实际成功读取并完成呈现归一化的新画面清除期限。持续损坏超过期限则结束来源及原 portal Session，回报“Wayland 屏幕画面恢复超时，请重新连接”。现有 GPU 重新协商沿用相同的有界恢复机制。
- 来源保留首次错误，随后资源退出的错误不覆盖最初原因。JPEG/视频最终输出仍核实来源，因此不能绕过已失效的来源发送缓存像素。
- `ScreenSource::check_active` 改为必须显式实现，新增来源必须说明自己的授权生命周期；无状态合成来源显式返回有效。

这不是对所有采集停滞的通用检测：没有新帧可以是正常静止；GPU 原生调用挂起隔离、来源重建与真实设备恢复仍须继续。

## 不再依赖已退役的 loginctl 测试替身

Linux 生产状态查询已改为 login1 D-Bus，原桌面跨模式验收的 PATH/loginctl 替身不再适用。新增入口：

```sh
python3 scripts/desktop_codec_acceptance.py
```

启动器构建并定位真实 native/desktop 测试 binary，创建独立 `dbus-run-session`、私有 runtime/config/data/cache 与随机所有权标记，只在该私有 bus 上提供 login1。清除实际 display/PipeWire 环境，验证就绪身份；服务启动和测试有期限，结束时只回收自己的进程。构建期间保留 Cargo 进度与诊断，避免长时间无输出。

默认运行桌面层 `same_synthetic_source_rebuilds_codec_in_both_directions_and_gates_real_h264`：同一 1600×1000 合成来源在 Auto→CB3.1→Auto 重建编码器，检查 ready/IDR 门控、真实 H.264 解码/SPS/尺寸及来源释放。`--fixture-only` 仅供诊断服务启动，不替代默认验收。desktop workflow 已加入默认入口；此前 workspace 单元测试负责预先构建。

## 本地证据

- macOS remote-desktop 43 项与严格 Clippy 通过；Windows GNU all-targets 严格 Clippy 通过，仅静态检查。
- Ubuntu 22.04 / 原 PipeWire 0.3.48 与较新 Linux / PipeWire 0.3.65 各 63 项、6 个 opt-in 默认忽略，严格检查通过。
- 两套 Linux 的真实私有 NV12 双平面来源分别验证：短暂 Header 损坏后恢复；随后连续空更新超过 3 秒仍有效；持续 Header 损坏退出；仅 UV plane chunk 持续损坏也退出；portal actor/PipeWire worker 最终归零。全过程约 11 秒。
- 较新 Linux 的默认桌面跨模式入口通过，另有 15 项桌面视频单元测试通过（1 个 opt-in，已由默认入口单独执行）。
- 两个独立只读文件覆盖对照：移除恢复期限后，持续坏帧未结束而触发 fixture deadline；将所有 `Ok(None)` 当成损坏后，静止空更新的活性断言失败。没有修改工作区源码来做反例；自有 target 随后清理该 crate 并用当前源码复验。

日志：`/tmp/dogcodes-codec-final-{mac,mac-clippy,windows,linux}.log`、`/tmp/dogcodes-corrupt-final-ubuntu22.log`、`/tmp/dogcodes-corruption-final.log`；反例为 `/tmp/dogcodes-{corrupt,empty}-before.log`。旧试验中的编译警告/错误不计通过，以上列出的最终记录为准。

## 继续范围

运行中最终尺寸/portal 逻辑几何变化的自动接续、Linux 硬编、Windows 原生纹理采集/零拷贝、GPU 挂起/设备恢复、静态文字补编码预算与真实解码质量、ABR/弱网/消费背压仍继续。平台/手机真机、权限/DPI/多屏/锁屏、TURN/地区网络、推送和官方部署，以及原目标的全部 PC 功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次均保留。

没有切换日常 Core、调用真实模型或访问用户桌面/键鼠/剪贴板/模拟器，原有三个 Docker dev 服务未变。

后续更新：[编码前余量与恢复归属](video-output-admission-2026-10-25.md)扩展默认 launcher 到同模块的两项 owned 验收，同时覆盖实际编码→RelayVideo→ACK 的满窗/最新画面/IDR 收尾。`--fixture-only` 仍不能代替默认完整入口。
