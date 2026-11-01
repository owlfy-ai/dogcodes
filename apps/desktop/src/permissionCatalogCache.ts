import type { CoreClient, ExecutorRef, PermissionCatalog, TaskId } from "@dogcodes/protocol";
import { translate as t } from "./i18n";
interface Entry { value?: PermissionCatalog; request?: Promise<PermissionCatalog>; dirty?: boolean; generation: number }
interface Inventory { entries: Map<string, Entry>; listeners: Set<(key?: string) => void> }
const clients = new WeakMap<CoreClient, Inventory>();
function inventory(client: CoreClient) {
  let state = clients.get(client);
  if (!state) { state = { entries: new Map(), listeners: new Set() }; clients.set(client, state); }
  return state;
}
export function invalidatePermissionCatalogs(client: CoreClient) {
  const state = inventory(client);
  state.entries.forEach(entry => { entry.dirty = true; entry.request = undefined; entry.generation++; });
  state.listeners.forEach(listener => listener());
}
export function subscribePermissionCatalog(client: CoreClient, listener: (key?: string) => void) {
  const state = inventory(client); state.listeners.add(listener);
  return () => { state.listeners.delete(listener); };
}
export function permissionCatalogEntry(client: CoreClient, executor: ExecutorRef, _taskId?: TaskId, version?: string | null) {
  const state = inventory(client);
  const key = JSON.stringify([executor.providerId, executor.installationId, version]);
  let entry = state.entries.get(key);
  if (!entry) { entry = { generation: 0 }; state.entries.set(key, entry); }
  const target = entry;
  return { key, peek: () => target.value, isDirty: () => !!target.dirty, needsRead: () => !target.value || !!target.dirty || !!target.request,
    get: (refresh = false) => {
      if (target.request) return target.request;
      if (!refresh && target.value && !target.dirty) return Promise.resolve(target.value);
      const generation = target.generation;
      const request = Promise.resolve().then(() => {
        if (!client.permissionCatalog) throw new Error(t("permissionPicker.apiUnavailable"));
        return refresh ? client.permissionCatalog(executor, undefined, true) : client.permissionCatalog(executor, undefined);
      }).then(value => {
        if (target.generation === generation) {
          target.value = value; target.dirty = false;
          state.listeners.forEach(listener => listener(key));
        }
        return value;
      }).finally(() => { if (target.request === request) target.request = undefined; });
      target.request = request; return request;
    },
  };
}
