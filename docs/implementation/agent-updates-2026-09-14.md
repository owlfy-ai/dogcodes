# Agent 版本检查和升级（2026-09-14）

额度监控卡片新增后台版本检查；更新状态合并在原版本号同一行，有新版时显示高亮的 `有新版本 版本号` 文案和无边框升级小图标。检查中/升级中为旋转图标，失败为重试图标，说明放在 Hover 和无障碍标签中。无需升级时不增加展示。App 完成安装探测后开始检查，不等待进入额度页。检查不发起模型任务；每个安装独立缓存，成功缓存 6 小时，失败缓存 1 分钟；前台定时检查周期 6 小时，失败支持手动重新检查。最多并发 3 个检查。

## 安装渠道

| Agent / 渠道 | 检查来源 | 点击升级的行为 |
| --- | --- | --- |
| Cursor CLI | `about --format json` 的 latestStatus/latestVersion | 当前安装的 `update` |
| Grok | `update --check --json` | 当前安装的 `update` |
| Antigravity CLI | 官方安装脚本所用平台 manifest | 当前安装的 `update` |
| Claude 原生安装 | 官方 GCS 发布频道；读取用户 autoUpdatesChannel | 当前安装的 `update` |
| Kimi 原生安装 | code.kimi.com/kimi-code/latest | 已核实 0.42.0 的原生 `__update_download VERSION --manual`；先检查子命令能力，然后扫描启动入口以应用已暂存更新 |
| CodeBuddy / Codex / Claude / Kimi npm 全局安装 | 官方 npm registry 对应包的 latest | 用该安装的 Node 和 npm-cli.js，在原 prefix 安装精确目标版本 |
| 已识别的 Homebrew 安装 | 官方 formula/cask 元数据 | 原 Homebrew 安装的 `brew upgrade` |
| ZCode 桌面内置 CLI | `/api/v1/releases/electron/manifest`，比较 Info.plist 的宿主版本 | 提供打开应用入口，在 ZCode 内完成升级 |
| ChatGPT/Codex 桌面内置 CLI | 当前只识别宿主管理方式，尚未接入其桌面发布检查 | 未确认新版时隐藏入口；需在宿主内检查及升级 |
| 其他/未识别渠道 | 明确标注暂不支持自动更新 | 使用原安装渠道；不会在其他位置新装一份 CLI |

ZCode 的版本接口来源于本机应用 `ManifestUpdateProvider`，未使用 app-update.yml 中的 localhost 占位 URL。接口采用 stable（channel=1）；目前未读取 ZCode 自定义更新源或 preview 设置。上述 Homebrew 和跨平台渠道尚未在真实安装上执行升级验收。

## 升级事务和限制

- Webview 只能提交 installationId；仅主窗口可调用。路径、程序和参数全部由 Rust 根据已探测安装生成，禁止前端提交 shell 命令。
- 安装级互斥 + 安装资源互斥。不同资源可并行升级；共用同一 npm prefix（包括本地 npm 安装）、同一 Homebrew 更新器或同一 Agent 原生更新器的升级仍互斥。npm 和 Homebrew 路径按真实路径匹配，避免软链接绕过。资源冲突时说明原因，完成后可重试。
- 并行升级共同持有核心 workspace mutation lease。有运行任务时仍拒绝开始；直到最后一个升级及重探测结束才释放 lease，避免安装过程中启动新的执行进程。前端仅阻止同一安装重复提交，其他 Agent 的升级和重试入口保持可用。
- 升级前重新读取版本，防止原生自动更新已发生但本应用缓存还旧而造成降级。网络查询和进程都有超时/输出限制。
- npm 使用同一 Node 安装与原 prefix。应用 bundle 中的 CLI 交给宿主更新，不修改签名应用内的文件。
- 后台拥有升级任务，切换页面不会取消；通过 Tauri 事件广播状态。CLI 超时/退出失败、扫描失败、目标版本未生效都不显示成功。
- 对 Cursor / Claude 的版本目录更新，沿升级前匹配的固定启动软链接定位新目录、新 installationId。版本校验通过后刷新 bootstrap，现有按版本缓存的模型列表随版本失效。
- 不修改用户任务的历史会话映射及权限设置。新版可用于后续选择；历史任务若仍引用旧安装，沿现有会话恢复/Agent 切换流程处理。
- 无 sudo、无新登录、无下载后直接执行远程 shell 脚本。原生更新器及 npm/brew 在用户点击时按其原有机制下载和安装。
- 错误返回退出码和经过约束的说明，不将 CLI 完整输出/账号字段暴露给前端。

## 验证

2026-09-14 本机真实只读检查（未执行已安装 Agent 的升级）：

| Agent | 本机 | 最新 | 结果 |
| --- | --- | --- | --- |
| Cursor | 2026.09.10-fd3934a | 同版本 | 最新 |
| Grok | 1.0.30 | 1.0.30 | 最新 |
| Kimi | 0.42.0 | 0.43.0 | 可升级 |
| Antigravity | 1.2.3 | 1.2.3 | 最新 |
| Claude Code | 2.1.269 | 2.1.272 | 可升级 |
| CodeBuddy | 2.150.0 | 2.151.0 | 可升级 |
| Codex | 0.154.0-alpha.6.2 | 未查询宿主发布版本 | 宿主管理 |
| ZCode 桌面 | 3.11.2 | 3.11.2 | 最新 |

- 前端 292 项测试通过；桌面 Rust 46 项测试通过（另有 1 项联网测试默认忽略，单独实际运行通过）。
- 测试覆盖点击升级、重复点击、切页保留状态、错误重试、宿主入口、安装指纹缓存、精确 npm prefix、应用 bundle 保护、目标版本校验、隔离假更新器成功/失败。
- `pnpm --filter ./apps/desktop build` 通过；沿用现有 Mermaid 大 bundle 警告。
- 使用实际 QuotaPanel 渲染的临时预览检查了浅色/暗色卡片、升级提示和按钮。真实安装升级仍需用户点击后验收；未声称已升级这些 Agent。

参考：
- https://github.com/MoonshotAI/kimi-code/blob/main/docs/en/reference/kimi-command.md
- https://antigravity.google/cli/install.sh
- https://code.claude.com/docs/en/setup

UI 补充：宿主管理状态本身不再展示外链图标。只有同时知道宿主当前版本和最新版本，且最新版本更高时，才显示“桌面应用有新版本”和打开应用的小图标。
