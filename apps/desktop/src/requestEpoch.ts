/** Reject late responses, including A -> B -> A and overlapping refreshes. */
export class RequestEpoch {
  private epoch = 0;

  invalidate() { this.epoch += 1; }

  begin(): () => boolean {
    const epoch = ++this.epoch;
    return () => epoch === this.epoch;
  }
}

/** Clicking the active conversation must not clear its already loaded data. */
export function changeSelection<T>(current: T, next: T, change: () => void): void {
  if (current !== next) change();
}
