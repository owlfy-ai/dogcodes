import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { ReviewFileMetadata } from "./ReviewFileMetadata";
import type { FileVersion } from "./reviewClient";

const file: FileVersion = { hash: "same", bytes: 0, kind: "file", executable: false, binary: false };
it("describes submodule replacement as a type transition rather than only a pointer change", () => {
  const html = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "child", kind: "modified", before: { ...file, kind: "gitlink" }, after: file }} />);
  expect(html).toContain("发生类型变化");
  expect(html).not.toContain("这里只比较子模块提交指针");
  expect(html).toContain("不修改子模块内部文件");
});
it("distinguishes empty additions and deletions from unchanged content", () => {
  for (const kind of ["added", "deleted"] as const) {
    const html = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "empty", kind, before: kind === "deleted" ? file : null, after: kind === "added" ? file : null }} />);
    expect(html).toContain("不存在");
    expect(html).toContain("0 字节");
    expect(html).toContain("空文件的新增或删除没有文本行差异");
    expect(html).not.toContain("内容未变");
  }
});
it("describes submodules as pointer-only operations", () => {
  const html = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "child", kind: "modified", before: { ...file, kind: "gitlink" }, after: { ...file, kind: "gitlink", hash: "next" } }} />);
  expect(html).not.toContain("预览子模块恢复");
  expect(html).toContain("子模块仅展示提交指针与状态");
});
it("shows executable-only and symlink type changes without claiming full permissions", () => {
  const html = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "file", kind: "modified", before: file, after: { ...file, executable: true } }} />);
  expect(html).toContain("文本文件"); expect(html).toContain("可执行");
  expect(html).toContain("内容未变"); expect(html).toContain("不包含完整系统权限");
  const link = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "link", kind: "modified", before: file, after: { ...file, kind: "symlink" } }} />);
  expect(link).toContain("符号链接"); expect(link).toContain("不读取目标文件内容");
});
it("shows both rename paths without claiming a permission-only change", () => {
  const html = renderToStaticMarkup(<ReviewFileMetadata change={{ path: "new", previousPath: "old", kind: "renamed", before: file, after: file }} />);
  expect(html).toContain("重命名文件");
  expect(html).toContain("old → new");
  expect(html).toContain("路径已变化");
  expect(html).not.toContain("仅文件类型或可执行属性变化");
});
