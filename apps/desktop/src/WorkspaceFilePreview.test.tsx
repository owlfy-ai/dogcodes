// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { WorkspaceFilePreview } from "./WorkspaceFilePreview";

it.each(["", " \n\t"])("identifies a loaded empty Markdown file (%j)", text => {
  const html = renderToStaticMarkup(<WorkspaceFilePreview preview={{ text, image: null }} path="README.md" />);
  expect(html).toContain("空文件"); expect(html).toContain("文件已读取");
  expect(html).not.toContain("正在读取");
});
it("keeps nonempty Markdown, empty source files and binary previews distinct", () => {
  expect(renderToStaticMarkup(<WorkspaceFilePreview preview={{ text: "# Hello", image: null }} path="README.md" />)).toContain("<h1>Hello</h1>");
  expect(renderToStaticMarkup(<WorkspaceFilePreview preview={{ text: "", image: null }} path="index.ts" />)).not.toContain("空文件");
  expect(renderToStaticMarkup(<WorkspaceFilePreview preview={{ text: null, image: null }} path="image.png" />)).not.toContain("空文件");
});
