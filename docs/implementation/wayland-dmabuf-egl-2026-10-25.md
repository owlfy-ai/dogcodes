# Wayland DMA-BUF 导入与 Linux 基线兼容

日期：2026-10-25。继续原远控视频目标；不改变手机或服务器协议。本轮交付代码与隔离验证，不代表真实 GPU、桌面或手机已验收。

## 实现与边界

- 新增 `wayland/gpu.rs`、`gpu/egl.rs`：可选动态加载 EGL，通过 device/offscreen context 查询实际 DRM 格式和 modifier。每个 context 先以合成四角颜色验证 external image 的导入、通道与方向，成功后才声明能力；无可用设备/扩展时继续提供原 SHM 路径。
- 支持既有 10 种 packed RGB 与双平面 NV12 对应的 DRM 格式；保留至多四个平面以容纳 modifier 辅助平面，分别传递 fd、偏移、stride 和 modifier。隐式布局使用 DRM 的 56-bit INVALID 值并省略 EGL modifier 属性，不能用 `u64::MAX` 替代。NV12 传递 BT.601/709 及 full/limited 提示。
- 不将 DMA-BUF 按普通线性内存 mmap，不以 `chunk.size/maxsize == 0` 判断 GPU 帧为空。导入期间借用原 fd，完成同步 readback、销毁 image 后才归还 PipeWire buffer；fd 仍由原持有者管理。
- 当前 GPU 路径将 external texture 渲染到自有 RGBA FBO 并 readback，接回既有像素/编码管线。**这是 GPU 来源接入，不是采集到编码的零拷贝，也不是 Linux 硬件编码。**
- GPU 描述先协商格式/modifier，遇到未固定的选择先固定可导入项。真实导入失败后排除该格式/modifier，再尝试其它 GPU/SHM 参数；参数更新通过 loop channel 延迟到当前 buffer 归还后，避免在 process 回调内重入。排除次数与恢复等待均有上限，超过 3 秒明确结束来源，不能永久保留旧帧。
- EGL display 可能被多个 capture/context 共用。本地引用计数只允许最后一个 owner 调用 `eglTerminate`，避免关闭一条视频使另一条失效。每个资源析构先核实自己的 context，部分初始化与失败也回收 image、texture、FBO、program 和 buffer。

## Linux 旧运行库与来源身份

Ubuntu 22.04 的系统 PipeWire 0.3.48 仍作为运行基线；未用升级发行版来绕过兼容问题。

- 现用 libspa Rust 绑定需要较新的 raw video flags/unsigned modifier 头文件。新增 `scripts/prepare_linux_spa_sdk.py`，按固定提交和 SHA-256 准备 SPA 0.3.65 **头文件**及独立 pkg-config 路径；不替换系统 PipeWire/WirePlumber、不虚报运行库版本。只有原有 SDK 缺少所需字段时使用，desktop/release workflow 已接入。
- 下载与解包均有限额，只接受验证后的普通头文件，拒绝路径穿越和链接。受管目录有身份标记和文件锁；重复调用重新核实内容，正确缓存保持原样，受管损坏文件可修复，用户其它目录不覆盖。手动入口见 `release/README.md`。
- 旧管理器还按 `node.target` 路由。新增 `binding.rs`：在受限 portal FD 上核实被授权节点和 object serial，在连接后再次完成 core sync 才允许发布；来源删除或 ID 被复用后永久失效。保留现代 `target.object` 与禁止自动重连，避免重新选择其它来源。
- 老版本的部分 core done 回调没有 SPA async 位，现用受控 C hook 归一化序号，避免高层绑定断言导致 FFI abort。hook/data/events 地址稳定，同 loop 注销后再释放，回调不允许 unwind 进入 C。
- 移除通用 MAP_BUFFERS，避免它将 GPU 当线性内存处理；CPU MemFd 由 `memory.rs` 分平面只读映射，核实 fd、文件长度、页面偏移和大小，映射在原 buffer loan 结束前释放。MemPtr 仍只读，纯像素边界检查继续保留。
- 私有合成 producer 自主管理每平面的 FD 与完整 Y/UV 数据，避免依赖旧版测试分配器；私有 WirePlumber 禁用物理设备监视器。隔离 socket、D-Bus、目录标记及关闭清理保持不变。

## 本地证据

- macOS：remote-desktop 34 项测试与 all-targets 严格 Clippy 通过。
- Ubuntu 22.04：确认仍链接系统 PipeWire **0.3.48**，remote-desktop 46 项通过、5 项 opt-in 默认忽略，all-targets 严格 Clippy 与生产 lib 编译通过。
- 较新 Linux/PipeWire **0.3.65**：相同 46 项、严格检查及生产编译通过。
- 两个 Linux 环境分别独立执行五种真实 PipeWire 合成来源：RGB、默认 NV12、BT.601、BT.709、双平面 BT.709；均验证受限 FD → 320×240 像素 → 320/160/320 宽的 H.264 编解码及颜色。较新环境额外通过 SPA videotestsrc RGB 来源。
- 两个 Linux 环境的 `owned_egl_renderer` 使用软件驱动，实际执行 external EGLImage 渲染/readback、四角方向/颜色、resize 与同 display 多 context 的退出验证。在独立挂载中去掉 display 共享计数保护，已复现 invalid display；未覆盖工作区文件。
- Windows GNU all-targets 严格 Clippy 通过，只是静态证据。
- SDK helper 5 项测试、两个 workflow 的 YAML 解析与 diff 空白检查通过。

最终输出：`/tmp/dogcodes-ubuntu22-video-final.log`、`/tmp/dogcodes-bookworm-video-final.log`、`/tmp/dogcodes-video-mac-final.log`、`/tmp/dogcodes-video-mac-final-clippy.log`、`/tmp/dogcodes-video-win-final-clippy.log`。先前反例为 `/tmp/dogcodes-ubuntu22-before-sdk.log`、`/tmp/dogcodes-egl-refcounts-before.log`；中途失败的旧测试分配器和管理器试验不计为通过。

实际 GPU 的独立入口仍为 **未执行**：

```sh
DOGCODES_OWNED_EGL=dmabuf-synthetic-only cargo test -p dogcodes-remote-desktop --locked owned_egl_dmabuf_export_import -- --ignored --nocapture
```

该入口仅导出/导入合成纹理，不能代替真实 portal DMA-BUF、多 GPU、NV12 驱动或手机验收。MESA 导出按最多四平面的 modifier 数组处理，支持后续平面复用唯一 fd，并由测试 owner 回收导出的所有 fd；不将没有 GPU 时跳过算作通过。

## 继续范围

Linux 硬件编码、真实 DMA-BUF/多 GPU、原生 EGL 调用挂起隔离与设备恢复、Windows 原生采集/零拷贝/真实 MFT，以及 crop/transform/更多光标元数据、HDR/更高编码能力仍开放。全部 PC 功能对齐与操作恢复审计、metadata/辅助事务、Core 强退与跨物理代次，以及三平台/手机实机、真实 TURN/弱网/地区网络、推送凭据和正式部署均保留。

本轮未访问实际屏幕、键鼠、剪贴板，未切换日常 Core，未安装或运行模拟器，未调用真实模型。已有 server/postgres/redis 三个 dev 服务未改动。
