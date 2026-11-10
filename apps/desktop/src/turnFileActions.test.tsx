// @vitest-environment jsdom
import { act } from "react";
import { webcrypto } from "node:crypto";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CoreClient } from "@dogcodes/protocol";
import { TurnFiles } from "./TurnFiles";
import { turnHasGitRoot, turnUndoAvailable } from "./turnFileActions";

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

  it("offers Undo only for a complete capture of this turn alone", () => {
    const turn = { attribution: "workspace_interval", files: [file(full("new"))] };
    expect(turnUndoAvailable(turn)).toBe(true);
    // The same refusals undo_turn makes, decided before the button is shown.
    expect(turnUndoAvailable({ ...turn, attribution: "workspace_recovery_interval", recoveredAfterInterruption: true })).toBe(false);
    expect(turnUndoAvailable({ ...turn, files: [file(omitted)] })).toBe(false);
    expect(turnUndoAvailable({ ...turn, attribution: "shared_workspace_interval", sharedWorkspace: true })).toBe(false);
    expect(turnUndoAvailable({ ...turn, files: [] })).toBe(false);
  });
});

let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("crypto", webcrypto);
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

it("shows a recovered screenshot capture in a plain folder without Commit or Undo", async () => {
  await render({
    schemaVersion: 1, backend: "filesystem", attribution: "workspace_recovery_interval", recoveredAfterInterruption: true,
    files: [{ path: "jianying_home.png", status: "A", before: null, after: omitted, binary: true }], patch: "", workspacePaths: ["/scratch"],
  });
  expect(host.textContent).toContain("jianying_home.png");
  expect(host.querySelector(".turn-summary-undo:not(.turn-summary-commit)")).toBeNull();
  expect(host.querySelector(".turn-summary-commit")).toBeNull();
});

it("warns that a capture shared with another conversation may include its files", async () => {
  await render({
    schemaVersion: 1, backend: "git", attribution: "shared_workspace_interval", sharedWorkspace: true,
    files: [{ path: "a.txt", status: "A", before: null, after: full("new") }], patch: "", workspacePaths: ["/repo"],
  });
  expect(host.querySelector(".turn-summary-undo:not(.turn-summary-commit)")).toBeNull();
  expect(host.querySelector(".turn-summary-commit")).not.toBeNull();
  expect(host.querySelector(".turn-summary-notice")?.textContent).toMatch(/其他对话|Other conversations/);
});

it("keeps Undo for a complete capture of this turn", async () => {
  await render({
    schemaVersion: 1, backend: "git", attribution: "workspace_interval",
    files: [{ path: "a.txt", status: "A", before: null, after: full("new") }], patch: "", workspacePaths: ["/repo"],
  });
  expect(host.querySelector(".turn-summary-undo:not(.turn-summary-commit)")).not.toBeNull();
});
