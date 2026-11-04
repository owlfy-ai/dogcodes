// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, SubmissionIntent, SubmissionIntentEntry } from "@dogcodes/protocol";
import { useStuckSubmissionCleanup } from "./submissionCleanup";

const target = { providerId: "codex", installationId: null };
const input = { prompt: "original", model: null, thinking: null, permissionMode: null, attachments: [] };
const intent = (requestedAt: string): SubmissionIntent => ({ schemaVersion: 1, operationId: "cmd_stuck", scope: "task:tsk_test", requestedAt,
  task: { taskId: "tsk_test", projectId: "prj_test", title: "Task", preferredExecutor: target, activePrimaryRun: null, streamVersion: 3 },
  input, target, creation: null, context: null, submission: null });
const closed = (resolution: Extract<SubmissionIntentEntry, { state: "closed" }>["resolution"]): SubmissionIntentEntry => ({ state: "closed", fingerprint: "bound", resolution });
const old = () => new Date(Date.now() - 3 * 60_000).toISOString();

let root: Root, host: HTMLDivElement;
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function setup(record: SubmissionIntent, busy = false) {
  const service = {
    submissionStart: vi.fn(), submissionContinue: vi.fn(), submissionIntentGet: vi.fn(async () => null),
    submissionRetire: vi.fn<(id: string) => Promise<SubmissionIntentEntry>>(async () => closed({ status: "invalidated", taskId: "tsk_test" })),
    submissionIntentList: vi.fn(async () => ({ operationIds: [] as string[], nextCursor: null })),
  };
  const onSettled = vi.fn(), onRestore = vi.fn(), onError = vi.fn();
  function Harness({ pending }: { pending: SubmissionIntent[] }) {
    useStuckSubmissionCleanup({ client: service as unknown as CoreClient, busy, pending, unconfirmed: [], onSettled, onRestore, onError });
    return null;
  }
  return { service, onSettled, onRestore, onError, render: (pending = [record]) => act(async () => { root.render(<Harness pending={pending}/>); await new Promise(resolve => setTimeout(resolve)); }) };
}

it("stops a stuck send and returns its text to the composer", async () => {
  const f = setup(intent(old())); await f.render();
  expect(f.service.submissionRetire).toHaveBeenCalledExactlyOnceWith("cmd_stuck");
  expect(f.onRestore).toHaveBeenCalledExactlyOnceWith("original", { taskId: "tsk_test", scope: "task:tsk_test" });
  expect(f.onSettled).toHaveBeenCalledWith({ status: "invalidated", taskId: "tsk_test" }, "cmd_stuck");
  expect(f.service.submissionContinue).not.toHaveBeenCalled();
});

it("still returns the text when the record disappears before the stop's reply arrives", async () => {
  const f = setup(intent(old()));
  let finish!: (entry: SubmissionIntentEntry) => void;
  f.service.submissionRetire.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await f.render();
  await f.render([]);
  await act(async () => { finish(closed({ status: "invalidated", taskId: "tsk_test" })); await new Promise(resolve => setTimeout(resolve)); });
  expect(f.onRestore).toHaveBeenCalledOnce();
});

it("reports the result without returning the text when the send was accepted after all", async () => {
  const f = setup(intent(old()));
  f.service.submissionRetire.mockResolvedValueOnce(closed({ status: "submitted", taskId: "tsk_test", sequence: 9, turnId: "trn_1" }));
  await f.render();
  expect(f.onRestore).not.toHaveBeenCalled();
  expect(f.onSettled).toHaveBeenCalledWith(expect.objectContaining({ status: "submitted" }), "cmd_stuck");
});

it("leaves a recent send and one this window is still sending alone", async () => {
  const recent = setup(intent(new Date().toISOString())); await recent.render();
  expect(recent.service.submissionRetire).not.toHaveBeenCalled();
  const sending = setup(intent(old()), true); await sending.render();
  expect(sending.service.submissionRetire).not.toHaveBeenCalled();
});

it("reports a failed stop and keeps the send blocked", async () => {
  const f = setup(intent(old()));
  f.service.submissionRetire.mockRejectedValueOnce(new Error("service busy"));
  await f.render();
  expect(f.onError).toHaveBeenCalledOnce();
  expect(f.onRestore).not.toHaveBeenCalled();
});
