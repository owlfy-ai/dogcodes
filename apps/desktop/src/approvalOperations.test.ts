// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ApprovalReceipt, BootstrapSnapshot, CoreClient, TaskSummary } from "@dogcodes/protocol";
import { approvalOperations as operations } from "./approvalOperations";
import { approvalFixture } from "./testFixtures/approval";
import { acceptedApproval, approvalRecoveryMocks } from "./testFixtures/approvalRecovery";
const task: TaskSummary = { taskId: "tsk_test", projectId: "prj_test", title: "private-title", streamVersion: 7, activePrimaryRun: null, preferredExecutor: { providerId: "codex", installationId: null } };
const answer = { cancelled: false, values: { password: "private-answer" } };
const approval = { ...approvalFixture, turnId: "trn_old" as const, requestedAtSequence: 5 };
let f: ReturnType<typeof fixture>;
function fixture() {
  const mocks = approvalRecoveryMocks();
  const respondApproval = vi.fn<CoreClient["respondApproval"]>().mockRejectedValue(new Error("offline"));
  const bootstrap = vi.fn<CoreClient["bootstrap"]>().mockResolvedValue({ tasks: [task] } as BootstrapSnapshot);
  const client = { ...mocks, respondApproval, bootstrap } as unknown as CoreClient;
  return { ...mocks, respondApproval, bootstrap, client };
}
const submit = (option = "native-once", value = answer) => operations.submit(f.client, task, approval, option, value);
const pending = () => operations.read(task.taskId)[0];
beforeEach(() => { localStorage.clear(); f = fixture(); });
afterEach(() => vi.restoreAllMocks());
it("saves only query metadata before dispatch, never the answer, option, form or task title", async () => {
  f.respondApproval.mockImplementation(async () => {
    expect(pending()).toMatchObject({ accepted: false, streamVersion: 7 });
    throw new Error("offline");
  });
  await expect(submit()).rejects.toThrow("offline");
  const saved = JSON.stringify(localStorage);
  for (const value of ["private-answer", "private-title", "native-once", "commandSummary", "question", "password"]) expect(saved).not.toContain(value);
  expect(pending().query.requestFingerprint).toMatch(/^v1:/);
});
it("whitelists host query fields and refuses to execute when persistence fails", async () => {
  const prepare = f.prepareApprovalResponse.getMockImplementation()!;
  f.prepareApprovalResponse.mockImplementation(async (...args) => ({ ...await prepare(...args), answer: "extra-private" }));
  await expect(submit()).rejects.toThrow("offline"); expect(JSON.stringify(localStorage)).not.toContain("extra-private");
  localStorage.clear(); f.respondApproval.mockClear();
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(submit()).rejects.toThrow("storage full"); expect(f.respondApproval).not.toHaveBeenCalled();
});
it("does not dispatch or persist if host identity preparation fails", async () => {
  f.prepareApprovalResponse.mockRejectedValue(new Error("offline preparation"));
  await expect(submit()).rejects.toThrow("offline preparation"); expect(operations.read(task.taskId)).toEqual([]); expect(f.respondApproval).not.toHaveBeenCalled();
});
it("recovers lost acceptance without a second decision", async () => {
  f.approvalReceipt.mockImplementation(async query => acceptedApproval(query, "sent"));
  await submit(); expect(f.respondApproval).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toEqual([]);
});
it("requires reentered answers to match and resends only the original version and ID", async () => {
  await expect(submit()).rejects.toThrow("offline"); const record = JSON.parse(JSON.stringify(pending()));
  await expect(submit("different-choice")).rejects.toThrow("与原请求不同");
  await expect(submit("native-once", { cancelled: false, values: { password: "changed" } })).rejects.toThrow("与原请求不同");
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
  await expect(operations.submit(f.client, { ...task, streamVersion: 100 }, approval, "native-once", answer)).rejects.toThrow("offline");
  expect(f.respondApproval.mock.calls).toEqual(Array(2).fill([{ taskId: task.taskId, streamVersion: 7 }, approval.approvalId, "native-once", { turnId: approval.turnId, requestedAtSequence: 5 }, answer, record.query.operationId]));
});
it("never resends accepted unknown or failed deliveries", async () => {
  await expect(submit()).rejects.toThrow();
  for (const delivery of ["unknown", "failed"] as const) {
    f.approvalReceipt.mockImplementation(async query => acceptedApproval(query, delivery));
    await submit("a different answer is ignored because the original was accepted");
    expect(pending().accepted).toBe(true);
  }
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
  operations.dismiss(pending()); expect(operations.read(task.taskId)).toEqual([]);
});
it("does not discard unresolved requests or assume missing acceptance permits a new ID", async () => {
  await expect(submit()).rejects.toThrow(); const record = pending();
  expect(() => operations.dismiss(record)).toThrow("先核实");
  expect(await operations.check(f.client, record)).toEqual({ kind: "missing" });
  expect(pending().query.operationId).toBe(record.query.operationId);
});
it("proves an old version cannot be admitted, rechecks the receipt, and requires a new explicit click", async () => {
  await expect(submit()).rejects.toThrow();
  f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
  const before = f.approvalReceipt.mock.calls.length;
  await expect(submit()).rejects.toThrow("请重新查看请求");
  expect(f.approvalReceipt.mock.calls.length - before).toBe(2);
  expect(f.respondApproval).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toEqual([]);
});
it("observes acceptance winning between the missing query and a newer task snapshot", async () => {
  await expect(submit()).rejects.toThrow(); const record = pending();
  f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
  f.approvalReceipt.mockResolvedValueOnce(null).mockResolvedValueOnce(acceptedApproval(record.query));
  expect((await operations.check(f.client, record)).kind).toBe("accepted");
  expect(pending().accepted).toBe(true); expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
it("preserves confirmed acceptance when the host cannot find its receipt", async () => {
  f.respondApproval.mockResolvedValue(); await submit(); const record = pending();
  f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
  await expect(operations.check(f.client, record)).rejects.toThrow("已确认受理");
  expect(pending()).toEqual(record); expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
it("stops on failed or mismatched receipt queries without executing again", async () => {
  await expect(submit()).rejects.toThrow(); const record = pending();
  f.approvalReceipt.mockRejectedValueOnce(new Error("network")); await expect(submit()).rejects.toThrow("network");
  f.approvalReceipt.mockResolvedValueOnce({ ...acceptedApproval(record.query), acknowledgement: { ...acceptedApproval(record.query).acknowledgement, requestFingerprint: "other" } });
  await expect(submit()).rejects.toThrow("与原请求不符"); expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
it("keeps different request events separate even with the same native ID", async () => {
  await expect(submit()).rejects.toThrow(); const first = pending();
  await expect(operations.submit(f.client, task, { ...approval, requestedAtSequence: 6 }, "native-once", answer)).rejects.toThrow();
  expect(operations.read(task.taskId)).toHaveLength(2);
  f.approvalReceipt.mockImplementation(async query => acceptedApproval(query, "sent"));
  await operations.check(f.client, first);
  expect(operations.read(task.taskId)).toHaveLength(1); expect(pending().query.requestedAtSequence).toBe(6);
});
it("late successful lookups cannot erase replacement tracking", async () => {
  await expect(submit()).rejects.toThrow(); const old = pending();
  let resolve!: (value: ApprovalReceipt) => void;
  f.approvalReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const lookup = operations.check(f.client, old);
  const key = Object.keys(localStorage).find(item => item.startsWith("dogcodes:pending-approval:"))!;
  const next = { ...old, query: { ...old.query, operationId: "cmd_new" } };
  localStorage.removeItem(key);
  localStorage.setItem(key.slice(0, -old.query.operationId.length) + next.query.operationId, JSON.stringify(next));
  resolve(acceptedApproval(old.query, "sent")); await lookup;
  expect(pending()).toEqual(next);
});
it("holds a fixed identity through concurrent submissions", async () => {
  f.respondApproval.mockResolvedValue();
  f.approvalReceipt.mockImplementation(async query => acceptedApproval(query));
  await Promise.all([submit(), submit()]); expect(f.respondApproval).toHaveBeenCalledTimes(1);
  expect(operations.read(task.taskId)).toHaveLength(1);
});

it("changes an unresolved decision only after the host invalidates its original version", async () => {
  await expect(submit()).rejects.toThrow(); const record = pending();
  const invalidate = vi.fn<NonNullable<CoreClient["invalidateApprovalResponse"]>>().mockResolvedValue({ status: "invalidated" });
  f.client.invalidateApprovalResponse = invalidate;
  expect(await operations.changeDecision(f.client, record)).toEqual({ kind: "obsolete" });
  expect(invalidate).toHaveBeenCalledExactlyOnceWith(record.query, 7);
  expect(operations.read(task.taskId)).toEqual([]); expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
it("keeps the original decision if acceptance wins or invalidation fails", async () => {
  await expect(submit()).rejects.toThrow(); const record = pending();
  const invalidate = vi.fn<NonNullable<CoreClient["invalidateApprovalResponse"]>>().mockRejectedValueOnce(new Error("offline"));
  f.client.invalidateApprovalResponse = invalidate;
  await expect(operations.changeDecision(f.client, record)).rejects.toThrow("offline"); expect(pending()).toEqual(record);
  invalidate.mockResolvedValueOnce({ status: "accepted", receipt: acceptedApproval(record.query) });
  expect((await operations.changeDecision(f.client, record)).kind).toBe("accepted");
  expect(pending().accepted).toBe(true); expect(f.respondApproval).toHaveBeenCalledTimes(1);
  await expect(operations.changeDecision(f.client, pending())).rejects.toThrow("不能撤回");
  expect(invalidate).toHaveBeenCalledTimes(2);
});

it("retains concurrent windows' identities without overwriting and requires separate resolution", async () => {
  await expect(submit()).rejects.toThrow(); const first = pending();
  const firstKey = Object.keys(localStorage).find(item => item.startsWith("dogcodes:pending-approval:"))!;
  const second = { ...first, query: { ...first.query, operationId: "cmd_other_window" } };
  localStorage.setItem(firstKey.slice(0, -first.query.operationId.length) + second.query.operationId, JSON.stringify(second));
  await expect(submit()).rejects.toThrow("多个待核实请求");
  expect(operations.read(task.taskId)).toHaveLength(2);
  f.approvalReceipt.mockImplementation(async query => acceptedApproval(query, "sent"));
  await operations.check(f.client, first);
  expect(operations.read(task.taskId)).toEqual([second]);
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
