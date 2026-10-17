const jobs = new Map<string, Promise<unknown>>();
/** Local UI coordination; durable service identities still govern acceptance. */
export function withClientOperationLock<T>(scope: string, action: () => Promise<T>): Promise<T> {
  const previous = jobs.get(scope) ?? Promise.resolve();
  const next = previous.catch(() => {}).then(async (): Promise<T> => {
    if (navigator.locks) return await navigator.locks.request(scope, action);
    return action();
  });
  jobs.set(scope, next);
  void next.finally(() => { if (jobs.get(scope) === next) jobs.delete(scope); }).catch(() => {});
  return next;
}
