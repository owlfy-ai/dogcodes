<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**所有编程 Agent，一个工作台，随时随地指挥。**

<a href="../../README.md">English</a> · <strong>简体中文</strong> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <a href="../../docs/i18n/README.es.md">Español</a> · <a href="../../docs/i18n/README.fr.md">Français</a> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes 把编程 Agent、对话、代码变更和项目运行命令放进同一个桌面应用。让一个 Agent 实现功能，换另一个检查代码，再运行项目、整理提交，整个流程都能在同一个工作台中完成。离开办公桌后，还能用手机继续指挥这一切。

[官网](https://dog.codes) · [功能](#features) · [手机遥控](#phone) · [开始使用](#getting-started)

## 支持的 Agent

连接你已经在用的编程助手。以下 12 家 Agent 均已有接入；可用模型、审批、用量数据及恢复能力因 Agent 和安装版本而异。请先通过各 Agent 的官方工具完成安装与登录。

<p align="center">
  <a href="https://developers.openai.com/codex/"><img src="../../docs/images/readme/agents/codex.png" alt="Codex" height="36" /></a>
  <a href="https://code.claude.com/docs/en/overview"><img src="../../docs/images/readme/agents/claude-code.png" alt="Claude Code" height="36" /></a>
  <a href="https://cursor.com/"><img src="../../docs/images/readme/agents/cursor.png" alt="Cursor" height="36" /></a>
  <a href="https://www.kimi.com/"><img src="../../docs/images/readme/agents/kimi.png" alt="Kimi Code" height="36" /></a>
  <a href="https://grok.com/"><img src="../../docs/images/readme/agents/grok.png" alt="Grok Build" height="36" /></a>
  <a href="https://z.ai/"><img src="../../docs/images/readme/agents/zcode.png" alt="ZCode" height="36" /></a>
  <br />
  <a href="https://antigravity.google/"><img src="../../docs/images/readme/agents/antigravity.png" alt="Antigravity" height="36" /></a>
  <a href="https://www.codebuddy.ai/"><img src="../../docs/images/readme/agents/codebuddy.png" alt="CodeBuddy" height="36" /></a>
  <a href="https://pi.dev/"><img src="../../docs/images/readme/agents/pi.png" alt="Pi" height="36" /></a>
  <a href="https://opencode.ai/"><img src="../../docs/images/readme/agents/opencode.png" alt="OpenCode" height="36" /></a>
  <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="../../docs/images/readme/agents/deepseek-harness.png" alt="DeepSeek Harness" height="36" /></a>
  <a href="https://kiro.dev/"><img src="../../docs/images/readme/agents/kiro.png" alt="Kiro" height="36" /></a>
</p>

## 把开发流程放在一起

<!-- 截图占位: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## 功能

### 多 Agent 无缝接续任务

**同一个对话中可随时更换执行的 Agent，自动交接上下文。**

- 随时把工作交给最适合的 Agent：擅长架构的负责设计，擅长细节的负责实现。
- 某个 Agent 额度用尽，换另一个 Agent 接着做，无需重新说明需求。
- 交接时会带上之前的对话内容，接手的 Agent 也能通过内置历史工具查阅过往记录，是真正接着做，而不是重新开始。

<!-- 截图占位: ../../docs/images/readme/switch-agent.png -->

### 随时掌握额度与用量

**集中管理 12 种 Coding Agent 的额度、用量、重置时间与版本。**

- 每个 Agent 的剩余额度和重置时间一目了然，方便安排工作，把每一份 token 都用足。
- 统一管理各 Agent 的版本，一键升级。
- 自动发现本机已安装的 Agent，并检测登录状态。

<!-- 截图占位: ../../docs/images/readme/usage.png -->

### 沿用你现有的订阅，无需另购 API

**直接调用本机已登录的 Claude Code、Codex、Cursor 等官方命令行工具，用的是你自己的订阅额度。**

- 不需要单独的 dog.codes 账号，也不必另外购买 API，已有的订阅直接派上用场。
- 本地优先：代码、对话和凭据都留在你的电脑上。可选的手机连接全程端到端加密，中继看不到你的数据。
- 启动 Agent 时只传入必要的环境变量，你的其他密钥不会暴露给 Agent。

<!-- 截图占位: ../../docs/images/readme/accounts.png -->

### 多 Agent 代码审查并修复

**指定另一个 Agent 审查刚修改的代码，减少潜在 bug 与设计缺陷，也可进行安全与性能分析。**

- 不同 Agent、不同模型交叉评审，跳出原有上下文换个视角，避免单一 Agent 在同一上下文中的盲点。
- 评审结果分为高、中、低优先级，选中问题后一键「开始修复」。
- 修复交回主 Agent 执行，并附上审查报告与选中的问题，上下文连贯，改得更准。

<!-- 截图占位: ../../docs/images/readme/review-fix.png -->

### 跨 Agent 分叉，并行比较方案

**从任意一轮分叉出新对话，交给另一个 Agent 尝试不同思路，原对话不受影响。**

- 配合 worktree，不同方案的代码改动彼此隔离。
- 让不同 Agent 对同一问题各出一版，比较后择优采用。

<!-- 截图占位: ../../docs/images/readme/fork.png -->

### 符合使用习惯的 Git 管理

**高度贴近 VS Code 的 Git 操作逻辑，在侧边栏用熟悉的方式管理 Diff。**

- 暂存、取消暂存、丢弃、忽略，Commit 及 Commit & Push，树状与列表视图自由切换。
- 可指定任一 Agent 一键生成 Commit Message，并可指定语言。

<!-- 截图占位: ../../docs/images/readme/git-changes.png -->

### 自动分析项目运行方式，一键执行

**自动分析项目的运行、调试命令或脚本，转成可点击的快速启动项。**

- 不用再记不同项目各自的启动命令，也不必专门打开终端，点一下就能执行。
- 运行输出集中显示，随时查看。

<!-- 截图占位: ../../docs/images/readme/run-project.png -->

### 多目录项目支持

**前端、后端、文档分散在不同目录？一个会话全部交给 Agent。**

- 原本不支持多目录的 Agent，经 dog.codes 管理后也能进行多目录项目开发。

<!-- 截图占位: ../../docs/images/readme/multi-folder.png -->

### 每一轮的用量明细一目了然

**每一轮的 token 消耗、上下文占用、缓存命中与缓存用量清清楚楚。**

- 详细掌握当前会话及每一轮对话的消耗，知道 token 花在了哪里。

<!-- 截图占位: ../../docs/images/readme/turn-usage.png -->

**开源 · 免费 · 跨平台**：Apache-2.0 授权，面向 macOS、Windows 和 Linux 构建。

<a id="phone"></a>

## 用手机指挥你的 Agent

**出门不必带电脑。Agent 在电脑上照常干活，手机始终掌握指挥权。**

手机只需配对一次，下面的一切都能在任何地方完成。你的代码和 Agent 始终留在自己的电脑上。

<!-- 截图占位: ../../docs/images/readme/phone-remote.png -->

- **接续对话或发起新对话。** 接着电脑上的任意对话继续，也可以新建会话：选择 Agent、模型和思考强度，实时查看流式输出，还能搜索、置顶、归档或分叉会话。
- **批准权限，回答提问。** 权限请求和 Agent 的提问会连同推送通知送到手机，点一下 Agent 就继续工作。每次批准都会与电脑上原本的运行核对。
- **审查并提交。** 查看每个项目目录的变更，暂存文件，让 Agent 写好提交说明，再提交、推送、拉取或同步。还能只读浏览提交历史、比较分支。
- **从电脑目录创建项目。** 浏览电脑上的文件夹，选一个添加为项目，不必坐回桌前。
- **在电脑终端里执行命令。** 在任意项目目录打开终端，使用你电脑上真实的 Shell。
- **运行项目，在手机上看效果。** 在电脑上启动运行项，手机里直接打开正在运行的网页应用。
- **浏览文件，发送附件。** 查看项目文件，并在消息中附带文件和图片。
- **需要时开启远程桌面。** 查看电脑屏幕，点按、拖动、滚动、捏合缩放、输入，或粘贴文字和图片，画质会随网络状况自适应。

<!-- 截图占位: ../../docs/images/readme/remote-desktop.png -->

### 连接方式

- 只需扫一次二维码，无需账号。连接全程端到端加密。可配对多台电脑、查看在线状态，并随时在桌面端撤销某部手机。
- 手机能直连时直连，否则回退到 dog.codes 中继服务，中继只转发加密数据。
- 电脑需保持开机，dog.codes 保持打开或最小化。有任务或远程会话进行时，dog.codes 可以阻止电脑自动睡眠，也可以关闭该选项。
- 远程桌面支持 macOS、Windows 和 Linux（X11 与 Wayland）电脑；系统会请求所需的屏幕与输入权限。

<a id="getting-started"></a>

## 开始使用

目前通过源码运行。先安装 **Rust 1.97.1**、**Node.js 22+**、**pnpm 11**，并准备好 [Tauri 平台依赖](https://v2.tauri.app/start/prerequisites/)。克隆或下载本仓库后，在根目录执行：

```bash
pnpm install
pnpm desktop:dev
```

1. 通过官方方式安装至少一个支持的 Agent，并完成登录。
2. 打开 dog.codes，连接可用 Agent，添加项目目录。
3. 新建对话，选择 Agent 和模型，描述要完成的工作。
4. 打开「运行」面板分析项目命令，或查看变更并发起审查。
5. 若要用手机指挥 Agent，请在桌面端打开「连接手机」，并用手机端扫描配对码。

Agent 对话、项目分析、审查和提交说明生成会使用所选 Agent 的账号，可能消耗对应额度。

<details>
<summary>参与开发</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` 启动使用模拟数据的浏览器界面。连接真实 Agent 和运行项目命令请使用 `pnpm desktop:dev`。

</details>

## 更多信息

提交 Pull Request 前请阅读 [CONTRIBUTING.md](../../CONTRIBUTING.md)；安全问题请按 [SECURITY.md](../../SECURITY.md) 私下报告。

Agent 图标复用本仓库已有素材，来源与声明见[图标说明](../../apps/desktop/src/assets/licenses/README.md)。产品名称及商标归各自所有者。
