import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { expect, it } from "vitest";
import { LOCALES } from "./index";

const han = /\p{Script=Han}/u;
const src = join(dirname(fileURLToPath(import.meta.url)), "..");
// Chinese here is data, not UI copy: demo/preview content, language endonyms, sentinels
// stored by the core, and text matched against native agent or core messages.
const allowedFiles = new Set([
  "demoClient.ts", "renderingPreview.tsx", "CommitSummarySettings.tsx", "displayNames.ts",
  "agentI18n.ts", "quotaPresentation.ts", "contextEdit.ts",
]);
const allowedLiterals = new Map([["PermissionPicker.tsx", ["空闲"]]]);

it("keeps user-visible text in locale files instead of hard-coding it in components", () => {
  const violations: string[] = [];
  const scan = (directory: string) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      const name = relative(src, path).replaceAll("\\", "/");
      if (entry.isDirectory()) { if (!["i18n", "testFixtures", "assets"].includes(name)) scan(path); continue; }
      if (!/\.tsx?$/.test(path) || /\.(test|bench|fixture)\./.test(path) || path.endsWith(".d.ts") || allowedFiles.has(name)) continue;
      const source = ts.createSourceFile(path, readFileSync(path, "utf8"), ts.ScriptTarget.Latest, true, path.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
      const visit = (node: ts.Node) => {
        // Object keys that map legacy core text to translation keys are data.
        const isKey = !!node.parent && ts.isPropertyAssignment(node.parent) && node.parent.name === node;
        const text = ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node) || ts.isJsxText(node) ? node.text : null;
        if (text && han.test(text) && !isKey && !allowedLiterals.get(name)?.includes(text)) {
          const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
          violations.push(`${name}:${line + 1} ${JSON.stringify(text.trim().slice(0, 40))}`);
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
  };
  scan(src);
  expect(violations).toEqual([]);
});

it("gives every locale a non-empty string for every UI key that English has", () => {
  const leaves = (value: unknown, prefix = ""): [string, unknown][] => typeof value === "object" && value
    ? Object.entries(value).flatMap(([key, child]) => leaves(child, prefix ? `${prefix}.${key}` : key)) : [[prefix, value]];
  const english = new Map(leaves(LOCALES.en));
  for (const [lang, dictionary] of Object.entries(LOCALES)) {
    const entries = new Map(leaves(dictionary));
    for (const [key, value] of english) {
      expect(typeof entries.get(key), `${lang} ${key}`).toBe("string");
      expect((entries.get(key) as string).trim(), `${lang} ${key}`).not.toBe("");
      const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort().join();
      expect(placeholders(entries.get(key) as string), `${lang} ${key}`).toBe(placeholders(value as string));
    }
  }
});
