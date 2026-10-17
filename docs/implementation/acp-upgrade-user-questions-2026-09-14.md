# ACP 升级、用户问答与会话恢复（2026-09-14）

## 实现结果

按依赖升级 → 现有功能回归 → 用户问答 → 会话恢复/配置保留的顺序实施。

- 工作区 ACP Rust SDK 从 2.0.0 升至 2.1.0；lockfile 固定 derive 2.1.0、schema 1.7.0。继续使用 v1 wire schema，未强制切换现有 Agent 的协议版本。
- 新增独立的 QuestionRequested / QuestionClosed / respond_to_question 驱动接口。问题不能通过普通“允许”按钮回答，权限审批也不能接受问答内容。
- Core 复用现有持久化交互通道，增加可选 question / answer 字段，兼容旧记录。校验必填项、选项原始 ID、字段类型和约束；保留当前任务/turn 归属校验、版本校验、重复提交保护和投递失败记录。
- 主窗口和灵动岛渲染相同的问答表单：单选、多选、可用时自定义输入，以及文本/数字/布尔字段。可取消回答；不会默认选择答案或自动提交。灵动岛收到问题时展开，长表单仅内容区滚动，提交/取消按钮保留在可见区域。后台状态刷新保留正在填写的答案，不卸载同一问题的表单。
- 原生取消请求或 turn 结束后，旧问题失效；问答取消不会转成执行失败。

## 实际协议路径和测试结果

下表“通过”表示真实启动本机已登录的 Agent，收到结构化问题，回传答案后核对最终输出；取消测试单独启动新会话，要求正常结束。不是只检查静态能力字段。

| Agent | 通讯路径 | 回答 | 取消 |
| --- | --- | --- | --- |
| Codex | app-server `item/tool/requestUserInput` | 通过 | 通过 |
| Claude Code | `can_use_tool` / `AskUserQuestion`，回传 `updatedInput.answers` | 通过 | 通过 |
| ZCode | `interaction/requestUserInput`，`action` + `content` | 通过 | 通过 |
| Kimi | ACP `elicitation/create` 的 form 模式 | 通过 | 通过 |
| Grok Build | `_x.ai/ask_user_question`，自定义答案通过 annotations 回传 | 通过 | 通过 |
| CodeBuddy | 支持 `_codebuddy.ai/question`；本机实际触发 `request_permission` 的 AskUserQuestion 兼容分支，使用 `_codebuddy.ai/resolveInterruption` 回答 | 通过 | 通过 |
| Antigravity | 已验证 Hook 拦截 `ask_question`，阻止原生自动跳过，将真实答案放入 Hook reason 交回 Agent | 通过 | 通过 |
| Cursor | 已实现 `cursor/ask_question` 和 `_cursor/ask_question` 的 ID 映射及答案回传 | 未通过原生触发验证 | 未通过原生触发验证 |

Cursor 本机版本为 2026.09.10-fd3934a。默认模式及通过真实 `session/set_mode` 切换的计划模式均报告 `AskQuestion` / `Tool not found`，没有发出结构化问题。因此这里只确认协议适配和选项映射测试通过，不能声称当前 Cursor 原生问答可用。没有把普通文本提问伪装成协议问答，也没有修改用户的全局 Cursor 配置。

Codex 在应用创建/恢复的会话配置中开启 `features.default_mode_request_user_input`，使普通工作模式能够使用提问工具；未更改全局配置。Antigravity 即使在完全访问模式下也保留问答 Hook，普通工具仍按所选权限策略执行。

ACP 仅声明 form elicitation 能力；未声明 URL elicitation。自定义回答、选择数量等能力由实际表单决定，例如本次 Kimi 返回的表单只有两个枚举选项。

## 会话恢复与配置保留

`acp-driver/src/session_state.rs` 按连接和原生会话隔离配置，保留模型、思考强度、权限/工作模式以及服务端响应中其他配置元数据：

- `session/new` 和 `session/load` 使用独立的 60 秒初始化/恢复预算，不再共享普通元数据请求的 15 秒预算。真实回归中发现 Cursor 并发启动偶尔超过 15 秒，修正超时范围后复测。
- 恢复、模型选择和权限选择保留完整响应；缺省字段不清空旧配置，明确返回的空数组则清空对应配置。
- 合并嵌套 modes/models 对象；早到的 current-mode 通知不会丢失 availableModes。
- 恢复期间的新配置通知优先于该次响应；修订号阻止过期的恢复结果覆盖新操作。
- 失败的恢复或配置请求丢弃未提交状态，保留最后确认的配置。不同会话之间不共享配置。
- 继续使用 SDK 的底层 `session/load` 请求，以保留现有全局通知处理、多目录参数和厂商扩展；没有宣称迁移到 SDK 高层 session builder。
- 这是驱动会话状态保留；应用重启后的配置来源仍是原生会话恢复及已有任务配置，不是新增跨账号持久缓存。

真实恢复测试使用临时工作目录、独立原生会话，关闭第一次连接后由新的 Driver 实例恢复原生 session，再要求回忆前一轮内容。最终验收使用随机 token，避免根据项目名称猜中答案。修正初始化预算后，Kimi、Grok、Cursor、CodeBuddy 四项最终验收全部通过。

## 验证与证据

- Rust 工作区 lib 测试通过；后续受影响 ACP/Claude/Store 测试及配置保留专项测试通过。
- `cargo check --workspace --all-targets` 通过，包括补齐示例程序的可选 answer 字段。
- 前端 70 个测试文件、289 项测试通过；类型检查与生产前端构建通过。
- 浏览器人工检查了浅色主窗口和深色灵动岛问答表单，包含四个长问题的滚动及按钮可见性。
- ACP 升级后的 Cursor、Kimi、Grok、CodeBuddy 真实批准/拒绝测试通过，并核对操作是否实际发生。
- Cursor、Kimi、Grok、CodeBuddy 图片内容识别和文件引用的真实回归全部通过。

验收程序：`crates/store/examples/question_acceptance.rs`，参数为 Agent 名称，第二个参数 `cancel` 测取消；仅使用临时会话，不允许读写项目文件、执行命令或委派任务。它需要显式启动，会使用本机登录和少量模型额度。

本机测试日志（不包含认证凭证；临时文件可能被系统清理）：

- `/tmp/agent-acp-upgrade-approvals.log`、`/tmp/agent-acp-upgrade-codebuddy.log`：原有权限回归。第一次 CodeBuddy 缺少测试程序的 executable 环境变量，补齐后通过。
- `/tmp/agent-question-live-*.log`：各原生问答。CodeBuddy 最终成功记录为 `agent-question-live-codebuddy-final.log`；Cursor 的 `*-final.log` 如实记录未触发。
- `/tmp/agent-question-cancel-*.log`：七个 Agent 的取消成功记录。
- `/tmp/agent-acp-attachments-{kimi,grok,cursor,codebuddy}.log`：图片及文件引用回归。
- `/tmp/agent-question-resume-fixed-final.log`：修正启动预算后的四个 ACP Agent 随机 token 恢复验收。之前的 `agent-question-resume-all-final.log` 记录 Cursor 初始化超时，其余三个通过。
- `/tmp/agent-question-workspace-tests.log`、`/tmp/agent-question-regression-final.log`、`/tmp/agent-session-state-final.log`：Rust 回归。
- `/tmp/agent-question-all-targets-final.log`、`/tmp/agent-question-island-final.log`、`/tmp/agent-question-ui-last.log`：编译、前端测试及构建。
- `/tmp/agent-question-desktop-build-final.log`、`/tmp/agent-question-frontend-build-final.log`：最终桌面开发程序和前端编译。

早期会话恢复测试只收集旧 TextDelta，忽略现有 Presentation message 导致空文本断言失败，已更新验收器。Cursor 另有一次恢复后空文本结果；保留失败日志，单独随机 token 重测成功，不以该次空回复作为成功证据。

## 用户升级 Cursor 后复测

再次检查：Cursor 桌面版为 3.20.17；当前 PATH 中 `cursor-agent` 与 `agent` 仍共同指向 `~/.local/share/cursor-agent/versions/2026.09.10-fd3934a/cursor-agent`，CLI 自报版本与此前一致。桌面版版本号不能作为本项目 ACP 所用 CLI 已升级的证据。

重新编译验收程序后，在临时会话中分别测试默认模式和通过 `session/set_mode` 设置的计划模式，两次都没有收到结构化提问请求。默认模式回复工具列表没有该工具，计划模式回复调用 `AskQuestion` 得到 `Tool not found`。因此无法进入回答/取消验收阶段，仍不能标记为支持真实问答；没有自动升级软件或修改全局配置。

日志：`/tmp/agent-cursor-retest-default.log`、`/tmp/agent-cursor-retest-plan.log`。
