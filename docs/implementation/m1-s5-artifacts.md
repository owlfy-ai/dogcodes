# M1-S5：受限 Artifact 查看与原始字节导出

更新日期：2026-09-03

## 实施状态

落实详细设计 §7.4、§10.4、§17.1、§19.2–§19.3：已实现 Task 归属校验、分页元数据列表、
32 KiB 分段读取、只读内容弹窗、预览后的原始字节导出。当前是证据查看，不是 §17.7 的结构化
Review 系统。只读文件/hunk/行号与原始视图切换现已完成，详见
[`m1-s5-diff-view.md`](./m1-s5-diff-view.md)；仍没有评审 Findings、采纳或写回。

后端/前端回归与桌面启动通过；Browser 实测通过空列表、主动选择、JSON 格式化预览、完整
hash 状态和确认前后按钮门槛。Computer Use 未识别未打包的桌面开发进程，原生保存对话框的
保存/取消交互尚待手工或可识别打包应用验收，不能将文件写入单测当成该 UI 验收。

## 接口与预算

| UI 命令 | 输入 | 返回 / 限制 |
|---|---|---|
| `core_task_artifacts` | Task ID、可选复合 cursor、limit | 1–100 条元数据；默认 20；无正文 |
| `core_read_artifact` | Task/Artifact ID、已审阅 SHA-256、offset、limit | 单段 1–32768 字节；最多 1 MiB Artifact |
| `core_export_artifact` | Task/Artifact ID、已审阅 SHA-256、敏感内容确认 | 原生路径选择；`saved` / `cancelled`；不返回绝对路径 |

- 三个桌面命令都由注入的 WebviewWindow 校验 `label == main`，不能由 UI 自报窗口或 actor。
- Core 的列表、内容及导出校验服务只接受 `LocalUser`；拒绝 `SystemRecovery` 和配对设备。
- Artifact/Task ID 经过强类型 ULID 反序列化；read/export request 使用 `deny_unknown_fields`，
  不能夹带 `path`。UI 不获得任意文件系统读写或 dialog/fs 插件权限。
- cursor = `(sourceSequence, artifactId)`，SQL 对两者倒序作 keyset pagination。同一事件的
  原稿/最终稿拥有相同 sequence，不能仅按 sequence 翻页，否则会丢掉其中一份。
- offset 恰好等于正文长度返回空 bytes/EOF；超过长度、0 或过大 limit 均拒绝。
- `ArtifactChunk.bytes` 是 JSON byte array；不把原始 Patch 强行转为 UTF-8。当前总量上限使
  Core 可以先校验完整内容，再切片返回；没有引入大文件流式 CAS 或无界 UI API。

## Task 归属 Projection 与迁移

- 数据库 `user_version = 2`，新增 `task_artifacts`，主键 `(task_id, artifact_id)`，引用
  tasks、artifacts 和 Canonical source sequence；存储冻结 Event metadata，不重复 blob。
- 只从 `handoff/reconciliation.package_confirmed`、`workspace.diff_captured`、`test.completed`
  投影归属；要求 payload Task 与 Task stream 完全匹配。
- Context 的 `artifact` 和 `originalArtifact` 都可查。编辑原稿 sourceKind 以 `.original`
  结尾，UI 区分“Hub 原稿”和“已确认”；未编辑时相同 Artifact ID 只列一次。
- version 0 先创建旧表，version 0/1 再建 v2 表、replay 全部 Projection、设置版本，均在同一
  Immediate 事务内完成；失败整事务回滚，未来版本拒绝。Event/CAS/dedup 不被回填改写。
- `rebuild_projections` 先清空 task_artifacts 再重建，外键顺序已覆盖。

## 内容完整性与显示

1. Core 查询 Task 对 Artifact 的显式归属；其他 Task 的 ID 返回不可用，不先读取正文。
2. 核对已审阅 hash、允许的三种媒体类型、1 MiB 上限。
3. 从 CAS 读取。SQL 的 CASE 防止损坏数据库把超预算 blob 无界拷入 Rust `Vec`。
4. 再核对 Event metadata 与 CAS metadata、实际字节数及完整 SHA-256。createdAt 按 SQLite
   毫秒精度比较，避免 Event 纳秒精度造成假阳性。失败不返回任何正文，也不开放导出。
5. UI 验证段的 Artifact ID/hash/offset/字节值/EOF，防止重复、乱序或错误版本拼接；收到
   全部字节后再做浏览器 SHA-256，成功才允许用户勾选导出确认。

弹窗仅用 React 文本节点，不渲染 HTML/Markdown、不解析 ANSI、不自动加载远程图片。
危险控制符和双向字符显示为可见转义；非 UTF-8 显示明确告警及最多 256 字节十六进制。
JSON 仅在完整读取后格式化显示，原始字节不改写。选择其他 Artifact 或关闭弹窗后，旧异步
响应不能覆盖当前选择；正文不自动进入 Bootstrap、Timeline 或 Context Package。

## 导出路径和文件生命周期

- Core 重新校验同 Task、Artifact ID 和审阅 hash 后才打开对话框；保存期间有单实例 mutex，
  避免重复对话框。原生对话框绑定主窗口，使用安全的 Artifact ID 派生文件名。
- 路径仅来自 Rust 侧原生保存对话框，取消返回 `cancelled`，不写文件。
- 同目录创建私有临时文件，写入完整原始 bytes 并 `sync_all`，再 `persist_noclobber`。
  既有文件和符号链接均不覆盖，失败提示选择新文件名；中途失败由临时文件 RAII 清理。
- Unix 单测确认新文件权限 0600。该文件生命周期不宣称断电下的完整目录 fsync 保证。
- Rust 端引入 `tauri-plugin-dialog`（lockfile 2.7.3）和 `tempfile`（3.27.0）；没有安装 JS
  dialog/fs 包或给 WebView 增加插件权限。实现参照 [Tauri 官方 Dialog 文档](https://v2.tauri.app/plugin/dialog/)。
- Demo 只保留浏览器本次确认产生的内存 Context Artifact；ID 已使用 ULID，metadata ruleset 0
  如实表示 Demo 未执行真实脱敏。它拒绝真实 Git 固化和导出，不能伪称完成本地文件写入。

## 验证证据

- 当前 Rust workspace：106 passed，1 ignored（含后续 Redaction v2/结构化诊断）；fmt/clippy 通过。前端：25 passed、typecheck/build 通过
  （其中新增 8 项结构化 Diff 回归，详见独立实施记录）。
- `local_artifact_reads_enforce_task_hash_budgets_and_integrity`：跨 Task/actor 拒绝、错误 hash、
  offset/limit/EOF、非 UTF-8 字节重组、正文/metadata 损坏、超预算 blob、v1→v2、rebuild，
  且只读操作不改变 Task/sequence。
- Context 编辑测试：同 sequence 的原稿/最终稿以 limit 1 翻页不丢失，均可验证读取；Test
  Artifact 测试验证脱敏报告走同一检索服务。
- Protocol 测试拒绝任意路径作为 ID 或额外参数；桌面测试拒绝非主窗口、覆盖已有文件及
  symlink，证明原始 bytes 和权限正确、失败没有遗留临时文件。
- `artifactView.test.ts`：段边界/错版本、UTF-8 跨段、非 UTF-8 hex、HTML/ANSI/双向字符、
  全文 hash 与 JSON 仅显示格式化；Demo 测试覆盖归属/hash 与不可导出。
- Browser 实测 1280×720：空列表、Context 元数据、点击后读取、完整 hash、勾选前禁用/后启用，
  JSON 格式化的热更新可见。未把浏览器 Demo 当成 Tauri IPC 或原生保存端到端证据。
- Tauri/Vite 启动成功；真实 app data 只读查询 `user_version = 2`、`quick_check = ok`，
  task_artifacts 表存在；开发进程已主动停止。本轮没有真实模型 Turn、额度消费或登录。

## 下一 agent 的优先接手步骤

1. 完成 macOS 原生保存/取消与已有文件拒绝的 UI 冒烟；可使用可被 Computer Use 识别的本地
   打包应用，仅使用临时测试项目，不修改用户原有 Artifact；记录选定路径、导出 hash 和取消
   无写入。Windows 同等 UI 验收仍未完成。
2. 只读结构化 Diff 已完成，继续保持 Artifact/hash 绑定、不应用补丁、不把原始净差异当成
   双层 Checkpoint。后续完整 Review 工作流见 M2 设计；当前实现/测试入口见
   [`m1-s5-diff-view.md`](./m1-s5-diff-view.md)。
3. 在为 Agent 开放检索前实现设计 §11.1 的 ContextAccess 审计/权限，不能复用这里的本机
   UI actor 作为 Agent 或硬件授权。快速切换/关闭和大段内容已有可复现浏览器夹具与实测，
   后续可固化为 CI 端到端套件；不能把该内存夹具当成原生 IPC/保存证据。
