# 逐轮工作区净变化

2026-09-11 更新：逐轮和操作捕获已扩展到全部项目目录，多目录使用 v2 结构和 MIME，持久化起点恢复校验完整目录集。任务 Artifact 投影和读取现包含操作变更。两目录同名文件、进程强制退出恢复幂等、真实 Codex/Kimi 文件交接均通过。见 [多目录](./multi-root-projects.md) 与 [历史查询](./history-query.md)；下文 v1 描述继续适用于单目录。


逐轮 ChangeSet 用于交接和审计，比较本轮起点与终点；Review 侧栏的非 Git 比较则始终使用会话初始基线，见 [Review](review-panel.md)。两者都不把共享工作区修改归因于单个 Agent。

## 实现与边界

- Git 在私有 index/object directory 冻结起止 tree，不更新用户 index、HEAD 或分支；拒绝外部过滤器，不采集子模块内部未提交内容。
- 非 Git 使用有界起止文件快照，不执行 git init、不主动跟随符号链接；固定排除 `.git`、`node_modules`、`target`、`.venv`、`__pycache__`、`.DS_Store`。Git 损坏或不可访问时不回退到非 Git 后端。
- 调用 Agent 前持久化 `turn-checkpoint.v1+json` Artifact；Git 起点包含有界 blob 原文，不依赖临时对象库存活。结束时写 `workspace.turn_changes_captured` 与不可变 `turn-changes.v1+json`，先于轮次终态；失败记录明确错误，不伪造空 diff。
- 变更文件版本原文预算 128,000 bytes，patch 上限 512 KiB，Artifact 上限 1 MiB。非 Git 单次遍历最多 1,000 个目录项并检查 5 秒扫描预算，超限标记失败。不是原子快照或完整项目备份。
- 交接验证 Artifact 哈希，去除文件版本十六进制正文及非 UTF-8 patch，对解码字段脱敏后按预算附带文件列表和文本 patch；省略内容明确标记。

## 宿主中断后的采集

桌面宿主持有 `host.lock` 后扫描未结束轮次，校验持久起点并补采集重启时的终点，写入 `workspace_recovery_interval`，将轮次结束为 `host_interrupted`。重复启动不重复结束轮次。终点可能包含停机期间用户或其他进程的修改。

这是补齐变化记录，不执行文件系统还原、自动重放提示词或 Git 提交。此流程与已删除的子模块/整树恢复无关。

## 已有验证

确定性测试覆盖 Git index 不变、临时对象销毁后起点仍可用、非 Git 文本/二进制/删除/排除/超限，以及真实子进程异常退出后的补采集、幂等与投影重建。

2026-09-09 的 Codex→Kimi→Codex 三轮真实接力验证了正文交接与 reconciliation，禁止文件/工具线索，未验证文件编辑。入口 `crates/store/examples/live_relay.rs` 接受两种 CLI 的绝对路径，运行会消耗模型额度。另一次 Codex/Claude 实际文件修改证据见 [Review 交付审计](review-delivery-audit.md)。

完整历史检索与细粒度交付覆盖仍是独立后续工作；大项目完整备份不作为 Diff 交付要求。
