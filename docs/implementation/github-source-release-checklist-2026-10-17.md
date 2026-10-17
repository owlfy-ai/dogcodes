# GitHub 源码发布检查单

日期：2026-10-17。对应 R2/R3 的源码部分。本文给出公开源码前已核实的项目、仍需维护者本人执行的动作，以及安装包首发前仍未完成的门槛。状态以 [当前工作清单](../CURRENT_WORK.md) 为准。

## 1. 已核实（源码可公开）

| 项目 | 结果 |
| --- | --- |
| 发布范围 | 新仓库只公开整理后的快照，不带阿里云历史。快照由 `scripts/prepare-source-snapshot.mjs` 从**干净的 HEAD worktree** 导出，避免带入本机未提交改动；导出 1073 个文件并生成摘要清单 |
| 凭据与隐私 | 全部受跟踪文件按常见令牌格式（OpenAI/GitHub/AWS/Slack/Google、私钥块、JWT）扫描，仅命中脱敏模块与存储测试中的合成样例；本机用户名、邮箱和私有远端地址无命中；本机签名文件被忽略且发布输入检查通过 |
| 仓库卫生 | 根目录只保留构建与说明文件；设计/验收文档在 `docs/`；与产品无关的页面已停止跟踪；全库 Markdown 相对链接无失效 |
| 社区文件 | `LICENSE`（Apache-2.0）、`THIRD_PARTY_NOTICES.md`、`CONTRIBUTING.md`、`SECURITY.md`、Issue/PR 模板；九种 README 链接到贡献与安全说明 |
| 自动化 | `desktop.yml`：PR/推送时检查并构建三平台未签名安装包；`release.yml`：`v*` 标签校验四处版本一致，三平台构建、检查、测试并生成带校验和的草稿预发布。所用 `actions/*` 版本均已确认存在 |
| 数据兼容 | 数据库格式号与“新格式只读拒绝、不修改”的规则已实现并写入 `release/README.md` |
| 干净构建 | 最终提交 `d3dc07f` 已复验，见第 5 节：快照目录内冻结锁文件安装、前端检查、发布脚本测试、Rust 格式/严格 Clippy/全工作区测试的结果 |

## 2. 已公开（2026-10-17）

经维护者确认，源码已发布到 <https://github.com/owlfy-ai/dogcodes>（公开）。该仓库在 2026-10-16 已建好且为空，未覆盖任何内容。推送内容为提交 `d3dc07f` 的干净快照（1077 个文件，与第 5 节复验的快照逐文件一致，仅导出时间不同），作为 `main` 上的首个提交 `92fa973`；作者使用 GitHub noreply 邮箱，未公开个人邮箱。已开启 Private vulnerability reporting。首次推送触发 `Desktop checks and unsigned packages`（run 39076082426），结果见第 6 节。

## 3. 原先需要维护者执行的动作

1. 在 GitHub 新建仓库并确定名称与可见性。
2. 从干净 worktree 导出快照并推送（示例，路径按需调整）：

   ```bash
   git worktree add --detach ../dogcodes-release HEAD
   cd ../dogcodes-release && node scripts/prepare-source-snapshot.mjs --output ../dogcodes-github
   cd ../dogcodes-github && git init -b main && git add -A && git commit -m "Initial public source"
   git remote add origin https://github.com/<owner>/<repo>.git && git push -u origin main
   ```

3. 在仓库设置中开启 **Private vulnerability reporting**（`SECURITY.md` 依赖它），按需设置 `main` 分支保护。
4. 确认首次推送触发的 `Desktop checks and unsigned packages` 工作流在四个平台通过；失败时据日志修复后再公开宣传。
5. README 中的截图位为 HTML 注释，不影响渲染；素材按 [README 维护](../README-MAINTENANCE.md) 由维护者补充。

## 4. 安装包首发（R1/R3）仍未完成

- Windows、Linux 尚无实机安装验收；macOS 只有本机构建。三平台核心流程与 DT-08/09/10 的实机验收（关窗后任务继续、重开附着、双客户端共享状态）仍待完成。
- Windows 与 Apple 签名/公证凭据按约定留空；未签名包会触发 Gatekeeper/SmartScreen 警告，`release.yml` 的草稿说明已写明。
- 推送 `v0.1.0` 标签前需先完成上述验收；工作流只生成草稿，需人工审阅后发布。

## 5. 干净快照验证记录

2026-10-17，macOS arm64，本机 Rust 1.97.1 / Node.js 22 / pnpm 11.0.9。从提交 `04e0934` 的干净 worktree 导出 1074 个文件（清单摘要 `5ef442bc…`），在不含原仓库历史的快照目录内执行（仅为运行基于 Git 的检查而临时初始化本地仓库，未推送）：

| 检查 | 结果 |
| --- | --- |
| `pnpm install --frozen-lockfile` | 通过，376 个包，锁文件未变 |
| `pnpm test:release` / `pnpm check:release-inputs` | 20 项通过 / 通过 |
| `pnpm check`（类型检查、前端测试、生产构建） | 137 个文件 959 项通过，构建成功 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过 |
| `cargo test --workspace --lib --tests --locked` | 917 项通过，77 项按原规则忽略（需真实 Agent 账号的测试），0 失败 |

同日在加入项目订阅、协议客户端测试和方法访问分类后，从提交 `d3dc07f` 重新导出 1077 个文件（清单摘要 `136ad209…`）并复跑全部检查：发布脚本 20 项、前端 137 个文件 959 项、格式与全工作区严格 Clippy 均通过，Rust 921 项通过、77 项忽略、0 失败。

第一次运行发现提交中的 `Cargo.lock` 缺少数据库格式测试新增的开发依赖条目，导致 `--locked` 构建失败；已在 `04e0934` 补交后重新导出并全部通过。本次未在 Windows/Linux 上运行，三平台由 CI 首次运行覆盖。
