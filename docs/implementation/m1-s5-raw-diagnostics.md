# M1-S5：结构化脱敏与离线诊断导出

更新日期：2026-09-03

## 状态与边界

已完成详细设计 §7.5 / §19.3 的首个独立安全切片：代码内固定的 schema/path allowlist、
有界离线 JSONL 诊断副本导出，以及可供 CI 调用的只读二次检查命令。没有启用生产 Raw
Recorder，没有新增后台录制、压缩、分片或保留清理，也没有接入 CI workflow。

本切片只消费操作者明确给出的本地文件，不启动 Codex、不安装 runtime、不读取凭据、
不消费模型额度。原始输入文件、Canonical 用户消息和进程内审批 request ID 均保持原样。

## 代码入口

| 入口 | 职责 |
|---|---|
| `crates/redaction/src/structured.rs` | `FieldPolicy`、`sanitize_json`、`SanitizedDocument::verify_again`、结构预算与脱敏审计 |
| `experiments/codex-app-server/src/diagnostics.rs` | Codex 最小诊断策略、`sanitize_codex_frame`、原子不覆盖导出和二次检查 |
| `experiments/codex-app-server/src/main.rs` | `sanitize-fixture` / `check-sanitized-fixture` 离线命令分支 |
| `experiments/codex-app-server/tests/fixture_export.rs` | 真实 CLI + 临时合成文件的端到端测试，不启动 Agent |

## 结构化策略

- `FieldPolicy` 只由受审阅代码声明；不接受 Frame/UI 自带白名单。Object 的每一级都精确匹配
  字段名及子策略，不能因为同名字段在另一条路径获准就放行。
- 解析 JSON 后先检查全部结构预算，包括最终会被丢弃的字段。敏感键先规范化并拒绝，随后
  才应用 allowlist；JSON 转义的 `token`、大小写/分隔符变体和非字符串 secret 同样拒绝。
- 未知键和值都丢弃。审计只记录 dropped/sensitive/masked/typeMismatch/textFinding 的数字
  合计，不记录未知键名、路径、正文、解析错误片段或被脱敏内容。
- 支持 Boolean、限定范围 Integer、固定 Enum、Array、Object、Mask 和 RedactedText。
  后者复用默认 ruleset 2；Codex 当前策略不放行任何自由文本，不能因启发式存在而开放正文。
- 类型/枚举/范围不匹配的对象字段丢弃；数组中不匹配元素替换为 null 以保留位置。Mask 对
  字符串输出 `[REDACTED_FIELD]`，其他类型输出 null，不保留原始结构或值。
- 输出重新检查结构和字节预算；替换造成膨胀也失败。`SanitizedDocument` 无公开构造器或
  Deserialize，持久化前再次应用相同策略，要求 payload 不变且新增文本命中为 0。

预算如下，超限整帧/整次导入失败，不输出截断的“成功”副本：

| 层级 | 上限 |
|---|---|
| 原始单帧 / 脱敏 payload | 64 KiB |
| 深度 / 节点数 | 16（根为 0）/ 4096 |
| 单数组 / 字符串 / 键 | 128 项 / 4096 bytes / 256 bytes |
| 单次导入 | 1 MiB、256 个非空帧 |
| 导出文件 | 2 MiB |
| 检查单条诊断记录 | 64 KiB + 1024 bytes 元数据余量 |

## Codex 诊断副本格式

`turn/started`、`turn/completed` 只保留指定路径的固定 status 枚举；thread/turn/request id
全部屏蔽，items/error 不展开。其他已知方法只保留固定 method 名，params/result 整体屏蔽。
Response 没有 request 方法关联，因此不猜测 result schema；错误仅允许有界 code，message/data
屏蔽。未知方法名不输出，只有 `recognizedMethod = false`。完整方法表以代码为准。

每条记录有 `schemaVersion = 1`、`policyId = codex-m0-diagnostic-v1`、
`parserVersion` / `driverVersion`（当前探针 crate 版本）、`driverId = codex-app-server-m0`、
`sourceKind = offline_import`、连续 ordinal 和 `diagnostic`。`nativeVersion = null` 明确表示
未验证原生 runtime 来源，不能据文件内容自行宣称版本。

这不是可发回 Agent 的原生 JSON-RPC，也不是保留请求关联的协议回放格式。不得将占位 ID
用于批准/拒绝请求，不得把状态字段当成 runtime verified capability 或 Canonical 业务事实。

## 文件边界与二次检查

使用方法见 [探针 README](../../experiments/codex-app-server/README.md)。路径必须绝对；输入
必须是普通文件且不能是最终路径上的 symlink，输出必须不存在。先在内存验证所有帧，再在
目标目录创建私有临时文件、写入安全副本、sync_all、persist_noclobber。Unix 输出权限 0600；
失败由临时文件 RAII 清理，既有文件或 symlink 不覆盖。错误只用固定原因，不回显输入路径。

输出按 serde_json Value 的稳定键序逐行序列化。checker 校验每行与重新序列化字节一致，
拒绝重复键中隐藏的前一个值、非规范空白/转义以及多余元数据；再按当前策略重新生成完整
记录并比较。原始审计计数不可重建，只校验五个计数键及 0–65536 范围，不视为来源真实性
证明。版本/策略变化后旧格式可能被拒绝，需要显式兼容策略，不能静默降级放行。

该路径检查不是恶意并发文件系统的沙箱：输入检查到打开之间的竞争、父目录替换和跨平台
权限仍需专项设计。临时文件原子发布也不等于包含父目录 fsync 的断电持久化保证。

## 验证

- 新增 7 个结构策略单测、3 个 Codex 策略单测、4 个真实 CLI 集成测试。
- 覆盖完整字段路径、转义敏感键、非字符串值、深度/节点/数组/文本/字节预算、输出膨胀、
  二次脱敏、未知字段名零泄漏、审批 ID 进程内保真、输入不变、不覆盖/symlink/0600、
  损坏第二帧和超限零输出、metadata/payload/audit 篡改与重复键拒绝。
- 当前全 workspace：Rust 106 passed、1 ignored；fmt/clippy 通过。`pnpm check` 通过，前端
  25 passed、typecheck/build 通过。未运行 ignored 安装探针或真实模型 Turn。
- 本切片没有改变 UI；此前原生保存/取消验收缺口仍保留，不能以 CLI 文件测试替代。
- 本轮 `pnpm desktop:dev` 的 Tauri/Vite 启动通过；app data 只读查询为 `user_version = 2`、
  `quick_check = ok`，task_artifacts 表存在。开发进程已主动停止，没有扩大 UI 验收结论。

## 下一 agent 接手

1. 审计旧 M0 summary report 的动态字符串/方法名路径，再考虑接入相同安全投影；本次没有
   改写旧 `print_report` 路径或历史 `fixtures/*.json`，它们也不是新 checker 接受的诊断格式。
2. 实现生产 Recorder 时只接收安全副本，补 instance 归属、显式启用/关闭、分片/压缩、
   7 天 / 200 MB 保留和崩溃清理。不能把离线工具直接挂到每个 live Frame 上当作完整 Recorder。
3. 新增 fixture 时先离线导出再二次扫描；确定目录/格式迁移后接 CI gate。现有自动测试会
   验证 checker，但尚无仓库级 fixture 扫描 workflow。历史证据不得批量重写或伪造 runtime 来源。
4. 扩展 provider/method policy 前先给出版本化字段依据和合成失败用例；审批、Canonical
   用户输入及原始 Patch 仍保持各自协议/字节语义。真实模型和 ACP runtime 门保持不变。
