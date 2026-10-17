// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { InlineFileEdit } from "./InlineFileEdit";
import { parseGitDiff } from "./diffView";
import { editPatch } from "./conversationDiff";

let host: HTMLDivElement, root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });
const toggle = async (open: boolean) => act(async () => {
  const details = host.querySelector("details")!;
  details.open = open; details.dispatchEvent(new Event("toggle"));
});

it("defers highlighted diff lines until expanded and retains them on collapse", async () => {
  const path = "/workspace/src/audio.rs";
  const result = parseGitDiff(new TextEncoder().encode(editPatch(path, "pub struct Capture {\n    count: u32,\n}\n", "pub struct Capture {\n    count: u64,\n}\n")));
  if (result.kind !== "structured") throw new Error(result.reason);
  await act(async () => root.render(<InlineFileEdit path={path} file={result.diff.files[0]} />));
  expect(host.textContent).toContain("audio.rs");
  expect(host.textContent).toContain("/workspace/src/");
  expect(host.querySelector(".inline-edit-code")).toBeNull();
  expect(host.querySelector(".hljs-keyword")).toBeNull();
  await toggle(true);
  expect(host.querySelector(".inline-edit-line.deletion")).not.toBeNull();
  expect(host.querySelector(".inline-edit-line.addition")).not.toBeNull();
  expect(host.querySelector('[aria-label="原行 2"]')).not.toBeNull();
  expect(host.querySelector('[aria-label="新行 2"]')).not.toBeNull();
  expect(host.querySelector(".hljs-keyword")).not.toBeNull();
  const code = host.querySelector(".inline-edit-code");
  await toggle(false); await toggle(true);
  expect(host.querySelector(".inline-edit-code")).toBe(code);
});
it("renders source as escaped text and gives an explicit fallback for missing patches", async () => {
  await act(async () => root.render(<InlineFileEdit path="unsafe.txt" rawPatch={'<script>alert("x")</script>'} />));
  await toggle(true);
  expect(host.querySelector("script")).toBeNull();
  expect(host.querySelector("pre")?.textContent).toBe('<script>alert("x")</script>');
  await act(async () => root.render(<InlineFileEdit path="image.png" />));
  expect(host.textContent).toContain("当前操作未提供文本差异");
});
