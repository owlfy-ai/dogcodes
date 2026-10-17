# Agent 对话渲染实现与验收

日期：2026-09-11。实现已接入桌面应用代码；未替换或重启用户正在使用的已打包应用。

## 已接入

- 主对话和子任务正文共用 MarkdownMessage：GFM、代码高亮和复制、KaTeX 行内/独立公式、Mermaid 图表/源码/放大缩放。HTML 不执行，数学渲染 trust=false，Mermaid strict；公式分隔符在代码外规范化。
- 已完成的 Mermaid 块保持组件身份，后续文本增量不重绘；未闭合围栏或公式先显示源码，错误在块内降级。图表按需加载并串行渲染，异步结果过期后丢弃。
- 思考文本仅在 Core 内存中短暂保留，通过 timeline 读取叠加到活跃展示数据；不进入 canonical log 或 SQLite 投影。结束后移除正文，保存观测耗时。跑马灯以约 160px/s 单次通过，新增量不重启动画，支持暂停和减弱动态效果。
- 新增 allowlist 原生事件解析，保留工具输入、输出、状态和明确的父子关联；分类展示读取、搜索、网页搜索、终端、修改等。连续成功的只读操作可折叠为查阅组；失败显示详情。
- 文字在工具前后分段，既有工具更新保留首次位置；终态不会被晚到的 running 覆盖。来源有事件 ID 时去重，ACP 消息与工具保留 session 归属。
- 点击子智能体打开右侧栏。Claude、ZCode 的父工具关联与 Grok 的子 session 关联进入同一详情视图；Cursor、Kimi 缺少完整详情时明确提示。Codex 使用只读 thread/read；Antigravity 按本次已知 child ID 读取固定子日志路径，标注部分记录，不把派生完成当成执行完成。
- 每次操作后检测到的工作区变更保存为独立 artifact，显示文件、统计、Diff 和打开按钮。原生提供的编辑片段也可直接查看。快照不能证明单个并行操作的写入归属，因此明确标注为操作期间检测到的工作区变化。
- turn 汇总使用既有开始/结束快照的 artifact；按文件展示净变化，不累计每次编辑的行数。复用 StructuredDiffView，并对 artifact 分段、大小和 SHA-256 做校验；非 Git 文件使用冻结 before/after 内容生成文本 Diff。
- 打开文件先验证属于当前任务工作区且文件仍存在，再交给文本编辑器；已删除文件禁用打开。macOS 用 open -t，Windows 用 notepad，Linux 用 gedit（未安装时显示打开失败）。
- 对话增量投影刷新合并到约 40ms；阅读历史时不强制滚动，提供回到最新。侧栏开关不触发模型运行。

## 主要代码

- `apps/desktop/src/MarkdownMessage.tsx`、`mathSyntax.ts`：正文与图表公式。
- `ConversationActivity.tsx`、`ThinkingTicker.tsx`、`ConversationScroll.tsx`：时间线与子详情交互。
- `TurnFiles.tsx`、`conversationDiff.ts`：冻结变更与文件操作。
- `crates/driver-api/src/presentation.rs`：各通道结构化展示数据提取。
- Core：临时思考缓存、分段、子记录读取、操作区间 artifact。
- Store：activity 更新合并、文件汇总投影、首次排序位置。数据库版本升至 6，升级时重建派生投影，原始 canonical log 保持不变。

`TimelineItem` 的既有 wire 结构兼容保留；新增 activity kind 的 text 承载版本内结构化 JSON，由 activityOf 解析。旧 kind 继续使用原渲染。后续扩展可将展示对象提升为独立协议类型，当前不要求旧记录包含新字段。

## 验证记录

- desktop 前端全套回归：28 个测试文件、136 项通过；其后新增流式图表稳定性、未闭合公式、冻结 turn Diff 用例，渲染专项共 10 项通过。
- Core / Store / driver-api 及五个驱动 crate 的非 live 单元与协议测试：109 项通过。需要本机账号/模型的 31 项 live 测试保持 ignored，本轮未重新调用收费模型。
- workspace cargo check 通过；desktop TypeScript 与 Vite production build 通过。
- 浏览器视觉检查使用实际渲染组件的开发预览：子详情打开与关闭、缺少详情提示、单次 Diff 展开、Markdown 表格、高亮代码、Mermaid、行内与独立公式、深浅主题。检查中发现并修复了公式内联布局样式丢失及深色 Diff 对比度问题。
- 专项用例覆盖：原始 HTML/危险链接不执行、代码与金额中的美元符号、思考终态无正文、明确父子关联且跨 turn 不串线、晚到状态不回退、消息分段与投影重建、artifact 内容校验、图表后续增量不重复绘制。

## 边界

本轮验证覆盖接入代码、协议形状与渲染组件，没有对所有已安装 Agent 再跑一次真实端到端写文件任务。供应商不发送的事件不能补造。Antigravity 日志只映射已识别的调用/结果，可能不包含完整过程。Codex 详情为按需历史读取，可手动刷新，不冒充已验证的实时子流。

操作快照沿用既有有界工作区捕获策略；并行工具和用户同时编辑时只能保证区间变化，不能保证逐条写入因果归属。快照超限/不可用时，原生编辑信息仍可显示，但不得声称已完整捕获所有文件；超限 artifact 不生成伪造统计。新增快照主要增加有文件变化的 artifact 存储，并在工具结束时增加扫描开销。

开发视觉入口：`apps/desktop/rendering-preview.html`，由 Vite 开发服务器提供；示例数据明确为验收内容，不是用户真实任务。该入口不在 production build 的应用入口中。

实现参考：[react-markdown 官方文档](https://github.com/remarkjs/react-markdown)、[Mermaid 使用与安全配置](https://mermaid.js.org/config/usage)。
