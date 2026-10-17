import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { expect, it } from "vitest";

it("uses shared tooltips instead of native HTML title attributes", () => {
  const root = dirname(fileURLToPath(import.meta.url));
  const violations: string[] = [];
  const scan = (directory: string) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) { scan(path); continue; }
      if (!path.endsWith(".tsx") || /\.(test|bench)\./.test(path)) continue;
      const source = ts.createSourceFile(path, readFileSync(path, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
      const visit = (node: ts.Node) => {
        if (ts.isJsxAttribute(node) && node.name.getText(source) === "title") {
          const element = node.parent.parent as ts.JsxOpeningElement | ts.JsxSelfClosingElement;
          const tag = element.tagName.getText(source);
          // Component headings and iframe accessible names are not tooltips.
          if (/^[a-z]/.test(tag) && tag !== "iframe") {
            const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
            violations.push(`${relative(root, path)}:${line + 1} <${tag}>: use data-tooltip`);
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
  };
  scan(root);
  expect(violations).toEqual([]);
});
