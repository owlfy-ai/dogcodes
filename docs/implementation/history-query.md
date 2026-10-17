# 历史按需查询（2026-09-12）

## 当前实现

侧栏“搜索”打开当前对话历史，提供分页浏览、持久化对话全文字面搜索、决策、逐轮文件变更及附件证据入口。模型菜单仍不包含搜索。分页按 sequence 升序，afterSequence 排他，首次 throughSequence 固定后沿用；进行中的投影文字可能继续更新。搜索不解释 SQL 通配符，不检索未持久化思考或附件正文；SQLite 对 ASCII 大小写不敏感，中文按字面匹配。

Core 校验任务存在、范围、类型、页长 1–100、搜索文本不超过 1024 字节。SQLite 直接对任务投影分页，不再为搜索整段加载所有时间线。决策/变更查询每页最多扫描 1000 条原始事件；空页仍可能有 nextCursor，UI 允许继续。决策只来自 decision.recorded，不推断用户决定；变更包括失败事件，不能把采集失败解释为无变更。

现有 Artifact 浏览器继续按任务和 hash 校验、32 KiB 分块读取。操作变更 Artifact 加入任务归属投影和读取白名单。当前未发布阶段仅初始化现行数据库结构，不执行旧数据迁移。

## Agent 按需入口（2026-09-12 增强）

每轮仅冻结当前任务的 Canonical Event 序号上界，不再预先加载 16 MiB 历史快照。五个 stdio MCP 工具通过 Core 按需读取 SQLite；取消、原生终态、错误或 Core 退出撤销该轮访问。端点仍按 task/executor 隔离，原生进程复用时下一轮绑定新的上界和 SessionMap。初始化尚未绑定 SessionMap 时不提供查询。

各 Driver 原生 MCP 配置注入方式保持不变：ACP 的 mcpServers、ZCode session 配置、Codex config.mcp_servers、Claude --mcp-config、Antigravity 私有 .agents/mcp_config.json；不向用户消息拼接 CLI 说明，不修改项目或全局 MCP 配置。原生权限策略保持生效。

- `getTurns/searchHistory` 读取持久化用户 Prompt、Assistant message.delta、测试及工具/审批/Artifact 关联证据（后者保留原始事件事实）。从上界内的 Canonical 片段按原生消息身份重建完整文字，而非读取可能继续变化的 Timeline 投影；后来追加到同一消息的文字不会泄入旧上界。SQLite 字面搜索支持跨片段短语，ASCII 大小写不敏感；GUI 仍使用 Timeline 全文字面搜索。
- `getDecisionLog` 只读显式 decision.recorded；`getChangesSinceCheckpoint` 返回采集成功和失败的原始事件事实，引用不代表已读取附件正文。
- 四类列表单次最多返回 100 条；决策/变更每次最多扫描 1024 个源事件，对话搜索由 SQLite 聚合冻结消息后筛选分页。总历史不再有 20,000 条、200 页或 16 MiB 上限。响应有 256 KiB 预算；单条文本最多返回 32 KiB UTF-8 字节。
- 大文本返回 `nextOffset`；调用者使用 `afterSequence=nextCursor, offset=nextOffset` 继续同一条事实。完成该条后将 offset 重置为 0。空扫描页也可能有 nextCursor，应继续查询；只有游标为空才表示本查询已读完。
- `getArtifact` 按任务归属、来源序号上界、元数据和 SHA-256 校验附件，32 KiB UTF-8 分块返回。保留既有单附件 1 MiB 存储/读取预算；非 UTF-8 正文仍通过 GUI 读取。取消的是全历史快照上限，不是单附件格式与完整性边界。
- 查询按已存储内容读取，不额外脱敏；不还原入库时已被脱敏的数据。MCP 子进程不直接打开数据库或缓存历史。

## 逐事实交付与自动交接

交接包最多保留 512 个自动交付范围，超出部分保守视为未确认；单次 MCP 响应也按最多 512 个源片段分块，后续通过 nextOffset 继续。

ContextPackage 新增 `deliveryFacts`：每项记录源 sequence、字段、完整源文字 SHA-256、offset/end 和总字节数。构建对话时直接跟踪源片段位置，不能通过相同文字的出现来推断另一条事实已交付。截断仅记录实际包含的前缀；脱敏改变原文字节时不确认原文覆盖。结构化决策与测试/变更原始元数据分别记录；文件引用和净变化摘要不确认附件全文覆盖。人工编辑的上下文清空自动覆盖声明，保持保守。

交付证据保存为 Canonical `history.facts_delivered`，按具体 SessionMap 隔离：

- `driver_prompt_accepted`：Driver.prompt 返回流后记录随 Prompt 交付的事实范围；调用失败不记录。它表示 Driver 接受调用，不代表供应商确认模型理解。
- `mcp_stdout_flushed`：查询返回临时 receipt；MCP 桥成功写出并 flush 工具结果后，带 receipt 回传 Core，Core 持久化已返回的事实字节范围。仅查询、拒绝审批、写出失败、过期/未知 receipt 不建立覆盖证据。最多保留 64 个待确认响应，超出后背压报错。进程崩溃丢失确认时允许保守重交，不假装已交付。
- `history.native_prompt_accepted` 记录本 SessionMap 的原生轮次；该轮用户输入和原生生成的消息不再作为跨 Agent 漏交事实。

切回旧 SessionMap 时，从冻结上界之前的事实中扣除该会话完整已交付的同哈希字节范围及自身原生轮次；部分范围合并后仍有缺口则继续保留。`lastSeenSequence` 保留为原生执行检查点和路由冲突检查，**不再决定交付覆盖或过滤漏交历史**。新 SessionMap 不继承旧会话确认。

交接 Prompt 仍有最近 10,000 条候选事实及文本/变更预算，被省略事实保持未交付，可由 MCP 按需读取。不会自动无限扩张 Prompt，也不会自动执行历史指令。收据直接来自事件流，数据库重开和显式投影重建保留；没有新增旧数据库升级或迁移。

当前增量计算分页扫描事件，并在内存合并该会话的交付范围；超大收据集合仍可能增加切换成本。MCP 响应独立按页读取，不依赖这一集合。原生子进程已经收到的数据不能通过撤销取回；确认也不表示模型记住、理解或采纳。

## 本次增强验证结果（2026-09-12）

- `cargo test -p agent-core -p agent-store --lib --test history_mcp --test multi_root_history`：67 项通过（Core 26、Store 37、MCP 集成 1、多目录集成 3）。
- MCP 集成使用隔离的真实 SQLite 文件与 WaitingDriver：18 条各约 1 MB 的消息，总量超过旧 16 MiB；验证远端历史读取、完整 UTF-8 分块拼回、跨片段短语搜索、冻结后同消息追加隔离、部分范围不能当完整交付、重复 receipt 拒绝、数据库重开/投影重建、不同 SessionMap 隔离、错误 token、附件上界及取消撤销。该测试通过 TCP 查询/确认 Core，不冒充真实供应商或 GUI 验收。
- Core 测试覆盖相同文字的不同来源不误确认、预算截断不切断 UTF-8、五工具协议与参数校验、二进制快照不进入自动上下文；已有 A→B→A 交接与失败/取消回归保持通过。
- `pnpm typecheck`、`pnpm test`（198 项）、`pnpm build`、`cargo check -p desktop` 通过。Vite 仍有既有大 chunk 提示。
- 普通 `cargo clippy -p agent-core -p agent-store --lib --no-deps` 可完成，存在警告；严格 `-D warnings` 被未改动 Driver API 的既有 must_use/文档/嵌套 if 等警告阻断，不记作无警告通过。
- 本次未重跑八家真实 Agent、MCP 子进程 stdout 到真实供应商消费链路或原生 GUI；下方真实验收记录属于增强前版本。交付确认仅按上述明确边界解释。

## 验证

- Core：分页、UTF-8 分块、任务内 Artifact ID、嵌套二进制版本排除、查询参数边界。
- SQLite + 真实临时文件夹：两目录同名文件的净变更、历史字面搜索、游标无重复、错误任务拒绝、持久化起点恢复。
- 前端：分页固定序号上界、切换任务丢弃迟到响应、空扫描页继续加载、双目录同名 Diff 路径分离。
- 浏览器 Demo：搜索入口、搜索命中和版面检查通过；不等同于原生桌面或真实 Agent 证据。
- 真实 Agent 验收脚本：`crates/store/examples/live_multi_root_history.rs`；隔离目录与数据库，三次真实请求，审批逐项查看后选择一次，不开启全权限。

真实矩阵与剩余工作以主实施进度置顶记录为准。

### 修订前 CLI 入口的历史验收结果

2026-09-11 `live_multi_root_history` 完整通过：Codex 在两个隔离目录分别写入同名文件；Kimi 经单次审批实际执行 getChangesSinceCheckpoint 和 getArtifact，工具返回包含附加目录旧标记；Kimi 写入新标记；切回 Codex 后同时确认附加目录新值和主目录未变值。以 CLI 返回事件、磁盘内容和最终对账共同断言，没有用最终答复是否复述旧值替代工具证据。早期试跑遇到脚本误设断言及三分钟审批等待超时，修正脚本后重新完整通过；没有改为全权限或自动批准所有操作。

## MCP 改造验收

当前历史 MCP 查询不再执行额外脱敏过滤。它保留仓库中已有的内容；若内容在入库时已被其他流程脱敏，本次改动不会还原。事件入库、诊断导出及交接包等其他流程的既有策略不在此次改动范围内。

确定性测试覆盖五工具协议、通知与未知工具、分页及 UTF-8、任务端点隔离、错误令牌、租约撤销、下一轮快照替换和端点关闭。SQLite/Core 集成测试验证用户消息保持原样、配置通过 SessionRequest 带外注入、取消后拒绝查询。

正式 Core + Driver + stdio MCP 的真实验收脚本为 `crates/store/examples/live_history_mcp.rs`：隔离工作区与数据库，先记录随机事实，关闭原生 Driver 后重新实例化并用同一会话 ID 查询；联合断言原生工具事件和最终标记。只批准指定 searchHistory 工具，拒绝其他操作。结果保存在 `experiments/history-tool-probe/production/`。该批证据验证增强前的 MCP 注入和原生查询，不用于宣称本次增强已通过八家真实 Agent 重验；本次确定性回归单独记录。

### 增强前正式 MCP 真实结果（2026-09-12，历史证据）

八家均通过跨进程续接后真实历史查询，原生会话 ID 不变。验收检查工具调用、工具返回中的短随机事实、最终答复三项，不把单纯原生记忆复述算作通过。

| Agent | 同 ID 续接 | MCP 返回历史事实 | 证据批次 |
| --- | --- | --- | --- |
| Codex | 通过 | 通过 | 030024 |
| ZCode | 通过 | 通过 | 030024 |
| Claude | 通过 | 通过 | 030829 |
| Antigravity | 通过 | 通过 | 030024 |
| Kimi | 通过 | 通过 | 030024 |
| Grok | 通过 | 通过 | 030024 |
| Cursor | 通过 | 通过；通过 stdio 透明转发日志补核，原生事件省略返回正文 | 030829 |
| CodeBuddy | 通过 | 通过 | 030024 |

批次完整前缀为 20260912-，目录在 experiments/history-tool-probe/production/。机器可读索引为该目录 results.json。透明转发器只用于隔离验收，实际服务仍是正式 Core/桌面 MCP 实现，生产运行不依赖 Python。

试跑发现并处理：Kimi 截断过长服务名；Antigravity 私有配置与审批 Hook 合并；Claude 的后台连接/延迟工具加载导致工具未发现，改为服务器级 alwaysLoad，保留其他 MCP 的加载策略。Claude 的用法依据 [官方 MCP 文档](https://code.claude.com/docs/en/mcp#exempt-a-server-from-deferral)。早期测试脚本错配审批语义、长随机事实被脱敏、仅检查最终回答导致的弱断言均已修正；025253/025643/030433 批次不计最终通过。

验证完成：相关 Core、Driver、Store 单元与集成测试通过，cargo check --workspace --all-targets 通过。测试进程和 MCP 子进程均已退出，未启动桌面 GUI 或占用开发服务端口。
