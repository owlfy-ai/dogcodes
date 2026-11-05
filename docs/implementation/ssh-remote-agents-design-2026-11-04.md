# SSH 远程 Agent 技术方案

日期：2026-11-04。状态：P0、P1、P2 已实现（见 §12～§17），P3、P4 尚未开始；工作量为粗估。

## 1. 结论

**Core、数据库、Driver 和 UI 全部留在本机。远端只放一个自动上传、免 root、不注册系统服务的单文件 `dogcodes-shim`，通过一条 SSH 连接提供进程、文件、PTY 和回连通道。**

- 远端 Agent 在 Core 看来只是**另一个 Installation**。`ExecutorRef { provider_id, installation_id }` 和 `register_driver` 不需要改语义，切换 Agent、审批、分叉、审查、用量等上层逻辑可以直接复用。
- Driver 的协议代码不重写。它们本来就只通过 stdin/stdout 与 CLI 通信，改动集中在“进程从哪里启动”这一层：约 12 个 `OwnedChild::spawn` 调用点，以及 12 个引用 `ChildStdin/ChildStdout` 的文件。
- SSH 使用**系统 `ssh` 可执行文件**，不内嵌 SSH 库。这样 `~/.ssh/config`、ProxyJump、ssh-agent、FIDO、证书、known_hosts 都能直接用。
- Agent 的账号凭据**始终留在服务器上**，不从本机下发。

> **已确认（2026-11-04）**：“不需要安装 dogcodes”指用户无需手动安装，也不部署完整的 dog.codes；接受应用自动上传 shim。

## 2. 目标与范围

| 一期目标 | 暂不做 |
| --- | --- |
| 添加 SSH 目标（密码、私钥、系统默认认证、从 SSH config 导入） | Windows 作为远端（首批：Linux x64/arm64、macOS x64/arm64） |
| 自动发现远端已安装的 Agent，并与本地 Agent 同样管理 | 同一项目跨机器（本机与远端、远端与远端）切换 Agent |
| 远程项目中的完整对话：流式输出、审批、提问、恢复、切换 Agent | 远端 Computer Use（不注入） |
| Git 变更与审查、文件树、终端、运行项（P2） | 把本机 Agent 账号同步到远端 |
| 网络中断时 Agent 继续运行，恢复后接着收（P3） | 手机端新建 SSH 连接（手机可使用已有远程项目） |
| Docker / WSL 目标（P4，复用同一传输形态） | |

## 3. 现状：能复用什么，哪里绑死了本机

**可以直接复用的抽象：**

| 现有能力 | 位置 | 对远程的意义 |
| --- | --- | --- |
| Installation 级路由 | `ExecutorRef`（`crates/domain/src/lib.rs:154`）、`core.register_driver` | 远端 Agent 就是新的 Installation |
| 工作区 trait | `WorkspaceContextProvider` / `WorkspaceTurnCapture`（`crates/core/src/lib.rs:168,193`），Core 内约 7 处调用 | 可以换成远端实现，Core 不感知 Git 在哪里运行 |
| stdio 协议 | 全部 manifest 的 framing 都是 `*_stdio` | 协议本身可以穿过 SSH |
| 进程退出证明语义 | `OwnedChild`（`crates/process-scope/src/lib.rs`）：“Supervisor 丢失是错误，不是退出证明” | 远端同样适用：断线即未知，不伪造退出 |
| 运行状态 | `RunState::Disconnected / Reconnecting` 已存在 | 断线状态无需新增 |
| 同一可执行文件的分发模式 | `main.rs` 中的 `--history-mcp`、`--computer-mcp` | `--ssh-askpass` 可沿用同一模式 |
| 不重放 prompt 的恢复 | `session_recovery`、`recovery` | 断线恢复沿用 |

**绑死本机的地方（改动清单的来源）：**

| 耦合点 | 位置 | 处理 |
| --- | --- | --- |
| Driver 直接构造本地 `tokio::process::Command` | 12 处 `OwnedChild::spawn`，另有 `run_scoped_bounded` | `TargetCommand`（§6.4） |
| `OwnedChild` 的 stdio 是具体类型 | 12 个文件引用 `ChildStdin/ChildStdout` | 改为同时支持本地和远端的流类型 |
| 环境变量取自本机 | `inherited_variable`、`apply_proxy_environment` 约 25 处；`bypass_permissions_available` 看本机 euid | 由目标提供环境 |
| MCP 命令是本机 exe 路径 | `history_bridge.rs:473` | 改写为 shim 中继（§6.7） |
| Driver 写本机临时文件 | `claude-mcp.json`（`claude-driver/src/lib.rs` `--mcp-config`）、Codex `current_dir(temp_dir)` | 目标侧私有目录 |
| 读厂商本地状态 | `claude-driver/quota.rs`（`~/.claude`）、`acp-driver/request_usage.rs`（`~/.kimi-code/sessions` 等）、`codex rollout_usage.rs` | P1 降级，P3 在 shim 内执行 |
| 附件以本机路径传递 | `driver-api/attachments.rs` 的 `file_text`、ZCode 的 `localPath` | 先上传再改写路径（§6.8） |
| 路径校验走本机 FS | `core/src/lib.rs:1039,1076`、`project_registry.rs:66` `canonical_directory` | 按目标分发校验 |
| 工作区重度依赖本地 FS | `crates/workspace` 约 260 行 fs/进程调用（grep 计数，含内联测试），涉及恢复事务、xattr、索引；`review*`、`workspace_service`、`project_runs`、`terminal_host` 在 Tauri crate 内 | **在 shim 中运行同一份代码**，不逐调用走 RPC |
| 扫描会把缺失项标记为已删除 | `is_missing_from_successful_scan`（`lib.rs` 扫描流程） | 按目标隔离；离线不算缺失 |

**命名注意**：仓库中 “remote” 已指手机远控（`crates/remote-*`、`daemon/remote/*`），`host_id` 也已用于配对的电脑。本功能统一用 **`ExecutionTarget` / `target_id`**，新 crate 也避开这两个词。界面文案可以写“远程设备”。

## 4. 架构选型

| 方案 | 做法 | 结论 |
| --- | --- | --- |
| A. 远端部署完整 Server（ZCode 路线） | 六平台发行包 + 常驻 daemon，Core 和数据库都在远端 | **不采用**。ZCode 为此要处理 Source Server 身份、双端版本兼容、数据迁移、Provider 配置下发（`r3-source-server-remote-architecture.md`）；DogCodes 的跨 Agent 交接依赖本机统一的 canonical history，拆到多端代价更大 |
| B. 完全无落地文件 | 每个操作都走 `ssh exec` / SFTP，MCP 走 `ssh -R` | **不采用**。无法可靠清理进程树（无 TTY 时远端收不到 SIGHUP）；静默断网时 sshd 可能很久才发现；每次操作一次握手；Git 恢复事务无法逐调用走网络；无法断线续跑；历史 MCP 需要 HTTP 版本，且部分 Agent 不支持 |
| **C. 本机 Hub + 薄 shim** | Core 和 Driver 在本机，shim 只负责执行与中继 | **采用**。单一数据权威，无需兼容层（shim 与 App 版本绑定），可复用 Rust 工作区代码 |

SSH 传输层的取舍：

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| **系统 `ssh`** | 配置语义完全一致；FIDO/证书/Kerberos/ProxyJump 无需额外开发；Docker、WSL 可用同样的“启动命令得到字节流”形态 | 依赖客户端有 OpenSSH；Windows 需实测 | **采用** |
| 内嵌 `russh` 等库 | 不依赖外部程序 | 要自己实现 config、known_hosts、agent、跳板 | 不采用 |

ZCode 选了 `ssh2` 库，它自己的评估（`docs/issues/remote-connection-ux/07-ssh-agent-default-key-evaluation.md`）显示：仅补 agent 和默认密钥就需要 7～10 人日，且 FIDO 与证书仍不支持。这正是我们不走这条路的依据。

## 5. 总体架构

```text
┌─────────────────────── 本机 dog.codes ───────────────────────┐
│  React UI ── Tauri 命令 ── Core ── SQLite（canonical 历史）  │
│                             │                                │
│        ┌────────────────────┼──────────────────┐             │
│   本地 Driver          远端 Driver         WorkspaceRouter   │
│   (LocalTarget)      (同一份 Driver 代码)   (target → 实现)   │
│                             └────────┬─────────┘             │
│                               TargetLink（多路复用）          │
│                                      │ 字节流                │
│                     系统 ssh -T alias -- … dogcodes-shim      │
└──────────────────────────────────────┼───────────────────────┘
                                       │ SSH
┌───────────────────── 远端服务器 ─────┼───────────────────────┐
│  ~/.dogcodes/shim/<版本-哈希>/dogcodes-shim                  │
│     ├─ 进程监管（进程组、stdio、退出证明）                   │
│     │     └─ codex app-server / claude / kimi acp / pi …     │
│     ├─ FS / 监听 / PTY / agent-workspace（Git、审查、恢复）  │
│     └─ 中继：MCP stdio ⇄ 本机 --history-mcp                  │
└──────────────────────────────────────────────────────────────┘
```

## 6. 详细设计

### 6.1 领域模型与身份

```rust
pub struct ExecutionTargetRecord {
    pub target_id: TargetId,          // 本机生成的稳定 ID，不含地址
    pub display_name: String,
    pub connection: TargetConnection, // Ssh { alias | host/port/user, auth, identity_file }
    pub machine_identity: Option<String>, // 远端机器 ID + uid 的哈希，用于发现连错机器
}
```

- `ProjectSummary`、`RegisterProject`、`InstallationSummary` 增加 `target_id: Option<TargetId>`，`None` 表示本机。`path` 始终是纯路径，**不把地址编码进路径或身份**。ZCode 早期把线路编进 workspace 身份，后来为此付出了迁移成本。
- 项目去重键改为 `(target_id, path)`。
- Core 在创建任务和选择执行者时**强校验**项目与 Installation 属于同一目标。
- 同一目标内切换 Agent、交接历史、审查修复全部不变，因为历史在本机。
- Core 的 `workspace_context` 改为按 `target_id` 查询的注册表，约 7 处调用。

**存储格式（已确认，2026-11-04）**：目前没有老用户，不做数据迁移，直接修改 schema 并升级 `STORE_FORMAT_VERSION`，沿用现有的“版本不符就拒绝打开”策略（`crates/store/src/lib.rs:104-124`）。代价是本功能合入后，开发机和已装 v0.1.1 的机器上的旧数据库会被拒绝打开，需要手动移走；应用不会自动删除它。

### 6.2 SSH 传输与认证

连接命令（全部使用结构化参数，不经过本机 shell）：

```text
ssh -T
    -o ServerAliveInterval=15 -o ServerAliveCountMax=3
    -o ForwardAgent=no -o ClearAllForwardings=yes -o PermitLocalCommand=no
    -o StrictHostKeyChecking=ask
    [-p port] [-l user] [-i key]  <alias 或 host>  -- <远端引导命令>
```

| 认证方式 | 实现 |
| --- | --- |
| 系统默认（默认选项，比 ZCode 截图多出的一项） | 什么都不传，由 ssh 读取 config 和 agent |
| 私钥 | `-i path`，口令走 askpass |
| 密码 / 2FA | 走 askpass，**密码绝不出现在参数或环境变量中** |

**askpass**：设置 `SSH_ASKPASS=<本应用 exe>` 和 `SSH_ASKPASS_REQUIRE=force`（OpenSSH ≥ 8.4），由 `main.rs` 在 Tauri 启动前分发 `--ssh-askpass`，通过一次性本地通道向主进程请求。主进程先查系统钥匙串（复用已有的 `keyring` 依赖），没有就弹窗。这一条路同时覆盖：

- 密码、私钥口令、keyboard-interactive（2FA）；
- 首次连接的主机指纹确认（ssh 会把 yes/no 问题交给 askpass）；
- 指纹变更时 ssh 直接拒绝，UI 显示明确的安全提示，**不提供一键忽略**。

**SSH config 导入**：解析 Host 列表（含 Include）只用于生成下拉选项，有效值用 `ssh -G alias` 查询，实际连接直接传 alias，由 ssh 自己解析。alias、host、user 必须校验不以 `-` 开头（防止参数注入）。

**选择哪个 ssh 客户端**：这里说的都是 DogCodes 所在电脑上的客户端，服务器只需要有 sshd。

| 客户端平台 | 使用的 ssh |
| --- | --- |
| macOS / Linux | 系统 `ssh`；缺失或低于 8.4 时明确提示 |
| Windows | 优先系统 `ssh.exe`（Windows 自带的 OpenSSH）；缺失或低于 8.4 时，使用安装包内置的 Win32-OpenSSH |

判断依据是运行时 `ssh -V` 的实际版本，不按 Windows 版本硬编码。社区从 ISO 提取的结果（[Win32-OpenSSH #2053](https://github.com/PowerShell/Win32-OpenSSH/issues/2053)，微软无官方对照表）显示，Windows 10 22H2 和 Server 2022 为 8.1p1，Windows 11 22H2/23H2 为 8.6p1；Windows 10 用户基本都会用到内置版本。

内置的 `ssh.exe` 同样读取用户 home 下的 `.ssh/config` 和 known_hosts，因此配置、跳板机和 agent 不受影响。代价是安装包变大一点，安全更新要跟随发布。实现时必须满足：

- **用带 `.exe` 的绝对路径启动**：Windows 上 `ProxyJump` 的下一跳复用 `argv0`，历史上不带后缀曾导致 `CreateProcessW failed error:2`。
- **保留环境变量**：启动 ssh 时保留 `SSH_AUTH_SOCK`、`USERPROFILE`、`HOME`、`PATH` 等，不沿用 Driver 的严格白名单。未设置 `SSH_AUTH_SOCK` 时，Windows 版默认连接 `\\.\pipe\openssh-ssh-agent`。
- **打包完整**：连同 `ssh.exe` 依赖的 DLL 一起打包，并更新第三方声明。
- **已知边界**（与系统 ssh 相同）：系统 agent 服务未运行时用不了 agent；Git Bash 启动的 Unix socket agent 连不上。

**P0 的 Windows 验收**：内置 `ssh.exe` 分别走 config 别名、`ProxyJump`、系统 ssh-agent 连接一次，并验证 askpass 能弹出密码框。

> 2026-11-04 实施进度：内置 `ssh.exe` 的打包尚未完成，留到 Windows 打包环境验证时做；在此之前，Windows 缺失 ssh 时明确报错，低于 8.4 时只能使用不需要提问的认证方式。

### 6.3 Shim 部署

- **打包**：`crates/target-shim`，构建 `x86_64/aarch64-unknown-linux-musl` 和 `x86_64/aarch64-apple-darwin` 四个静态二进制，放入 Tauri `resources`。用 musl 是为了不依赖服务器的 glibc 版本。Release 工作流增加一个先行 job，产物供各平台打包使用。
- **体积**：目标为每个压缩后小于 10 MB，需实测（它会链接 `agent-workspace`，其中包含 `rusqlite`）。
- **不需要“资源下载方式”选项**：ZCode 截图里的这个选项对应它的完整运行时发行包。shim 是内置于 App 的单个小文件，直接经 SSH 上传，也不要求服务器能访问外网 CDN。

连接流程，每步都写入连接日志：

1. `ssh … -- sh -c '…'`：取 `uname -sm`、`$HOME`。因为用户的登录 shell 可能是 fish 或 csh，所有远端命令都用 `sh -c` 包裹。
2. 检查 `~/.dogcodes/shim/<版本-哈希>/dogcodes-shim` 是否存在且校验一致。
3. 缺失则通过 stdin 流式上传：`cat > tmp && chmod 700 tmp && 校验 && mv tmp 目标`。不依赖 SFTP 子系统，服务器禁用 SFTP 也能工作。
4. 启动 `dogcodes-shim serve --nonce <随机>`。本机**丢弃魔数和 nonce 之前的所有字节**，因为 `.bashrc`、MOTD 可能向 stdout 输出内容。
5. 版本目录不可变，新旧版本并存；旧版本无运行中进程时清理。App 与 shim 版本绑定，**不写兼容层**，符合现有约定。

目录权限：`~/.dogcodes` 为 0700，启动前检查属主和权限，不符合就拒绝。

**非 root 用户**：这是推荐的默认场景。shim 只写当前用户自己的 home，以当前用户身份运行，进程组、`setsid`、PTY、Unix socket、`inotify` 都不需要特权，也不使用 sudo。部署前探测以下情况，并在连接日志中给出具体原因：

| 情况 | 处理 |
| --- | --- |
| home 不存在、只读或磁盘配额已满 | 报错，并允许在高级选项中自定义目录 |
| home 挂载为 noexec，或 SELinux 等策略禁止执行 | 上传后运行 `--self-check` 检测，报错，并允许自定义目录 |
| 受限账号（`rbash`、`git-shell`、`ForceCommand`、`authorized_keys` 的 `command=`） | 无法启动 shim，明确提示该账号不允许执行命令 |
| home 在 NFS 上，多台机器共享 | 二进制可以共享；socket 和 SQLite 数据放在按“机器 ID + uid”区分的目录，优先 `$XDG_RUNTIME_DIR` |
| home 路径过长，超出 Unix socket 路径上限 | socket 放到 `/tmp/dogcodes-<uid>/`（0700，启动前检查属主） |

切换用户还会带来几个行为差异，界面上要说明：

- **Agent 按用户隔离**：只安装在 `/root/.local/bin` 的 CLI 对普通用户不可见，厂商登录状态（`~/.claude`、`~/.codex` 等）也要用该账号重新登录。
- **项目权限**：Agent 只能访问该账号有权限的目录。仓库属于其他用户时，Git 可能报 `dubious ownership`，直接显示错误并给出提示，不自动修改 Git 配置。
- **sudo**：Agent 的命令没有 TTY，需要输入密码的 sudo 会失败，不代为提权。

### 6.4 执行层接缝（改动最关键的部分）

在 `crates/runtime` 中增加：

```rust
#[async_trait]
pub trait ExecutionTarget: Send + Sync {
    fn target_id(&self) -> Option<&TargetId>;          // None = 本机
    fn platform(&self) -> Platform;
    fn env(&self, name: &str) -> Option<OsString>;     // 取代 inherited_variable
    fn private_dir(&self) -> TargetPath;               // 取代 temp_dir
    fn command(&self, program: &str) -> TargetCommand; // 取代 agent_command
    async fn read_limited(&self, p: &TargetPath, max: usize) -> io::Result<Vec<u8>>;
    async fn write_private(&self, p: &TargetPath, bytes: &[u8]) -> io::Result<()>;
    async fn metadata(&self, p: &TargetPath) -> io::Result<TargetMetadata>;
    fn map_app_mcp(&self, mcp: AppMcp) -> io::Result<AppMcp>; // 本机返回原值
}
```

- `TargetCommand` 提供与 `tokio::process::Command` 相同的 builder 方法（`arg/args/env/env_clear/current_dir/stdin/stdout/stderr`），加 `spawn_owned()`。**Driver 的改动几乎是机械替换**，本地路径行为保持不变。
- `OwnedChild` 内部变为 `Local | Remote` 两种实现，`stdin/stdout/stderr` 改为 `ProcessInput` / `ProcessOutput`（实现 `AsyncWrite` / `AsyncRead`）。`wait`、`terminate` 语义不变：远端必须收到 shim 的“进程组已结束”帧才算确认；**连接丢失返回错误，不算退出证明**。
- 代理环境不向远端传递：本机代理对服务器没有意义。远端代理由服务器环境决定。
- 各 Driver 的 `connect_ready` 增加 `Arc<dyn ExecutionTarget>` 参数；`lib.rs` 中的 `register_ready_*` 按目标循环调用。

### 6.5 Shim 协议

一条 SSH stdio 上跑自定义分帧：`stream_id | kind | payload`。之所以不用 yamux，是因为 P3 需要偏移量续传，自定义分帧只需几百行。

- **流量控制**：每条流独立 credit；单帧不超过 64 KB。控制流优先，防止大 diff 阻塞审批和中断。
- **流类型**：`Control`（JSON RPC）、`Process`（stdin/stdout/stderr/exit）、`Pty`、`Relay`（MCP 回连）、`Tcp`（端口转发）、`WorkspaceRpc`。
- **进程监管**：每个进程独立 session 和进程组（Linux 额外设置 `PR_SET_PDEATHSIG`）。连接断开后按策略处理（§6.10）。
- **本机不信任远端**：所有来自 shim 的帧都按不可信输入处理，限制长度和路径；shim 只能回连已签发 token 的中继，不能让本机执行任意命令。

### 6.6 Agent 发现与环境

- `sshd` 执行的非交互命令通常不加载 `.zshrc`，nvm、`~/.local/bin` 等不在 PATH 中。shim 启动时用 `$SHELL -lic` 带超时捕获环境（与现有 `record_shell_path` 思路相同），并缓存。
- `agent-registry` 的 `DiscoveryContext` 已有平台和路径参数；把 `exists`、`fingerprint`、`run_scoped_bounded` 改为经 `ExecutionTarget` 调用，manifest 里已有的 Linux 路径可直接复用。
- `is_missing_from_successful_scan` 按目标隔离。目标离线时，其 Installation 保留并显示“设备离线”，不标记为缺失。

### 6.7 MCP 回连（历史工具）

跨 Agent 交接依赖 `--history-mcp`，它的命令是本机 exe 路径，远端不存在。方案：

```text
远端 Agent → shim mcp-relay <socket> <token>
           → Unix socket（0600）→ shim → Relay 流 → 本机
           → 本机启动现有的 dogcodes --history-mcp 127.0.0.1:port token
```

shim **只转发字节，不含 MCP 逻辑**。`history_bridge.rs` 和 lease、token、回执机制都不用改，只在 `core/src/lib.rs:3308` 附近由 `map_app_mcp` 改写命令。相比 `ssh -R` 暴露端口，这个方案不在服务器上开放 TCP 端口，同机其他用户无法连接。

Computer Use 在远程项目中不注入：`computer_use_lease`（`lib.rs:3300`）判断目标不是本机时返回 `None`。

### 6.8 附件

- 图片已经是 base64 内联（`attachments.rs` 的 `codex`、`claude`、`zcode`），无需改动。
- 文件附件在发送前上传到目标私有目录 `~/.dogcodes/run/<hub>/attachments/<id>/`，再把 `PromptAttachment.path` 改写为远端路径。上传完成才算发送成功，任务结束或删除时清理。

### 6.9 工作区（Git、文件、终端、运行）

**核心决策：shim 链接 `agent-workspace`，在服务器上运行同一份代码。** 把这些本地 fs 操作和恢复事务改造成逐调用走网络既慢又破坏原子性；在远端运行，语义和本地完全一致。

| 能力 | 做法 |
| --- | --- |
| `WorkspaceContextProvider` / `TurnCapture` | 新增 `RemoteWorkspaceProvider`，转发到 shim 中的 `GitWorkspaceContextProvider`；审查数据存于 `~/.dogcodes/data/<hub>/` |
| Git 面板、审查操作 | `review*`、`workspace_service` 目前在 Tauri crate 内，先抽出到 `crates/workspace-ops`（无 Tauri 依赖），再由 shim 复用 |
| 文件树、预览、内联编辑 | 同上，走 `WorkspaceRpc` |
| 文件监听 | shim 中运行 `notify`，事件经流推送；大仓库沿用现有过滤 |
| 终端 | `terminal_host` 增加远端 Session 后端，shim 内用 `portable-pty` |
| 运行项 | `project_runs` 建立在终端之上，自动获得远端支持 |
| 网页预览 | `Tcp` 流做本地端口转发（P3） |
| Open-with | 本地编辑器隐藏，保留 VS Code 的 `vscode-remote://ssh-remote+alias/path` 深链接 |

P2 完成前，远程项目的这些面板显示“暂不支持”，不能让它们去读本机同名路径。这是 P1 必须守住的边界：所有 Tauri 工作区命令入口先判断目标。

### 6.10 断线与生命周期

| 场景 | 行为 |
| --- | --- |
| 网络抖动 / 合盖（P1） | Driver 收到 EOF → `Disconnected`；shim 宽限 60 秒后结束进程组；重连后走现有原生恢复，**不重放 prompt** |
| 网络抖动 / 合盖（P3） | `serve` 拆为无状态 connector 与 `shimd`（用户态，`setsid`，无 systemd）。每条流有序号和确认，断线期间写入磁盘 spool（0600，有上限，满了对 Agent 施加背压）。重连后**从偏移继续**，Driver 只感知到一段延迟，运行状态为 `Reconnecting` |
| 本机 App 重启 | Driver 内存状态已丢失，无法接回运行中的协议。新实例以新 lease 连接，shim 结束旧 lease 的进程后走原生恢复，与本地“Core 异常退出”策略一致 |
| 正常退出 App | 同本地策略：结束受管进程。是否提供“退出后继续运行”开关留待 P3 决定 |
| 多台电脑连同一服务器 | 按 hub 实例隔离 socket 和目录，互不影响 |
| 自动重连 | 在线断开后 2/4/8 秒重试，之后每 2 分钟静默探测（参考 ZCode）；只在凭据可静默获取时进行，否则停留在离线态等待用户 |
| 离线时发送消息 | 保留草稿、禁止发送、不排队，避免恢复后产生意外操作（与 ZCode 的处理一致） |

一个需要实测的风险：部分发行版的 `systemd-logind` 开启 `KillUserProcesses=yes` 时，SSH 登出会杀掉用户进程，`shimd` 可能受影响。

### 6.11 账号、额度与升级

| 能力 | P1 | 后续 |
| --- | --- | --- |
| 登录 | 未登录时显示“在远端终端登录”，用 PTY 运行厂商登录命令，凭据留在服务器 | |
| 额度 | 原生协议能返回的继续显示（如 Codex app-server）；读本地文件的显示“暂不可用” | P3：`provider-quota` 链接进 shim，**在服务器上读取和请求**，凭据不经过 SSH 传回 |
| 单轮用量 | 协议事件照常；读会话文件的走 `target.read_limited` | |
| Agent 升级 | 隐藏入口，只显示版本 | P4：`agent_updates` 经目标执行 |

### 6.12 Driver 适配矩阵

| Agent | 优先级 | 主要工作 |
| --- | --- | --- |
| Codex | P1 首批 | 换接缝；`current_dir` 用目标目录；rollout 读取走目标 |
| Claude Code | P1 首批 | 换接缝；`claude-mcp.json` 写到目标；`bypass_permissions_available` 看远端身份和环境 |
| ACP 系（Kimi、Cursor、Grok、OpenCode、DeepSeek、Kiro、CodeBuddy） | P1 | 同一 Driver，一次性覆盖；会话文件读取走目标 |
| Pi | P1 | 换接缝 |
| Antigravity | P4 | `PreToolUse` Hook 依赖本机 exe 和回环地址，需要复用 Relay |
| ZCode | P4 | 当前发现依赖桌面 Electron（`ELECTRON_RUN_AS_NODE` + 内置脚本），服务器上需走 CLI 发现路径 |

### 6.13 手机端

手机 → 桌面 → SSH 的链路不需要改协议：对手机而言只是又多了一些项目和 Installation。需要做的只有两点：连接需要交互认证时，提示回到电脑处理；手机上“从 PC 目录添加项目”只列本机目录。

## 7. 界面

**项目选择器**（对应截图 1）：列表按设备分组；底部保留“创建项目”，新增“新建远程连接…”；搜索框支持搜设备。

**新建连接弹窗**（对应截图 2）：

```text
新建连接                          user@host:22
[ SSH ] [ Docker(P4) ]            从 SSH config 导入 ▾
主机 [                      ]  端口 [ 22 ]
用户名 [                    ]
认证方式 ( 系统默认 | 密码 | 私钥 )
▸ 高级选项：shim 安装目录、连接超时
                           [ 取消 ]  [ 添加并连接 ]
```

与 ZCode 的差异：增加“系统默认”并设为默认项；去掉“资源下载方式”。

**连接进度**：连接 → 认证 → 探测平台 → 部署 shim → 扫描 Agent，每步显示状态，可展开日志（已脱敏）。失败要定位到具体步骤和原因，例如区分“密码错误”“指纹变更”“noexec”。

**连接成功后**：显示检测到的 Agent 列表，然后进入远程目录浏览器选择项目目录（系统文件对话框不能用，需新组件），也可以手动输入路径。未检测到 Agent 时给出安装指引和“打开远端终端”。

**其他界面**：

- 侧边栏项目行显示设备标记和在线状态；
- Agent 选择器只列当前项目所在目标的 Agent；
- 断线时会话顶部显示横幅（状态、原因、重新连接、详情），已加载内容只读保留；
- 设置中新增“远程设备”页：测试、编辑、断开、移除（可选同时删除服务器上的 shim 和数据）；
- 新文案同步全部 11 种界面语言，功能上线后同步 9 种 README。

## 8. 安全

- **主机验证**：使用 OpenSSH 的 known_hosts 机制，首次连接弹指纹确认。ZCode 的 `packages` 目录中没有搜到 `hostVerifier` 或 `known_hosts` 相关代码，这一点不应照搬。
- **凭据**：密码和口令只进系统钥匙串，不进数据库、事件、日志；连接日志经 `redaction` 处理。Agent 账号不离开服务器。
- **SSH 选项**：默认关闭 agent 转发和端口转发。开启 agent 转发意味着服务器上的 Agent 可以用你的密钥去访问别的机器。
- **远端隔离**：目录 0700、socket 0600；同机其他用户无法访问。token 一次性且绑定 lease。
- **反向信任**：服务器被攻破不应波及本机。shim 不能请求本机执行命令；Relay 只连已签发的端点；所有帧限长校验。
- **二进制完整性**：上传后校验哈希。这只防传输损坏，**防不了已拥有该账号权限的攻击者替换文件**，这种情况下服务器本身已不可信。
- **风险提示**：添加目标时明确告知 Agent 将以该 SSH 账号的权限运行；不建议使用 root。root 下 Agent 的命令可以影响整台服务器，而且 Claude Code 在 root 且未声明沙箱时不允许跳过权限（`bypass_permissions_available`）。现有审批机制在远端同样生效。

## 9. 分期与验收

| 阶段 | 内容 | 粗估（单人全职，不含实机矩阵） |
| --- | --- | --- |
| **P0 连接** | 目标模型、存储格式更新、SSH + askpass、shim 部署与协议骨架、连接弹窗与日志、设备管理 | 1～2 周 |
| **P1 对话** | `ExecutionTarget` 接缝重构（本地回归不变）、远端发现、Codex/Claude/ACP/Pi、历史 MCP 中继、附件、目录浏览器、断线横幅 | 3～4 周 |
| **P2 工作区** | 抽出 `workspace-ops`、Git 与审查、文件树与监听、终端、运行项、分叉 worktree | 4～6 周 |
| **P3 韧性** | `shimd` 与偏移续传、自动重连、远端额度、端口转发 | 2～3 周 |
| **P4 扩展** | Docker / WSL 目标、Antigravity、ZCode、远端升级 | 按需 |

**P1 结束时的验收标准**：在一台只安装了 Codex 和 Claude Code CLI 的 Linux 服务器上，通过 ProxyJump + ssh-agent 连接；完成对话、工具审批、中途中断；Codex 切换到 Claude 并通过历史工具查询到之前的轮次；杀掉本机 ssh 进程后，会话显示断开，远端无残留进程，重连后原生恢复且没有重复发送 prompt。

### 测试

- **接缝回归**：本地路径的现有测试必须全部通过，这是 P1 重构的硬门槛。
- **自动化**：Docker 中运行 sshd 容器，覆盖密码、密钥、agent、ProxyJump 认证；脏 `.bashrc`、fish 登录 shell、noexec home、指纹变更；用脚本版假 Agent 走完整 Driver 流程，不消耗模型额度。
- **故障注入**：杀 ssh、丢包或断网、杀 shim、磁盘满；断言无孤儿进程，且没有伪造的退出证明。
- **实机矩阵**：真实 CLI + Linux x64/arm64 + macOS 远端 + 三个客户端平台（Windows OpenSSH 是重点风险）。真实 CLI 和平台验收按现有方式安排。

## 10. 待决策

| # | 问题 | 建议 |
| --- | --- | --- |
| 1 | P3 的续跑是否提前 | 按当前顺序。P1 先验证链路，但如果“合盖继续跑”是首发卖点，可以把 P3 提到 P2 之前，两者相互独立 |
| 2 | App 正常退出时，远端 Agent 是否继续运行 | 默认结束，与本地一致；P3 再考虑开关 |

已确认（2026-11-04）：接受自动上传 shim；暂无老用户，不做数据库迁移。

## 11. 主要风险

| 风险 | 缓解 |
| --- | --- |
| P1 接缝重构影响本地稳定性 | 单独一个 PR，纯机械替换，无功能变化，先合入 |
| shim 链接 `agent-core` 导致体积大或 musl 编译问题（`rusqlite`、`cap-std`、`rustix`） | P0 先做 musl 构建与体积验证；必要时把 trait 从 `agent-core` 拆到更小的 crate |
| Windows 客户端 OpenSSH 版本和 askpass 行为不一致 | 版本不足时使用内置 `ssh.exe`；P0 尽早实测 askpass 和 ssh-agent |
| 厂商 CLI 在无桌面环境下行为不同（登录、keychain、浏览器 OAuth） | 按 Agent 写入 `docs/compatibility/`，不假设与本地一致 |
| 高延迟下体验差 | 控制流优先；工作区操作在服务器本地执行；文件树按需加载 |

## 12. P0 实施记录（2026-11-04）

**已实现：**

| 内容 | 位置 |
| --- | --- |
| 目标模型，按版本写入、可重建、重开不丢 | `crates/protocol`、`crates/core/src/execution_targets.rs`、`crates/store/src/execution_targets.rs` |
| 分帧协议（目前只有控制流） | `crates/target-wire` |
| shim：`--self-check`、`serve`、心跳、机器身份哈希 | `crates/target-shim` |
| 系统 ssh、askpass、单连接启动脚本与部署、SSH config 候选、错误分类 | `crates/target-link` |
| 服务侧连接管理与钥匙串，仅主窗口、HostOnly | `apps/desktop/src-tauri/src/execution_targets.rs` |
| 项目选择器入口、连接页、连接弹窗、进度与提问，11 种语言 | `apps/desktop/src/ExecutionTargets.tsx` 等 |
| shim 交叉构建与打包 | `scripts/prepare-shims.mjs`、`scripts/package-desktop.mjs` |

**与设计的差异：**

- 目标列表沿用设置事件流，P0 **没有改表结构，也没有升级 `STORE_FORMAT_VERSION`**。项目和 Installation 的 `target_id` 到 P1 真正用到时再加，届时才需要升级格式。
- shim 实测不到 1 MB（gzip 后约 0.33～0.41 MB），远小于预估的 10 MB；但它尚未链接 `agent-workspace`，P2 接入后需要重新测量。
- 连接列表的“删除”只移除本机记录和钥匙串内容，**不清理服务器上的文件**，这一点界面文案已说明。

**验证：**

- 单元测试：目标模型、ssh 参数、askpass、错误分类、SSH config 解析。
- 端到端（opt-in，真实 `ssh` 客户端 + 测试私有 `sshd`，macOS arm64）：`cargo test -p target-link --test local_sshd -- --ignored --test-threads=1` 共 16 项，覆盖首次部署与复用、目录权限、断开后 shim 退出、passphrase、未知主机确认和拒绝、指纹变更、拒绝的密钥、受限账号、嘈杂的登录 shell、不安全和不可写的安装目录。
- 服务路径（opt-in）：`cargo test -p desktop --lib execution_targets -- --include-ignored`，覆盖保存、连接、记录身份、断开。
- 全量 `desktop` 测试要像 CI 一样设置 `RUST_MIN_STACK`：默认 2 MB 栈下，`review_rpc_builds_project_packet_in_service_and_replays_without_gui_snapshot` 会栈溢出，未改动的基线代码同样如此。

**未验证 / 未完成：**

- **真实服务器只验证了一次首连**：用户在开发版应用里连接真实服务器成功（2026-11-04），这也是连接弹窗首次在真实窗口中走通。重复连接不再上传、断开后服务器上 shim 退出，目前只在 macOS 的私有 `sshd` 上验证过；ARM 版 Linux shim 未实际运行过。
- **密码认证未端到端验证**：非 root 的测试 sshd 无法校验系统密码；askpass 通道已由 passphrase 用例覆盖。
- **Windows 客户端未验证**；内置 `ssh.exe` 未打包。
- **Release 流水线**：Windows/Linux 打包机只能构建 Linux 版 shim，要让所有安装包都带 macOS 版，需要在流水线增加先行构建 job。
- **未做**：旧版本 shim 清理、断线自动重连（P3）、服务器文件清理、Agent 扫描（P1）。

## 13. P1 第一步：进程启动接缝（2026-11-04）

这一步没有功能变化，也还没有远端实现，目的是让 Driver 不再假定进程跑在本机：

- `OwnedChild` 分为本地和远端两种 scope，stdio 改为 `ProcessInput` / `ProcessStream`，远端流是 `DuplexStream`，直接复用流量控制。
- `crates/runtime/src/target.rs` 新增 `ExecutionTarget`、`TargetExecutable`、`TargetCommand`。环境变量默认不继承，`inherit` 取目标自己的值；本机代理和 npm Node 路径只用于本机目标。
- Codex、Claude、ACP 系、Pi 的主进程启动改走接缝。公开构造函数接受 `impl Into<TargetExecutable>`，传本地路径的现有调用无需改动。

**与 §6.4 的差异**：Antigravity 和 ZCode（P4）只换了流类型，启动仍是本地代码。以下辅助进程也仍在本机运行，P1 接入远端时必须按目标屏蔽或改造：Claude 额度、Cursor 钥匙串读取；Claude 的 `claude-mcp.json` 也还写在本机。

## 14. P1 第二步：shim 进程监管与真实服务器实测（2026-11-04）

**已实现：**

- 协议增加 `Data`、`Eof`、`Credit` 帧。每条进程流独立的 256 KB 窗口，控制流不受影响。
- shim 用 `process-scope` 监管进程：独立进程组，shim 退出时一并结束。退出消息在输出之后发送。
- `target_link::TargetLink::target()` 提供远端 `ExecutionTarget`。环境变量来自 `$SHELL -lic` 捕获的登录环境。
- 断线时等待方得到错误，不算退出证明。
- `crates/target-link/examples/remote_agent_probe.rs`：真实服务器上驱动 ACP Agent 的探针。

**实测**（Linux x86_64 服务器，root，密码登录；Kimi CLI 2.5.0）：

| 项目 | 结果 |
| --- | --- |
| 真实 Linux 上部署与运行 shim | 通过 |
| 密码登录，含输错后重试 | 通过 |
| 再次连接不重复上传 | 通过 |
| 登录环境中的 `PATH`，定位 `/root/.kimi-code/bin/kimi` | 通过 |
| ACP 握手（约 1.3 秒）、模型列表、创建会话 | 通过 |
| 一轮完整对话：提示词经 SSH 发出，流式回复 `OK` | 通过 |
| 登录脚本向 stderr 输出杂讯（`flatpak: symbol lookup error`） | 不影响 |

ACP 的协议代码没有为此改动。

**实测发现并修复**：ACP 探测把本机临时目录作为 `cwd` 发给远端 Agent，导致 `session/new` 失败。现在按目标选择目录。

**实测发现、尚未处理：**

- 服务器主动断开（`Connection closed by`，推测是等密码超过 `LoginGraceTime`）只显示通用的“连接失败”，应给出明确提示。
- 探针未走应用界面。真实服务器上断开后 shim 是否退出，尚未确认（私有 `sshd` 上已验证）。
- 工具审批、中断、恢复、历史 MCP 和附件还没在远端测过。
- shim 增大到约 1.26 MB（x86_64 Linux），仍未链接 `agent-workspace`。

## 15. P1 第三步：在应用里使用远程 Agent（2026-11-04）

**已实现：**

- 项目和 Installation 增加 `target_id`，同一路径在不同机器上是不同的 Installation。`STORE_FORMAT_VERSION` 升到 2，**不做迁移**，旧数据库需要手动移走。
- 连接成功后按现有 manifest 扫描服务器上的 Agent，就绪的注册为该目标专属的 Driver；断开时注销。本机扫描不会动远端 Installation，反之亦然。
- 服务器目录浏览，路径在服务器上解析后再保存。同一目录重复打开时复用项目。
- Agent 选择器和默认 Agent 只看项目所在机器；Core 也会拒绝把其他机器上的 Agent 指派给项目。
- 设备未连接时，会话顶部显示提示并可跳到连接页。

**本地路径边界**：远程项目的路径绝不会被当作本机路径读取。工作区、审查、撤销、终端、运行项、打开目录都明确拒绝或为空，轮次的工作区快照跳过。终端和审查面板入口在界面上禁用。

**实测（2026-11-04）**：在开发版应用里连接真实 Linux 服务器，通过 Kimi 完成实测，用户确认通过。具体覆盖了提示清单中的哪些项，未逐项记录。

**这一阶段的限制：**

- 远程项目没有 Git 变更、审查、文件树、终端和运行项（P2）。
- 没有 App MCP：`supports_app_mcp` 对远端返回 false，因此没有历史工具；上下文交接仍可通过提示词进行（第四步接中继）。
- 文件附件还不会上传到服务器，只有图片（内联）能到达 Agent（第四步）。
- 没有额度显示；远程 Installation 不进入 Agent 管理页，也没有升级入口。
- 只支持 Codex、Claude、ACP 系和 Pi 协议；Claude 的 MCP 配置和 root 下的权限判断还没有按目标处理。
- 应用重启后不会自动重连，需要手动连接；设备离线时发送消息只会得到通用的 Agent 不可用错误。

## 16. P1 第四步：历史工具、附件与 Claude（2026-11-04）

**已实现：**

- **MCP 中继**：Agent 启动 `dogcodes-shim mcp-relay <socket> <token>`，经私有目录中的 Unix socket 接入 shim，再经 SSH 到本机，由本机启动现有的 `--history-mcp`。shim 只转发字节，服务器上不开 TCP 端口。token 由本机签发，只能选择本机已登记的程序，未知 token 不会运行任何东西。
- **文件附件**：发送前按内容哈希上传到服务器临时目录（0600），`delivered_path` 记录远端路径，`path` 仍是本机副本。图片仍内联。文件名只取最后一段，不能决定落在哪里。
- **Claude**：MCP 配置在远端改为命令行内联 JSON；root 判断和沙箱变量取服务器上的值；后台任务输出文件在远端不读取。
- **Codex**：rollout 用量文件在远端不读取。
- Computer Use 不注入远端。服务器主动断开、设备离线发送消息、附件上传失败都有明确提示。

**修复的 bug**：`serve` 和 `mcp-relay` 退出时，tokio 的 stdin 读线程会阻塞运行时关闭。`serve` 在空闲超时后因此可能退不出去。

**Kimi 实测（47.101.41.40）：**

- 历史工具已通：Kimi 列出并调用了 `getTurns` 等工具，结果由本机返回。这些工具只查当前对话，新对话里查不到别的对话，本地项目也一样。
- 附件第一次失败：文件已上传，但 ACP 驱动发给 Agent 的仍是本机路径。已改为发送服务器上的路径，并补了单元测试，复测通过。

**尚未处理：**

- 上传到服务器的附件不会清理，留在临时目录直到系统清理。
- 旧版本 shim 和 relay 目录的清理。
- 其他面向用户的远程错误（如“远程项目暂不支持这项操作”）还没进翻译目录，界面入口已禁用，一般不会看到。

## 17. P2：远程项目的工作区（2026-11-04）

远程项目现在有终端、运行项、文件树与预览、目录监听、Git 面板、审查、撤销和分叉 worktree。

**做法与 §6.9 的差异：**

- **代码放在哪：** 审查和文件操作从 Tauri crate 移到新 crate `workspace-ops`，Tauri 命令留在桌面端的 `review_commands.rs`。本机项目直接调用它，服务器项目由 shim 运行同一份代码。
- **任务信息怎么来：** `workspace-ops` 通过 `ReviewFacts` 询问任务、产物和互斥锁。本机由 Core 回答（`core` feature）；服务器上的操作经连接问回桌面。这比设计里的“提前打包事实”简单，也不用预估操作需要什么。
- **通道：** `SpawnRequest.operation` 启动 shim 内的一次操作，流量控制和进程共用同一套窗口，消息是逐行 JSON，因此没有 1 MB 单帧上限。
- **shim 的依赖：** `agent-workspace` 不再依赖 Core（`WorkspaceContextProvider` 等 trait 移到 `agent-protocol`）。`sqlite-index` 和 `system-proxy` 改为可选 feature，shim 不带 C 依赖和桌面库，仍是静态二进制。没有 SQLite 时，失效快照索引用定长记录的文件。
- **Core：** `workspace_for(target)` 按项目所在机器选择提供者。服务器未连接时回答“不可用”，绝不读本机同名路径。`TurnWorkspaceScope` 带上对应的提供者。
- **终端：** shim 用 `portable-pty` 创建终端。桌面端的远程会话实现了和本地相同的 `MasterPty` / `Child` trait，所以读取、停止、记录只有一份代码。失去联系记为 `unknown`，不算退出。
- **运行项：** 分析用服务器上的 Agent，文件在服务器上扫描，平台记为 `server`。

**安全边界：**

- shim 发来的内容都不可信。`TargetFacts` 只回答当前操作所属的任务。快照里的项目、任务、已删除任务、会话映射和伴侣面板只含这台服务器的；已安装 Agent、运行中列表和最近完成记录不提供。
- 服务器不能让本机的 Agent 运行：提交信息生成和项目审查要求 Agent 安装在同一台服务器上。界面也只列出这些 Agent。
- 返回路径让本机打开的操作（`WorkspacePath`、`RevealPath`）对远程项目直接拒绝，否则可能打开本机同名文件。

**数据位置：** 服务器上的审查数据在 `<安装根目录>/data/review`（默认 `~/.dogcodes/data/review`）。

**shim 体积：** 从约 1.4 MB 增加到 6～7 MB（未压缩）。

**尚未处理：**

- 手机端从远程项目导入附件、打开远程项目的网页预览仍返回错误。文件预览、目录读取和目录监听已走远程路由。
- 远程项目暂不能修改目录。
- 在本机用编辑器打开远程文件（设计里的 VS Code `vscode-remote` 链接）未做，目前提示文件在服务器上。

**已补上：**

- **`git` 查找：** sshd 给的 `PATH` 找不到 `git` 时，shim 用登录 shell 的 `PATH` 再找一次。只在找不到时才问，避免慢的 shell 配置拖慢每次操作。
- **已删除任务的数据：** 服务器每次连接时，把永久删除过的任务逐个告知它。删除不存在的数据没有副作用，所以不用按服务器记录进度，也不需要数据库迁移。中途失败就停止，下次连接再试。

**Git 询问凭据：** Git 需要账号、密码或口令时，运行 `GIT_ASKPASS` 指向的程序，也就是 Git 所在机器上的应用自身（本机是 desktop，服务器是 shim）。程序经带令牌的本机回环连接把问题交给发起这次 Git 操作的进程；服务器上再经操作通道问回桌面。桌面进程保存问题，界面在 Git 操作运行时轮询并弹窗。

- 答案不进命令行参数、环境变量和文件，应用也不保存；要记住凭据，用 Git 自己的 `credential.helper`。
- 问题文字来自 Git 或远端，只当普通文本显示，长度有上限。除了明确询问用户名的，都按密码隐藏输入。
- 取消或 5 分钟无人回答，都按拒绝处理，Git 会自己报错。Git 不再等待时，弹窗随之消失。
- 只有桌面能回答，手机端不会收到这些问题。
- 带询问的 Git 命令超时从 2 分钟放宽到 10 分钟。等待期间这个任务的审查门锁一直持有，其他写操作要等。

**实测中修复：**

- 运行分析的临时目录原来建在本机，现在建在 Agent 所在的机器上，用完删除。
- 远程项目的运行配置，重启后能读取；运行面板只列出项目所在机器上的 Agent。
- 服务器离线时，横幅和右边栏都能就地重连，共用一份连接状态。
- 连接断开后状态未知的终端可以关闭：只从列表移除，不向任何进程发信号，记录保留以防重放启动请求。
- Run 面板去掉后台保留：已结束的命令直接关闭，运行中的先确认再停止。
- 提交不再被遗留的未核实记录永久挡住（处理它的界面此前已被删除）：提交前自动核实，并显示真实原因和 Git 的原文。

**已知：** 某些服务器登录时会打印和应用无关的提示。这次遇到的 flatpak 符号错误，原因是系统 glib 2.56 比 `libappstream-glib` 需要的旧。

**验证：** 私有 sshd 端到端测试覆盖了终端（输入、尺寸、停止）、审查（查询、跨调用快照、暂存、互斥锁）、提交、Git 询问凭据、文件（列表、预览、搜索、越界拒绝、监听）、回合捕获、分叉 worktree、撤销和断线。47.101.41.40 上实测通过：终端、文件和自动刷新、Git 面板、提交并推送（含凭据弹窗）、撤销、运行分析和运行命令、断线和重连。

**本地开发**：连接前先构建当前机器和 Linux 的 shim。

```bash
node scripts/prepare-shims.mjs
```
