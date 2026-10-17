# 前端界面约定

## Loading

- 同一画面只出现一只奔跑的狗：`ActivityLoader` 只用于主区域（启动画面、对话区）的整块等待，侧边面板、菜单、行内一律不用。
- 侧边面板只用骨架屏：工作区面板用 `ReviewPanelLoading`，文件正文预览用 `ReviewFileLoading`，其他面板内整块等待用 `LoadingStatus layout="panel"`（骨架线条）。
- 菜单、按钮、子目录和行内提示用默认的 `LoadingStatus`（跳动的点加文字流光）。
- 同一区域只保留一种加载提示，不在骨架屏上再叠加动画或第二条加载文字。
- 时机：所有加载提示在等待满 300ms 后才出现，快速读取不闪烁。`LoadingStatus` 由 CSS 自动延迟；主区域和面板这类会替换内容的加载层，用 `useLoadingVisibility` 控制，出现后至少保持 400ms，期间不显示内容。按钮内的提示不延迟。
- 加载成功、失败或取消后结束 Loading，分别展示内容、错误或空状态；保留可访问的状态文案，不用动画代替错误信息。

## Tooltip

所有悬停提示统一使用 `Tooltips.tsx` 和 `tooltips.css`，即会话历史图标使用的共享组件。

- 在目标 DOM 元素上设置 `data-tooltip="提示内容"`，不要使用原生 HTML `title` 属性实现 tooltip，也不要新增独立的 tooltip 样式或实现。
- 复用组件需要接收提示文案时，使用 `tooltip` prop，并传递到目标元素的 `data-tooltip`。普通业务数据、菜单标题和文档标题仍可命名为 `title`；iframe 的可访问名称也可保留语义性的 `title`。
- 每个窗口或独立预览入口只挂载一个 `<Tooltips />`。主窗口和独立审查窗口共享 `main.tsx` 中的实例。
- 只有文本截断时才显示的提示，使用 `data-tooltip-overflow=""` 检测目标自身，或传入子元素选择器，例如 `data-tooltip-overflow=".task-title"`。
- 图标按钮仍须保留 `aria-label`，tooltip 不能替代可访问名称。共享组件处理键盘焦点、Escape、滚动关闭、顶层定位及 `aria-describedby`。
- 结构化数据面板仍使用共享 Tooltip：`data-tooltip` 为标题，`data-tooltip-rows` 为 `[标签, 值][]` 的 JSON 字符串；不传 HTML。
- 多行文案直接使用 `\n`；动态提示直接更新 `data-tooltip`。禁用控件的提示也接入同一组件。
- 更新 UI 测试和验收脚本时，优先使用角色和可访问名称定位；检查提示内容时读取 `data-tooltip` 或实际的 `[role="tooltip"]`。
- 修改提示行为后运行 `Tooltips.test.tsx`；`tooltipConvention.test.ts` 检查源码，防止重新引入原生 `title` 提示。

## Agent 选择

- Agent 的排序和启用状态统一使用 `agentPreferences.ts`，按 providerId 保存；管理页与所有选择菜单使用同一顺序。
- 菜单候选必须来自当前已安装且就绪的 Agent，不得用完整支持列表补齐候选；再用 `enabledAgents` / `useEnabledAgents` 过滤禁用项。
- 复用 `AgentPickerMenu`；默认选择和执行入口也须检查启用状态，避免历史选择绕过禁用。禁用不取消正在运行的任务。
