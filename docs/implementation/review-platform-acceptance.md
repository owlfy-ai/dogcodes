# Review 平台验收

Windows/Linux 在目标机器执行。macOS 的结果不能替代目标平台实机结果。

## 构建与测试

```sh
pnpm check
cargo test -p agent-workspace --lib
cargo test -p desktop --lib --features review-acceptance
cargo clippy -p desktop --all-targets --features review-acceptance -- -D warnings
```

## 原生隔离验收

启动前端 `pnpm dev`，另一个终端运行 `cargo run -p desktop --features review-acceptance`。数据仅写入新建的临时 fixture，路径打印到终端。

设置 `DOGCODES_REVIEW_MANUAL=1` 时只准备隔离数据，由操作者手动检查；默认运行普通 UI 验收脚本。多目录场景使用 `DOGCODES_REVIEW_MULTI_TEST=1`。真实多 Agent 的复验入口见下文，不作为每次修改的重复模型调用要求。

检查：

- Git 五种比较模式、合并提交父版本、非 Git 初始基线、多目录同名文件隔离。
- 文件树、筛选、行内/左右对比、上下文展开、刷新后的阅读位置。
- 普通文件暂存/取消暂存、丢弃/撤销、重命名配对、后续修改拒绝覆盖。
- 子模块展示和提交指针暂存；不出现整树恢复或目录恢复入口。
- 已有 Git 锁时拒绝操作，不改写或删除其他工具的锁。
- 重启后非 Git 基线和普通文件备份保持。

记录环境、退出码、fixture/报告路径以及未执行项。旧的子模块恢复、锁隔离、孤立目录和缓存操作台验收不再适用。

## 真实多 Agent 复验

前端开发服务运行后，从仓库根执行，CLI 路径替换为本机实际安装位置。需要有效登录，会调用两个 Agent 共六轮并消耗额度。

```sh
DOGCODES_REVIEW_LIVE_MIXED=1 \
DOGCODES_REVIEW_LIVE_CODEX_PATH=/absolute/path/to/codex \
DOGCODES_REVIEW_LIVE_CLAUDE_PATH=/absolute/path/to/claude \
cargo run -p desktop --features review-acceptance
```

已有产物可用 `node scripts/verify-review-multi-agent.mjs <隔离目录绝对路径> mixed` 只读复核；已提交并完成历史补验的产物使用 `history`。脚本不调用模型，也不替代原生界面验收。

历史补验要求隔离仓库已有 `review-acceptance-base` 指向原 HEAD，并已提交第一轮暂存内容、保留第三轮工作区内容。设置 `DOGCODES_REVIEW_REOPEN=<fixture目录名>` 和 `DOGCODES_REVIEW_HISTORY_REOPEN=1` 后运行上述 Cargo 命令；不要对真实用户仓库准备这些测试提交。

## 按需性能复验

在 `agent-workspace` 中运行指定 lib 测试并加 `-- --ignored --nocapture`：`review_large_repository_performance`、`review_non_git_session_performance`、`review_dense_changes_performance`、`review_mixed_files_performance`。分别覆盖 Git 大仓库小修改、非 Git 持久基线、密集修改与混合文件大小。仅在性能相关变更时执行；记录机器、冷/热缓存和耗时，不把旧机器数字当作通用承诺。
