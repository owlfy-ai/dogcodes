# 通用 Computer Use 设计

日期：2026-10-31。状态：原始设计存档。第一阶段已经实现，实际架构、能力范围与验收见 [实现与验收报告](computer-use-implementation-2026-10-31.md)。

> 实施时按产品需求调整为任务级开启的宿主机模式，优先保证截图、输入、软件测试与结果验证。本文原先的逐应用授权、bounded manifest、严格 15 KB schema 和“唯一安全边界”等表述不代表当前实现。第一阶段实测 Codex 与 DeepSeek Harness；完整 12 个 agent 的验收仍属后续工作。

## 结论

- 底层采用 [Cua Driver](https://github.com/trycua/cua/tree/main/libs/cua-driver)（Rust，MIT，v0.38.0），作为固定版本的 sidecar 进程，不并入本仓库 workspace。
- dog.codes 不直接把 `cua-driver mcp` 暴露给 agent，而是复用 `--history-mcp` 的模式，新增 app 自有的 `--computer-mcp` stdio 代理。代理负责：精简工具清单、向 core 申请授权、转发给 cua-driver、上报审计事件。
- 授权以 dog.codes 为唯一权威：对我们自己的 MCP 服务器，各 agent 的原生审批一律自动通过（与 history 工具的 `automatic_approval` 同理），实际放行由代理向 core 查询授权决定。这样 12 个 agent 的审批语义差异不影响安全边界。
- 默认关闭，按任务开启，按应用授权。

[Open Computer Use](https://github.com/iFurySt/open-codex-computer-use) 只作参考：它的 Windows 实现每次调用都启动 `powershell.exe -ExecutionPolicy Bypass` 执行嵌入脚本，延迟高，易被安全软件拦截；但其 9 个工具的精简接口和 `docs/windows-input.md` 中的输入降级策略可借鉴。

## PoC 证据（Windows 11，2026-10-31）

| 项目 | 结果 |
|---|---|
| 源码构建 | 4.5 分钟，`cua-driver.exe` 38 MB。`regorus` 依赖的 `msvc_spectre_libs` 要求安装 VS 的 Spectre 缓解库，本机未装，PoC 用本地替身绕过；正式构建需在 CI 安装该组件 |
| 遥测 | 默认开启；`DO_NOT_TRACK=1` 时 `telemetry status` 为 disabled，不创建安装 ID |
| MCP stdio | `initialize` 54 ms；`get_window_state` 180–320 ms；`type_text` 107 ms（UIA ValuePattern，自带回读确认）；截图 646 ms / 88 KB；中文正常；前台窗口与鼠标位置不变 |
| 工具清单 | 63 个工具，schema 212 KB（约 5 万 token）；`bounded` 模式不缩减 `tools/list` |
| `bounded` 授权 | 清单外窗口返回 `bounded_resource_outside_manifest`，可作为宿主授权的执行层 |
| Claude Code 2.1.309 | `--strict-mcp-config` 注入，6 轮 / 27 秒 / $0.28 完成追加文本，前台不变 |
| Codex 0.164 | 自带 `unified-computer-use@openai-bundled` 插件会被优先使用，且自带应用审批拒绝了记事本；禁用插件并改名服务器后，写操作仍被 Codex 的 MCP 审批拦截；设置 `default_tools_approval_mode="approve"` 后完成，中途出现一次重复插入后自行修正，54 秒 / 约 6 万 token |
| CLI 单次调用 | `cua-driver call` 每次是独立连接，快照 token 不能跨调用使用（`stale_element_token`），所以 CLI 方式不适合多步操作 |

## 架构

```
agent (Claude/Codex/ACP/...)
   │ stdio MCP（服务器名 dogcodes_cu_<随机12位>）
   ▼
dog.codes.exe --computer-mcp <127.0.0.1:port> <token>   ← 新增，与 --history-mcp 同构
   │  ① tools/list：返回精简后的工具清单
   │  ② tools/call：先向 core 申请授权（loopback + token）
   │  ③ 放行后转发给子进程 cua-driver mcp，结果原样返回
   │  ④ 上报审计事件
   ├──────────────► core ComputerUseEndpoint（授权、租约、串行化、审计）
   ▼
cua-driver mcp（子进程，bounded 模式，manifest 由 core 生成）
```

### 为什么由代理启动 cua-driver

- cua-driver 的快照（`snapshot_id` / `element_token`）存在于进程内，必须让一个 agent 会话独占一个长期连接。PoC 已证明跨连接会失效。
- 代理随 agent 的 MCP 生命周期启停，agent 退出时 cua-driver 一起退出，不留常驻 daemon。
- 换掉底层实现（自研或换别的项目）时，只需要改代理，agent 看到的工具接口不变。

### core 侧：ComputerUseEndpoint

参照 `crates/core/src/history_bridge.rs` 的 `HistoryEndpoint` / `HistoryLease`：

- **租约**：每个 turn 获取 `ComputerUseLease`，turn 结束或任务撤销时失效。失效后代理拒绝一切调用并结束 cua-driver 子进程。与 history 一样，描述符不持久化到任务/会话投影中。
- **授权接口**：代理发送 `{token, authorize: {tool, target: {pid, window_id, exe}, delivery_mode}}`，core 返回 `allow`、`deny{reason}` 或 `pending{id}`。history 的 3 秒请求超时不适用，因为授权要等用户点击：`pending` 由代理长轮询，用户无响应 120 秒后按拒绝处理，并向 agent 返回可读的拒绝原因。
- **manifest 生成**：core 把已授权的应用/窗口写入 lease 目录下的 YAML（version 3，带 `expires_after` / `idle_timeout`，无 BOM），通过 `CUA_DRIVER_CAPABILITY_MANIFEST_FILE` 传给子进程。即使代理逻辑有漏洞，cua-driver 自身也会拒绝清单外的目标，形成双层保障。授权变更时代理重启子进程（快照随之失效，agent 会收到 stale 提示并重新读取）。
- **全局串行**：同一时刻只允许一个任务操作桌面（cua 文档的规则也是"一个共享桌面一个控制者"）。另一任务请求时返回 `deny{reason:"desktop_busy"}`。
- **与手机远控互斥**：`remote-desktop` 有活跃的远程输入会话时，暂停 agent 的输入类工具（读取类仍可用）。远程端可以实时看到 agent 的操作，这是一个可宣传的组合能力。

### 驱动接入：从 `history_mcp` 推广为 `app_mcps`

`driver_api::SessionRequest.history_mcp: Option<HistoryMcp>` 改为 `app_mcps: Vec<AppMcp>`（`HistoryMcp` 改名为 `AppMcp`，增加 `kind: History | Computer`）。`native_servers`、`codex_config` 等辅助函数改为接受切片。`supports_history_mcp()` 改为 `supports_app_mcp()`，语义不变。

| 驱动 | 覆盖的 agent | 改动 |
|---|---|---|
| `claude-driver` | Claude Code | 两个服务器写进同一个 `claude-mcp.json`；history 保持 `alwaysLoad: true`，computer **不设** alwaysLoad，交给 Claude 的工具延迟加载以降低上下文成本；`--allowedTools mcp__<name>` 免除原生提示 |
| `codex-driver` | Codex | `mcp_servers.<name>` 加 `default_tools_approval_mode="approve"`；启用 computer use 时设 `plugins."unified-computer-use@openai-bundled".enabled=false`，避免 Codex 自带插件抢占；插件键名随 Codex 版本可能变化，启动时用 `config/read` 校验，失败则不启用 computer use 并提示 |
| `acp-driver` | Cursor、Kimi、Grok、CodeBuddy、Kiro、OpenCode、DeepSeek Harness | `session/new` / `session/load` 的 `mcpServers` 数组追加一项，已有通道 |
| `antigravity-driver` | Antigravity | `approval_bridge.rs` 写入的 `mcp_config.json` 追加 |
| `zcode-driver` | ZCode | 已透传 `history_mcp`，随类型改名 |
| `pi-driver` | Pi | 不支持 MCP，第一期不提供；后续可评估 Pi 扩展机制 |

对仍会发出原生审批的 agent，core 在现有 `automatic_approval` 的位置增加 computer 服务器的匹配（服务器名精确匹配，规则同 `is_history_tool`），直接应答 ApproveOnce，避免与 dog.codes 自己的授权重复弹窗。安全性由代理和 manifest 保证，不依赖原生审批。

## 工具接口

对 agent 只暴露精简后的工具，schema 由代理改写（裁掉不开放的参数和冗长说明），目标总大小在 15 KB 以内：

| 开放 | 说明 |
|---|---|
| `list_windows`、`list_apps` | 只读 |
| `get_window_state` | 主观察手段；默认 `include_screenshot:false`，鼓励 `query`、`since:"latest"` |
| `click`、`type_text`、`set_value`、`press_key`、`hotkey`、`scroll`、`drag` | 输入类，需要目标已授权 |
| `run_steps` | 批量执行并只返回变化，节省轮次和 token |
| `launch_app` | 需单独授权（新进程） |

第一期不开放：`browser_*`、`page`、`clipboard_*`、`kill_app`、`run_script`、`set_config`、录制相关、`install_*`、`check_for_update`、感知扩展、会话管理、光标主题、`move_cursor`。`delivery_mode:"foreground"` 只有在任务授权为"可见操作"时才放行，否则由代理改为拒绝并说明原因。cua-driver 自带的 MCP `instructions`（约 1.5 KB）保留并附加 dog.codes 的规则说明。

## 授权与安全

**开关层级**

1. 全局设置：Computer use 总开关，默认关闭。
2. 任务级：在输入框或任务设置中开启，范围只限本任务。
3. 应用级授权：首次操作某应用时弹出 dog.codes 授权卡片（应用名、图标、可执行文件路径、操作类型），选项为"仅本次 / 本任务内允许 / 拒绝"。"可见操作"（会抢前台的 foreground 模式）需单独勾选。

**始终拒绝的目标**：dog.codes 自身窗口（防止 agent 替用户点掉授权卡片或改设置）、系统安全与凭据界面（UAC、Windows 安全中心、钥匙串/凭据管理器、系统设置中的隐私与安全）、常见密码管理器。名单按平台维护，命中时代理直接拒绝，不弹授权。

**紧急停止**：界面常驻"停止操作电脑"按钮，并提供全局快捷键。触发后撤销租约，代理结束 cua-driver 子进程，当前 turn 收到明确的中止结果。

**审计**：每次输入类调用记录一条任务事件（工具、目标应用和窗口标题、结果码、耗时），显示在会话活动中。`type_text` / `set_value` 的文本只记长度不记内容，与 cua 自身的遥测脱敏原则一致。

**提示注入**：窗口内容是不可信数据。代理在返回给 agent 的结果中不做内容改写，但授权卡片展示的是 dog.codes 自己识别的目标信息，而不是 agent 提供的描述。

## 打包与平台

- **版本与构建**：Tauri 资源中携带 `cua-driver` 可执行文件，固定上游 tag，在 CI 中三平台源码构建（Windows runner 需安装 Spectre 缓解库组件），记录 sha256；运行时校验哈希后再启动。体积约 38 MB（Windows release）。
- **进程环境**：`DO_NOT_TRACK=1`、`CUA_DRIVER_RS_TELEMETRY_ENABLED=0`、`CUA_DRIVER_RS_HOME=<app data>/computer-use`、`CUA_DRIVER_PERMISSION_MODE=bounded` 及 manifest 变量。不开放 `check_for_update`，版本只随 dog.codes 发布更新。
- **Windows**：PoC 已验证可用。UIPI 限制下无法操作以管理员身份运行的窗口，代理把 `background_uipi_blocked` 翻译成用户能看懂的提示。
- **macOS**：使用 cua 的 embedded 模式（`CUA_DRIVER_EMBEDDED=1`、`CUA_DRIVER_HOST_BUNDLE_ID`），让辅助功能和屏幕录制权限归属 dog.codes 本身。`remote-desktop` 已在申请同样的两项权限，引导流程可以复用。第三方资料称需要 macOS 15 及以上，尚未核实，也未实测。
- **Linux**：X11 与 Sway 有上游测试矩阵；GNOME 需要安装辅助组件，KDE 的输入目前被上游禁用。第一期标为实验性，未实测。
- **许可**：Driver 为 MIT，在 `THIRD_PARTY_NOTICES.md` 中登记。不安装、不分发 `cua-perception` 扩展（含 AGPL-3.0 组件）。

## 分期与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| P0 | `AppMcp` 推广；`--computer-mcp` 代理；ComputerUseEndpoint（租约、授权、串行、紧急停止）；Windows sidecar 打包；Claude 与 Codex 接入；设置开关与授权卡片；12 种语言文案 | Claude、Codex 各自在后台完成记事本与计算器任务；未授权应用被拒绝；dog.codes 自身窗口被拒绝；紧急停止在 1 秒内生效；工具 schema ≤ 15 KB |
| P1 | ACP 系 7 个 agent、Antigravity、ZCode 接入；审计事件在会话活动中展示 | 每个 agent 跑同一组脚本化任务并记录成功率、轮次与 token |
| P2 | macOS embedded 模式与权限引导；Linux 实验支持；与远程桌面互斥 | 三平台实机验收；远控输入期间 agent 输入被暂停 |
| P3 | Pi 方案评估；按需开放浏览器工具（需另行设计登录态与下载的授权） | 另行立项 |

## 风险与未决问题

- **上游变化快**：Cua Driver 仍是 0.x，且 Cua Cloud 已于 2026-10-18 停止服务。固定版本加上代理隔离接口可以降低影响；上游停止维护时，可以按同一接口换成自研实现（`remote-desktop` 已有截屏和输入能力，缺的是无障碍树读取）。
- **Codex 插件键名**：`unified-computer-use@openai-bundled` 来自本机 0.164 的会话记录，没有公开文档，需要每次升级 Codex 时回归验证。
- **模型能力差异**：PoC 中 Codex 出现过一次误操作后自行纠正。验收阶段需要按 agent 统计成功率，必要时在 instructions 中加入先读后写、按 element_token 操作、关键步骤后回读校验等规则。
- **上下文成本**：精简后的实际 token 占用要在每个 agent 上实测；Claude 的延迟加载能否对 computer 工具生效也要实测。
- **授权等待超时**：120 秒是初始值，需要结合实际体验调整；超时后 agent 的行为（放弃还是重试）要在各 agent 上验证。
