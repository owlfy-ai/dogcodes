# 全部 Agent 初始化实测与进程复用

日期：2026-09-28。本机 macOS，开发构建。

## 测量口径

对当前安装并注册的 8 个真实 Agent 逐一执行就绪检测和两次新建会话。
使用临时工作目录、默认模型与权限，以及无工具的本地 MCP 协议夹具。
没有发送模型提示词，也没有修改项目文件。版本如下表。

`detection` 包含原生协议就绪检测与驱动构造，不包含安装路径发现或版本命令。
`new_session` 是驱动 `create_selected_session` 返回耗时，不是用户发送到首字的总耗时。
两次会话使用同一个驱动及工作目录，第一份 handle 保留至第二次创建完成；
原生驱动可能自行串行化或退休旧进程，因此第二次耗时不能直接等同于产品下一轮耗时。
每组数据是单次样本，存在网络、原生缓存和机器负载波动。

## 优化前基线

单位：ms。Cursor 已包含上一轮保留就绪探测连接的优化。

| Agent | 安装版本 | 检测 | 首次建会话 | 第二次建会话 |
| --- | --- | ---: | ---: | ---: |
| Codex | 0.158.0-alpha.2.1 | 1541 | 147 | 123 |
| Claude Code | 2.1.282 | 1149 | 260 | 296 |
| Cursor | 2026.9.23-86fc751 | 8635 | 3199 | 3273 |
| Grok | 1.0.41 | 4154 | 276 | 116 |
| Kimi | 2.1.1 | 709 | 449 | 62 |
| CodeBuddy | 2.158.0 | 5914 | 999 | 18 |
| ZCode | 0.16.9 | 873 | 827 | 136 |
| Antigravity | 1.2.12 | 8212 | 8565 | 7995 |

### Antigravity 的额外边界

该驱动返回会话 handle 时原生初始化尚未完成，不能把 8565ms 当作完全就绪。
本次日志进一步显示：

- 首次审批 Hook 验证缓存未命中，耗时 8562ms。
- 启动子进程约 0ms，随后原生 `native_init` 耗时 6502ms。
- 合计约 15.1 秒才到首次原生初始化完成，不含模型推理。
- 第二次 Hook 验证缓存命中，约 0ms；主要耗时为等待前一进程退出的 `spawn_lock`（7994ms）。

安装版本的 CLI 帮助中未找到可验证有效 Hook 的轻量离线模式。
已有的 5 分钟验证缓存继续保留。不能跳过有效 Hook 验证或凭证写入串行保护来换取表面速度。

## 本轮改动与复测

Grok、Kimi 的 ACP 连接与具体工作目录无关，继 Cursor 后，注册时直接保留
`connect_ready` 返回的已验证驱动。正常会话仍重新传入工作目录、MCP、模型及权限设置。

ZCode 新增 `connect_ready`：验证 session/list、临时会话创建、模型目录和临时会话关闭后，
保留 app-server 连接。探测会话不加入任务会话映射。缓存进程已退出或读取通道关闭时，
下次操作重新启动连接。原有独立审查驱动仍保持隔离，不共享任务进程。

| Agent | 优化后检测 | 优化后首次建会话 | 优化后第二次建会话 |
| --- | ---: | ---: | ---: |
| Grok | 271 | 137 | 130 |
| Kimi | 454 | 96 | 369 |
| ZCode | 732 | 141 | 148 |

Grok 检测下降还包含省去探测结束后的进程退出等待；Kimi 第二次建会话有明显波动，
因此不把首次 96ms 描述为稳定上限。以上对照支持消除重复进程与握手，但不代表首字耗时同幅下降。

其余 Agent 的决策：

- **Codex / Claude Code**：首次建会话分别约 147/260ms，本轮没有引入额外预热或进程池。
  Claude 测试配置了 MCP，因此覆盖真实控制通道初始化。
- **Cursor**：延续上一轮连接保留，原生新会话调用仍约 3.2s，检测时临时会话仍约 8.6s。
- **CodeBuddy**：进程绑定启动 cwd，检测进程不能直接服务任意项目。
  保留探测进程会把退出等待移入首次任务路径，因此维持原实现。
  同驱动第二次 18ms 仅是协议实验；当时 Core 不复用 CodeBuddy 的存活会话。
  后续已将正常完成与取消分开处理，见下文。
- **Antigravity**：主要瓶颈在原生启动、Hook 验证和串行退出等待，本轮维持验证与保护机制。

## 重现与验证

基准源码：`apps/desktop/src-tauri/examples/initialization_benchmark.rs`。
输入为安装元数据 JSON 数组，字段为 `providerId`、`installationId`、`executable`、`version`，
不需要服务访问令牌或用户提示词。

```sh
cargo build -p desktop --example initialization_benchmark
target/debug/examples/initialization_benchmark /tmp/installations.json kimi
DOGCODES_BENCH_RETAIN_PROBE=1 target/debug/examples/initialization_benchmark /tmp/installations.json kimi
```

Cursor 默认保留检测连接；环境变量只用于对照 ACP 和 ZCode 的两种工厂路径。
单次建会话上限 60 秒，清理上限 15 秒，建会话或清理失败返回非零状态。
Antigravity 的原生初始化时间还需结合 stderr 阶段日志。

自动回归覆盖：ZCode 就绪进程复用、探测会话隔离、进程退出后恢复、缺少模型时拒绝注册；
ACP 驱动回归和桌面库回归。真实测试仅覆盖初始化，未重新执行模型首字或前端渲染基准。

验证结果：ACP 33、ZCode 18、桌面库 82 项测试通过（共 133 项）；
ACP/ZCode 严格 Clippy、修改文件格式检查、桌面程序和基准示例构建通过。
服务确认空闲后已切换至新构建，8 个真实 Agent 均成功注册。

## 对照 codex-host 后的进一步优化

参考本机 `~/Workspace/opensource/codex-host`，只读对照，未修改参考项目。
主要来源：

- `packages/adapters/codebuddy/src/session.ts`：正常完成后保留 client，取消后 `#resumeAfterCancel` 重建并恢复。
- `packages/adapters/codebuddy/src/acp-client.ts`：一个原生进程绑定一个 Session，关闭时等待 1 秒后强制结束。
- `packages/adapters/antigravity/src/antigravity-adapter.ts`：用 `agy models` 检测；每轮启动进程。
- `docs/harnesses/antigravity/antigravity-tool-approval.md`：只提供原生 Skip permissions，
  不验证全工具审批 Hook，私有 Hook 只处理提问。

### CodeBuddy：只在需要时重建

原来为避开原生取消状态残留，所有轮次结束后都关闭连接，造成成功轮次也重复启动。
现在允许支持原生恢复的 CodeBuddy 保留正常连接，沿用已有 ACP 会话绑定检查：

- 同一工作目录、MCP 和会话绑定不变时复用；模型、思考强度和权限仍执行校验/配置。
- 取消、非正常 stop reason 或协议失败后标记连接需要重建，下次使用前等待旧连接结束，
  再通过原生 `session/load` 恢复。用户取消与成功响应竞争时也强制重建。
- 识别 `_meta["codebuddy.ai/outcome"]` 中的取消/失败，不把 ACP `end_turn` 一律当成功；
  `PARTIAL_SUCCESS` 保持正常结束语义。
- 切换工作目录仍重建进程，不把探测时的 cwd 当成用户项目，不创建项目预热池。
- 在发出终态事件前更新连接复用状态和运行状态，避免下一轮读到未完成的收尾状态。

真实 CodeBuddy 2.158.0 四轮测试：首次建会话 **954ms**；成功后两次恢复准备分别
**1ms / 0ms**；取消后准备 **1147ms**。测试检查连接身份、正常复用、取消后更换连接，
并用随机标记验证第二轮和取消后的第四轮仍能回忆原生上下文。四轮均通过。
工作目录为临时目录，无 MCP、无工具操作；这些数字不含模型生成时间，不能当作完整 UI 首字耗时，
也不代表首次新项目从 954ms 降到了 1ms。

### Antigravity：没有找到可等价迁移的启动捷径

`agy models` 本机命令单次实测约 **6116ms**，并非轻量离线探测，且不证明 stream-json 协议可用。
当前安装发现也已使用该命令确认产品身份，直接替换 readiness 会降低协议验证覆盖。

codex-host 没有我们的全工具桌面审批，因此省去了有效 Hook 检测；这是功能取舍，不能作为同等审批下的性能对照。
dogcodes 继续保留有效 Hook 验证、5 分钟缓存和凭证生命周期串行保护。
现有同一活动会话本就复用原生进程；没有改成 codex-host 的每轮重新启动方式。
没有因性能优化更改用户权限选择，也没有缩短凭证写入退出等待。

本轮验证：ACP 35、Core 41、桌面库 82 项自动测试通过（共 158 项），另有真实 CodeBuddy
四轮验收通过；ACP 严格 Clippy、桌面构建通过。空闲服务已加载新构建，8 个真实 Agent 均 Ready 并注册成功。
