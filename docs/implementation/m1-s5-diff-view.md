# 冻结 Artifact 的只读 Diff

这是已保存补丁的结构化查看器，与 [工作区 Review 侧栏](review-panel.md) 分开。输入来自 Task Artifact，完整字节 SHA-256 校验成功后才展示；不重新读取工作目录或应用补丁。Agent ReviewRun/Findings 属于另一项评审功能。

## 实现

- `apps/desktop/src/diffView.ts`：解析普通 Git unified patch、计算行号及展示预算。
- `StructuredDiffView.tsx`：文件导航、hunk、变化计数、重命名/复制、模式与二进制说明。
- `ArtifactBrowser.tsx`、`artifactView.ts`：哈希验证、原始内容切换与安全文本显示。
- 无效、不完整、非 UTF-8 或 combined diff 回退原始/hex 视图。路径只作标签，二进制 payload 不执行或应用。
- 字节预算 1 MiB，最多展示 500 文件、2,000 hunk、10,000 行；截断明确标记，统计仅覆盖已解析前缀，原始字节不变。

## 验证入口

`diffView.test.ts` 覆盖真实 Git 输出、行号、路径编码、文件类型、非法输入和预算。常规检查使用 `pnpm check`。

前端开发服务运行后打开 `/tests/artifact-review.html`：检查导航与原始切换、无效 hunk、分段读取与哈希、行数预算、非 UTF-8 回退；开启延迟后快速切换及关闭重开，确认旧响应不会恢复正文。

该页面使用内存夹具，不连接业务数据库或导出文件；原生保存交互见 [Artifact](m1-s5-artifacts.md)。
