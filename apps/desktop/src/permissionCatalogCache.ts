import type { CoreClient, ExecutorRef, PermissionCatalog, TaskId } from "@dogcodes/protocol";

// Session-only inventory shared across tasks; each task stores its own selection.
const clients = new WeakMap<CoreClient, Map<string, { value?: PermissionCatalog; request?: Promise<PermissionCatalog> }>>();
export function permissionCatalogEntry(client: CoreClient, executor: ExecutorRef, taskId?: TaskId, version?: string | null) {
  let entries = clients.get(client);
  if (!entries) { entries = new Map(); clients.set(client, entries); }
  const key = JSON.stringify([executor.providerId, executor.installationId, version]);
  let entry = entries.get(key);
  if (!entry) {
    entry = {}; entries.set(key, entry);
  }
  const target = entry;
  return {
    peek: () => target.value,
    get: () => {
      if (target.value) return Promise.resolve(target.value);
      if (target.request) return target.request;
      const request = Promise.resolve().then(() => {
        if (!client.permissionCatalog) throw new Error("权限策略接口不可用");
        return client.permissionCatalog(executor, taskId);
      }).then(value => {
        target.value = value;
        return value;
      }).finally(() => { target.request = undefined; });
      target.request = request;
      return request;
    },
  };
}
