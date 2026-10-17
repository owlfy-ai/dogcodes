# GitHub 源码快照、演示截图与干净源码验证

日期：2026-10-17。对应 R2；不代表三平台安装包已完成交付。

## 已确认范围

用户选择新 GitHub 仓库只公开整理后的源码快照，现有开发历史留在阿里云仓库。保留原 origin、提交历史及工作区；不把原仓库历史推到新仓库。本轮没有创建 GitHub 仓库、提交、推送、tag 或 Release。

## 快照导出

新增 `pnpm source:snapshot`，基于工作文件与显式审阅的未跟踪路径清单导出，不复制 `.git`、本机依赖、构建输出及私有配置。工作区删除保持删除，执行位保留。拒绝未审阅的新文件、符号链接、索引冲突、填入秘密的签名模板、被强制跟踪的私有路径和未忽略的本地签名配置；复制后复核文件摘要与目录清单，发现并发变化就放弃此次新输出，不覆盖已有候选。

摘要、文件大小、删除记录与审阅清单写入旁置的本机 manifest。原仓库不执行 Git add/commit；验证环境是独立临时副本，只对 manifest 中的文件建立空历史索引进行发布输入核对。用法见 [release 说明](../../release/README.md#github-source-snapshot)。

20 项发布脚本测试通过，包括原打包配置/忽略规则用例，以及 6 项快照用例：工作文件与审阅新源文件、原索引不变、私有资料排除、未知文件拒绝、已有目录保护、符号链接拒绝、删除及执行权限保留。测试发现并修复 macOS `/var` 与 `/private/var` 父路径别名影响目标范围判断的问题。

## 七张说明页截图

七张 PNG 已使用现有 App、ReviewPanel、ProjectRunPanel、终端、Agent 菜单和用量浮层的真实渲染替换占位图片。全部使用合成项目、对话、模型、用量和终端输出；截图水印和九种语言 README 均明确说明示例数据，不将模拟结果当作真实供应商或原生平台验收。

新增只读开发入口 `readme-fixture.html` 与 `scripts/capture-readme.mjs`。独立浏览器上下文阻止外部 HTTP 请求，桥接命令只有内存查询白名单，且入口拒绝覆盖已有桌面连接。截图生成完成、七图逐张检查，无页面运行错误或业务 alert；旧占位图生成脚本已删除，防止覆盖正式截图。维护方式见 [README 维护](../README-MAINTENANCE.md)。

## 干净源码发现的构建阻断及修复

第一份导出包含 1040 个文件，不含 `.git` 和 `node_modules`。复制到独立临时目录、按冻结锁文件离线安装后，前端构建失败：Markdown 插件使用 `unified@11.0.0`，流式解析器直接依赖 `11.0.5`，`Processor/Plugin` 类型不兼容。本机旧依赖布局此前隐藏了该问题。

工作区现在通过 `overrides` 将 Markdown 依赖树统一到已有的 `unified@11.0.5`，同步直接依赖和锁文件，不通过类型断言绕过检查。采用 pnpm 文档中的[根级 overrides](https://pnpm.io/settings/dependency-resolution#overrides)；没有升级其他依赖。

第二份重新导出的源码，在另一个独立临时目录完成以下验证，全部 exit 0：

- `pnpm install --offline --frozen-lockfile --ignore-scripts`。
- `pnpm check:release-inputs`：两份模板的工作文件及独立索引均为空白，本地签名输入被忽略。
- `pnpm build`：完整 TypeScript 检查及 Vite 生产构建通过，构建约 4.68 秒；现有大 chunk 提示仍保留。
- `streamingMarkdown.test.tsx`：3 项通过，含逐字符流式解析与完整解析一致性。
- `cargo metadata --offline --no-deps --locked --format-version 1`：工作区与本地路径依赖解析通过，**没有据此宣称原生编译或测试通过**。

本机日志：`/tmp/dogcodes-source-snapshot-tests-final.log`、`/tmp/dogcodes-readme-capture.log`、`/tmp/dogcodes-clean-source-validation-v2-20261017.log`。当前快照及摘要均在忽略的 `release/private/`，最终文件清单以导出时的 manifest 为准。后续文档证据更新可重新导出；不能把已验证版本之后的业务源码改动自动视为通过。

## 干净源码的完整测试与 CI 覆盖

在同一第二份快照的独立临时源码目录继续完成一次集中验证：

- `pnpm test`：138 个文件、1004 项用例全部通过，exit 0；此前全套前端超时不再作为未解决失败。
- `cargo test --workspace --lib --tests --locked --offline`：43 份测试二进制报告合计 912 项通过、0 失败、77 项忽略，exit 0。使用全新 target，包含原生测试编译；不等于 release 配置或安装包构建通过。
- `cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` 均 exit 0；Clippy 覆盖所有工作区 target，包括 examples 和测试。
- 原生测试沿用仓库的忽略规则，没有全局启用 `--ignored`，没有调用真实供应商测试。测试数据与工作区正式应用数据隔离，没有停止现有后台服务。

CI 原来只运行 `--lib`，漏掉 `crates/*/tests` 等集成测试；现在同样运行 `--lib --tests`，真实供应商用例仍显式 opt-in。现有 Actions v7 标签已通过官方仓库核实存在，没有仅因版本较新而降级。

完整测试日志为 `/tmp/dogcodes-source-full-check-20261017.log`，格式/Clippy 日志为 `/tmp/dogcodes-source-native-quality-20261017.log`。除 CI 与文档之外，本轮没有修改已通过测试的业务源码。

## 仍需收口

用户明确素材由其后续补充，本轮不再核验素材或将其列为 Agent 待办。剩余桌面流程与资源/性能问题、发行配置原生构建及三平台安装验收仍按 CURRENT_WORK 推进。GitHub 目标仓库尚未指定，公开动作等待具体候选就绪。Windows/Apple 签名及公证配置继续留空，本地文件继续忽略，由用户最后配置。
