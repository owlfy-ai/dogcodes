# 多目录项目实施

## 2026-09-11 逐轮采集补齐

所有项目目录现在参与逐轮起点、净变化、操作快照和边界采集。多目录 Artifact schemaVersion=2，files 保留 workspacePath、relativePath 和绝对 path，roots 保留分目录 patch；同名文件不会混淆。单目录 v1 继续可读。多目录 MIME 使用 v2，操作变更和逐轮变更均可在当前任务读取。

多目录 checkpoint 记录完整有序目录集；启动恢复校验当前目录集与起点一致后逐目录比较，明确归因为 workspace_recovery_interval。任一目录失败或整体超过 1 MiB 时记录失败，不发布假完整结果；这不是全项目备份。非 Git 不执行 git init。旧 v1 checkpoint 只恢复当时记录的主目录，不补造附加目录证据。

自动化已覆盖真实两目录同名文件、净 patch、终态前落盘及进程强制退出后的双目录恢复和重复启动幂等。Codex/Kimi 真实双目录单次审批与 A→B→A 文件接力已通过；原生目录选择、全供应商取消/恢复矩阵仍独立保留。


更新：2026-09-09。用户明确：开发阶段，不要求旧版本数据兼容；不得擅自删除本机数据。

## 范围

- 创建项目弹窗：必填名称、多个文件夹、删除目录、指定主目录；只保存本地引用。
- 项目由 Project ID 区分，不按主目录合并；会话仍从属于一个项目。
- path 为主目录，additionalDirectories 为附加目录，持久化于注册事件及项目投影。
- 创建/恢复原生会话传完整目录列表。原生目录不是免审批授权。
- ACP 按实际 initialize 的 additionalDirectories 能力协商；Kimi 支持，Grok 当前未声明，后者不发送该字段。
- Claude：--add-dir；Antigravity：主目录 cwd + 独立 --new-project + 全部 --add-dir；恢复沿用 conversation 所属项目。
- Codex：turn/start 的 workspaceWrite.writableRoots，保持原审批策略，不开放网络。
- Grok、ZCode、未声明能力的 ACP 使用主目录 + JSON 路径提示。提示明确访问失败必须报告，不能绕过权限。

## 当前边界

- 目录清单暂随每轮请求注入，保证恢复、切换及上下文压缩后仍能获得准确范围；不会写入用户消息显示。后续可按原生会话确认及目录摘要去重。
- 创建后暂不提供目录编辑；切换 Agent 不改写项目目录。
- Review 侧栏已支持多目录比较，见 [当前范围](review-panel.md)。逐轮变更交付采集仍以主目录为准。
- 新数据库 schema 直接包含目录数组，不新增旧版本迁移。桌面改用 `app-v5.db`，旧 `app.db` 不读取、不删除；重启后开发项目和会话列表重新开始。原生 Agent 登录配置不变。

## 验收清单

- [x] 前端：创建/取消/删除/主目录选择/重复目录；同主目录不同项目不合并；全前端 91 项测试通过。
- [x] 后端：规范化与去重、拒绝不存在/相对目录、存储与投影回放断言；`cargo test --workspace --lib` 通过（真实测试保持 ignored）。
- [x] ACP 有/无 capability 的 new/load 序列化断言，Antigravity 新建/恢复 argv 断言。
- [x] Codex 进程级 fixture：两目录 writableRoots、cwd、网络策略与审批拒绝后的流式终态；product_driver 两项通过。
- [x] `cargo check --workspace --all-targets`、前端 typecheck 和生产 build 通过。
- [x] 浏览器 Demo 浅色弹窗视觉/创建验证：两目录项目出现在侧栏，聊天框显示 2 个目录；无模型请求。临时 1421 服务已停止。
- [ ] 全供应商两个目录取消/恢复矩阵；Codex/Kimi 文件接力单列实际验收结果。
- [ ] 桌面浅深主题布局与文件夹选择。
- [ ] Claude 多目录专门 argv/恢复 fixture（现有单目录 driver tests 通过）。

官方依据：
- https://agentclientprotocol.com/rfds/additional-directories
- https://antigravity.google/docs/cli/projects
- https://learn.chatgpt.com/docs/app-server （按 OpenAI Docs 核对 turn/start sandboxPolicy，不套用 CLI 参数）
