# CLI 与桌面版本的升级入口

Windows 的 Codex、Claude 桌面应用会下载自己的 CLI。CLI 的发布版本和桌面应用版本独立：不能拿 npm/CLI 发布源的版本提示去打开桌面应用更新器。

- `available` 表示 CLI 有新版，仅调用 CLI 升级接口；不再回退到打开客户端或官网。
- `managed` 表示已接入桌面发布源。前后端均须确认桌面当前版本和最新版本，并且最新版本更高，才能提供打开应用的入口。
- Windows 桌面附带的 Codex/Claude CLI 首次点击升级后，通过官方 npm 包安装到用户的 `LOCALAPPDATA/dogcodes/cli/<provider>`。不会覆盖桌面应用管理的二进制，也不修改系统 PATH。
- 安装完成后，检查包身份、实际 CLI 版本与目标版本；验证通过才原子写入管理记录，重新扫描并使用独立 CLI。以后点击升级继续更新这份 CLI。
- 已有独立终端 CLI/自定义命令仍按原优先级选择。独立 CLI 已安装但扫描中断时，重试先检查独立版本，防止回退到较旧的发布版本。
- 检查版本不会安装 CLI，安装只由升级按钮触发。原有运行任务检查、安装资源互斥和升级后扫描继续生效。

验证覆盖：Codex/Claude CLI 按钮不打开桌面应用；桌面已是最新或版本未知时不提供打开入口；失败或未验证的 CLI 不被选中；验证后切换到独立 CLI、保留原桌面文件及自定义终端命令；只读检查和安装命令构造不执行安装。

## 其他双形态 Agent 核查（2026-11-01）

| Agent / 安装形态 | 核查结果与处理 |
| --- | --- |
| DeepSeek Harness 桌面附带 CLI（Windows/macOS） | 原来 Windows 落入外部 CLI 检查而不能直接升级，macOS 落入泛化宿主分支。现在复用独立 CLI 安装、校验和切换，不覆盖 Electron 资源。既有 npm/npx 安装保持原更新渠道。 |
| OpenCode 桌面附带/下载的 CLI | 原来 Windows 桌面 2.x 引擎可能与 npm 1.x 发布流比较，误报最新。现在归属桌面更新：读取应用版本及受验证的官方桌面发布源，仅桌面有新版才允许打开应用。已失去宿主的下载型引擎不再使用 npm 版本推断“最新”。保持桌面数据库与 2.x 引擎的兼容选择，不降级到 1.x。 |
| Claude macOS 下载型 CLI | 与 Windows 一致，点击升级后安装独立 CLI，不再原地更新桌面管理的下载目录。 |
| Cursor、Kiro、Antigravity CLI | 当前识别的独立 CLI 使用各自 CLI 的原生检查及升级；Kiro CLI 自身的 `.app` 更新与 Kiro IDE 分开。未发现 CLI 新版打开 IDE 的同类错配。 |
| CodeBuddy CLI | 当前 npm 安装查询及更新同一个 CLI 包；未发现同类错配。 |
| ZCode 桌面附带 CLI | 已使用桌面版本及桌面发布源，已有版本比较守卫；不拿 CLI 版本打开桌面更新。 |

OpenCode 桌面源由安装内 `@opencode/desktop` 包身份与 `app-update.yml` 的官方 URL、provider、channel 共同确认，不信任任意本地 URL。Windows 使用 `latest.yml`，macOS 使用 `latest-mac.yml`；解析桌面 manifest 的顶层版本，不使用其中嵌套的版本或 CLI 版本。未确认的桌面来源保留未知状态，不提供虚假的“最新”或升级入口。

本轮验证：后端相关测试 50 项、终端安装选择 9 项通过；12 个数据库安装的只读版本检查通过。另对本机桌面附带的两个入口单独检查：OpenCode CLI/桌面/官方桌面源均为 2.2.5，正确不提供升级入口；DeepSeek 桌面 CLI 与官方 CLI 源均为 0.2.1-rc.5，正确保留 CLI 更新路线。实际 OpenCode 扫描仍选中兼容的 2.x 桌面引擎。macOS 使用安装布局夹具验证，未做实机验证；本轮没有执行任何 Agent 安装或升级。
