# 历史工具注入最小实测（2026-09-12）

> 发布范围说明：本文的原始运行资料仅保留本地，公开源码不包含；历史结论不替代当前版本验收。见[实验资料与证据政策](source-evidence.md)。

> 本文记录前置固定事实探针。后续正式 MCP 实现与生产 Driver 验证见 [历史查询](history-query.md)，下文“未替换生产桥接”仅指探针批次。

结论：八家 Agent 的历史 MCP 最小接入探针均已通过真实工具调用与跨进程重新绑定。Antigravity 后续通过工作区配置与私有附加目录完成验证；这不等于生产历史服务或八家 GUI 全链路已验收。此次只新增实验脚本，未替换生产历史桥接，也未修改全局 MCP 配置。

## 方法

本地 stdio MCP 服务仅暴露 searchHistory，返回隔离测试任务的一条随机事实，不提供 Shell、文件系统或网络工具。事实不写入用户提示。每家新建原生会话，通过原生工具目录发现并调用；关闭测试进程后，用同一原生 ID 恢复，注入另一份测试服务配置、返回新的随机事实。调用日志与原生响应联合核对。权限请求只允许该测试工具，不开放其他工具或持久权限。

## 结果

| Agent | 临时接入方式 | 新建调用 | 跨进程重新绑定 |
| --- | --- | --- | --- |
| Kimi | ACP session/new、session/load 的 mcpServers | 通过 | 同 ID、新事实通过 |
| Grok | ACP mcpServers | 通过 | 同 ID、新事实通过 |
| Cursor | ACP mcpServers | 通过 | 同 ID、新事实通过 |
| CodeBuddy | ACP mcpServers | 通过 | 同 ID、新事实通过 |
| Claude | --mcp-config，--resume 原生 ID | 通过 | 同 ID、新事实通过 |
| Codex | thread/start、thread/resume 的临时 config.mcp_servers | 通过 | 同 ID、新事实通过 |
| ZCode | session/create、session/resume 的 mcpServers | 通过 | 同 ID、新事实通过；需要现有 Driver 的模型运行配置恢复 |
| Antigravity | 工作区或私有 --add-dir 下的 .agents/mcp_config.json；精确工具审批 Hook | 两种布局均通过 | 两种布局均同 ID、新事实通过 |

Antigravity 的 `agy --mcp-config … --input-format stream-json --output-format stream-json` 返回退出码 2，错误 `flags provided but not defined: -mcp-config`；没有发送模型消息。帮助仅提供 mcp add/remove/list 等配置管理入口。本次没有修改用户全局配置，亦未证明不存在其他会话级 RPC/插件接入方式。后续工作区配置实测如下，已确认不需要该 CLI 参数。

## 初次探测差异

- Cursor 第一次的 factReturned=false 是脚本未拼接 ACP 文本分片，实际工具调用和完整回答均正确；已按拼接结果修订证据。
- Codex 首次服务 ready，但模型未主动发现工具，回答不可用。允许原生工具发现后，同会话恢复调用通过；另外新建会话复测也通过。生产方案应确保工具可发现，不能把 MCP ready 等同于模型已使用。
- ZCode 恢复请求不能带新建专用 titleGenerationEnabled；修正后，还需执行已有 Driver 的 restore_runtime 逻辑。实验脚本复用现有 runtime-reader/原生配置读取后恢复通过，没有修改原生模型配置文件。

## 正文与范围

七家发出的用户消息均只含测试请求，不拼接 CLI 路径、临时配置文件或历史桥接说明。直接检查 ZCode 原生数据库的两条测试用户消息、Claude 原生 transcript，确认正文无历史桥接前缀；Codex 原生 userMessage 事件也保持原始请求。此次不是八家原生客户端 GUI 展示验收，不保证工具元数据在原生内部日志中不可见。

此次固定事实 MCP 是接入探针，不是生产 history-query 服务。真实历史分页、脱敏、任务隔离、并发、轮次租约撤销、取消/断连失效、从原生客户端续接时的权限与工具可用性，仍需在正式实现后单独验证。测试配置及服务仅处理自建固定事实，不具备访问用户任务历史的能力。

## 证据

- [实验脚本与汇总](../../experiments/history-tool-probe/)
- 七家结果索引（本地证据：`experiments/history-tool-probe/results.json`）
- 原生正文检查（本地证据：`experiments/history-tool-probe/native-message-check.json`）
- runs/20260912-021428：首轮七家探测。
- runs/20260912-021546：恢复测试及 Antigravity 参数探测。
- runs/20260912-021640：ZCode 恢复未补运行配置的失败证据。
- runs/20260912-021641：Codex 新建成功复测。
- runs/20260912-021723：ZCode 完整恢复成功复测。

测试父进程与 MCP 服务进程均已退出；未启动桌面或占用 1420/1431。

## Antigravity 工作区 MCP 补充验证

依据官方 [MCP 文档](https://www.antigravity.google/docs/mcp) 的工作区 `.agents/mcp_config.json` 配置入口，使用本机已登录 agy，未安装 SDK 或改动全局 MCP 配置。

| 场景 | 结果 | 证据目录 |
| --- | --- | --- |
| 工作区配置，默认 request-review | 成功加载并发出 call_mcp_tool，原生权限拒绝；服务调用数 0 | runs/20260912-023226/antigravity-workspace |
| 工作区配置，测试 Hook 仅批准指定 MCP 工具 | 新建、跨进程续接各调用一次，最终回答包含各自的新随机事实 | runs/20260912-023329/antigravity-workspace |
| 私有附加目录配置，cwd 保持独立工作区 | 新建、跨进程续接各调用一次，最终回答包含各自的新随机事实 | runs/20260912-023415/antigravity-workspace |

后两组复用生产桌面审批的原生接入机制：`--dangerously-skip-permissions` 配合匹配全部工具的 PreToolUse Hook。测试 Hook 严格核对 `call_mcp_tool`、ServerName=`dogcodes_history_probe`、ToolName=`searchHistory`，其余工具一律拒绝；四次 MCP 调用都有 Hook 允许记录。此次不是桌面按钮审批验收。

私有目录通过 `--add-dir` 加入，目录内放 `.agents/mcp_config.json` 与审批 Hook；实际 cwd 的 `.agents` 没有 MCP 配置。两组续接均关闭上一进程、保留同一原生 conversation_id、替换配置中的 fixture 路径，验证返回新事实而非复述历史。续接复用了各组配置目录，未验证每轮更换目录或存活进程内热更新。

用户提示仅包含查询请求，不含历史桥接说明、可执行路径、配置路径或随机事实。原生 stream-json 中已核对工具结果及最终回答；未增加原生客户端 GUI 展示验收。

结论：Antigravity 可以继续使用当前 CLI，通过应用私有附加目录接入历史 MCP，无须为此切换 SDK。正式实现仍需处理真实历史服务、并发隔离、租约撤销、同名 MCP 冲突及取消/断连等边界。

脚本：`experiments/history-tool-probe/antigravity-workspace.py`。测试结束移除自建临时目录中的 MCP/Hook 配置，运行证据保留在 runs 下；未修改生产 Driver。
