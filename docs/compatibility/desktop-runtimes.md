# 桌面运行时优先级与桌面单独安装验证

验证日期：2026-11-15，macOS arm64。

## 当前选择规则

Claude Code、Codex、ZCode、DeepSeek Harness、OpenCode 优先选择通过产品身份、
版本和支持范围校验的桌面运行时，再回退独立 CLI。桌面运行时也优先于 DogCodes
管理的独立 CLI；OpenCode 不再限定只有独立 CLI 1.x 才能切换到桌面版本。
Mac 用户 Applications、系统 Applications 和已有 Windows 桌面路径均参与选择。

“桌面运行时”包括 App 内置程序和桌面客户端下载、维护的原生程序，不表示向正在
打开的 GUI 窗口发送消息。运行时一经选择，对话沿用该安装对应的原生协议。
凭证来源与运行时来源分别处理：借用桌面凭证的独立 CLI 仍然显示 CLI。

Welcome 与 Agent 管理卡片在名称后显示“桌面应用”或“CLI”，悬停显示所选程序路径。
本机与远程管理页复用同一标识。未发现可执行文件时，CLI-only Agent 仍显示
“支持 CLI”，安装链接显示“安装 CLI”；Welcome 提示“未检测到 CLI”，管理页说明
仅安装桌面客户端无法使用，避免将 CLI 未安装误认为桌面程序检测故障。

## 四个桌面程序的隔离验证

每个进程使用临时 HOME、配置与缓存目录，以及不包含用户 CLI 的受限 PATH。
Cursor 下载入口用失败的 curl 替身拦截，未下载安装 CLI；其他入口仅检查协议参数、
帮助和启动行为，没有模型请求。CodeBuddy、Kimi 使用自带的后台 Electron Helper，
没有启动 GUI 主程序。

| 桌面程序 | 本机版本 | 实测入口与结果 | 当前 DogCodes 桌面单独安装可用性 |
| --- | --- | --- | --- |
| Cursor | 3.30.8 | `app/bin/cursor agent --help` 在私有 HOME 找不到 `~/.local/bin/cursor-agent`，尝试下载独立 CLI；拦截后退出 1 | 不可用，需要 Cursor Agent CLI |
| Antigravity | 2.31.2 | `Resources/bin/language_server --input-format stream-json --output-format stream-json` 退出 2：`flags provided but not defined: -input-format` | 不可用，现有 stream-json Driver 需要独立 `agy` |
| CodeBuddy | 4.12.0 | 自带 Electron 运行 `app/out/cli.js --help` 成功，但只提供 `buddy [options] [paths...]` 编辑器入口，不是现有 `codebuddy --acp` | 不可用，需要 CodeBuddy Code CLI |
| Kimi Code | 1.1.3 | 自带 Electron Node 模式运行 `app.asar/out/main.cjs acp` 退出 1，桌面入口依赖 Electron GUI 模块；包声明也没有独立 CLI `bin` | 不可用，需要提供 `kimi acp` 的独立 CLI |

这些结果验证了当前桌面包不能直接替代已接入的 CLI 协议；不代表未来不能实现专用
桌面协议适配。Windows 路径优先级由离线目录与进程夹具覆盖，本轮未做 Windows 实机验证。

## 本地复验

普通回归不访问下载地址、不要求安装这些 Agent：

```sh
cargo test -p agent-registry --lib
cargo test -p desktop --lib terminal_installation::tests
pnpm --filter @dogcodes/desktop exec vitest run src/agentRuntimeSource.test.ts src/AgentWelcome.test.tsx src/quotaComponents.test.ts src/agentPreferences.test.tsx src/Tooltips.test.tsx src/tooltipConvention.test.ts
```

已安装环境的选择校验单独显式运行，不发送模型 Turn：

```sh
cargo test -p desktop --lib current_desktop_adapters_are_preferred -- --ignored --nocapture
```
