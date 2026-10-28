# Wayland 可映射 NV12 来源与格式失效处理

日期：2026-10-25。范围是 PC 的已授权 PipeWire 来源，不修改手机/服务端协议，不访问实际桌面。

## 问题与实现

原采集只协商六种 packed RGB 格式，并读取第一个数据平面；NV12-only 来源无法连接，双平面的色度也没有读取。格式解析的返回值被忽略，新描述失败时可能继续使用旧描述。

现在支持：

- 保留 RGB 优先，补 ARGB/ABGR/xRGB/xBGR 与 NV12；NV12 支持连续 Y+UV 或两个独立的平面。各平面分别核实偏移、长度、正负行跨度及填充。
- BT.601/BT.709、full/limited 的 8-bit SDR 转换。显式元数据优先；省略字段时使用 BT.601 limited 兼容默认值，不按分辨率猜测。不支持的显式矩阵/范围报错，不冒充已正确转换。
- 按 SPA 约定将 chunk offset 对 maxsize 取模、size 限制在 maxsize 内；只读取/拼接当前图像需要的部分，不因为声明了很大的 chunk 而复制全部尾部内存。尺寸、跨度和长度在图像分配之前检查。
- 输入 MemFd/MemPtr 只建立不可变视图，避免 SDK `Data::data()` 在只读映射上构造可变切片。缺失 chunk、不可读/非 CPU 内存明确失败；任一平面为空或标记 corrupted 时丢弃整个帧。
- 每次格式协商先清除旧描述，再解析和校验新描述。初始未选定格式允许等待；活动格式被撤销或解析失败则结束当前来源，避免无限显示旧画面。原生具体错误优先于随后的通用“授权会话已结束”。
- buffer 协商只声明已实现的 MemFd/MemPtr；不将带 modifier 或 DMA-BUF 的来源当普通线性字节读取。生产路径继续只连接 portal 交给它的受限 FD，并按授权 object serial 定位。

PipeWire [chunk 定义](https://docs.pipewire.org/structspa__chunk.html)规定 offset 取模及 size 限制；[颜色字段](https://docs.pipewire.org/structspa__video__info__raw.html)分别描述矩阵和范围。未声明 NV12 的默认选择沿用 [libyuv NV12ToARGB 的 BT.601 惯例](https://chromium.googlesource.com/libyuv/libyuv/+/refs/heads/main/source/convert_argb.cc)，转换代码为本项目独立实现。实际采用其他色彩空间却省略元数据的来源无法自动推断，不能据此宣称所有来源色彩准确。

## 验证入口

`scripts/wayland_acceptance.py` 保留原 RGB 模式，并支持：

```sh
python3 scripts/wayland_acceptance.py
python3 scripts/wayland_acceptance.py --format nv12-default
python3 scripts/wayland_acceptance.py --format nv12-bt601
python3 scripts/wayland_acceptance.py --format nv12-bt709
python3 scripts/wayland_acceptance.py --format nv12-bt709-planes
```

每次创建独立 D-Bus、PipeWire 和 WirePlumber。NV12 producer 只编译在 opt-in 测试中，核对私有目录标记，通过该目录的 socket 提供合成红色帧；不使用系统屏幕或输入设备。双平面模式包含每平面 32-byte 偏移及 336-byte 行跨度，实际宽度为 320。每种来源均验证 portal FD → 像素 → 不同尺寸的 H.264 编码 → OpenH264 解码，并核对解码后的红色像素，以及既有授权/输入/关闭流程。CI 的 desktop workflow 已接入五种模式。

普通回归包括格式失败/撤销清空、只读双平面、第二平面损坏丢弃、非法内存类型/缺失 chunk、颜色矩阵/范围、负跨度、偏移填充、wrap/clamp 与截断尺寸。生产编译不需要测试 producer 的驱动时钟 API；仅 Linux dev-dependency 启用 PipeWire v0_3_34。

## 本轮证据

- Mac 视频及像素回归 34 项、all-targets 严格 Clippy 通过。
- Linux 回归 39 项通过，3 项原生/隔离入口默认忽略；其中 portal 与合成 producer 由上述五条入口独立执行，五条全部通过。Linux all-targets 严格 Clippy 与非测试 lib 编译通过。
- Windows GNU all-targets 严格 Clippy 通过，仅为静态检查。
- 在另一只读挂载路径覆盖 HEAD 的旧 capture 实现，使用同一 NV12 BT.709 合成来源复现“首帧超时”；没有替换工作区文件。新实现的同一验收通过。
- workflow YAML 解析及 diff 空白检查通过。未调用实际 Agent、安装/运行模拟器、切换日常 Core 或接触用户屏幕/键鼠；原有三个 server/postgres/redis dev 服务不变。

详细输出：`/tmp/dogcodes-nv12-color-roundtrip.log`、`/tmp/dogcodes-nv12-linux-verification.log`、`/tmp/dogcodes-nv12-mac-tests.log`、`/tmp/dogcodes-nv12-mac-clippy.log`、`/tmp/dogcodes-nv12-win-clippy.log`、`/tmp/dogcodes-nv12-before-witness.log`。较早 GStreamer 试验发现其当前版本未传递颜色元数据，已改用独立的原生合成 producer；失败试验不计为通过证据。

## 继续范围

本阶段只完成可映射的 8-bit NV12，不等于 DMA-BUF/EGL 导入、modifier/多 GPU、Linux 硬件编码、HDR/色彩管理、crop/transform/更多光标元数据或更高编码规格完成。Windows 设备丢失/零拷贝、全部平台与手机真机、真实 TURN/弱网/地区网络、推送凭据/官方部署仍需继续。

原目标的 PC 全功能/操作恢复核对、Agent metadata 超时与迟到回调、共享 Agent 自有资源、辅助事务/背压、Core 强退和跨物理代次也保留，不以本阶段替代。

后续进展：DMA-BUF/EGL 导入、独立 MemFd 映射及旧 Linux 基线兼容的代码与隔离验证见 [后续记录](wayland-dmabuf-egl-2026-10-25.md)。本节之前的“继续范围”描述本阶段交付时状态；真实 GPU/硬件编码与全目标验收仍未完成。
