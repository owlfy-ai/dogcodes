# Kiro CLI 版本检查与更新

2026-09-28。Kiro CLI 接入现有版本栏、升级进度、失败重试、任务/进程占用确认及升级后扫描验证。

## 更新方式

- 本机官方 macOS CLI 安装虽然位于 `Kiro CLI.app` 内，但它自带更新器。校验 App 的 CLI 路径、bundle identifier/executable、原生 manifest 的 dmg/full 安装信息后，将其识别为可原生更新；其他 IDE/App 内置程序保持由宿主管理。
- 检查使用 `kiro-cli debug get-index stable`。该只读命令使用当前 CLI 的原生发布源；解析其 JSON，匹配 `universal-apple-darwin`、`full`、`dmg` 后比较语义版本，不根据其他平台的发布判断是否有更新。
- 当前自动检查/升级覆盖已验证的 macOS 稳定频道；检查 `default_channel` 和只读 `settings app.beta --format json`。测试或未知频道不猜测更新目标，保留明确说明。未声称已验证 Linux/Windows 安装更新。
- 原生索引含历史版本，本机约 1.2 MiB；为此命令单独设置 4 MiB 有界输出上限。截断、无效 JSON、缺少匹配平台或版本都会显示检查失败，不伪装成最新版本。
- 用户点击升级后执行当前安装的 `kiro-cli update --non-interactive`。由原生更新器下载、验证和替换 App；本项目不自行改写已签名 App 内文件，也不通过安装脚本另装一份。
- 占用检查涵盖当前 CLI App 内的 ACP 进程及桌面辅助进程，排除单独的 Kiro IDE。仍按已有确认流程处理占用，不静默结束运行任务。
- 更新后通过原启动入口等待实际版本生效（最多 30 秒），再重新扫描并校验版本；旧版本不变或启动入口暂不可用时不报告成功。

## 验证

- 本机 Kiro 2.24.1 原生帮助确认支持 `update --non-interactive`、`debug get-index`；原生版本索引当前稳定版为 2.24.1。
- 桌面 Rust lib 97 项测试、更新入口及额度卡片 19 项前端测试通过；真实只读版本检查与桌面构建通过。lib/tests Clippy 在沿用前次的 collapsible_if / format_collect 既有告警豁免后通过。
- 测试覆盖官方 CLI App 与其他宿主的区分、跨平台发布过滤、语义版本排序、无效索引、超过 64 KiB 的索引、只读检查不启动更新、非交互更新参数、测试频道拒绝、App 辅助进程范围、版本替换等待与旧版本失败。
- 实际更新使用隔离替身命令验证；本轮未替换用户的 Kiro 安装。运行中的旧桌面需重新打开新构建，才能加载桌面主进程中的更新实现。

依据：[Kiro 官方 CLI update 文档](https://kiro.dev/docs/cli/reference/cli-commands/)、本机 2.24.1 的原生帮助和发布 manifest/index。[官方安装入口](https://cli.kiro.dev/install)仅用于核对资料，未执行下载的安装脚本。
