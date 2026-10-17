# Codex 兼容性记录

> 最新跨 Agent 验收状态与可重复命令以
> [`../implementation/beta-agent-acceptance.md`](../implementation/beta-agent-acceptance.md) 为准；
> 本文只记录 Codex 协议细节与历史证据。

更新日期：2026-10-01

## 当前结论

- 2026-10-01 修复长交接上下文发送失败：本机 Codex `0.159.2` 的启动、模型列表和建会话正常，
  最近两次切换到 Codex 的交接包经 JSON 转义后约为 69,851 / 75,778 字节，超过旧驱动
  64 KiB 发送限制，在到达原生进程前即失败。RPC 收发帧上限统一提升为 8 MiB，覆盖
  Core 允许的 1 MiB 交接包及 JSON 转义、原生 userMessage 回显；审批详情仍保留 64 KiB 上限。
  回归覆盖长交接包发送、响应前大帧回显和超限拒绝。实际 `0.159.2` 接受 176,345 字节
  的协议请求并返回预期的不存在会话错误，证明该大小通过原生请求解析；该探针未启动模型推理，
  不作为真实模型对话成功的证据。

- 2026-09-27 修复升级后的安装发现：本机 `0.158.0-alpha.2.1` 将桌面内置入口迁移到
  `Contents/Resources/codex-cli/bin/codex`，原 `Contents/Resources/codex` 已不存在。
  manifest 现覆盖系统/用户 Applications 下 ChatGPT/Codex 的新旧布局，仍优先使用终端命令。
  目录升级回归测试与真实 `initialize/account-read` 检测通过；运行中的桌面服务已报告
  新路径为 `Ready`，且已注册 Codex executor。本轮未启动模型 Turn。
  可重复验证：`cargo test -p desktop terminal_installation::tests --lib`；
  本机零 Turn 验证：`cargo test -p desktop current_codex_desktop_is_ready --lib -- --ignored`。
- 2026-09-08 修复桌面发现回归：`codex` 仍是单一 Provider，桌面 App 内置 Runtime 与独立 CLI
  作为具体 Installation；manifest 显式覆盖系统/用户 Applications 内的 ChatGPT/Codex bundle，
  并按 canonical executable 去重。本机重新发现 0.153.4 并注册 Ready。安装差集仅在已记录路径
  确实消失时发 removed，不再把 PATH/候选缺失冒充卸载。
- `codex-driver` 已进入桌面 Beta Product Driver：具体 Installation 绑定、真实
  initialize/account readiness 和合成进程的 Thread/Context/Turn 契约已通过。实现状态见
  [`m1-s4-codex-driver.md`](../implementation/m1-s4-codex-driver.md)。
- 2026-09-02 的 M0 基线曾发现两份 canonical executable；下表保留为历史版本证据，不表示
  当前磁盘仍有同样安装。`--version` 单独仍只能提升到 Installed。
- Product Driver 另做 initialize + `account/read(refreshToken=false)`；只有 account 存在或
  runtime 明确不要求 OpenAI auth 才提升 Ready。它不保存 account payload，也不证明 Turn 能力。
- 版本探针使用结构化 argv、清空环境后只传递 `PATH`；handshake 额外透传既有 home/temp 定位变量，让 Codex 自己管理运行状态。dogcodes 不读取或输出 Agent 配置、Token、Cookie、API Key 或原生会话库。
- 两版本的 stable initialize request/response 均已通过真实 stdio JSONL round-trip；独立 `handshake` 模式只发送 `initialized` notification，不启动 thread 或 turn。

## 2026-09-03 Product Driver 事实

| 安装 | 版本 | Readiness | 零 Turn 跨进程 resume | 当前能力 |
|---|---|---|---|---|
| ChatGPT macOS app resource | `0.153.0-alpha.5` | initialize/account-read 通过（32 ms） | `-32600 / not_found` | Ready；live Session 复用，restart resume Unknown |

本轮没有发送模型 Turn。零 Turn resume 使用隔离临时 `CODEX_HOME`，没有触碰用户原生会话；
start/name-set 成功但没有形成 rollout。该结果只约束恢复能力，不否定完成真实 Turn 后可能恢复。

## 2026-09-02 历史安装证据

| 来源 | Canonical executable | 版本 | 结果 |
|---|---|---|---|
| ChatGPT macOS app resource | `/Applications/ChatGPT.app/Contents/Resources/codex` | `0.150.0-alpha.12.2` | Installed；版本探针 13 ms |
| 用户已有 Node 安装 | `$HOME/.deskclaw/node/lib/node_modules/@openai/codex/bin/codex.js` | `0.144.5` | Installed；版本探针 145 ms |

耗时是单次本机观测，不是性能承诺。持久化 fingerprint 包含 canonical path、文件长度和 mtime；fingerprint 未变化时，桌面启动扫描复用已有版本探针结果。

## Schema 与 handshake 事实

| Codex 版本 | Stable JSON 文件 | Stable bundle | Canonical SHA-256 | Initialize |
|---|---:|---:|---|---|
| `0.150.0-alpha.12.2` | 301 | 571,525 bytes | `56f52801…3269ca0` | 通过 |
| `0.144.5` | 267 | 471,114 bytes | `5d8251e1…2877621` | 通过 |

- 两版本的 stable schema 数量、大小和 canonical hash 均不同，Driver 必须按安装版本做协议适配。
- `0.144.5` 对同一语义 schema 的 raw SHA 在两次生成间不同，但递归排序 JSON object key 后的 canonical SHA 相同。兼容门只使用 canonical SHA，raw SHA 只作诊断。
- 两版本 handshake 都返回 `platformFamily=unix`、`platformOs=macos` 和 `codexHome`；fixture 只记录字段存在，不保存路径值。
- 两版本均成功创建 `cwd=/tmp`、`sandbox=read-only`、`approvalPolicy=never` 的 ephemeral thread；返回 `status=idle`、`ephemeral=true`、`path=null`。fixture 只记录 thread id 存在，不保存其值。
- 两版本在 `thread/start` response 前都穿插了 `remoteControl/status/changed` notification。探针 transport 证明可保留这种 interleaving；该未纳入最小 Canonical 子集的方法按 `VendorExtension` 处理，不能静默丢弃。
- 两版本对同连接刚创建且仍已加载的 ephemeral thread 立即调用 `thread/resume`，均返回 JSON-RPC `-32600`；响应前穿插 `thread/started` 和两条 `mcpServer/startupStatus/updated`。fixture 不保存错误 message，且该负向结果不能外推为跨进程 persistent resume 不可用。
- 隔离 `resume-smoke` 会创建临时 `CODEX_HOME`，并在创建 thread 前校验 initialize 返回的 `codexHome` 与其 canonical identity 完全相同；探针不接触用户原生会话库，也不启动模型 Turn。
- `0.144.5` 在非 ephemeral 空 thread 上完成固定 `thread/name/set` 后，rollout path 在关闭前存在；第二个 app-server 成功 resume，返回相同 thread identity、`idle`、零 Turn。
- `0.150.0-alpha.12.2` 接受同样的 name update，但一秒有界等待后 rollout path 仍不存在；第二个 app-server 返回 `-32600 / not_found`。因此该版本在首个 Turn 前不能用当前方法证明恢复能力，Driver 不得假设“收到 thread/start response 即可恢复”。
- M0 transport 对单帧和请求前 interleaving 均设上限；stderr 持续 drain 但只保留有限字节，避免子进程因 pipe 回压卡死。
- 官方说明 schema 输出与生成它的 Codex 版本匹配，并要求每条连接先完成一次 `initialize`，随后发送 `initialized` notification：<https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md>。

## 复现

```bash
cargo test -p agent-registry \
  probes_the_current_users_codex_installation -- --ignored --nocapture

cargo run -p codex-app-server-probe -- \
  schema /absolute/path/to/codex /tmp/codex-schema

cargo run -p codex-app-server-probe -- \
  handshake /absolute/path/to/codex

cargo run -p codex-app-server-probe -- \
  thread-start /absolute/path/to/codex

cargo run -p codex-app-server-probe -- \
  resume-smoke /absolute/path/to/codex

# 会调用模型；必须由操作者显式确认额度消耗后执行。
cargo run -p codex-app-server-probe -- \
  turn-smoke /absolute/path/to/codex --allow-model-usage
```

测试 fixture 与 manifest：

- `crates/agent-registry/src/lib.rs`
- `drivers/manifests/codex.toml`
- `experiments/codex-app-server/fixtures/schema-fingerprints.json`
- `experiments/codex-app-server/fixtures/initialize-handshakes.json`
- `experiments/codex-app-server/fixtures/persistent-resume.json`

`turn-smoke` 已实现安全门和脱敏聚合器，但截至本记录更新时尚未运行，因此不能把 streaming/usage/cancel 标记为 runtime verified。

## M0-CODEX 未决事实门

2026-09-03 新增纯离线 `sanitize-fixture` / `check-sanitized-fixture`：代码内固定的结构化
allowlist、有界不覆盖导出、规范 JSONL 与二次策略检查。仅有合成 CLI 测试，没有新增真实
模型或 runtime 能力证据，原生版本明确未知。旧 summary fixture 格式不变，不能直接送入
新 checker。实现与生产 Recorder/CI 接手点见
[`m1-s5-raw-diagnostics.md`](../implementation/m1-s5-raw-diagnostics.md)。

- `app-server generate-json-schema` 的命令形态、schema 指纹和版本差异。
- stdio JSONL 初始化/版本协商与关闭行为。
- `0.153.0-alpha.5`（以及历史 `0.150.0-alpha.12.2`）在至少一个真实 Turn 已形成 rollout 后
  的跨进程 `thread/resume`；二者无 Turn 的 name update 均未创建可恢复记录。`0.144.5` 的
  零 Turn跨进程恢复已验证。
- `turn/start` 的最小真实会话闭环。
- 流式文本、tool、Diff、提问与 approval 的真实 frame fixture。
- cancel/interrupt 的确认语义、超时、进程退出和孤儿清理。
- 认证/账号/额度只采用 App Server 官方自报能力；如果没有证据，保持 Unknown。

Manifest 当前为 `v2_minimal_beta`：只有真实 account readiness 通过的具体 Installation 才进入
可选执行者列表。它不是 Production 声明；下面未决事实仍按 Unknown/安全降级处理。

2026-09-08：账户窗口由 `account/rateLimits/read` 主动展示；任一新鲜窗口剩余 0% 时 UI 明确
显示“已用尽”并禁用发送。Turn error 仅在 code 为 `usage_limit_reached`/`quota_exceeded` 或
明确耗尽措辞时映射结构化 `TurnQuotaExhausted`；普通 429 不作订阅耗尽推断。
