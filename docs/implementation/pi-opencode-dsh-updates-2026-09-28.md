# Pi、OpenCode、DSH 版本检查与升级

2026-09-28。三家接入现有额度监控卡片中的版本检查、升级、失败重试和升级后重新扫描流程。检查只读取版本元数据；安装更新仍由用户点击升级触发。

## 安装渠道

| Agent / 安装方式 | 检查来源 | 更新动作 |
| --- | --- | --- |
| Pi 官方托管安装 | `https://pi.dev/api/latest-version` | 通过原托管 launcher 执行 `pi update --self`，由原生更新器管理 release/staging 和激活版本 |
| OpenCode 官方脚本安装（`~/.opencode/bin/opencode`） | npm `opencode-ai/latest` | 当前 executable 的 `upgrade VERSION --method curl`，固定目标版本和原安装渠道 |
| DSH 已缓存 npx 安装 | npm `@deepseek-ai/dsh/latest` | 在原缓存目录执行 npm install，指定精确版本、原 prefix 和工作目录，不迁移为全局安装 |
| 三家已识别的 npm 全局安装 | 对应 npm 包的 `latest` | 沿用原 Node、npm 和 prefix 安装精确目标版本 |
| OpenCode Homebrew 安装 | Homebrew formula/cask 元数据 | 原 Homebrew 的 upgrade |

Pi 托管方式校验 `managed-install.json`、当前版本和包名；不把任意同名启动脚本视为可自更新。DSH 缓存校验 `_npx` 目录、包身份及缓存根 manifest 的依赖声明；项目本地依赖或未知安装来源保持不支持。应用 bundle 内的 npm 包优先归属宿主，不改写应用内文件。

## 运行与恢复

- DSH 本地安装和 Pi 托管更新通过用户登录 shell 解析 Node/npm，检查 npm 包身份，并给更新器设置运行时 PATH，支持从 GUI 启动。
- 沿用升级互斥、任务占用检查和明确的结束进程确认。Pi 进程范围包含当前托管安装的 releases，因此 Node 执行 release 中 CLI 时也能正确识别；不匹配其他 Agent 或提示词中提到的路径。
- Pi 记住托管 launcher，更新切换版本后重新检测入口；不会继续把旧 release 文件当成升级后的版本。
- 更新器退出后重新扫描并校验实际版本，才报告完成。命令失败、网络失败、安装来源未知或目标版本未生效均不显示成功。

## 验证

- 本机只读版本检查及更新命令生成通过：Pi 0.87.1 → 0.87.1；DSH 0.1.7-rc.2 → 同版本；OpenCode 1.15.10 → 1.18.33 可更新。检查没有发起模型调用或修改原生安装。
- Rust desktop lib：92 项通过、5 项忽略。新增覆盖托管 Pi 标记、路径边界、进程范围、三家 npm prefix、DSH 缓存原地更新、命令执行与参数、OpenCode 目标版本/渠道。
- 前端更新入口及额度卡片：19 项通过。类型检查和生产构建通过。
- Desktop lib/tests Clippy 原有 `turn_timing.rs` collapsible_if 和 review 测试 format_collect 警告仍在；本轮检查仅放行这两类既有告警，其余以 `-D warnings` 验证。all-targets 还会触发已有初始化 benchmark 示例的 duration_suboptimal_units 警告。
- 未实际升级用户已安装的 CLI；隔离替身更新器用于验证执行路径。此能力位于桌面主进程，运行中的旧桌面需要重新打开新构建，单独重启 core service 不会加载更新入口的 Rust 实现。

参考：[OpenCode CLI upgrade](https://opencode.ai/docs/cli/#upgrade)，本机 Pi 0.87.1 的 `docs/cli.md`、`dist/package-manager-cli.js`、`dist/utils/version-check.js`，DSH 安装包 manifest 和原缓存目录 manifest。
