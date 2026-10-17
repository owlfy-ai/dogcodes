import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { keepReviewLease } from "./reviewLease";
import type { ReviewLeaseRequest } from "./reviewClient";
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });
it("uses independent reader identities for a shared snapshot and stops only the disposed reader", async () => {
  const lease = vi.fn<(request: ReviewLeaseRequest) => Promise<void>>().mockResolvedValue(undefined);
  const a = keepReviewLease(lease, "task", "shared", vi.fn());
  const b = keepReviewLease(lease, "task", "shared", vi.fn());
  const first = lease.mock.calls[0][0], second = lease.mock.calls[1][0];
  expect(first.leaseId).not.toBe(second.leaseId);
  a(); a();
  expect(lease).toHaveBeenLastCalledWith({ ...first, revision: 2, release: true });
  await vi.advanceTimersByTimeAsync(30000);
  expect(lease).toHaveBeenLastCalledWith({ ...second, revision: 2 });
  expect(lease.mock.calls.filter(([request]) => request.leaseId === first.leaseId)).toHaveLength(2);
  b();
  expect(lease).toHaveBeenLastCalledWith({ ...second, revision: 3, release: true });
});
it("does not let a late renewal failure affect a released reader or a newer successful renewal", async () => {
  let rejectFirst!: (error: Error) => void;
  const lease = vi.fn<(request: ReviewLeaseRequest) => Promise<void>>()
    .mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectFirst = reject; })).mockResolvedValue(undefined);
  const onError = vi.fn(), stop = keepReviewLease(lease, "task", "snapshot", onError);
  await vi.advanceTimersByTimeAsync(30000);
  rejectFirst(new Error("old failure")); await Promise.resolve();
  expect(onError).not.toHaveBeenCalled();
  stop();
  expect(lease.mock.calls.map(([request]) => request.revision)).toEqual([1, 2, 3]);
});
it("reports a current renewal failure and gives a newly mounted reader a new identity", async () => {
  const lease = vi.fn<(request: ReviewLeaseRequest) => Promise<void>>().mockRejectedValue(new Error("expired"));
  const onError = vi.fn(), stop = keepReviewLease(lease, "task", "snapshot", onError);
  await Promise.resolve(); expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: "expired" }));
  stop();
  lease.mockResolvedValue(undefined);
  const reopened = keepReviewLease(lease, "task", "snapshot", onError);
  expect(lease.mock.calls[0][0].leaseId).not.toBe(lease.mock.lastCall![0].leaseId);
  reopened();
});
