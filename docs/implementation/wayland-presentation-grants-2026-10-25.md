# Wayland 可见区域、方向与编码后原授权

日期：2026-10-25。继续 [RustDesk 对照](rustdesk-video-reference-2026-10-25.md) 的第一项；手机与服务器协议不变。本轮是代码和隔离验证，未操作实际桌面。

## 编码后仍须属于原授权

仅查询当前电脑未锁定，不足以证明原 portal 授权仍有效：编码期间可能撤销授权、结束来源或换掉物理 owner。

`ScreenSource::check_active` 提供不读取/推进像素的活性核实，Wayland 实现复用原 Session、来源错误及恢复期限。VideoStream 的最终输出同时核实 OS 和原 source；JPEG 在开始、unchanged 回复之前及压缩/base64 完成后核实同一来源。结束后不能借已有像素发出迟到输出；静态刷新也走相同的最终检查。每个来源实现都必须显式提供活性检查，避免后续新增来源漏写授权生命周期；stateless 合成来源显式返回有效。

私有 login1 在 OS 仍回覆 unlocked 时结束原 Session，分别验证 JPEG 和视频不再发布；VideoStream/来源释放后等待真实 portal actor 与 PipeWire worker 退出。

## Buffer 的元数据与呈现

对照 RustDesk `libs/scrap/src/wayland/pipewire.rs` 的 crop 使用，并补齐它在本机 `src/server/wayland.rs` 中明确保留的方向边界。

- `loan.rs` 独占一项原生 PipeWire buffer，和 Stream 绑定在同一 loop 线程；归还使用原 buffer，所有返回/错误路径都通过 Drop 归还。数据描述符上限 16、元数据上限 64，先核实数量及空指针再建立视图。
- `metadata.rs` 对已知 header/crop/transform 做有界 POD 副本，支持非对齐 payload；缺失可选项使用默认值，已提供但截断、空指针、重复或未知方向明确失败。指针不会跨越原 loan。
- header 标记 corrupted 时，在读取其它 payload、mmap 或 GPU 导入之前丢弃整个帧。SyncTimeline 来源需要显式 acquire/release，本轮明确拒绝，不能当成隐式同步。
- `geometry.rs` 在存储坐标中裁出可见区域，再归一化到展示方向；覆盖八种旋转/镜像组合。无有效 crop 的行为遵循 SPA region validity（任一边为零即未设置）；非空区域的负位置、越界和溢出都拒绝。普通整帧/无变换直接复用 image，不增加一份像素复制；裁剪/方向处理合为一次输出分配。
- crop/方向错误独立于 GPU 导入错误，不因错误区域而拉黑工作的 modifier 或重新协商 GPU。CPU 和 EGL RGBA readback 共用后处理。
- 连接建立时即声明 header/crop/transform 元数据需求，格式选择后继续维护。旧 PipeWire 的试验暴露了等待格式回调后才声明会错过初始缓冲区，现已提前声明并重复验证。

方向依据：[SPA videotransform](https://docs.pipewire.org/structspa__meta__videotransform.html) 描述已经施加于 buffer 的变换；枚举使用逆时针，呈现时须消除它。另核对了 [OBS 的 PipeWire consumer](https://github.com/obsproject/obs-studio/blob/master/plugins/linux-pipewire/pipewire.c) 的 crop/呈现顺序。代码独立实现：90° 元数据对应呈现时顺时针还原，镜像与旋转顺序不交换；不是仅交换宽高。

输入继续使用原授权 node 的逻辑坐标；像素归一化后不再对输入做第二次旋转或使用存储区偏移。

## 本地证据

- macOS remote-desktop 43 项及严格 Clippy；Windows GNU all-targets 严格 Clippy通过（静态检查）。
- Ubuntu 22.04 原 PipeWire 0.3.48、较新 Linux/PipeWire 0.3.65，各 62 项、严格检查及生产 lib 编译通过；5 个 opt-in 默认忽略。
- 两个 Linux 各七种私有来源：原 RGB/NV12 五种，新增 `rgb-crop-rotate90` 与 `rgb-crop-flipped90`。后两者存储为 320×400，包含 40px 位置、240×320 crop、非对称四角颜色及裁剪外的洋红色填充。实际产出 320×240，核对原像素、JPEG 和 320/160/320 宽 H.264 实际解码后的四角颜色。
- 同一来源的四个归一化输入角点发送到原 node 的 `(0,0)/(1727,0)/(0,1116)/(1727,1116)`，避免把存储旋转再施加于输入；均为私有 mock 记录，不是实机点击。较新 Linux 另通过 SPA videotestsrc RGB。
- 纯像素测试逐一核对八种方向的非对称编号矩阵、裁剪顺序及非法边界；元数据测试包括未设置、截断/空/重复、非对齐、损坏 header 先丢弃及显式同步拒绝。
- 三个独立只读挂载对照：移除 JPEG 原授权复核、移除视频原授权复核、忽略 geometry，分别复现 JPEG 授权结束后返回、视频授权结束后输出、320×400 未裁剪画面的断言失败。对照使用自有 Docker target 的单 crate 清理/重编译，不覆盖工作区；最终当前源码已重新编译复验。

入口：

```sh
python3 scripts/wayland_acceptance.py --format rgb-crop-rotate90
python3 scripts/wayland_acceptance.py --format rgb-crop-flipped90
```

desktop workflow 已加入两个模式。日志：`/tmp/dogcodes-source-geometry-{mac,mac-clippy,win}.log`、`/tmp/dogcodes-source-geometry-{ubuntu22,bookworm}-final.log`；对照为 `/tmp/dogcodes-{jpeg,video,geometry}-grant-geometry-before.log`。初始化元数据的旧运行库失败试验保留在 `/tmp/dogcodes-source-geometry-ubuntu22.log`，不计为通过。

## 保留范围

本轮验证固定最终布局及初始方向；若后续最终可见尺寸变化，现有 Source 仍明确结束并重连。自动更新 portal 逻辑几何、通知接收端并重建/接续的全流程仍需实现或核实，不能据此宣称热旋转/热插拔完成。还需持续坏帧/停滞的恢复核对。

真实 compositor/GPU/镜像/多 GPU、实际鼠标命中/DPI/多屏、权限撤销/锁屏与手机联调均待实机；Linux 硬编、Windows 原生采集/零拷贝、GPU 挂起/设备恢复、更多光标/色彩/HDR/编码能力、静态文字质量与弱网反馈仍继续。跨模式桌面合成验收的独立 login1 夹具也需接续（旧 PATH/loginctl 替身已不适用）。

原目标的 PC 全功能对齐、操作恢复、metadata/辅助事务、Core 强退与跨物理代次，以及真实 TURN/地区网络、推送与官方部署全部保留。未调用真实模型、切换日常 Core、运行模拟器或访问用户屏幕/键鼠/剪贴板，原有三个 Docker dev 服务未改动。

后续更新：[持续坏帧与桌面跨模式验收](video-corruption-codec-acceptance-2026-10-25.md)已补首次损坏开始的有界恢复、空更新区分、独立 login1 的 desktop 启动器及当前源码复验。动态布局与其它设备/业务恢复范围保持开放。
