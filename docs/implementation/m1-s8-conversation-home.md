# M1-S8 对话优先主界面

更新日期：2026-09-03

## 目标

桌面应用启动后直接进入类似 Codex/ZCode 的对话工作台，不再用“连接第一个项目”阻塞用户。
用户不选择项目也能输入并发起 Turn；项目成为可选上下文，而不是产品入口。

## 已实现

- 主界面改为两栏：左侧对话/项目导航，右侧对话时间线或居中的空白对话 Composer。
- 侧栏以持久化的 `project.path` 为一级分组；指向同一工作目录的多个 Project 记录会合并展示，
  每个目录下可包含任意数量的 Task/会话。
- 会话行优先显示 active Run executor，否则显示 Task `preferredExecutor.providerId` 图标；Composer 使用自定义 Agent 菜单，
  当前项和全部候选均显示同一套图标及版本/状态。时间线中的 Agent 头像也复用该组件。
- “连接项目”改为侧栏中的显式动作和模态窗口，启动流程不再打开该表单。
- Desktop Host 在数据库没有 Task 时创建应用数据目录下的 `scratch/` 以及“快速对话”项目和
  “新对话” Task。UI 将该内部项目显示为“无项目”，但 Driver 始终获得真实、可写的工作目录。
- 初始 Task 和后续“新建对话”按 `codex → claude-code → kimi → grok → 其他 Ready` 选择执行者；
  保留探测得到的精确 `installationId`。没有 Ready 外部 Agent 时才回退 `fake`。
- TS `CoreClient.createTask/setPreferredExecutor` 改为传递完整 `ExecutorRef`，避免多安装场景丢失身份。
- Composer 支持 `⌘/Ctrl + Enter` 发送；有历史时固定在底部，无历史时居中并提供快捷提示。
- 原有 Timeline、停止、Handoff/Reconciliation、Diff 和 Artifact 入口继续保留。
- `driver.protocol_warning` 与 `driver.unknown_event_observed` 只保留在 Canonical Event 中供诊断，
  不再投影为聊天消息。Timeline 查询同时过滤旧版本已写入的诊断行，所以已有数据库无需清理；
  Projection rebuild 会自然移除这些旧行。

## 边界与不变量

- `scratch/` 是应用管理的本地目录，不是假路径；不选择项目不等于允许 Driver 在未知 CWD 执行。
- 只有状态为 `Ready` 的外部安装可成为自动默认项；`Installed/NotConfigured/Broken` 不可提升。
- 自动创建只发生在数据库没有 Task 时，不改写已有 Task 的用户选择。
- 本工作包没有发送真实模型 Turn，也没有读取或迁移 Agent 凭据。

## 验证

- Desktop 单测覆盖：无安装时创建 scratch + fake；Ready Codex 时保留精确 installationId。
- Store 回归覆盖：诊断事件仍可审计、不会进入 Timeline；模拟旧数据库脏行时立即隐藏，rebuild 前后
  用户可见时间线一致。
- `pnpm check` 覆盖 TS 类型、26 个前端测试及 production build；新增同路径多 Project 合并测试。
- 本地 Vite 视觉冒烟覆盖侧栏、历史时间线、Composer 和 Agent 下拉。

## 下一接手点

1. 用全新 app-data 启动原生 Tauri，记录空白对话首页截图；现有 app-data 会保留已有历史。
2. 增加原生目录选择器，替代“连接项目”弹窗中的手输路径，但不得扩大 WebView 文件权限。
3. 增加搜索、侧栏折叠、任务重命名/归档；当前入口仅为视觉占位。
4. 若产品要把既有 fake Task 自动迁移到 Ready Agent，必须设计显式预览/确认，不能静默改写。


## 新对话草稿

“新建对话”只打开前端草稿页，不创建 Task，也不向项目分组添加空会话。选择项目仅改变草稿所属目录；创建项目只注册项目，再回到该项目的草稿。首次发送时才创建正式 Task 并归类；未选择项目则使用“无项目”工作区。创建成功但发送失败时保留该 Task 和输入内容，重试复用，避免重复会话。已有历史会话不自动删除。
