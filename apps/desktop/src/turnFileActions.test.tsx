// @vitest-environment jsdom
import { act } from "react";
import { webcrypto } from "node:crypto";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CoreClient } from "@dogcodes/protocol";
import { TurnFiles } from "./TurnFiles";
import { turnHasGitRoot } from "./turnFileActions";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));

const full = (text: string) => ({ kind: "file", mode: 33188, sha256: "x", hex: Buffer.from(text).toString("hex") });
const omitted = { kind: "file", mode: 33188, sha256: "x", contentOmitted: true };
const file = (after: unknown) => ({ path: "a.txt", status: "A", before: null, after });

describe("which turn file actions apply", () => {
  it("offers Commit only for a capture of a Git repository", () => {
    expect(turnHasGitRoot({ backend: "git", files: [] })).toBe(true);
    // A plain folder, such as a conversation without a project.
    expect(turnHasGitRoot({ backend: "filesystem", files: [] })).toBe(false);
    expect(turnHasGitRoot({ files: [], roots: [{ changes: { backend: "filesystem" } }, { changes: { backend: "git" } }] })).toBe(true);
    expect(turnHasGitRoot({ files: [], roots: [{ changes: { backend: "filesystem" } }] })).toBe(false);
  });


});

let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("crypto", webcrypto);
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  vi.mocked(invoke).mockReset().mockResolvedValue(false);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

async function render(data: object) {
  const bytes = new TextEncoder().encode(JSON.stringify(data));
  const hash = Array.from(new Uint8Array(await webcrypto.subtle.digest("SHA-256", bytes)), n => n.toString(16).padStart(2, "0")).join("");
  const metadata = { artifactId: `art_${hash.slice(0, 8)}` as `art_${string}`, payloadSha256: hash, byteLength: bytes.length, mediaType: "application/json", createdAt: "2026-11-09T00:00:00Z", redactionRulesetVersion: 0, redactionCount: 0 };
  const client = { readArtifact: vi.fn(async () => ({ metadata, offset: 0, bytes: Array.from(bytes), nextOffset: null })) } as unknown as CoreClient;
  await act(async () => root.render(<TurnFiles taskId="tsk_test" metadata={metadata} client={client} />));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
}

it("keeps a Diff icon and explains unavailable undo for a recovered image", async () => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "jianying_home.png": "incomplete" } : false);
  await render({
    schemaVersion: 1, backend: "filesystem", attribution: "workspace_recovery_interval", recoveredAfterInterruption: true,
    files: [{ path: "jianying_home.png", status: "A", before: null, after: omitted, binary: true }], patch: "", workspacePaths: ["/scratch"],
  });
  expect(host.textContent).toContain("jianying_home.png");
  const diff = host.querySelector('button[aria-label="查看 jianying_home.png 的历史 Diff"]');
  expect(diff?.querySelector("svg")).not.toBeNull();
  const undo = diff?.nextElementSibling;
  expect(undo?.getAttribute("aria-disabled")).toBe("true");
  expect(undo?.getAttribute("data-tooltip")).toBe("快照不完整，无法撤销");
  expect(host.textContent).not.toContain("快照不完整，无法撤销");
  expect(host.querySelector(".turn-summary-commit")).toBeNull();
});

it("warns that a capture shared with another conversation may include its files", async () => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "a.txt": "unverified" } : false);
  await render({
    schemaVersion: 1, backend: "git", attribution: "shared_workspace_interval", sharedWorkspace: true,
    files: [{ path: "a.txt", status: "A", before: null, after: full("new") }], patch: "", workspacePaths: ["/repo"],
  });
  expect(host.querySelector("summary .turn-summary-undo:not(.turn-summary-commit)")).toBeNull();
  expect(host.querySelector(".turn-summary-commit")).not.toBeNull();
  expect(host.querySelector('[aria-label="撤销 a.txt 的修改"]')?.getAttribute("data-tooltip")).toContain("无法确认修改归属");
});

it("keeps Undo for a complete capture of this turn", async () => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "a.txt": "ready" } : false);
  await render({
    schemaVersion: 1, backend: "git", attribution: "workspace_interval",
    files: [{ path: "a.txt", status: "A", before: null, after: full("new") }], patch: "", workspacePaths: ["/repo"],
  });
  expect(host.querySelector(".turn-summary-undo:not(.turn-summary-commit)")).not.toBeNull();
});

it("offers undo only for the eligible file and explains the changed file", async () => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "a.txt": "ready", "b.txt": "changed" } : false);
  await render({ attribution: "shared_workspace_interval", files: [file(full("a")), { ...file(full("b")), path: "b.txt" }] });
  expect(host.querySelector("summary .turn-summary-undo:not(.turn-summary-commit)")).toBeNull();
  const available = host.querySelector('[aria-label="撤销 a.txt 的修改"]');
  expect(available?.textContent).toBe("");
  expect(available?.hasAttribute("aria-disabled")).toBe(false);
  const blocked = host.querySelector<HTMLButtonElement>('[aria-label="撤销 b.txt 的修改"]')!;
  expect(blocked.getAttribute("aria-disabled")).toBe("true");
  expect(blocked.getAttribute("data-tooltip")).toBe("已被外部修改，无法撤销");
  expect(host.textContent).not.toContain("已被外部修改，无法撤销");
  await act(async () => blocked.click());
  expect(document.querySelector("dialog")).toBeNull();
});

it("offers both individual actions and a whole-turn action when every shared file is verified", async () => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "a.txt": "ready", "b.txt": "ready" } : false);
  await render({ attribution: "shared_workspace_interval", sharedWorkspace: true,
    files: [file(full("a")), { ...file(full("b")), path: "b.txt" }] });
  expect(host.querySelector('[aria-label="撤销本轮全部文件修改"]')).not.toBeNull();
  expect(host.querySelectorAll(".turn-summary-file .turn-summary-undo")).toHaveLength(2);
});

const gitTurn = {
  backend: "git", attribution: "workspace_interval",
  files: [file(full("a")), { ...file(full("b")), path: "b.txt" }],
};

it("replaces Commit with the restored status after undoing the whole turn", async () => {
  let state = "ready";
  vi.mocked(invoke).mockImplementation(async cmd => {
    if (cmd === "core_turn_undo") state = "restored";
    return cmd === "core_turn_undo_files" ? { "a.txt": state, "b.txt": state } : null;
  });
  await render(gitTurn);
  expect(host.querySelector(".turn-summary-commit")).not.toBeNull();
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="撤销本轮全部文件修改"]')!.click());
  await act(async () => [...document.querySelectorAll("dialog button")].find(button => button.textContent === "确认撤销")!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  expect(host.querySelector(".turn-summary-commit")).toBeNull();
  expect(host.querySelector("summary .turn-summary-notice")?.textContent).toBe("已恢复到轮前状态");
});

it.each([
  { state: "unchanged", showsCommit: false },
  { state: "ready", showsCommit: true },
])("shows Commit on a saved turn with a restored file and a $state file only when changes remain", async ({ state, showsCommit }) => {
  vi.mocked(invoke).mockImplementation(async cmd => cmd === "core_turn_undo_files" ? { "a.txt": "restored", "b.txt": state } : null);
  await render(gitTurn);
  expect(!!host.querySelector(".turn-summary-commit")).toBe(showsCommit);
});
