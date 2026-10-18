// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, SubmissionIntent, SubmissionIntentEntry, SubmissionStartRequest } from "@dogcodes/protocol";
import { PendingSubmissionError, UnconfirmedSubmissionError, submissionOperations as operations } from "./submissionOperations";
import { DemoCoreClient } from "./demoClient";

const input = { prompt: "original", model: null, thinking: null, permissionMode: null, attachments: [] };
const target = { providerId: "codex", installationId: null };
function intent(operationId: string): SubmissionIntent {
  return { schemaVersion: 1, operationId, scope: "task:tsk_test", requestedAt: "2026-10-17T00:00:00Z",
    task: { taskId: "tsk_test", projectId: "prj_test", title: "Task", preferredExecutor: target, activePrimaryRun: null, streamVersion: 3 },
    input, target, creation: null, context: null, submission: null };
}
const submitted = (operationId: string): SubmissionIntentEntry => ({ state: "closed", fingerprint: "bound",
  resolution: { status: "submitted", taskId: "tsk_test", sequence: 9, turnId: `trn_${operationId}` } });
function fixture() {
  const service = {
    submissionStart: vi.fn<(request: SubmissionStartRequest) => Promise<SubmissionIntentEntry>>(),
    submissionContinue: vi.fn<(id: string) => Promise<SubmissionIntentEntry>>(),
    submissionRetire: vi.fn<(id: string) => Promise<SubmissionIntentEntry>>(),
    submissionIntentGet: vi.fn<(id: string) => Promise<SubmissionIntentEntry | null>>(async () => null),
    submissionIntentList: vi.fn(async () => ({ operationIds: [] as string[], nextCursor: null })),
  };
  return { ...service, client: service as unknown as CoreClient };
}
const request = { scope: "task:tsk_test", taskId: "tsk_test" as const, projectId: "prj_test" as const, target, input };
beforeEach(() => localStorage.clear());

it("sends one request to the service and keeps nothing in local storage", async () => {
  const f = fixture();
  f.submissionStart.mockImplementation(async call => submitted(call.operationId));
  const result = await operations.start(f.client, request);
  expect(f.submissionStart).toHaveBeenCalledExactlyOnceWith({ ...request, operationId: expect.stringMatching(/^cmd_/) });
  expect(result).toMatchObject({ status: "submitted", taskId: "tsk_test" });
  expect(localStorage.length).toBe(0);
});

it("trusts only the service's record of this exact identity after a lost reply", async () => {
  const f = fixture();
  let id = "";
  f.submissionStart.mockImplementation(async call => { id = call.operationId; throw new Error("reply lost"); });
  f.submissionIntentGet.mockImplementation(async asked => asked === id ? submitted(id) : null);
  expect(await operations.start(f.client, request)).toMatchObject({ status: "submitted" });

  // Stored but unfinished: recovery owns the original from now on.
  f.submissionIntentGet.mockImplementation(async asked => asked === id ? { state: "pending", intent: intent(id) } : null);
  const pending = await operations.start(f.client, request).catch(cause => cause);
  expect(pending).toBeInstanceOf(PendingSubmissionError);
  expect((pending as PendingSubmissionError).intent.operationId).toBe(id);
  expect((pending as PendingSubmissionError).message).toBe("reply lost");

  // Never stored: the caller may return the content to the composer.
  f.submissionIntentGet.mockResolvedValue(null);
  const rejected = await operations.start(f.client, request).catch(cause => cause);
  expect(rejected).not.toBeInstanceOf(PendingSubmissionError);
  expect(f.submissionContinue).not.toHaveBeenCalled();
});

it("continues or retires with the original identity and lists only service-pending originals", async () => {
  const f = fixture();
  const original = intent("cmd_original");
  f.submissionContinue.mockResolvedValue(submitted("cmd_original"));
  f.submissionRetire.mockResolvedValue({ state: "closed", fingerprint: "bound", resolution: { status: "invalidated", taskId: "tsk_test" } });
  expect(await operations.resume(f.client, original)).toMatchObject({ status: "submitted" });
  expect(await operations.retire(f.client, original)).toEqual({ status: "invalidated", taskId: "tsk_test" });
  expect(f.submissionContinue).toHaveBeenCalledExactlyOnceWith("cmd_original");
  expect(f.submissionRetire).toHaveBeenCalledExactlyOnceWith("cmd_original");

  f.submissionIntentList.mockResolvedValue({ operationIds: ["cmd_original", "cmd_done"], nextCursor: null });
  f.submissionIntentGet.mockImplementation(async id => id === "cmd_original" ? { state: "pending", intent: original } : submitted(id));
  await operations.discover(f.client);
  expect(operations.list(f.client)).toEqual([original]);
});

it("rejects empty content and clients without the service workflow before sending", async () => {
  const f = fixture();
  await expect(operations.start(f.client, { ...request, input: { ...input, prompt: " " } })).rejects.toThrow("发送内容无效");
  await expect(operations.start({} as CoreClient, request)).rejects.toThrow("不支持可靠发送");
  expect(f.submissionStart).not.toHaveBeenCalled();
});

it("keeps an unknown outcome with its original identity until verified, without a second start", async () => {
  const f = fixture();
  let id = "";
  f.submissionStart.mockImplementation(async call => { id = call.operationId; throw new Error("reply lost"); });
  f.submissionIntentGet.mockRejectedValue(new Error("service restarting"));
  const unknown = await operations.start(f.client, request).catch(cause => cause);
  expect(unknown).toBeInstanceOf(UnconfirmedSubmissionError);
  expect((unknown as UnconfirmedSubmissionError).request.operationId).toBe(id);
  expect(JSON.parse(operations.unconfirmed()).records.map((r: { operationId: string }) => r.operationId)).toEqual([id]);
  // The lookup fails again: still unknown, still kept, nothing resent.
  await expect(operations.verifyUnconfirmed(f.client, (unknown as UnconfirmedSubmissionError).request)).rejects.toThrow("service restarting");
  expect(JSON.parse(operations.unconfirmed()).records).toHaveLength(1);
  // The service had in fact finished the original: verification reports it and clears the record.
  f.submissionIntentGet.mockImplementation(async asked => asked === id ? submitted(id) : null);
  expect(await operations.verifyUnconfirmed(f.client, (unknown as UnconfirmedSubmissionError).request)).toMatchObject({ status: "submitted" });
  expect(JSON.parse(operations.unconfirmed()).records).toEqual([]);
  expect(f.submissionStart).toHaveBeenCalledTimes(1);
});

it("resends an unconfirmed request only with its original identity, and drops it once absence is verified", async () => {
  const f = fixture();
  let id = "";
  f.submissionStart.mockImplementationOnce(async call => { id = call.operationId; throw new Error("reply lost"); });
  f.submissionIntentGet.mockRejectedValueOnce(new Error("offline"));
  const { request: original } = await operations.start(f.client, request).catch(cause => cause) as UnconfirmedSubmissionError;
  f.submissionIntentGet.mockResolvedValueOnce(null);
  expect(await operations.verifyUnconfirmed(f.client, original)).toEqual({ status: "not_received" });
  expect(JSON.parse(operations.unconfirmed()).records).toHaveLength(1);
  f.submissionStart.mockImplementationOnce(async call => submitted(call.operationId));
  expect(await operations.resendUnconfirmed(f.client, original)).toMatchObject({ status: "submitted" });
  expect(f.submissionStart.mock.calls[1][0].operationId).toBe(id);
  expect(JSON.parse(operations.unconfirmed()).records).toEqual([]);

  // Verified absent after a failed start: nothing is kept, so the composer may take the text back.
  f.submissionStart.mockRejectedValueOnce(new Error("rejected"));
  f.submissionIntentGet.mockResolvedValueOnce(null);
  const rejected = await operations.start(f.client, request).catch(cause => cause);
  expect(rejected).not.toBeInstanceOf(UnconfirmedSubmissionError);
  expect(JSON.parse(operations.unconfirmed()).records).toEqual([]);
});

it("sends nothing when the retry identity cannot be stored first", async () => {
  const f = fixture();
  vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => { throw new Error("quota"); });
  await expect(operations.start(f.client, request)).rejects.toThrow("quota");
  expect(f.submissionStart).not.toHaveBeenCalled();
});

it("keeps the original message and attachments when an explicit recovery retry is rejected", async () => {
  const f = fixture();
  const originalRequest = { ...request, input: { ...input, attachments: [{ name: "notes.txt", path: "/app/notes.txt", mimeType: "text/plain" }] } };
  f.submissionStart.mockRejectedValueOnce(new Error("reply lost"));
  f.submissionIntentGet.mockRejectedValueOnce(new Error("offline"));
  const failure = await operations.start(f.client, originalRequest).catch(cause => cause) as UnconfirmedSubmissionError;
  const original = failure.request;
  f.submissionIntentGet.mockResolvedValue(null);
  expect(await operations.verifyUnconfirmed(f.client, original)).toEqual({ status: "not_received" });
  f.submissionStart.mockRejectedValueOnce(new Error("conversation busy"));
  await expect(operations.resendUnconfirmed(f.client, original)).rejects.toThrow("conversation busy");
  expect(JSON.parse(operations.unconfirmed()).records).toEqual([original]);
  expect(f.submissionStart.mock.calls[1][0].operationId).toBe(original.operationId);
  expect(f.submissionIntentGet).toHaveBeenLastCalledWith(original.operationId, undefined,
    { ...originalRequest, operationId: original.operationId });
  f.submissionStart.mockImplementationOnce(async call => submitted(call.operationId));
  await expect(operations.resendUnconfirmed(f.client, original)).resolves.toMatchObject({ status: "submitted" });
  expect(f.submissionStart.mock.calls[2][0].operationId).toBe(original.operationId);
  expect(JSON.parse(operations.unconfirmed()).records).toEqual([]);
});

it("does not turn a changed request's rejection into the previous send's success", async () => {
  const client = new DemoCoreClient();
  const task = (await client.bootstrap()).tasks[0];
  const original = { ...request, operationId: "cmd_original", scope: "draft:checked", taskId: null,
    projectId: task.projectId, target: task.preferredExecutor };
  await expect(operations.start(client, original)).resolves.toMatchObject({ status: "submitted" });
  const changed = { ...original, input: { ...input, prompt: "different message" } };
  const failure = await operations.start(client, changed).catch(cause => cause);
  expect(failure).toBeInstanceOf(UnconfirmedSubmissionError);
  expect(failure.message).toContain("其他内容");
  await expect(operations.verifyUnconfirmed(client, failure.request)).rejects.toThrow("内容不一致");
  expect(JSON.parse(operations.unconfirmed()).records[0].input.prompt).toBe("different message");
});
