import type { ReviewLeaseRequest } from "./reviewClient";
/** One lifetime per mounted reader/snapshot, even when reads share a snapshot. */
export function keepReviewLease(
  lease: (request: ReviewLeaseRequest) => Promise<void>, taskId: string, snapshotId: string,
  onError: (error: unknown) => void,
): () => void {
  const leaseId = crypto.randomUUID();
  let revision = 0, active = true;
  const renew = () => {
    const current = ++revision;
    void lease({ taskId, snapshotId, leaseId, revision: current, release: false }).catch(error => {
      if (active && current === revision) onError(error);
    });
  };
  renew();
  const timer = setInterval(renew, 30000);
  return () => {
    if (!active) return;
    active = false; clearInterval(timer);
    void lease({ taskId, snapshotId, leaseId, revision: ++revision, release: true }).catch(() => {});
  };
}
